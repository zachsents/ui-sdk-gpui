//! Port of `foundation/color/mix.ts`: CSS `color-mix()`.

use std::fmt;

use crate::convert::{Color, ColorSpace, convert_color, js_max, js_min};

/// Which way a mix goes around the hue wheel, as in CSS `color-mix()`:
/// shorter, longer, increasing or decreasing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorHueInterpolation {
    #[default]
    Shorter,
    Longer,
    Increasing,
    Decreasing,
}

impl ColorHueInterpolation {
    /// The method's name, as the original writes it, like `"shorter"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorHueInterpolation::Shorter => "shorter",
            ColorHueInterpolation::Longer => "longer",
            ColorHueInterpolation::Increasing => "increasing",
            ColorHueInterpolation::Decreasing => "decreasing",
        }
    }
}

impl fmt::Display for ColorHueInterpolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Options for [`mix_color`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorMixOptions {
    /// The space the colors mix in, which the result is in too. Oklab by
    /// default.
    pub space: ColorSpace,
    /// Which way the hue goes, in a space with one. Shorter by default.
    pub hue: ColorHueInterpolation,
}

impl Default for ColorMixOptions {
    fn default() -> Self {
        Self {
            space: ColorSpace::Oklab,
            hue: ColorHueInterpolation::Shorter,
        }
    }
}

impl ColorMixOptions {
    pub fn space(mut self, space: ColorSpace) -> Self {
        self.space = space;
        self
    }

    pub fn hue(mut self, hue: ColorHueInterpolation) -> Self {
        self.hue = hue;
        self
    }
}

fn hue_index(space: ColorSpace) -> Option<usize> {
    match space {
        ColorSpace::Hsl | ColorSpace::Hsv | ColorSpace::Hwb => Some(0),
        ColorSpace::Lch | ColorSpace::Oklch => Some(2),
        _ => None,
    }
}

fn normalize(hue: f64) -> f64 {
    ((hue % 360.0) + 360.0) % 360.0
}

// CSS Color 4 adjusts the two hues before interpolating, by the method's rule.
fn fix_hues(mut first: f64, mut second: f64, method: ColorHueInterpolation) -> (f64, f64) {
    let delta = second - first;
    match method {
        ColorHueInterpolation::Shorter => {
            if delta > 180.0 {
                first += 360.0;
            } else if delta < -180.0 {
                second += 360.0;
            }
        }
        ColorHueInterpolation::Longer => {
            if delta > 0.0 && delta < 180.0 {
                first += 360.0;
            } else if delta > -180.0 && delta <= 0.0 {
                second += 360.0;
            }
        }
        ColorHueInterpolation::Increasing => {
            if delta < 0.0 {
                second += 360.0;
            }
        }
        ColorHueInterpolation::Decreasing => {
            if delta > 0.0 {
                first += 360.0;
            }
        }
    }
    (first, second)
}

/// Mixes two colors like CSS `color-mix()`: `amount` of the second, from 0 to
/// 1 (the original's default is 0.5, halfway), in Oklab by default. Channels
/// mix premultiplied by alpha, and a channel missing from one color takes the
/// other's.
pub fn mix_color(first: &Color, second: &Color, amount: f64, options: ColorMixOptions) -> Color {
    let space = options.space;
    let hue = hue_index(space);
    let weight = js_min(1.0, js_max(0.0, amount));
    let a = convert_color(first, space);
    let b = convert_color(second, space);
    let alpha = a.alpha + (b.alpha - a.alpha) * weight;
    let mut coords = [None; 3];
    for (index, coord) in coords.iter_mut().enumerate() {
        let (own, other) = (a.coords[index], b.coords[index]);
        let (from, to) = match (own, other) {
            (None, None) => continue,
            (Some(own), None) => (own, own),
            (None, Some(other)) => (other, other),
            (Some(own), Some(other)) => (own, other),
        };
        *coord = Some(if Some(index) == hue {
            let (start, end) = fix_hues(normalize(from), normalize(to), options.hue);
            normalize(start + (end - start) * weight)
        } else {
            let mixed = from * a.alpha + (to * b.alpha - from * a.alpha) * weight;
            if alpha == 0.0 {
                from + (to - from) * weight
            } else {
                mixed / alpha
            }
        });
    }
    Color {
        space,
        coords,
        alpha,
    }
}
