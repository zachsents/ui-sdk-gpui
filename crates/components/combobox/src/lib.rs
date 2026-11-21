//! Port of `components/combobox`: a text input that filters a list of
//! options and holds the chosen one, or several as chips, with an option
//! that creates one, results loaded as you type, and a sheet on small touch
//! screens.
//!
//! An app makes a [`Combobox`] with `cx.new(|cx| Combobox::new(options,
//! window, cx))` and renders its parts: [`Combobox::input`], the native text
//! field the combobox makes, and [`Combobox::content`], with
//! [`Combobox::list`] and an [`Combobox::item`] per option inside, plus
//! whichever of `label`, `trigger`, `clear_part`, `chips`, `chip`,
//! `chip_remove`, `chips_overflow`, `create`, `select_all`, `group`,
//! `group_label`, `separator`, `empty`, `status`, `item_text`,
//! `item_indicator`, `item_icon`, `value_icon`, `arrow`, `viewport` and
//! `header` it needs. [`Combobox::value`] hands out the value's labels for
//! chips or a summary.
//!
//! # TypeScript file → Rust module
//!
//! `combobox.ts` → [`combobox`](mod@combobox): the options, props and part
//! states, and the agent surface; `combobox-api.ts` → [`combobox_api`];
//! `combobox-keyboard.ts` → [`combobox_keyboard`]; `combobox-parts.ts` →
//! [`combobox_parts`]; `combobox-chips.ts` → [`combobox_chips`]; the module's
//! exports → these re-exports.
//!
//! # In gpui
//!
//! - The input is the native crate's text field, whose element is the
//!   `input` part: it takes the combobox role over the field's, keeps focus
//!   while arrows move the highlight, and points `Aria::active_descendant`
//!   at the highlighted option's `option_id`. The field is told the list is
//!   open (`set_expanded`). What the original passes through to the native
//!   `<input>` are options: `placeholder` and `field_style`, with
//!   `autocomplete`, `autocapitalize` and `spellcheck` off.
//! - The browser's own work on the input is the combobox's: Enter that
//!   nothing took submits its form implicitly, and it reports its value to
//!   its form through its internals (the lists crate's `hidden_input_part`),
//!   one key, or every key under its name with `multiple`. A Field disables
//!   and names it the way the original's adapters fold the Field's
//!   `disabled` and `name` into its options.
//! - The list opens through the popups crate's `combobox_preset`, placed
//!   below the chips while they're in the page, else the input, and binds as
//!   the lists crate's `list_content_config` says. The content reveals the
//!   highlighted item in itself, so make it the element that scrolls.
//! - The original's store subscribers run where each change happens: a
//!   value arriving shows its label, the highlight follows the list once
//!   items render, and result counts are announced once typing pauses.
//! - Items tell the list their label: gpui can't read the text an item
//!   renders, so an item without a `label` uses its value's key, which is
//!   what typing filters by and what the input shows once it's chosen. It
//!   isn't the item's accessible name: give that as `aria_label`.
//! - Chips collapse into the overflow part from the bounds the parts laid
//!   out with, deciding once the frame that drew the overflow part is over.
//!   The chips' padding, borders and gap and the input's minimum width,
//!   which the original reads with `getComputedStyle`, are what the app
//!   styled those parts with.
//! - In a sheet, the app renders the input inside the `header` part while
//!   it's there, the original's portal into the header. A tap that opens a
//!   sheet doesn't focus the input; the input's focus in a sheet grows it to
//!   its largest snap point, which the original's sheet does on `focusin`.
//! - The create option's `on_create` returns an `ActionOutcome`; pending
//!   work marks it pending and is announced after a second.
//!
//! # Dropped, and why
//!
//! - `createComboboxController`, `ComboboxController` and the `controller`
//!   option: the entity is the controller, and its methods are its api.
//! - `combobox-generic.ts`, `ComboboxPrimitive`, `ComboboxChangeOptions` and
//!   `ItemsOf`: types renderers and TypeScript read. The value type is a
//!   type parameter, and changes take the kernel's `ChangeOptions`.
//! - The `root` part (`rootPart`) and the `form-input` and `value-input`
//!   parts: a web component's element and the hidden inputs, which the
//!   internals stand for. `container` (`PortalProps`): `deferred` replaces
//!   portals.
//! - The listing (`keepListing`, `unreadByItems`, `selects`), which lets an
//!   option render only when its own entry changed: gpui renders a
//!   component's parts together.
//! - `learnLabels`, `adoptValue`, `restore`, `takeTypedText` and
//!   `afterBind`: labels found among an adapter's children, web components'
//!   keys, a browser's restore, text typed before the page's script ran, and
//!   Vue's hydration.
//! - `labelClicked`: a native label passing its click to the input. The
//!   label part focuses the input itself.
//! - `pageLocale` and `bindPageLocale`: the value's labels read the app's
//!   language as they're formatted (`locale_of`).
//! - The `text` props of the create and overflow parts, which write "Create
//!   “…”" and "+N more" for web components.
//!
//! # Deviations
//!
//! - `allow_custom_value` keeps text as the value when the value type holds
//!   text; for another type, `custom_value` says what text becomes.
//! - The api's `clear` and the `clear` slot share a name, so the part is
//!   `clear_part`; the create option's runner is `create_item`.
//! - An item's `group` is the key of the group part around it, which the app
//!   passes, where renderers fill it in.
//! - An item that's a link says so with `link`, where the original finds an
//!   `a[href]`; Enter clicks it (`el.click()`), and the app follows it in its
//!   own press on the part.
//! - `min_length` counts characters, not UTF-16 code units.

pub mod combobox;
pub mod combobox_api;
pub mod combobox_chips;
pub mod combobox_keyboard;
pub mod combobox_parts;

pub use combobox::*;
pub use combobox_api::{CREATE, Combobox, ComboboxItemValue};
pub use combobox_chips::{ChipsLayout, ChipsOverflow, chips_that_fit};
pub use combobox_parts::{ComboboxChips, ComboboxContent};
pub use ui_sdk_lists::{
    AsyncOptionsStatus, ListAnnouncement, ListData, ListFilter, ListGroup, ListHighlightDetails, ListItem,
    ListNode, ListSeparator, ListValue,
};
pub use ui_sdk_text::MatchSegment;
