//! Port of `foundation/motion/curves.ts`.
//!
//! Styling curves: pure functions of the extent and the resolved snap points,
//! which drivers turn into state parts style from. `snap_extents` are
//! ascending and exclude 0.

use crate::js;

fn clamp01(value: f64) -> f64 {
    js::max(0.0, js::min(1.0, value))
}

// An index past the end reads as `undefined` in the original, which turns
// the arithmetic into NaN.
fn extent_at(snap_extents: &[f64], index: usize) -> f64 {
    snap_extents.get(index).copied().unwrap_or(f64::NAN)
}

/// 0 at or below the largest undimmed snap point, 1 at the next one. With no
/// undimmed snap point (`None`), the dim ramps in from 0 to the smallest snap
/// point, as iOS fades the dimming view in while a sheet presents.
pub fn dim_amount(extent: f64, snap_extents: &[f64], largest_undimmed_index: Option<usize>) -> f64 {
    let (lower, upper) = match largest_undimmed_index {
        None => (0.0, snap_extents.first().copied()),
        Some(index) => (
            extent_at(snap_extents, index),
            index.checked_add(1).and_then(|next| snap_extents.get(next).copied()),
        ),
    };
    match upper {
        Some(upper) if !(upper <= lower) => clamp01((extent - lower) / (upper - lower)),
        _ => {
            if extent > lower {
                1.0
            } else {
                0.0
            }
        }
    }
}

/// 0 at the second-largest snap point, 1 at the largest: the range where iOS
/// pushes the presenting view back. With one snap point, the whole travel.
pub fn expand_amount(extent: f64, snap_extents: &[f64]) -> f64 {
    let largest = snap_extents.last().copied().unwrap_or(f64::NAN);
    let lower = if snap_extents.len() > 1 {
        snap_extents[snap_extents.len() - 2]
    } else {
        0.0
    };
    if largest <= lower {
        return if extent >= largest { 1.0 } else { 0.0 };
    }
    clamp01((extent - lower) / (largest - lower))
}

/// 0 closed, 1 at the largest snap point. Never past 1, so the rubber band
/// doesn't overshoot effects that follow it.
pub fn progress_amount(extent: f64, snap_extents: &[f64]) -> f64 {
    let largest = snap_extents.last().copied().unwrap_or(f64::NAN);
    if largest > 0.0 {
        clamp01(extent / largest)
    } else {
        0.0
    }
}
