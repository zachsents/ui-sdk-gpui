//! Port of `components/password-input`: a native password field with a
//! button that shows and hides what's typed.
//!
//! `password-input.ts` → [`password_input`], `index.ts` → these re-exports,
//! with the reason type `password-input.ts` re-exports from `ui-sdk-forms`.
//!
//! An app makes a [`PasswordInput`] with `cx.new(|cx|
//! PasswordInput::new(options, window, cx))` and renders
//! [`PasswordInput::root`], the field's box, with [`PasswordInput::control`],
//! the native field, and [`PasswordInput::toggle`] inside it. The control's
//! value, reasons, resets and form participation are the forms crate's
//! `TextFieldApi`, over the native crate's `TextField`, which masks the
//! password while it's hidden.
//!
//! # In gpui
//!
//! - The control's native attributes, `placeholder`, `id`, `min_length`,
//!   `max_length`, `pattern` and `size`, which the original takes on the
//!   control part, are options: the component makes its native field.
//! - The browser's own work on the control is the component's: Enter
//!   submits its form implicitly, a disabled fieldset or a Field disables
//!   it, and `pattern` and the lengths of a typed password reach the form as
//!   a `Problem`, with Chromium's English messages.
//! - Caps Lock is gpui's `window.capslock()`, read on the control's key,
//!   mouse and modifier events.
//! - `"page-hide"` is the window being hidden: gpui has no page to leave or
//!   keep in a back-forward cache.
//! - The toggle is a native button, as adapters render it: disabled, it
//!   leaves the tab order and reports itself disabled.
//! - `set_options` takes the window: a new controlled value shows in the
//!   field at once.
//!
//! # Dropped, and why
//!
//! - `createPasswordInputController`, `PasswordInputController` and the
//!   `controller` option: the app holds the entity, whose methods are the
//!   controller's.
//! - `autoComplete` (`"current-password"`, `"new-password"`): gpui has no
//!   autofill or password managers to tell which password to offer.
//! - `controlId`: the id adapters pass for web component markup. The
//!   control's `id` option is its id for labels; relations name the part.
//! - `showText`'s second restore a frame later: it works around Chromium and
//!   WebKit moving the caret after a mouse press switches an input's type.
//!   The native field keeps its selection through the switch.
//! - `buttonKeydown` and `buttonKeyup` on the toggle: gpui turns Enter and
//!   Space on a focused part into a keyboard click itself.
//! - `PasswordInputPrimitive`: a type renderers read.

pub mod password_input;

pub use password_input::{
    PasswordInput, PasswordInputOptions, PasswordInputState, PasswordInputVisibleChangeReason,
};
pub use ui_sdk_forms::InputValueChangeReason;
pub use ui_sdk_native::TextFieldStyle;
