//! Port of `foundation/gauge`: what Progress and Meter share, where a value
//! sits between bounds, how it reads, and the ARIA and placement of their
//! parts.
//!
//! Dropped, with why:
//! - Inline styles (`indicatorStyle`'s `style` text, through `css`): gpui
//!   styles elements directly, so [`IndicatorStyle::apply`] places the
//!   indicator.
//! - The page's language in the settings, read once the root binds: parts
//!   format in `ui_sdk_text::locale_of`, the app's language, as they render.
//! - `gaugeApi`'s count of Label parts, which a label's `bind` adds to: the
//!   kernel's part registry knows which parts are in the page, so
//!   [`labelled_by`] asks it.

pub mod gauge;

pub use gauge::{
    GaugeAria, GaugeLabelDetails, GaugeOptions, GaugeSettings, GetValueLabel, IndicatorStyle,
    finite, format_gauge_value, gauge_aria, gauge_settings, indicator_style, label_id,
    labelled_by, percent_between, round_percent,
};
pub use ui_sdk_number::NumberFormat;
