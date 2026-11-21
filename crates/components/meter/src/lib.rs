//! Port of `components/meter`: a measurement within a known range.
//!
//! `meter.ts` → [`meter`]. The parts are [`Meter::root`], [`Meter::track`],
//! [`Meter::indicator`], [`Meter::label`] and [`Meter::value`]. Pass a new
//! `value` with `set_options` when the reading changes.
//!
//! In gpui a div's plain text children name nothing, so the Label part takes
//! the role accesskit gives text labels, which lets the root's labelled-by
//! find it, and its text is the app's `aria_label` on it, or accessible
//! text. The Value part needs no hiding: its plain text isn't read on its
//! own, as a meter's presentational content isn't in the original.
//!
//! # Dropped, and why
//!
//! - The root's inline style `--meter-value`: gpui has no CSS variables, and
//!   [`MeterState::percent`] carries it.
//! - `bindPageLocale` and the `pageLocale` state: the gauge formats in the
//!   app's language as parts render.
//! - `gaugeApi`'s count of Label parts and the root's `bound` flag: the
//!   kernel's part registry knows which parts are in the page, which
//!   `ui_sdk_gauge::labelled_by` asks.

pub mod meter;

pub use meter::{Meter, MeterLabelDetails, MeterOptions, MeterState, MeterValueState};
pub use ui_sdk_gauge::NumberFormat;
