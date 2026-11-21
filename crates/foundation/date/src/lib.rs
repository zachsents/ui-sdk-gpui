//! Port of `foundation/date`: calendar dates without a date library. Dates
//! are ISO strings (`YYYY-MM-DD`) with whole-day arithmetic, and every name,
//! order and first day of the week comes from CLDR, as the browser's `Intl`
//! gives them to the original. Calendar and Date Picker are built on it, and
//! so can any date component of your own.
//!
//! `date.ts` re-exports the files' API for apps and `index.ts` the part the
//! rest of core imports; this crate root does for both.
//!
//! | TypeScript | Rust |
//! | --- | --- |
//! | `plain-date.ts` | [`plain_date`] |
//! | `week.ts` | [`week`] |
//! | `format.ts` | [`format`](mod@format) |
//! | `parse.ts` | [`parse`] |
//! | the browser's `Intl` | [`intl`] |
//!
//! # Locales
//!
//! A locale is a BCP 47 tag, as the original takes one, or `None` for the
//! runtime's own: the system's language. A tag CLDR has no data for formats
//! in the runtime's locale, as browsers do. [`intl`] says how its
//! `DateTimeFormat` matches a browser's output.
//!
//! # Deviations
//!
//! - [`ISODate`] is a `SharedString`, and functions take any `&str`.
//!   `isISODate(value: unknown)` and `dateParts(date | null)` take `&str`.
//! - A JavaScript `Date` is a [`std::time::SystemTime`] where the original
//!   reads one's local date ([`to_iso_date`]), and a [`DateTimeValue`] where
//!   it formats a moment. A `Temporal.PlainDate` is a `jiff::civil::Date`.
//!   [`to_iso_date`] takes any [`ToISODate`].
//! - Default arguments become explicit ones: a week starting on 0
//!   ([`start_of_week`], [`end_of_week`], [`weekday_names`]),
//!   [`MonthGridOptions::default`], [`TextWidth::Long`] for
//!   [`month_names`] and [`TextWidth::Short`] for [`weekday_names`], and
//!   `None` options for [`format_date`] and [`format_date_to_parts`] (the
//!   long date) and [`format_date_range`] (the medium date).
//! - `Intl.DateTimeFormatOptions` is [`DateTimeFormatOptions`], and the
//!   formatter [`date_formatter`] caches is [`DateTimeFormat`].
//! - What throws a `RangeError` in the original panics, with its message: an
//!   ISO date that isn't one in arithmetic or formatting, a locale tag that
//!   isn't well formed, a time zone that isn't one. What the original
//!   catches, it still catches: [`first_day_of_week`] of a tag that isn't
//!   well formed is Monday.
//! - The `iso8601` calendar formats as Gregorian: ICU4X has no patterns of
//!   its own for it.
//!
//! # Dropped
//!
//! - `week.ts`'s table of CLDR regions, for browsers without
//!   `Intl.Locale#getWeekInfo`: ICU4X always has the week data.
//! - `todayDate`'s advice on server rendering, and the reason
//!   `formatDateRange` replaces thin spaces: there's no server. The spaces
//!   are still replaced, since ICU4X writes thin ones too.

mod field_names;
pub mod format;
pub mod intl;
mod numeric_padding;
pub mod parse;
pub mod plain_date;
pub mod week;

pub use format::{
    date_formatter, format_date, format_date_range, format_date_to_parts, month_names, today_date,
    weekday_names,
};
pub use intl::{
    DateTimeField, DateTimeFormat, DateTimeFormatOptions, DateTimeFormatPart, DateTimeFormatPartType,
    DateTimeStyle, DateTimeValue, HourCycle, MonthWidth, NumericWidth, ResolvedDateTimeFormatOptions,
    TextWidth, date_time_field_name,
};
pub use parse::{DateField, DatePattern, ParseDateOptions, date_pattern, parse_date};
pub use plain_date::{
    DateParts, ISODate, MonthGridOptions, ToISODate, add_days, add_months, add_years, clamp_date,
    date_parts, days_between, days_in_month, end_of_month, end_of_week, is_iso_date, is_same_month,
    iso_date, month_grid, start_of_month, start_of_week, to_iso_date, weekday,
};
pub use week::first_day_of_week;
