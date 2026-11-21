//! Port of `foundation/kernel/events.ts`, and the keys behaviors match.
//!
//! The original matches `event.key`, like `"ArrowDown"`. gpui names keys its
//! own way, like `"down"`; [`Key`] gives them back the original's names, so a
//! port matches the keys its source does.
//!
//! `on` adds a DOM listener and returns its removal; gpui elements take
//! listeners as they render, and subscriptions end when dropped, so it has no
//! counterpart.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use gpui::{
    App, DispatchPhase, Global, KeyDownEvent, Keystroke, Modifiers, Subscription, Window, WindowId,
};

use super::details::InputEvent;

/// A pressed key, as `KeyboardEvent.key` names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    /// `" "`.
    Space,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    /// `F1` to `F35`.
    Function(u8),
    /// A key that types, as the character it types, like `"a"`, `"A"` with
    /// Shift, or `"ß"` with Option on a Mac.
    Character(String),
    /// Any other key, by gpui's name.
    Other(String),
}

impl Key {
    pub fn of(keystroke: &Keystroke) -> Key {
        match keystroke.key.as_str() {
            "up" => Key::ArrowUp,
            "down" => Key::ArrowDown,
            "left" => Key::ArrowLeft,
            "right" => Key::ArrowRight,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "enter" => Key::Enter,
            "space" => Key::Space,
            "escape" => Key::Escape,
            "tab" => Key::Tab,
            "backspace" => Key::Backspace,
            "delete" => Key::Delete,
            "insert" => Key::Insert,
            name => {
                if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
                    && (1..=35).contains(&n)
                {
                    return Key::Function(n);
                }
                match &keystroke.key_char {
                    Some(typed) if !typed.is_empty() => Key::Character(typed.clone()),
                    _ if name.chars().count() == 1 => Key::Character(name.to_string()),
                    _ => Key::Other(name.to_string()),
                }
            }
        }
    }

    /// The original's name for it, like `"ArrowDown"` or `" "`.
    pub fn name(&self) -> &str {
        match self {
            Key::ArrowUp => "ArrowUp",
            Key::ArrowDown => "ArrowDown",
            Key::ArrowLeft => "ArrowLeft",
            Key::ArrowRight => "ArrowRight",
            Key::Home => "Home",
            Key::End => "End",
            Key::PageUp => "PageUp",
            Key::PageDown => "PageDown",
            Key::Enter => "Enter",
            Key::Space => " ",
            Key::Escape => "Escape",
            Key::Tab => "Tab",
            Key::Backspace => "Backspace",
            Key::Delete => "Delete",
            Key::Insert => "Insert",
            Key::Function(n) => FUNCTION_NAMES.get(*n as usize).copied().unwrap_or("F"),
            Key::Character(typed) => typed,
            Key::Other(name) => name,
        }
    }

    /// The character it types, for typeahead: a character key, or Space.
    pub fn typed(&self) -> Option<&str> {
        match self {
            Key::Character(typed) => Some(typed),
            Key::Space => Some(" "),
            _ => None,
        }
    }
}

const FUNCTION_NAMES: [&str; 36] = [
    "F", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
    "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24", "F25", "F26", "F27",
    "F28", "F29", "F30", "F31", "F32", "F33", "F34", "F35",
];

/// The key of a key event.
pub fn key(event: &KeyDownEvent) -> Key {
    Key::of(&event.keystroke)
}

/// The modifiers held with a key event.
pub fn modifiers(event: &KeyDownEvent) -> Modifiers {
    event.keystroke.modifiers
}

// The windows whose focused text field holds marked text, an input method's
// composition, as the field reports it.
#[derive(Default)]
struct Composing(HashSet<WindowId>);

impl Global for Composing {}

/// Whether an input method is composing text in `window`, like Enter
/// choosing one of its candidates, so a key handler leaves the key alone
/// (`isComposing`). gpui keeps the composition in the focused element's
/// input handler, its marked text range, and the platform hands it most
/// composing keys before any element; the text field that owns the handler
/// reports the rest with [`set_composing`].
pub fn is_composing(window: &Window, cx: &App) -> bool {
    cx.try_global::<Composing>()
        .is_some_and(|composing| composing.0.contains(&window.window_handle().window_id()))
}

/// Reports whether the focused text field in `window` holds marked text. A
/// text field calls it from its input handler as marked text is set and
/// cleared (`replace_and_mark_text_in_range`, `unmark_text`).
pub fn set_composing(window: &Window, composing: bool, cx: &mut App) {
    let id = window.window_handle().window_id();
    let windows = &mut cx.default_global::<Composing>().0;
    if composing {
        windows.insert(id);
    } else {
        windows.remove(&id);
    }
}

thread_local! {
    // The key press being dispatched, and whether the platform repeats it
    // because the key is held, as a focusable part on its way saw it.
    static PRESSED: RefCell<Option<(Keystroke, bool)>> = const { RefCell::new(None) };
}

// Forgets the last press as each new one starts, before any element has it.
struct KeyTracking(#[allow(dead_code)] Subscription);

impl Global for KeyTracking {}

/// Whether the key press being dispatched now repeats because the key is
/// held (`event.repeat`), for those that hear it without its `KeyDownEvent`,
/// like a keystroke observer. gpui's observers don't say, so the focused
/// part notes it as the press reaches it. A press with no part focused, like
/// one with nothing focused, reads as not held.
pub fn key_held(keystroke: &Keystroke) -> bool {
    PRESSED.with(|pressed| {
        pressed
            .borrow()
            .as_ref()
            .is_some_and(|(seen, held)| *held && seen == keystroke)
    })
}

/// Notes each key press that reaches the element painting now, for
/// [`key_held`]. Call it as the element paints.
pub(crate) fn hear_key_presses(window: &mut Window, cx: &mut App) {
    if cx.try_global::<KeyTracking>().is_none() {
        let subscription = cx.intercept_keystrokes(|_, _, _| {
            PRESSED.with(|pressed| pressed.borrow_mut().take());
        });
        cx.set_global(KeyTracking(subscription));
    }
    window.on_key_event(|event: &KeyDownEvent, phase, _window, _cx| {
        if phase == DispatchPhase::Capture {
            PRESSED.with(|pressed| *pressed.borrow_mut() = Some((event.keystroke.clone(), event.is_held)));
        }
    });
}

/// What an `on_select` handler receives, with the event behind the select
/// if there is one: calling `prevent_default()` keeps what the select would
/// have done from happening, like a menu closing after its item.
#[derive(Debug, Default)]
pub struct SelectEvent {
    pub event: Option<InputEvent>,
    prevented: Cell<bool>,
}

impl SelectEvent {
    pub fn new(event: Option<InputEvent>) -> Self {
        Self {
            event,
            prevented: Cell::new(false),
        }
    }

    pub fn prevent_default(&self) {
        self.prevented.set(true);
    }

    pub fn default_prevented(&self) -> bool {
        self.prevented.get()
    }
}

/// What an `on_select` handler receives when no event is left to cancel,
/// like for a shortcut or a controller's call (`selectEvent`).
pub fn select_event() -> SelectEvent {
    SelectEvent::new(None)
}
