//! Port of `foundation/popups/sheet-presentation.ts`: a popup presented as a
//! sheet: [`attach_sheet`] on the content, inside the window-sized viewport
//! the component renders around it. The popup keeps its role, keyboard
//! model, focus and dismissal; the sheet adds the motion, a swipe to dismiss
//! and the dim, which the viewport paints unless the popup has a backdrop
//! part.
//!
//! The sheet attaches as the popup shows and lets go once it has hidden, so
//! a list kept in the page while closed, like a select's, is a plain hidden
//! element, never a window-wide layer.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Context, Window};
use ui_sdk_kernel::Reason;
use ui_sdk_motion::{SheetSnapReason, SnapPoint, timestamp};
use ui_sdk_platform::{Layer, Region};

use crate::popup::{AttachConfig, PopupLayer, PopupOpener, ShowCleanup};
use crate::presentation::SheetConfig;
use crate::sheet::{Sheet, SheetOptions, attach_sheet, parent_sheet, register_sheet_layer};

thread_local! {
    // Parents a popup drilled into, with the content shown in their place
    // and the trigger kept above it.
    static DRILLED: RefCell<Vec<(Region, Region, Option<Region>)>> = const { RefCell::new(Vec::new()) };
    // Drilled-in contents hidden as they close.
    static DRILL_HIDDEN: RefCell<Vec<Region>> = const { RefCell::new(Vec::new()) };
}

/// What a popup's content renders in place of its own items while a popup
/// inside it is drilled in, like a submenu in a sheet: the content shown in
/// its place and the trigger kept above it. A component whose popup can
/// present as a sheet checks it for its content, and renders only those two
/// while it's set, as the original hides every other element.
pub fn drilled_into(parent: &Region) -> Option<(Region, Option<Region>)> {
    DRILLED.with(|drilled| {
        drilled
            .borrow()
            .iter()
            .find(|(own, _, _)| own == parent)
            .map(|(_, child, trigger)| (child.clone(), trigger.clone()))
    })
}

/// Whether a drilled-in content hides as it closes, so its exit never shows
/// in place.
pub fn is_drill_hidden(content: &Region) -> bool {
    DRILL_HIDDEN.with(|hidden| hidden.borrow().contains(content))
}

fn set_drill_hidden(content: &Region, hidden_now: bool) {
    DRILL_HIDDEN.with(|hidden| {
        let mut hidden = hidden.borrow_mut();
        hidden.retain(|other| other != content);
        if hidden_now {
            hidden.push(content.clone());
        }
    });
}

fn drill_in<T: 'static>(this: &mut T, layer: &PopupLayer<T>, el: Region, config: SheetConfig<T>, window: &mut Window, cx: &mut Context<T>) {
    let Some(drill) = config.drill_in.clone() else {
        return;
    };
    let shown_el = el.clone();
    let closed_el = el.clone();
    let attach = AttachConfig {
        position: None,
        inline: true,
        on_show: Some(Rc::new(move |_this: &mut T, _layer: &Layer, _window: &mut Window, _cx: &mut Context<T>| {
            set_drill_hidden(&shown_el, false);
            let (parent, trigger) = drill();
            let parent = parent?;
            DRILLED.with(|drilled| {
                let mut drilled = drilled.borrow_mut();
                drilled.retain(|(own, _, _)| *own != parent);
                drilled.push((parent.clone(), shown_el.clone(), trigger));
            });
            Some(Box::new(move |_this: &mut T, _window: &mut Window, _cx: &mut Context<T>| {
                DRILLED.with(|drilled| drilled.borrow_mut().retain(|(own, _, _)| *own != parent));
            }) as ShowCleanup<T>)
        })),
        on_close: Some(Rc::new(move |_this: &mut T, _window: &mut Window, cx: &mut Context<T>| {
            DRILLED.with(|drilled| drilled.borrow_mut().retain(|(_, child, _)| *child != closed_el));
            set_drill_hidden(&closed_el, true);
            cx.notify();
        })),
        ..config.attach
    };
    layer.bind(this, el, attach, window, cx);
}

/// Presents a popup's content as a sheet in place of the layer's own `bind`
/// (`presentSheet`): its host is the config's, the viewport around the
/// content.
pub fn present_sheet<T: 'static>(
    this: &mut T,
    layer: &PopupLayer<T>,
    el: Region,
    config: SheetConfig<T>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    if config.drill_in.is_some() {
        return drill_in(this, layer, el, config, window, cx);
    }
    let Some(host) = config.attach.host.as_ref().and_then(|host| host()) else {
        return layer.bind(this, el, AttachConfig { position: None, ..config.attach }, window, cx);
    };
    let sheet: Rc<RefCell<Option<Sheet<T>>>> = Rc::new(RefCell::new(None));
    let shown = sheet.clone();
    let closing = sheet.clone();
    let popup = layer.clone();
    let show_el = el.clone();
    let show_config = config.clone();
    let attach = AttachConfig {
        position: None,
        host: Some(Rc::new(move || Some(host.clone()))),
        on_show: Some(Rc::new(move |this: &mut T, layer: &Layer, window: &mut Window, cx: &mut Context<T>| {
            let (current, cleanup) = show_sheet(this, show_el.clone(), &popup, show_config.clone(), layer, window, cx);
            *shown.borrow_mut() = Some(current);
            let slot = shown.clone();
            Some(Box::new(move |this: &mut T, window: &mut Window, cx: &mut Context<T>| {
                cleanup(this, window, cx);
                *slot.borrow_mut() = None;
            }) as ShowCleanup<T>)
        })),
        on_close: Some(Rc::new({
            let presence_layer = layer.clone();
            move |this: &mut T, window: &mut Window, cx: &mut Context<T>| {
                let Some(sheet) = closing.borrow().clone() else {
                    return;
                };
                sheet.close(this, window, cx);
                // The exit waits for the sheet to settle closed.
                if let Some(remaining) = sheet.motion_remaining() {
                    presence_layer.presence().track_animation(remaining);
                }
            }
        })),
        ..config.attach
    };
    layer.bind(this, el, attach, window, cx);
}

// Attaches the sheet as the popup shows, and returns it with what lets it go.
fn show_sheet<T: 'static>(
    this: &mut T,
    el: Region,
    popup: &PopupLayer<T>,
    config: SheetConfig<T>,
    layer: &Layer,
    window: &mut Window,
    cx: &mut Context<T>,
) -> (Sheet<T>, ShowCleanup<T>) {
    let rest: Rc<RefCell<Option<SnapPoint>>> = Rc::new(RefCell::new(None));
    let stacked_on = parent_sheet(layer);
    let dims_host = config.attach.backdrop.as_ref().and_then(|backdrop| backdrop()).is_none();
    let host = config
        .attach
        .host
        .as_ref()
        .and_then(|host| host())
        .unwrap_or_default();
    let sheet_cell: Rc<RefCell<Option<Sheet<T>>>> = Rc::new(RefCell::new(None));
    let snap_points = config.snap_points.clone();
    let snapped = rest.clone();
    let resting = rest.clone();
    let dismissing = popup.clone();
    let dismiss_layer = layer.clone();
    let dismiss_sheet = sheet_cell.clone();
    let dismiss_rest = rest.clone();
    let attempt = config.on_dismiss_attempt.clone();
    let mut options = SheetOptions::new(host, move |this: &mut T, event, window, cx| {
        dismissing.request_close(this, Reason::Swipe, event.cloned(), window, cx);
        // A refused swipe goes back to where it rested.
        let layer = dismiss_layer.clone();
        let sheet = dismiss_sheet.clone();
        let rest = dismiss_rest.clone();
        let entity = dismissing.entity();
        window.on_next_frame(move |window, cx| {
            let Some(sheet) = sheet.borrow().clone() else {
                return;
            };
            let point = rest
                .borrow()
                .or_else(|| sheet.snap_points().first().map(|point| point.value));
            if let Some(point) = point
                && layer.is_active()
            {
                entity
                    .update(cx, |this, cx| sheet.snap_to(this, &point, SheetSnapReason::Imperative, window, cx))
                    .ok();
            }
        });
    });
    options.snap_points = Some(Rc::new(move || snap_points.clone().unwrap_or_else(|| vec![SnapPoint::Content])));
    options.snap_point = Some(Rc::new(move || *resting.borrow()));
    options.backdrop = config.attach.backdrop.clone();
    options.host_backdrop = dims_host;
    options.dismissible = config.dismissible.clone();
    options.on_dismiss_attempt = attempt.map(|attempt| {
        Rc::new(move |_this: &mut T, event: Option<&ui_sdk_kernel::InputEvent>, window: &mut Window, cx: &mut Context<T>| {
            attempt(event, window, cx)
        }) as Rc<dyn Fn(&mut T, Option<&ui_sdk_kernel::InputEvent>, &mut Window, &mut Context<T>)>
    });
    options.parent = Some(Rc::new(move || stacked_on.clone()));
    options.on_snap = Some(Rc::new(move |_this: &mut T, point: SnapPoint, _reason, _event, _window, _cx| {
        *snapped.borrow_mut() = Some(point);
    }));
    let sheet = attach_sheet(el, options, cx);
    *sheet_cell.borrow_mut() = Some(sheet.clone());
    let registered = register_sheet_layer(layer, sheet.stack());
    if let Some(on_sheet) = &config.on_sheet {
        on_sheet(Some(sheet.clone()));
    }
    if layer.is_active() {
        sheet.open(this, popup.opener() != PopupOpener::Initial, window, cx);
    }
    let destroyed = sheet.clone();
    let on_sheet = config.on_sheet.clone();
    let cleanup: ShowCleanup<T> = Box::new(move |this: &mut T, window: &mut Window, cx: &mut Context<T>| {
        drop(registered);
        destroyed.destroy(this, window, cx);
        if let Some(on_sheet) = on_sheet {
            on_sheet(None);
        }
    });
    (sheet, cleanup)
}

impl<T: 'static> Sheet<T> {
    /// How long the settle in progress has left, which a popup's exit waits
    /// for.
    pub fn motion_remaining(&self) -> Option<Duration> {
        let (start, duration) = self.motion_window()?;
        let left = start + duration - timestamp();
        (left > 0.0).then(|| Duration::from_secs_f64(left / 1000.0))
    }
}
