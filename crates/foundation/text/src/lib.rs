//! Port of `foundation/text`: finding items by typing, ranking and
//! highlighting search results, accessible names, bringing back earlier
//! messages, the editor a suggestion menu writes into, and the language text
//! is in.
//!
//! The original matches and collates through `Intl.Collator` and
//! `String.prototype.normalize`; here ICU4X's compiled CLDR and Unicode data
//! do the same. Its locale comes from `lang` attributes and the navigator;
//! here it's the app's ([`set_page_locale`]) and the system's
//! ([`user_locale`]).
//!
//! Text offsets are byte offsets into Rust strings, where the original counts
//! UTF-16 units: match ranges, editor ranges and diffs all slice the text they
//! came from.
//!
//! Dropped, with why:
//! - Reading text, `aria-*`, labels and roles off DOM elements (`textOf`'s
//!   tree walk, `accessibleName`'s attribute reads, `roleOf`'s tag names):
//!   gpui elements keep none of it where it can be read back, so parts report
//!   their text in their `PartInfo` and their names through [`NameSources`].
//! - A section's own `lang` (`pageLocale(el)`): gpui elements carry no
//!   attributes to inherit; a component's `locale` option does it.
//! - Server rendering: `bindPageLocale`'s wait for the element to be in the
//!   document, and English until then. There is always a window.
//! - The `languagechange` listener: gpui hears no such event, so the user's
//!   language is read once, and [`set_match_locale`] starts matching over.
//! - The caret mirror (`measureInMirror`, Chromium's value ranges and their
//!   cache): a gpui text field has its last layout and reports its caret
//!   through [`TextField::caret_rect`].
//! - `execCommand` and `setRangeText`: a [`TextField`] edits as one undoable
//!   step itself.
//! - `usage: "search"` collation: ICU4X ships no search tailorings, so the
//!   standard collation decides, which agrees at primary strength on single letters.
//! - The production stub as a separate module (`name-check-stub.ts`): release
//!   builds compile [`check_name`] and [`name_from`] to nothing.

pub mod accessible_name;
pub mod caret;
pub mod fuzzy;
pub mod input_history;
pub mod locale;
pub mod r#match;
pub mod name_check;
pub mod text_editor;
pub mod text_match;
pub mod typeahead;

pub use accessible_name::{
    NAMED_BY_CONTENT, NameContent, NameKind, NameSources, accessible_name, collapse,
    has_accessible_name, named_by_content, role_of,
};
pub use caret::{
    TextDiff, TextField, TextFieldAdapter, TextFieldEvent, TextFieldKind, TextFieldListener,
    text_diff, text_field_adapter,
};
pub use fuzzy::{fuzzy_filter, fuzzy_score};
pub use input_history::{
    AtEdge, HistoryDirection, InputHistory, InputHistoryOptions, at_text_edge, create_input_history,
};
pub use locale::{
    DEFAULT_LOCALE, bind_page_locale, locale_from, locale_of, page_locale, set_page_locale,
    to_locale, user_locale,
};
pub use r#match::{
    Folder, MatchMode, MatchRange, MatchSegment, create_folder, fold_text, is_js_space, js_trim, leading_spaces,
    match_ranges, match_segments, set_match_locale, starts_word,
};
pub use name_check::{NameFrom, check_name, name_from};
pub use text_editor::{
    EditorChange, EditorContent, EditorListener, EditorReplaceOptions, PendingToken, TextBefore,
    TextEditorAdapter, TextRange, TextRect, Token, TokenInsert, TokenStatus, TokenTrigger, TokenUpdate,
};
pub use typeahead::{
    TYPEAHEAD_TIMEOUT, Typeahead, create_typeahead, is_typeahead_key, label_of, normalize, text_of,
};
