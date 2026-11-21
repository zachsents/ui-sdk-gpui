//! Port of `components/waterfall/waterfall-time.ts`: where spans fall on a
//! waterfall's time axis, the axis's ticks, and how times read. Pure, so it
//! needs no window.

use ui_sdk_number::{NumberFormat, NumberFormatOptions, NumberStyle, UnitDisplay, clamp, format_number};

/// The stretch of time an axis shows, `[start, end]` in milliseconds.
pub type Range = [f64; 2];

/// What the axis reads of a span: when it starts, how long it lasts, and the
/// spans it started (`Span`).
pub trait Span: Sized {
    /// When it starts, in milliseconds.
    fn start(&self) -> f64;
    /// How long it lasts, in milliseconds.
    fn duration(&self) -> f64;
    /// The spans it started.
    fn children(&self) -> &[Self];
}

/// One tick of the axis: its time, the text it shows, and where it sits on
/// the axis, from 0 to 100.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterfallTickData {
    /// The moment it marks, in milliseconds.
    pub time: f64,
    /// The time as the waterfall's `format` and locale show it.
    pub label: String,
    /// Where it sits between the range's start and end, from 0 to 100. Also
    /// `--waterfall-tick` in the original.
    pub position: f64,
}

// `Math.min` and `Math.max`: NaN wins, where Rust's ignore it.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

/// The earliest start and latest end of every span, or `[0, 0]` for none.
pub fn span_range<S: Span>(items: &[S]) -> Range {
    fn visit<S: Span>(list: &[S], start: &mut f64, end: &mut f64) {
        for item in list {
            *start = js_min(*start, item.start());
            *end = js_max(*end, item.start() + js_max(0.0, item.duration()));
            visit(item.children(), start, end);
        }
    }
    let mut start = f64::INFINITY;
    let mut end = f64::NEG_INFINITY;
    visit(items, &mut start, &mut end);
    if start.is_finite() { [start, end] } else { [0.0, 0.0] }
}

// `Math.round`: the nearest integer, halves toward +∞.
fn js_round(value: f64) -> f64 {
    let rounded = value.round();
    if value - rounded == 0.5 { rounded + 1.0 } else { rounded }
}

fn round(percent: f64) -> f64 {
    js_round(percent * 1e4) / 1e4
}

/// Where a moment sits between the range's ends, from 0 to 100, clamped.
pub fn percent_at(time: f64, [start, end]: Range) -> f64 {
    if end > start {
        round((clamp(time, start, end) - start) / (end - start) * 100.0)
    } else {
        0.0
    }
}

/// The part of the axis a span covers, where it starts and how much it
/// covers, both from 0 to 100: zero wide when it falls outside the range.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Placement {
    pub start: f64,
    pub size: f64,
}

/// The part of the axis a span covers (`spanPlacement`).
pub fn span_placement<S: Span>(span: &S, range: Range) -> Placement {
    let start = percent_at(span.start(), range);
    let end = percent_at(span.start() + js_max(0.0, span.duration()), range);
    Placement {
        start,
        size: round(end - start),
    }
}

// `10 ** n`, NaN and infinities included.
fn power_of_ten(exponent: f64) -> f64 {
    10f64.powf(exponent)
}

// The smallest step of 1, 2 or 5 times a power of ten that fits `max` ticks,
// each with a step of room after it.
fn tick_step(span: f64, max: usize) -> f64 {
    let rough = span / (max as f64 + 1.0);
    let power = power_of_ten(rough.log10().floor());
    for factor in [1.0, 2.0, 5.0, 10.0] {
        if factor * power >= rough {
            return factor * power;
        }
    }
    10.0 * power
}

// `Number(value.toFixed(digits))`.
fn to_fixed(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// The times of the ticks: round steps, at most `max` of them, inside the
/// range. A label sits after its tick, in the step before the next, so the
/// last tick needs a step of room before the end too.
pub fn tick_times([start, end]: Range, max: usize) -> Vec<f64> {
    if !(end > start) || max < 1 {
        return vec![start];
    }
    let step = tick_step(end - start, max);
    // Steps like 0.1 add up float error, so each tick is rounded to the
    // step's precision.
    let digits = (-step.log10().floor()).max(0.0) as usize;
    let first = (start / step).ceil() * step;
    let mut times = Vec::new();
    let mut time = first;
    while time <= end - step + step * 1e-9 && times.len() < max {
        times.push(to_fixed(time, digits));
        time += step;
    }
    if times.is_empty() && first <= end {
        vec![first]
    } else {
        times
    }
}

// The decimals a step needs, like 2 for 0.05, so ticks a step apart read
// differently.
fn step_digits(step: f64) -> u8 {
    (-step.log10() - 1e-9).ceil().clamp(0.0, 6.0) as u8
}

/// Milliseconds, seconds or minutes, picked by `scale` (the time itself
/// unless given), so every tick of an axis reads in one unit, with the
/// decimals `step` needs in that unit. A `format` of the waterfall's own
/// decides instead.
pub fn format_time(
    time: f64,
    locale: &str,
    format: Option<&NumberFormat>,
    scale: Option<f64>,
    step: Option<f64>,
) -> String {
    if let Some(format) = format {
        return format_number(Some(time), Some(locale), Some(format));
    }
    let size = scale.unwrap_or(time).abs();
    let (unit, ms, digits) = if size < 1000.0 {
        ("millisecond", 1.0, if time.abs() < 10.0 { 1 } else { 0 })
    } else if size < 60_000.0 {
        ("second", 1000.0, 2)
    } else {
        ("minute", 60_000.0, 1)
    };
    let needed = match step {
        Some(step) if step > 0.0 => step_digits(step / ms),
        _ => 0,
    };
    let options = NumberFormatOptions::default()
        .style(NumberStyle::Unit)
        .unit(unit)
        .unit_display(UnitDisplay::Short)
        .maximum_fraction_digits(digits.max(needed));
    format_number(Some(time / ms), Some(locale), Some(&NumberFormat::Options(options)))
}

/// The axis's ticks for a range: at most `max`, at round times, each with
/// its label and place (`waterfallTicks`).
pub fn waterfall_ticks(range: Range, max: usize, locale: &str, format: Option<&NumberFormat>) -> Vec<WaterfallTickData> {
    let scale = range[0].abs().max(range[1].abs());
    let step = tick_step(range[1] - range[0], max);
    tick_times(range, max)
        .into_iter()
        .map(|time| WaterfallTickData {
            time,
            label: format_time(time, locale, format, Some(scale), Some(step)),
            position: percent_at(time, range),
        })
        .collect()
}
