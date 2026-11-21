//! Port of `foundation/date/format.ts`.
//!
//! Names and labels for calendar dates from `Intl`, which every browser ships
//! with the locale data a date library would bundle; here from ICU4X through
//! [`DateTimeFormat`]. Each date is formatted as a UTC midnight in UTC, so
//! neither the runtime's time zone nor daylight saving can move it to another
//! day.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::intl::{
    DateTimeFormat, DateTimeFormatOptions, DateTimeFormatPart, DateTimeValue, MonthWidth, NumericWidth,
    TextWidth,
};
use crate::plain_date::ISODate;

// Calendars that share the Gregorian calendar's months and days, so a
// Gregorian grid labelled in them is right: only the year and era differ, as
// in Thai (Buddhist) or Japanese (era) years. A locale that defaults to
// another calendar, like `fa-IR` (Persian), is formatted in the Gregorian one,
// or the heading would name a different month than the grid shows.
// `DateTimeFormat` formats only these, and the Gregorian calendar for any
// other.

type FormatterKey = (Option<String>, DateTimeFormatOptions);

thread_local! {
    // Making a formatter costs about 25 times more than using one, and a month
    // grid formats dozens of labels, so each is made once.
    static FORMATTERS: RefCell<HashMap<FormatterKey, Rc<DateTimeFormat>>> = RefCell::default();
}

/// A cached [`DateTimeFormat`] for calendar dates in a locale: UTC, and in a
/// calendar whose months match the Gregorian ones.
///
/// # Panics
///
/// When the locale isn't well formed, as `Intl` throws a `RangeError`.
pub fn date_formatter(locale: Option<&str>, options: &DateTimeFormatOptions) -> Rc<DateTimeFormat> {
    let key = (locale.map(str::to_string), options.clone());
    if let Some(formatter) = FORMATTERS.with(|formatters| formatters.borrow().get(&key).cloned()) {
        return formatter;
    }
    let formatter = Rc::new(DateTimeFormat::new(locale, options));
    FORMATTERS.with(|formatters| formatters.borrow_mut().insert(key, formatter.clone()));
    formatter
}

/// The moment `Intl` formats for an ISO date: its UTC midnight.
fn utc(date: &str) -> DateTimeValue {
    match DateTimeValue::from_iso(date) {
        Some(value) => value,
        None => panic!("\"{date}\" is not an ISO date (YYYY-MM-DD)."),
    }
}

/// An ISO date as people read it in a locale, like "October 16, 2026" or
/// "16.10.2026". `options` are `Intl.DateTimeFormat`'s; `None` is the
/// locale's long date, the label a calendar's day cells use.
///
/// # Panics
///
/// When `date` isn't an ISO date, as the original throws a `RangeError`, or
/// the locale isn't well formed.
pub fn format_date(date: &str, locale: Option<&str>, options: Option<&DateTimeFormatOptions>) -> String {
    let long = long_date();
    date_formatter(locale, options.unwrap_or(&long)).format(utc(date))
}

/// A range of ISO dates as people read it in a locale, like
/// "Oct 16 – 20, 2026". `None` options are the locale's medium date.
///
/// # Panics
///
/// When either date isn't an ISO date, as the original throws a
/// `RangeError`, or the locale isn't well formed.
pub fn format_date_range(
    start: &str,
    end: &str,
    locale: Option<&str>,
    options: Option<&DateTimeFormatOptions>,
) -> String {
    let medium = medium_date();
    // Browsers write plain spaces where Node's ICU writes thin ones, and ICU4X
    // writes thin ones too, so labels read the same everywhere.
    date_formatter(locale, options.unwrap_or(&medium))
        .format_range(utc(start), utc(end))
        .replace(THIN_SPACES, " ")
}

/// The parts `Intl` formats an ISO date into, for building a label of your
/// own. `None` options are the locale's long date.
///
/// # Panics
///
/// When `date` isn't an ISO date, as the original throws a `RangeError`, or
/// the locale isn't well formed.
pub fn format_date_to_parts(
    date: &str,
    locale: Option<&str>,
    options: Option<&DateTimeFormatOptions>,
) -> Vec<DateTimeFormatPart> {
    let long = long_date();
    date_formatter(locale, options.unwrap_or(&long)).format_to_parts(utc(date))
}

/// The twelve month names in a locale, January first, in the form that
/// stands alone, as in a month select or a heading: "январь" rather than the
/// "января" of "16 января". `Long` is the original's default width.
///
/// # Panics
///
/// When the locale isn't well formed.
pub fn month_names(locale: Option<&str>, width: TextWidth) -> [String; 12] {
    let formatter = date_formatter(locale, &DateTimeFormatOptions::default().month(MonthWidth::from(width)));
    std::array::from_fn(|index| formatter.format(DateTimeValue::new(2026, index as i32 + 1, 1)))
}

/// The seven weekday names in a locale, starting on `week_starts_on` (0 for
/// Sunday, the original's default). `Short` is the original's default width.
///
/// # Panics
///
/// When the locale isn't well formed.
pub fn weekday_names(locale: Option<&str>, width: TextWidth, week_starts_on: u32) -> [String; 7] {
    let formatter = date_formatter(locale, &DateTimeFormatOptions::default().weekday(width));
    // January 4 2026 was a Sunday.
    std::array::from_fn(|index| {
        let offset = ((i64::from(week_starts_on) + index as i64) % 7) as i32;
        formatter.format(DateTimeValue::new(2026, 1, 4 + offset))
    })
}

/// Today's ISO date in a time zone, like `"America/New_York"`, or in the
/// runtime's own when `None`. Two machines in different zones can disagree,
/// so pass the same zone wherever a date must match.
///
/// # Panics
///
/// When the time zone isn't one, as `Intl` throws a `RangeError`.
pub fn today_date(time_zone: Option<&str>) -> ISODate {
    let zone = today_zone(time_zone);
    let today = jiff::Timestamp::now().to_zoned_datetime(zone).date();
    format!("{:04}-{:02}-{:02}", today.year(), today.month(), today.day()).into()
}

thread_local! {
    static TODAY_ZONES: RefCell<HashMap<String, jiff::tz::TimeZone>> = RefCell::default();
}

fn today_zone(time_zone: Option<&str>) -> jiff::tz::TimeZone {
    let key = time_zone.unwrap_or("").to_string();
    if let Some(zone) = TODAY_ZONES.with(|zones| zones.borrow().get(&key).cloned()) {
        return zone;
    }
    let zone = match time_zone {
        None => jiff::tz::TimeZone::system(),
        Some(name) => match zone_of(name) {
            Some(zone) => zone,
            None => panic!("Invalid time zone specified: {name}"),
        },
    };
    TODAY_ZONES.with(|zones| zones.borrow_mut().insert(key, zone.clone()));
    zone
}

// A time zone as `Intl` names one: an IANA name in any case, or an offset
// like `+05:30`, `+0530` or `+05`.
fn zone_of(name: &str) -> Option<jiff::tz::TimeZone> {
    if let Some(offset) = offset_of(name) {
        return Some(jiff::tz::TimeZone::fixed(offset));
    }
    jiff::tz::TimeZone::get(name).ok()
}

fn offset_of(name: &str) -> Option<jiff::tz::Offset> {
    let sign = match name.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits = name[1..].replacen(':', "", 1);
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) || !(digits.len() == 2 || digits.len() == 4) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits.get(2..).filter(|rest| !rest.is_empty()).map_or(Ok(0), str::parse).ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    jiff::tz::Offset::from_seconds(sign * (hours * 3600 + minutes * 60)).ok()
}

pub(crate) fn long_date() -> DateTimeFormatOptions {
    DateTimeFormatOptions::default()
        .weekday(TextWidth::Long)
        .year(NumericWidth::Numeric)
        .month(MonthWidth::Long)
        .day(NumericWidth::Numeric)
}

pub(crate) fn medium_date() -> DateTimeFormatOptions {
    DateTimeFormatOptions::default()
        .year(NumericWidth::Numeric)
        .month(MonthWidth::Short)
        .day(NumericWidth::Numeric)
}

const THIN_SPACES: [char; 2] = ['\u{2009}', '\u{202F}'];
