//! JavaScript's number semantics, which the original's math leans on, so the
//! port computes what it does: `Math.min` and `Math.max` carry NaN,
//! `Math.sign(0)` is 0, `Math.round` rounds halves up, and `String(number)`
//! names numbers the way snap point ids and CSS `linear()` stops read.

/// `Math.min(a, b)`: NaN when either is.
pub(crate) fn min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

/// `Math.max(a, b)`: NaN when either is.
pub(crate) fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `Math.sign(value)`: 1, -1, or the value itself for zero and NaN.
pub(crate) fn sign(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        value
    }
}

/// `Math.round(value)`: the nearest integer, halves toward +∞.
pub(crate) fn round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// `String(value)`: the shortest digits that read back as the same number,
/// in exponent form below 1e-6 and from 1e21.
pub(crate) fn number_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let magnitude = value.abs();
    if (1e-6..1e21).contains(&magnitude) {
        return format!("{value}");
    }
    let written = format!("{value:e}");
    match written.split_once('e') {
        Some((digits, exponent)) if !exponent.starts_with('-') => format!("{digits}e+{exponent}"),
        _ => written,
    }
}
