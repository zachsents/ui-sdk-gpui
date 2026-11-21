//! Port of `components/input`: Input, a native text field for short values;
//! Textarea, one for several lines; and InputGroup, text or buttons around an
//! input, with a Clear part.
//!
//! `input.ts` → [`input`](mod@input), `index.ts` → these re-exports, with
//! the reason type `input.ts` re-exports from `ui-sdk-forms`.
//!
//! An app makes an [`Input`] with `cx.new(|cx| Input::new(options, window,
//! cx))` and renders [`Input::root`], the native field itself, which it
//! styles as the input and names with `aria_label` or a forms crate `Label`.
//! Its value, reasons, controlled state, resets and form participation are
//! the forms crate's `TextFieldApi`, over the native crate's `TextField`. An
//! [`InputGroup`] renders [`InputGroup::group`] around an input and the text
//! or buttons beside it, and [`InputGroup::clear_part`], which is `None`
//! while there's nothing to clear.
//!
//! # In gpui
//!
//! - What the original passes through to the native `<input>` or
//!   `<textarea>` as attributes, like `type`, `placeholder`, `name`,
//!   `maxLength`, `pattern`, `rows` or `id`, are options here: the component
//!   makes its native field. So are the root's props, `disabled`,
//!   `read_only` and `required`, which decide how the native field behaves.
//! - The browser's own work on a native field is the component's:
//!   - Enter in an Input submits its form implicitly, once every listener
//!     had the key and none prevented it, after the field's `change`.
//!   - A disabled fieldset, or a Field that disables the control, disables
//!     the native field.
//!   - Constraint validation: `pattern`, `min_length` and `max_length` (on
//!     text the user typed), the type of an email or URL field, and a number
//!     field's bad input, range and step reach the form as a `Problem`, with
//!     Chromium's English messages. `required` is the value api's.
//!   - The field's `on_submit`, which asks to submit the form it's in, like
//!     a suggestion that sends what's typed, submits the form the forms crate
//!     found.
//! - Input hints (`input_hints`) become the field's text input configuration:
//!   corrections, capitals, suggestions and the Return key's label.
//! - Textarea's `auto_resize` is the native field's, which grows with the
//!   text between the part's `min_h` and `max_h`, as `field-sizing: content`
//!   does.
//! - An InputGroup finds its field among the forms crate's nodes inside its
//!   element, a frame after they draw, and shows that field's state.
//! - `set_options` takes the window: a new controlled value shows in the
//!   field at once, and writing the field takes the window.
//!
//! # Dropped, and why
//!
//! - `createInputController`, `InputController` and the `controller`
//!   option: the app holds the entities, whose methods are the controller's.
//! - `InputProps` and `TextareaProps`: the root's props are options, since
//!   they set the native field the component makes.
//! - `fitContent`: measuring a textarea to its text. The native field sizes
//!   itself with `auto_resize`.
//! - `InputGroup`'s fallback for a field that isn't a text field of ours
//!   (`writeValue(el, "", { asUser: true })`): every text field here is one.
//! - `inputmode` and `autocomplete` among the hints: gpui's text input takes
//!   no keyboard type or autofill kind. The field's kind and `autocomplete`
//!   still reach the agent bridge as the part's native attributes, so a
//!   password, a card or a one-time code is secret there.
//! - `InputPrimitive`, `TextareaPrimitive`, `InputGroupPrimitive`: types
//!   renderers read.
//!
//! # Deviations
//!
//! - Lengths count characters, as the native field's maximum does, not
//!   UTF-16 code units.
//! - A number field's bad input still submits its text, where a browser
//!   submits nothing.

pub mod input;

pub use input::{
    Input, InputGroup, InputGroupOptions, InputGroupState, InputOptions, InputState, InputStep,
    Textarea, TextareaOptions, TextareaState, TextareaSubmitCallback, TextareaSubmitDetails,
    TextareaSubmitShortcut,
};
pub use ui_sdk_forms::{InputHintKind, InputHintOptions, InputHints, InputValueChangeReason, input_hints};
pub use ui_sdk_native::{TextFieldKind, TextFieldStyle};
