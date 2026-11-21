//! Port of `foundation/color/format.ts`: writing a color as CSS.

use std::fmt;
use std::str::FromStr;

use crate::convert::{Color, ColorSpace, convert_color, js_max, js_min};

/// What [`format_color`] writes: [`ColorFormat::Hex`], or a space in its CSS
/// syntax. [`ColorFormat::Srgb`] writes `rgb()`, and the spaces without a
/// function of their own write `color()`. HSV has no CSS syntax, so it isn't
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorFormat {
    Hex,
    Srgb,
    SrgbLinear,
    DisplayP3,
    Hsl,
    Hwb,
    Lab,
    Lch,
    Oklab,
    Oklch,
    XyzD65,
    XyzD50,
}

impl ColorFormat {
    /// Every format, hex first.
    pub const ALL: [ColorFormat; 12] = [
        ColorFormat::Hex,
        ColorFormat::Srgb,
        ColorFormat::SrgbLinear,
        ColorFormat::DisplayP3,
        ColorFormat::Hsl,
        ColorFormat::Hwb,
        ColorFormat::Lab,
        ColorFormat::Lch,
        ColorFormat::Oklab,
        ColorFormat::Oklch,
        ColorFormat::XyzD65,
        ColorFormat::XyzD50,
    ];

    /// The format's name, as the original writes it: `"hex"`, `"srgb"`,
    /// `"oklch"` and so on.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorFormat::Hex => "hex",
            ColorFormat::Srgb => "srgb",
            ColorFormat::SrgbLinear => "srgb-linear",
            ColorFormat::DisplayP3 => "display-p3",
            ColorFormat::Hsl => "hsl",
            ColorFormat::Hwb => "hwb",
            ColorFormat::Lab => "lab",
            ColorFormat::Lch => "lch",
            ColorFormat::Oklab => "oklab",
            ColorFormat::Oklch => "oklch",
            ColorFormat::XyzD65 => "xyz-d65",
            ColorFormat::XyzD50 => "xyz-d50",
        }
    }

    /// The space the format writes: `srgb` for hex.
    pub fn space(self) -> ColorSpace {
        match self {
            ColorFormat::Hex | ColorFormat::Srgb => ColorSpace::Srgb,
            ColorFormat::SrgbLinear => ColorSpace::SrgbLinear,
            ColorFormat::DisplayP3 => ColorSpace::DisplayP3,
            ColorFormat::Hsl => ColorSpace::Hsl,
            ColorFormat::Hwb => ColorSpace::Hwb,
            ColorFormat::Lab => ColorSpace::Lab,
            ColorFormat::Lch => ColorSpace::Lch,
            ColorFormat::Oklab => ColorSpace::Oklab,
            ColorFormat::Oklch => ColorSpace::Oklch,
            ColorFormat::XyzD65 => ColorSpace::XyzD65,
            ColorFormat::XyzD50 => ColorSpace::XyzD50,
        }
    }

    /// The format a space writes in by default: its own, and `hsl()` for HSV.
    pub fn of_space(space: ColorSpace) -> Self {
        match space {
            ColorSpace::Srgb => ColorFormat::Srgb,
            ColorSpace::SrgbLinear => ColorFormat::SrgbLinear,
            ColorSpace::DisplayP3 => ColorFormat::DisplayP3,
            ColorSpace::Hsl | ColorSpace::Hsv => ColorFormat::Hsl,
            ColorSpace::Hwb => ColorFormat::Hwb,
            ColorSpace::Lab => ColorFormat::Lab,
            ColorSpace::Lch => ColorFormat::Lch,
            ColorSpace::Oklab => ColorFormat::Oklab,
            ColorSpace::Oklch => ColorFormat::Oklch,
            ColorSpace::XyzD65 => ColorFormat::XyzD65,
            ColorSpace::XyzD50 => ColorFormat::XyzD50,
        }
    }
}

impl fmt::Display for ColorFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The text isn't the name of a color format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownColorFormat(pub String);

impl fmt::Display for UnknownColorFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\" is not a color format", self.0)
    }
}

impl std::error::Error for UnknownColorFormat {}

impl FromStr for ColorFormat {
    type Err = UnknownColorFormat;

    /// Reads a format's name, like `"hex"` or `"oklch"`.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        ColorFormat::ALL
            .into_iter()
            .find(|format| format.as_str() == name)
            .ok_or_else(|| UnknownColorFormat(name.to_string()))
    }
}

/// Options for [`format_color`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorFormatOptions {
    /// Significant digits per channel, counted against the channel's range: 5
    /// writes `rgb(212.41 0 52.94)` and `oklch(0.6276 0.2577 29.23)`. 5 by
    /// default.
    pub precision: u32,
}

impl Default for ColorFormatOptions {
    fn default() -> Self {
        Self { precision: 5 }
    }
}

impl ColorFormatOptions {
    pub fn precision(mut self, precision: u32) -> Self {
        self.precision = precision;
        self
    }
}

struct Syntax {
    name: &'static str,
    scale: f64,
    ranges: [f64; 3],
    percent: bool,
}

const fn syntax(name: &'static str, scale: f64, ranges: [f64; 3], percent: bool) -> Syntax {
    Syntax {
        name,
        scale,
        ranges,
        percent,
    }
}

// The syntax of each format but hex.
fn syntax_of(format: ColorFormat) -> Syntax {
    match format {
        ColorFormat::Hex | ColorFormat::Srgb => syntax("rgb", 255.0, [255.0, 255.0, 255.0], false),
        ColorFormat::Hsl => syntax("hsl", 1.0, [360.0, 100.0, 100.0], true),
        ColorFormat::Hwb => syntax("hwb", 1.0, [360.0, 100.0, 100.0], true),
        ColorFormat::Lab => syntax("lab", 1.0, [100.0, 125.0, 125.0], false),
        ColorFormat::Lch => syntax("lch", 1.0, [100.0, 150.0, 360.0], false),
        ColorFormat::Oklab => syntax("oklab", 1.0, [1.0, 0.4, 0.4], false),
        ColorFormat::Oklch => syntax("oklch", 1.0, [1.0, 0.4, 360.0], false),
        ColorFormat::SrgbLinear => syntax("color(srgb-linear", 1.0, [1.0, 1.0, 1.0], false),
        ColorFormat::DisplayP3 => syntax("color(display-p3", 1.0, [1.0, 1.0, 1.0], false),
        ColorFormat::XyzD65 => syntax("color(xyz-d65", 1.0, [1.0, 1.0, 1.0], false),
        ColorFormat::XyzD50 => syntax("color(xyz-d50", 1.0, [1.0, 1.0, 1.0], false),
    }
}

// The formats CSS clamps to sRGB, where a color from a wider space is clipped.
fn is_srgb_bound(format: ColorFormat) -> bool {
    matches!(
        format,
        ColorFormat::Hex | ColorFormat::Srgb | ColorFormat::Hsl | ColorFormat::Hwb
    )
}

fn clamp(value: f64, max: f64) -> f64 {
    js_min(max, js_max(0.0, value))
}

fn write_number(value: f64, range: f64, precision: u32) -> String {
    let digits = if range < 1.0 {
        1
    } else {
        range.log10().floor() as i64 + 1
    };
    let text = to_fixed(value, (i64::from(precision) - digits).max(0) as usize);
    let trimmed = if text.contains('.') {
        // `text.replace(/\.?0+$/, "")`
        let without_zeros = text.trim_end_matches('0');
        if without_zeros.len() < text.len() {
            without_zeros.strip_suffix('.').unwrap_or(without_zeros).to_string()
        } else {
            text
        }
    } else {
        text
    };
    if trimmed == "-0" {
        "0".to_string()
    } else {
        trimmed
    }
}

fn clamp_channel(value: Option<f64>, max: f64) -> Option<f64> {
    value.map(|value| clamp(value, max))
}

// The color in the format's space, clipped to the range that format can hold.
// A hue that only went missing in the conversion writes 0, as browsers do.
fn fit(color: &Color, format: ColorFormat) -> Color {
    let space = format.space();
    let bound = is_srgb_bound(format);
    if color.space == space {
        if !bound {
            return *color;
        }
        let [first, second, third] = color.coords;
        let max = if space == ColorSpace::Srgb { 1.0 } else { 100.0 };
        return Color {
            coords: [
                if space == ColorSpace::Srgb {
                    clamp_channel(first, 1.0)
                } else {
                    first
                },
                clamp_channel(second, max),
                clamp_channel(third, max),
            ],
            ..*color
        };
    }
    let mut source = *color;
    if bound {
        let srgb = convert_color(color, ColorSpace::Srgb);
        source = Color {
            coords: srgb.coords.map(|value| Some(clamp(value.unwrap_or(0.0), 1.0))),
            ..srgb
        };
    }
    let converted = convert_color(&source, space);
    Color {
        coords: converted.coords.map(|value| Some(value.unwrap_or(0.0))),
        ..converted
    }
}

fn write_hex(color: &Color) -> String {
    let mut bytes: Vec<f64> = color.coords.iter().map(|value| value.unwrap_or(0.0)).collect();
    if color.alpha < 1.0 {
        bytes.push(color.alpha);
    }
    let mut hex = String::from("#");
    for value in bytes {
        let byte = to_hex_string(js_round(value * 255.0));
        if byte.chars().count() < 2 {
            hex.push('0');
        }
        hex.push_str(&byte);
    }
    hex
}

/// Writes a color as CSS: in `format`, or in its own space when `None` (HSV
/// writes `hsl()`). `hex`, `rgb()`, `hsl()` and `hwb()` hold only sRGB colors,
/// so a wider color is clipped to fit: map it with `to_gamut` first to keep its
/// lightness and hue. A missing channel writes `none`.
pub fn format_color(color: &Color, format: Option<ColorFormat>, options: ColorFormatOptions) -> String {
    let target = format.unwrap_or_else(|| ColorFormat::of_space(color.space));
    let precision = options.precision;
    let fitted = fit(color, target);
    if target == ColorFormat::Hex {
        return write_hex(&fitted);
    }
    let Syntax {
        name,
        scale,
        ranges,
        percent,
    } = syntax_of(target);
    let channels: Vec<String> = fitted
        .coords
        .iter()
        .enumerate()
        .map(|(index, value)| match value {
            None => "none".to_string(),
            Some(value) => {
                let text = write_number(value * scale, ranges[index], precision);
                if percent && index > 0 {
                    format!("{text}%")
                } else {
                    text
                }
            }
        })
        .collect();
    let alpha = if fitted.alpha < 1.0 {
        format!(" / {}", write_number(fitted.alpha, 1.0, precision))
    } else {
        String::new()
    };
    let open = if name.starts_with("color(") { " " } else { "(" };
    format!("{name}{open}{}{alpha})", channels.join(" "))
}

// What JavaScript's numbers do where Rust's formatting differs.

/// `Math.round`: halves round up, toward positive infinity.
pub(crate) fn js_round(value: f64) -> f64 {
    let up = value.ceil();
    if up - 0.5 > value { up - 1.0 } else { up }
}

/// `Number.prototype.toFixed`. Rust rounds an exact tie to even, where
/// JavaScript picks the larger magnitude, so ties are found and rounded here.
pub(crate) fn to_fixed(value: f64, digits: usize) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    // `-0` writes as `0`, as in JavaScript.
    let negative = value < 0.0;
    let magnitude = value.abs();
    if magnitude >= 1e21 {
        return js_to_string(value);
    }
    let text = if is_tie(magnitude, digits) {
        round_tie_up(magnitude, digits)
    } else {
        format!("{magnitude:.digits$}")
    };
    if negative { format!("-{text}") } else { text }
}

// Whether `magnitude` (finite, not negative) lies exactly halfway between two
// numbers with `digits` decimals. `magnitude = mantissa × 2^exponent`, so
// `magnitude × 10^digits = mantissa × 5^digits × 2^(exponent + digits)`, whose
// fraction is exactly ½ when the power of two left after the mantissa's
// trailing zeros is 2⁻¹.
fn is_tie(magnitude: f64, digits: usize) -> bool {
    let bits = magnitude.to_bits();
    let exponent_bits = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1_u64 << 52) - 1);
    let (mantissa, exponent) = if exponent_bits == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1_u64 << 52), exponent_bits - 1075)
    };
    if mantissa == 0 {
        return false;
    }
    i64::from(mantissa.trailing_zeros()) + exponent + digits as i64 == -1
}

// A tie written with one more decimal is exact and ends in 5: drop the 5 and
// add one in the last place.
fn round_tie_up(magnitude: f64, digits: usize) -> String {
    let exact = format!("{magnitude:.precision$}", precision = digits + 1);
    let mut kept: Vec<u8> = exact.as_bytes()[..exact.len() - 1].to_vec();
    if kept.last() == Some(&b'.') {
        kept.pop();
    }
    let mut carry = true;
    for byte in kept.iter_mut().rev() {
        if !carry {
            break;
        }
        match byte {
            b'.' => {}
            b'9' => *byte = b'0',
            _ => {
                *byte += 1;
                carry = false;
            }
        }
    }
    if carry {
        kept.insert(0, b'1');
    }
    String::from_utf8(kept).unwrap_or(exact)
}

// `Number.prototype.toString` for numbers `toFixed` won't write in full.
fn js_to_string(value: f64) -> String {
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let text = format!("{value:e}");
    match text.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
        _ => text,
    }
}

// `Number.prototype.toString(16)` of a whole number.
fn to_hex_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let magnitude = value.abs() as u128;
    if value < 0.0 && magnitude != 0 {
        format!("-{magnitude:x}")
    } else {
        format!("{magnitude:x}")
    }
}
