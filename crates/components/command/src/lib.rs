//! Port of `components/command`: Command, a search field over an always-open
//! list of commands, like a command palette. Typing filters and ranks the
//! commands, arrow keys move a highlight while focus stays in the field,
//! Enter runs the highlighted one, and commands can open pages of more
//! commands. It's the lists crate's autocomplete core rendered inline.
//!
//! `command.ts` → [`command`](mod@command), `command-api.ts` →
//! [`command_api`], `command-data.ts` → [`command_data`], `command-parts.ts`
//! → [`command_parts`], `index.ts` → these re-exports.
//!
//! An app makes a [`Command`] with `cx.new(|cx| Command::new(options, window,
//! cx))` and renders [`Command::root`] with [`Command::input`] (named with
//! `aria_label`), [`Command::empty`] and [`Command::list`] inside, and in the
//! list a [`Command::item`] per command, with [`Command::item_text`],
//! [`Command::item_icon`] and [`Command::shortcut`] inside, in
//! [`Command::group`]s under a [`Command::group_label`], with
//! [`Command::separator`]s and [`Command::status`]. Commands on a page sit in
//! that [`Command::page`], which is `None` unless it's the current page. Data
//! commands (`items`) render from the list's `state.items`, each with
//! [`CommandItemProps::from_data`]; one with `items` of its own opens them as
//! a page.
//!
//! # In gpui
//!
//! - The search field is the native crate's text field, which the command
//!   makes, with its completion, capitals and spelling marks off, as the
//!   field part has them. Its placeholder and colors are options.
//! - A command's `value` is required, since a part needs a key its app
//!   gives, and its `label` is what the search matches and what names the
//!   option, since gpui can't read the text an element shows.
//! - The highlight is `Option<SharedString>`, so a controlled `value` is
//!   `Some(None)` to highlight nothing.
//! - A page part is `None` while it isn't on top of the stack, so its
//!   commands don't render, the original's `present` and `inPage`; an item
//!   names its group in its props, as the original's renderers pass it.
//! - `on_action` and `on_select` return an [`ActionOutcome`](ui_sdk_kernel::ActionOutcome)
//!   where the original reads a returned Promise: `Pending` makes the command
//!   pending until its work settles, the first failure settling it, as
//!   `Promise.all` does; `Failed` is its error at once; `Canceled` from
//!   `on_select` is `prevent_default()`.
//! - Data commands' shortcuts register while the root renders, again when
//!   the data changes, and composed ones while their item renders; both
//!   belong to the search field's layer, so a command in a dialog runs its
//!   shortcuts while the dialog is open. Their keys are named for
//!   `is_apple(cx)` as parts render, the original's `apple` state.
//! - Revealing the highlighted command scrolls the scroll handle the list
//!   part tracks ([`Command::scroll_handle`]); the app makes the list scroll.
//! - The highlight settles once the frame that rendered the commands is
//!   drawn, as the lists crate's core does. A composed page that opens waits
//!   for its commands, which the core counts as they join and leave
//!   (`mounts`).
//! - A command that's a link says so with `link`, where the original finds
//!   an `a[href]`; Enter clicks it (`el.click()`), and the app follows it in
//!   its own press on the part.
//! - `on_value_change` gets a [`CommandValueChangeDetails`], which derefs to
//!   the kernel's `ChangeDetails` and carries `index`.
//! - `set_options` takes the window, since a controlled value is
//!   highlighted, and new text written into the field, at once.
//!
//! # Dropped, and why
//!
//! - `createCommandController`, `CommandController` and the `controller`
//!   option: the app holds the entity, whose methods are the controller's
//!   (`run` is [`Command::run`]).
//! - `filterAttribute`: an attribute for web components.
//! - `bindFieldName` and the field name: the list is labelled by the search
//!   field, which accesskit names by the field's own label, never its text.
//! - `selects`, `unreadByItems`, `live` and `present`: how adapters render
//!   only what changed. Parts render from the entity's state.
//! - `syncPlatform`, `adoptPlatform` and the `apple` state: gpui knows the
//!   platform from the first frame.
//! - The `mounts`, `settled`, `focusInList`, `groupLabels` and `listing`
//!   state: the lists crate's core, its `ListSettled` and the kernel's bound
//!   parts keep them.

pub mod command;
pub mod command_api;
pub mod command_data;
pub mod command_parts;

pub use command::{
    Command, CommandAction, CommandActionDetails, CommandChangeOptions, CommandInputState, CommandItemProps,
    CommandItemState, CommandListState, CommandOptions, CommandPagesChangeReason, CommandQueryChangeReason,
    CommandSelect, CommandShortcutProps, CommandShortcutState, CommandState, CommandValueChange,
    CommandValueChangeDetails, CommandValueChangeReason,
};
pub use command_data::{
    CommandData, CommandDataFields, CommandDataGroup, CommandDataItem, CommandDataItemExt, CommandShortcut,
    command, command_shortcuts, find_command, page_items,
};
pub use ui_sdk_lists::{AsyncOptionsStatus, ListAnnouncement, ListSeparator, SearchFilter};
pub use ui_sdk_text::MatchSegment;
