//! Port of `components/date-input`: a date, a date and time, or a time typed
//! in segments, in the order and on the clock the locale uses.
//!
//! | TypeScript | Rust |
//! | --- | --- |
//! | `date-input.ts` | [`date_input`]: [`DateInput`] and its options |
//! | `time-input.ts` | [`time_input`]: [`TimeInput`] and its options |
//! | `date-input-api.ts` | [`date_input_api`]: the state and api both share, [`DateInputCore`] |
//! | `date-input-model.ts` | [`date_input_model`]: segments, typing and stepping as pure functions |
//! | `date-input-parts.ts` | [`date_input_parts`]: the root and segment parts, and validity |
//!
//! # Segments are text fields
//!
//! Each segment you type in is a `ui_sdk_native::TextField`, as the
//! original's is an `<input>`: typing, an input method, pasting, cutting,
//! deleting and assistive technology's edits reach it through the
//! platform's text input, and the field's `beforeinput` hook hands each to
//! the input, which refuses it and works out the segment itself, as the
//! original's `beforeinput` listener does. The field selects its text on
//! focus and after every change, shows the placeholder while empty, and
//! carries the key context that tells shortcuts a field is focused. A part
//! that took typed keys itself would get no input method, no dictation and
//! no screen reader's typing, which only reach a platform text input.
//!
//! # Deviations
//!
//! - An input method's commit, which the native field reports as a
//!   cancelable `insertFromComposition`, is let through and read back, as
//!   the original reads what the browser wrote once a composition ends
//!   (`adopt`), so full-width digits still fill the segment.
//! - The segments' text is written once the update that changed the state
//!   is done, since a field's own hook can't write the field it runs in.
//! - gpui reports a blur once the next frame is drawn, where a browser's
//!   `focusout` comes as focus moves: the input finishes a segment's typing
//!   as it moves focus from it itself, a segment's blur finishes only what
//!   was typed in it, and the segment the input just moved focus to keeps
//!   its mark for the separator typed next.
//! - Values are `Option<SharedString>`, and the `value` and `default_value`
//!   options `Option<Option<SharedString>>`: `None` leaves the input
//!   uncontrolled, `Some(None)` controls it with no value.
//! - The root reports the value, a missing value and the value's problems
//!   to its form through its internals, with the first segment as where the
//!   message points, as the original's root does through a custom element's
//!   internals. A value before `min` or after `max` reports the original's
//!   English message: there's no native input to read the platform's from.
//! - Enter finishes what's typed and submits the form implicitly
//!   (`ui_sdk_forms::submit_implicitly`), which counts no segment as a text
//!   field, as `bindSegmentSubmit` does.
//! - What a stylesheet sets on each segment's `<input>` that a part's style
//!   can't reach, like the caret's color, a width in `ch` or an alignment
//!   that follows the reading direction, comes from `field_style`, by the
//!   segment's type. Without a width, a segment is `cols` wide, the
//!   original's `size`: 4 `ch` for a year and 2 for the rest.
//! - The `HourCycle` of 12 or 24 is an enum.
//!
//! # Dropped, and why
//!
//! - The `hidden-input` slot and `bindValueInput`: every control reports to
//!   its form through its internals here.
//! - `createDateInputController`, `createTimeInputController` and the
//!   `controller` option: the entity is the controller, and each of its
//!   methods is one call.
//! - `restore` and the `restore` reason, and the value saved for it: gpui
//!   has no form restoring.
//! - Reading digits typed before the page ran (`adopt` as a segment binds):
//!   there is always a window.
//! - Waiting a frame before selecting a focused segment's text: iOS's
//!   scrolling, which it avoids, isn't gpui's.
//! - `inputmode`, `autocomplete` and `spellcheck`: the native field drops
//!   `inputmode`, and its default text input configuration turns off
//!   corrections, capitals and suggestions.
//! - Each segment's `form` attribute and `required` for validity: the root
//!   reports to the form; `required` still marks each segment for
//!   assistive technology.
//! - `preventScroll`: focusing never scrolls in gpui.
//! - The data attributes: parts expose their state.

pub mod date_input;
pub mod date_input_api;
pub mod date_input_model;
pub mod date_input_parts;
pub mod time_input;

pub use date_input::{DateInput, DateInputOptions};
pub use date_input_api::{
    DateInputChangeOptions, DateInputCore, DateInputFieldStyle, DateInputHost,
    DateInputValueChangeReason, SharedOptions, ValueProblem,
};
pub use date_input_model::{
    DateInputGranularity, DateInputSegmentData, DateInputSegmentType, EditableType, HourCycle,
    Kind,
};
pub use date_input_parts::{DateInputSegmentState, DateInputState};
pub use time_input::{
    TimeInput, TimeInputChangeOptions, TimeInputGranularity, TimeInputOptions,
    TimeInputSegmentData, TimeInputSegmentState, TimeInputSegmentType, TimeInputState,
    TimeInputValueChangeReason,
};
pub use ui_sdk_calendar::{DateMatcher, DateMatchers};
pub use ui_sdk_native::{TextFieldAlign, TextFieldStyle};
