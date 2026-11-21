//! Port of `foundation/number/steps.ts`.
//!
//! Bounds and steps every numeric control shares: clamping, snapping to a step
//! without floating point drift, stepping by a key's move, and wrapping around
//! a loop. Pure, so it runs anywhere, like on a background thread.

use std::fmt;
use std::rc::Rc;

/// Keeps a value between `min` and `max`. A NaN anywhere gives NaN, as
/// `Math.min` and `Math.max` do.
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    js_min(js_max(value, min), max)
}

/// How many decimals a number has, counting exponents like 1e-7.
pub fn decimals_of(value: f64) -> i32 {
    if !value.is_finite() || value.fract() == 0.0 {
        return 0;
    }
    // The shortest text that reads back as the value, as `String(value)` is,
    // in exponent form so a mantissa and an exponent are always there.
    let text = format!("{:e}", value);
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    let decimals = mantissa.split_once('.').map_or(0, |(_, fraction)| fraction.len()) as i32;
    (decimals - exponent.parse::<i32>().unwrap_or(0)).max(0)
}

/// Rounds to a number of decimals, which also drops floating point noise like
/// 0.1 + 0.2. Rounds half away from zero on the value's exact binary value, as
/// `Number(value.toFixed(decimals))` does; decimals are kept between 0 and 100.
pub fn round_to(value: f64, decimals: i32) -> f64 {
    // `toFixed` writes these as `String(value)` does, which reads back the same.
    if !value.is_finite() || value.abs() >= 1e21 {
        return value;
    }
    let decimals = decimals.clamp(0, 100);
    // `toFixed` takes the sign apart, so -0 reads "0" and -0.001 reads "-0.00".
    let rounded = Exact::of(value.abs()).rounded_at(-decimals);
    if value < 0.0 { -rounded } else { rounded }
}

// A count of steps without floating point noise. Noise grows with magnitude,
// so it's cut by significant digits: 123456788.99999999 is 123456789. Doubles
// hold 15 digits exactly, so 98765432101.23 still counts its cents.
fn step_count(distance: f64, step: f64) -> f64 {
    to_precision(distance / step, 15)
}

/// The bounds and step every numeric control shares. The step grid starts at
/// `min` when it's finite, and at 0 otherwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepRange {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl StepRange {
    pub fn new(min: f64, max: f64, step: f64) -> Self {
        Self { min, max, step }
    }
}

impl From<&NumberRange> for StepRange {
    fn from(range: &NumberRange) -> Self {
        range.step_range()
    }
}

// The step grid starts at `min` when it's finite, like a native range input,
// and at 0 otherwise.
fn step_base(range: StepRange) -> f64 {
    if range.min.is_finite() { range.min } else { 0.0 }
}

fn precision_of(range: StepRange) -> i32 {
    decimals_of(range.step).max(decimals_of(step_base(range)))
}

/// The highest value on the step grid, which is `max` only when the grid lands
/// on it, like 9 for `{ min: 1, max: 10, step: 2 }`.
pub fn max_on_grid(range: StepRange) -> f64 {
    if !range.max.is_finite() || !(range.step > 0.0) {
        return range.max;
    }
    let base = step_base(range);
    let steps = step_count(range.max - base, range.step).floor();
    round_to(base + steps * range.step, precision_of(range))
}

/// A value on the step grid, rounded to the nearest step the way `Math.round`
/// rounds, halves up; not bounded.
pub fn on_grid(value: f64, range: StepRange) -> f64 {
    on_grid_with(value, range, math_round)
}

/// A value on the step grid, rounded with `round`, like `f64::floor` for where
/// a drag on a track starts; not bounded.
pub fn on_grid_with(value: f64, range: StepRange, round: impl Fn(f64) -> f64) -> f64 {
    if !(range.step > 0.0) {
        return value;
    }
    let base = step_base(range);
    round_to(
        base + round(step_count(value - base, range.step)) * range.step,
        precision_of(range),
    )
}

/// The nearest value on the step grid, within the bounds. Public as
/// [`scrub_to`] without snap points.
pub fn snap_to_step(value: f64, range: StepRange) -> f64 {
    if !(range.step > 0.0) {
        return clamp(value, range.min, range.max);
    }
    round_to(
        clamp(on_grid(value, range), range.min, max_on_grid(range)),
        precision_of(range),
    )
}

/// The value `steps` steps away on the grid, within the bounds; negative steps
/// go down. A value off the grid moves to the grid first, so 1.5 steps up to 2,
/// like `stepUp()`.
pub fn step_value(value: f64, steps: f64, range: StepRange) -> f64 {
    if !(range.step > 0.0) {
        return clamp(value + steps, range.min, range.max);
    }
    let base = step_base(range);
    let position = step_count(value - base, range.step);
    let on_grid = position.fract() == 0.0;
    let from = if on_grid {
        position
    } else if steps > 0.0 {
        position.floor()
    } else {
        position.ceil()
    };
    let next = base + (from + steps) * range.step;
    round_to(clamp(next, range.min, max_on_grid(range)), precision_of(range))
}

/// Wraps a value into the loop from `min` up to `max`, like an angle into 0 to
/// 360 or an index around a carousel: `wrap(370.0, 0.0, 360.0)` is 10.
pub fn wrap(value: f64, min: f64, max: f64) -> f64 {
    let span = max - min;
    if !(span > 0.0) {
        return min;
    }
    (((value - min) % span) + span) % span + min
}

/// Which way a step goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StepDirection {
    Up,
    Down,
}

impl StepDirection {
    /// 1 for up, -1 for down.
    pub fn sign(self) -> f64 {
        match self {
            StepDirection::Up => 1.0,
            StepDirection::Down => -1.0,
        }
    }
}

/// Which step a move takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StepSize {
    /// The range's step.
    Step,
    /// The large step, what Shift and Page Up and Down move by.
    Large,
    /// The small step, what Alt moves by.
    Small,
}

/// A bound a move goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NumberBound {
    Min,
    Max,
}

/// What a key or a gesture asks of a number: steps in a direction, by the step,
/// the large step or the small one, or a bound. [`step_from_key`] reads one
/// from a key.
///
/// [`step_from_key`]: crate::step_from_key
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NumberMove {
    Step { direction: StepDirection, size: StepSize },
    Bound(NumberBound),
}

/// Values a scrub lands on when it comes close, like a timeline's chapters or keyframes.
#[derive(Clone, Debug, PartialEq)]
pub struct NumberSnap {
    /// The values to land on.
    pub points: Rc<[f64]>,
    /// How close a value comes before it lands on a point, in the range's units.
    pub threshold: f64,
}

impl NumberSnap {
    pub fn new(points: impl Into<Rc<[f64]>>, threshold: f64) -> Self {
        Self {
            points: points.into(),
            threshold,
        }
    }
}

/// A step range with the larger and smaller steps a move can ask for, and the
/// points it snaps to.
#[derive(Clone, Debug, PartialEq)]
pub struct NumberRange {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    /// What Shift and Page Up and Down move by. Ten steps when unset.
    pub large_step: Option<f64>,
    /// What Alt moves by, on a grid of its own. The step when unset.
    pub small_step: Option<f64>,
    /// Points [`scrub_to`] and [`step_by`] land on when they come close.
    pub snap: Option<NumberSnap>,
}

impl NumberRange {
    pub fn new(min: f64, max: f64, step: f64) -> Self {
        Self {
            min,
            max,
            step,
            large_step: None,
            small_step: None,
            snap: None,
        }
    }

    pub fn large_step(mut self, large_step: f64) -> Self {
        self.large_step = Some(large_step);
        self
    }

    pub fn small_step(mut self, small_step: f64) -> Self {
        self.small_step = Some(small_step);
        self
    }

    pub fn snap(mut self, snap: NumberSnap) -> Self {
        self.snap = Some(snap);
        self
    }

    /// Its bounds and step.
    pub fn step_range(&self) -> StepRange {
        StepRange::new(self.min, self.max, self.step)
    }
}

impl From<StepRange> for NumberRange {
    fn from(range: StepRange) -> Self {
        Self::new(range.min, range.max, range.step)
    }
}

/// A step, large step or small step on the grid, without snap points, so a
/// control that never snaps, like a slider, needs none.
pub fn step_on_grid(value: f64, direction: StepDirection, size: StepSize, range: &NumberRange) -> f64 {
    let grid = range.step_range();
    if size == StepSize::Large {
        return snap_to_step(
            clamp(
                value + direction.sign() * range.large_step.unwrap_or(range.step * 10.0),
                range.min,
                range.max,
            ),
            grid,
        );
    }
    if size == StepSize::Small
        && let Some(small_step) = range.small_step.filter(|small| *small != 0.0 && !small.is_nan())
    {
        return step_value(value, direction.sign(), StepRange { step: small_step, ..grid });
    }
    step_value(value, direction.sign(), grid)
}

/// The value a move lands on: a step on the grid, a large step snapped back to
/// it, a small step on its own grid, or a bound, where `max` is the highest
/// value on the grid. With `range.snap`, a step stops at the first point it
/// reaches ahead, and leaves the point it's on freely; `snap: false` steps past them.
pub fn step_by(value: f64, step: NumberMove, range: &NumberRange, snap: bool) -> f64 {
    let (direction, size) = match step {
        NumberMove::Bound(NumberBound::Min) => return range.min,
        NumberMove::Bound(NumberBound::Max) => return max_on_grid(range.step_range()),
        NumberMove::Step { direction, size } => (direction, size),
    };
    let next = step_on_grid(value, direction, size, range);
    let Some(points) = range.snap.as_ref().filter(|_| snap) else {
        return next;
    };
    let sign = direction.sign();
    let mut stop: Option<f64> = None;
    for &point in points.points.iter() {
        let ahead = (point - value) * sign > 0.0;
        let reached = (next + sign * points.threshold - point) * sign >= 0.0;
        if ahead && reached && stop.is_none_or(|stop| (point - stop) * sign < 0.0) {
            stop = Some(point);
        }
    }
    match stop {
        Some(stop) => clamp(stop, range.min, range.max),
        None => next,
    }
}

/// Where a scrub is: its `value`, and the snap `point` it's pulled to, or
/// `None` when it's on its own, so a release can commit `point.unwrap_or(value)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberScrub {
    pub value: f64,
    pub point: Option<f64>,
}

/// How a value near a snap point moves toward it, for [`ScrubOptions::ease`].
#[derive(Clone)]
pub enum ScrubEase {
    /// Pulls in proportion to closeness, from 0 at the threshold to 1 on the point.
    Linear,
    /// A curve over closeness, from 0 at the threshold to 1 on the point.
    Curve(Rc<dyn Fn(f64) -> f64>),
}

impl fmt::Debug for ScrubEase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScrubEase::Linear => f.write_str("Linear"),
            ScrubEase::Curve(_) => f.write_str("Curve"),
        }
    }
}

/// What [`scrub_to`] takes besides the value and the range.
#[derive(Clone, Debug)]
pub struct ScrubOptions {
    /// Lands on the range's snap points. `false` ignores them, like for typed
    /// numbers or while Cmd is held, and keeps the grid. On by default.
    pub snap: bool,
    /// Moves a value near a point toward it instead of jumping there.
    pub ease: Option<ScrubEase>,
}

impl Default for ScrubOptions {
    fn default() -> Self {
        Self {
            snap: true,
            ease: None,
        }
    }
}

impl ScrubOptions {
    pub fn snap(mut self, snap: bool) -> Self {
        self.snap = snap;
        self
    }

    /// Pulls a value toward a point in proportion to closeness (`ease: true`).
    pub fn ease(mut self) -> Self {
        self.ease = Some(ScrubEase::Linear);
        self
    }

    /// Pulls a value toward a point along a curve over closeness.
    pub fn ease_with(mut self, curve: impl Fn(f64) -> f64 + 'static) -> Self {
        self.ease = Some(ScrubEase::Curve(Rc::new(curve)));
        self
    }
}

/// Where a value lands in a range: on the nearest of `range.snap`'s points
/// within its threshold, or else on the step grid within the bounds.
/// `snap: false` ignores the points, like for typed numbers or while Cmd is
/// held, and keeps the grid; a range with a step of 0 keeps values exactly.
///
/// With `ease`, a value near a point moves toward it instead of jumping there:
/// not at all at the threshold, faster as it closes in, and onto the point at
/// the end. [`ScrubEase::Linear`] pulls in proportion to closeness, from 0 at
/// the threshold to 1 on the point; pass a curve over closeness to change it.
pub fn scrub_to(value: f64, range: &NumberRange, options: &ScrubOptions) -> NumberScrub {
    let mut nearest: Option<f64> = None;
    let threshold = range.snap.as_ref().map_or(0.0, |snap| snap.threshold);
    if options.snap
        && let Some(snap) = &range.snap
    {
        for &point in snap.points.iter() {
            if (point - value).abs() <= threshold
                && nearest.is_none_or(|nearest| (point - value).abs() < (nearest - value).abs())
            {
                nearest = Some(point);
            }
        }
    }
    let Some(nearest) = nearest else {
        return NumberScrub {
            value: snap_to_step(value, range.step_range()),
            point: None,
        };
    };
    let point = clamp(nearest, range.min, range.max);
    let Some(ease) = options.ease.as_ref().filter(|_| threshold > 0.0) else {
        return NumberScrub {
            value: point,
            point: Some(point),
        };
    };
    let closeness = 1.0 - (point - value).abs() / threshold;
    let pull = match ease {
        ScrubEase::Linear => closeness,
        ScrubEase::Curve(curve) => curve(closeness),
    };
    NumberScrub {
        value: clamp(value + (point - value) * pull, range.min, range.max),
        point: Some(point),
    }
}

// `Math.max`: NaN wins, and +0 is above -0.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == b {
        if a.is_sign_negative() { b } else { a }
    } else if a > b {
        a
    } else {
        b
    }
}

// `Math.min`: NaN wins, and -0 is below +0.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == b {
        if a.is_sign_negative() { a } else { b }
    } else if a < b {
        a
    } else {
        b
    }
}

// `Math.round`: the nearest integer, halves toward +∞, and -0 for values
// from -0.5 up to 0.
fn math_round(value: f64) -> f64 {
    if !value.is_finite() || value.fract() == 0.0 {
        return value;
    }
    let floor = value.floor();
    let rounded = if value - floor >= 0.5 { floor + 1.0 } else { floor };
    if rounded == 0.0 && value < 0.0 { -0.0 } else { rounded }
}

// `Number(value.toPrecision(digits))`: rounded to significant digits, halves
// away from zero, on the exact binary value.
fn to_precision(value: f64, digits: i32) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    let exact = Exact::of(value.abs());
    let rounded = exact.rounded_at(exact.magnitude - digits + 1);
    if value < 0.0 { -rounded } else { rounded }
}

// The exact decimal expansion of a finite, non-negative double, which always
// ends: its digits without leading or trailing zeros, and the power of ten of
// the first one.
struct Exact {
    digits: Vec<u8>,
    magnitude: i32,
}

impl Exact {
    fn of(value: f64) -> Self {
        // A double is an integer times a power of two, so its expansion has as
        // many decimals as that power's negative, and Rust writes that many
        // decimals exactly.
        let text = format!("{:.*}", binary_decimals(value), value);
        let (integer, fraction) = text.split_once('.').unwrap_or((&text, ""));
        let mut digits: Vec<u8> = integer.bytes().chain(fraction.bytes()).collect();
        let mut magnitude = integer.len() as i32 - 1;
        let leading = digits.iter().take_while(|digit| **digit == b'0').count();
        if leading == digits.len() {
            return Self {
                digits: vec![b'0'],
                magnitude: 0,
            };
        }
        digits.drain(..leading);
        magnitude -= leading as i32;
        while digits.last() == Some(&b'0') {
            digits.pop();
        }
        Self { digits, magnitude }
    }

    // The value rounded at the decimal `position`, keeping the digits whose
    // power of ten is at least `position`; halves round up.
    fn rounded_at(&self, position: i32) -> f64 {
        let keep = self.magnitude - position + 1;
        if keep < 0 || self.digits == [b'0'] {
            return 0.0;
        }
        let keep = keep as usize;
        let mut kept: Vec<u8> = self.digits.iter().take(keep).copied().collect();
        kept.resize(keep, b'0');
        let up = self.digits.get(keep).is_some_and(|digit| *digit >= b'5');
        if up {
            let mut carry = true;
            for digit in kept.iter_mut().rev() {
                if *digit == b'9' {
                    *digit = b'0';
                } else {
                    *digit += 1;
                    carry = false;
                    break;
                }
            }
            if carry {
                kept.insert(0, b'1');
            }
        }
        if kept.is_empty() {
            return 0.0;
        }
        let text = format!("{}e{}", String::from_utf8_lossy(&kept), position);
        text.parse().unwrap_or(0.0)
    }
}

// How many decimals a non-negative double's exact value has.
fn binary_decimals(value: f64) -> usize {
    if value == 0.0 {
        return 0;
    }
    let bits = value.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mantissa, exponent) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), biased - 1075)
    };
    let exponent = exponent + mantissa.trailing_zeros() as i32;
    if exponent >= 0 { 0 } else { (-exponent) as usize }
}
