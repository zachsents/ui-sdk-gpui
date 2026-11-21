//! Port of `foundation/focus`: one tab stop for a group with arrow keys
//! between its items, the same for a grid's cells and for items laid out
//! anywhere, what Tab can reach, and focus that survives the part holding it
//! leaving or moving.
//!
//! | Original | Here |
//! | --- | --- |
//! | `collection.ts` | [`collection`]: key steps, orientation, the tab stop rule |
//! | `roving-focus.ts` | [`roving_focus`]: [`RovingFocus`] |
//! | `roving-scope.ts` | [`roving_scope`]: [`RovingMember`], [`RovingScope`] |
//! | `grid-focus.ts` | [`grid_focus`]: [`GridFocus`] |
//! | `spatial.ts` | [`spatial`]: [`nearest_in_direction`], [`spatial_move`] |
//! | `focus-recovery.ts` | [`focus_recovery`]: [`FocusRecovery`] |
//! | `keep-focus.ts` | [`keep_focus`](mod@keep_focus) |
//! | `press-keeps-focus.ts` | [`press_keeps_focus`](mod@press_keeps_focus) |
//! | `tabbable.ts` | [`tabbable`]: [`tabbables`], [`next_tabbable`], [`tab_within`], [`TabStop`], [`bind_tab_keys`] |
//!
//! # In gpui
//!
//! The original binds each part's element and reads the DOM. Here the
//! members of a group are the parts a component renders: each binds as it
//! renders, which is page order, with the focus handle its element tracks
//! and what the group needs to know of it, like [`RovingFocus::bind`], and
//! the part that holds them calls `begin`. A part that stops rendering has
//! left. Once the frame that rendered them is
//! drawn, the behavior settles, as the original's work a microtask after
//! binding does: it moves the stop where its rules put it, notes where focus
//! is, which gpui only tells when it draws, and focuses an item a move was
//! waiting for. Behaviors take the window the component renders in for
//! that.
//!
//! Tab order is gpui's tab stops: a handle tracked with `tab_stop(true)`,
//! ordered by `tab_index` within each `tab_group`. Browsers move focus with
//! Tab; an app calls [`bind_tab_keys`] once for the same. [`tabbables`] and
//! the rest read the order Tab moves in, inside a container's focus handle.
//!
//! Keys are matched with [`ui_sdk_kernel::Key`], and right to left is the
//! app's direction, [`ui_sdk_platform::is_rtl`].
//!
//! The original imports `afterBind` from the kernel, which the kernel crate
//! drops with Vue's hydration; its timing is here, as a task gpui runs once
//! the frame that rendered the parts is drawn. The groups keep their own
//! page order rather than the kernel's [`Parts`](ui_sdk_kernel::Parts),
//! since a member may bring the focus handle its element tracks, like a
//! toggle in a toolbar, and each bind says what the item is now, which is
//! the original's `onProps`.
//!
//! # Dropped
//!
//! - `createRovingFocus`'s `slot` and `key` options: items bind themselves
//!   rather than through a slot, and the group keeps the stop
//!   ([`RovingFocus::stop`], [`RovingFocus::reset`]) rather than a state key.
//! - Its `disabled`, `focusable`, `preferred` and `claims` callbacks: each
//!   item says them as it renders ([`RovingMember`]), since a callback kept
//!   in the group can't read the component it lives in. A change that moves
//!   the stop before the items render it, like a radio group's new value,
//!   hands [`RovingFocus::check_with`] which items are preferred now, as the
//!   original's `check` reads `preferred`.
//! - `memberDisabled` and `memberFocusable`, which read members' callbacks:
//!   members pass values, which the group reads itself.
//! - `byCell`, a `selects` reader so a move renders only the two cells it
//!   touches: gpui has no subscriptions per part; parts render from their
//!   entity's state.
//! - `positiveTabStops`: in the DOM a positive `tabindex` comes before the
//!   whole page, so a trap steps onto and off those itself. gpui orders
//!   `tab_index` within each tab group, which [`tabbables`] returns, and a
//!   trap moves with [`tab_within`].
//! - `radioGroup` and `isTabbable`'s rule that one radio per group takes Tab:
//!   gpui has no native radios; a radio group's one tab stop is roving focus.
//! - `focusables`, a selector for what can take focus: gpui can't list
//!   elements that take focus without being tab stops. [`tabbables`] lists
//!   those that are, and focus is a handle that stays with its element when
//!   it moves, what the drag and drop and inline edit code it served needs.
//! - `isTabbable`'s `:disabled`, `inert` and visibility checks, and
//!   `keepFocus`'s: gpui has no such states; an element takes Tab when its
//!   handle is a tab stop and it rendered.
//! - `bindTabStop`'s `skipWhenHolding`, for Firefox making every scroller a
//!   tab stop, and its keeping a `tabindex` the author wrote: neither exists
//!   in gpui. Its `MutationObserver` becomes Tab checking what's inside the
//!   container when it gets there.
//! - `createGridFocus`'s `key` option: the grid keeps the active cell
//!   ([`GridFocus::active`], [`GridFocus::set_active`]).
//! - `preventScroll`: focusing never scrolls in gpui, so where the original
//!   lets focus scroll a grid's cell into view, `reveal` does.
//! - Rendering on the server: `claims` keeps the first frame to one tab stop
//!   instead, before the items have settled.

mod after_bind;
pub mod collection;
pub mod focus_recovery;
pub mod grid_focus;
pub mod keep_focus;
pub mod press_keeps_focus;
pub mod roving_focus;
pub mod roving_scope;
pub mod spatial;
pub mod tabbable;

pub use collection::{
    Arrows, Direction, FieldCaret, Orientation, RovingCandidate, Step, field_keeps_key, in_order,
    move_index, orientation_of, radio_step, roving_stop, scroll_to_show, step_for,
};
pub use focus_recovery::{FocusRecovery, LostFocus};
pub use grid_focus::{
    GridCell, GridDisabledFn, GridFocus, GridFocusDetails, GridFocusOptions, GridFocusReason,
    GridMoveDetails, GridMoveFn, GridRevealFn,
};
pub use keep_focus::keep_focus;
pub use press_keeps_focus::{hold_focus, press_keeps_focus};
pub use roving_focus::{
    Claim, ItemsChangeFn, RovingFocus, RovingFocusItem, RovingFocusMoveContext, RovingFocusOptions,
    RovingMoveFn, RovingMoved, RovingOrientation, create_claim,
};
pub use roving_scope::{RovingMember, RovingScope, follow_stop, queued};
pub use spatial::{
    SpatialMoveOptions, SpatialRect, SpatialRectFn, nearest_in_direction, spatial_move,
};
pub use tabbable::{
    TabStop, bind_tab_keys, bind_tab_stop, composed_tabbables, is_tabbable, next_tabbable,
    tab_within, tabbables,
};
