//! Port of `foundation/text/typeahead.ts`.
//!
//! Typeahead keys are matched with [`ui_sdk_kernel::Key`], which names them as
//! `KeyboardEvent.key` does. The original reads an item's text off its DOM
//! element; gpui elements keep no text to read, so a part reports its own,
//! in its [`PartInfo`], and the label helpers take that.

use std::time::{Duration, Instant};

use gpui::{Keystroke, SharedString};
use ui_sdk_kernel::{Key, PartInfo};

use crate::accessible_name::collapse;
use crate::r#match::{fold_text, js_trim};

/// Comparable form of a label in the user's language: case and accents that
/// don't count there are gone, and so is surrounding space. "Éditer" matches
/// "e". Folding is remembered, since filtering and typeahead fold the same
/// labels on every keystroke.
pub fn normalize(text: &str) -> SharedString {
    let folded = fold_text(text);
    let trimmed = js_trim(&folded);
    if trimmed.len() == folded.len() {
        folded
    } else {
        SharedString::new(trimmed)
    }
}

/// Whether a key press types a character a typeahead should search for: one
/// printable character, without a shortcut modifier (Ctrl, Cmd or Alt).
pub fn is_typeahead_key(keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    !modifiers.control
        && !modifiers.platform
        && !modifiers.alt
        && Key::of(keystroke)
            .typed()
            .is_some_and(|typed| typed.encode_utf16().count() == 1)
}

/// Text a user would type to find a part: the text it reports in its
/// [`PartInfo`]. The original walks the element's text, skipping hidden parts
/// like shortcuts and parts that aren't shown, like an unchecked item's
/// indicator, but not the element's own `hidden`, so an item the search
/// filtered out keeps its text and can match again when the search changes.
/// A part reports its text the same way: without its hidden parts, and
/// whether or not it's filtered out.
pub fn text_of(info: &PartInfo) -> &str {
    info.text.as_deref().unwrap_or_default()
}

/// What a user would type to find an item: its text, with whitespace
/// collapsed as the page shows it, or its `aria_label` when it shows none,
/// like an item with only an icon. The text leaves out what a user can't see,
/// like shortcut hints.
pub fn label_of(text: &str, aria_label: Option<&str>) -> String {
    let text = collapse(text);
    if text.is_empty() {
        aria_label.unwrap_or_default().to_string()
    } else {
        text
    }
}

/// How long a typeahead query lasts after the last key, by default.
pub const TYPEAHEAD_TIMEOUT: Duration = Duration::from_millis(500);

/// Typeahead as the WAI-ARIA Authoring Practices describe it for menus,
/// listboxes and trees: typed characters build a query that clears after
/// `timeout`, and the next item whose label starts with it is the match.
/// Pressing the same character again steps through the items that start with
/// it, unless a label starts with the repeated characters, like "Llama" for
/// "ll". Case and accents fold the way the user's language folds them.
///
/// [`Typeahead::search`] gives the index to focus, with [`Typeahead::reset`]
/// and [`Typeahead::active`] (whether a query is in progress, so Space
/// continues it rather than activating).
///
/// ```ignore
/// let mut typeahead = create_typeahead(TYPEAHEAD_TIMEOUT);
/// // In a key handler:
/// if is_typeahead_key(&event.keystroke) {
///     let labels: Vec<String> = items.entries().iter().map(|(_, info)| label_of(text_of(info), None)).collect();
///     if let Some(index) = typeahead.search(&Key::of(&event.keystroke), &labels, current) {
///         // Focus items[index].
///     }
/// }
/// ```
#[derive(Clone, Debug)]
pub struct Typeahead {
    timeout: Duration,
    query: String,
    last_key: String,
    stepping: bool,
    // When the query clears, the timer each key restarts.
    expires: Option<Instant>,
}

impl Default for Typeahead {
    fn default() -> Self {
        Self::new(TYPEAHEAD_TIMEOUT)
    }
}

/// A typeahead whose query clears `timeout` after the last key.
pub fn create_typeahead(timeout: Duration) -> Typeahead {
    Typeahead::new(timeout)
}

impl Typeahead {
    /// A typeahead whose query clears `timeout` after the last key
    /// (`createTypeahead`).
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            query: String::new(),
            last_key: String::new(),
            stepping: false,
            expires: None,
        }
    }

    /// Clears the query now, like when the component handles Escape.
    pub fn reset(&mut self) {
        self.query.clear();
        self.last_key.clear();
        self.stepping = false;
        self.expires = None;
    }

    /// Whether a query is in progress, so Space continues it rather than activating.
    pub fn active(&self) -> bool {
        !self.query.is_empty() && !self.expired()
    }

    // The timer has run out: the query cleared then.
    fn expired(&self) -> bool {
        self.expires.is_some_and(|expires| Instant::now() >= expires)
    }

    /// The index of the item to move to for `key`, given each item's label and
    /// the current index, or `None` when nothing matches or the key types
    /// nothing.
    pub fn search<L: AsRef<str>>(&mut self, key: &Key, labels: &[L], current: Option<usize>) -> Option<usize> {
        if self.expired() {
            self.reset();
        }
        let key = key.typed()?;
        // Space only continues a query. Before one, it's the key that activates.
        if key.encode_utf16().count() != 1 || (key == " " && self.query.is_empty()) {
            return None;
        }
        self.expires = Some(Instant::now() + self.timeout);
        let normalized = normalize(key);
        let char = if normalized.is_empty() {
            key.to_string()
        } else {
            normalized.to_string()
        };
        self.stepping = char == self.last_key && (self.stepping || self.query.chars().count() == 1);
        self.last_key = char.clone();
        self.query.push_str(&char);

        if self.stepping {
            return scan(labels, &self.query, current.unwrap_or(0))
                .or_else(|| scan(labels, &char, search_start(current, &char)));
        }
        scan(labels, &self.query, search_start(current, &self.query))
    }
}

fn search_start(current: Option<usize>, query: &str) -> usize {
    match current {
        Some(current) if query.chars().count() == 1 => current + 1,
        Some(current) => current,
        None => 0,
    }
}

fn scan<L: AsRef<str>>(labels: &[L], prefix: &str, start: usize) -> Option<usize> {
    let count = labels.len();
    (0..count)
        .map(|step| (start + step) % count)
        .find(|&index| normalize(labels[index].as_ref()).starts_with(prefix))
}
