//! Port of `foundation/canvas`: panning and zooming a canvas larger than
//! the screen, and connecting its ports.
//!
//! | Original | Here |
//! | --- | --- |
//! | `viewport.ts` | [`viewport`]: [`CanvasViewport`], [`create_viewport`], [`Viewport`], [`CanvasRect`] |
//! | `connect.ts` | [`connect`]: [`Connect`], [`create_connect`], [`connect_message`] |
//!
//! # In gpui
//!
//! Each behavior is a handle the component makes in its constructor and
//! keeps in a field; where the original reads and writes a state key, it
//! keeps the state itself or reads the component through its options, and
//! calls that may change something take the component, `this`. Drags follow
//! the pointer through a drag session from `ui-sdk-drag-and-drop`, whose
//! listener the component renders on every frame. The surface's box comes
//! from measuring it ([`CanvasViewport::surface`]), and the ports' from the
//! kernel [`Parts`](ui_sdk_kernel::Parts) of their slot; arrows go to the
//! nearest port through `ui_sdk_focus::nearest_in_direction`.
//!
//! # Deviations and drops
//!
//! - The world's transform: gpui has no transforms, so
//!   [`CanvasViewport::world`] places the world at the pan and scales the
//!   rem size inside it by the zoom. What's inside, sized and placed in
//!   [`CanvasViewport::units`], scales; lengths in pixels don't.
//! - `bindFindable` and the `find-in-page` reason: gpui has no find in page,
//!   and nothing scrolls a pane to bring an element into view. A focused
//!   item brings itself into view with [`CanvasViewport::reveal`].
//! - Two-finger pinches on the surface: gpui follows one touch at a time and
//!   doesn't recognize pinches, so fingers can't zoom the canvas. A trackpad
//!   pinch comes as gpui's `PinchEvent`: [`CanvasViewport::pinch`] zooms at
//!   its center and reports `pinch`, with the event as `InputEvent::Pinch`,
//!   where the original hears a Ctrl wheel and reports `wheel`.
//! - The wheel's page `deltaMode`: gpui reports lines or pixels.
//! - The press's `preventDefault` on the surface: what it stops in a
//!   browser, text selection and native dragging, gpui doesn't do, and the
//!   surface still takes focus, for the zoom keys.
//! - Ports' keys are generic (`K`), where the original's are strings; a
//!   port without a `label` is named by its `Display`.

pub mod connect;
pub mod viewport;

pub use connect::{
    CanConnectFn, Connect, ConnectDetails, ConnectDraft, ConnectFn, ConnectMessage,
    ConnectMessageFn, ConnectOptions, ConnectReason, ConnectRole, LabelFn, PointFn, RadiusFn,
    connect_message, create_connect,
};
pub use viewport::{
    CanvasRect, CanvasViewport, FitOptions, SetViewportFn, Viewport, ViewportDetails, ViewportFn,
    ViewportOptions, ViewportReason, ZoomFn, ZoomMessageFn, create_viewport,
};
