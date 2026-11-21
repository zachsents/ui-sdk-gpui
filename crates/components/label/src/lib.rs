//! Port of `components/label`: a label that names any control and lets a
//! press on it reach the control.
//!
//! # TypeScript file → Rust module
//!
//! `label.ts` → [`label`].
//!
//! # In gpui
//!
//! - `htmlFor` and the label's text are options: gpui elements carry no
//!   attributes, and text children don't name a div.
//! - `ownControl`, `nameControl` and `writeName` are the forms crate's
//!   [`Label`](ui_sdk_forms::Label): it finds the control its `for_id` names,
//!   or the first one inside it, once a frame has drawn, and the control's
//!   internals point their `labelled_by` at every label naming it, in page
//!   order. A control's own `labelled_by`, merged after, wins, as an author's
//!   `aria-labelledby` does.
//! - The original tells a native `<label>`, which the browser activates its
//!   labelable control for, from a label rendered as something else, which
//!   it activates itself. gpui has no native label, so the label does both,
//!   with the kernel's press, which assistive technology's click reaches
//!   too.
//! - The label's `id` is its element's id, as the instance's `id` is the
//!   original's, so a control's `labelled_by` can name it.
//!
//! # Dropped, and why
//!
//! - The `mousedown` handler that keeps a double click from selecting the
//!   label's text: gpui text isn't selected by clicks.
//! - `watchTree`, the `MutationObserver`s and `queueMicrotask`: the forms
//!   crate works out what follows from the tree once each frame has drawn.
//! - A press on a link inside the label counting as the link's: gpui has no
//!   links of its own; a control inside the label still keeps its press.

pub mod label;

pub use label::{Label, LabelOptions};
