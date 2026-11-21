//! Port of `foundation/color/gamut.ts`: what a screen can show.

use std::fmt;

use crate::compare::color_distance;
use crate::convert::{Color, ColorSpace, convert_color, js_max, js_min};

/// A range of colors a screen can show: [`ColorGamut::Srgb`], which every
/// screen shows, or [`ColorGamut::P3`], the wider range of most phones and
/// laptops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorGamut {
    #[default]
    Srgb,
    P3,
}

impl ColorGamut {
    /// The gamut's name, as the original writes it: `"srgb"` or `"p3"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorGamut::Srgb => "srgb",
            ColorGamut::P3 => "p3",
        }
    }
}

impl fmt::Display for ColorGamut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn space_of(gamut: ColorGamut) -> ColorSpace {
    match gamut {
        ColorGamut::P3 => ColorSpace::DisplayP3,
        ColorGamut::Srgb => ColorSpace::Srgb,
    }
}

// How far past an edge still counts as inside, so a color that round-trips
// through another space doesn't fall out of its gamut.
const TOLERANCE: f64 = 0.000075;

const JUST_NOTICEABLE: f64 = 0.02;

fn clip(color: &Color) -> Color {
    Color {
        coords: color
            .coords
            .map(|value| Some(js_min(1.0, js_max(0.0, value.unwrap_or(0.0))))),
        ..*color
    }
}

/// Whether a screen with this gamut can show the color. sRGB is the
/// original's default.
pub fn is_in_gamut(color: &Color, gamut: ColorGamut) -> bool {
    convert_color(color, space_of(gamut))
        .coords
        .iter()
        .all(|value| {
            let value = value.unwrap_or(0.0);
            (-TOLERANCE..=1.0 + TOLERANCE).contains(&value)
        })
}

/// The closest color the gamut holds, in the color's own space. It keeps the
/// lightness and hue and lowers the chroma in OKLCH until the color fits, the
/// way CSS Color 4 maps colors (binary search with local MINDE). sRGB is the
/// original's default.
pub fn to_gamut(color: &Color, gamut: ColorGamut) -> Color {
    convert_color(&map_to_gamut(color, gamut), color.space)
}

fn map_to_gamut(color: &Color, gamut: ColorGamut) -> Color {
    let space = space_of(gamut);
    let origin = convert_color(color, ColorSpace::Oklch);
    let [lightness, chroma, hue] = origin.coords;
    if lightness.unwrap_or(0.0) >= 1.0 {
        return Color::new(space, [1.0, 1.0, 1.0], color.alpha);
    }
    if lightness.unwrap_or(0.0) <= 0.0 {
        return Color::new(space, [0.0, 0.0, 0.0], color.alpha);
    }
    if is_in_gamut(&origin, gamut) {
        return *color;
    }
    let with_chroma = |value: f64| Color {
        coords: [lightness, Some(value), hue],
        ..origin
    };
    let mut clipped = clip(&convert_color(&origin, space));
    if color_distance(&clipped, &origin) < JUST_NOTICEABLE {
        return clipped;
    }
    let mut min = 0.0;
    let mut max = chroma.unwrap_or(0.0);
    let mut min_in_gamut = true;
    while max - min > 0.0001 {
        let middle = (min + max) / 2.0;
        let current = with_chroma(middle);
        if min_in_gamut && is_in_gamut(&current, gamut) {
            min = middle;
            continue;
        }
        clipped = clip(&convert_color(&current, space));
        let distance = color_distance(&clipped, &current);
        if distance >= JUST_NOTICEABLE {
            max = middle;
        } else if JUST_NOTICEABLE - distance < 0.0001 {
            return clipped;
        } else {
            min_in_gamut = false;
            min = middle;
        }
    }
    clipped
}
