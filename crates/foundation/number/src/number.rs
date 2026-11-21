//! Port of `foundation/number/number.ts`, the `@ui-sdk/core/number` entry:
//! steps, scales, formatting and reading numbers in someone's language, and
//! one keyboard map. Slider, Number Input, Progress and Meter are built on it,
//! and so can any numeric control of your own.
//!
//! The crate root exports all of it too, with what the components share
//! besides.

pub use crate::format::{
    NumberFormat, NumberFormatOptions, NumberFormatter, format_number, format_number_range,
    format_number_to_parts, number_formatter,
};
pub use crate::keys::{NumberKeyOptions, step_from_key};
pub use crate::parse::{is_partial_number, parse_number};
pub use crate::scale::{NumberScale, fraction_of, value_at};
pub use crate::steps::{
    NumberMove, NumberRange, NumberScrub, NumberSnap, StepRange, clamp, decimals_of, max_on_grid,
    round_to, scrub_to, step_by, step_value, wrap,
};
