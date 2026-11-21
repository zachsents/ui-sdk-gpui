//! Port of `components/resizable`: panels a person resizes by dragging the
//! handle between them or with the keyboard, collapses, and keeps the
//! layout of.
//!
//! `resizable.ts` → [`resizable`](mod@resizable), `resizable-api.ts` →
//! [`resizable_api`], `resizable-dom.ts` → [`resizable_dom`],
//! `resizable-handles.ts` → `resizable_handles`, `resizable-layout.ts` →
//! [`resizable_layout`], `resizable-storage.ts` → [`resizable_storage`]
//! (`storageKey` is [`resizable_storage_key`]), `index.ts` → these
//! re-exports.
//!
//! An app makes a [`Resizable`] with `cx.new(|cx| Resizable::new(options,
//! window, cx))` and renders [`Resizable::root`] (build it first, give it a
//! size, and render it on every frame), with a [`Resizable::panel`] per pane
//! and a [`Resizable::handle`] between them, named with `aria_label`. A
//! group nests inside a panel, like an editor above a terminal. Each
//! panel's size is a percentage of the space the panels share; the layout is
//! one per panel, in order, adding up to 100.
//!
//! A handle is the window splitter pattern's focusable separator: its value
//! is the size of the panel before it, with the bounds the panels allow, and
//! it controls that panel. Arrows along the group's axis move it by `step`,
//! or `large_step` with Shift, Home and End as far as the panels allow, and
//! Enter or a double-click collapse or restore a collapsible panel beside
//! it. A panel at its minimum passes the rest of a move on to the next one,
//! up to a disabled handle. Escape during a drag puts the panels back.
//! `on_layout_change` runs on every step, and `on_layout_commit` once the
//! interaction ends, which is also when `auto_save_id` saves.
//!
//! # Deviations
//!
//! - Panels and handles take a key, the panel's `id` and the handle's,
//!   which the original generates when left out: a part there are many of
//!   needs its key here.
//! - Sizes are a [`ResizableSize`], percent, pixels or rems, where the
//!   original takes numbers and strings with a unit;
//!   [`ResizableSize::parse`] reads those strings. A panel the methods take
//!   is a [`ResizablePanelRef`], an id or an index. Rems follow the window's
//!   rem size.
//! - Panels and handles register as they render, which is page order, and
//!   once a frame has drawn the group it reconciles them when they or their
//!   props changed, and measures the space the panels share, where the
//!   original reconciles a microtask after they bind and measures in a
//!   `ResizeObserver`. Before the first reconcile, a panel takes its
//!   `default_size` as its flex basis, as the original's server markup does.
//! - The window's hit areas: the first group to paint in a frame registers
//!   the window's listeners for every group in it, where the original
//!   listens on the document. A press counts only when the group's element
//!   is under it, as gpui's hitboxes say, so a popup above a handle keeps
//!   its presses. The listeners hear a press in its bubble phase, after the
//!   panels' content: the handle never takes focus from a press, but
//!   focusable content beside a handle, inside its hit area, can.
//! - The cursor is the window's (`set_window_cursor_style`) while a handle
//!   is hovered or dragged; `move`, for handles of two axes that meet, is
//!   gpui's open hand.
//! - Panels take the group role: gpui makes an accessibility node only for
//!   a part with one, and a handle's `aria-controls` points at the panel
//!   before it. A panel collapsed to nothing is hidden from assistive
//!   technology, the original's `inert`; gpui can't make what's inside it
//!   unfocusable, so its state's `inert` tells the app to leave focusable
//!   content out.
//! - A drag follows its pointer through the group's drag session, one drag
//!   per group, as `startDrag` allows; handles of nested groups that meet
//!   drag together, each in its group. A drag that ends some other way, like
//!   the group being disabled or its handle leaving, stops without its
//!   `on_end`, as the original's cleanup does.
//! - `storage` is a [`ResizableStorage`], implemented for
//!   `ui_sdk_storage::Kind`. The default is local storage through
//!   `ui_sdk_storage::create_local_storage`, skipped while it can't be
//!   written, like before the app names its file with
//!   `ui_sdk_storage::init`.
//! - The `layout` option controls the group through `set_options` or
//!   [`Resizable::set_controlled_layout`]; it applies once it fits the
//!   panels there are, as in the original.
//!
//! # Dropped, and why
//!
//! - `createResizableController`, `ResizableController` and the
//!   `controller` option: the app holds the entity, whose methods are the
//!   controller's.
//! - `layoutAttribute` and `sizeAttribute`: values written as attributes,
//!   for web components.
//! - `live`, `selects` and the `sizes` and `handleValues` records, so a move
//!   writes straight to the panels' styles and reaches only the parts it
//!   changes: parts render from the entity's state, and the handles' values
//!   are worked out once per change to what they depend on.
//! - `holdTouch`: gpui hands a touch to the drag that claims it as it
//!   starts, so the page can't pan it away.
//! - The page's style sheet (`setPageStyle`, `pageSheet`), which stops text
//!   selection and iframes during a drag: gpui selects no text on a drag and
//!   has no iframes.
//! - `pressedScrollbar`: gpui draws no native scrollbars.
//! - The pointer type of a hover: gpui's mouse moves carry none, so hovers
//!   use the mouse's hit area.
//! - `data-*` attributes and `--resizable-panel-size`: part state.

pub mod resizable;
pub mod resizable_api;
pub mod resizable_dom;
mod resizable_handles;
pub mod resizable_layout;
pub mod resizable_storage;

pub use resizable::{
    Resizable, ResizableChangeOptions, ResizableHandleProps, ResizableHandleState,
    ResizableLayoutChangeDetails, ResizableLayoutChangeReason, ResizableLayoutCommitDetails,
    ResizableOptions, ResizableOrientation, ResizablePanelProps, ResizablePanelRef, ResizablePanelState,
    ResizableState,
};
pub use resizable_api::HandleValue;
pub use resizable_layout::{PanelConstraints, PanelSizes, ResizableSize};
pub use resizable_storage::{ResizableStorage, storage_key as resizable_storage_key};
