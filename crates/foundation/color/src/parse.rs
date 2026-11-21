//! Port of `foundation/color/parse.ts`: reading a CSS color.
//!
//! The original matches with regular expressions; here small hand-written
//! matchers accept exactly the text they do, with JavaScript's whitespace
//! (`\s` and `trim`) and ASCII digits (`\d`).

use crate::convert::{Color, ColorSpace, js_max, js_min};
use crate::names::NAMED_COLORS;

// What a plain number and 100% mean on a channel, and the range CSS clamps it
// to when it parses.
#[derive(Clone, Copy)]
enum Channel {
    Hue,
    Scale {
        number: f64,
        percent: f64,
        min: Option<f64>,
        max: Option<f64>,
    },
}

const fn scale(number: f64, percent: f64, min: Option<f64>, max: Option<f64>) -> Channel {
    Channel::Scale {
        number,
        percent,
        min,
        max,
    }
}

const RGB: Channel = scale(1.0 / 255.0, 1.0, Some(0.0), Some(1.0));
const PERCENTAGE: Channel = scale(1.0, 100.0, None, None);
const UNIT: Channel = scale(1.0, 1.0, None, None);
const ALPHA: Channel = scale(1.0, 1.0, Some(0.0), Some(1.0));

struct Function {
    space: ColorSpace,
    channels: [Channel; 3],
}

fn function(name: &str) -> Option<Function> {
    let (space, channels) = match name {
        "rgb" => (ColorSpace::Srgb, [RGB, RGB, RGB]),
        "hsl" => (
            ColorSpace::Hsl,
            [Channel::Hue, scale(1.0, 100.0, Some(0.0), None), PERCENTAGE],
        ),
        "hwb" => (ColorSpace::Hwb, [Channel::Hue, PERCENTAGE, PERCENTAGE]),
        "lab" => (
            ColorSpace::Lab,
            [
                scale(1.0, 100.0, Some(0.0), Some(100.0)),
                scale(1.0, 125.0, None, None),
                scale(1.0, 125.0, None, None),
            ],
        ),
        "lch" => (
            ColorSpace::Lch,
            [
                scale(1.0, 100.0, Some(0.0), Some(100.0)),
                scale(1.0, 150.0, Some(0.0), None),
                Channel::Hue,
            ],
        ),
        "oklab" => (
            ColorSpace::Oklab,
            [
                scale(1.0, 1.0, Some(0.0), Some(1.0)),
                scale(1.0, 0.4, None, None),
                scale(1.0, 0.4, None, None),
            ],
        ),
        "oklch" => (
            ColorSpace::Oklch,
            [
                scale(1.0, 1.0, Some(0.0), Some(1.0)),
                scale(1.0, 0.4, Some(0.0), None),
                Channel::Hue,
            ],
        ),
        _ => return None,
    };
    Some(Function { space, channels })
}

fn predefined(name: &str) -> Option<ColorSpace> {
    Some(match name {
        "srgb" => ColorSpace::Srgb,
        "srgb-linear" => ColorSpace::SrgbLinear,
        "display-p3" => ColorSpace::DisplayP3,
        "xyz" => ColorSpace::XyzD65,
        "xyz-d65" => ColorSpace::XyzD65,
        "xyz-d50" => ColorSpace::XyzD50,
        _ => return None,
    })
}

fn angle(suffix: &str) -> Option<f64> {
    Some(match suffix {
        "" | "deg" => 1.0,
        "grad" => 0.9,
        "rad" => 180.0 / std::f64::consts::PI,
        "turn" => 360.0,
        _ => return None,
    })
}

// A number and its unit, as `/^([+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?)(%|deg|grad|rad|turn)?$/`
// reads them.
fn token(text: &str) -> Option<(f64, &str)> {
    let bytes = text.as_bytes();
    let digits_from = |mut at: usize| {
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        at
    };
    let mut at = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        at += 1;
    }
    let integer_end = digits_from(at);
    if integer_end > at {
        at = integer_end;
        if bytes.get(at) == Some(&b'.') {
            at = digits_from(at + 1);
        }
    } else if bytes.get(at) == Some(&b'.') && digits_from(at + 1) > at + 1 {
        at = digits_from(at + 1);
    } else {
        return None;
    }
    if bytes.get(at) == Some(&b'e') {
        let mut exponent = at + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        let exponent_end = digits_from(exponent);
        if exponent_end > exponent {
            at = exponent_end;
        }
    }
    let suffix = &text[at..];
    if !matches!(suffix, "" | "%" | "deg" | "grad" | "rad" | "turn") {
        return None;
    }
    let value = text[..at].parse::<f64>().ok()?;
    Some((value, suffix))
}

// One channel's text as a number, `Some(None)` for `none`, or `None` when the
// channel can't take it, like an angle on a lightness.
fn read_channel(text: &str, channel: Channel) -> Option<Option<f64>> {
    if text == "none" {
        return Some(None);
    }
    let (value, suffix) = token(text)?;
    match channel {
        Channel::Hue => {
            let factor = angle(suffix)?;
            Some(Some((((value * factor) % 360.0) + 360.0) % 360.0))
        }
        Channel::Scale {
            number,
            percent,
            min,
            max,
        } => {
            if !suffix.is_empty() && suffix != "%" {
                return None;
            }
            let scaled = if suffix.is_empty() {
                value * number
            } else {
                (value / 100.0) * percent
            };
            Some(Some(js_min(
                max.unwrap_or(f64::INFINITY),
                js_max(min.unwrap_or(f64::NEG_INFINITY), scaled),
            )))
        }
    }
}

fn read_channels(
    texts: &[&str],
    channels: [Channel; 3],
    alpha_text: Option<&str>,
    space: ColorSpace,
) -> Option<Color> {
    if texts.len() != 3 {
        return None;
    }
    let mut coords = [None; 3];
    for (index, text) in texts.iter().enumerate() {
        coords[index] = read_channel(text, channels[index])?;
    }
    let opacity = match alpha_text {
        None => Some(1.0),
        Some(text) => read_channel(text, ALPHA)?,
    };
    Some(Color {
        space,
        coords,
        alpha: opacity.unwrap_or(0.0),
    })
}

fn parse_hex(digits: &str) -> Color {
    let full: String = if digits.len() < 5 {
        digits.chars().flat_map(|digit| [digit, digit]).collect()
    } else {
        digits.to_string()
    };
    let bytes: Vec<f64> = full
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).unwrap_or("00");
            f64::from(u8::from_str_radix(pair, 16).unwrap_or(0)) / 255.0
        })
        .collect();
    Color {
        space: ColorSpace::Srgb,
        coords: [Some(bytes[0]), Some(bytes[1]), Some(bytes[2])],
        alpha: bytes.get(3).copied().unwrap_or(1.0),
    }
}

fn parse_function(name: &str, body: &str) -> Option<Color> {
    if name == "color" {
        let mut parts = body.split('/').map(js_trim);
        let channels = parts.next().unwrap_or("");
        let alpha_text = parts.next();
        let extra = parts.next();
        let words = split_whitespace(channels);
        let (space_name, texts) = words.split_first()?;
        let space = predefined(space_name)?;
        if extra.is_some() {
            return None;
        }
        return read_channels(texts, [UNIT, UNIT, UNIT], alpha_text, space);
    }
    let legacy_name = if name == "rgba" || name == "hsla" {
        &name[..3]
    } else {
        name
    };
    let definition = function(legacy_name)?;
    if body.contains(',') {
        if definition.space != ColorSpace::Srgb && definition.space != ColorSpace::Hsl {
            return None;
        }
        let parts: Vec<&str> = body.split(',').map(js_trim).collect();
        if parts.len() > 4 || parts.contains(&"none") {
            return None;
        }
        return read_channels(
            &parts[..parts.len().min(3)],
            definition.channels,
            parts.get(3).copied(),
            definition.space,
        );
    }
    let mut parts = body.split('/').map(js_trim);
    let channels = parts.next().unwrap_or("");
    let alpha_text = parts.next();
    if parts.next().is_some() {
        return None;
    }
    read_channels(
        &split_whitespace(channels),
        definition.channels,
        alpha_text,
        definition.space,
    )
}

/// Reads a CSS color: hex, a named color, `transparent`, or `rgb()`, `hsl()`,
/// `hwb()`, `lab()`, `lch()`, `oklab()`, `oklch()` and `color()` in the
/// `srgb`, `srgb-linear`, `display-p3` and `xyz` spaces. The color keeps the
/// space it was written in. Returns `None` for anything else.
pub fn parse_color(text: &str) -> Option<Color> {
    read_color(&js_trim(text).to_lowercase())
}

fn read_color(value: &str) -> Option<Color> {
    if value == "transparent" {
        return Some(Color::new(ColorSpace::Srgb, [0.0, 0.0, 0.0], 0.0));
    }
    if let Some(digits) = value.strip_prefix('#') {
        // `/^#([\da-f]{3,4}|[\da-f]{6}|[\da-f]{8})$/`
        let hex = matches!(digits.len(), 3 | 4 | 6 | 8)
            && digits.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if hex {
            return Some(parse_hex(digits));
        }
    }
    // `/^([a-z]+)\((.*)\)$/`: a name, then its body up to the closing
    // parenthesis that ends the text, on one line.
    let name_end = value
        .bytes()
        .position(|byte| !byte.is_ascii_lowercase())
        .unwrap_or(value.len());
    if name_end > 0
        && value[name_end..].starts_with('(')
        && value.len() > name_end + 1
        && value.ends_with(')')
    {
        let body = &value[name_end + 1..value.len() - 1];
        if !body.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
            return parse_function(&value[..name_end], js_trim(body));
        }
    }
    if name_end != value.len() || value.is_empty() {
        return None;
    }
    let named = NAMED_COLORS.find(&format!(" {value}:"))?;
    let at = named + value.len() + 2;
    Some(parse_hex(&NAMED_COLORS[at..at + 6]))
}

/// The whitespace JavaScript's `trim` removes and `\s` matches: Unicode's
/// `White_Space` but the next line character, and the byte order mark.
pub(crate) fn is_js_space(c: char) -> bool {
    c == '\u{FEFF}' || (c.is_whitespace() && c != '\u{85}')
}

/// `String.prototype.trim`.
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

// `text.split(/\s+/)` of trimmed text: its words, or one empty word.
fn split_whitespace(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![""];
    }
    text.split(is_js_space)
        .filter(|word| !word.is_empty())
        .collect()
}
