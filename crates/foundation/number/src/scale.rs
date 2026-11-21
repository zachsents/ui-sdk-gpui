//! Port of `foundation/number/scale.ts`.
//!
//! Where a value sits between bounds, and the value at a position, evenly or
//! along a scale. Pure, so it runs anywhere.

use std::fmt;
use std::rc::Rc;

use crate::steps::clamp;

/// How positions map to values: evenly ([`NumberScale::Linear`]), by ratio
/// ([`NumberScale::Log`], for volume, frequency or zoom, with a positive
/// `min`), or through your own `to` and `from`, which map values into an even
/// space and back.
#[derive(Clone, Default)]
pub enum NumberScale {
    #[default]
    Linear,
    Log,
    Custom {
        to: Rc<dyn Fn(f64) -> f64>,
        from: Rc<dyn Fn(f64) -> f64>,
    },
}

impl NumberScale {
    /// A scale through your own `to`, from values into an even space, and
    /// `from`, back.
    pub fn custom(to: impl Fn(f64) -> f64 + 'static, from: impl Fn(f64) -> f64 + 'static) -> Self {
        NumberScale::Custom {
            to: Rc::new(to),
            from: Rc::new(from),
        }
    }

    fn to(&self, value: f64) -> f64 {
        match self {
            NumberScale::Linear => value,
            NumberScale::Log => value.ln(),
            NumberScale::Custom { to, .. } => to(value),
        }
    }

    fn from(&self, position: f64) -> f64 {
        match self {
            NumberScale::Linear => position,
            NumberScale::Log => position.exp(),
            NumberScale::Custom { from, .. } => from(position),
        }
    }
}

impl fmt::Debug for NumberScale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumberScale::Linear => f.write_str("Linear"),
            NumberScale::Log => f.write_str("Log"),
            NumberScale::Custom { .. } => f.write_str("Custom"),
        }
    }
}

impl PartialEq for NumberScale {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (NumberScale::Linear, NumberScale::Linear) | (NumberScale::Log, NumberScale::Log) => true,
            (NumberScale::Custom { to: a, from: b }, NumberScale::Custom { to: c, from: d }) => {
                Rc::ptr_eq(a, c) && Rc::ptr_eq(b, d)
            }
            _ => false,
        }
    }
}

/// Where a value sits between `min` and `max`, from 0 to 1, along a scale: how
/// far to fill a progress bar or place a slider's thumb. Values outside the
/// bounds sit at an end, and bounds that are the same give 0.
pub fn fraction_of(value: f64, min: f64, max: f64, scale: &NumberScale) -> f64 {
    let low = scale.to(min);
    let span = scale.to(max) - low;
    let fraction = (scale.to(clamp(value, min, max)) - low) / span;
    if fraction.is_finite() {
        clamp(fraction, 0.0, 1.0)
    } else {
        0.0
    }
}

/// The value at a fraction from 0 to 1 between `min` and `max`, along a scale,
/// like where a press on a track lands. Unsnapped; [`scrub_to`] puts it on the grid.
///
/// [`scrub_to`]: crate::scrub_to
pub fn value_at(fraction: f64, min: f64, max: f64, scale: &NumberScale) -> f64 {
    let low = scale.to(min);
    let value = scale.from(low + clamp(fraction, 0.0, 1.0) * (scale.to(max) - low));
    clamp(value, min, max)
}
