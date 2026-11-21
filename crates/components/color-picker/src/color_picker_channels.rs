//! Port of `components/color-picker/color-picker-channels.ts`: the channels a
//! color picker edits in each space, their ranges in the units people read
//! them in, and the gradients that draw them.

use gpui::{
    Hsla, IntoElement, ParentElement, Styled, div, hsla, linear_color_stop, linear_gradient, relative,
};
use ui_sdk_color::{
    Color, ColorFormat, ColorFormatOptions, ColorGamut, ColorSpace, convert_color, format_color, is_in_gamut,
    parse_color, to_gamut,
};
use ui_sdk_number::round_to;

/// The spaces a color picker edits in: HSV (also called HSB), HSL, sRGB's
/// red, green and blue, and OKLCH (`ColorPickerSpace`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorPickerSpace {
    #[default]
    Hsv,
    Hsl,
    Srgb,
    Oklch,
}

impl ColorPickerSpace {
    /// The original's name for it, like `"oklch"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorPickerSpace::Hsv => "hsv",
            ColorPickerSpace::Hsl => "hsl",
            ColorPickerSpace::Srgb => "srgb",
            ColorPickerSpace::Oklch => "oklch",
        }
    }

    /// The color space its colors are held in.
    pub fn color_space(self) -> ColorSpace {
        match self {
            ColorPickerSpace::Hsv => ColorSpace::Hsv,
            ColorPickerSpace::Hsl => ColorSpace::Hsl,
            ColorPickerSpace::Srgb => ColorSpace::Srgb,
            ColorPickerSpace::Oklch => ColorSpace::Oklch,
        }
    }

    /// Its channels, in order, with alpha last.
    pub fn channels(self) -> [ColorChannel; 4] {
        use ColorChannel::*;
        match self {
            ColorPickerSpace::Hsv => [Hue, Saturation, Brightness, Alpha],
            ColorPickerSpace::Hsl => [Hue, Saturation, Lightness, Alpha],
            ColorPickerSpace::Srgb => [Red, Green, Blue, Alpha],
            ColorPickerSpace::Oklch => [Lightness, Chroma, Hue, Alpha],
        }
    }
}

/// A channel a color picker part edits. Which ones there are depends on the
/// space: `Brightness` is HSV's, `Chroma` OKLCH's, `Lightness` HSL's or
/// OKLCH's. `Alpha` is in every space (`ColorChannel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorChannel {
    Hue,
    Saturation,
    Brightness,
    Lightness,
    Chroma,
    Red,
    Green,
    Blue,
    Alpha,
}

impl ColorChannel {
    /// The original's name for it, like `"hue"`, its `data-channel`.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorChannel::Hue => "hue",
            ColorChannel::Saturation => "saturation",
            ColorChannel::Brightness => "brightness",
            ColorChannel::Lightness => "lightness",
            ColorChannel::Chroma => "chroma",
            ColorChannel::Red => "red",
            ColorChannel::Green => "green",
            ColorChannel::Blue => "blue",
            ColorChannel::Alpha => "alpha",
        }
    }

    /// The channel a name names, like `"hue"`.
    pub fn from_name(name: &str) -> Option<Self> {
        use ColorChannel::*;
        [
            Hue, Saturation, Brightness, Lightness, Chroma, Red, Green, Blue, Alpha,
        ]
        .into_iter()
        .find(|channel| channel.as_str() == name)
    }

    /// What screen readers call it, in English (`labels`).
    pub fn label(self) -> &'static str {
        match self {
            ColorChannel::Hue => "Hue",
            ColorChannel::Saturation => "Saturation",
            ColorChannel::Brightness => "Brightness",
            ColorChannel::Lightness => "Lightness",
            ColorChannel::Chroma => "Chroma",
            ColorChannel::Red => "Red",
            ColorChannel::Green => "Green",
            ColorChannel::Blue => "Blue",
            ColorChannel::Alpha => "Alpha",
        }
    }
}

/// The unit a channel reads in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChannelUnit {
    Degrees,
    Percent,
    None,
}

impl ChannelUnit {
    /// How it's written after a number: `"°"`, `"%"` or nothing.
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelUnit::Degrees => "°",
            ChannelUnit::Percent => "%",
            ChannelUnit::None => "",
        }
    }
}

/// A channel of a space: which coordinate it is, its range and step in the
/// units people read it in, and the scale that turns a coordinate into
/// them, like 255 for red or 100 for OKLCH's lightness (`ChannelSpec`). Index
/// 3 is alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelSpec {
    pub index: usize,
    pub max: f64,
    pub step: f64,
    pub scale: f64,
    pub unit: ChannelUnit,
}

const HUE: ChannelSpec = ChannelSpec {
    index: 0,
    max: 360.0,
    step: 1.0,
    scale: 1.0,
    unit: ChannelUnit::Degrees,
};

const fn percent(index: usize) -> ChannelSpec {
    ChannelSpec {
        index,
        max: 100.0,
        step: 1.0,
        scale: 1.0,
        unit: ChannelUnit::Percent,
    }
}

const fn byte(index: usize) -> ChannelSpec {
    ChannelSpec {
        index,
        max: 255.0,
        step: 1.0,
        scale: 255.0,
        unit: ChannelUnit::None,
    }
}

const ALPHA: ChannelSpec = ChannelSpec {
    index: 3,
    max: 100.0,
    step: 1.0,
    scale: 100.0,
    unit: ChannelUnit::Percent,
};

// A space's channel, if it has it (`channels`).
fn spec_of(space: ColorPickerSpace, channel: ColorChannel) -> Option<ChannelSpec> {
    use ColorChannel::*;
    Some(match (space, channel) {
        (_, Alpha) => ALPHA,
        (ColorPickerSpace::Hsv, Hue) | (ColorPickerSpace::Hsl, Hue) => HUE,
        (ColorPickerSpace::Hsv, Saturation) | (ColorPickerSpace::Hsl, Saturation) => percent(1),
        (ColorPickerSpace::Hsv, Brightness) | (ColorPickerSpace::Hsl, Lightness) => percent(2),
        (ColorPickerSpace::Srgb, Red) => byte(0),
        (ColorPickerSpace::Srgb, Green) => byte(1),
        (ColorPickerSpace::Srgb, Blue) => byte(2),
        (ColorPickerSpace::Oklch, Lightness) => ChannelSpec {
            index: 0,
            max: 100.0,
            step: 0.1,
            scale: 100.0,
            unit: ChannelUnit::Percent,
        },
        (ColorPickerSpace::Oklch, Chroma) => ChannelSpec {
            index: 1,
            max: 0.4,
            step: 0.001,
            scale: 1.0,
            unit: ChannelUnit::None,
        },
        (ColorPickerSpace::Oklch, Hue) => ChannelSpec { index: 2, ..HUE },
        _ => return None,
    })
}

/// The area's axes and the slider's channel when a part names none
/// (`defaultAxes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DefaultAxes {
    pub x: ColorChannel,
    pub y: ColorChannel,
    pub slider: ColorChannel,
}

pub fn default_axes(space: ColorPickerSpace) -> DefaultAxes {
    use ColorChannel::*;
    match space {
        ColorPickerSpace::Hsv => DefaultAxes {
            x: Saturation,
            y: Brightness,
            slider: Hue,
        },
        ColorPickerSpace::Hsl => DefaultAxes {
            x: Saturation,
            y: Lightness,
            slider: Hue,
        },
        ColorPickerSpace::Srgb => DefaultAxes {
            x: Red,
            y: Green,
            slider: Blue,
        },
        ColorPickerSpace::Oklch => DefaultAxes {
            x: Chroma,
            y: Lightness,
            slider: Hue,
        },
    }
}

/// A channel of a space, with its name and label (`Channel`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Channel {
    pub name: ColorChannel,
    pub label: &'static str,
    pub spec: ChannelSpec,
}

impl std::ops::Deref for Channel {
    type Target = ChannelSpec;

    fn deref(&self) -> &ChannelSpec {
        &self.spec
    }
}

/// A space's channel. A channel the space doesn't have reads as its
/// slider's, so a wrong one never fails (`channelOf`).
pub fn channel_of(space: ColorPickerSpace, channel: Option<ColorChannel>) -> Channel {
    let name = channel
        .filter(|channel| spec_of(space, *channel).is_some())
        .unwrap_or(default_axes(space).slider);
    let spec = spec_of(space, name).unwrap_or(HUE);
    Channel {
        name,
        label: name.label(),
        spec,
    }
}

/// Black, in a space (`black`).
pub fn black(space: ColorPickerSpace) -> Color {
    convert_color(
        &Color::new(ColorSpace::Srgb, [0.0, 0.0, 0.0], 1.0),
        space.color_space(),
    )
}

/// A color in a space. Spaces other than OKLCH hold only sRGB colors, so a
/// wider one fits sRGB first (`inSpace`).
pub fn in_space(color: &Color, space: ColorPickerSpace) -> Color {
    let color = if space == ColorPickerSpace::Oklch {
        *color
    } else {
        to_gamut(color, ColorGamut::Srgb)
    };
    convert_color(&color, space.color_space())
}

/// Any CSS color, read into a space, or `None` for text that isn't one
/// (`colorIn`).
pub fn color_in(text: &str, space: ColorPickerSpace) -> Option<Color> {
    parse_color(text).map(|parsed| in_space(&parsed, space))
}

/// A channel's value in the units people read it in (`readChannel`).
pub fn read_channel(color: &Color, spec: &ChannelSpec) -> f64 {
    let coord = if spec.index == 3 {
        Some(color.alpha)
    } else {
        color.coords[spec.index]
    };
    round_to(coord.unwrap_or(0.0) * spec.scale, 4)
}

/// The color with a channel at a value, kept within its range
/// (`withChannel`).
pub fn with_channel(color: &Color, spec: &ChannelSpec, value: f64) -> Color {
    let coord = js_min(spec.max, js_max(0.0, value)) / spec.scale;
    let mut next = *color;
    if spec.index == 3 {
        next.alpha = coord;
    } else {
        next.coords[spec.index] = Some(coord);
    }
    next
}

// How many decimals a step shows: 1 for 0.1, 3 for 0.001.
pub(crate) fn step_decimals(step: f64) -> i32 {
    if step < 1.0 {
        number_text(step).len() as i32 - 2
    } else {
        0
    }
}

/// A channel's value as it's written, like `"212°"` or `"40%"`
/// (`channelText`).
pub fn channel_text(value: f64, spec: &ChannelSpec) -> String {
    let rounded = round_to(value, step_decimals(spec.step));
    format!("{}{}", number_text(rounded), spec.unit.as_str())
}

/// What the picker shows on screen: the color fit into the gamut, unless
/// there is none (`shown`).
pub fn shown(color: &Color, gamut: Option<ColorGamut>) -> Color {
    match gamut {
        Some(gamut) if !is_in_gamut(color, gamut) => to_gamut(color, gamut),
        _ => *color,
    }
}

/// A color as CSS writes it, HSV as `hsl()` (`cssOf`).
pub fn css_of(color: &Color) -> String {
    let format = match color.space {
        ColorSpace::Hsv => ColorFormat::Hsl,
        space => ColorFormat::of_space(space),
    };
    format_color(color, Some(format), ColorFormatOptions::default())
}

/// A track's background (`--color-slider-background`): the color as the
/// channel runs from its start to its end, the rest held, as evenly spaced
/// stops. Hues take more stops, so the gradient follows the wheel
/// (`channelGradient`).
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelGradient {
    /// The colors from the track's start to its end, as gpui paints them.
    pub stops: Vec<Hsla>,
    /// Whether it runs up the track, rather than across it.
    pub vertical: bool,
    /// Whether it runs from the right, for a track across a right to left
    /// layout.
    pub rtl: bool,
}

impl ChannelGradient {
    /// The gradient as an element that fills the one it's in: a layer for
    /// each pair of stops, since gpui's gradients have two. Put it in the
    /// track first, under its thumb.
    pub fn element(&self) -> impl IntoElement + use<> {
        let segments = self.stops.len().saturating_sub(1).max(1);
        let span = 1.0 / segments as f32;
        let layers = self.stops.windows(2).enumerate().map(|(at, pair)| {
            let start = relative(at as f32 * span);
            let (from, to) = (linear_color_stop(pair[0], 0.0), linear_color_stop(pair[1], 1.0));
            let layer = div().absolute();
            if self.vertical {
                layer
                    .left_0()
                    .right_0()
                    .bottom(start)
                    .h(relative(span))
                    .bg(linear_gradient(0.0, from, to))
            } else if self.rtl {
                layer
                    .top_0()
                    .bottom_0()
                    .right(start)
                    .w(relative(span))
                    .bg(linear_gradient(270.0, from, to))
            } else {
                layer
                    .top_0()
                    .bottom_0()
                    .left(start)
                    .w(relative(span))
                    .bg(linear_gradient(90.0, from, to))
            }
        });
        let layers: Vec<_> = layers.collect();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .overflow_hidden()
            .children(layers)
    }
}

/// The gradient a channel's track draws (`channelGradient`).
pub fn channel_gradient(color: &Color, spec: &ChannelSpec, vertical: bool, rtl: bool) -> ChannelGradient {
    let count = if spec.unit == ChannelUnit::Degrees {
        13
    } else if spec.index == 3 {
        2
    } else {
        5
    };
    let stops = (0..count)
        .map(|at| {
            let stop = with_channel(color, spec, (spec.max * at as f64) / (count - 1) as f64);
            let stop = if spec.index == 3 {
                stop
            } else {
                Color { alpha: 1.0, ..stop }
            };
            stop.to_hsla()
        })
        .collect();
    ChannelGradient { stops, vertical, rtl }
}

/// The area's background, where two gradients draw it exactly
/// (`--color-area-background`): saturation by brightness in HSV, and
/// saturation by lightness in HSL. Others take the area's canvas
/// (`areaGradient`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaGradient {
    /// The hue it's drawn in, in degrees, on hundredths.
    pub hue: f64,
    /// HSL's lightness, white along the top, rather than HSV's brightness.
    pub lightness: bool,
    /// Whether saturation runs from the right, right to left.
    pub rtl: bool,
}

impl AreaGradient {
    /// The gradients as an element that fills the one it's in: saturation
    /// across, and over it brightness or lightness up. Put it in the area
    /// first, under its thumb.
    pub fn element(&self) -> impl IntoElement + use<> {
        let hue = (self.hue / 360.0) as f32;
        let across = if self.rtl { 270.0 } else { 90.0 };
        let (start, end) = if self.lightness {
            (hsla(hue, 0.0, 0.5, 1.0), hsla(hue, 1.0, 0.5, 1.0))
        } else {
            (hsla(0.0, 0.0, 1.0, 1.0), hsla(hue, 1.0, 0.5, 1.0))
        };
        let fill = || div().absolute().left_0().right_0();
        let black = hsla(0.0, 0.0, 0.0, 1.0);
        let up = if self.lightness {
            // Black at the bottom, through clear, to white at the top.
            vec![
                fill().bottom_0().h(relative(0.5)).bg(linear_gradient(
                    0.0,
                    linear_color_stop(black, 0.0),
                    linear_color_stop(black.opacity(0.0), 1.0),
                )),
                fill().top_0().h(relative(0.5)).bg(linear_gradient(
                    0.0,
                    linear_color_stop(hsla(0.0, 0.0, 1.0, 0.0), 0.0),
                    linear_color_stop(hsla(0.0, 0.0, 1.0, 1.0), 1.0),
                )),
            ]
        } else {
            vec![fill().top_0().bottom_0().bg(linear_gradient(
                0.0,
                linear_color_stop(black, 0.0),
                linear_color_stop(black.opacity(0.0), 1.0),
            ))]
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .overflow_hidden()
            .bg(linear_gradient(
                across,
                linear_color_stop(start, 0.0),
                linear_color_stop(end, 1.0),
            ))
            .children(up)
    }
}

/// The area's gradient for its axes, or `None` where no gradient draws it.
pub fn area_gradient(color: &Color, x: ColorChannel, y: ColorChannel, rtl: bool) -> Option<AreaGradient> {
    let flat = matches!(color.space, ColorSpace::Hsv | ColorSpace::Hsl);
    if !flat
        || x != ColorChannel::Saturation
        || (y != ColorChannel::Brightness && y != ColorChannel::Lightness)
    {
        return None;
    }
    let hue = (color.coords[0].unwrap_or(0.0) * 100.0).round() / 100.0;
    Some(AreaGradient {
        hue,
        lightness: color.space == ColorSpace::Hsl,
        rtl,
    })
}

const HUE_NAMES: [(f64, &str); 11] = [
    (20.0, "pink"),
    (45.0, "red"),
    (70.0, "orange"),
    (110.0, "yellow"),
    (135.0, "lime"),
    (170.0, "green"),
    (215.0, "cyan"),
    (270.0, "blue"),
    (320.0, "purple"),
    (350.0, "magenta"),
    (360.0, "pink"),
];

/// A name screen readers say for a color, like "dark vivid blue", in
/// English (`describeColor`).
pub fn describe_color(color: &Color) -> String {
    if color.alpha == 0.0 {
        return "transparent".into();
    }
    let [lightness, chroma, angle] = convert_color(color, ColorSpace::Oklch)
        .coords
        .map(|coord| coord.unwrap_or(0.0));
    let opacity = if color.alpha < 1.0 {
        format!(", {}% opaque", js_round(color.alpha * 100.0))
    } else {
        String::new()
    };
    if chroma < 0.02 {
        let gray = if lightness < 0.15 {
            Some("black")
        } else if lightness > 0.97 {
            Some("white")
        } else {
            None
        };
        if let Some(gray) = gray {
            return format!("{gray}{opacity}");
        }
    }
    let tone = if lightness < 0.45 {
        "dark "
    } else if lightness > 0.82 {
        "light "
    } else {
        ""
    };
    let strength = if chroma < 0.02 {
        ""
    } else if chroma < 0.08 {
        "grayish "
    } else if chroma > 0.2 {
        "vivid "
    } else {
        ""
    };
    let name = if chroma < 0.02 {
        "gray"
    } else {
        let angle = angle % 360.0;
        HUE_NAMES
            .iter()
            .find(|(end, _)| angle < *end)
            .map_or("pink", |(_, name)| name)
    };
    format!("{tone}{strength}{name}{opacity}")
}

// `Math.min` and `Math.max`: a NaN anywhere gives NaN.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

// `Math.round`: halves round up, toward positive infinity.
pub(crate) fn js_round(value: f64) -> f64 {
    let up = value.ceil();
    if up - 0.5 > value { up - 1.0 } else { up }
}

/// A number as `String(value)` writes it.
pub(crate) fn number_text(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if value == 0.0 {
        return "0".into();
    }
    if (1e-6..1e21).contains(&value.abs()) {
        return value.to_string();
    }
    let text = format!("{value:e}");
    match text.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
        _ => text,
    }
}
