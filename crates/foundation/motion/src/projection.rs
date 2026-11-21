//! Port of `foundation/motion/projection.ts`.
//!
//! Where a released sheet comes to rest. UIScrollView decelerates by a fixed
//! rate per millisecond, so a release at v px/ms travels v·r/(1 − r) before it
//! stops (WWDC18 803). 0.998 is `.normal`, 0.99 is `.fast`.

use crate::js;

/// How far something thrown at `velocity` px/ms travels before it stops,
/// slowing by `rate` per ms, like 0.998 for a scroll view.
pub fn project_distance(velocity: f64, rate: f64) -> f64 {
    (velocity * rate) / (1.0 - rate)
}

/// What `choose_snap_target` picks from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapTargetInput<'a> {
    /// The extent at release, in px.
    pub extent: f64,
    /// px/ms, positive toward a larger extent.
    pub velocity: f64,
    /// Ascending resting extents, in px.
    pub candidates: &'a [f64],
    /// Per-millisecond deceleration used for the projection.
    pub rate: f64,
    /// Below this speed, in px/ms, the nearest candidate wins.
    pub min_fling_velocity: f64,
    /// Whether a fling can pass over a candidate.
    pub skip_snap_points: bool,
}

/// Picks the index of the candidate extent a release settles on.
/// `candidates` are ascending extents, including 0 when the sheet can be
/// dismissed. `velocity` is the extent's velocity in px/ms, positive when
/// growing.
pub fn choose_snap_target(input: SnapTargetInput<'_>) -> usize {
    let SnapTargetInput {
        extent,
        velocity,
        candidates,
        ..
    } = input;
    if velocity.abs() < input.min_fling_velocity {
        return nearest_index(candidates, extent);
    }
    let projected = extent + project_distance(velocity, input.rate);
    let next_in_direction = first_beyond(candidates, extent, js::sign(velocity));
    let mut target = nearest_index(candidates, projected);
    let is_behind = candidates.get(target).is_some_and(|&candidate| {
        if velocity > 0.0 {
            candidate <= extent + 0.5
        } else {
            candidate >= extent - 0.5
        }
    });
    if let Some(next) = next_in_direction {
        if is_behind {
            target = next;
        }
        if !input.skip_snap_points {
            target = if velocity > 0.0 {
                target.min(next)
            } else {
                target.max(next)
            };
        }
    }
    target
}

/// The index of the candidate nearest `value`, the first of equals, 0 when
/// there are none.
pub fn nearest_index(candidates: &[f64], value: f64) -> usize {
    let mut best = 0;
    for index in 1..candidates.len() {
        if (candidates[index] - value).abs() < (candidates[best] - value).abs() {
            best = index;
        }
    }
    best
}

// The first candidate strictly past `extent` in `direction`.
fn first_beyond(candidates: &[f64], extent: f64, direction: f64) -> Option<usize> {
    if direction > 0.0 {
        return candidates.iter().position(|&candidate| candidate > extent + 0.5);
    }
    (0..candidates.len()).rev().find(|&index| candidates[index] < extent - 0.5)
}
