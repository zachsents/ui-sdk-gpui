//! Port of `components/select`: a button that opens a list of options and
//! shows the chosen one, like a native `<select>`, on the listbox engine and
//! the value selection Combobox shares, in a popup from `ui-sdk-popups`.
//!
//! `select.ts` → [`select`](mod@select) (options, value, reasons, part
//! states, the agent surface), `select-api.ts` → [`select_api`]
//! ([`Select`], its state and api), `select-keyboard.ts` →
//! [`select_keyboard`], `select-parts.ts` → [`select_parts`], `index.ts` →
//! these re-exports.
//!
//! An app makes a [`Select`] with `cx.new(|cx| Select::new(options, window,
//! cx))` and renders [`Select::label`], [`Select::trigger`] with
//! [`Select::value_part`] (and [`Select::value_icon`]) inside, and
//! [`Select::content`], which is `None` while the list is out of the page,
//! with [`Select::item`] for each option and [`Select::item_indicator`],
//! [`Select::item_text`] and [`Select::item_icon`] inside them. Groups,
//! separators, a search field ([`Select::input`] above [`Select::list`]),
//! the empty and status messages, the select-all option, the clear button
//! ([`Select::clear_part`]), the arrow and the sheet's [`Select::viewport`]
//! are parts too. The value is a [`SelectValue`]: `Single` without
//! `multiple`, `Multiple` with it.
//!
//! The popup is `ui_sdk_popups::select_preset`'s: like a native select's,
//! Tab closes it and focus moves on from the trigger. It's placed below the
//! trigger, or with `align_item_with_trigger` with the selected item over
//! it, modal to the pointer, and with `presentation` it presents as a sheet.
//!
//! # Dropped, and why
//!
//! - `select-generic.ts`: TypeScript's inference of the value type for
//!   renderers. It's the select's type parameter.
//! - `createSelectController`, `SelectController`, the `controller` option
//!   and the trigger's `controller`, which connects a trigger outside the
//!   select: the app holds the entity, so a trigger anywhere is its part.
//! - The `root` slot, web components' form element, and the `form-input`
//!   and `value-input` slots: the trigger reports the value through its form
//!   internals (`ui_sdk_lists::hidden_input_part`).
//! - `container`: popups render above the window with `deferred`, which
//!   needs no portal.
//! - `learnLabels`: an adapter finding the value's labels among its children
//!   before the items render, like on the server. The closed list renders
//!   hidden so its items tell their labels.
//! - `restore`, `autofill` and `adoptValue`: a browser restoring or
//!   autofilling a form, and web components reading `default-value` as keys.
//! - The value's `text` prop and `stashContent`: web components writing
//!   their own content.
//! - The listing (`keepListing`), `unreadByItems`, `selects`, `direct` and
//!   `live`: a change renders the select's parts again.
//! - `data-*` attributes and the CSS variables: part state.
//!
//! # Deviations
//!
//! - gpui can't read text back from what an item renders, so an item says
//!   it in its props' `text`: what typing and the search find, what the
//!   trigger shows once it's chosen, and its `aria_label`.
//! - The value part is [`Select::value_part`] and the clear button
//!   [`Select::clear_part`], since `value` and `clear` are the api's. The
//!   value's state carries the `text` it shows, the label or the
//!   placeholder, which the trigger reports as its value to assistive
//!   technology.
//! - gpui clicks the focused trigger as Enter or Space comes up, the native
//!   button's click the original opens with. Where the original prevents
//!   that click, a Space that continues a search and the open list's keys
//!   with `virtual_focus`, the trigger prevents it as the key goes down.
//!   gpui clicks a focused option the same way; the list's keys choose it,
//!   so that click is ignored, except Enter on a link, which follows it and
//!   chooses it. The select clicks a link it only points at from the search
//!   field or the trigger (`el.click()`).
//! - The press that drags to an item hears its release through a listener
//!   the trigger paints while it lasts, and finds the item under the
//!   pointer by where items laid out.
//! - Where focus goes as the list opens waits for composed items to join
//!   the list, which happens once the frame that rendered them is drawn,
//!   after the popup asks.
//! - `alignItemWithTrigger` measures the selected item once the content and
//!   its items have laid out; the content waits out of sight for that frame.
//! - The list is named by the label part, else by the field's labels, else
//!   by the trigger. The original copies the trigger's name, since a
//!   reference to a combobox names a list after its value; accesskit names a
//!   node labelled by the trigger with the trigger's own name.
//! - The search field is a native text field the select makes, with its
//!   own look and placeholder from the `search` option. Its text follows the
//!   query once the update that changed it is over.
//! - The content's modality (`trackModality`) follows the keys and pointer
//!   presses and moves it hears, and the trigger's keys with
//!   `virtual_focus`; its state's `modality` is `data-modality`.
//! - A sheet's selected item scrolls into view once the sheet's items have
//!   rendered, the original's microtask after they bind.

pub mod select;
pub mod select_api;
pub mod select_keyboard;
pub mod select_parts;

pub use select::{
    HighlightCallback, ItemEqualFn, ItemToLabelFn, ItemToValueFn, MoreLabelFn, SelectArrowState,
    SelectChangeOptions, SelectClearProps, SelectClearState, SelectContentProps, SelectContentState,
    SelectDismissReason, SelectEmptyState, SelectInputState, SelectItemIconProps, SelectItemIconState,
    SelectItemIndicatorProps, SelectItemIndicatorState, SelectItemProps, SelectItemState, SelectItemTextState,
    SelectOpenChangeReason, SelectOptions, SelectQueryChangeReason, SelectSelectAllState, SelectStatusState,
    SelectTriggerProps, SelectTriggerState, SelectValue, SelectValueChangeReason, SelectValueProps,
    SelectValueState,
};
pub use select_api::{InitialHighlight, Select};
pub use select_keyboard::{DRAG_PX, HOLD_MS, initial_highlight_for, select_step};
pub use ui_sdk_lists::listbox_engine::{
    ListData, ListFilter, ListGroup, ListHighlightDetails, ListItem, ListNode, ListSeparator,
};
pub use ui_sdk_lists::listbox_search::ListAnnouncement;
