//! Port of `components/listbox`: an always-visible list of options to choose
//! one or several from, like a native select with several visible rows,
//! with keyboard navigation, typeahead and range selection, on the listbox
//! engine Select and Combobox share.
//!
//! `listbox-root.ts` → [`listbox_root`] (options, value, reasons, part
//! states, the agent surface), `listbox-root-api.ts` → [`listbox_root_api`]
//! ([`Listbox`], its state and api), `listbox-root-keyboard.ts` →
//! [`listbox_root_keyboard`], `listbox-root-parts.ts` →
//! [`listbox_root_parts`], `index.ts` → these re-exports.
//!
//! An app makes a [`Listbox`] with `cx.new(|cx| Listbox::new(options, window,
//! cx))` and renders [`Listbox::root`] around its items: [`Listbox::item`]
//! for each option, with [`Listbox::item_indicator`] and
//! [`Listbox::item_icon`] inside, and [`Listbox::group`],
//! [`Listbox::group_label`] and [`Listbox::separator`] around them. The root
//! starts the items' render, so build it first. The value is a
//! [`ListboxValue`]: `Single` without `multiple`, `Multiple` with it.
//! Values are any type with a key, `ui_sdk_lists::ItemKey` or
//! `item_to_value`; the default is `SharedString`.
//!
//! # Dropped, and why
//!
//! - `listbox-root-generic.ts`: TypeScript's inference of the value type for
//!   renderers. It's the listbox's type parameter.
//! - `createListboxController`, `ListboxController` and the `controller`
//!   option: the app holds the entity, whose methods are the controller's.
//! - The `form-input` and `value-input` slots, the hidden inputs a form
//!   submits: the root reports the value through its form internals, its
//!   key, or with `multiple` every key under its name
//!   (`ui_sdk_lists::bind_form_root`).
//! - `internal.restore`, `internal.autofill` and `internal.adoptValue`: a
//!   browser restoring or autofilling a form, and web components reading
//!   `default-value` as keys. gpui has none of them.
//! - The api's `icons`: no listbox part reads them; an item icon needs no
//!   registry here.
//! - The Shift press that stops the page selecting text: gpui text isn't
//!   selected by clicks.
//! - `selects` and `live`: a change renders the listbox's parts again.
//! - `data-*` attributes: part state.
//!
//! # Deviations
//!
//! - gpui can't read text back from what an item renders, so an item says
//!   it in its props' `text`: what typing finds and what names it, as its
//!   `aria_label`, since a text child names no option in gpui. `label` still
//!   wins for typing.
//! - Focus landing on an option, from a key, a press or Tab, is read as the
//!   options settle, once each frame that rendered them is drawn, since gpui
//!   tells where focus is when it draws; the stop's check runs then too, as
//!   the original's microtask waits for them to bind.
//! - gpui clicks a focused option as Enter or Space comes up, where an
//!   option element in the page has no click of its own. The root's keys
//!   choose the focused option, so that click is ignored, except Enter on a
//!   link, which follows it and chooses it. With `virtual_focus`, Enter
//!   clicks the link the list points at (`el.click()`).
//! - Keys from a listbox nested inside another bubble through; a registry
//!   of listbox roots stands in for `closest('[role="listbox"]')`.
//! - A long list scrolls in the root: give it a height and
//!   `overflow_y_scroll`, and the root's own scroll handle reveals options.
//!   Its CSS `scroll-padding` isn't read, so none is kept clear.
//! - gpui hands a tap over as a mouse click; the window's last input tells
//!   them apart, as the original's `pointerType` does.

pub mod listbox_root;
pub mod listbox_root_api;
pub mod listbox_root_keyboard;
pub mod listbox_root_parts;

pub use listbox_root::{
    HighlightCallback, ItemEqualFn, ItemToValueFn, ListboxActivation, ListboxChangeOptions,
    ListboxItemIconProps, ListboxItemIconState, ListboxItemIndicatorProps, ListboxItemIndicatorState,
    ListboxItemProps, ListboxItemState, ListboxOptions, ListboxOrientation, ListboxState, ListboxValue,
    ListboxValueChangeReason,
};
pub use listbox_root_api::Listbox;
pub use ui_sdk_lists::listbox_engine::{ListData, ListGroup, ListHighlightDetails, ListItem, ListNode, ListSeparator};
