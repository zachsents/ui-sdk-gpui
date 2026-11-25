//! Port of `components/date-picker`: type a date or choose one from a
//! calendar, one date or a range, in a form.
//!
//! | TypeScript | Rust |
//! | --- | --- |
//! | `date-picker.ts` | [`date_picker`]: [`DatePicker`], its options, the parts' states and props, and its agent surface |
//! | `date-picker-api.ts` | [`date_picker_api`]: the state and the api: typing, committing, presets, clearing, the popup and the form |
//! | `date-picker-parts.ts` | [`date_picker_parts`]: the parts, and Calendar's inside the popup |
//! | `index.ts` | these re-exports |
//!
//! # Building on Calendar and the popups
//!
//! The picker keeps a `ui_sdk_calendar::CalendarModelState` and implements
//! `CalendarHost` over it, as the original builds on `calendarApi`: Calendar's
//! keys, range picking and parts work in its popup, rendered with the
//! picker's entity ([`DatePicker::grid`], [`DatePicker::day`], ..), and a
//! picked date, or a range's end, closes the popup. The popup is the popups
//! crate's, with `date_picker_preset`: placed below the input and lined up
//! with its start, a non-modal dialog in Tab's flow, presenting as a sheet
//! with `presentation`. Its trigger and inputs open it, so the picker builds
//! its own trigger, and its own content, which moves focus to the
//! calendar's tab stop as it opens.
//!
//! # Deviations
//!
//! - Each input is a `ui_sdk_native::TextField` the picker makes, as the
//!   original's is an `<input type="text">`. What the original passes
//!   through to it as attributes, its `id`, `placeholder` and the style a
//!   stylesheet would set, are [`DatePickerOptions`] (`id`, `placeholder`,
//!   `end_placeholder`, `field_style`). The text is written to the field from
//!   the state once the update that changed it is over, or at once from
//!   `set_options`, which takes the window for it.
//! - An input takes only typing and pasting that leave its text a date, or
//!   the start of one in any form it reads ("1", "10/1", "oct"), as Date
//!   Input's segments take only what makes a date; the original's input
//!   takes any text and marks it invalid once committed. Deleting always
//!   goes through, so text that isn't a date still commits as invalid.
//! - What the browser does with a native input is the picker's: Enter
//!   submits its form implicitly once the typed date is committed, and the
//!   form hears `Input` as you type and `Change` as an edit is committed.
//! - The root reports to its form through its internals, as the original's
//!   form-associated root does: the date, or each end under its name, a
//!   missing date while `required` and an input is empty, and text that
//!   isn't a date as a bad input, with that input as where the message
//!   points. The native inputs' own `required` and custom validity, and the
//!   hidden inputs, are gone with them. A Field, and labels, name the
//!   start's input through it.
//! - The inputs are described by the hint, and, since accesskit computes no
//!   description from what a part points at, by its format too.
//! - gpui reports a blur once the next frame is drawn, so a typed date
//!   commits a frame after focus leaves its input.
//! - gpui can't make a closing popup inert. A press on a day of another
//!   month turns the months as it closes the popup, and the calendar may
//!   move focus to that day while the popup animates out; once it has
//!   closed, focus left in it goes back where the popup returns focus.
//! - Values are Calendar's `CalendarValue` ([`DatePickerValue`]), with
//!   `Empty` for `null`, and the mode is [`DatePickerMode`].
//! - The content's state adds the popup's presence, placement and sheet,
//!   which the original writes as attributes and CSS variables. Its props
//!   add the `timing` its CSS transitions would declare and the sheet's dim.
//! - The `clear` slot is [`DatePicker::clear_part`], since `clear` is the
//!   api's, and Calendar's `previous` and `next` are `previous_part` and
//!   `next_part`, as in Calendar.
//! - `DatePickerInputProps` and `DatePickerPresetProps` are their part
//!   methods' arguments: an input's `end`, and a preset's `value`.
//! - A range's two inputs count as one text field when Enter decides
//!   whether to submit a form with no submit button.
//!
//! # Dropped, and why
//!
//! - `createDatePickerController` and the `controller` option: the entity is
//!   the controller, and each of its methods is one call.
//! - `DatePickerPrimitive` and `DatePickerApi`: types renderers read. The
//!   api is the entity's methods.
//! - The `hidden-input` slot and `bindValueInput`: every control reports to
//!   its form through its internals here.
//! - `restore`, the `restore` reason and the value saved for it: gpui has no
//!   form restoring.
//! - `takeTypedText`, and reading the page's language as an input binds:
//!   text typed before a page's script ran. There is always a window.
//! - `container` on the root and the content (`PortalProps`): the popup
//!   renders above the window in `deferred(..)`.
//! - `asCalendar`, which gives Calendar's parts Calendar's `data-slot` so one
//!   stylesheet styles both: gpui has no stylesheets; parts style from their
//!   state.
//! - The state's `apple`: nothing in the date picker reads it.
//! - `rootValidates`: the root always reports.
//! - The inputs' `autocomplete`, `autocorrect` and `spellcheck`: gpui has no
//!   autofill, and the native field's text input configuration already
//!   turns off corrections.
//! - The slots' `type: "button"` and the data attributes: parts take their
//!   roles and expose their state.

pub mod date_picker;
pub mod date_picker_api;
pub mod date_picker_parts;

pub use date_picker::{
    DatePicker, DatePickerChangeOptions, DatePickerClearState, DatePickerContentProps,
    DatePickerContentState, DatePickerDismissReason, DatePickerHintState, DatePickerInputState,
    DatePickerMode, DatePickerMonthChangeReason, DatePickerOpenChangeReason, DatePickerOptions,
    DatePickerPresetState, DatePickerState, DatePickerTriggerState, DatePickerValue,
    DatePickerValueChangeReason,
};
pub use date_picker_api::INVALID_DATE_MESSAGE;
pub use date_picker_parts::INPUT_KEYS;

// Calendar's types that its options and the calendar parts in its popup use.
pub use ui_sdk_calendar::{
    CalendarDayState, CalendarGridCell, CalendarGridState, CalendarHeadingState,
    CalendarSelectKind, CalendarSelectOption, CalendarSelectState, CalendarStepState,
    CalendarValue, CalendarWeekdayState, DateMatcher, DateMatchers, ISOMonth, PageBy,
};
pub use ui_sdk_date::TextWidth;
pub use ui_sdk_native::{TextFieldAlign, TextFieldStyle};
pub use ui_sdk_popups::{Presentation, PresentationSetting};
