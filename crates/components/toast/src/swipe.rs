//! Port of `components/toast/swipe.ts`: which way a press on a toast moves,
//! how far the toast follows it, and whether letting go dismisses it.
//!
//! Distances are pixels and velocities pixels per millisecond, as the
//! original's. A swipe follows the pointer through the region's
//! [`DragSession`], so it keeps going outside the toast and the window.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, Pixels, Point, Window, point, px};
use ui_sdk_drag_and_drop::{
    BindDragOptions, DragSession, PointerEvent, PointerSample, StartOnMoveOptions,
};
use web_time::Instant;

/// A direction a swipe can dismiss toward.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

impl SwipeDirection {
    /// The original's name for it, like `"up"`.
    pub fn as_str(self) -> &'static str {
        match self {
            SwipeDirection::Up => "up",
            SwipeDirection::Down => "down",
            SwipeDirection::Left => "left",
            SwipeDirection::Right => "right",
        }
    }
}

/// The axis a swipe moves along, `"x"` or `"y"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SwipeAxis {
    X,
    Y,
}

const SLOP: f32 = 8.;

/// Movement toward a direction: positive when the pointer moved that way.
pub fn along(direction: SwipeDirection, x: f32, y: f32) -> f32 {
    match direction {
        SwipeDirection::Up => -y,
        SwipeDirection::Down => y,
        SwipeDirection::Left => -x,
        SwipeDirection::Right => x,
    }
}

fn resist(distance: f32) -> f32 {
    if distance == 0. {
        return 0.;
    }
    distance.signum() * 4. * (distance.abs() / 4.).ln_1p()
}

/// Where a swiped element shows, from how far the pointer moved: freely toward
/// an allowed direction, and resisting on the rest of that axis.
pub fn swipe_offset(
    x: Pixels,
    y: Pixels,
    axis: SwipeAxis,
    directions: &[SwipeDirection],
) -> Point<Pixels> {
    let free = |value: f32, negative: SwipeDirection, positive: SwipeDirection| {
        if (value < 0. && directions.contains(&negative))
            || (value > 0. && directions.contains(&positive))
        {
            value
        } else {
            resist(value)
        }
    };
    match axis {
        SwipeAxis::X => point(
            px(free(
                f32::from(x),
                SwipeDirection::Left,
                SwipeDirection::Right,
            )),
            px(0.),
        ),
        SwipeAxis::Y => point(
            px(0.),
            px(free(f32::from(y), SwipeDirection::Up, SwipeDirection::Down)),
        ),
    }
}

/// A released swipe, as [`swipe_outcome`] decides it: how far it moved and
/// how fast it was going, with how far or how fast dismisses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwipeRelease {
    pub x: f32,
    pub y: f32,
    /// Pixels per millisecond.
    pub velocity_x: f32,
    pub velocity_y: f32,
    /// How far toward a direction dismisses. Defaults to 40.
    pub threshold: f32,
    /// How fast toward a direction dismisses, once it moved past a few
    /// pixels. Defaults to 0.3.
    pub flick: f32,
}

impl SwipeRelease {
    pub fn new(x: f32, y: f32, velocity_x: f32, velocity_y: f32) -> Self {
        Self {
            x,
            y,
            velocity_x,
            velocity_y,
            threshold: 40.,
            flick: 0.3,
        }
    }

    pub fn threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold;
        self
    }

    pub fn flick(mut self, flick: f32) -> Self {
        self.flick = flick;
        self
    }
}

/// Which direction a released swipe dismisses toward, or `None` when it snaps
/// back. Far enough, or fast enough toward a direction, dismisses. Velocity is
/// in px per ms.
pub fn swipe_outcome(
    release: &SwipeRelease,
    directions: &[SwipeDirection],
) -> Option<SwipeDirection> {
    directions.iter().copied().find(|&direction| {
        let distance = along(direction, release.x, release.y);
        let speed = along(direction, release.velocity_x, release.velocity_y);
        distance >= release.threshold || (distance > SLOP && speed >= release.flick)
    })
}

/// Which axis a press moved along first, if one of its directions is on it:
/// `None` until it has moved a few pixels, then `Some` of the axis, or
/// `Some(None)` when no direction lies on it.
pub fn swipe_axis(x: f32, y: f32, directions: &[SwipeDirection]) -> Option<Option<SwipeAxis>> {
    if x.hypot(y) < SLOP {
        return None;
    }
    let axis = if x.abs() > y.abs() {
        SwipeAxis::X
    } else {
        SwipeAxis::Y
    };
    let allowed = match axis {
        SwipeAxis::X => directions
            .iter()
            .any(|direction| matches!(direction, SwipeDirection::Left | SwipeDirection::Right)),
        SwipeAxis::Y => directions
            .iter()
            .any(|direction| matches!(direction, SwipeDirection::Up | SwipeDirection::Down)),
    };
    Some(allowed.then_some(axis))
}

/// The press became a swipe along an axis.
pub type SwipeStartCallback<T> = Rc<dyn Fn(&mut T, SwipeAxis, &mut Window, &mut Context<T>)>;

/// Where the element should show now.
pub type SwipeMoveCallback<T> = Rc<dyn Fn(&mut T, Point<Pixels>, &mut Window, &mut Context<T>)>;

/// It ended: the direction it dismisses toward, or `None` to snap back, with
/// the event that ended it.
pub type SwipeEndCallback<T> =
    Rc<dyn Fn(&mut T, Option<SwipeDirection>, &PointerEvent, &mut Window, &mut Context<T>)>;

/// What [`track_swipe`] tells the component, inside its update.
pub struct SwipeCallbacks<T: 'static> {
    pub on_start: SwipeStartCallback<T>,
    pub on_move: SwipeMoveCallback<T>,
    pub on_end: SwipeEndCallback<T>,
}

#[derive(Clone, Copy)]
struct Sample {
    x: Pixels,
    y: Pixels,
    time: Instant,
}

struct Tracking {
    start: Point<Pixels>,
    // The last two positions, for the release's velocity.
    samples: Vec<Sample>,
    axis: SwipeAxis,
}

// Milliseconds from one sample to the next.
fn elapsed_ms(from: &Sample, to: &Sample) -> f32 {
    to.time.saturating_duration_since(from.time).as_secs_f32() * 1000.
}

/// Follows a press that may become a swipe toward `directions`, as the
/// session's one drag. The press stays a press until the pointer moves past a
/// few pixels along an axis with a direction, so clicks on buttons inside keep
/// working. Then the session follows the pointer until it lifts.
pub fn track_swipe<T: 'static>(
    drags: &DragSession<T>,
    down: &PointerEvent,
    directions: Vec<SwipeDirection>,
    callbacks: SwipeCallbacks<T>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let start = down.position;
    let tracking = Rc::new(RefCell::new(Tracking {
        start,
        samples: vec![Sample {
            x: start.x,
            y: start.y,
            time: down.time,
        }],
        axis: SwipeAxis::X,
    }));
    let directions: Rc<[SwipeDirection]> = directions.into();

    let pick = {
        let tracking = tracking.clone();
        let directions = directions.clone();
        let on_start = callbacks.on_start.clone();
        move |this: &mut T, event: &PointerEvent, window: &mut Window, cx: &mut Context<T>| {
            let start = tracking.borrow().start;
            let picked = swipe_axis(
                f32::from(event.position.x - start.x),
                f32::from(event.position.y - start.y),
                &directions,
            )?;
            let Some(axis) = picked else {
                return Some(false);
            };
            tracking.borrow_mut().axis = axis;
            on_start(this, axis, window, cx);
            Some(true)
        }
    };
    let on_move = {
        let tracking = tracking.clone();
        let directions = directions.clone();
        let on_move = callbacks.on_move.clone();
        move |this: &mut T, sample: &PointerSample, window: &mut Window, cx: &mut Context<T>| {
            let offset = {
                let mut tracking = tracking.borrow_mut();
                tracking.samples.push(Sample {
                    x: sample.x,
                    y: sample.y,
                    time: sample.event.time,
                });
                if tracking.samples.len() > 2 {
                    tracking.samples.remove(0);
                }
                let start = tracking.start;
                swipe_offset(
                    sample.x - start.x,
                    sample.y - start.y,
                    tracking.axis,
                    &directions,
                )
            };
            on_move(this, offset, window, cx);
        }
    };
    let on_end = {
        let on_end = callbacks.on_end.clone();
        move |this: &mut T,
              event: &PointerEvent,
              canceled: bool,
              window: &mut Window,
              cx: &mut Context<T>| {
            let direction = {
                let tracking = tracking.borrow();
                let from = tracking.samples[0];
                let to = tracking.samples.get(1).copied();
                let elapsed = to.map_or(f32::INFINITY, |to| elapsed_ms(&from, &to).max(1.));
                let x = f32::from(event.position.x - tracking.start.x);
                let y = f32::from(event.position.y - tracking.start.y);
                let velocity = |delta: fn(&Sample, &Sample) -> f32| {
                    to.map_or(0., |to| delta(&from, &to) / elapsed)
                };
                if canceled {
                    None
                } else {
                    let release = SwipeRelease::new(
                        if tracking.axis == SwipeAxis::X { x } else { 0. },
                        if tracking.axis == SwipeAxis::Y { y } else { 0. },
                        velocity(|from, to| f32::from(to.x - from.x)),
                        velocity(|from, to| f32::from(to.y - from.y)),
                    );
                    swipe_outcome(&release, &directions)
                }
            };
            on_end(this, direction, event, window, cx);
        }
    };
    let options =
        StartOnMoveOptions::new(BindDragOptions::new().on_move(on_move).on_end(on_end)).pick(pick);
    drags.start_on_move(down, options, window, cx);
}
