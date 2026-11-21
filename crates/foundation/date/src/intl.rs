//! What the original asks the browser's `Intl` for, from ICU4X's compiled
//! CLDR data: `Intl.DateTimeFormat` as [`DateTimeFormat`], the date fields'
//! `Intl.DisplayNames` as [`date_time_field_name`], and the locale-aware case
//! mapping, digits and character classes `parse.rs` reads text with.
//!
//! No TypeScript file matches this module: the browser provides it to the
//! original.
//!
//! # Locales
//!
//! A locale is a BCP 47 tag, as `Intl` takes one, or `None` for the
//! runtime's own: the system's language here. A locale CLDR has no data for
//! falls back to the runtime's, as browsers do. A tag that isn't well formed
//! panics, where `Intl` throws a `RangeError`.
//!
//! # Formatting
//!
//! [`DateTimeFormat`] formats in UTC, which is all the original asks of it.
//! Browsers pick a pattern for the fields asked for from CLDR's
//! `availableFormats`, then stretch its fields to the widths asked for. ICU4X
//! ships three patterns per set of fields, one per length, so this picks the
//! closest of those with the same distance ICU's pattern generator uses, and
//! adjusts the widths by its rules: text fields take the width asked for,
//! numeric fields are padded to two digits when asked, and a field is never
//! turned from text to a number or back. Ranges come from ICU4X's interval
//! patterns, with the same widths.
//!
//! Browsers write CLDR's narrow no-break space, as in "4:05 PM", as a plain
//! space, and so does this; the thin spaces of a range stay.
//!
//! Numbers in a date written in numbers pad as the browser pads them, from a
//! table of CLDR's `availableFormats`, since ICU4X's patterns only show how a
//! locale's short date pads: German writes `5.1.2026`, British English
//! `05/01/2026`.
//!
//! Gone, as they mean nothing for a calendar date formatted in UTC: the
//! `timeZone` and `timeZoneName` options, and the zone in `timeStyle`'s full
//! and long forms. `dayPeriod` writes AM and PM, since ICU4X has no flexible
//! day periods ("in the afternoon"). Calendars whose months aren't the
//! Gregorian ones format as Gregorian, as `dateFormatter` asks; `iso8601`
//! does too, since ICU4X has no patterns of its own for it. A 1 to 24 clock
//! is a 0 to 23 one whose midnight writes 24, since ICU4X has no `k`.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use gpui::SharedString;
use icu_calendar::cal::{Buddhist, Japanese, Roc};
use icu_calendar::preferences::CalendarPreferences;
use icu_calendar::{AnyCalendarKind, Date, Gregorian};
use icu_datetime::fieldsets::builder::{DateFields, FieldSetBuilder};
use icu_datetime::fieldsets::enums::CompositeDateTimeFieldSet;
use icu_datetime::input::{DateTime, Time};
use icu_datetime::options::{Length, SubsecondDigits, TimePrecision, YearStyle};
use icu_datetime::pattern::{DateTimePattern, FixedCalendarDateTimeNames};
use icu_datetime::provider::fields::{self, Field, FieldLength, FieldSymbol};
use icu_datetime::provider::pattern::{PatternItem, runtime};
use icu_datetime::range::FixedCalendarDateRangeFormatter;
use icu_datetime::{DateTimeFormatterPreferences, FixedCalendarDateTimeFormatter};
use icu_locale_core::extensions::unicode::{Value, key};
use icu_locale_core::{LanguageIdentifier, Locale};
use writeable::{Part, PartsWrite, TryWriteable, Writeable};

use crate::field_names::FIELD_NAMES;
use crate::numeric_padding::NUMERIC_PADDING;
use crate::plain_date::{DateParts, from_day_number, to_days};

// Locales.

/// A locale tag as `Intl` reads one, or `None` when it isn't well formed.
pub(crate) fn parse_locale(tag: &str) -> Option<Locale> {
    Locale::try_from_str(tag).ok()
}

/// The runtime's own locale, what `Intl` uses when it's given none: the
/// system's language, read as the text crate's `toLocale` reads a POSIX tag.
pub(crate) fn default_locale() -> Locale {
    thread_local! {
        static DEFAULT: Locale = read_default_locale();
    }
    DEFAULT.with(Clone::clone)
}

fn read_default_locale() -> Locale {
    sys_locale::get_locale()
        .and_then(|tag| {
            // A POSIX locale's encoding and modifier go, as in `de_DE.UTF-8@euro`.
            let tag = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-");
            parse_locale(&tag)
        })
        .filter(has_own_data)
        .unwrap_or(icu_locale_core::locale!("en-US"))
}

/// The locale `Intl` formats in for a tag: the tag's, or the runtime's when
/// CLDR has no data for its language.
///
/// # Panics
///
/// When the tag isn't well formed, as `Intl` throws a `RangeError`.
pub(crate) fn resolve_locale(tag: Option<&str>) -> Locale {
    let Some(tag) = tag else {
        return default_locale();
    };
    match parse_locale(tag) {
        Some(locale) if has_own_data(&locale) => locale,
        Some(_) => default_locale(),
        None => panic!("Incorrect locale information provided: \"{tag}\""),
    }
}

// Whether CLDR has date names for the locale's language, not only the root's.
fn has_own_data(locale: &Locale) -> bool {
    use icu_datetime::provider::names::DatetimeNamesMonthGregorianV1;
    if locale.id.language.is_unknown() {
        return false;
    }
    use icu_datetime::provider::semantic_skeletons::marker_attrs;
    use icu_provider::prelude::*;
    let data_locale = DataLocale::from(locale);
    let request = DataRequest {
        id: DataIdentifierBorrowed::for_marker_attributes_and_locale(marker_attrs::WIDE, &data_locale),
        ..Default::default()
    };
    match DataProvider::<DatetimeNamesMonthGregorianV1>::load(&icu_datetime::provider::Baked, request)
    {
        Ok(response) => !response
            .metadata
            .locale
            .is_some_and(|resolved| resolved.is_unknown()),
        Err(_) => false,
    }
}

// Text.

/// `String.prototype.toLocaleLowerCase(locale)`.
pub(crate) fn to_locale_lower_case(text: &str, locale: &Locale) -> String {
    icu_casemap::CaseMapper::new()
        .lowercase_to_string(text, &locale.id)
        .into_owned()
}

/// `String.prototype.toLocaleUpperCase(locale)`.
pub(crate) fn to_locale_upper_case(text: &str, locale: &Locale) -> String {
    icu_casemap::CaseMapper::new()
        .uppercase_to_string(text, &locale.id)
        .into_owned()
}

/// `String.prototype.normalize("NFKC")`.
pub(crate) fn normalize_nfkc(text: &str) -> String {
    icu_normalizer::ComposingNormalizer::new_nfkc()
        .normalize(text)
        .into_owned()
}

/// The locale's digits, nine first, as
/// `new Intl.NumberFormat(locale, { useGrouping: false }).format(9876543210)`
/// writes them.
pub(crate) fn native_digits(locale: &Locale) -> String {
    use icu_decimal::input::Decimal;
    use icu_decimal::options::{DecimalFormatterOptions, GroupingStrategy};
    let mut options = DecimalFormatterOptions::default();
    options.grouping_strategy = Some(GroupingStrategy::Never);
    match icu_decimal::DecimalFormatter::try_new(locale.into(), options) {
        Ok(formatter) => formatter
            .format(&Decimal::from(9876543210_u64))
            .write_to_string()
            .into_owned(),
        Err(_) => "9876543210".to_string(),
    }
}

fn general_category(c: char) -> icu_properties::props::GeneralCategory {
    icu_properties::CodePointMapData::<icu_properties::props::GeneralCategory>::new().get(c)
}

/// `\p{L}`.
pub(crate) fn is_letter(c: char) -> bool {
    icu_properties::props::GeneralCategoryGroup::Letter.contains(general_category(c))
}

/// `\p{M}`.
pub(crate) fn is_mark(c: char) -> bool {
    icu_properties::props::GeneralCategoryGroup::Mark.contains(general_category(c))
}

/// `\p{N}`.
pub(crate) fn is_number(c: char) -> bool {
    icu_properties::props::GeneralCategoryGroup::Number.contains(general_category(c))
}

/// `\p{Nd}`.
pub(crate) fn is_decimal_digit(c: char) -> bool {
    general_category(c) == icu_properties::props::GeneralCategory::DecimalNumber
}

// Field names.

/// A date or time field `Intl.DisplayNames` names with `type: "dateTimeField"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateTimeField {
    Year,
    Month,
    Day,
    DayPeriod,
    Hour,
    Minute,
    Second,
}

impl DateTimeField {
    /// The field's name in English, which `Intl` writes where it has no data.
    pub fn english(self) -> &'static str {
        ENGLISH_FIELD_NAMES[self as usize]
    }
}

const ENGLISH_FIELD_NAMES: [&str; 7] = ["year", "month", "day", "AM/PM", "hour", "minute", "second"];

/// A field's name in a locale, like "Jahr" or "month", as
/// `new Intl.DisplayNames(locale, { type: "dateTimeField" }).of(field)` gives
/// it. A locale without its own falls back as `Intl` does: to its parent
/// locales, then the runtime's locale, then English.
///
/// # Panics
///
/// When the locale isn't well formed, as `Intl` throws a `RangeError`.
pub fn date_time_field_name(locale: Option<&str>, field: DateTimeField) -> SharedString {
    let locale = match locale {
        Some(tag) => parse_locale(tag)
            .unwrap_or_else(|| panic!("Incorrect locale information provided: \"{tag}\"")),
        None => default_locale(),
    };
    let names = field_names_of(&locale)
        .or_else(|| field_names_of(&default_locale()))
        .unwrap_or(&ENGLISH_FIELD_NAMES);
    SharedString::new_static(names[field as usize])
}

// The names for a locale or one of its parents.
fn field_names_of(locale: &Locale) -> Option<&'static [&'static str; 7]> {
    find_for_locale(FIELD_NAMES, locale)
}

// A table's entry for a locale or one of its parents: CLDR's fallback chain,
// which infers a script where the tag has none, so `zh-TW` reads `zh-Hant`.
fn find_for_locale<T>(table: &'static [(&'static str, T)], locale: &Locale) -> Option<&'static T> {
    let find = |tag: &str| {
        table
            .binary_search_by(|(key, _)| key.cmp(&tag))
            .ok()
            .map(|index| &table[index].1)
    };
    let id = &locale.id;
    let mut candidates: Vec<LanguageIdentifier> = vec![LanguageIdentifier {
        language: id.language,
        script: id.script,
        region: id.region,
        variants: Default::default(),
    }];
    let fallbacker = icu_locale_fallback::LocaleFallbacker::new();
    let mut chain = fallbacker
        .for_config(Default::default())
        .fallback_for(icu_provider::DataLocale::from(locale));
    while !chain.get().is_unknown() {
        let step = chain.get();
        candidates.push(LanguageIdentifier {
            language: step.language,
            script: step.script,
            region: step.region,
            variants: Default::default(),
        });
        chain.step();
    }
    candidates.iter().find_map(|candidate| find(&candidate.to_string()))
}

// `DateTimeFormat`'s options.

/// How wide a text field is: `"narrow"`, `"short"` or `"long"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextWidth {
    Narrow,
    Short,
    Long,
}

impl TextWidth {
    /// The width's name, as `Intl` writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            TextWidth::Narrow => "narrow",
            TextWidth::Short => "short",
            TextWidth::Long => "long",
        }
    }
}

/// How a number field is written: `"numeric"`, or `"2-digit"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NumericWidth {
    Numeric,
    TwoDigit,
}

/// How a month is written: as a number, `"2-digit"`, or a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MonthWidth {
    Numeric,
    TwoDigit,
    Narrow,
    Short,
    Long,
}

impl From<TextWidth> for MonthWidth {
    fn from(width: TextWidth) -> Self {
        match width {
            TextWidth::Narrow => MonthWidth::Narrow,
            TextWidth::Short => MonthWidth::Short,
            TextWidth::Long => MonthWidth::Long,
        }
    }
}

/// A stock date or time format: `dateStyle` and `timeStyle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateTimeStyle {
    Full,
    Long,
    Medium,
    Short,
}

/// A clock: `"h11"` (0 to 11), `"h12"` (1 to 12), `"h23"` (0 to 23) or
/// `"h24"` (1 to 24).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HourCycle {
    H11,
    H12,
    H23,
    H24,
}

impl HourCycle {
    /// The cycle's name, as `Intl` writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            HourCycle::H11 => "h11",
            HourCycle::H12 => "h12",
            HourCycle::H23 => "h23",
            HourCycle::H24 => "h24",
        }
    }
}

/// `Intl.DateTimeFormatOptions`: the fields to show and how wide, or a stock
/// style, and the calendar, digits and clock to use.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DateTimeFormatOptions {
    pub weekday: Option<TextWidth>,
    pub era: Option<TextWidth>,
    pub year: Option<NumericWidth>,
    pub month: Option<MonthWidth>,
    pub day: Option<NumericWidth>,
    /// Written as AM and PM: ICU4X has no flexible day periods.
    pub day_period: Option<TextWidth>,
    pub hour: Option<NumericWidth>,
    pub minute: Option<NumericWidth>,
    pub second: Option<NumericWidth>,
    /// 1 to 3 digits of a second.
    pub fractional_second_digits: Option<u8>,
    /// A 12 hour clock, or a 24 hour one; wins over `hour_cycle`.
    pub hour12: Option<bool>,
    pub hour_cycle: Option<HourCycle>,
    /// A calendar, like `"gregory"`, which wins over the locale's `-u-ca-`.
    pub calendar: Option<SharedString>,
    /// A numbering system, like `"latn"`, which wins over the locale's `-u-nu-`.
    pub numbering_system: Option<SharedString>,
    /// A stock date format. With a style, the field options are ignored.
    pub date_style: Option<DateTimeStyle>,
    /// A stock time format.
    pub time_style: Option<DateTimeStyle>,
}

impl DateTimeFormatOptions {
    pub fn weekday(mut self, width: TextWidth) -> Self {
        self.weekday = Some(width);
        self
    }

    pub fn era(mut self, width: TextWidth) -> Self {
        self.era = Some(width);
        self
    }

    pub fn year(mut self, width: NumericWidth) -> Self {
        self.year = Some(width);
        self
    }

    pub fn month(mut self, width: impl Into<MonthWidth>) -> Self {
        self.month = Some(width.into());
        self
    }

    pub fn day(mut self, width: NumericWidth) -> Self {
        self.day = Some(width);
        self
    }

    pub fn day_period(mut self, width: TextWidth) -> Self {
        self.day_period = Some(width);
        self
    }

    pub fn hour(mut self, width: NumericWidth) -> Self {
        self.hour = Some(width);
        self
    }

    pub fn minute(mut self, width: NumericWidth) -> Self {
        self.minute = Some(width);
        self
    }

    pub fn second(mut self, width: NumericWidth) -> Self {
        self.second = Some(width);
        self
    }

    pub fn fractional_second_digits(mut self, digits: u8) -> Self {
        self.fractional_second_digits = Some(digits);
        self
    }

    pub fn hour12(mut self, hour12: bool) -> Self {
        self.hour12 = Some(hour12);
        self
    }

    pub fn hour_cycle(mut self, hour_cycle: HourCycle) -> Self {
        self.hour_cycle = Some(hour_cycle);
        self
    }

    pub fn calendar(mut self, calendar: impl Into<SharedString>) -> Self {
        self.calendar = Some(calendar.into());
        self
    }

    pub fn numbering_system(mut self, numbering_system: impl Into<SharedString>) -> Self {
        self.numbering_system = Some(numbering_system.into());
        self
    }

    pub fn date_style(mut self, style: DateTimeStyle) -> Self {
        self.date_style = Some(style);
        self
    }

    pub fn time_style(mut self, style: DateTimeStyle) -> Self {
        self.time_style = Some(style);
        self
    }

    // Any field option, which a style can't be used with.
    fn has_fields(&self) -> bool {
        self.era.is_some() || self.asks_for_a_field()
    }

    // A field that keeps Intl from adding its default date: any but the era.
    fn asks_for_a_field(&self) -> bool {
        self.weekday.is_some()
            || self.year.is_some()
            || self.month.is_some()
            || self.day.is_some()
            || self.day_period.is_some()
            || self.hour.is_some()
            || self.minute.is_some()
            || self.second.is_some()
            || self.fractional_second_digits.is_some()
    }
}

/// A moment [`DateTimeFormat`] formats: a date, and a time of day on it, in
/// UTC. The month counts from 1, as ISO dates do. Fields past their range
/// carry over, as `Date.UTC` carries them: month 13 is January of the next
/// year, and day 0 the last day of the month before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DateTimeValue {
    pub year: i32,
    pub month: i32,
    pub day: i32,
    pub hour: i32,
    pub minute: i32,
    pub second: i32,
    pub millisecond: i32,
}

impl DateTimeValue {
    /// Midnight of a date.
    pub const fn new(year: i32, month: i32, day: i32) -> Self {
        Self {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
            millisecond: 0,
        }
    }

    /// The same date at a time of day.
    pub const fn at(mut self, hour: i32, minute: i32, second: i32) -> Self {
        self.hour = hour;
        self.minute = minute;
        self.second = second;
        self
    }

    pub const fn millisecond(mut self, millisecond: i32) -> Self {
        self.millisecond = millisecond;
        self
    }

    /// Midnight of an ISO date, or `None` when it isn't one.
    pub fn from_iso(date: &str) -> Option<Self> {
        crate::plain_date::date_parts(date).map(Self::from)
    }
}

impl From<DateParts> for DateTimeValue {
    fn from(parts: DateParts) -> Self {
        Self::new(parts.year, parts.month, parts.day)
    }
}

// The value with every field in range, as `Date.UTC` reads it.
#[derive(Clone, Copy, Debug)]
struct Civil {
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    nanosecond: u32,
}

const DAY: i64 = 86_400_000;

fn civil(value: DateTimeValue) -> Civil {
    let month = i64::from(value.month) - 1;
    let year = i64::from(value.year) + month.div_euclid(12);
    let first = to_days(year, month.rem_euclid(12) + 1, 1);
    let time = ((i64::from(value.hour) * 60 + i64::from(value.minute)) * 60 + i64::from(value.second))
        * 1000
        + i64::from(value.millisecond);
    let days = first + i64::from(value.day) - 1 + time.div_euclid(DAY);
    let time = time.rem_euclid(DAY);
    let parts = from_day_number(days);
    Civil {
        year: parts.year,
        month: parts.month as u8,
        day: parts.day as u8,
        hour: (time / 3_600_000) as u8,
        minute: (time / 60_000 % 60) as u8,
        second: (time / 1000 % 60) as u8,
        nanosecond: (time % 1000) as u32 * 1_000_000,
    }
}

// The moment the original formats when it builds a pattern: a day whose
// fields all have two digits, at a time with an afternoon hour.
const SAMPLE: DateTimeValue = DateTimeValue::new(2026, 10, 16).at(16, 5, 6);

/// A part of a formatted date: `Intl.DateTimeFormatPart`'s `type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateTimeFormatPartType {
    Day,
    DayPeriod,
    Era,
    FractionalSecond,
    Hour,
    Literal,
    Minute,
    Month,
    RelatedYear,
    Second,
    TimeZoneName,
    Weekday,
    Year,
    YearName,
    Unknown,
}

impl DateTimeFormatPartType {
    /// The type's name, as `Intl` writes it, like `"month"`.
    pub fn as_str(self) -> &'static str {
        use DateTimeFormatPartType::*;
        match self {
            Day => "day",
            DayPeriod => "dayPeriod",
            Era => "era",
            FractionalSecond => "fractionalSecond",
            Hour => "hour",
            Literal => "literal",
            Minute => "minute",
            Month => "month",
            RelatedYear => "relatedYear",
            Second => "second",
            TimeZoneName => "timeZoneName",
            Weekday => "weekday",
            Year => "year",
            YearName => "yearName",
            Unknown => "unknown",
        }
    }

    fn of(part: &Part) -> Option<Self> {
        use DateTimeFormatPartType::*;
        if part.category != "datetime" {
            return None;
        }
        Some(match part.value {
            "day" => Day,
            "dayPeriod" => DayPeriod,
            "era" => Era,
            "hour" => Hour,
            "minute" => Minute,
            "month" => Month,
            "relatedYear" => RelatedYear,
            "second" => Second,
            "timeZoneName" => TimeZoneName,
            "weekday" => Weekday,
            "year" => Year,
            "yearName" => YearName,
            "startRange" | "endRange" => return None,
            _ => Unknown,
        })
    }
}

/// One piece of a formatted date, `Intl.DateTimeFormatPart`: what it is and
/// its text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DateTimeFormatPart {
    pub r#type: DateTimeFormatPartType,
    pub value: String,
}

/// What a [`DateTimeFormat`] settled on, `resolvedOptions()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedDateTimeFormatOptions {
    /// The locale it formats in, as a tag.
    pub locale: SharedString,
    /// The calendar, like `"gregory"` or `"buddhist"`.
    pub calendar: SharedString,
    /// The digits, like `"latn"` or `"arab"`.
    pub numbering_system: SharedString,
    /// Always `"UTC"`.
    pub time_zone: SharedString,
    /// The clock, when an hour shows.
    pub hour_cycle: Option<HourCycle>,
    /// Whether that clock has 12 hours.
    pub hour12: Option<bool>,
    pub date_style: Option<DateTimeStyle>,
    pub time_style: Option<DateTimeStyle>,
}

// What a pattern's fields are, for matching and stretching them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Era,
    Year,
    Month,
    Day,
    Weekday,
    DayPeriod,
    Hour,
    Minute,
    Second,
}

const KINDS: [Kind; 9] = [
    Kind::Era,
    Kind::Year,
    Kind::Month,
    Kind::Day,
    Kind::Weekday,
    Kind::DayPeriod,
    Kind::Hour,
    Kind::Minute,
    Kind::Second,
];

fn kind_of(symbol: FieldSymbol) -> Option<Kind> {
    Some(match symbol {
        FieldSymbol::Era => Kind::Era,
        FieldSymbol::Year(_) => Kind::Year,
        FieldSymbol::Month(_) => Kind::Month,
        FieldSymbol::Day(fields::Day::DayOfMonth) => Kind::Day,
        FieldSymbol::Weekday(_) => Kind::Weekday,
        FieldSymbol::DayPeriod(_) => Kind::DayPeriod,
        FieldSymbol::Hour(_) => Kind::Hour,
        FieldSymbol::Minute => Kind::Minute,
        FieldSymbol::Second(fields::Second::Second) | FieldSymbol::DecimalSecond(_) => Kind::Second,
        _ => return None,
    })
}

fn length_of(length: FieldLength) -> Option<u8> {
    Some(match length {
        FieldLength::One => 1,
        FieldLength::Two => 2,
        FieldLength::Three => 3,
        FieldLength::Four => 4,
        FieldLength::Five => 5,
        FieldLength::Six => 6,
        // A number in another numbering system, which keeps its form.
        _ => return None,
    })
}

fn field_length(length: u8) -> FieldLength {
    match length {
        1 => FieldLength::One,
        2 => FieldLength::Two,
        3 => FieldLength::Three,
        4 => FieldLength::Four,
        5 => FieldLength::Five,
        _ => FieldLength::Six,
    }
}

fn is_text(field: Field) -> bool {
    let length = length_of(field.length).unwrap_or(1);
    match field.symbol {
        FieldSymbol::Era | FieldSymbol::DayPeriod(_) => true,
        FieldSymbol::Year(fields::Year::Cyclic) => true,
        FieldSymbol::Month(_) => length >= 3,
        FieldSymbol::Weekday(fields::Weekday::Format) => true,
        FieldSymbol::Weekday(_) => length >= 3,
        _ => false,
    }
}

// A width as the matcher compares it: an era or weekday written with one to
// three letters is abbreviated either way.
fn width_of(field: Field) -> Option<u8> {
    let length = length_of(field.length)?;
    Some(match field.symbol {
        FieldSymbol::Era | FieldSymbol::Weekday(fields::Weekday::Format) if length <= 3 => 3,
        _ => length,
    })
}

// A field the options ask for, as a skeleton field: its kind, width and
// whether it's text.
#[derive(Clone, Copy, Debug)]
struct Want {
    kind: Kind,
    length: u8,
    text: bool,
}

fn text_length(width: TextWidth) -> u8 {
    match width {
        TextWidth::Short => 3,
        TextWidth::Long => 4,
        TextWidth::Narrow => 5,
    }
}

fn numeric_length(width: NumericWidth) -> u8 {
    match width {
        NumericWidth::Numeric => 1,
        NumericWidth::TwoDigit => 2,
    }
}

fn wants_of(options: &DateTimeFormatOptions) -> Vec<Want> {
    let mut wants = Vec::new();
    let mut want = |kind, length, text| wants.push(Want { kind, length, text });
    if let Some(era) = options.era {
        want(Kind::Era, text_length(era), true);
    }
    if let Some(year) = options.year {
        want(Kind::Year, numeric_length(year), false);
    }
    if let Some(month) = options.month {
        match month {
            MonthWidth::Numeric => want(Kind::Month, 1, false),
            MonthWidth::TwoDigit => want(Kind::Month, 2, false),
            MonthWidth::Short => want(Kind::Month, 3, true),
            MonthWidth::Long => want(Kind::Month, 4, true),
            MonthWidth::Narrow => want(Kind::Month, 5, true),
        }
    }
    if let Some(day) = options.day {
        want(Kind::Day, numeric_length(day), false);
    }
    if let Some(weekday) = options.weekday {
        want(Kind::Weekday, text_length(weekday), true);
    }
    if let Some(day_period) = options.day_period {
        want(Kind::DayPeriod, text_length(day_period), true);
    }
    if let Some(hour) = options.hour {
        want(Kind::Hour, numeric_length(hour), false);
    }
    if let Some(minute) = options.minute {
        want(Kind::Minute, numeric_length(minute), false);
    }
    if let Some(second) = options.second {
        want(Kind::Second, numeric_length(second), false);
    }
    wants
}

// How far a pattern is from the fields asked for, as ICU's pattern generator
// counts it: a field missing costs most, an extra one less, a number for a
// name less still, and a different width least. An era or a day period a
// pattern adds, as calendars and clocks do, costs nothing.
fn distance(items: &[PatternItem], wants: &[Want]) -> u32 {
    let fields: Vec<Field> = items
        .iter()
        .filter_map(|item| match item {
            PatternItem::Field(field) => Some(*field),
            PatternItem::Literal(_) => None,
        })
        .collect();
    let mut distance = 0;
    for kind in KINDS {
        let want = wants.iter().find(|want| want.kind == kind);
        let have = fields.iter().find(|field| kind_of(field.symbol) == Some(kind));
        distance += match (want, have) {
            (Some(want), Some(field)) => {
                let wanted = if want.text && want.length <= 3 && matches!(kind, Kind::Era | Kind::Weekday) {
                    3
                } else {
                    want.length
                };
                if is_text(*field) != want.text {
                    100
                } else if width_of(*field) != Some(wanted) {
                    1
                } else {
                    0
                }
            }
            (Some(_), None) => 100_000,
            (None, Some(_)) if matches!(kind, Kind::Era | Kind::DayPeriod) => 0,
            (None, Some(_)) => 10_000,
            (None, None) => 0,
        };
    }
    distance
}

// The pattern with its fields at the widths asked for, as ICU's pattern
// generator adjusts the one it found: text takes the width asked for,
// numbers are padded to two digits when asked and otherwise keep the
// locale's padding for the date asked for, minutes and seconds keep theirs,
// and nothing turns from text to a number or back.
fn adjust(items: &[PatternItem], wants: &[Want], padding: Padding) -> Vec<PatternItem> {
    items
        .iter()
        .map(|item| {
            let PatternItem::Field(field) = *item else {
                return *item;
            };
            let Some(kind) = kind_of(field.symbol) else {
                return *item;
            };
            let Some(want) = wants.iter().find(|want| want.kind == kind) else {
                return *item;
            };
            if matches!(kind, Kind::Minute | Kind::Second | Kind::DayPeriod) || is_text(field) != want.text {
                return *item;
            }
            let Some(length) = length_of(field.length) else {
                return *item;
            };
            let pads = match kind {
                Kind::Day => padding.day,
                Kind::Month => padding.month,
                _ => None,
            };
            let length = if !want.text {
                match (want.length, pads) {
                    (2, _) => 2.max(length),
                    (_, Some(true)) => 2,
                    (_, Some(false)) => 1,
                    _ => length,
                }
            } else if want.length == 3
                && length <= 3
                && matches!(field.symbol, FieldSymbol::Era | FieldSymbol::Weekday(fields::Weekday::Format))
            {
                length
            } else {
                want.length
            };
            PatternItem::Field(Field {
                symbol: field.symbol,
                length: field_length(length),
            })
        })
        .collect()
}

// The pattern of a lone field whose closest pattern writes other fields too,
// as ICU's pattern generator writes the field itself.
fn lone_field(want: Want, closest: &[PatternItem]) -> PatternItem {
    // Minutes and seconds keep the generator's one digit, whatever was asked.
    let length = match want.kind {
        Kind::Minute | Kind::Second => FieldLength::One,
        _ => field_length(want.length),
    };
    let symbol = match want.kind {
        Kind::Era => FieldSymbol::Era,
        Kind::Year => FieldSymbol::Year(fields::Year::Calendar),
        Kind::Month => FieldSymbol::Month(fields::Month::Format),
        Kind::Day => FieldSymbol::Day(fields::Day::DayOfMonth),
        Kind::Weekday => FieldSymbol::Weekday(fields::Weekday::Format),
        Kind::DayPeriod => FieldSymbol::DayPeriod(fields::DayPeriod::AmPm),
        Kind::Hour => closest
            .iter()
            .find_map(|item| match item {
                PatternItem::Field(Field {
                    symbol: symbol @ FieldSymbol::Hour(_),
                    ..
                }) => Some(*symbol),
                _ => None,
            })
            .unwrap_or(FieldSymbol::Hour(fields::Hour::H23)),
        Kind::Minute => FieldSymbol::Minute,
        Kind::Second => FieldSymbol::Second(fields::Second::Second),
    };
    PatternItem::Field(Field { symbol, length })
}

// The set of fields ICU4X formats for the ones asked for, filling in the
// fields between, as its own options bag does: a year and a day show the
// month too.
fn date_fields_of(options: &DateTimeFormatOptions) -> Option<DateFields> {
    let year = options.era.is_some() || options.year.is_some();
    let month = options.month.is_some();
    let day = options.day.is_some();
    let weekday = options.weekday.is_some();
    Some(match (year, month, day, weekday) {
        (false, false, false, false) => return None,
        (true, false, false, false) => DateFields::Y,
        (false, true, false, false) => DateFields::M,
        (true, true, false, false) => DateFields::YM,
        (false, false, true, false) => DateFields::D,
        (false, true, true, false) => DateFields::MD,
        (true, _, true, false) => DateFields::YMD,
        (false, false, false, true) => DateFields::E,
        (false, false, true, true) => DateFields::DE,
        (false, true, _, true) => DateFields::MDE,
        (true, _, _, true) => DateFields::YMDE,
    })
}

fn has_year(fields: DateFields) -> bool {
    matches!(
        fields,
        DateFields::Y | DateFields::YM | DateFields::YMD | DateFields::YMDE
    )
}

fn is_calendar_period(fields: DateFields) -> bool {
    matches!(fields, DateFields::Y | DateFields::M | DateFields::YM)
}

fn time_precision_of(options: &DateTimeFormatOptions) -> Option<TimePrecision> {
    if let Some(digits) = options.fractional_second_digits {
        return Some(TimePrecision::Subsecond(match digits {
            0 | 1 => SubsecondDigits::S1,
            2 => SubsecondDigits::S2,
            _ => SubsecondDigits::S3,
        }));
    }
    if options.second.is_some() {
        Some(TimePrecision::Second)
    } else if options.minute.is_some() {
        Some(TimePrecision::Minute)
    } else if options.hour.is_some() || options.day_period.is_some() {
        Some(TimePrecision::Hour)
    } else {
        None
    }
}

fn year_style_of(options: &DateTimeFormatOptions) -> Option<YearStyle> {
    if options.era.is_some() {
        return Some(YearStyle::WithEra);
    }
    match options.year {
        Some(NumericWidth::Numeric) => Some(YearStyle::Full),
        Some(NumericWidth::TwoDigit) => Some(YearStyle::Auto),
        None => None,
    }
}

// The length ICU4X's options bag picks for the widths asked for: long for a
// long name, medium for a short or narrow one, short for a numeric month.
fn preferred_length(options: &DateTimeFormatOptions) -> Length {
    let long = matches!(options.month, Some(MonthWidth::Long))
        || matches!(options.weekday, Some(TextWidth::Long))
        || matches!(options.day_period, Some(TextWidth::Long));
    let abbreviated = matches!(options.month, Some(MonthWidth::Short | MonthWidth::Narrow))
        || matches!(options.weekday, Some(TextWidth::Short | TextWidth::Narrow))
        || matches!(options.day_period, Some(TextWidth::Short | TextWidth::Narrow));
    if long {
        Length::Long
    } else if abbreviated {
        Length::Medium
    } else if matches!(options.month, Some(MonthWidth::Numeric | MonthWidth::TwoDigit)) {
        Length::Short
    } else {
        Length::Medium
    }
}

// Whether the locale pads a numeric day or month to two digits in the date
// asked for, from CLDR's patterns for dates in numbers, where ICU4X's lengths
// don't say. `None` where it has no word on it.
#[derive(Clone, Copy, Debug, Default)]
struct Padding {
    day: Option<bool>,
    month: Option<bool>,
}

fn padding_of(locale: &Locale, options: &DateTimeFormatOptions) -> Padding {
    if !matches!(options.month, Some(MonthWidth::Numeric | MonthWidth::TwoDigit)) {
        return Padding::default();
    }
    let year = options.year.is_some() || options.era.is_some();
    let day = options.day.is_some();
    let weekday = options.weekday.is_some();
    // The bits of `yMd`, `Md`, `yM`, `MEd` and `yMEd`.
    let (day_bit, month_bit) = match (year, day, weekday) {
        (true, true, false) => (Some(0), 1),
        (false, true, false) => (Some(2), 3),
        (true, false, false) => (None, 4),
        (false, true, true) => (Some(5), 6),
        (true, true, true) => (Some(7), 8),
        _ => return Padding::default(),
    };
    let Some(mask) = find_for_locale(NUMERIC_PADDING, locale) else {
        return Padding::default();
    };
    Padding {
        day: day_bit.map(|bit| mask & (1 << bit) != 0),
        month: Some(mask & (1 << month_bit) != 0),
    }
}

fn build_field_set(
    length: Length,
    date_fields: Option<DateFields>,
    time_precision: Option<TimePrecision>,
    year_style: Option<YearStyle>,
) -> Option<CompositeDateTimeFieldSet> {
    let mut builder = FieldSetBuilder::default();
    builder.length = Some(length);
    builder.date_fields = date_fields;
    builder.time_precision = time_precision;
    builder.year_style = year_style.filter(|_| date_fields.is_some_and(has_year));
    builder.build_composite_datetime().ok()
}

// Writes formatted output and the parts it marks.
#[derive(Default)]
struct Collector {
    text: String,
    parts: Vec<(usize, usize, Part)>,
}

impl fmt::Write for Collector {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.text.push_str(text);
        Ok(())
    }
}

impl PartsWrite for Collector {
    type SubPartsWrite = Collector;

    fn with_part(
        &mut self,
        part: Part,
        mut write: impl FnMut(&mut Self::SubPartsWrite) -> fmt::Result,
    ) -> fmt::Result {
        let start = self.text.len();
        write(self)?;
        self.parts.push((start, self.text.len(), part));
        Ok(())
    }
}

// Which date of a range a part shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Start,
    End,
}

// A typed piece of formatted text.
#[derive(Clone, Debug)]
struct Piece {
    r#type: DateTimeFormatPartType,
    value: String,
    side: Side,
}

// The formatted text in pieces, as `formatToParts` gives them: each field,
// and the literal text between, with a second's fraction apart from it, and
// spaces as browsers write them.
fn pieces(collector: &Collector) -> Vec<Piece> {
    let text = &collector.text;
    let mut spans: Vec<(usize, usize, DateTimeFormatPartType)> = collector
        .parts
        .iter()
        .filter_map(|(start, end, part)| Some((*start, *end, DateTimeFormatPartType::of(part)?)))
        .collect();
    spans.sort_by_key(|(start, end, _)| (*start, std::cmp::Reverse(*end)));
    let side_of = |at: usize| {
        let within = |value: &str| {
            collector
                .parts
                .iter()
                .any(|(start, end, part)| part.value == value && *start <= at && at < *end)
        };
        if within("endRange") { Side::End } else { Side::Start }
    };
    let decimal = |start: usize, end: usize, value: &str| {
        collector
            .parts
            .iter()
            .find(|(from, to, part)| {
                part.category == "decimal" && part.value == value && *from >= start && *to <= end
            })
            .map(|(from, to, _)| (*from, *to))
    };
    let mut pieces: Vec<Piece> = Vec::new();
    let literal = |pieces: &mut Vec<Piece>, value: &str, side: Side| {
        if value.is_empty() {
            return;
        }
        if let Some(last) = pieces.last_mut()
            && last.r#type == DateTimeFormatPartType::Literal
        {
            last.value.push_str(value);
            return;
        }
        pieces.push(Piece {
            r#type: DateTimeFormatPartType::Literal,
            value: value.to_string(),
            side,
        });
    };
    let mut at = 0;
    for (start, end, r#type) in spans {
        if start < at {
            continue;
        }
        literal(&mut pieces, &text[at..start], side_of(at));
        let side = side_of(start);
        let fraction = (r#type == DateTimeFormatPartType::Second)
            .then(|| decimal(start, end, "decimal").zip(decimal(start, end, "fraction")))
            .flatten();
        match fraction {
            Some(((point, point_end), (fraction, fraction_end))) => {
                pieces.push(Piece {
                    r#type,
                    value: text[start..point].to_string(),
                    side,
                });
                literal(&mut pieces, &text[point..point_end], side);
                pieces.push(Piece {
                    r#type: DateTimeFormatPartType::FractionalSecond,
                    value: text[fraction..fraction_end].to_string(),
                    side,
                });
            }
            None => pieces.push(Piece {
                r#type,
                value: text[start..end].to_string(),
                side,
            }),
        }
        at = end;
    }
    literal(&mut pieces, &text[at..], side_of(at));
    // Browsers write CLDR's narrow no-break space, as before "PM", as a space.
    for piece in &mut pieces {
        if piece.value.contains('\u{202F}') {
            piece.value = piece.value.replace('\u{202F}', " ");
        }
    }
    pieces
}

type Names<C> = FixedCalendarDateTimeNames<C, CompositeDateTimeFieldSet>;

// A stretched pattern and the names it needs, or `None` when they can't load.
type NamesEntry<C> = Option<Rc<(DateTimePattern, Names<C>)>>;

// One calendar's formatters: the closest pattern's, the names a stretched
// pattern needs, and the range formatter once a range is asked for.
struct Engine<C: icu_datetime::scaffold::CldrCalendar> {
    calendar: C,
    prefs: DateTimeFormatterPreferences,
    field_set: CompositeDateTimeFieldSet,
    formatter: FixedCalendarDateTimeFormatter<C, CompositeDateTimeFieldSet>,
    names: RefCell<HashMap<String, NamesEntry<C>>>,
    range: OnceCell<Option<FixedCalendarDateRangeFormatter<C, CompositeDateTimeFieldSet>>>,
}

macro_rules! engine {
    ($calendar:ty, $make:expr) => {
        impl Engine<$calendar> {
            fn try_new(
                prefs: DateTimeFormatterPreferences,
                field_set: CompositeDateTimeFieldSet,
            ) -> Option<Self> {
                let formatter =
                    FixedCalendarDateTimeFormatter::<$calendar, CompositeDateTimeFieldSet>::try_new(
                        prefs, field_set,
                    )
                    .ok()?;
                Some(Self {
                    calendar: $make,
                    prefs,
                    field_set,
                    formatter,
                    names: RefCell::default(),
                    range: OnceCell::new(),
                })
            }

            fn input(&self, value: DateTimeValue) -> DateTime<$calendar> {
                let civil = civil(value);
                let date = Date::try_new_iso(civil.year, civil.month, civil.day)
                    .or_else(|_| Date::try_new_iso(civil.year.clamp(-9999, 9999), civil.month, civil.day))
                    .unwrap_or_else(|_| Date::try_new_iso(1970, 1, 1).expect("1970-01-01 is a date"));
                let time = Time::try_new(civil.hour, civil.minute, civil.second, civil.nanosecond)
                    .unwrap_or_else(|_| Time::start_of_day());
                DateTime {
                    date: date.to_calendar(self.calendar),
                    time,
                }
            }

            fn pattern(&self, input: &DateTime<$calendar>) -> Vec<PatternItem> {
                let pattern = runtime::Pattern::from(self.formatter.format(input).pattern());
                pattern.items.iter().collect()
            }

            fn names(&self, items: &[PatternItem]) -> NamesEntry<$calendar> {
                let pattern = DateTimePattern::from(runtime::Pattern::from(items.to_vec()));
                let key = pattern.write_to_string().into_owned();
                if let Some(entry) = self.names.borrow().get(&key) {
                    return entry.clone();
                }
                let entry = Names::<$calendar>::try_new(self.prefs).ok().and_then(|mut names| {
                    names.include_for_pattern(&pattern).ok()?;
                    Some(Rc::new((pattern, names)))
                });
                self.names.borrow_mut().insert(key, entry.clone());
                entry
            }

            // Formats the moment, with the stretched pattern when it differs.
            fn write(&self, value: DateTimeValue, items: Option<&[PatternItem]>, sink: &mut Collector) {
                let input = self.input(value);
                if let Some(items) = items
                    && let Some(entry) = self.names(items)
                {
                    let mut stretched = Collector::default();
                    let written = entry
                        .1
                        .with_pattern_unchecked(&entry.0)
                        .format(&input)
                        .try_write_to_parts(&mut stretched);
                    if matches!(written, Ok(Ok(()))) {
                        *sink = stretched;
                        return;
                    }
                }
                let _ = self.formatter.format(&input).write_to_parts(sink);
            }

            // One field of the moment, alone.
            fn write_field(&self, field: Field, value: DateTimeValue) -> Option<String> {
                let mut sink = Collector::default();
                let entry = self.names(&[PatternItem::Field(field)])?;
                let written = entry
                    .1
                    .with_pattern_unchecked(&entry.0)
                    .format(&self.input(value))
                    .try_write_to_parts(&mut sink);
                matches!(written, Ok(Ok(()))).then_some(sink.text)
            }

            fn write_range(&self, start: DateTimeValue, end: DateTimeValue, sink: &mut Collector) -> bool {
                let range = self.range.get_or_init(|| {
                    FixedCalendarDateRangeFormatter::<$calendar, CompositeDateTimeFieldSet>::try_new(
                        self.prefs,
                        self.field_set,
                    )
                    .ok()
                });
                let Some(range) = range else {
                    return false;
                };
                range
                    .format(&self.input(start), &self.input(end))
                    .write_to_parts(sink)
                    .is_ok()
            }
        }
    };
}

engine!(Gregorian, Gregorian);
engine!(Buddhist, Buddhist);
engine!(Japanese, Japanese::new());
engine!(Roc, Roc);

enum AnyEngine {
    Gregorian(Engine<Gregorian>),
    Buddhist(Engine<Buddhist>),
    Japanese(Engine<Japanese>),
    Roc(Engine<Roc>),
}

macro_rules! each_engine {
    ($engine:expr, $name:ident => $body:expr) => {
        match $engine {
            AnyEngine::Gregorian($name) => $body,
            AnyEngine::Buddhist($name) => $body,
            AnyEngine::Japanese($name) => $body,
            AnyEngine::Roc($name) => $body,
        }
    };
}

// The calendars whose months are the Gregorian ones, which a formatter keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CalendarKind {
    Gregorian,
    Buddhist,
    Japanese,
    Roc,
}

impl CalendarKind {
    fn name(self) -> &'static str {
        match self {
            CalendarKind::Gregorian => "gregory",
            CalendarKind::Buddhist => "buddhist",
            CalendarKind::Japanese => "japanese",
            CalendarKind::Roc => "roc",
        }
    }

    fn engine(self, prefs: DateTimeFormatterPreferences, field_set: CompositeDateTimeFieldSet) -> Option<AnyEngine> {
        Some(match self {
            CalendarKind::Gregorian => AnyEngine::Gregorian(Engine::<Gregorian>::try_new(prefs, field_set)?),
            CalendarKind::Buddhist => AnyEngine::Buddhist(Engine::<Buddhist>::try_new(prefs, field_set)?),
            CalendarKind::Japanese => AnyEngine::Japanese(Engine::<Japanese>::try_new(prefs, field_set)?),
            CalendarKind::Roc => AnyEngine::Roc(Engine::<Roc>::try_new(prefs, field_set)?),
        })
    }
}

/// `Intl.DateTimeFormat`, for the moments [`DateTimeValue`] holds, in UTC.
pub struct DateTimeFormat {
    locale: Locale,
    calendar: CalendarKind,
    options: DateTimeFormatOptions,
    hour_cycle: Option<HourCycle>,
    wants: Vec<Want>,
    padding: Padding,
    // A lone field whose closest pattern writes others too, written alone.
    lone: Option<PatternItem>,
    engine: AnyEngine,
}

impl fmt::Debug for DateTimeFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DateTimeFormat")
            .field("locale", &self.locale.to_string())
            .field("calendar", &self.calendar.name())
            .field("options", &self.options)
            .finish()
    }
}

impl DateTimeFormat {
    /// A formatter for a locale (the runtime's when `None`) and options, as
    /// `new Intl.DateTimeFormat(locale, { ...options, timeZone: "UTC" })`.
    ///
    /// # Panics
    ///
    /// When the locale isn't well formed, as `Intl` throws a `RangeError`.
    pub fn new(locale: Option<&str>, options: &DateTimeFormatOptions) -> Self {
        let mut locale = resolve_locale(locale);
        let mut options = options.clone();
        let styled = options.date_style.is_some() || options.time_style.is_some();
        if styled && options.has_fields() && cfg!(debug_assertions) {
            log::warn!(
                "[ui-sdk] dateStyle and timeStyle can't be used with date or time fields; the fields are ignored."
            );
        }
        if !styled && !options.asks_for_a_field() {
            // Intl's default: the date in numbers. An era alone gets it too.
            options.year = Some(NumericWidth::Numeric);
            options.month = Some(MonthWidth::Numeric);
            options.day = Some(NumericWidth::Numeric);
        }
        // The options win over the locale's own extension keywords.
        let mut set_keyword = |name: &str, value: &Option<SharedString>| {
            if let Some(value) = value
                && let Ok(value) = Value::try_from_str(value)
            {
                let keyword = match name {
                    "ca" => key!("ca"),
                    _ => key!("nu"),
                };
                locale.extensions.unicode.keywords.set(keyword, value);
            }
        };
        set_keyword("ca", &options.calendar);
        set_keyword("nu", &options.numbering_system);
        let calendar = match AnyCalendarKind::try_new(CalendarPreferences::from(&locale)) {
            Ok(AnyCalendarKind::Buddhist) => CalendarKind::Buddhist,
            #[allow(deprecated)]
            Ok(AnyCalendarKind::Japanese | AnyCalendarKind::JapaneseExtended) => CalendarKind::Japanese,
            Ok(AnyCalendarKind::Roc) => CalendarKind::Roc,
            _ => CalendarKind::Gregorian,
        };
        let hour_cycle = match options.hour12 {
            Some(true) => Some(HourCycle::H12),
            Some(false) => Some(HourCycle::H23),
            None => options.hour_cycle.or_else(|| {
                let value = locale.extensions.unicode.keywords.get(&key!("hc"))?.to_string();
                Some(match value.as_str() {
                    "h11" => HourCycle::H11,
                    "h12" => HourCycle::H12,
                    "h23" => HourCycle::H23,
                    "h24" => HourCycle::H24,
                    _ => return None,
                })
            }),
        };
        let mut prefs = DateTimeFormatterPreferences::from(&locale);
        prefs.calendar_algorithm = None;
        if let Some(hour_cycle) = hour_cycle {
            use icu_datetime::preferences::HourCycle as IcuHourCycle;
            prefs.hour_cycle = Some(match hour_cycle {
                HourCycle::H11 => IcuHourCycle::H11,
                HourCycle::H12 => IcuHourCycle::H12,
                // ICU4X has no 1 to 24 clock: midnight's 0 is written as 24 instead.
                HourCycle::H23 | HourCycle::H24 => IcuHourCycle::H23,
            });
        }
        let (engine, wants, lone) = if styled {
            let (date_fields, length) = match options.date_style {
                Some(DateTimeStyle::Full) => (Some(DateFields::YMDE), Length::Long),
                Some(DateTimeStyle::Long) => (Some(DateFields::YMD), Length::Long),
                Some(DateTimeStyle::Medium) => (Some(DateFields::YMD), Length::Medium),
                Some(DateTimeStyle::Short) => (Some(DateFields::YMD), Length::Short),
                None => (
                    None,
                    match options.time_style {
                        Some(DateTimeStyle::Full | DateTimeStyle::Long) => Length::Long,
                        Some(DateTimeStyle::Medium) | None => Length::Medium,
                        Some(DateTimeStyle::Short) => Length::Short,
                    },
                ),
            };
            let time_precision = options.time_style.map(|style| match style {
                DateTimeStyle::Short => TimePrecision::Minute,
                _ => TimePrecision::Second,
            });
            let engine = build_field_set(length, date_fields, time_precision, None)
                .and_then(|field_set| calendar.engine(prefs, field_set));
            (engine, Vec::new(), None)
        } else {
            Self::closest(&options, calendar, prefs)
        };
        let engine = engine
            .or_else(|| {
                let field_set = build_field_set(Length::Medium, Some(DateFields::YMD), None, None)?;
                CalendarKind::Gregorian.engine(prefs, field_set)
            })
            .expect("ICU4X formats a Gregorian date");
        let padding = padding_of(&locale, &options);
        Self {
            locale,
            calendar,
            options,
            hour_cycle,
            wants,
            padding,
            lone,
            engine,
        }
    }

    // The closest of ICU4X's patterns for the fields asked for, by the
    // pattern generator's distance.
    fn closest(
        options: &DateTimeFormatOptions,
        calendar: CalendarKind,
        prefs: DateTimeFormatterPreferences,
    ) -> (Option<AnyEngine>, Vec<Want>, Option<PatternItem>) {
        let wants = wants_of(options);
        let mut date_fields = date_fields_of(options);
        let time_precision = time_precision_of(options);
        if time_precision.is_some() && date_fields.is_some_and(is_calendar_period) {
            // ICU4X can't show a time with a month or year alone.
            date_fields = Some(DateFields::YMD);
        }
        let year_style = year_style_of(options);
        let preferred = preferred_length(options);
        let mut lengths = vec![preferred];
        lengths.extend(
            [Length::Long, Length::Medium, Length::Short]
                .into_iter()
                .filter(|length| *length != preferred),
        );
        let mut best: Option<(u32, AnyEngine, Vec<PatternItem>)> = None;
        for length in lengths {
            let Some(field_set) = build_field_set(length, date_fields, time_precision, year_style) else {
                continue;
            };
            let Some(engine) = calendar.engine(prefs, field_set) else {
                continue;
            };
            let items = each_engine!(&engine, engine => engine.pattern(&engine.input(SAMPLE)));
            let score = distance(&items, &wants);
            if best.as_ref().is_none_or(|(best, _, _)| score < *best) {
                let exact = score == 0;
                best = Some((score, engine, items));
                if exact {
                    break;
                }
            }
        }
        let Some((score, engine, items)) = best else {
            return (None, wants, None);
        };
        // ICU's pattern generator writes any single field alone, so one asked
        // for alone never carries others. A number where a name was asked for
        // stays, as the generator keeps the locale's pattern.
        let lone = match wants.as_slice() {
            [want] if score >= 10_000 => Some(lone_field(*want, &items)),
            _ => None,
        };
        (Some(engine), wants, lone)
    }

    // The pattern a moment formats with, stretched to the widths asked for,
    // or `None` when ICU4X's own is already it.
    fn stretched(&self, value: DateTimeValue) -> Option<Vec<PatternItem>> {
        if let Some(lone) = self.lone {
            return Some(vec![lone]);
        }
        if self.wants.is_empty() {
            return None;
        }
        let items = each_engine!(&self.engine, engine => engine.pattern(&engine.input(value)));
        if items.iter().any(|item| {
            matches!(
                item,
                PatternItem::Field(Field {
                    symbol: FieldSymbol::DecimalSecond(_),
                    ..
                })
            )
        }) {
            return None;
        }
        let adjusted = adjust(&items, &self.wants, self.padding);
        (adjusted != items).then_some(adjusted)
    }

    fn collect(&self, value: DateTimeValue) -> Collector {
        let stretched = self.stretched(value);
        let mut collector = Collector::default();
        each_engine!(&self.engine, engine => engine.write(value, stretched.as_deref(), &mut collector));
        collector
    }

    // Midnight written as 24, on a 1 to 24 clock.
    fn fix_hours(&self, pieces: &mut [Piece], hour_of: impl Fn(Side) -> u8) {
        if self.hour_cycle != Some(HourCycle::H24) {
            return;
        }
        let digits: Vec<char> = native_digits(&self.locale).chars().collect();
        // The digits run from nine down to zero.
        let digit = |n: usize| digits.get(9 - n).copied().unwrap_or((b'0' + n as u8) as char);
        for piece in pieces {
            if piece.r#type == DateTimeFormatPartType::Hour && hour_of(piece.side) == 0 {
                piece.value = [digit(2), digit(4)].iter().collect();
            }
        }
    }

    /// The moment as people read it in the locale: `format`.
    pub fn format(&self, value: impl Into<DateTimeValue>) -> String {
        self.format_to_parts(value)
            .into_iter()
            .map(|part| part.value)
            .collect()
    }

    /// The pieces the moment formats into: `formatToParts`.
    pub fn format_to_parts(&self, value: impl Into<DateTimeValue>) -> Vec<DateTimeFormatPart> {
        let value = value.into();
        let mut pieces = pieces(&self.collect(value));
        let hour = civil(value).hour;
        self.fix_hours(&mut pieces, |_| hour);
        pieces
            .into_iter()
            .map(|piece| DateTimeFormatPart {
                r#type: piece.r#type,
                value: piece.value,
            })
            .collect()
    }

    /// Two moments as a range, like "Oct 16 – 20, 2026": `formatRange`. When
    /// the fields it shows don't differ, it's one moment.
    pub fn format_range(&self, start: impl Into<DateTimeValue>, end: impl Into<DateTimeValue>) -> String {
        let (start, end) = (start.into(), end.into());
        let mut collector = Collector::default();
        let ranged = self.lone.is_none()
            && each_engine!(&self.engine, engine => engine.write_range(start, end, &mut collector));
        if !ranged {
            // The fields alone can't make a range pattern: both moments, apart.
            return format!("{} – {}", self.format(start), self.format(end));
        }
        let mut pieces = pieces(&collector);
        // Stretch the fields as a single moment's are.
        if let Some(adjusted) = self.stretched(start) {
            let own = each_engine!(&self.engine, engine => engine.pattern(&engine.input(start)));
            let changes: Vec<(Kind, Field)> = own
                .iter()
                .zip(&adjusted)
                .filter_map(|(own, adjusted)| match (own, adjusted) {
                    (PatternItem::Field(own), PatternItem::Field(adjusted)) if own != adjusted => {
                        Some((kind_of(adjusted.symbol)?, *adjusted))
                    }
                    _ => None,
                })
                .collect();
            for piece in &mut pieces {
                let kind = match piece.r#type {
                    DateTimeFormatPartType::Era => Kind::Era,
                    DateTimeFormatPartType::Year => Kind::Year,
                    DateTimeFormatPartType::Month => Kind::Month,
                    DateTimeFormatPartType::Day => Kind::Day,
                    DateTimeFormatPartType::Weekday => Kind::Weekday,
                    DateTimeFormatPartType::Hour => Kind::Hour,
                    _ => continue,
                };
                let Some((_, field)) = changes.iter().find(|(changed, _)| *changed == kind) else {
                    continue;
                };
                let value = if piece.side == Side::End { end } else { start };
                if let Some(text) = each_engine!(&self.engine, engine => engine.write_field(*field, value)) {
                    piece.value = text;
                }
            }
        }
        let (start_hour, end_hour) = (civil(start).hour, civil(end).hour);
        self.fix_hours(&mut pieces, |side| if side == Side::End { end_hour } else { start_hour });
        pieces.into_iter().map(|piece| piece.value).collect()
    }

    /// What the formatter settled on: `resolvedOptions`.
    pub fn resolved_options(&self) -> ResolvedDateTimeFormatOptions {
        let items = self
            .stretched(SAMPLE)
            .unwrap_or_else(|| each_engine!(&self.engine, engine => engine.pattern(&engine.input(SAMPLE))));
        let hour_cycle = items.iter().find_map(|item| match item {
            PatternItem::Field(Field {
                symbol: FieldSymbol::Hour(hour),
                ..
            }) => Some(match hour {
                fields::Hour::H11 => HourCycle::H11,
                fields::Hour::H12 => HourCycle::H12,
                fields::Hour::H23 if self.hour_cycle == Some(HourCycle::H24) => HourCycle::H24,
                fields::Hour::H23 => HourCycle::H23,
            }),
            _ => None,
        });
        ResolvedDateTimeFormatOptions {
            locale: self.locale.to_string().into(),
            calendar: self.calendar.name().into(),
            numbering_system: numbering_system(&self.locale).into(),
            time_zone: "UTC".into(),
            hour_cycle,
            hour12: hour_cycle.map(|cycle| matches!(cycle, HourCycle::H11 | HourCycle::H12)),
            date_style: self.options.date_style,
            time_style: self.options.time_style,
        }
    }

    /// The locale it formats in.
    pub fn locale(&self) -> &Locale {
        &self.locale
    }
}

// The numbering system a locale writes numbers in: its `-u-nu-` when there
// are digits for it, else its own.
fn numbering_system(locale: &Locale) -> String {
    use icu_decimal::provider::{DecimalDigitsV1, DecimalSymbolsV1};
    use icu_provider::prelude::*;
    let provider = &icu_decimal::provider::Baked;
    if let Some(value) = locale.extensions.unicode.keywords.get(&key!("nu")) {
        let name = value.to_string();
        if let Ok(attributes) = DataMarkerAttributes::try_from_str(&name) {
            let request = DataRequest {
                id: DataIdentifierBorrowed::for_marker_attributes(attributes),
                ..Default::default()
            };
            if DataProvider::<DecimalDigitsV1>::load(provider, request).is_ok() {
                return name;
            }
        }
    }
    let data_locale = DataLocale::from(&LanguageIdentifier::from((
        locale.id.language,
        locale.id.script,
        locale.id.region,
    )));
    let request = DataRequest {
        id: DataIdentifierBorrowed::for_locale(&data_locale),
        ..Default::default()
    };
    DataProvider::<DecimalSymbolsV1>::load(provider, request)
        .map(|response| response.payload.get().numsys().to_string())
        .unwrap_or_else(|_| "latn".to_string())
}
