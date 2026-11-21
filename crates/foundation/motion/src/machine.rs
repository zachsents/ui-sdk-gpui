//! Port of `foundation/motion/machine.ts`.
//!
//! The sheet's gesture state machine, shared by every driver (pointer and
//! scroll snap) and by the golden-trajectory replay. It is a reducer:
//! `next(state, event, config)` returns the next state and a list of effects,
//! which drivers execute. No elements, no timers. Extents are px from the
//! attached edge, times are ms on the input clock ([`timestamp`]), and the
//! finger's `y` is its position along the sheet's axis, growing toward the
//! attached edge (down for a bottom sheet). `x` is across the axis.
//!
//! Indices that are -1 in the original, a closed sheet's resting snap point
//! and a dismissal's target, are `None` here.
//!
//! [`timestamp`]: crate::velocity::timestamp

use gpui::SharedString;
use ui_sdk_kernel::Reason;

use crate::content_scroll::ContentScroll;
use crate::js;
use crate::physics::SheetPhysics;
use crate::projection::{SnapTargetInput, choose_snap_target, nearest_index};
use crate::rubber_band::{rubber_band_inverse, rubber_band_slope, rubber_band_with};
use crate::snap_points::ResolvedSnapPoint;
use crate::spring::solve_spring;

/// A resting extent the machine knows: its id and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetSnapPoint {
    pub id: SharedString,
    pub extent: f64,
}

impl From<&ResolvedSnapPoint> for SheetSnapPoint {
    fn from(point: &ResolvedSnapPoint) -> Self {
        Self {
            id: point.id.clone(),
            extent: point.extent,
        }
    }
}

/// What the machine moves a sheet between, and how.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetConfig {
    /// Resting extents, ascending, in px. 0 (dismissed) is implied.
    pub snap_points: Vec<SheetSnapPoint>,
    /// The rubber band's dimension: the container's size on the axis, in px.
    pub dimension: f64,
    /// Whether a swipe can close the sheet.
    pub dismissible: bool,
    /// Whether dragging up on scrollable content first expands the sheet.
    pub expand_on_scroll: bool,
    pub physics: SheetPhysics,
    /// The hand-off to scroll areas under the finger, for sheets with scrollable content.
    pub content_scroll: Option<ContentScroll>,
}

/// What the scroll area under the finger can do. Moves carry `None` when there is none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ScrollContext {
    pub at_start: bool,
    pub at_end: bool,
}

/// Why a sheet moved to a snap point: a swipe, new geometry, or a call from code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SheetSnapReason {
    Swipe,
    ContentResize,
    Imperative,
}

impl SheetSnapReason {
    /// The original's name for it, like `"content-resize"`.
    pub fn as_str(self) -> &'static str {
        self.reason().as_str()
    }

    /// The same reason in the shared vocabulary.
    pub fn reason(self) -> Reason {
        match self {
            SheetSnapReason::Swipe => Reason::Swipe,
            SheetSnapReason::ContentResize => Reason::ContentResize,
            SheetSnapReason::Imperative => Reason::Imperative,
        }
    }
}

impl From<SheetSnapReason> for Reason {
    fn from(reason: SheetSnapReason) -> Self {
        reason.reason()
    }
}

/// Why a sheet closed: a swipe, or a call from code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SheetDismissReason {
    Swipe,
    Imperative,
}

impl SheetDismissReason {
    /// The original's name for it, `"swipe"` or `"imperative"`.
    pub fn as_str(self) -> &'static str {
        self.reason().as_str()
    }

    /// The same reason in the shared vocabulary.
    pub fn reason(self) -> Reason {
        match self {
            SheetDismissReason::Swipe => Reason::Swipe,
            SheetDismissReason::Imperative => Reason::Imperative,
        }
    }
}

impl From<SheetDismissReason> for Reason {
    fn from(reason: SheetDismissReason) -> Self {
        reason.reason()
    }
}

/// What kind of pointer pressed: its slop depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerType {
    Touch,
    Mouse,
    Pen,
}

/// What a driver tells the machine.
#[derive(Clone, Debug, PartialEq)]
pub enum SheetEvent {
    Down {
        time: f64,
        x: f64,
        y: f64,
        pointer: PointerType,
    },
    Move {
        time: f64,
        x: f64,
        y: f64,
        scroll: Option<ScrollContext>,
    },
    /// `velocity` is the finger's, along y, in px/ms.
    Up { time: f64, velocity: f64 },
    Settled,
    /// The scroll-snap driver's scroll came to rest.
    Rest { extent: f64 },
    /// Settle on a snap point, like a handle press, a controller or new
    /// geometry. `reason` comes back on the snap effect.
    SnapTo {
        time: f64,
        id: SharedString,
        reason: SheetSnapReason,
    },
    Close { time: f64 },
    /// A refused dismissal, like Escape on a sheet that can't be dismissed: a
    /// small push toward closing that springs back.
    Nudge { time: f64 },
}

/// What the machine asks a driver to do, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum SheetEffect {
    /// Cancel any animation and show this extent.
    Move { extent: f64 },
    Settle {
        from: f64,
        to: f64,
        velocity: f64,
        response: f64,
        damping_ratio: f64,
    },
    /// Scroll the content forward by `delta` px (negative scrolls back).
    Scroll { delta: f64 },
    /// Let the content coast at `velocity` px/ms, forward when positive.
    FlingScroll { velocity: f64 },
    Snap {
        id: SharedString,
        reason: SheetSnapReason,
    },
    Dismiss { reason: SheetDismissReason },
    /// A refused dismissal, by a swipe.
    AttemptDismiss,
}

/// Where a sheet is: its extent, the snap point it last rested on (`None`
/// when closed), and what the gesture is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetState {
    pub extent: f64,
    pub resting_index: Option<usize>,
    pub phase: SheetPhase,
}

/// What a sheet's gesture is doing, with what each phase keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SheetPhase {
    Idle,
    Pending {
        origin_x: f64,
        origin_y: f64,
        slop: f64,
    },
    Dragging {
        /// The finger's y when `anchor_extent` was shown 1:1.
        anchor_y: f64,
        /// The unconstrained extent at `anchor_y`, before the rubber band.
        anchor_extent: f64,
        last_y: f64,
        /// The finger currently scrolls the content instead of moving the sheet.
        is_scrolling_content: bool,
    },
    Scrolling {
        last_y: f64,
    },
    Settling {
        /// The snap point it settles on, `None` for dismissal.
        target_index: Option<usize>,
        from: f64,
        to: f64,
        velocity: f64,
        response: f64,
        damping_ratio: f64,
        start_time: f64,
    },
}

impl SheetPhase {
    /// The original's name for it, like `"settling"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            SheetPhase::Idle => "idle",
            SheetPhase::Pending { .. } => "pending",
            SheetPhase::Dragging { .. } => "dragging",
            SheetPhase::Scrolling { .. } => "scrolling",
            SheetPhase::Settling { .. } => "settling",
        }
    }
}

/// A sheet's extent and velocity at a moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SheetMotion {
    pub extent: f64,
    pub velocity: f64,
}

// The velocity, in px/ms toward closing, that a refused dismissal starts with.
// With the iOS spring it dips about 20px and comes back.
const NUDGE_VELOCITY: f64 = 1.0;

/// A sheet at rest at `extent`, on the snap point at `resting_index`.
pub fn initial_state(extent: f64, resting_index: Option<usize>) -> SheetState {
    SheetState {
        extent,
        resting_index,
        phase: SheetPhase::Idle,
    }
}

/// The next state, and the effects for the driver to execute in order.
pub fn next(
    state: &SheetState,
    event: &SheetEvent,
    config: &SheetConfig,
) -> (SheetState, Vec<SheetEffect>) {
    match event {
        SheetEvent::Down {
            time,
            x,
            y,
            pointer,
        } => on_down(state, *time, *x, *y, *pointer, config),
        SheetEvent::Move { .. } => on_move(state, event, config),
        SheetEvent::Up { .. } => on_up(state, event, config),
        SheetEvent::Settled => on_settled(state),
        SheetEvent::Rest { extent } => on_rest(*extent, state, config),
        SheetEvent::SnapTo { time, id, reason } => on_snap_to(state, *time, id, *reason, config),
        SheetEvent::Nudge { time } => on_nudge(state, *time, config),
        SheetEvent::Close { time } => settle_to(
            state,
            config,
            Settle {
                target_index: None,
                motion: current_motion(state, *time),
                time: *time,
                reason: SheetSnapReason::Imperative,
                outcome: Some(SheetEffect::Dismiss {
                    reason: SheetDismissReason::Imperative,
                }),
            },
        ),
    }
}

/// The extent and velocity of a settling sheet at `time`, or its resting
/// extent.
pub fn current_motion(state: &SheetState, time: f64) -> SheetMotion {
    let SheetPhase::Settling {
        from,
        to,
        velocity,
        response,
        damping_ratio,
        start_time,
        ..
    } = state.phase
    else {
        return SheetMotion {
            extent: state.extent,
            velocity: 0.0,
        };
    };
    let solver = solve_spring(response, damping_ratio, from - to, velocity);
    // A settle can be scheduled to start a little later, like an open that
    // waits for the sheet's first frames. Until then it's where it starts.
    let elapsed = js::max(0.0, time - start_time);
    SheetMotion {
        extent: to + solver.position(elapsed),
        velocity: solver.velocity(elapsed),
    }
}

fn on_down(
    state: &SheetState,
    time: f64,
    x: f64,
    y: f64,
    pointer: PointerType,
    config: &SheetConfig,
) -> (SheetState, Vec<SheetEffect>) {
    let slop = if pointer == PointerType::Mouse {
        config.physics.mouse_slop
    } else {
        config.physics.touch_slop
    };
    if state.phase == SheetPhase::Idle && state.resting_index.is_some() {
        let pending = SheetState {
            phase: SheetPhase::Pending {
                origin_x: x,
                origin_y: y,
                slop,
            },
            ..*state
        };
        return (pending, Vec::new());
    }
    if !matches!(state.phase, SheetPhase::Settling { .. }) {
        return (*state, Vec::new());
    }
    // A touch during a settle catches the sheet where the spring is now and
    // drags from there with no slop.
    let extent = current_motion(state, time).extent;
    let dragging = SheetState {
        extent,
        resting_index: state.resting_index,
        phase: SheetPhase::Dragging {
            anchor_y: y,
            anchor_extent: unconstrain_extent(extent, config),
            last_y: y,
            is_scrolling_content: false,
        },
    };
    (dragging, vec![SheetEffect::Move { extent }])
}

fn on_move(
    state: &SheetState,
    event: &SheetEvent,
    config: &SheetConfig,
) -> (SheetState, Vec<SheetEffect>) {
    match (state.phase, config.content_scroll) {
        (SheetPhase::Pending { .. }, _) => leave_pending(state, event, config),
        (SheetPhase::Scrolling { .. }, Some(content_scroll)) => content_scroll.r#move(state, event),
        (SheetPhase::Dragging { .. }, _) => drag(state, event, config),
        _ => (*state, Vec::new()),
    }
}

fn leave_pending(
    state: &SheetState,
    event: &SheetEvent,
    config: &SheetConfig,
) -> (SheetState, Vec<SheetEffect>) {
    let (SheetPhase::Pending {
        origin_x,
        origin_y,
        slop,
    }, &SheetEvent::Move { x, y, .. }) = (state.phase, event)
    else {
        return (*state, Vec::new());
    };
    let dx = x - origin_x;
    let dy = y - origin_y;
    if js::max(dx.abs(), dy.abs()) < slop {
        return (*state, Vec::new());
    }
    // Off-axis moves belong to the page, so a horizontal carousel keeps working.
    if dx.abs() > dy.abs() {
        return (initial_state(state.extent, state.resting_index), Vec::new());
    }
    let scrolling = config
        .content_scroll
        .and_then(|content_scroll| content_scroll.claim(state, event, dy < 0.0, config));
    if let Some(scrolling) = scrolling {
        return (scrolling, Vec::new());
    }
    // Anchor where the finger crossed the slop, not where this sample landed,
    // so the sheet trails the finger by exactly the slop at any input rate.
    let anchor_y = origin_y + js::sign(dy) * slop;
    let anchor_extent = state.extent;
    let extent = constrain_extent(anchor_extent + (anchor_y - y), config);
    let dragging = SheetState {
        extent,
        resting_index: state.resting_index,
        phase: SheetPhase::Dragging {
            anchor_y,
            anchor_extent,
            last_y: y,
            is_scrolling_content: false,
        },
    };
    (dragging, vec![SheetEffect::Move { extent }])
}

/// A sheet that starts dragging from where it is, with the finger at `y`.
pub fn start_dragging(state: &SheetState, y: f64) -> SheetState {
    SheetState {
        extent: state.extent,
        resting_index: state.resting_index,
        phase: SheetPhase::Dragging {
            anchor_y: y,
            anchor_extent: state.extent,
            last_y: y,
            is_scrolling_content: false,
        },
    }
}

fn drag(
    state: &SheetState,
    event: &SheetEvent,
    config: &SheetConfig,
) -> (SheetState, Vec<SheetEffect>) {
    let (SheetPhase::Dragging {
        anchor_y,
        anchor_extent,
        last_y,
        is_scrolling_content,
    }, &SheetEvent::Move { y, .. }) = (state.phase, event)
    else {
        return (*state, Vec::new());
    };
    // Positive when the finger moves up: more extent, or content scrolled forward.
    let delta = last_y - y;
    // Once the content is back at its start, the sheet continues from here.
    let (anchor_y, anchor_extent) = if is_scrolling_content {
        (last_y, state.extent)
    } else {
        (anchor_y, anchor_extent)
    };
    let finger_extent = anchor_extent + (anchor_y - y);
    let scrolled = config.content_scroll.and_then(|content_scroll| {
        content_scroll.drag(state, event, delta, finger_extent, config)
    });
    if let Some(scrolled) = scrolled {
        return scrolled;
    }
    let extent = constrain_extent(finger_extent, config);
    let dragging = SheetState {
        extent,
        resting_index: state.resting_index,
        phase: SheetPhase::Dragging {
            anchor_y,
            anchor_extent,
            last_y: y,
            is_scrolling_content: false,
        },
    };
    (dragging, vec![SheetEffect::Move { extent }])
}

fn on_up(
    state: &SheetState,
    event: &SheetEvent,
    config: &SheetConfig,
) -> (SheetState, Vec<SheetEffect>) {
    if matches!(
        state.phase,
        SheetPhase::Pending { .. } | SheetPhase::Scrolling { .. }
    ) {
        return (initial_state(state.extent, state.resting_index), Vec::new());
    }
    let (SheetPhase::Dragging {
        anchor_y,
        anchor_extent,
        last_y,
        ..
    }, &SheetEvent::Up { time, velocity: finger_velocity }) = (state.phase, event)
    else {
        return (*state, Vec::new());
    };
    let released = config
        .content_scroll
        .and_then(|content_scroll| content_scroll.release(state, event, config));
    if let Some(released) = released {
        return released;
    }
    let velocity = -finger_velocity * band_slope(state.extent, config);
    let mut candidates = Vec::with_capacity(config.snap_points.len() + 1);
    candidates.push(0.0);
    candidates.extend(snap_extents(config));
    let physics = &config.physics;
    let target = |extent: f64, velocity: f64| SnapTargetInput {
        extent,
        velocity,
        candidates: &candidates,
        rate: physics.projection_rate,
        min_fling_velocity: physics.min_fling_velocity,
        skip_snap_points: physics.skip_snap_points,
    };
    let candidate = choose_snap_target(target(state.extent, velocity));
    let release = |target_index: Option<usize>, outcome: Option<SheetEffect>| Settle {
        target_index,
        motion: SheetMotion {
            extent: state.extent,
            velocity,
        },
        time,
        reason: SheetSnapReason::Swipe,
        outcome,
    };
    if config.dismissible {
        if candidate > 0 {
            return settle_to(state, config, release(Some(candidate - 1), None));
        }
        return settle_to(
            state,
            config,
            release(
                None,
                Some(SheetEffect::Dismiss {
                    reason: SheetDismissReason::Swipe,
                }),
            ),
        );
    }
    // The band hides how far the finger went, so a refused dismissal is judged
    // by the finger: released where it would have dismissed, it's an attempt.
    let finger_extent = anchor_extent + (anchor_y - last_y);
    let would_dismiss = choose_snap_target(target(finger_extent, -finger_velocity)) == 0;
    let index = candidate.saturating_sub(1);
    if !would_dismiss {
        return settle_to(state, config, release(Some(index), None));
    }
    settle_to(
        state,
        config,
        release(Some(0), Some(SheetEffect::AttemptDismiss)),
    )
}

fn on_settled(state: &SheetState) -> (SheetState, Vec<SheetEffect>) {
    let SheetPhase::Settling {
        to, target_index, ..
    } = state.phase
    else {
        return (*state, Vec::new());
    };
    (initial_state(to, target_index), Vec::new())
}

fn on_rest(extent: f64, state: &SheetState, config: &SheetConfig) -> (SheetState, Vec<SheetEffect>) {
    if extent < 1.0 && config.dismissible {
        return (
            initial_state(0.0, None),
            vec![SheetEffect::Dismiss {
                reason: SheetDismissReason::Swipe,
            }],
        );
    }
    let index = nearest_index(&snap_extents(config), extent);
    // With no snap points there is nothing to rest on.
    let Some(point) = config.snap_points.get(index) else {
        return (initial_state(extent, None), Vec::new());
    };
    let effects = if Some(index) == state.resting_index {
        Vec::new()
    } else {
        vec![SheetEffect::Snap {
            id: point.id.clone(),
            reason: SheetSnapReason::Swipe,
        }]
    };
    (initial_state(extent, Some(index)), effects)
}

fn on_snap_to(
    state: &SheetState,
    time: f64,
    id: &SharedString,
    reason: SheetSnapReason,
    config: &SheetConfig,
) -> (SheetState, Vec<SheetEffect>) {
    let index = config.snap_points.iter().position(|point| &point.id == id);
    let Some(index) = index else {
        return (*state, Vec::new());
    };
    if matches!(state.phase, SheetPhase::Dragging { .. }) {
        return (*state, Vec::new());
    }
    settle_to(
        state,
        config,
        Settle {
            target_index: Some(index),
            motion: current_motion(state, time),
            time,
            reason,
            outcome: None,
        },
    )
}

fn on_nudge(state: &SheetState, time: f64, config: &SheetConfig) -> (SheetState, Vec<SheetEffect>) {
    if state.phase != SheetPhase::Idle || state.resting_index.is_none() {
        return (*state, Vec::new());
    }
    settle_to(
        state,
        config,
        Settle {
            target_index: state.resting_index,
            motion: SheetMotion {
                extent: state.extent,
                velocity: -NUDGE_VELOCITY,
            },
            time,
            reason: SheetSnapReason::Imperative,
            outcome: None,
        },
    )
}

struct Settle {
    /// The snap point to settle on, `None` to close.
    target_index: Option<usize>,
    motion: SheetMotion,
    time: f64,
    reason: SheetSnapReason,
    /// Dismissal, or a refused one, reported after the settle and snap effects.
    outcome: Option<SheetEffect>,
}

fn settle_to(state: &SheetState, config: &SheetConfig, options: Settle) -> (SheetState, Vec<SheetEffect>) {
    let Settle {
        target_index,
        motion,
        time,
        reason,
        outcome,
    } = options;
    let physics = &config.physics;
    let to = target_index
        .and_then(|index| config.snap_points.get(index))
        .map_or(0.0, |point| point.extent);
    let has_momentum = motion.velocity.abs() >= physics.min_fling_velocity;
    let damping_ratio = if has_momentum {
        physics.momentum_damping_ratio
    } else {
        physics.damping_ratio
    };
    let settling = SheetState {
        extent: motion.extent,
        resting_index: state.resting_index,
        phase: SheetPhase::Settling {
            target_index,
            from: motion.extent,
            to,
            velocity: motion.velocity,
            response: physics.response,
            damping_ratio,
            start_time: time,
        },
    };
    let mut effects = vec![SheetEffect::Settle {
        from: motion.extent,
        to,
        velocity: motion.velocity,
        response: physics.response,
        damping_ratio,
    }];
    if let Some(index) = target_index
        && target_index != state.resting_index
        && let Some(point) = config.snap_points.get(index)
    {
        effects.push(SheetEffect::Snap {
            id: point.id.clone(),
            reason,
        });
    }
    if let Some(outcome) = outcome {
        effects.push(outcome);
    }
    (settling, effects)
}

// The extent a finger at `finger_extent` shows: 1:1 between the smallest and
// largest snap points, rubber-banded past the largest, and past the smallest
// when the sheet can't be dismissed.
fn constrain_extent(finger_extent: f64, config: &SheetConfig) -> f64 {
    let largest = largest_extent(config);
    let (coefficient, dimension) = band(config);
    if finger_extent > largest {
        return largest + rubber_band_with(finger_extent - largest, dimension, coefficient);
    }
    if config.dismissible {
        return js::max(0.0, finger_extent);
    }
    let smallest = smallest_extent(config);
    if finger_extent < smallest {
        return smallest - rubber_band_with(smallest - finger_extent, dimension, coefficient);
    }
    finger_extent
}

fn unconstrain_extent(extent: f64, config: &SheetConfig) -> f64 {
    let largest = largest_extent(config);
    let (coefficient, dimension) = band(config);
    if extent > largest {
        return largest + rubber_band_inverse(extent - largest, dimension, coefficient);
    }
    let smallest = smallest_extent(config);
    if !config.dismissible && extent < smallest {
        return smallest - rubber_band_inverse(smallest - extent, dimension, coefficient);
    }
    extent
}

// The rubber band's coefficient and dimension.
fn band(config: &SheetConfig) -> (f64, f64) {
    (
        config.physics.rubber_band,
        config.physics.rubber_band_dimension.unwrap_or(config.dimension),
    )
}

// Inside the band, the shown extent moves slower than the finger, so the
// spring starts with the finger's velocity scaled by the band's slope there.
fn band_slope(extent: f64, config: &SheetConfig) -> f64 {
    let (coefficient, dimension) = band(config);
    let largest = largest_extent(config);
    let smallest = smallest_extent(config);
    let overshoot = if extent > largest {
        extent - largest
    } else if !config.dismissible && extent < smallest {
        smallest - extent
    } else {
        0.0
    };
    if overshoot <= 0.0 {
        return 1.0;
    }
    rubber_band_slope(
        rubber_band_inverse(overshoot, dimension, coefficient),
        dimension,
        coefficient,
    )
}

/// The largest snap point's extent, 0 with none.
pub fn largest_extent(config: &SheetConfig) -> f64 {
    config.snap_points.last().map_or(0.0, |point| point.extent)
}

fn smallest_extent(config: &SheetConfig) -> f64 {
    config.snap_points.first().map_or(0.0, |point| point.extent)
}

/// The snap points' extents, ascending.
pub fn snap_extents(config: &SheetConfig) -> Vec<f64> {
    config.snap_points.iter().map(|point| point.extent).collect()
}
