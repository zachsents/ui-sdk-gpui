//! Port of `components/file-input/file-input-dropzone.ts`: a file input's
//! drop zones: files dropped on one, or anywhere in the window with a
//! `Document` target, and files pasted while focus is inside one or in its
//! `paste_target`. A zone isn't a control: the trigger is the way in for
//! keyboards and screen readers.
//!
//! gpui finds a drag of files over an element as it paints the element, the
//! way it applies `drag_over` styles, by hit testing, children and all, and
//! hands a drop to the innermost element under the pointer that takes one,
//! which stops it there. So a zone is dragged over while a frame finds it
//! under the pointer, and of nested zones the inner one takes what lands on
//! it, as the original's nearest zone does. A window-wide zone hears every
//! move of a drag of files in the window, and takes a drop once the zones
//! under the pointer have had it. A paste is the platform's paste keys, or
//! the text field's `Paste` action, which an app's Edit menu or key bindings
//! send.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    App, Context, Entity, ExternalPaths, FileDropEvent, InteractiveElement, KeyDownEvent,
    Keystroke, MouseButton, MouseUpEvent, Pixels, Point, SharedString, Subscription, Window,
};
use ui_sdk_forms::drag_status;
use ui_sdk_kernel::modality::window_source;
use ui_sdk_kernel::{ChangeOptions, InputEvent, Part, Reason, held};
use ui_sdk_native::Paste;
use ui_sdk_platform::is_apple;

use crate::file_drop::{
    DragTracker, DroppedFiles, claim, dragged_types, dropped_files, is_claimed, pasted,
    pasted_files,
};
use crate::file_input::{
    FileInput, FileInputDropzoneProps, FileInputDropzoneState, FileInputDropzoneTarget,
    FileInputPasteTarget,
};

/// One drop zone, as its frames see it.
#[derive(Default)]
pub(crate) struct Zone {
    // The props it last rendered with, which a paste reads when it happens.
    props: RefCell<FileInputDropzoneProps>,
    // The files a drag over the zone carried as this frame painted it.
    over: RefCell<Option<ExternalPaths>>,
    // The files a drag in the window carries, for a window-wide zone, until
    // the drag ends.
    window: RefCell<Option<ExternalPaths>>,
    tracker: Cell<DragTracker>,
}

/// The platform's paste keys, as a text field pastes with them: ⌘V on a
/// Mac, Control-V or Shift-Insert elsewhere.
pub(crate) fn is_paste(keystroke: &Keystroke, cx: &App) -> bool {
    let modifiers = keystroke.modifiers;
    let key = keystroke.key.as_str();
    if is_apple(cx) {
        return key == "v" && modifiers.platform && !modifiers.control;
    }
    !modifiers.platform
        && !modifiers.alt
        && match key {
            "v" => modifiers.control,
            "insert" => modifiers.shift && !modifiers.control,
            _ => false,
        }
}

/// Puts a drop zone's behavior on its part. Its props are read when each
/// drag, drop or paste happens, so new ones apply without binding again.
pub(crate) fn bind_dropzone(
    part: Part<FileInputDropzoneState>,
    this: &Entity<FileInput>,
    key: SharedString,
    props: FileInputDropzoneProps,
    cx: &App,
) -> Part<FileInputDropzoneState> {
    let input = this.read(cx);
    let zone = input.zone(&key);
    let document = props.target == FileInputDropzoneTarget::Document;
    *zone.props.borrow_mut() = props;
    zone.over.replace(None);

    // Files over the zone, as this frame paints it. Text and elements dragged
    // within the app are other drags, never these.
    let painted = zone.clone();
    let part = part.drag_over::<ExternalPaths>(move |style, paths, _window, _cx| {
        *painted.over.borrow_mut() = Some(paths.clone());
        style
    });

    // A window-wide zone follows every move of a drag of files in the window.
    let part = if document {
        let moved = zone.clone();
        part.on_drag_move::<ExternalPaths>(move |event, _window, cx| {
            let paths = event.drag(cx);
            if moved.window.borrow().as_ref() != Some(paths) {
                *moved.window.borrow_mut() = Some(paths.clone());
            }
        })
    } else {
        part
    };

    // Once the frame is drawn: whether files are over the zone, or over the
    // window for a window-wide one. A drop anywhere ends the drag, even one
    // another zone took, and so does the drag leaving the window.
    let settled = zone.clone();
    let entity = this.downgrade();
    let moved_key = key.clone();
    let part = part.on_props(move |_window, cx| {
        let paths = if document {
            if !cx.has_active_drag() {
                settled.window.replace(None);
            }
            settled.window.borrow().clone()
        } else {
            settled.over.borrow().clone()
        };
        let mut tracker = settled.tracker.get();
        if tracker.moved(paths.is_some()).is_none() {
            return;
        }
        settled.tracker.set(tracker);
        entity
            .update(cx, |input, cx| input.drag_moved(&moved_key, paths.as_ref(), cx))
            .ok();
    });

    let part = if document {
        // A window-wide zone takes what no other zone did, once the zones
        // under the pointer have had the drop.
        let listener = {
            let zone = zone.clone();
            let entity = this.downgrade();
            Rc::new(move |_event: &MouseUpEvent, window: &mut Window, cx: &mut App| {
                let Some(paths) = zone.window.borrow().clone() else {
                    return;
                };
                let position = window.mouse_position();
                let entity = entity.clone();
                window.defer(cx, move |window, cx| {
                    if is_claimed() {
                        return;
                    }
                    claim(cx);
                    entity
                        .update(cx, |input, cx| input.take_drop(&paths, position, window, cx))
                        .ok();
                });
            })
        };
        let inside = listener.clone();
        part.capture_any_mouse_up(move |event, window, cx| inside(event, window, cx))
            .on_mouse_up_out(MouseButton::Left, move |event, window, cx| listener(event, window, cx))
    } else {
        // gpui hands the drop to the innermost zone under the pointer and
        // stops it there, so nothing past the zone takes it. A disabled zone
        // takes it too, and does nothing with it.
        let dropped = this.downgrade();
        part.on_drop::<ExternalPaths>(move |paths, window, cx| {
            claim(cx);
            let position = window.mouse_position();
            dropped
                .update(cx, |input, cx| input.take_drop(paths, position, window, cx))
                .ok();
        })
    };

    // Pastes while focus is inside the zone: the paste keys, or the text
    // field's `Paste` action. Only one with files takes them, and keeps the
    // text field inside from pasting their names.
    let keyed = this.downgrade();
    let acted = this.downgrade();
    part.capture_key_down(move |event: &KeyDownEvent, window, cx| {
        if !is_paste(&event.keystroke, cx) {
            return;
        }
        let event = InputEvent::KeyDown(event.clone());
        let took = keyed
            .update(cx, |input, cx| input.paste(Some(event), window, cx))
            .unwrap_or(false);
        if took {
            window.prevent_default();
            cx.stop_propagation();
        }
    })
    .capture_action(move |_: &Paste, window, cx| {
        let took = acted
            .update(cx, |input, cx| input.paste(None, window, cx))
            .unwrap_or(false);
        if took {
            cx.stop_propagation();
        }
    })
}

/// Pastes into each zone's `paste_target`, read when the paste happens, so
/// the target can render after the zone. They come ahead of everything else
/// in the window, as the original's capture on the document does.
pub(crate) fn paste_targets(window: &Window, cx: &mut Context<FileInput>) -> Subscription {
    let entity = cx.weak_entity();
    let handle = window.window_handle();
    cx.intercept_keystrokes(move |event, window, cx| {
        if window.window_handle() != handle || !is_paste(&event.keystroke, cx) {
            return;
        }
        let keystroke = event.keystroke.clone();
        let took = entity
            .update(cx, |input, cx| input.paste_from_targets(keystroke, window, cx))
            .unwrap_or(false);
        if took {
            window.prevent_default();
            cx.stop_propagation();
        }
    })
}

impl FileInput {
    // The zone with `key`, made the first time it renders.
    pub(crate) fn zone(&self, key: &SharedString) -> Rc<Zone> {
        let mut zones = self.zones.borrow_mut();
        if let Some((_, zone)) = zones.iter().find(|(each, _)| each == key) {
            return zone.clone();
        }
        let zone = Rc::new(Zone::default());
        zones.push((key.clone(), zone.clone()));
        zone
    }

    // Files came over a zone, or the window for a window-wide one, or left.
    // How they look is judged once, as they come: from their types and
    // count, which is what a drag shows before the drop.
    pub(crate) fn drag_moved(
        &mut self,
        key: &SharedString,
        paths: Option<&ExternalPaths>,
        cx: &mut Context<Self>,
    ) {
        let status = paths
            .filter(|_| !self.is_disabled())
            .map(|paths| drag_status(&dragged_types(paths), &self.rules()));
        self.set_drag(key, status, cx);
    }

    // Zones that left the page take their drags with them.
    pub(crate) fn prune_zones(&mut self, cx: &mut Context<Self>) {
        let instance = &self.instance;
        self.zones
            .borrow_mut()
            .retain(|(key, _)| held(instance, "dropzone", Some(key)));
        let before = self.drags.len();
        self.drags.retain(|key, _| held(instance, "dropzone", Some(key)));
        if self.drags.len() != before {
            cx.notify();
        }
    }

    // Files dropped on a zone, or anywhere for a window-wide one.
    pub(crate) fn take_drop(
        &mut self,
        paths: &ExternalPaths,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_disabled() {
            return;
        }
        let event = InputEvent::FileDrop(FileDropEvent::Submit { position });
        let files = dropped_files(paths, cx);
        self.take(files, ChangeOptions::event_of(Reason::Drop, event), window, cx);
    }

    // The files a drop or a paste brings, right away or once they're read.
    fn take(&mut self, files: DroppedFiles, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) {
        match files {
            DroppedFiles::Ready(files) => {
                self.receive(files, options, window, cx);
            }
            DroppedFiles::Reading(read) => {
                let id = self.next_read;
                self.next_read += 1;
                let task = cx.spawn_in(window, async move |this, cx| {
                    let files = read.await;
                    this.update_in(cx, |this, window, cx| {
                        this.reading.remove(&id);
                        this.receive(files, options, window, cx);
                    })
                    .ok();
                });
                self.reading.insert(id, task);
            }
        }
    }

    // A paste reaches the zone and its paste target, and nested zones, but
    // only the first takes it, and only when it carries files.
    pub(crate) fn paste(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.is_disabled() || is_claimed() {
            return false;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let files = pasted_files(&item);
        if files.is_empty() {
            return false;
        }
        claim(cx);
        let options = match event {
            Some(event) => ChangeOptions::event_of(Reason::Paste, event),
            // An action from a menu or a binding has no event: how the
            // window was used last says.
            None => ChangeOptions::reason(Reason::Paste).source(window_source(window)),
        };
        let files = pasted(files, cx);
        self.take(files, options, window, cx);
        true
    }

    // A paste keystroke anywhere in the window, for the zones whose paste
    // target holds focus now.
    pub(crate) fn paste_from_targets(
        &mut self,
        keystroke: Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let targets: Vec<FileInputPasteTarget> = self
            .zones
            .borrow()
            .iter()
            .filter(|(key, _)| held(&self.instance, "dropzone", Some(key)))
            .filter_map(|(_, zone)| zone.props.borrow().paste_target.clone())
            .collect();
        for target in targets {
            let inside = match &target {
                FileInputPasteTarget::Document => true,
                FileInputPasteTarget::Element(handle) => handle.contains_focused(window, cx),
                FileInputPasteTarget::Function(target) => {
                    target(cx).is_some_and(|handle| handle.contains_focused(window, cx))
                }
            };
            if inside {
                let event = InputEvent::KeyDown(KeyDownEvent {
                    keystroke,
                    is_held: false,
                    prefer_character_input: false,
                });
                return self.paste(Some(event), window, cx);
            }
        }
        false
    }
}
