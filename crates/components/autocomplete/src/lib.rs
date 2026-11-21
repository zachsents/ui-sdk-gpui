//! Port of `components/autocomplete`: a text field that suggests as you
//! type. The text is the value, and any text is valid: choosing a
//! suggestion only fills it in. It can complete inline, open on focus with
//! recent searches, submit like a search box, load results as you type, and
//! present as a sheet on small touch screens.
//!
//! An app makes an [`Autocomplete`] with `cx.new(|cx| Autocomplete::new(options,
//! window, cx))` and renders its parts: [`Autocomplete::input`], the native
//! text field the autocomplete makes, and [`Autocomplete::content`], with
//! [`Autocomplete::list`] and an [`Autocomplete::item`] per suggestion
//! inside, plus whichever of `label`, `trigger`, `clear_part`, `item_text`,
//! `item_icon`, `group`, `group_label`, `separator`, `empty`, `status`,
//! `arrow`, `viewport` and `header` it needs.
//!
//! # TypeScript file → Rust module
//!
//! `autocomplete.ts` → [`autocomplete`](mod@autocomplete): the options, props
//! and part states, and the agent surface; `autocomplete-api.ts` →
//! [`autocomplete_api`]; `autocomplete-parts.ts` → [`autocomplete_parts`];
//! the module's exports → these re-exports.
//!
//! # In gpui
//!
//! - It's built on the lists crate's autocomplete core: the listbox engine
//!   with virtual focus, where the highlight goes as the list changes, the
//!   arrow keys, and the field and option parts. The core hears what changed
//!   when the autocomplete changes what it reads, and once each frame that
//!   renders the suggestions is drawn.
//! - The field is the native crate's text field, whose element is the
//!   `input` part: it takes the combobox role over the field's, keeps focus
//!   while arrows move the highlight, and points `Aria::active_descendant`
//!   at the highlighted suggestion's `option_id`. The field is told the list
//!   is open (`set_expanded`). What the original passes through to the
//!   native `<input>` are options: `placeholder` and `field_style`, with
//!   `autocomplete`, `autocapitalize` and `spellcheck` off.
//! - The browser's own work on the input is the autocomplete's: its text
//!   reports to its form through its internals under its `name`, missing
//!   while `required` and empty; typing tells the form `Input`, and a commit
//!   `Change`; a form reset puts `default_value` back; and Enter that nothing
//!   prevented submits the form implicitly. A Field disables and names it
//!   the way the original's adapters fold the Field's `disabled` and `name`
//!   into its options.
//! - The list opens through the popups crate's `combobox_preset`, placed
//!   below the field, and binds as the lists crate's `list_content_config`
//!   says. The content reveals the highlighted suggestion in itself, so make
//!   it the element that scrolls.
//! - Inline completion writes the completed text into the field and selects
//!   what it added with the field's `set_selection_range`, as the original
//!   does with `setSelectionRange`. ArrowRight and End reach the field after
//!   the autocomplete took them, and move the caret as they do in the
//!   browser. Escape reaches the list's layer after the field, as a
//!   document's listener hears it after the input's.
//! - Suggestions tell the list their label: gpui can't read the text a
//!   suggestion renders, so one without a `label` uses its value, which is
//!   what typing filters and completes by and what the field gets once it's
//!   chosen. It isn't the suggestion's accessible name: give that as
//!   `aria_label`.
//! - The label part does what a native label does: it focuses the field,
//!   and passes a press on to a field that has focus already, which
//!   reopens suggestions that open on focus.
//! - In a sheet, the app renders the input inside the `header` part while
//!   it's there, the original's portal into the header. A tap that opens a
//!   sheet doesn't focus the field; the field's focus in a sheet grows it to
//!   its largest snap point, which the original's sheet does on `focusin`.
//! - New options reach the core once they're handed over, after the app's
//!   render, so callbacks like `on_highlight` don't run inside it.
//!
//! # Dropped, and why
//!
//! - `createAutocompleteController`, `AutocompleteController`,
//!   `AutocompleteChangeOptions` and the `controller` option: the entity is
//!   the controller, its methods are its api, and changes take the kernel's
//!   `ChangeOptions`.
//! - `container` (`PortalProps`): `deferred` replaces portals.
//! - The listing (`unreadByItems`, `selects`), which lets a suggestion
//!   render only when its own entry changed: gpui renders a component's
//!   parts together.
//! - `takeTypedText`: text typed before the page's script ran.
//! - The `live` and `direct` attribute writes, which keep the highlight and
//!   filtering from rendering every suggestion: gpui renders them together.
//!
//! # Deviations
//!
//! - `on_value_change` takes the chosen suggestion as its own argument,
//!   where the original puts it in `details.item`.
//! - `on_submit` stops the form with `details.prevent_default()`, the
//!   original's `details.event.preventDefault()`.
//! - The api's `clear` and the `clear` slot share a name, so the part is
//!   `clear_part`.
//! - A suggestion's `group` is the key of the group part around it, which
//!   the app passes, where renderers fill it in.
//! - A suggestion that's a link says so with `link`, where the original
//!   finds an `a[href]`; Enter clicks it (`el.click()`), and the app follows
//!   it in its own press on the part.
//! - Inline completion counts the typed text in characters, not UTF-16 code
//!   units, when it takes the rest of a label.
//! - A value from outside, like a controlled one, settles the highlight as
//!   `Imperative`. The original can take the reason an arrow key's fill-in
//!   left behind, and with `inline_complete` fill a suggestion in over the
//!   value.

pub mod autocomplete;
pub mod autocomplete_api;
pub mod autocomplete_parts;

pub use autocomplete::*;
pub use autocomplete_api::Autocomplete;
pub use autocomplete_parts::AutocompleteContent;
pub use ui_sdk_lists::{
    AsyncOptionsStatus, ListAnnouncement, ListData, ListFilter, ListGroup, ListHighlightDetails, ListItem,
    ListNode, ListSeparator, SearchFilter,
};
pub use ui_sdk_text::MatchSegment;
