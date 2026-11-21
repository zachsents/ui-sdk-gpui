//! Port of `foundation/direct-manipulation`: handling things directly:
//! moving and resizing objects, swiping a row aside to its actions,
//! pressing and holding, picking a point in two dimensions, drawing ranges
//! on tracks, and trying an option before choosing it.
//!
//! | Original | Here |
//! | --- | --- |
//! | `area.ts` | [`area`]: [`Area`], [`create_area`] |
//! | `hold.ts` | [`hold`]: [`Hold`], [`bind_hold`] |
//! | `repeat.ts` | [`repeat`]: [`RepeatPresses`], [`bind_repeat_presses`], [`Repeat`], [`bind_repeat`] |
//! | `resize.ts` | [`resize`]: [`Resize`], [`create_resize`], [`resize_box`], [`resize_cursors`], [`ResizeHandle`] |
//! | `move.ts` | `r#move`: [`Move`], [`create_move`] |
//! | `spans.ts` | [`spans`]: [`Spans`], [`create_spans`], [`stack_spans`], [`is_span_free`] |
//! | `swipe-actions.ts` | [`swipe_actions`]: [`SwipeActions`], [`create_swipe_actions`] |
//! | `preview.ts` | [`preview`]: [`Preview`], [`create_preview`] |
//! | `index.ts` | these re-exports |
//!
//! # In gpui
//!
//! Each behavior is a handle the component makes in its constructor, keeps
//! in a field and clones before a call that takes the component, like
//! `self.area.clone().keydown(self, event, window, cx)`, as drag and drop's
//! are. Callbacks run inside the component's update with the component, and
//! every change goes through a `set` option with the kernel's
//! [`ChangeOptions`](ui_sdk_kernel::ChangeOptions), so it hands straight to
//! `Controllable::change`. The original's state keys become what a behavior
//! reads from the component through its options (`value`, `boxes`) and the
//! drafts it keeps itself (`draft`, `open`, the preview), which render as
//! they change: the original writes some drafts straight to the element so a
//! drag renders nothing, and here the component renders again.
//!
//! Pointer drags are drag and drop's sessions: render each behavior's
//! `listener` on every frame, and hand it presses from `on_pointer_down`,
//! which the `bind_*` methods wire, keeping the element's own position. A
//! drag claims its touch as it starts, the original's `touch-action: none`.
//! A hold and a swipe claim nothing, so taps and scrolls still work;
//! [`hold`] and [`swipe_actions`] say how they hear a finger. What the
//! original reads from the page, like tracks, rows, actions, options or a
//! surface, joins as it renders and records where it lays out, and a part
//! inside another marks the events it sees, so the one around it leaves
//! them alone, which the original asks with `closest(..)`.
//!
//! Keys match with the kernel's [`Key`](ui_sdk_kernel::Key), right to left
//! is the app's direction ([`ui_sdk_platform::is_rtl`]), and screen readers
//! hear changes through [`announce`](fn@ui_sdk_platform::announce),
//! assertively where the original is. Styles the original writes as CSS
//! strings are small structs whose `apply` places an element
//! ([`AreaThumbStyle`], [`SpanStyle`], [`ResizeBoxStyle`]), and its
//! translates are an element that draws its child moved, its layout where
//! it was ([`Move::follow`], [`SwipeActions::content`]).
//!
//! # Deviations and drops
//!
//! - Keys are generic, `K`, in move, resize, swipe actions and preview,
//!   where the original's are strings; one with no label is named by its
//!   `Display`. Spans keep string ids and tracks, and [`SpanLike`] stands for
//!   `S extends Span`.
//! - The `context` argument and `onDestroy`: a behavior goes with its
//!   component, and a drag a teardown cuts short stops without `on_end`, as
//!   drag and drop's sessions do.
//! - `deriveState` and `settleDraft`: a draft follows a value the owner
//!   changed the next time it's read, which rendering and every change do,
//!   and a dropped one clears once its value arrives, or a task later.
//! - CSS: inline styles, custom properties and `data-*` attributes are
//!   fields and `apply` methods. `-webkit-touch-callout`: gpui has no
//!   callout. The menu a long press opens, which a hold keeps closed for a
//!   finger, is a tooltip in gpui, which it keeps closed the same way.
//! - A drag claims its touch as it starts, so on a touch screen a tap on a
//!   moved item, a resized box or a span clicks nothing, where the original
//!   still clicks under `touch-action: none`; drag and drop's sessions
//!   decide what a touch is as it starts.
//! - Pointer ids, `isPrimary` and second fingers: gpui follows one touch at
//!   a time. `"pen"` isn't an input: gpui reports a pen as a mouse or a
//!   finger.
//! - `hold`: a short press of a key other than Enter or Space clicks
//!   nothing, since gpui has no `click()`; `on_end` hears it, `held` and
//!   `canceled` false. The click after Enter or Space is gpui's own keyboard
//!   click, which gpui skips with a modifier held or when another key goes
//!   down or up mid-press, where `el.click()` always clicks. A finger's
//!   moves are heard only once an element claims the touch; before that, a
//!   pan past gpui's touch slop cancels the press, as `pointercancel` does,
//!   and a touch the system takes before gpui decides what it is ends the
//!   press at the next touch. A held touch claims the long press gpui
//!   offers, so its finger lifting taps nothing; a scroll after it no longer
//!   cancels.
//! - `repeat`: `bindRepeat`'s `click` listener is [`Repeat::click`], which
//!   the part's `on_press` calls; a click with no press behind it
//!   (`detail === 0`) is a keyboard click or an accessibility action. A
//!   press's first step that stops it ([`RepeatPresses::stop`]) ends the
//!   press, as the original's unbinding does.
//! - `swipe-actions`: a finger swipes through gpui's pans, not a pointer
//!   session, which would keep the list from scrolling. Hidden actions'
//!   `inert` is [`SwipeActionsAttrs::inert`], for the app to take their
//!   buttons out of the tab order, and [`SwipeActions::show`] with focus
//!   focuses the first after the frame that renders them shown.
//! - `spans`: a key while something inside the span has focus is that
//!   thing's, where the original asks whether it's a field. `valueAt` takes
//!   a point. Moves' `preventDefault` and auto-scroll's `ignore` have
//!   nothing to do here, as drag and drop says.
//! - `preview`: the `slot` option, since options join through
//!   [`Preview::bind_option`]; the `key` state is the preview's own, or the
//!   component's through `current`.
//! - `resize`: `resizeCursors` gives gpui's cursors, with a closed hand for
//!   `move`, as gpui's `cursor_move`; `boxStyle` places the box as it shows,
//!   the draft included.
//! - `area`: the thumb's focus without scrolling: gpui never scrolls to
//!   focus.
//! - Messages write numbers as JavaScript does, like `540`, `1.5` and
//!   `1e-7`.
//!
//! Ported.

mod js;
mod target;
mod translate;

pub mod area;
pub mod hold;
pub mod r#move;
pub mod preview;
pub mod repeat;
pub mod resize;
pub mod spans;
pub mod swipe_actions;

pub use area::{
    AREA_KEYS, Area, AreaAxis, AreaCommitFn, AreaOptions, AreaReason, AreaTextFn, AreaThumbAttrs,
    AreaThumbStyle, AreaValue, SetAreaFn, create_area,
};
pub use hold::{
    BindHoldOptions, Hold, HoldDetails, HoldEndDetails, HoldEndFn, HoldEvent, HoldFn, HoldInput,
    HoldPressFn, bind_hold,
};
pub use r#move::{
    Move, MoveBounds, MoveDraft, MoveMessage, MoveOptions, MovePoint, MoveReason, MoveStyle,
    SetPositionsFn, create_move, move_message,
};
pub use preview::{ChooseFn, Preview, PreviewOptions, PreviewReason, SetPreviewFn, create_preview};
pub use repeat::{
    BindRepeatOptions, Repeat, RepeatDetails, RepeatEndDetails, RepeatEndFn, RepeatInput,
    RepeatPresses, RepeatStepFn, bind_repeat, bind_repeat_presses,
};
pub use resize::{
    Resize, ResizeBounds, ResizeBox, ResizeBoxStyle, ResizeDraft, ResizeHandle, ResizeHandleAttrs,
    ResizeLimits, ResizeMessage, ResizeOptions, ResizeReason, ResizeSize, SetBoxesFn,
    create_resize, resize_box, resize_cursors, resize_message,
};
pub use spans::{
    CreateSpanFn, SetSpansFn, Span, SpanAction, SpanAttrs, SpanChange, SpanEdge, SpanEditFn,
    SpanFn, SpanLane, SpanLike, SpanPatch, SpanStyle, Spans, SpansDraft, SpansMessage,
    SpansMessageType, SpansOptions, SpansReason, create_spans, is_span_free, spans_message,
    stack_spans,
};
pub use swipe_actions::{
    FullSwipeFn, SwipeActions, SwipeActionsAttrs, SwipeActionsOptions, SwipeContentAttrs,
    SwipeDraft, SwipeOpen, SwipeSide, create_swipe_actions,
};
