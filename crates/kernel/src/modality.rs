//! Port of `foundation/kernel/modality.ts`. gpui tracks the window's input
//! modality itself, so this reads it instead of listening on a document, and
//! adds what gpui doesn't know: focus the kernel moved, and focus the window
//! got back when the user returned to it.

use std::collections::HashMap;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Global, Keystroke, Pixels, Point, Subscription,
    Window, WindowId,
};

use super::details::{ChangeSource, InputEvent};

/// How the window's last input came: a key, a touch or a mouse.
pub fn window_source(window: &Window) -> ChangeSource {
    if window.last_input_was_keyboard() {
        ChangeSource::Keyboard
    } else if window.last_input_was_touch() {
        ChangeSource::Touch
    } else {
        ChangeSource::Mouse
    }
}

/// Whether focus should show a ring now, `:focus-visible` in the original.
pub fn focus_visible(window: &Window) -> bool {
    window.last_input_was_keyboard()
}

/// A focus event, with the source the window's modality gives it.
pub fn focus_event(window: &Window) -> InputEvent {
    InputEvent::Focus(window_source(window))
}

/// How an event came about: a mouse, a finger or the keyboard, or assistive
/// technology's click, `Virtual`. Without an event, how the user is working
/// now, by their last key or pointer press: `source_of(None, window) ==
/// ChangeSource::Keyboard` while they use the keyboard (`sourceOf`). Every
/// gpui event is the user's, so none reads as `Programmatic`.
pub fn source_of(event: Option<&InputEvent>, window: &Window) -> ChangeSource {
    match event {
        Some(event) => event.source(),
        None => window_source(window),
    }
}

// What the kernel tracks per window beside gpui's modality.
#[derive(Default)]
struct Tracking {
    windows: HashMap<WindowId, WindowInput>,
    keystrokes: Option<Subscription>,
}

impl Global for Tracking {}

#[derive(Default)]
struct WindowInput {
    // Whether the window lost focus since the last key press. gpui focuses
    // the same element again when the user comes back, and that isn't
    // keyboard use either.
    window_left: bool,
    // Focus `move_focus` or `restore_focus` moved, until its focus events
    // have run.
    moved: Option<FocusHandle>,
    tracker: Option<Entity<ActivationTracker>>,
}

struct ActivationTracker {
    _activation: Subscription,
}

/// Starts tracking input in `window`, so later focus can tell how it came
/// (`trackInput`). Safe to call often. A component that asks
/// `is_focus_from_keyboard` calls it as it's made.
pub fn track_input(window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    if cx
        .try_global::<Tracking>()
        .is_some_and(|tracking| tracking.windows.get(&id).is_some_and(|input| input.tracker.is_some()))
    {
        return;
    }
    let tracker = cx.new(|cx: &mut Context<ActivationTracker>| ActivationTracker {
        _activation: cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() {
                let id = window.window_handle().window_id();
                cx.default_global::<Tracking>()
                    .windows
                    .entry(id)
                    .or_default()
                    .window_left = true;
            }
        }),
    });
    let keystrokes = cx.try_global::<Tracking>().is_none_or(|tracking| tracking.keystrokes.is_none());
    let subscription = keystrokes.then(|| {
        cx.observe_keystrokes(|_event, window, cx| {
            let id = window.window_handle().window_id();
            if let Some(input) = cx.default_global::<Tracking>().windows.get_mut(&id) {
                input.window_left = false;
            }
        })
    });
    let tracking = cx.default_global::<Tracking>();
    if subscription.is_some() {
        tracking.keystrokes = subscription;
    }
    tracking.windows.entry(id).or_default().tracker = Some(tracker);
}

/// Whether the focus that just moved came from the keyboard, rather than a
/// pointer, a popup moving it in or back, or the window getting focus back
/// (`isFocusFromKeyboard`).
pub fn is_focus_from_keyboard(window: &mut Window, cx: &mut App) -> bool {
    track_input(window, cx);
    if !window.last_input_was_keyboard() {
        return false;
    }
    let id = window.window_handle().window_id();
    let Some(input) = cx.try_global::<Tracking>().and_then(|tracking| tracking.windows.get(&id)) else {
        return true;
    };
    !input.window_left && !input.moved.as_ref().is_some_and(|moved| moved.is_focused(window))
}

/// Moves focus back after a popup closed, marked as a restoration while its
/// focus events run (`restoreFocus`).
pub fn restore_focus(handle: &FocusHandle, window: &mut Window, cx: &mut App) {
    mark_moved(handle, window, cx);
    window.focus(handle, cx);
}

/// Moves focus into a popup that's opening, marked so a tooltip on the
/// element it lands on stays closed: the user opened the popup, not the
/// tooltip (`moveFocus`).
pub fn move_focus(handle: &FocusHandle, window: &mut Window, cx: &mut App) {
    mark_moved(handle, window, cx);
    window.focus(handle, cx);
}

// gpui runs focus events at the end of the next frame it draws, so the mark
// stays until the frame after that.
fn mark_moved(handle: &FocusHandle, window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    cx.default_global::<Tracking>()
        .windows
        .entry(id)
        .or_default()
        .moved = Some(handle.clone());
    let marked = handle.clone();
    window.on_next_frame(move |window, _cx| {
        window.on_next_frame(move |window, cx| {
            let id = window.window_handle().window_id();
            if let Some(input) = cx.default_global::<Tracking>().windows.get_mut(&id)
                && input.moved.as_ref() == Some(&marked)
            {
                input.moved = None;
            }
        });
    });
}

/// How the user works in a popup now: with the keyboard or a pointer
/// (`data-modality`). Virtual focus never shows a focus ring, so a popup's
/// highlighted items style by this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Modality {
    Keyboard,
    Pointer,
}

/// The modality a popup starts in, from what opened it, by the opener's
/// name, like `"pointer"`, `"touch"`, `"keyboard"`, `"hover"`,
/// `"imperative"` or `"initial"`. Anything but a pointer, like a key or a
/// screen reader's virtual click, counts as the keyboard (`modalityOf`).
pub fn modality_of(opener: &str) -> Modality {
    if opener == "pointer" || opener == "touch" {
        Modality::Pointer
    } else {
        Modality::Keyboard
    }
}

/// How the user works in a popup, from how it opened, then following key
/// presses on the popup or on another element that keeps focus, like a
/// combobox's input, and pointer presses or moves of more than 2px on the
/// popup (`trackModality`). The popup feeds it its events and exposes
/// [`ModalityTracker::get`] in its content's state.
#[derive(Clone, Debug)]
pub struct ModalityTracker {
    current: Modality,
    last: Option<Point<Pixels>>,
}

/// A tracker that starts at `initial` (`trackModality`).
pub fn track_modality(initial: Modality) -> ModalityTracker {
    ModalityTracker {
        current: initial,
        last: None,
    }
}

impl ModalityTracker {
    pub fn get(&self) -> Modality {
        self.current
    }

    /// A key pressed on the popup. Modifier keys alone, and ⌘ or Ctrl
    /// shortcuts, aren't keyboard use. Returns whether the modality changed.
    pub fn key(&mut self, keystroke: &Keystroke) -> bool {
        let modifiers = keystroke.modifiers;
        if modifiers.platform
            || modifiers.control
            || matches!(keystroke.key.as_str(), "shift" | "control" | "alt" | "platform" | "function")
        {
            return false;
        }
        self.set(Modality::Keyboard)
    }

    /// A pointer press on the popup. Returns whether the modality changed.
    pub fn pointer_down(&mut self) -> bool {
        self.set(Modality::Pointer)
    }

    /// The pointer moved on the popup. More than 2px from where it last was
    /// counts as pointer use. Returns whether the modality changed.
    pub fn pointer_move(&mut self, position: Point<Pixels>) -> bool {
        let moved = self.last.is_some_and(|last| {
            let dx = f32::from(position.x - last.x);
            let dy = f32::from(position.y - last.y);
            dx.hypot(dy) > 2.0
        });
        self.last = Some(position);
        moved && self.set(Modality::Pointer)
    }

    fn set(&mut self, next: Modality) -> bool {
        let changed = self.current != next;
        self.current = next;
        changed
    }
}
