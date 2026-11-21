//! Port of `components/scroll-area`: a scrolling region with scrollbars that
//! match the app's design, edge indicators, and following the end as content
//! grows.
//!
//! | Original | Here |
//! | --- | --- |
//! | `scroll-area.ts` | [`scroll_area`]: [`ScrollArea`], its options, parts and states |
//! | `scroll-area-view.ts` | [`scroll_area_view`]: the engine, [`ScrollAreaModel`], [`ScrollAreaScrollOptions`] |
//! | `scroll-area-pointer.ts` | [`scroll_area_pointer`]: thumb drags, track presses, wheels over a scrollbar |
//! | `scroll-area-model.ts` | [`scroll_area_model`]: the pure decisions |
//!
//! # In gpui
//!
//! The viewport is a gpui scroll container, `overflow_scroll` tracking the
//! scroll area's [`ScrollHandle`](gpui::ScrollHandle), so wheels, trackpads
//! and touch pans scroll it as gpui does. gpui draws no scrollbars: the
//! scrollbar and thumb parts are the only ones. Every frame the scroll area
//! renders, its parts read the handle, and the thumb takes its length and
//! offset, and the root its edge distances and progress, from the scroll
//! position then, which is what the original writes as CSS variables. After
//! the frame, the engine reads what the frame laid out, as the original's
//! resize observers and frame callback do, and patches the flags parts style
//! by: overflow, edges, scrolling, hovering, visibility and sticking.
//!
//! Sticking to the end follows growth in the frame that grows: while it
//! follows and nothing moved the view since the last frame, the viewport
//! asks gpui to put it at the bottom as it lays out. A browser also scrolls
//! a focused scroller with arrows, Page Up and Down, Home, End and Space;
//! gpui doesn't, so the viewport does, unless a text field inside has focus
//! or a widget inside used the key. Keys scroll at once, and Space only on
//! the viewport itself, so a button inside keeps its press.
//!
//! Right to left is the app's direction (`ui_sdk_platform::is_rtl`). gpui
//! lays a scroll container's content out from the left either way, so right
//! to left the horizontal position counts from the right end of the range,
//! and the view starts there, a frame after the viewport first lays out with
//! room to scroll; the app lays out mirrored content in reverse. Scrollbars,
//! the corner and the horizontal thumb mirror.
//!
//! The thumb is positioned relative to where it sits in its scrollbar, which
//! measures its own content box as it lays out: the track. The thumb's length
//! lags a frame behind the track's first layout, as content growth does in
//! the original.
//!
//! # Dropped, and why
//!
//! - `createScrollAreaController` and the `controller` option: the entity is
//!   the controller.
//! - Native scrollbars, `scrollbar-width: none` and the `counted` flag that
//!   decides it: gpui draws no native scrollbars to hide, so without
//!   scrollbar parts there are none.
//! - `registerUninherited`, the content's variable shield, and the CSS
//!   variables themselves: gpui has no CSS. The values are fields of the
//!   root's and the scrollbar's states.
//! - `touch-action` and `user-select` on the scrollbar: a drag session claims
//!   the touch, and gpui selects no text in parts.
//! - `scaleOf`: gpui has no transforms or `zoom` to scale a scrollbar.
//! - `suspendSnap`: gpui has no scroll snapping to suspend during a drag.
//! - `scrollsInside`, which walks from an event's target to the viewport for
//!   a scroller that still scrolls toward its start: gpui can't walk from a
//!   target. Upward input lets go of the end; a text field that scrolls its
//!   own text takes the wheel itself.
//! - Pages that follow the viewport's `scroll-behavior`: gpui has none, so a
//!   track press pages at once.
//! - The 16px line a Firefox wheel reports: gpui gives wheel lines in the
//!   window's line height, which a forwarded wheel scrolls by, as the
//!   viewport itself does.
//! - The `inert` attribute on a hidden scroll-to-end button: gpui can't make
//!   a subtree inert, so the part takes no focus or presses and is hidden
//!   from assistive technology.

pub mod scroll_area;
pub mod scroll_area_model;
pub mod scroll_area_pointer;
pub mod scroll_area_view;

pub use scroll_area::{
    ScrollArea, ScrollAreaOptions, ScrollAreaOrientation, ScrollAreaScrollToEndState,
    ScrollAreaScrollbar, ScrollAreaScrollbarElement, ScrollAreaScrollbarState, ScrollAreaState,
    ScrollAreaThumbState,
};
pub use scroll_area_model::{Axis, AxisMetrics, ScrollAreaType};
pub use scroll_area_view::{ScrollAreaModel, ScrollAreaScrollOptions, ScrollBehavior};
