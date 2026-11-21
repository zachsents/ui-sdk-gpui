//! Port of `components/progress`: a task's progress, or an indeterminate
//! state while how far it has come is unknown.
//!
//! `progress.ts` → [`progress`]. The parts are [`Progress::root`],
//! [`Progress::track`], [`Progress::indicator`], [`Progress::label`] and
//! [`Progress::value`]. Pass the latest `value` with `set_options` as the
//! task goes on, and `None` while how far it has come is unknown.
//!
//! The original's `data-indeterminate`, `data-progressing` and
//! `data-complete` on every part are the `status` in every part's state.
//!
//! In gpui a div's plain text children name nothing, so the Label part takes
//! the role accesskit gives text labels, which lets the root's labelled-by
//! find it, and its text is the app's `aria_label` on it, or accessible
//! text. The Value part needs no hiding: its plain text isn't read on its
//! own, as a progress bar's presentational content isn't in the original.
//!
//! # Dropped, and why
//!
//! - The root's inline style `--progress-value`: gpui has no CSS variables,
//!   and [`ProgressState::percent`] carries it.
//! - `bindPageLocale` and the `pageLocale` state: the gauge formats in the
//!   app's language as parts render.
//! - `gaugeApi`'s count of Label parts and the root's `bound` flag: the
//!   kernel's part registry knows which parts are in the page, which
//!   `ui_sdk_gauge::labelled_by` asks.

pub mod progress;

pub use progress::{
    Progress, ProgressLabelDetails, ProgressOptions, ProgressState, ProgressStatus,
    ProgressValueState,
};
pub use ui_sdk_gauge::NumberFormat;
