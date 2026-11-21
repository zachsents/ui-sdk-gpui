//! Port of `components/calendar`: months of days in a grid, selecting one
//! date, any number, or a range.
//!
//! | TypeScript | Rust |
//! | --- | --- |
//! | `calendar.ts` | [`calendar`]: [`Calendar`], its options and the parts' states |
//! | `calendar-api.ts` | [`calendar_api`]: the state, behavior and parts Date Picker shares |
//! | `calendar-model.ts` | [`calendar_model`]: the pure decisions |
//!
//! # Building on it
//!
//! The original shares `calendarApi` and `calendarSlots` with Date Picker.
//! Here a component keeps a [`CalendarModelState`] and implements
//! [`CalendarHost`] over it, then renders the part functions of
//! [`calendar_api`] (`heading`, `previous_part`, `next_part`,
//! `month_select`, `year_select`, `grid`, `weekdays`, `weekday`, `week`,
//! `day`) with its own entity and calls its functions (`set_value`,
//! `change_value`, `reveal`, `focus`, ..). `calendar_picked` is the
//! original's `picked` hook.
//!
//! # Deviations
//!
//! - The keyboard runs on `ui_sdk_focus::RovingFocus`, as the original's on
//!   `createRovingFocus`, with a move by date: arrows by day and week,
//!   Page Up and Page Down by month, Shift with them by year, Home and End
//!   to the week's ends, kept within `min` and `max`, turning the months
//!   when the date isn't shown. `GridFocus` moves by row and column and
//!   can't reach a date in a month not yet shown.
//! - Enter and Space alone reach a day as gpui's keyboard click, on key up,
//!   which presses it; with Shift, which gpui doesn't click for, the key
//!   picks the day itself.
//! - Focus landing on a day from a pointer is asked about
//!   (`on_focused_change`) as the pointer presses, before gpui moves it, so
//!   a refusal keeps it where it was. Focus landing any other way, like a
//!   screen reader's, is asked about once the frame that moved it is drawn,
//!   since gpui reports focus with frames ([`calendar_api::check_focus`]).
//! - `month-select` and `year-select` are native `<select>`s in the
//!   original. gpui has none, and `ui-sdk-native` stands in only for
//!   `<input>` and `<textarea>`, so each part holds a select's state (its
//!   value, options and disabled state) and is a group with its name, and
//!   the select the app puts inside, like a `ui-sdk-select`, calls
//!   [`Calendar::select`] with its choice, the original's `change`. Name the
//!   select too, like "Month": the group's name doesn't name what's in it.
//! - The heading is a level-2 heading, the adapters' `h2`, named by its
//!   label, since a part needs a role to be heard as the live region it is.
//! - Values are [`CalendarValue`], one variant per shape the original
//!   allows, with `Empty` for `null`; matchers are [`DateMatchers`].
//!   `min_days` and `max_days` are numbers, as in the original. A value
//!   reads as the original's in `observe` and history
//!   ([`CalendarValue::to_data`]), which puts it back exactly.
//! - The weekday, weekdays and week parts take their grid's offset, so each
//!   grid's rows have ids of their own.
//! - Previous and Next are `previous_part` and `next_part`, as in the
//!   pagination, since `previous` and `next` are the api's.
//!
//! # Dropped, and why
//!
//! - `createCalendarController` and the `controller` option: the entity is
//!   the controller, and each of its methods is one call.
//! - `selects`, which renders only the two days a key moves the stop
//!   between: gpui has no subscriptions per part.
//! - `createFocusRecovery`, which focuses the day that replaces one that
//!   left with focus: a day's focus handle is its date's, so a day that
//!   renders again elsewhere, like in another grid as the months turn,
//!   keeps focus, since gpui focus follows the handle, not the element.
//! - The `mousedown` that keeps focus on a press on a day past `min` or
//!   `max`: gpui moves focus on a press only to an element that takes it.
//! - `bindRoot`'s page language read once and today read on the client:
//!   parts read the page's language as they render (`locale_of`), and
//!   today is read as the calendar is made, since there is no server.
//! - The slots' `type: "button"` and the data attributes: parts take their
//!   roles and expose their state.

pub mod calendar;
pub mod calendar_api;
pub mod calendar_model;

pub use calendar::{
    Calendar, CalendarAnchorChangeReason, CalendarChangeOptions, CalendarDayState,
    CalendarFocusedChangeReason, CalendarGridCell, CalendarGridState, CalendarHeadingState,
    CalendarMonthChangeReason, CalendarOptions, CalendarSelectKind, CalendarSelectOption,
    CalendarSelectState, CalendarState, CalendarStepState, CalendarValueChangeReason,
    CalendarWeekdayState,
};
pub use calendar_api::{CalendarHost, CalendarModelState};
pub use calendar_model::{
    CalendarCell, CalendarMode, CalendarValue, DateMatcher, DateMatchers, GridWeeksOptions,
    ISOMonth, MoveDateOptions, PageBy, PressedRange, RangeRules, VisibleRange, add_months_to,
    grid_weeks, is_allowed_range, is_iso_month, matches_date, month_of, month_to_show, move_date,
    normalize_value, press_range, range_to, selected_dates, visible_range,
};
