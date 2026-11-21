//! Port of `foundation/color/convert.ts`: the spaces a color can be in, and
//! the same color in another one.
//!
//! The math runs in `f64`, as JavaScript's numbers do, with the original's
//! matrices, constants and order of operations. Where Rust's numbers behave
//! differently from JavaScript's (`Math.max` with `NaN` or `-0`, `Math.hypot`
//! of three values), the helpers at the bottom do what JavaScript does.

use std::fmt;
use std::str::FromStr;

use gpui::{Background, Fill, Hsla, Rgba};

/// A color space a [`Color`] can hold its channels in. The names are CSS's:
/// `color(display-p3 …)`, `oklch(…)` and so on, plus [`ColorSpace::Hsv`],
/// which CSS has no syntax for but pickers draw their square in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    Srgb,
    SrgbLinear,
    DisplayP3,
    Hsl,
    Hsv,
    Hwb,
    Lab,
    Lch,
    Oklab,
    Oklch,
    XyzD65,
    XyzD50,
}

impl ColorSpace {
    /// Every space, in the order the original lists them.
    pub const ALL: [ColorSpace; 12] = [
        ColorSpace::Srgb,
        ColorSpace::SrgbLinear,
        ColorSpace::DisplayP3,
        ColorSpace::Hsl,
        ColorSpace::Hsv,
        ColorSpace::Hwb,
        ColorSpace::Lab,
        ColorSpace::Lch,
        ColorSpace::Oklab,
        ColorSpace::Oklch,
        ColorSpace::XyzD65,
        ColorSpace::XyzD50,
    ];

    /// The space's name, as the original writes it: `"srgb"`, `"display-p3"`,
    /// `"oklch"` and so on.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorSpace::Srgb => "srgb",
            ColorSpace::SrgbLinear => "srgb-linear",
            ColorSpace::DisplayP3 => "display-p3",
            ColorSpace::Hsl => "hsl",
            ColorSpace::Hsv => "hsv",
            ColorSpace::Hwb => "hwb",
            ColorSpace::Lab => "lab",
            ColorSpace::Lch => "lch",
            ColorSpace::Oklab => "oklab",
            ColorSpace::Oklch => "oklch",
            ColorSpace::XyzD65 => "xyz-d65",
            ColorSpace::XyzD50 => "xyz-d50",
        }
    }
}

impl fmt::Display for ColorSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The text isn't the name of a color space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownColorSpace(pub String);

impl fmt::Display for UnknownColorSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\" is not a color space", self.0)
    }
}

impl std::error::Error for UnknownColorSpace {}

impl FromStr for ColorSpace {
    type Err = UnknownColorSpace;

    /// Reads a space's name, like `"oklch"`.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        ColorSpace::ALL
            .into_iter()
            .find(|space| space.as_str() == name)
            .ok_or_else(|| UnknownColorSpace(name.to_string()))
    }
}

/// A color as plain data: its space, three channels and an alpha from 0 to 1.
/// Channels use CSS's ranges: 0 to 1 for `srgb`, `display-p3` and Oklab's
/// lightness, 0 to 100 for percentages, degrees for hues. A `None` channel is
/// missing, CSS's `none`, like the hue of a grey.
///
/// gpui paints a color through [`Hsla`], [`Rgba`] or a fill, which it
/// converts to: `.bg(color)`, `.text_color(color)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    /// The space the channels are in.
    pub space: ColorSpace,
    /// The three channels, in the order CSS writes them.
    pub coords: [Option<f64>; 3],
    /// How opaque the color is, from 0 to 1.
    pub alpha: f64,
}

impl Color {
    /// A color with all three channels present.
    pub const fn new(space: ColorSpace, coords: [f64; 3], alpha: f64) -> Self {
        Self {
            space,
            coords: [Some(coords[0]), Some(coords[1]), Some(coords[2])],
            alpha,
        }
    }

    /// The color as gpui paints it: in sRGB, clipped to what sRGB holds, as
    /// `rgb()` writes a wider color. Map it with `to_gamut` first to keep its
    /// lightness and hue.
    pub fn to_rgba(&self) -> Rgba {
        let [red, green, blue] = clipped_srgb(self);
        Rgba {
            r: red as f32,
            g: green as f32,
            b: blue as f32,
            a: paint_alpha(self.alpha) as f32,
        }
    }

    /// The color as gpui's [`Hsla`]: in sRGB, clipped to what sRGB holds, with
    /// hue, saturation and lightness from 0 to 1. A grey's missing hue is 0.
    pub fn to_hsla(&self) -> Hsla {
        let [hue, saturation, lightness] = srgb_to_hsl(clipped_srgb(self));
        let unit = |value: f64| if value.is_nan() { 0.0 } else { value.clamp(0.0, 1.0) };
        Hsla {
            h: unit(hue / 360.0) as f32,
            s: unit(saturation / 100.0) as f32,
            l: unit(lightness / 100.0) as f32,
            a: paint_alpha(self.alpha) as f32,
        }
    }
}

// The sRGB channels gpui paints: missing channels count as 0, and each is
// clipped to 0 to 1. A channel that isn't a number paints as 0.
fn clipped_srgb(color: &Color) -> Coords {
    convert_color(color, ColorSpace::Srgb)
        .coords
        .map(|value| match value {
            Some(value) if !value.is_nan() => value.clamp(0.0, 1.0),
            _ => 0.0,
        })
}

fn paint_alpha(alpha: f64) -> f64 {
    if alpha.is_nan() { 0.0 } else { alpha.clamp(0.0, 1.0) }
}

impl From<Color> for Rgba {
    fn from(color: Color) -> Self {
        color.to_rgba()
    }
}

impl From<&Color> for Rgba {
    fn from(color: &Color) -> Self {
        color.to_rgba()
    }
}

impl From<Color> for Hsla {
    fn from(color: Color) -> Self {
        color.to_hsla()
    }
}

impl From<&Color> for Hsla {
    fn from(color: &Color) -> Self {
        color.to_hsla()
    }
}

impl From<Color> for Background {
    fn from(color: Color) -> Self {
        color.to_hsla().into()
    }
}

impl From<Color> for Fill {
    fn from(color: Color) -> Self {
        color.to_hsla().into()
    }
}

impl From<Rgba> for Color {
    /// gpui's sRGB color, in the `srgb` space.
    fn from(color: Rgba) -> Self {
        Color::new(
            ColorSpace::Srgb,
            [color.r.into(), color.g.into(), color.b.into()],
            color.a.into(),
        )
    }
}

impl From<Hsla> for Color {
    /// gpui's HSL color, in the `hsl` space with CSS's ranges.
    fn from(color: Hsla) -> Self {
        Color::new(
            ColorSpace::Hsl,
            [
                f64::from(color.h) * 360.0,
                f64::from(color.s) * 100.0,
                f64::from(color.l) * 100.0,
            ],
            color.a.into(),
        )
    }
}

pub(crate) type Coords = [f64; 3];

type Matrix = [Coords; 3];

const LINEAR_SRGB_TO_XYZ: Matrix = [
    [506752.0 / 1228815.0, 87881.0 / 245763.0, 12673.0 / 70218.0],
    [87098.0 / 409605.0, 175762.0 / 245763.0, 12673.0 / 175545.0],
    [7918.0 / 409605.0, 87881.0 / 737289.0, 1001167.0 / 1053270.0],
];

const XYZ_TO_LINEAR_SRGB: Matrix = [
    [12831.0 / 3959.0, -329.0 / 214.0, -1974.0 / 3959.0],
    [-851781.0 / 878810.0, 1648619.0 / 878810.0, 36519.0 / 878810.0],
    [705.0 / 12673.0, -2585.0 / 12673.0, 705.0 / 667.0],
];

const LINEAR_P3_TO_XYZ: Matrix = [
    [608311.0 / 1250200.0, 189793.0 / 714400.0, 198249.0 / 1000160.0],
    [35783.0 / 156275.0, 247089.0 / 357200.0, 198249.0 / 2500400.0],
    [0.0, 32229.0 / 714400.0, 5220557.0 / 5000800.0],
];

const XYZ_TO_LINEAR_P3: Matrix = [
    [446124.0 / 178915.0, -333277.0 / 357830.0, -72051.0 / 178915.0],
    [-14852.0 / 17905.0, 63121.0 / 35810.0, 423.0 / 17905.0],
    [11844.0 / 330415.0, -50337.0 / 660830.0, 316169.0 / 330415.0],
];

const D65_TO_D50: Matrix = [
    [1.0479297925449969, 0.022946870601609652, -0.05019226628920524],
    [0.02962780877005599, 0.9904344267538799, -0.017073799063418826],
    [-0.009243040646204504, 0.015055191490298152, 0.7518742814281371],
];

const D50_TO_D65: Matrix = [
    [0.955473421488075, -0.02309845494876471, 0.06325924320057072],
    [-0.0283697093338637, 1.0099953980813041, 0.021041441191917323],
    [0.012314014864481998, -0.020507649298898964, 1.330365926242124],
];

const XYZ_TO_LMS: Matrix = [
    [0.819022437996703, 0.3619062600528904, -0.1288737815209879],
    [0.0329836539323885, 0.9292868615863434, 0.0361446663506424],
    [0.0481771893596242, 0.2642395317527308, 0.6335478284694309],
];

const LMS_TO_OKLAB: Matrix = [
    [0.210454268309314, 0.7936177747023054, -0.0040720430116193],
    [1.9779985324311684, -2.42859224204858, 0.450593709617411],
    [0.0259040424655478, 0.7827717124575296, -0.8086757548750474],
];

const OKLAB_TO_LMS: Matrix = [
    [1.0, 0.3963377773761749, 0.2158037573099136],
    [1.0, -0.1055613458156586, -0.0638541728258133],
    [1.0, -0.0894841775298119, -1.2914855480194092],
];

const LMS_TO_XYZ: Matrix = [
    [1.2268798758459243, -0.5578149944602171, 0.2813910456659647],
    [-0.0405757452148008, 1.112286803280317, -0.0717110580655164],
    [-0.0763729366746601, -0.4214933324022432, 1.5869240198367816],
];

const D50_WHITE: Coords = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];

const KAPPA: f64 = 24389.0 / 27.0;

const EPSILON: f64 = 216.0 / 24389.0;

// Below these, a hue no longer shows, so it's stored as missing, as CSS does.
const ACHROMATIC_LCH: f64 = 0.0015;
const ACHROMATIC_OKLCH: f64 = 0.000004;
const ACHROMATIC_RGB: f64 = 1e-7;

fn multiply(matrix: &Matrix, [a, b, c]: Coords) -> Coords {
    [
        matrix[0][0] * a + matrix[0][1] * b + matrix[0][2] * c,
        matrix[1][0] * a + matrix[1][1] * b + matrix[1][2] * c,
        matrix[2][0] * a + matrix[2][1] * b + matrix[2][2] * c,
    ]
}

fn to_linear(value: f64) -> f64 {
    let size = value.abs();
    if size <= 0.04045 {
        value / 12.92
    } else {
        sign(value) * ((size + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(value: f64) -> f64 {
    let size = value.abs();
    if size > 0.0031308 {
        sign(value) * (1.055 * size.powf(1.0 / 2.4) - 0.055)
    } else {
        12.92 * value
    }
}

pub(crate) fn normalize_hue(hue: f64) -> f64 {
    ((hue % 360.0) + 360.0) % 360.0
}

fn rgb_hue([red, green, blue]: Coords, max: f64, delta: f64) -> f64 {
    if delta < ACHROMATIC_RGB {
        return f64::NAN;
    }
    if max == red {
        return 60.0 * ((green - blue) / delta + if green < blue { 6.0 } else { 0.0 });
    }
    if max == green {
        return 60.0 * ((blue - red) / delta + 2.0);
    }
    60.0 * ((red - green) / delta + 4.0)
}

fn srgb_to_hsl(rgb: Coords) -> Coords {
    let max = max_of(&rgb);
    let min = min_of(&rgb);
    let lightness = (max + min) / 2.0;
    let mut hue = rgb_hue(rgb, max, max - min);
    let mut saturation = if hue.is_nan() || lightness == 0.0 || lightness == 1.0 {
        0.0
    } else {
        (max - lightness) / js_min(lightness, 1.0 - lightness)
    };
    if saturation < 0.0 {
        hue += 180.0;
        saturation = -saturation;
    }
    [normalize_hue(hue), saturation * 100.0, lightness * 100.0]
}

fn hsl_to_srgb([hue, saturation, lightness]: Coords) -> Coords {
    let s = saturation / 100.0;
    let l = lightness / 100.0;
    let channel = |n: f64| {
        let k = (n + hue / 30.0) % 12.0;
        l - s * js_min(l, 1.0 - l) * js_max(-1.0, min_of(&[k - 3.0, 9.0 - k, 1.0]))
    };
    [channel(0.0), channel(8.0), channel(4.0)]
}

fn srgb_to_hsv(rgb: Coords) -> Coords {
    let max = max_of(&rgb);
    let delta = max - min_of(&rgb);
    let hue = rgb_hue(rgb, max, delta);
    [
        normalize_hue(hue),
        if hue.is_nan() || max == 0.0 {
            0.0
        } else {
            (delta / max) * 100.0
        },
        max * 100.0,
    ]
}

fn hsv_to_srgb([hue, saturation, value]: Coords) -> Coords {
    let s = saturation / 100.0;
    let v = value / 100.0;
    let channel = |n: f64| {
        let k = (n + hue / 60.0) % 6.0;
        v - v * s * js_max(0.0, min_of(&[k, 4.0 - k, 1.0]))
    };
    [channel(5.0), channel(3.0), channel(1.0)]
}

fn hsv_to_hwb([hue, saturation, value]: Coords) -> Coords {
    [hue, (100.0 - saturation) * (value / 100.0), 100.0 - value]
}

fn hwb_to_hsv([hue, whiteness, blackness]: Coords) -> Coords {
    let total = whiteness + blackness;
    if total >= 100.0 {
        return [hue, 0.0, (whiteness / total) * 100.0];
    }
    let value = 100.0 - blackness;
    [
        hue,
        if value == 0.0 {
            0.0
        } else {
            (1.0 - whiteness / value) * 100.0
        },
        value,
    ]
}

fn xyz_to_lab(xyz: Coords) -> Coords {
    let [x, y, z] = [0, 1, 2].map(|index| {
        let scaled = xyz[index] / D50_WHITE[index];
        if scaled > EPSILON {
            scaled.cbrt()
        } else {
            (KAPPA * scaled + 16.0) / 116.0
        }
    });
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}

fn lab_to_xyz([lightness, a, b]: Coords) -> Coords {
    let y = (lightness + 16.0) / 116.0;
    let x = a / 500.0 + y;
    let z = y - b / 200.0;
    [
        (if x.powf(3.0) > EPSILON {
            x.powf(3.0)
        } else {
            (116.0 * x - 16.0) / KAPPA
        }) * D50_WHITE[0],
        (if lightness > KAPPA * EPSILON {
            y.powf(3.0)
        } else {
            lightness / KAPPA
        }) * D50_WHITE[1],
        (if z.powf(3.0) > EPSILON {
            z.powf(3.0)
        } else {
            (116.0 * z - 16.0) / KAPPA
        }) * D50_WHITE[2],
    ]
}

fn to_polar(threshold: f64, [lightness, a, b]: Coords) -> Coords {
    let chroma = hypot(&[a, b]);
    let hue = if chroma < threshold {
        f64::NAN
    } else {
        normalize_hue((b.atan2(a) * 180.0) / std::f64::consts::PI)
    };
    [lightness, chroma, hue]
}

fn from_polar([lightness, chroma, hue]: Coords) -> Coords {
    let angle = ((if hue.is_nan() { 0.0 } else { hue }) * std::f64::consts::PI) / 180.0;
    [lightness, chroma * angle.cos(), chroma * angle.sin()]
}

// Each space but `xyz-d65` converts to and from a base space, so any two
// spaces meet on the way to `xyz-d65`.
fn base(space: ColorSpace) -> Option<ColorSpace> {
    match space {
        ColorSpace::XyzD65 => None,
        ColorSpace::XyzD50 => Some(ColorSpace::XyzD65),
        ColorSpace::SrgbLinear => Some(ColorSpace::XyzD65),
        ColorSpace::Srgb => Some(ColorSpace::SrgbLinear),
        ColorSpace::DisplayP3 => Some(ColorSpace::XyzD65),
        ColorSpace::Hsl => Some(ColorSpace::Srgb),
        ColorSpace::Hsv => Some(ColorSpace::Srgb),
        ColorSpace::Hwb => Some(ColorSpace::Hsv),
        ColorSpace::Lab => Some(ColorSpace::XyzD50),
        ColorSpace::Lch => Some(ColorSpace::Lab),
        ColorSpace::Oklab => Some(ColorSpace::XyzD65),
        ColorSpace::Oklch => Some(ColorSpace::Oklab),
    }
}

fn to_base(space: ColorSpace, coords: Coords) -> Coords {
    match space {
        ColorSpace::XyzD65 => coords,
        ColorSpace::XyzD50 => multiply(&D50_TO_D65, coords),
        ColorSpace::SrgbLinear => multiply(&LINEAR_SRGB_TO_XYZ, coords),
        ColorSpace::Srgb => coords.map(to_linear),
        ColorSpace::DisplayP3 => multiply(&LINEAR_P3_TO_XYZ, coords.map(to_linear)),
        ColorSpace::Hsl => hsl_to_srgb(coords),
        ColorSpace::Hsv => hsv_to_srgb(coords),
        ColorSpace::Hwb => hwb_to_hsv(coords),
        ColorSpace::Lab => lab_to_xyz(coords),
        ColorSpace::Lch => from_polar(coords),
        ColorSpace::Oklab => multiply(
            &LMS_TO_XYZ,
            multiply(&OKLAB_TO_LMS, coords).map(|value| value.powf(3.0)),
        ),
        ColorSpace::Oklch => from_polar(coords),
    }
}

fn from_base(space: ColorSpace, coords: Coords) -> Coords {
    match space {
        ColorSpace::XyzD65 => coords,
        ColorSpace::XyzD50 => multiply(&D65_TO_D50, coords),
        ColorSpace::SrgbLinear => multiply(&XYZ_TO_LINEAR_SRGB, coords),
        ColorSpace::Srgb => coords.map(from_linear),
        ColorSpace::DisplayP3 => multiply(&XYZ_TO_LINEAR_P3, coords).map(from_linear),
        ColorSpace::Hsl => srgb_to_hsl(coords),
        ColorSpace::Hsv => srgb_to_hsv(coords),
        ColorSpace::Hwb => hsv_to_hwb(coords),
        ColorSpace::Lab => xyz_to_lab(coords),
        ColorSpace::Lch => to_polar(ACHROMATIC_LCH, coords),
        ColorSpace::Oklab => multiply(&LMS_TO_OKLAB, multiply(&XYZ_TO_LMS, coords).map(f64::cbrt)),
        ColorSpace::Oklch => to_polar(ACHROMATIC_OKLCH, coords),
    }
}

// The space and the bases it passes through to `xyz-d65`, which every space
// reaches within five steps.
struct Lineage {
    steps: [ColorSpace; 5],
    len: usize,
}

impl Lineage {
    fn of(space: ColorSpace) -> Self {
        let mut steps = [space; 5];
        let mut len = 1;
        while let Some(next) = base(steps[len - 1]) {
            steps[len] = next;
            len += 1;
        }
        Lineage { steps, len }
    }

    fn steps(&self) -> &[ColorSpace] {
        &self.steps[..self.len]
    }

    fn index_of(&self, space: ColorSpace) -> usize {
        self.steps()
            .iter()
            .position(|step| *step == space)
            .unwrap_or(self.len)
    }
}

/// The same color in another space. A missing channel counts as 0, and a hue
/// that doesn't show, like a grey's, comes back missing.
pub fn convert_color(color: &Color, space: ColorSpace) -> Color {
    if color.space == space {
        return *color;
    }
    let from = Lineage::of(color.space);
    let to = Lineage::of(space);
    // `xyz-d65` ends both, so they always meet.
    let meeting = from
        .steps()
        .iter()
        .copied()
        .find(|step| to.steps().contains(step))
        .unwrap_or(ColorSpace::XyzD65);
    let mut coords = color.coords.map(|value| value.unwrap_or(0.0));
    for step in &from.steps()[..from.index_of(meeting)] {
        coords = to_base(*step, coords);
    }
    for step in to.steps()[..to.index_of(meeting)].iter().rev() {
        coords = from_base(*step, coords);
    }
    Color {
        space,
        coords: coords.map(|value| if value.is_nan() { None } else { Some(value) }),
        alpha: color.alpha,
    }
}

// What JavaScript's `Math` does where Rust's `f64` methods differ.

/// `Math.sign`: `NaN`, `0` and `-0` keep their value.
pub(crate) fn sign(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        value
    }
}

/// `Math.max` of two values: `NaN` if either is, and `0` over `-0`.
pub(crate) fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == b {
        if a.is_sign_positive() { a } else { b }
    } else if a > b {
        a
    } else {
        b
    }
}

/// `Math.min` of two values: `NaN` if either is, and `-0` over `0`.
pub(crate) fn js_min(a: f64, b: f64) -> f64 {
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

/// `Math.max(...values)`.
pub(crate) fn max_of(values: &[f64]) -> f64 {
    values.iter().fold(f64::NEG_INFINITY, |max, value| js_max(max, *value))
}

/// `Math.min(...values)`.
pub(crate) fn min_of(values: &[f64]) -> f64 {
    values.iter().fold(f64::INFINITY, |min, value| js_min(min, *value))
}

/// `Math.hypot(...values)`, as V8 computes it: each value scaled by the
/// largest, squared and summed with Kahan's compensation.
pub(crate) fn hypot(values: &[f64]) -> f64 {
    let mut max = 0.0_f64;
    let mut one_is_nan = false;
    for value in values {
        if value.is_nan() {
            one_is_nan = true;
        } else if value.abs() > max {
            max = value.abs();
        }
    }
    if max == f64::INFINITY {
        return f64::INFINITY;
    }
    if one_is_nan {
        return f64::NAN;
    }
    if max == 0.0 {
        return 0.0;
    }
    let mut sum = 0.0_f64;
    let mut compensation = 0.0_f64;
    for value in values {
        let n = value.abs() / max;
        let summand = n * n - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    sum.sqrt() * max
}
