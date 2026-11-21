//! Port of `components/number-input`: a text field that keeps a numeric
//! value while it shows formatted text, steps it with the keyboard, its
//! buttons (which repeat while held), the mouse wheel or a drag across a
//! scrub area, and reads what's typed in its locale.
//!
//! `number-input.ts` → [`number_input`](mod@number_input),
//! `number-input-api.ts` → [`number_input_api`]. The component has no
//! `index.ts`; these are its re-exports.
//!
//! An app makes a [`NumberInput`] with `cx.new(|cx| NumberInput::new(options,
//! window, cx))` and renders [`NumberInput::root`] on every frame, with
//! [`NumberInput::group`] around [`NumberInput::decrement_button`],
//! [`NumberInput::control`] and [`NumberInput::increment_button`], and any
//! [`NumberInput::scrub_area`]s. Typing updates the value while the text is
//! a number within the bounds; Enter or leaving the field commits it:
//! parsed, clamped, snapped to the step and formatted. Arrows step, Shift
//! takes the large step and Alt the small one, Page Up and Page Down the
//! large step, Home and End the bounds when there are some. `on_value_change`
//! runs on every change, and `on_value_commit` once the interaction ends.
//!
//! # In gpui
//!
//! - The control is the native crate's `TextField`, its element the part,
//!   a text box like the original's plain text input. What the original passes
//!   through to its native input as attributes are options here:
//!   `placeholder`, and `field_style` for the text's colors and alignment.
//! - The control's part in forms stands for both the original's hidden input
//!   and its native input: it submits the number as is under `name`, or an
//!   empty value, and while it's required a missing value blocks a submit.
//!   A field finds it as its control, and a fieldset or a field disables
//!   it. Enter submits its form implicitly, as a native input's does.
//! - A typed change carries no event, since the native field reports edits
//!   rather than DOM events; it reports how it was typed as its source.
//! - The buttons take no focus: the original's are out of the tab order and
//!   keep focus where it is when pressed, which in gpui is a button that
//!   never takes it. A mouse press focuses the input; a screen reader's
//!   click steps once.
//! - The wheel: gpui's deltas point the way the content moves, so a step
//!   reads them the other way round, as the browser's.
//! - `set_options` takes the window: a new controlled value shows in the
//!   field at once.
//!
//! # Dropped, and why
//!
//! - `createNumberInputController`, `NumberInputController`, the `controller`
//!   option and the scrub area's `controller` prop: the app holds the
//!   entity, whose methods are the controller's, and renders a scrub area
//!   anywhere, inside the root or outside it.
//! - The `hidden-input` slot: the control reports to forms through its
//!   internals.
//! - `controlId`: a field writes the id its label points at on the control's
//!   part in forms, and the kernel makes element ids, which the buttons'
//!   `aria-controls` names.
//! - `valueAttribute`: values written as attributes, for web components.
//! - `restore` and the `Restore` reason: gpui restores no forms after a
//!   reload.
//! - `takeTypedText`: text typed before the page's script ran. There is
//!   always a window.
//! - `inputMode`, `readPlatform` and the `platform` state: gpui's text input
//!   takes no keyboard type.
//! - Pointer lock: gpui can't lock the pointer, so a scrub stops where the
//!   pointer meets the screen's edge, and [`NumberInput::scrub_area_cursor`]
//!   stays hidden.
//! - Blocking `selectstart` during a scrub, `user-select` and `touch-action`:
//!   gpui selects no text outside text fields, and a scrub claims its touch
//!   as it starts.
//!
//! # Deviations
//!
//! - The value is `Option<f64>`, `None` for an empty field, and the
//!   controlled `value` option is `Option<Option<f64>>`: `None` leaves the
//!   field uncontrolled, as `undefined` does.
//! - `increment` and `decrement` take `large` as an argument beside their
//!   change options. The `increment` and `decrement` parts are
//!   [`NumberInput::increment_button`] and [`NumberInput::decrement_button`],
//!   named apart from them.
//! - A scrub area's props take a `key`, which tells several of one input
//!   apart, since each part's element needs an id of its own.

pub mod number_input;
pub mod number_input_api;

pub use number_input::{
    NumberInput, NumberInputButtonProps, NumberInputButtonState, NumberInputChangeOptions,
    NumberInputOptions, NumberInputScrubAreaCursorState, NumberInputScrubAreaOrientation,
    NumberInputScrubAreaProps, NumberInputScrubAreaState, NumberInputState, NumberInputValueChangeReason,
};
pub use ui_sdk_native::{TextFieldAlign, TextFieldStyle};
pub use ui_sdk_number::NumberFormatOptions;
