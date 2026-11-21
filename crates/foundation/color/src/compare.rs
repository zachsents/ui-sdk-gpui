//! Port of `foundation/color/compare.ts`: how far apart two colors look, and
//! how well one reads on the other.

use crate::convert::{Color, ColorSpace, convert_color, hypot, js_max};

/// How different two colors look, as ΔEOK: the distance between them in
/// Oklab. About 0.02 is the smallest difference most people notice. Alpha
/// doesn't count.
pub fn color_distance(first: &Color, second: &Color) -> f64 {
    let [l1, a1, b1] = convert_color(first, ColorSpace::Oklab).coords;
    let [l2, a2, b2] = convert_color(second, ColorSpace::Oklab).coords;
    hypot(&[
        l1.unwrap_or(0.0) - l2.unwrap_or(0.0),
        a1.unwrap_or(0.0) - a2.unwrap_or(0.0),
        b1.unwrap_or(0.0) - b2.unwrap_or(0.0),
    ])
}

fn luminance(color: &Color) -> f64 {
    js_max(
        0.0,
        convert_color(color, ColorSpace::XyzD65).coords[1].unwrap_or(0.0),
    )
}

/// The WCAG 2 contrast ratio between two colors, from 1 to 21, whichever is
/// lighter. Text needs 4.5 against its background, or 3 when it's large.
/// Alpha doesn't count, so blend a translucent color over its background first.
pub fn contrast_ratio(first: &Color, second: &Color) -> f64 {
    let (a, b) = (luminance(first), luminance(second));
    // `[a, b].sort((a, b) => b - a)`: the lighter first. A comparison with
    // `NaN` keeps the order.
    let (lighter, darker) = if b - a > 0.0 { (b, a) } else { (a, b) };
    (lighter + 0.05) / (darker + 0.05)
}
