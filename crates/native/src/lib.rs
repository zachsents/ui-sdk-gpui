//! Stand-ins for the elements a browser provides that the original builds on.
//!
//! [`SelectableText`] is a page's words, which the reader selects and copies
//! but doesn't edit: runs of text sharing a [`TextSelection`], selected by
//! press and drag across them, by word after a double press and by paragraph
//! after a triple press, as a browser and a Mac's text do (module
//! [`selectable_text`]).
//!
//! [`TextField`] is `<input>` and `<textarea>`, which every text-entry
//! component builds on: an entity with a `root` part, the api the original
//! uses on `HTMLInputElement` (`value`, `set_value`, `selection_start`,
//! `selection_end`, `set_selection_range`, `select`, `focus`, `blur`), and
//! the events it listens for, as hooks given the field itself
//! ([`TextFieldOptions`]) and as [`TextFieldEvent`]s for subscribers:
//! `beforeinput` with a veto, `input` with the kind of edit, `change`,
//! `selectionchange`, `focus`, `blur` and composition.
//!
//! It edits as each platform's text fields do: by character, word and line,
//! with Option and Command on a Mac (and its Control-key Emacs moves) and
//! Control elsewhere; selects with the mouse and the keyboard; composes with
//! input methods; copies, cuts and pastes; and undoes and redoes in steps.
//! It's headless: the app styles the part, the font and its features (like
//! tabular numbers) come from the part's text style, and the text, caret,
//! selection and placeholder take their colors from [`TextFieldStyle`],
//! which defaults to the inherited text color. The style also aligns the
//! text by the reading direction and sizes the field in `ch`, which gpui's
//! own styles can't.
//!
//! While an input method composes, the field says so to the kernel
//! (`ui_sdk_kernel::set_composing`), so key handlers leave keys alone through
//! `ui_sdk_kernel::is_composing`, the original's `isComposing`. Its
//! [`TextFieldHandle`] is the field as `ui-sdk-text`'s editor helpers take
//! one, `ui_sdk_text::TextField`.
//!
//! # Modules
//!
//! The browser is the original here, so the modules follow the field:
//! [`text_field`] (the entity, its options, part and focus), [`events`]
//! (`beforeinput`, `input` and the rest), [`style`], [`adapter`] (the text
//! crate's handle), and, inside, editing, the undo history, keys, the mouse,
//! layout, the shown text, the platform's text input and accessibility.
//!
//! # Dropped, and why
//!
//! - Spelling and grammar marks, autofill, the context menu, and dragging
//!   text in and out: gpui has no platform service for them.
//! - `inputmode` and `autocomplete`: gpui's text input takes corrections,
//!   capitals, suggestions and the Return key's label
//!   ([`TextInputConfiguration`](gpui::TextInputConfiguration)), not a
//!   keyboard type or an autofill kind.
//! - Right-to-left text: gpui lays text out left to right. A style's `Start`
//!   and `End` alignments still follow the app's direction
//!   (`ui_sdk_platform::is_rtl`).
//! - Secure input for passwords: gpui can't turn off input methods or turn
//!   on the platform's secure keyboard entry. A password masks what it
//!   shows, and never copies, cuts or reaches the platform's text services.
//! - Search fields clearing on Escape: no component relies on it.
//! - An agent surface: the original's agent bridge reads components, not the
//!   native elements inside them, so the component built on a field is what
//!   an agent reads and sets.
//! - Offsets count UTF-8 bytes, Rust's, rather than UTF-16 code units, and
//!   the maximum length counts characters. The platform's text input
//!   converts at the edge.

mod accessibility;
pub mod adapter;
mod boundaries;
mod display;
mod editing;
mod element;
pub mod events;
mod history;
mod input_handler;
mod keys;
mod layout;
mod mouse;
pub mod selectable_text;
pub mod style;
pub mod text_field;

pub use adapter::TextFieldHandle;
pub use boundaries::{paragraph_at, word_at};
pub use events::{
    BeforeEdit, Edit, FieldHook, FieldNotice, InputType, SelectionDirection, TextFieldEvent,
    TextFilter,
};
pub use selectable_text::{
    Granularity, Join, LinkLook, SelectableText, TextPosition, TextSelection,
    default_selection_wash,
};
pub use style::{TextFieldAlign, TextFieldKind, TextFieldStyle};
pub use text_field::{
    Copy, Cut, KEY_CONTEXT, Paste, Redo, SelectAll, TextField, TextFieldOptions, TextFieldState,
    Undo, focused_text_field, is_typing,
};
