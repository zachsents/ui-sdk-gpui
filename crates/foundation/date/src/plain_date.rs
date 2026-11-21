//! Port of `foundation/date/plain-date.ts`.
//!
//! Calendar dates as ISO strings (`YYYY-MM-DD`), with arithmetic on whole day
//! numbers. A date has no time and no time zone, so nothing here makes a
//! moment in time: a moment at midnight shifts a day across time zones and
//! daylight saving, which is the most common date picker bug.

use std::time::SystemTime;

use gpui::SharedString;

/// A calendar date as an ISO 8601 string, `YYYY-MM-DD`, like `"2026-10-16"`.
/// What `<input type="date">` submits. Valid dates sort in date order, so
/// `start < end` compares them.
pub type ISODate = SharedString;

/// A calendar date's fields: a year from 1 to 9999, a month from 1 to 12 and
/// a day from 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DateParts {
    pub year: i32,
    pub month: i32,
    pub day: i32,
}

impl DateParts {
    pub const fn new(year: i32, month: i32, day: i32) -> Self {
        Self { year, month, day }
    }
}

// The first and last dates an ISO string can hold, as day numbers.
const FIRST_DAY: i64 = to_days(1, 1, 1);
const LAST_DAY: i64 = to_days(9999, 12, 31);

/// Whether a value is a real ISO date, like `"2026-10-16"`. `"2026-02-30"` isn't.
pub fn is_iso_date(value: &str) -> bool {
    date_parts(value).is_some()
}

/// The year, month and day of an ISO date, or `None` when it isn't a real date.
pub fn date_parts(date: &str) -> Option<DateParts> {
    // `/^(\d{4})-(\d{2})-(\d{2})$/`
    let bytes = date.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
    if !shaped {
        return None;
    }
    let number = |range: std::ops::Range<usize>| date[range].parse::<i32>().ok();
    let parts = DateParts {
        year: number(0..4)?,
        month: number(5..7)?,
        day: number(8..10)?,
    };
    is_real_date(parts).then_some(parts)
}

/// The ISO date of a year, a month from 1 to 12 and a day, or `None` when
/// there's no such date.
pub fn iso_date(year: i32, month: i32, day: i32) -> Option<ISODate> {
    let parts = DateParts { year, month, day };
    is_real_date(parts).then(|| format(parts))
}

/// What [`to_iso_date`] reads: an ISO string, a [`SystemTime`] (its local
/// year, month and day, as the user saw them), [`DateParts`], or a
/// `jiff::civil::Date`, the Rust counterpart of a `Temporal.PlainDate`.
pub trait ToISODate {
    /// The ISO date it names, or `None`.
    fn to_iso_date(&self) -> Option<ISODate>;
}

impl ToISODate for str {
    fn to_iso_date(&self) -> Option<ISODate> {
        is_iso_date(self).then(|| SharedString::from(self.to_string()))
    }
}

impl ToISODate for String {
    fn to_iso_date(&self) -> Option<ISODate> {
        self.as_str().to_iso_date()
    }
}

impl ToISODate for SharedString {
    fn to_iso_date(&self) -> Option<ISODate> {
        is_iso_date(self).then(|| self.clone())
    }
}

impl ToISODate for DateParts {
    fn to_iso_date(&self) -> Option<ISODate> {
        iso_date(self.year, self.month, self.day)
    }
}

impl ToISODate for SystemTime {
    fn to_iso_date(&self) -> Option<ISODate> {
        let timestamp = jiff::Timestamp::try_from(*self).ok()?;
        let date = timestamp.to_zoned_datetime(jiff::tz::TimeZone::system()).date();
        iso_date(date.year().into(), date.month().into(), date.day().into())
    }
}

impl ToISODate for jiff::civil::Date {
    fn to_iso_date(&self) -> Option<ISODate> {
        iso_date(self.year().into(), self.month().into(), self.day().into())
    }
}

impl<T: ToISODate + ?Sized> ToISODate for &T {
    fn to_iso_date(&self) -> Option<ISODate> {
        (**self).to_iso_date()
    }
}

impl<T: ToISODate> ToISODate for Option<T> {
    fn to_iso_date(&self) -> Option<ISODate> {
        self.as_ref()?.to_iso_date()
    }
}

/// The ISO date of an ISO string, a [`SystemTime`] (its local year, month and
/// day, as the user saw them) or anything with a year, month and day, like
/// [`DateParts`] or a `jiff::civil::Date`. `None` for anything else.
pub fn to_iso_date(value: impl ToISODate) -> Option<ISODate> {
    value.to_iso_date()
}

/// The number of days in a month, from 1 to 12, of a year. 0 for a month that
/// doesn't exist.
pub fn days_in_month(year: i32, month: i32) -> i32 {
    if month == 2 && is_leap_year(year.into()) {
        return 29;
    }
    match month {
        1..=12 => MONTH_LENGTHS[(month - 1) as usize],
        _ => 0,
    }
}

const MONTH_LENGTHS: [i32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// The date `days` days later, or earlier when negative, kept between years 1
/// and 9999.
///
/// # Panics
///
/// When `date` isn't an ISO date, as the original throws a `RangeError`.
pub fn add_days(date: &str, days: i64) -> ISODate {
    from_days(clamp_days(days_of(date).saturating_add(days)))
}

/// The date `months` months later, or earlier, keeping the day where it can:
/// January 31 plus a month is the last day of February.
///
/// # Panics
///
/// When `date` isn't an ISO date, as the original throws a `RangeError`.
pub fn add_months(date: &str, months: i64) -> ISODate {
    let DateParts { year, month, day } = parts_of(date);
    let index = i64::from(year) * 12 + i64::from(month) - 1;
    let index = index.saturating_add(months);
    let next_year = index.div_euclid(12);
    if next_year < 1 {
        return format(from_day_number(FIRST_DAY));
    }
    if next_year > 9999 {
        return format(from_day_number(LAST_DAY));
    }
    let next_month = (index - next_year * 12 + 1) as i32;
    let next_year = next_year as i32;
    format(DateParts {
        year: next_year,
        month: next_month,
        day: day.min(days_in_month(next_year, next_month)),
    })
}

/// The date `years` years later, or earlier: February 29 becomes February 28
/// in a common year.
///
/// # Panics
///
/// When `date` isn't an ISO date, as the original throws a `RangeError`.
pub fn add_years(date: &str, years: i64) -> ISODate {
    add_months(date, years.saturating_mul(12))
}

/// Days from `from` to `to`: positive when `to` is later.
///
/// # Panics
///
/// When either isn't an ISO date, as the original throws a `RangeError`.
pub fn days_between(from: &str, to: &str) -> i64 {
    days_of(to) - days_of(from)
}

/// The day of the week, 0 for Sunday to 6 for Saturday, as `Date#getDay`
/// counts.
///
/// # Panics
///
/// When `date` isn't an ISO date, as the original throws a `RangeError`.
pub fn weekday(date: &str) -> u32 {
    // Day 0, January 1 1970, was a Thursday.
    (days_of(date) + 4).rem_euclid(7) as u32
}

/// The first day of the week `date` is in, for weeks starting on
/// `week_starts_on` (0 for Sunday, the original's default).
///
/// # Panics
///
/// When `date` isn't an ISO date, as the original throws a `RangeError`.
pub fn start_of_week(date: &str, week_starts_on: u32) -> ISODate {
    let back = (i64::from(weekday(date)) - i64::from(week_starts_on) + 7) % 7;
    add_days(date, -back)
}

/// The last day of the week `date` is in, for weeks starting on
/// `week_starts_on` (0 for Sunday, the original's default).
///
/// # Panics
///
/// When `date` isn't an ISO date, as the original throws a `RangeError`.
pub fn end_of_week(date: &str, week_starts_on: u32) -> ISODate {
    add_days(&start_of_week(date, week_starts_on), 6)
}

/// The first day of the month `date` is in.
pub fn start_of_month(date: &str) -> ISODate {
    let head: String = date.chars().take(8).collect();
    format!("{head}01").into()
}

/// The last day of the month `date` is in.
///
/// # Panics
///
/// When `date` isn't an ISO date, as the original throws a `RangeError`.
pub fn end_of_month(date: &str) -> ISODate {
    let DateParts { year, month, .. } = parts_of(date);
    format(DateParts {
        year,
        month,
        day: days_in_month(year, month),
    })
}

/// Whether two dates are in the same month of the same year.
pub fn is_same_month(a: &str, b: &str) -> bool {
    a.chars().take(7).eq(b.chars().take(7))
}

/// `date`, moved onto `min` or `max` when it's outside them. Either bound may
/// be missing.
pub fn clamp_date(date: &str, min: Option<&str>, max: Option<&str>) -> ISODate {
    if let Some(min) = min.filter(|min| !min.is_empty())
        && date < min
    {
        return SharedString::from(min.to_string());
    }
    if let Some(max) = max.filter(|max| !max.is_empty())
        && date > max
    {
        return SharedString::from(max.to_string());
    }
    SharedString::from(date.to_string())
}

/// Options for [`month_grid`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonthGridOptions {
    /// The day weeks start on, 0 for Sunday. 0 by default.
    pub week_starts_on: u32,
    /// Always six weeks, so a calendar keeps its height from month to month.
    /// On by default.
    pub fixed_weeks: bool,
}

impl Default for MonthGridOptions {
    fn default() -> Self {
        Self {
            week_starts_on: 0,
            fixed_weeks: true,
        }
    }
}

impl MonthGridOptions {
    pub fn week_starts_on(mut self, week_starts_on: u32) -> Self {
        self.week_starts_on = week_starts_on;
        self
    }

    pub fn fixed_weeks(mut self, fixed_weeks: bool) -> Self {
        self.fixed_weeks = fixed_weeks;
        self
    }
}

/// The weeks a month shows in a calendar, each seven ISO dates starting on
/// `week_starts_on` (0 for Sunday). Days of the months around it fill the
/// first and last weeks. With `fixed_weeks`, the default, there are always
/// six, so a calendar keeps its height from month to month.
///
/// # Panics
///
/// When `month` isn't an ISO date, as the original throws a `RangeError`.
pub fn month_grid(month: &str, options: MonthGridOptions) -> Vec<[ISODate; 7]> {
    let first = start_of_month(month);
    let start = days_of(&start_of_week(&first, options.week_starts_on));
    let end = days_of(&end_of_month(&first));
    let count = if options.fixed_weeks {
        6
    } else {
        // `Math.ceil((end - start + 1) / 7)`
        (end - start + 1 + 6).div_euclid(7)
    };
    (0..count)
        .map(|row| {
            std::array::from_fn(|column| from_days(clamp_days(start + row * 7 + column as i64)))
        })
        .collect()
}

// Day numbers count days from January 1 1970 in the proleptic Gregorian
// calendar, using Howard Hinnant's civil-from-days algorithms, which need no
// date library and no loops: https://howardhinnant.github.io/date_algorithms.html

pub(crate) const fn to_days(year: i64, month: i64, day: i64) -> i64 {
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

pub(crate) fn from_day_number(days: i64) -> DateParts {
    let shifted = days + 719468;
    let era = shifted.div_euclid(146097);
    let day_of_era = shifted - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    DateParts {
        year: year.clamp(i32::MIN.into(), i32::MAX.into()) as i32,
        month: month as i32,
        day: day as i32,
    }
}

fn from_days(days: i64) -> ISODate {
    format(from_day_number(days))
}

fn clamp_days(days: i64) -> i64 {
    days.clamp(FIRST_DAY, LAST_DAY)
}

pub(crate) fn parts_of(date: &str) -> DateParts {
    match date_parts(date) {
        Some(parts) => parts,
        None => panic!("\"{date}\" is not an ISO date (YYYY-MM-DD)."),
    }
}

fn days_of(date: &str) -> i64 {
    let DateParts { year, month, day } = parts_of(date);
    to_days(year.into(), month.into(), day.into())
}

const fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn is_real_date(DateParts { year, month, day }: DateParts) -> bool {
    (1..=9999).contains(&year) && (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month)
}

fn format(DateParts { year, month, day }: DateParts) -> ISODate {
    format!("{year:04}-{month:02}-{day:02}").into()
}
