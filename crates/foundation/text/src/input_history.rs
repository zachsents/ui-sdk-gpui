//! Port of `foundation/text/input-history.ts`.
//!
//! The original's options hold three functions it calls on each key:
//! `entries`, `read` and `write`. A gpui component calls the history from
//! inside its own update, where those functions couldn't read or write the
//! component, so [`InputHistory::keydown`] takes the entries, the value shown
//! and the write with each key instead. The field the key went to, the
//! original's `event.target`, is a [`TextField`].

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use gpui::{App, KeyDownEvent, SharedString, Window};
use ui_sdk_kernel::{Key, is_composing};

use crate::caret::{TextField, TextFieldKind, char_boundary};
use crate::text_editor::TextRange;

/// Which way an arrow steps through the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HistoryDirection {
    Up,
    Down,
}

/// Whether the caret can't move any further in a direction, for
/// [`InputHistoryOptions::at_edge`].
pub type AtEdge = Rc<dyn Fn(HistoryDirection, &KeyDownEvent, &Window, &App) -> bool>;

/// What [`create_input_history`] takes.
pub struct InputHistoryOptions<T> {
    /// Whether the caret can't move any further in `direction`, so the arrow
    /// reaches the earlier values. Defaults to the first and last line of a
    /// text area, and anywhere in a text field.
    pub at_edge: Option<AtEdge>,
    /// A key for each value, like a message's id. With it, values are matched
    /// by key: values made again from the same messages keep their place, and
    /// so does the value shown when a new one arrives. Without it, values are
    /// compared as they are, and any change starts over.
    pub get_entry_key: Option<Rc<dyn Fn(&T) -> SharedString>>,
}

impl<T> Default for InputHistoryOptions<T> {
    fn default() -> Self {
        Self {
            at_edge: None,
            get_entry_key: None,
        }
    }
}

impl<T> InputHistoryOptions<T> {
    pub fn at_edge(mut self, at_edge: impl Fn(HistoryDirection, &KeyDownEvent, &Window, &App) -> bool + 'static) -> Self {
        self.at_edge = Some(Rc::new(at_edge));
        self
    }

    pub fn get_entry_key(mut self, key: impl Fn(&T) -> SharedString + 'static) -> Self {
        self.get_entry_key = Some(Rc::new(key));
        self
    }
}

impl<T> fmt::Debug for InputHistoryOptions<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputHistoryOptions")
            .field("at_edge", &self.at_edge.is_some())
            .field("get_entry_key", &self.get_entry_key.is_some())
            .finish()
    }
}

// The kinds of text field whose arrows bring back values. A number field
// keeps its steps.
const RECALLABLE_KINDS: [TextFieldKind; 4] = [
    TextFieldKind::Text,
    TextFieldKind::Search,
    TextFieldKind::Url,
    TextFieldKind::Tel,
];

/// Whether the caret is at the edge of a field in `direction`: anywhere in a
/// text field of a kind that recalls, and the first or last row of a text
/// area. A long line wraps, so only its first or last visual row is the edge.
/// No field is an edge.
pub fn at_text_edge(direction: HistoryDirection, field: Option<&dyn TextField>, window: &Window, cx: &App) -> bool {
    let Some(field) = field else {
        return true;
    };
    let kind = field.kind(cx);
    if kind != TextFieldKind::TextArea {
        return RECALLABLE_KINDS.contains(&kind);
    }
    let value = field.value(cx);
    let selection = field.selection_range(cx);
    let caret = char_boundary(&value, selection.start);
    if caret != selection.end {
        return false;
    }
    let end = match direction {
        HistoryDirection::Up => 0,
        HistoryDirection::Down => value.len(),
    };
    if caret == end {
        return true;
    }
    let between = match direction {
        HistoryDirection::Up => &value[..caret],
        HistoryDirection::Down => &value[caret..],
    };
    if between.contains('\n') {
        return false;
    }
    let row = |offset: usize| field.caret_rect(offset, window, cx).map(|rect| rect.origin.y);
    row(caret) == row(end)
}

/// Steps through earlier values with the arrow keys, like a terminal or a
/// chat box: ArrowUp brings back the previous value, ArrowDown the next, and
/// ArrowDown past the newest brings back what was being typed. Edits to a
/// value brought back last until the entries change, like after a send.
///
/// ```ignore
/// let mut history = create_input_history(InputHistoryOptions::default());
/// // In the field's key handler:
/// let entries = self.sent.clone();
/// let shown = self.draft.clone();
/// if history.keydown(event, Some(&field), &entries, shown, |value, window, cx| {
///     field.set_value(value, window, cx);
///     true
/// }, window, cx) {
///     return;
/// }
/// ```
pub struct InputHistory<T> {
    options: InputHistoryOptions<T>,
    seen: Vec<T>,
    // The index shown, where `seen.len()` is the value being typed.
    at: usize,
    edits: HashMap<usize, T>,
}

/// A history over the values of a field (`createInputHistory`).
pub fn create_input_history<T: Clone + PartialEq>(options: InputHistoryOptions<T>) -> InputHistory<T> {
    InputHistory {
        options,
        seen: Vec::new(),
        at: 0,
        edits: HashMap::new(),
    }
}

impl<T: Clone + PartialEq> InputHistory<T> {
    /// Starts again from the newest value, and forgets edits to values
    /// brought back. The next key reads the entries again.
    pub fn reset(&mut self) {
        self.seen.clear();
        self.at = 0;
        self.edits.clear();
    }

    // Starts again from the newest of `entries`.
    fn restart(&mut self, entries: &[T]) {
        self.seen = entries.to_vec();
        self.at = self.seen.len();
        self.edits.clear();
    }

    // Keeps the value shown, and edits to values brought back, on their keys.
    fn follow(&mut self, entries: &[T], key: &dyn Fn(&T) -> SharedString) {
        let shown = self.seen.get(self.at).map(key);
        let mut edited: HashMap<SharedString, T> = HashMap::new();
        for (index, value) in &self.edits {
            if let Some(entry) = self.seen.get(*index) {
                edited.insert(key(entry), value.clone());
            }
        }
        let draft = self.edits.get(&self.seen.len()).cloned();
        self.seen = entries.to_vec();
        self.edits.clear();
        for (index, entry) in self.seen.iter().enumerate() {
            if let Some(value) = edited.get(&key(entry)) {
                self.edits.insert(index, value.clone());
            }
        }
        if let Some(draft) = draft {
            self.edits.insert(self.seen.len(), draft);
        }
        self.at = shown
            .and_then(|shown| self.seen.iter().position(|entry| key(entry) == shown))
            .unwrap_or(self.seen.len());
    }

    fn step(
        &mut self,
        up: bool,
        field: Option<&dyn TextField>,
        entries: &[T],
        current: T,
        write: impl FnOnce(&T, &mut Window, &mut App) -> bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        if entries != self.seen.as_slice() {
            match self.options.get_entry_key.clone() {
                Some(key) => self.follow(entries, key.as_ref()),
                None => self.restart(entries),
            }
        }
        let next = if up { self.at.checked_sub(1) } else { Some(self.at + 1) };
        let Some(next) = next.filter(|next| *next <= self.seen.len()) else {
            return false;
        };
        self.edits.insert(self.at, current);
        let value = match self.edits.get(&next) {
            Some(edit) => edit.clone(),
            None => self.seen[next].clone(),
        };
        if !write(&value, window, cx) {
            return false;
        }
        self.at = next;
        if let Some(field) = field {
            let end = field.value(cx).len();
            field.set_selection_range(TextRange::caret(end), window, cx);
        }
        true
    }

    /// Handles ArrowUp and ArrowDown on the field: brings back an earlier
    /// value from `entries`, oldest first, through `write`, given the value
    /// `current`ly shown. `write` shows the value as if the user typed it, in
    /// the field before it returns, and returns false when the change was
    /// canceled, which leaves the key to the field. Returns whether it brought
    /// back a value, having stopped the key's default; your own keys, like
    /// Enter to send, come after it.
    ///
    /// It leaves the arrows alone while a modifier is held, while an input
    /// method is composing, while text is selected in a text area, on a
    /// read-only field, and while a popup the field controls is open.
    #[allow(clippy::too_many_arguments)]
    pub fn keydown(
        &mut self,
        event: &KeyDownEvent,
        field: Option<&dyn TextField>,
        entries: &[T],
        current: T,
        write: impl FnOnce(&T, &mut Window, &mut App) -> bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let direction = match Key::of(&event.keystroke) {
            Key::ArrowUp => HistoryDirection::Up,
            Key::ArrowDown => HistoryDirection::Down,
            _ => return false,
        };
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented()
            || modifiers.alt
            || modifiers.control
            || modifiers.platform
            || modifiers.shift
            || is_composing(window, cx)
            || field.is_some_and(|field| field.read_only(cx) || field.expanded(cx))
        {
            return false;
        }
        let at_edge = match &self.options.at_edge {
            Some(at_edge) => at_edge(direction, event, window, cx),
            None => at_text_edge(direction, field, window, cx),
        };
        if !at_edge {
            return false;
        }
        if !self.step(direction == HistoryDirection::Up, field, entries, current, write, window, cx) {
            return false;
        }
        window.prevent_default();
        true
    }
}
