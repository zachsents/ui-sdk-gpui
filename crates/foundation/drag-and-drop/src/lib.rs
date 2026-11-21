//! Port of `foundation/drag-and-drop`: sorting a list or a grid, nesting a
//! tree, moving items between lists, with a pointer or the keyboard, and
//! the pointer gestures under them: following one pointer until it lifts,
//! repeating while a press is held, a copy that follows the pointer, and
//! scrolling near an edge.
//!
//! | Original | Here |
//! | --- | --- |
//! | `pointer.ts` | [`pointer`](mod@pointer): [`DragSession`], [`bind_drag`], [`DRAG_THRESHOLD`], [`is_drag`], [`is_drag_press`], [`swallow_next_click`], [`repeat_while_held`], [`on_pointer_down`] |
//! | `reorder.ts` | [`reorder`]: [`Reorder`], [`create_reorder`], [`reorder_message`], [`drop_index`], [`is_same_keys`] |
//! | `tree-reorder.ts` | [`tree_reorder`]: [`TreeReorder`], [`create_tree_reorder`], [`subtree_end`] |
//! | `transfer.ts` | [`transfer`]: [`Transfer`], [`create_transfer`], [`transfer_message`] |
//! | `draft-move.ts` | [`draft_move`]: the draft, drop and cancel the three share |
//! | `drag-preview.ts` | [`drag_preview`]: [`DragPreview`], [`bind_drag_preview`] |
//! | `auto-scroll.ts` | [`auto_scroll`]: [`AutoScroll`], [`create_auto_scroll`] |
//! | `index.ts` | these re-exports |
//!
//! # In gpui
//!
//! A drag follows the pointer through window listeners an element registers
//! as it paints: a [`DragSession`]'s [`listener`](DragSession::listener),
//! which the component renders on every frame, hears every move and the
//! release wherever the pointer goes, as the original's pointer capture and
//! document listeners do. Presses come through [`on_pointer_down`], which
//! hears a mouse button and a touch gpui offers as a drag; following a touch
//! claims it as it starts, which is what `touch-action: none` does. Callbacks
//! run inside the component's update with the component, `&mut T`, so
//! behaviors take the component where the original reads its store: each is
//! a handle the component keeps in a field and clones before a call, like
//! `self.reorder.clone().keydown(self, event, &key, window, cx)`.
//!
//! The behaviors keep their drafts, the original's `draft` state keys, and
//! read the component's value through their options. Items join as they
//! render (`bind`) and record their bounds (`measure`), what the original
//! reads from the DOM. Every change goes through the `set` option with the
//! kernel's [`ChangeOptions`](ui_sdk_kernel::ChangeOptions), so it hands
//! straight to `Controllable::change`; keyboard steps are announced through
//! `ui_sdk_platform::announce_with`, assertively, as the original does.
//!
//! The original's `isSameKeys` comes from selection's `listbox-range.ts`,
//! and selection builds on this crate, so it's defined here
//! ([`is_same_keys`]) and `ui_sdk_selection::listbox_range` re-exports it.
//!
//! # Deviations and drops
//!
//! - Pointer lock (`lock`, `PointerSample::locked`): gpui can't lock the
//!   pointer, so a locked drag falls back to plain movement, as the original
//!   does where the browser refuses.
//! - Pointer ids (`swallowNextClick`'s `pointerId`, `isSecondary`): gpui
//!   follows one mouse and one touch at a time, so there's no second finger
//!   to tell apart.
//! - `defaultPrevented` on a press: an inner element that handles a mouse
//!   press stops it propagating in gpui, so outer handlers never hear it;
//!   gpui's own `prevent_default` on a mouse press means focus moved, which
//!   doesn't stop a drag. For a touch, it means another drag claimed it.
//! - A component destroyed mid-drag: its drag stops without `on_end`, since
//!   the draft it would put back goes with it and there's no window to call
//!   it with.
//! - `startOnMove` with a touch: gpui decides what a touch is as it starts,
//!   so the session claims it then; a touch `pick` lets go doesn't scroll,
//!   and one that lifts first reaches `on_tap` rather than clicking.
//! - Clicks: gpui fires a click in the release's bubble phase, so
//!   [`swallow_next_click`] is called in the release's capture phase (a
//!   drag's `on_end`, `capture_any_mouse_up`, a window listener) or on the
//!   press with `until_release`, and needs a session's listener or
//!   [`swallow_listener`] painted in the window. gpui hands a tap over as a
//!   mouse press and release once the finger lifts, so a tap's press is the
//!   touch's own, not the next press.
//! - Transfers between lists follow the pointer through a [`DragSession`]
//!   as the original does, not gpui's typed drag (`on_drag`, `on_drop`):
//!   that starts only from a mouse past its own 2px, can't be canceled with
//!   Escape, and reports nothing when the pointer lets go outside every
//!   list, where the original still drops the item where its draft is.
//! - Auto-scroll: gpui can't list the elements under a point, and its
//!   windows don't scroll, so a drag names the [`ScrollHandle`](gpui::ScrollHandle)s
//!   it may scroll, innermost first, and the window fallback and `ignore`
//!   (the dragged element, to look past it) have nothing to do. gpui lays a
//!   scroll out as it draws the next frame, where the original's layout
//!   follows at once, so `on_scroll` runs once that frame is drawn.
//! - The drag preview: gpui can't clone an element and its computed styles,
//!   so the component renders the copy's content in
//!   [`DragPreview::render`], which places it above everything; its
//!   `data-drag-preview`, `data-drag-count` and `--drag-count` are what the
//!   behaviors' previews say.
//! - `--drag-x` and `--drag-y`: [`Reorder::follow`] moves the dragged item
//!   itself, as the translate the original's CSS applies.
//! - Keys and lists are generic (`K`), where the original's are strings;
//!   messages name a key without a label or text by its `Display`.
//! - `preventDefault` on drag moves: what it stops in a browser, selecting
//!   text and scrolling, a gpui drag doesn't do.

pub mod auto_scroll;
pub mod draft_move;
pub mod drag_preview;
pub mod pointer;
pub mod reorder;
pub mod transfer;
pub mod tree_reorder;

pub use auto_scroll::{AutoScroll, AutoScrollCallback, AutoScrollOptions, create_auto_scroll};
pub use draft_move::MessageType;
pub use drag_preview::{DragPreview, bind_drag_preview};
pub use pointer::{
    BindDragOptions, BoundDrag, DRAG_THRESHOLD, DragEndCallback, DragMoveCallback,
    DragPickCallback, DragSession, DragStartCallback, DragTapCallback, PointerEvent, PointerKind,
    PointerPhase, PointerSample, RepeatOptions, StartOnMoveOptions, bind_drag, create_drag_session,
    is_drag, is_drag_press, on_pointer_down, repeat_while_held, swallow_listener,
    swallow_next_click,
};
pub use reorder::{
    KeyFn, MessageFn, ReadFn, Reorder, ReorderDraft, ReorderItem, ReorderMessage, ReorderOptions,
    ReorderOrientation, ReorderPreview, ReorderReason, SetOrderFn, columns_of, create_reorder,
    drop_index, grid_drop_index, is_same_keys, moved_key_to, moved_to, reorder_message,
};
pub use transfer::{
    AcceptsFn, SetListsFn, Transfer, TransferDraft, TransferItem, TransferLists, TransferMessage,
    TransferMove, TransferOptions, TransferPreview, TransferReason, create_transfer,
    transfer_message,
};
pub use tree_reorder::{
    SetRowsFn, TreePlace, TreeReorder, TreeReorderDraft, TreeReorderItem, TreeReorderMessage,
    TreeReorderOptions, TreeReorderPreview, TreeReorderReason, TreeReorderRow,
    create_tree_reorder, depth_range, moved_rows, place_of, subtree_end, tree_reorder_message,
};
