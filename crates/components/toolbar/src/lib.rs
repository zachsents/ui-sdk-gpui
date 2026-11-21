//! Port of `components/toolbar`: gives buttons, toggles, menus and fields one
//! tab stop with arrow-key navigation.
//!
//! # TypeScript file → Rust module
//!
//! `toolbar.ts` → [`toolbar`] (`toolbar` is [`Toolbar`], `ToolbarApi` its
//! methods), `toolbar-fields.ts` → [`toolbar_fields`].
//!
//! # In gpui
//!
//! - The toolbar keeps its tab stop in a [`RovingFocus`](ui_sdk_focus::RovingFocus):
//!   each item binds as it renders, with the focus handle its element
//!   tracks, and the stop settles once the frame has drawn.
//! - A component rendered through a toolbar button (`render` on a
//!   `ToolbarButton`), like a menu or select trigger, is its own part,
//!   which [`Toolbar::join`] puts in the toolbar's tab stop. A text field is
//!   anything that implements `ui_sdk_text::TextField`, like the native
//!   crate's, rendered through [`Toolbar::input`].
//! - Toggles and toggle groups join through [`Toolbar::scope`], a
//!   [`RovingScope`](ui_sdk_focus::RovingScope), or [`Toolbar::scope_in`] for
//!   one in a group, whose `disabled` they see as the toolbar's.
//! - The original's adapters pass each item the `disabled` of the group it
//!   sits in, from context (`groupDisabled`). Here an item names its group by
//!   key, and the toolbar reads the group's `disabled` from its last render.
//! - What a button renders as is its `element` prop, which the original reads
//!   off the element once it binds (`elements`, `setElement`).
//! - A disabled item refuses presses and keys before its own handlers and
//!   those of a component rendered through it, as the original's capturing
//!   listeners do: its key handler runs first in the capture phase, and a
//!   press lands on a layer over it that focuses it and stops there. The
//!   window's listeners, like a popup's for a press outside it, still hear
//!   it. Keys a disabled text field keeps reach it, so its caret moves as in
//!   a read-only field; [`Toolbar::input_state`] says when to make the field
//!   read-only or disabled, which the original writes on its input. A press
//!   on a disabled field focuses it without moving its caret.
//! - Groups and separators take a key, since every part has an element id.
//!
//! # Dropped, and why
//!
//! - `createToolbarController` and the `controller` option: the entity is the
//!   controller ([`Toolbar::focus`]).
//! - `buttonKeydown` and `buttonKeyup`: gpui turns Enter and Space on a
//!   focused part into a click, and the page has nothing for Space to
//!   scroll.
//! - The `bound` flag and the `"unknown"` element before the root binds,
//!   which server markup renders with: there's no server render.
//! - The `onProps` check that queues the stop when an item's `disabled`
//!   changes: the toolbar's roving focus checks its stop once every frame
//!   that renders the items has drawn.
//! - `selects`, which narrows which parts hear a change: parts render from
//!   their entity's state.
//! - The native `type="button"`: a toolbar button takes no part in forms.

pub mod toolbar;
pub mod toolbar_fields;

pub use toolbar::{
    Toolbar, ToolbarButtonProps, ToolbarButtonState, ToolbarGroupProps, ToolbarGroupState,
    ToolbarInputProps, ToolbarInputState, ToolbarLinkProps, ToolbarLinkState, ToolbarOptions,
    ToolbarOrientation, ToolbarSeparatorState, ToolbarState,
};
