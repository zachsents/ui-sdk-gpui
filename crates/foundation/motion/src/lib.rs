//! Port of `foundation/motion`: transitions that follow state, and the
//! physics of gestures.
//!
//! Motion follows state. When state changes, a part's transitions run, and it
//! stays in the page until they finish. Someone who prefers reduced motion
//! gets the end at once.
//!
//! - Presence ([`Presence`]): a part's enter and exit animations, the
//!   original's `animateIn` and `animateOut`. The original waits for CSS;
//!   here the app supplies the timing its CSS would declare, the presence
//!   runs it on animation frames, and parts style from its state: phase,
//!   starting and ending styles, `inert`, and how far it shows.
//! - Panels ([`Panel`]): a panel's opening and closing, `bindPanel`: its
//!   presence, a size that animates between 0 and its content's own, and
//!   focus out of a panel closing around it.
//! - The sheet: its state machine ([`next`]), the content scroll hand-off,
//!   snap points, physics presets and styling curves, under Drawer and the
//!   popups' sheets.
//! - Gestures ([`gesture`]): velocity tracking, projection, springs, the
//!   rubber band, and which way a stroke points.
//!
//! The physics is the original's math, in `f64`, with its NaN and rounding
//! where they show. gpui's own springs and animations work in `f32` with
//! other parameters, so drivers sample these. Times are ms on the input
//! clock, [`timestamp`], since gpui's events carry no time.
//!
//! Dropped from the original:
//! - `writeOpen`, which writes `data-open` or `data-closed` ahead of the
//!   framework's render, so exit keyframes keyed on them run when
//!   `animateOut` reads them. Parts render from state in the frame it
//!   changes, so they take `open` from their component's state.
//! - `bindFindable`, and `hidden="until-found"` in `hiddenAttrs` and
//!   `bindPanel` (`isFindable`, `reveal`): find in page has no gpui meaning.
//!   [`supports_hidden_until_found`] stays, always false, so callers take
//!   their path for content find in page can't reveal.
//! - `stateAndRenders`, a panel's subscription to its store and its part's
//!   renders: the owner calls [`Panel::check`] or [`Panel::sync`] after its
//!   state or the panel's options change.
//! - What presence reads from the page: computed durations, delays and
//!   iteration counts on the element and its `::backdrop`, running
//!   animations, and a `MutationObserver` that keeps an owner's own `inert`.
//!   Timing comes from [`PresenceTiming`] and [`Presence::track_animation`],
//!   and a presence's `inert` is its own, so an owner's is never overwritten.
//! - `bindPanel`'s `size` callback, which writes CSS variables: [`PanelState`]
//!   carries the size, already animated, for the panel to take.
//! - Finishing every animation on the frame before a panel's first paint:
//!   nothing animates on a first paint in gpui.
//! - `measured`, the devtools' mark on a write that comes from reading the
//!   page, like an exit with nothing to animate finishing at once.
//! - `springLinearEasing` returns a [`LinearEasing`], which writes as the
//!   same CSS `linear()` string and samples as an easing for gpui's
//!   animations.

pub mod content_scroll;
pub mod curves;
pub mod gesture;
pub mod handoff;
mod js;
pub mod machine;
pub mod panel;
pub mod physics;
pub mod presence;
pub mod projection;
pub mod rubber_band;
pub mod sector;
pub mod snap_points;
pub mod spring;
pub mod velocity;

pub use content_scroll::ContentScroll;
pub use curves::{dim_amount, expand_amount, progress_amount};
pub use handoff::{HandoffInput, MoveOwner, handoff};
pub use machine::{
    PointerType, ScrollContext, SheetConfig, SheetDismissReason, SheetEffect, SheetEvent,
    SheetMotion, SheetPhase, SheetSnapPoint, SheetSnapReason, SheetState, current_motion,
    initial_state, largest_extent, next, snap_extents, start_dragging,
};
pub use panel::{
    Opening, Panel, PanelBinding, PanelExitedCallback, PanelSettledCallback, PanelState,
    hidden_attrs, supports_hidden_until_found,
};
pub use physics::{SHEET_PHYSICS, SheetPhysics, SheetPhysicsPresets};
pub use presence::{
    AnimationId, Easing, Presence, PresencePhase, PresenceState, PresenceTiming, Timing,
};
pub use projection::{SnapTargetInput, choose_snap_target, nearest_index, project_distance};
pub use rubber_band::{rubber_band, rubber_band_with};
pub use sector::{SectorOptions, sector_of};
pub use snap_points::{
    ResolvedSnapPoint, SnapPoint, SnapPointContext, SnapPointKey, find_snap_index,
    is_at_or_below, resolve_snap_points,
};
pub use spring::{
    LinearEasing, SettleTimeOptions, SpringSolver, settle_time, solve_spring,
    spring_linear_easing,
};
pub use velocity::{VelocityTracker, VelocityTrackerOptions, create_velocity_tracker, timestamp};
