//! Port of `foundation/number`: steps, scales, formatting and reading numbers
//! in someone's language, and one keyboard map. Slider, Number Input, Progress
//! and Meter are built on it, and so can any numeric control of your own.
//!
//! The original formats and reads numbers through `Intl.NumberFormat`. gpui
//! has no `Intl`, so [`NumberFormatter`] is a port of it on ICU4X's compiled
//! CLDR data: every `Intl.NumberFormat` option, with the same rounding, signs,
//! grouping, digits, percent, currency, unit and compact patterns, and the
//! same parts.
//!
//! Where ICU4X has no data, the formatter does what most locales do:
//! - The exponent symbol is "E", the range separator "–" and NaN "NaN".
//! - Units ICU4X has no names for (digital units, celsius, fahrenheit,
//!   degree and percent) read as CLDR's English writes them, in every locale.
//! - A compound unit like `kilometer-per-hour` is its two units through the
//!   locale's "per" pattern, so it can read "km/hr" where a browser has "km/h".
//!
//! Options `Intl.NumberFormat` throws on, like a currency style without a
//! currency or a minimum above the maximum, fall back to what's closest and
//! warn in debug builds instead.
//!
//! Dropped, with why:
//! - Server rendering: there is always a window, and nothing here reads one.
//! - `localeMatcher`: ICU4X falls back along CLDR's parent locales, which is
//!   what `"best fit"` does in a browser.

extern crate alloc;

pub mod format;
pub mod keys;
pub mod number;
pub mod parse;
pub mod scale;
pub mod steps;

pub use format::{
    CompactDisplay, CurrencyDisplay, CurrencySign, NumberFormat, NumberFormatOptions,
    NumberFormatPart, NumberFormatter, NumberPartType, NumberStyle, Notation,
    ResolvedNumberFormatOptions, RoundingMode, RoundingPriority, SignDisplay, TrailingZeroDisplay,
    UnitDisplay, UseGrouping, display_options, format_number, format_number_range,
    format_number_to_parts, number_formatter,
};
pub use keys::{Arrows, Dir, NumberKeyOptions, step_from_key};
pub use parse::{is_partial_number, parse_number, parse_range};
pub use scale::{NumberScale, fraction_of, value_at};
pub use steps::{
    NumberBound, NumberMove, NumberRange, NumberScrub, NumberSnap, ScrubEase, ScrubOptions,
    StepDirection, StepRange, StepSize, clamp, decimals_of, max_on_grid, on_grid, on_grid_with,
    round_to, scrub_to, snap_to_step, step_by, step_on_grid, step_value, wrap,
};
