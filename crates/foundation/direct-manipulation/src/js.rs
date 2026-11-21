//! JavaScript's number semantics where the original's results show them:
//! `Math.round`, `Math.sign`, `Math.min` and `Math.max` with NaN, and a
//! number as a template string writes it.

/// `Math.round`: the nearest integer, halves toward +∞, and -0 for values
/// that round up to zero from below.
pub(crate) fn round(value: f64) -> f64 {
    if !value.is_finite() || value.fract() == 0.0 {
        return value;
    }
    let floor = value.floor();
    let rounded = if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    };
    if rounded == 0.0 && value < 0.0 {
        -0.0
    } else {
        rounded
    }
}

/// `Math.sign`: 1 or -1, and the value itself for 0, -0 and NaN, where
/// `f64::signum` gives 1 for 0.
pub(crate) fn sign(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        value
    }
}

/// `Math.min`: NaN wins, and -0 is below +0.
pub(crate) fn min(a: f64, b: f64) -> f64 {
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

/// `Math.max`: NaN wins, and +0 is above -0.
pub(crate) fn max(a: f64, b: f64) -> f64 {
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

/// A number as `${value}` writes it, like `540` or `1.5`.
pub(crate) fn number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if value == 0.0 {
        return "0".into();
    }
    let size = value.abs();
    if size >= 1e21 || size < 1e-6 {
        // Exponent form, with a sign on the exponent, like `1e+21`.
        let text = format!("{value:e}");
        return match text.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => {
                format!("{mantissa}e+{exponent}")
            }
            _ => text,
        };
    }
    if value.fract() == 0.0 {
        return format!("{value:.0}");
    }
    format!("{value}")
}

/// A fraction rounded as the original writes percentages in styles,
/// `Math.round(n * 1e4) / 100` percent, as a fraction for gpui's `relative`.
pub(crate) fn style_fraction(value: f64) -> f32 {
    (round(value * 1e4) / 1e4) as f32
}
