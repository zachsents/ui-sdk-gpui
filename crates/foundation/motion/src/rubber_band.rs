//! Port of `foundation/motion/rubber-band.ts`.
//!
//! UIScrollView's rubber band: x px of travel past an edge shows as
//! (1 − 1 / (x·c / d + 1))·d, which approaches but never reaches d.
//! <https://oleb.net/blog/2014/05/scrollview-inertia-bouncing-rubberbanding/>

use crate::js;

/// The coefficient iOS scroll views use, `c` above.
pub const IOS_RUBBER_BAND_COEFFICIENT: f64 = 0.55;

/// How far something dragged `overshoot` px past an edge shows, resisting
/// more the further it goes, like iOS.
pub fn rubber_band(overshoot: f64, dimension: f64) -> f64 {
    rubber_band_with(overshoot, dimension, IOS_RUBBER_BAND_COEFFICIENT)
}

/// `rubber_band` with its own coefficient.
pub fn rubber_band_with(overshoot: f64, dimension: f64, coefficient: f64) -> f64 {
    if overshoot <= 0.0 {
        return 0.0;
    }
    (1.0 - 1.0 / ((overshoot * coefficient) / dimension + 1.0)) * dimension
}

/// The travel that shows as `shown`. Used when a drag starts inside the band,
/// so the finger picks up the sheet where it is instead of jumping.
pub fn rubber_band_inverse(shown: f64, dimension: f64, coefficient: f64) -> f64 {
    if shown <= 0.0 {
        return 0.0;
    }
    let ratio = js::min(shown / dimension, 0.999999);
    ((1.0 / (1.0 - ratio) - 1.0) * dimension) / coefficient
}

/// d(shown)/d(travel). A release inside the band hands the spring the
/// finger's velocity times this slope, so the sheet doesn't jump in speed.
pub fn rubber_band_slope(overshoot: f64, dimension: f64, coefficient: f64) -> f64 {
    if overshoot <= 0.0 {
        return 1.0;
    }
    let base = (overshoot * coefficient) / dimension + 1.0;
    coefficient / (base * base)
}
