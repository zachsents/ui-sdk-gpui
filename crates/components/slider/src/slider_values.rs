//! Port of `components/slider/slider-values.ts`: the decisions a slider
//! makes, as pure functions of its values and bounds: where each thumb may
//! go, what a key does, and which thumb a press moves. `slider_api.rs` wires
//! them to the parts.

use gpui::{App, Bounds, Keystroke, Pixels, Point, Window};
use ui_sdk_number::{
    NumberBound, NumberKeyOptions, NumberMove, NumberRange, StepRange, clamp, max_on_grid, snap_to_step,
    step_from_key, step_on_grid,
};

use crate::slider::SliderOrientation;

/// The bounds and steps a slider moves its thumbs by (`SliderRange`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderRange {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    /// How far Page Up, Page Down and Shift with an arrow key move.
    pub large_step: f64,
    /// How far apart neighbouring thumbs stay, in steps.
    pub min_steps_between_thumbs: f64,
}

impl SliderRange {
    /// Its bounds and step.
    pub fn step_range(&self) -> StepRange {
        StepRange::new(self.min, self.max, self.step)
    }

    // The range a key's step moves on: the grid, with the large step.
    fn number_range(&self) -> NumberRange {
        NumberRange::new(self.min, self.max, self.step).large_step(self.large_step)
    }
}

/// Where one thumb may go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbBounds {
    pub lower: f64,
    pub upper: f64,
}

/// Where one thumb may go: between its neighbours, kept apart by the minimum
/// distance, and never past the bounds. When the thumbs are too close to keep
/// the distance, like `[0, 0]` set from outside, the bounds win.
pub fn thumb_bounds(values: &[f64], index: usize, range: &SliderRange) -> ThumbBounds {
    let before = index
        .checked_sub(1)
        .and_then(|before| values.get(before).copied());
    bounds_between(before, values.get(index + 1).copied(), range)
}

/// Where a thumb may go between the values of the thumbs before and after
/// it, if any.
pub fn bounds_between(before: Option<f64>, after: Option<f64>, range: &SliderRange) -> ThumbBounds {
    let gap = range.min_steps_between_thumbs * range.step;
    let end = max_on_grid(range.step_range());
    let lower = clamp(before.map_or(range.min, |before| before + gap), range.min, end);
    let upper = clamp(after.map_or(end, |after| after - gap), lower, end);
    ThumbBounds { lower, upper }
}

/// Moves one thumb, snapped and kept between its neighbours. Thumbs never
/// pass each other, so each keeps its meaning, like "minimum price", and its
/// focus. Returns `None` when nothing moved, where the original returns the
/// same array.
pub fn move_thumb(values: &[f64], index: usize, target: f64, range: &SliderRange) -> Option<Vec<f64>> {
    let current = *values.get(index)?;
    let ThumbBounds { lower, upper } = thumb_bounds(values, index, range);
    let next = clamp(snap_to_step(target, range.step_range()), lower, upper);
    if next == current {
        return None;
    }
    let mut result = values.to_vec();
    result[index] = next;
    Some(result)
}

/// Values sorted and within the bounds, for values that come from outside.
pub fn normalize_values(values: &[f64], range: StepRange) -> Vec<f64> {
    let mut values: Vec<f64> = values
        .iter()
        .map(|value| clamp(*value, range.min, range.max))
        .collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values
}

/// How a slider lays out, for [`key_target`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyTargetOptions {
    /// Right to left, which swaps Left and Right on a horizontal slider.
    pub rtl: bool,
    pub orientation: SliderOrientation,
}

/// What a key does to a thumb's value, or `None` for keys a slider ignores.
/// Home and End go to the thumb's own bounds. Shift comes with the key.
pub fn key_target(
    keystroke: &Keystroke,
    options: KeyTargetOptions,
    value: f64,
    bounds: ThumbBounds,
    range: &SliderRange,
    window: &Window,
    cx: &App,
) -> Option<f64> {
    let keys = NumberKeyOptions::default()
        .orientation(options.orientation)
        .rtl(options.rtl);
    match step_from_key(keystroke, keys, window, cx)? {
        NumberMove::Bound(NumberBound::Min) => Some(bounds.lower),
        NumberMove::Bound(NumberBound::Max) => Some(bounds.upper),
        NumberMove::Step { direction, size } => {
            Some(step_on_grid(value, direction, size, &range.number_range()))
        }
    }
}

/// The thumb a press moves, and the thumbs stacked with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbPress {
    /// The thumb, or `None` when the press is right on a stack, so the first
    /// move decides.
    pub index: Option<usize>,
    pub stack: Vec<usize>,
}

/// The thumb a press moves. The nearest one, and among thumbs stacked on one
/// value, the one on the side the press is on: below them the lowest, above
/// them the highest. Pressed right on the stack, the first move decides, so
/// the index is `None` and the stack comes back.
pub fn thumb_for_press(values: &[f64], pressed: f64) -> ThumbPress {
    let mut nearest = 0;
    for index in 1..values.len() {
        if (values[index] - pressed).abs() < (values[nearest] - pressed).abs() {
            nearest = index;
        }
    }
    let stack = stack_at(values, nearest);
    if stack.len() == 1 {
        return ThumbPress {
            index: Some(nearest),
            stack,
        };
    }
    let at = values.get(nearest).copied().unwrap_or(f64::NAN);
    let index = if pressed < at {
        stack.first().copied()
    } else if pressed > at {
        stack.last().copied()
    } else {
        None
    };
    ThumbPress { index, stack }
}

/// The thumbs that share a thumb's value.
pub fn stack_at(values: &[f64], index: usize) -> Vec<usize> {
    let Some(value) = values.get(index) else {
        return Vec::new();
    };
    (0..values.len())
        .filter(|other| values[*other] == *value)
        .collect()
}

/// Which thumb of a stack follows a drag: the first move's direction
/// decides, and a stack at either bound can only move away from it.
pub fn thumb_for_direction(stack: &[usize], direction: f64) -> Option<usize> {
    if direction < 0.0 {
        stack.first().copied()
    } else {
        stack.last().copied()
    }
}

/// Where a value sits between the bounds, from 0 to 100, for positioning.
pub fn percent_of(value: f64, range: StepRange) -> f64 {
    if !(range.max > range.min) {
        return 0.0;
    }
    round_percent((clamp(value, range.min, range.max) - range.min) / (range.max - range.min) * 100.0)
}

// Four decimals keep positions exact to well under a pixel.
pub(crate) fn round_percent(percent: f64) -> f64 {
    (percent * 1e4).round() / 1e4
}

/// Where a point falls along a box, from 0 at its start to 1 at its end:
/// left to right, right to left, or bottom to top.
pub fn fraction_along(point: Point<Pixels>, bounds: Bounds<Pixels>, rtl: bool, vertical: bool) -> f64 {
    let (x, y) = (f64::from(point.x), f64::from(point.y));
    let (left, top) = (f64::from(bounds.origin.x), f64::from(bounds.origin.y));
    let (width, height) = (f64::from(bounds.size.width), f64::from(bounds.size.height));
    let fraction = if vertical {
        (top + height - y) / height
    } else if rtl {
        (left + width - x) / width
    } else {
        (x - left) / width
    };
    if fraction.is_finite() {
        clamp(fraction, 0.0, 1.0)
    } else {
        0.0
    }
}

/// The value at a fraction of the track, unsnapped, so presses pick thumbs
/// by distance.
pub fn value_at(fraction: f64, range: StepRange) -> f64 {
    range.min + clamp(fraction, 0.0, 1.0) * (range.max - range.min)
}
