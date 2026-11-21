//! Port of `components/otp-input`: a one-time code input, a slot per
//! character, for a code sent by text or email.
//!
//! `otp-input.ts` → [`otp_input`](mod@otp_input), `otp-input-api.ts` →
//! [`otp_input_api`], `otp-input-model.ts` → [`otp_input_model`],
//! `index.ts` → these re-exports.
//!
//! An app makes an [`OtpInput`] with `cx.new(|cx| OtpInput::new(options,
//! window, cx))` and renders [`OtpInput::root`] holding its
//! [`OtpInput::group`]s of [`OtpInput::slot`]s, with
//! [`OtpInput::separator`]s between groups, and [`OtpInput::control`] last,
//! over them. Like the original, one native field holds the whole code: the
//! native crate's `TextField`, invisible and over the slots, which render
//! from its value and selection. The field's own typing, deleting, undo,
//! paste and input methods work on the code; the component keeps what goes
//! in to what the pattern accepts, spreads a pasted code across the slots,
//! keeps a slot selected under the caret, moves a slot at a time with the
//! arrow keys, Home and End, and takes over a mouse press to select the slot
//! under it.
//!
//! # In gpui
//!
//! - The field takes part in forms through its own `ElementInternals`,
//!   since its value, reasons and commit aren't a plain text field's: the
//!   code submits under `name`, `required` blocks an empty code, and the
//!   pattern and the code's length reach the form as a `Problem`, with
//!   Chromium's English messages: a partial code the user typed is too
//!   short, and a partial default or written code mismatches the pattern.
//!   Enter submits the form implicitly, and a form reset returns to the
//!   default.
//! - A pattern that isn't a preset is a `regex::Regex`, flags inline, like
//!   `(?i)[a-f0-9]`. [`read_pattern`] reads one from text, as the original
//!   reads the element's attribute.
//! - What reaches the field is decided before it applies, in its
//!   `on_before_input`: one fitting character is typing, and anything else,
//!   a paste whole, goes through the component once the field's update is
//!   over, as one step the field can undo.
//! - A controlled input waits a frame for its owner to pass a typed code
//!   back, where the original waits a microtask: owners render on the next
//!   frame here.
//! - Groups and separators take an index, which tells them apart: gpui
//!   needs each part's id unique.
//! - Positions count characters rather than UTF-16 units, and convert to the
//!   field's byte offsets.
//!
//! # Dropped, and why
//!
//! - `createOTPInputController`, `OTPInputController` and the `controller`
//!   option: the app holds the entity, whose methods are the controller's.
//! - `inputMode` and `inputModeFor`, and `autoComplete`
//!   (`"one-time-code"`): gpui's text input takes no keyboard type, and has
//!   no autofill or codes from messages to offer.
//! - The password manager badge (`badges`, `badgeSpace`, `checkBadge`,
//!   `pushBadge`, the `clip-path`): there are no password manager extensions
//!   to make room for.
//! - `hideNativeHighlights`'s stylesheet: the field's style paints its text,
//!   caret, selection and composition transparent itself.
//! - `inputStyle`'s 16px monospace text and negative letter spacing: they
//!   keep iOS from zooming in and the browser's text from scrolling, which
//!   don't happen here.
//! - Restoring the code a browser put back after going back to the page
//!   (the `restore` reason): gpui has no history to restore from.
//! - The `pointerdown` tracking of what pressed last: gpui's events say it.
//! - `slotCache`: slots computed once per state so parts compare the same
//!   array. Rust compares state by value.
//! - `writing`: set while the core writes the input's value so its `input`
//!   listener skips it. A write fires no `input` here.
//! - `OtpInputPrimitive`: a type renderers read.

pub mod otp_input;
pub mod otp_input_api;
pub mod otp_input_model;

pub use otp_input::{OtpInput, OtpInputSlotState, OtpInputState, read_pattern};
pub use otp_input_api::{
    OtpInputChangeOptions, OtpInputOptions, OtpInputTextAlign, OtpInputValueChangeReason, clean_code,
};
pub use otp_input_model::{OtpInputPattern, OtpInputSlotData};
