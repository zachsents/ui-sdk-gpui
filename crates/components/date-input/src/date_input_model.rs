//! Port of `components/date-input/date-input-model.ts`: dates and times as
//! segments: which a locale shows, in what order, and what typing, arrow keys
//! and clearing do to them. Values are the strings native inputs submit:
//! `YYYY-MM-DD`, `YYYY-MM-DDTHH:mm[:ss]` and `HH:mm[:ss]`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::SharedString;
use ui_sdk_date::{
    DateTimeField, DateTimeFormatOptions, DateTimeFormatPartType, DateTimeValue, MonthWidth,
    NumericWidth, date_formatter, date_time_field_name, days_in_month, iso_date,
};
use ui_sdk_text::is_js_space;

/// What a segment holds. A `Literal` is the text between segments, like a
/// slash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateInputSegmentType {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    DayPeriod,
    Literal,
}

impl DateInputSegmentType {
    /// The type's name, as the original writes it, like `"dayPeriod"`.
    pub fn as_str(self) -> &'static str {
        match self {
            DateInputSegmentType::Literal => "literal",
            other => other.editable().map_or("literal", EditableType::as_str),
        }
    }

    /// The type a person types in, or `None` for the text between segments.
    pub fn editable(self) -> Option<EditableType> {
        Some(match self {
            DateInputSegmentType::Year => EditableType::Year,
            DateInputSegmentType::Month => EditableType::Month,
            DateInputSegmentType::Day => EditableType::Day,
            DateInputSegmentType::Hour => EditableType::Hour,
            DateInputSegmentType::Minute => EditableType::Minute,
            DateInputSegmentType::Second => EditableType::Second,
            DateInputSegmentType::DayPeriod => EditableType::DayPeriod,
            DateInputSegmentType::Literal => return None,
        })
    }
}

/// A segment a person types in: every type but `Literal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditableType {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    DayPeriod,
}

impl EditableType {
    /// Every editable type.
    pub const ALL: [EditableType; 7] = [
        EditableType::Year,
        EditableType::Month,
        EditableType::Day,
        EditableType::Hour,
        EditableType::Minute,
        EditableType::Second,
        EditableType::DayPeriod,
    ];

    /// The type's name, as the original writes it, like `"month"`.
    pub fn as_str(self) -> &'static str {
        match self {
            EditableType::Year => "year",
            EditableType::Month => "month",
            EditableType::Day => "day",
            EditableType::Hour => "hour",
            EditableType::Minute => "minute",
            EditableType::Second => "second",
            EditableType::DayPeriod => "dayPeriod",
        }
    }

    /// The type a name names, like `"month"`.
    pub fn from_name(name: &str) -> Option<Self> {
        EditableType::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    fn field(self) -> DateTimeField {
        match self {
            EditableType::Year => DateTimeField::Year,
            EditableType::Month => DateTimeField::Month,
            EditableType::Day => DateTimeField::Day,
            EditableType::Hour => DateTimeField::Hour,
            EditableType::Minute => DateTimeField::Minute,
            EditableType::Second => DateTimeField::Second,
            EditableType::DayPeriod => DateTimeField::DayPeriod,
        }
    }
}

impl From<EditableType> for DateInputSegmentType {
    fn from(kind: EditableType) -> Self {
        match kind {
            EditableType::Year => DateInputSegmentType::Year,
            EditableType::Month => DateInputSegmentType::Month,
            EditableType::Day => DateInputSegmentType::Day,
            EditableType::Hour => DateInputSegmentType::Hour,
            EditableType::Minute => DateInputSegmentType::Minute,
            EditableType::Second => DateInputSegmentType::Second,
            EditableType::DayPeriod => DateInputSegmentType::DayPeriod,
        }
    }
}

/// The smallest unit an input shows. `Day` shows a date alone, and a time
/// unit adds the time down to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DateInputGranularity {
    #[default]
    Day,
    Hour,
    Minute,
    Second,
}

impl DateInputGranularity {
    pub fn as_str(self) -> &'static str {
        match self {
            DateInputGranularity::Day => "day",
            DateInputGranularity::Hour => "hour",
            DateInputGranularity::Minute => "minute",
            DateInputGranularity::Second => "second",
        }
    }
}

/// A 12 or 24 hour clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HourCycle {
    H12,
    H24,
}

/// One segment, as the root's state lists them in the order the locale shows
/// them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateInputSegmentData {
    /// Tells segments apart: the type, or `"literal-<n>"` for the text
    /// between them.
    pub key: SharedString,
    pub r#type: DateInputSegmentType,
    /// What it shows: the number, padded, or AM and PM in the locale. Empty
    /// while it holds nothing.
    pub text: SharedString,
    /// The number it holds, or `None` while empty. AM is 0 and PM is 1.
    pub value: Option<i32>,
    /// The smallest number it takes.
    pub min: i32,
    /// The largest number it takes, like the days in the month.
    pub max: i32,
    /// Whether it holds nothing yet.
    pub empty: bool,
    /// Whether it's a segment you type in, rather than the text between
    /// segments.
    pub editable: bool,
    /// Its name in the locale, like "month", which names it to assistive
    /// technology.
    pub label: SharedString,
}

/// Whether an input holds a date, or a time alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Date,
    Time,
}

/// What each segment holds, `None` while empty. AM is 0 and PM is 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Fields {
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub hour: Option<i32>,
    pub minute: Option<i32>,
    pub second: Option<i32>,
    pub day_period: Option<i32>,
}

impl Fields {
    pub fn get(&self, kind: EditableType) -> Option<i32> {
        match kind {
            EditableType::Year => self.year,
            EditableType::Month => self.month,
            EditableType::Day => self.day,
            EditableType::Hour => self.hour,
            EditableType::Minute => self.minute,
            EditableType::Second => self.second,
            EditableType::DayPeriod => self.day_period,
        }
    }

    pub fn set(&mut self, kind: EditableType, value: Option<i32>) {
        let field = match kind {
            EditableType::Year => &mut self.year,
            EditableType::Month => &mut self.month,
            EditableType::Day => &mut self.day,
            EditableType::Hour => &mut self.hour,
            EditableType::Minute => &mut self.minute,
            EditableType::Second => &mut self.second,
            EditableType::DayPeriod => &mut self.day_period,
        };
        *field = value;
    }

    /// The fields with `kind` set to `value` (`{ ...fields, [kind]: value }`).
    pub fn with(mut self, kind: EditableType, value: Option<i32>) -> Self {
        self.set(kind, value);
        self
    }
}

/// Every field empty.
pub fn empty_fields() -> Fields {
    Fields::default()
}

/// The digits typed into a segment that may take another.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Buffer {
    pub r#type: EditableType,
    pub text: String,
}

/// What a value is read and written in: the kind, the smallest unit and the
/// clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ValueFormat {
    pub kind: Kind,
    pub granularity: DateInputGranularity,
    pub hour_cycle: HourCycle,
}

/// A value's format and the language its segments show in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Format {
    pub kind: Kind,
    pub granularity: DateInputGranularity,
    pub hour_cycle: HourCycle,
    pub locale: SharedString,
}

impl Format {
    pub fn value_format(&self) -> ValueFormat {
        ValueFormat {
            kind: self.kind,
            granularity: self.granularity,
            hour_cycle: self.hour_cycle,
        }
    }
}

fn pad(value: i32, length: usize) -> String {
    format!("{value:0length$}")
}

// The marks that keep right-to-left text in order, which aren't the text.
fn is_bidi_mark(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}')
}

/// The types a format shows, in no order. A time input always shows
/// minutes.
pub fn types_of(format: ValueFormat) -> Vec<EditableType> {
    let unit = if format.kind == Kind::Time && format.granularity == DateInputGranularity::Day {
        DateInputGranularity::Minute
    } else {
        format.granularity
    };
    let mut types = if format.kind == Kind::Date {
        vec![EditableType::Year, EditableType::Month, EditableType::Day]
    } else {
        Vec::new()
    };
    if unit == DateInputGranularity::Day {
        return types;
    }
    types.push(EditableType::Hour);
    if unit != DateInputGranularity::Hour {
        types.push(EditableType::Minute);
    }
    if unit == DateInputGranularity::Second {
        types.push(EditableType::Second);
    }
    if format.hour_cycle == HourCycle::H12 {
        types.push(EditableType::DayPeriod);
    }
    types
}

thread_local! {
    static HOUR_CYCLES: RefCell<HashMap<SharedString, HourCycle>> = RefCell::default();
    static LAYOUTS: RefCell<HashMap<Format, Rc<Vec<LayoutItem>>>> = RefCell::default();
    static PERIOD_NAMES: RefCell<HashMap<SharedString, Rc<[String; 2]>>> = RefCell::default();
    static FIELD_NAMES: RefCell<HashMap<(SharedString, EditableType), SharedString>> = RefCell::default();
}

/// The clock a locale shows hours on.
pub fn hour_cycle_of(locale: &str) -> HourCycle {
    let key = SharedString::from(locale.to_string());
    if let Some(cycle) = HOUR_CYCLES.with(|cycles| cycles.borrow().get(&key).copied()) {
        return cycle;
    }
    let resolved = date_formatter(Some(locale), &DateTimeFormatOptions::default().hour(NumericWidth::Numeric))
        .resolved_options();
    let cycle = match resolved.hour_cycle {
        Some(ui_sdk_date::HourCycle::H11 | ui_sdk_date::HourCycle::H12) => HourCycle::H12,
        _ => HourCycle::H24,
    };
    HOUR_CYCLES.with(|cycles| cycles.borrow_mut().insert(key, cycle));
    cycle
}

// A segment or the text between two, in the order a locale shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LayoutItem {
    pub(crate) r#type: DateInputSegmentType,
    pub(crate) text: String,
}

fn part_type(r#type: DateTimeFormatPartType) -> Option<EditableType> {
    Some(match r#type {
        DateTimeFormatPartType::Year => EditableType::Year,
        DateTimeFormatPartType::Month => EditableType::Month,
        DateTimeFormatPartType::Day => EditableType::Day,
        DateTimeFormatPartType::Hour => EditableType::Hour,
        DateTimeFormatPartType::Minute => EditableType::Minute,
        DateTimeFormatPartType::Second => EditableType::Second,
        DateTimeFormatPartType::DayPeriod => EditableType::DayPeriod,
        _ => return None,
    })
}

pub(crate) fn layout_of(format: &Format) -> Rc<Vec<LayoutItem>> {
    if let Some(layout) = LAYOUTS.with(|layouts| layouts.borrow().get(format).cloned()) {
        return layout;
    }
    let types = types_of(format.value_format());
    let has = |kind: EditableType| types.contains(&kind);
    let mut options = DateTimeFormatOptions::default()
        .calendar("gregory")
        .numbering_system("latn");
    if has(EditableType::Year) {
        options = options
            .year(NumericWidth::Numeric)
            .month(MonthWidth::TwoDigit)
            .day(NumericWidth::TwoDigit);
    }
    if has(EditableType::Hour) {
        options = options.hour(NumericWidth::TwoDigit).hour_cycle(match format.hour_cycle {
            HourCycle::H12 => ui_sdk_date::HourCycle::H12,
            HourCycle::H24 => ui_sdk_date::HourCycle::H23,
        });
    }
    if has(EditableType::Minute) {
        options = options.minute(NumericWidth::TwoDigit);
    }
    if has(EditableType::Second) {
        options = options.second(NumericWidth::TwoDigit);
    }
    let formatter = date_formatter(Some(&format.locale), &options);
    let mut layout: Vec<LayoutItem> = Vec::new();
    for part in formatter.format_to_parts(DateTimeValue::new(2001, 2, 3).at(16, 5, 6)) {
        if part.r#type == DateTimeFormatPartType::Literal {
            // Engines differ on the space before AM or PM, so every space is a plain one.
            let text: String = part
                .value
                .chars()
                .filter(|c| !is_bidi_mark(*c))
                .map(|c| if is_js_space(c) { ' ' } else { c })
                .collect();
            match layout.last_mut() {
                Some(last) if last.r#type == DateInputSegmentType::Literal => last.text.push_str(&text),
                _ if !text.is_empty() => layout.push(LayoutItem {
                    r#type: DateInputSegmentType::Literal,
                    text,
                }),
                _ => {}
            }
        } else if let Some(kind) = part_type(part.r#type).filter(|kind| has(*kind)) {
            layout.push(LayoutItem {
                r#type: kind.into(),
                text: String::new(),
            });
        }
    }
    while layout.first().is_some_and(|item| item.r#type == DateInputSegmentType::Literal) {
        layout.remove(0);
    }
    while layout.last().is_some_and(|item| item.r#type == DateInputSegmentType::Literal) {
        layout.pop();
    }
    let layout = Rc::new(layout);
    LAYOUTS.with(|layouts| layouts.borrow_mut().insert(format.clone(), layout.clone()));
    layout
}

/// The names of AM and PM in a locale.
pub fn day_periods_of(locale: &str) -> Rc<[String; 2]> {
    let key = SharedString::from(locale.to_string());
    if let Some(names) = PERIOD_NAMES.with(|names| names.borrow().get(&key).cloned()) {
        return names;
    }
    let formatter = date_formatter(
        Some(locale),
        &DateTimeFormatOptions::default()
            .hour(NumericWidth::Numeric)
            .hour_cycle(ui_sdk_date::HourCycle::H12),
    );
    let name = |hour: i32| {
        formatter
            .format_to_parts(DateTimeValue::new(2001, 1, 1).at(hour, 0, 0))
            .into_iter()
            .find(|part| part.r#type == DateTimeFormatPartType::DayPeriod)
            .map(|part| part.value)
    };
    let names = Rc::new([
        name(0).unwrap_or_else(|| "AM".to_string()),
        name(12).unwrap_or_else(|| "PM".to_string()),
    ]);
    PERIOD_NAMES.with(|periods| periods.borrow_mut().insert(key, names.clone()));
    names
}

const ENGLISH_NAMES: [(EditableType, &str); 7] = [
    (EditableType::Year, "year"),
    (EditableType::Month, "month"),
    (EditableType::Day, "day"),
    (EditableType::Hour, "hour"),
    (EditableType::Minute, "minute"),
    (EditableType::Second, "second"),
    (EditableType::DayPeriod, "AM/PM"),
];

fn english_name(kind: EditableType) -> SharedString {
    let name = ENGLISH_NAMES
        .iter()
        .find(|(each, _)| *each == kind)
        .map_or("", |(_, name)| *name);
    SharedString::new_static(name)
}

/// A segment's name in a locale, like "month", or English where the locale
/// isn't one.
pub fn label_of(kind: EditableType, locale: &str) -> SharedString {
    let key = (SharedString::from(locale.to_string()), kind);
    if let Some(name) = FIELD_NAMES.with(|names| names.borrow().get(&key).cloned()) {
        return name;
    }
    let name = match ui_sdk_text::to_locale(Some(locale)) {
        Some(tag) => date_time_field_name(Some(&tag), kind.field()),
        None => english_name(kind),
    };
    FIELD_NAMES.with(|names| names.borrow_mut().insert(key, name.clone()));
    name
}

/// The numbers a segment takes. A day takes what its month has, or 31
/// before the month is known, and February 29 before the year is.
pub fn bounds_of(kind: EditableType, fields: &Fields, hour_cycle: HourCycle) -> (i32, i32) {
    match kind {
        EditableType::Year => (1, 9999),
        EditableType::Month => (1, 12),
        EditableType::Day => (
            1,
            match fields.month {
                None => 31,
                Some(month) => days_in_month(fields.year.unwrap_or(2000), month),
            },
        ),
        EditableType::Hour => match hour_cycle {
            HourCycle::H12 => (1, 12),
            HourCycle::H24 => (0, 23),
        },
        EditableType::DayPeriod => (0, 1),
        EditableType::Minute | EditableType::Second => (0, 59),
    }
}

fn digits_of(kind: EditableType) -> usize {
    if kind == EditableType::Year { 4 } else { 2 }
}

/// A day past its month's end becomes the month's last day, as when a month
/// with fewer days is picked.
pub fn clamp_day(fields: Fields) -> Fields {
    let (Some(day), Some(month)) = (fields.day, fields.month) else {
        return fields;
    };
    let last = days_in_month(fields.year.unwrap_or(2000), month);
    if day > last { fields.with(EditableType::Day, Some(last)) } else { fields }
}

fn text_of(kind: EditableType, value: Option<i32>, locale: &str) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if kind == EditableType::DayPeriod {
        let names = day_periods_of(locale);
        return names.get(value as usize).cloned().unwrap_or_default();
    }
    pad(value, digits_of(kind))
}

/// The segments an input shows for its fields, in the locale's order.
/// What's being typed shows as typed, unpadded, until the segment is done.
pub fn segments_of(format: &Format, fields: &Fields, buffer: Option<&Buffer>) -> Vec<DateInputSegmentData> {
    let mut literals = 0;
    layout_of(format)
        .iter()
        .map(|item| {
            let Some(kind) = item.r#type.editable() else {
                let key = format!("literal-{literals}");
                literals += 1;
                return DateInputSegmentData {
                    key: key.into(),
                    r#type: DateInputSegmentType::Literal,
                    text: item.text.clone().into(),
                    value: None,
                    min: 0,
                    max: 0,
                    empty: false,
                    editable: false,
                    label: SharedString::default(),
                };
            };
            let value = fields.get(kind);
            let (min, max) = bounds_of(kind, fields, format.hour_cycle);
            let typing = buffer.filter(|buffer| buffer.r#type == kind).map(|buffer| buffer.text.clone());
            DateInputSegmentData {
                key: SharedString::new_static(kind.as_str()),
                r#type: kind.into(),
                empty: value.is_none() && typing.as_ref().is_none_or(String::is_empty),
                text: typing.unwrap_or_else(|| text_of(kind, value, &format.locale)).into(),
                value,
                min,
                max,
                editable: true,
                label: label_of(kind, &format.locale),
            }
        })
        .collect()
}

/// What typing a digit leaves: the fields, what's still being typed, and
/// whether the segment is done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Typed {
    pub fields: Fields,
    pub buffer: Option<Buffer>,
    pub done: bool,
}

fn number(text: &str) -> i64 {
    text.parse::<i64>().unwrap_or(0)
}

/// A digit typed into a segment. It adds to what's being typed while the
/// number still fits, else starts over, and the segment is done once no
/// other digit could follow, like "4" for a month or "12" for an hour.
pub fn type_digit(
    fields: &Fields,
    buffer: Option<&Buffer>,
    kind: EditableType,
    digit: &str,
    hour_cycle: HourCycle,
) -> Typed {
    let (min, max) = bounds_of(kind, fields, hour_cycle);
    let digits = digits_of(kind);
    let mut text = match buffer.filter(|buffer| buffer.r#type == kind) {
        Some(buffer) => format!("{}{digit}", buffer.text),
        None => digit.to_string(),
    };
    if text.len() > digits || number(&text) > i64::from(max) {
        text = digit.to_string();
    }
    let value = number(&text);
    let done = text.len() >= digits || value * 10 > i64::from(max);
    let value = (value >= i64::from(min) && value <= i64::from(max)).then_some(value as i32);
    let next = fields.with(kind, value);
    Typed {
        // 2, 20 and 202 aren't leap years, so a year still being typed keeps February 29.
        fields: if kind == EditableType::Year && !done { next } else { clamp_day(next) },
        buffer: (!done).then_some(Buffer { r#type: kind, text }),
        done,
    }
}

/// A letter typed into AM/PM: the period whose name starts with it, in the
/// locale or in English.
pub fn type_period(fields: &Fields, letter: &str, locale: &str) -> Option<Fields> {
    let lower = letter.to_lowercase();
    let names = day_periods_of(locale);
    let index = names.iter().enumerate().position(|(at, name)| {
        [name.as_str(), if at == 1 { "pm" } else { "am" }]
            .iter()
            .any(|candidate| candidate.to_lowercase().starts_with(&lower))
    })?;
    Some(fields.with(EditableType::DayPeriod, Some(index as i32)))
}

/// Steps a segment by `delta`, wrapping at its ends without touching the
/// others: a minute past 59 is 0, and the hour stays. An empty segment
/// starts at `start`.
pub fn step_field(fields: &Fields, kind: EditableType, delta: i32, hour_cycle: HourCycle, start: i32) -> Fields {
    let (min, max) = bounds_of(kind, fields, hour_cycle);
    let span = max - min + 1;
    let value = match fields.get(kind) {
        None => start,
        Some(current) => min + (current - min + delta).rem_euclid(span),
    };
    clamp_day(fields.with(kind, Some(value)))
}

/// Which end [`step_to_end`] goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Min,
    Max,
}

/// Sets a segment to its smallest or largest number.
pub fn step_to_end(fields: &Fields, kind: EditableType, end: End, hour_cycle: HourCycle) -> Fields {
    let (min, max) = bounds_of(kind, fields, hour_cycle);
    clamp_day(fields.with(kind, Some(if end == End::Min { min } else { max })))
}

/// The step Page Up and Page Down take in a segment.
pub fn page_step_of(kind: EditableType) -> i32 {
    match kind {
        EditableType::Year => 5,
        EditableType::Month => 3,
        EditableType::Day => 7,
        EditableType::Hour => 3,
        EditableType::Minute | EditableType::Second => 15,
        EditableType::DayPeriod => 1,
    }
}

/// The value the fields make, or `None` while any segment shown is empty.
pub fn value_of(fields: &Fields, format: ValueFormat) -> Option<SharedString> {
    let types = types_of(format);
    if types.iter().any(|kind| fields.get(*kind).is_none()) {
        return None;
    }
    let mut date = String::new();
    if format.kind == Kind::Date {
        date = iso_date(fields.year?, fields.month?, fields.day?)?.to_string();
        if !types.contains(&EditableType::Hour) {
            return Some(date.into());
        }
    }
    let hour = fields.hour?;
    let hour = match format.hour_cycle {
        HourCycle::H12 => hour % 12 + if fields.day_period == Some(1) { 12 } else { 0 },
        HourCycle::H24 => hour,
    };
    let mut time = format!("{}:{}", pad(hour, 2), pad(fields.minute.unwrap_or(0), 2));
    if types.contains(&EditableType::Second) {
        time.push(':');
        time.push_str(&pad(fields.second.unwrap_or(0), 2));
    }
    Some(if format.kind == Kind::Date { format!("{date}T{time}") } else { time }.into())
}

// What a value holds: its date, and its time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Parts {
    date: Option<(i32, i32, i32)>,
    hour: Option<i32>,
    minute: Option<i32>,
    second: Option<i32>,
}

// `/^(\d{4})-(\d{2})-(\d{2})$/`.
fn date_pattern(text: &str) -> Option<(i32, i32, i32)> {
    let bytes = text.as_bytes();
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
    Some((
        text[0..4].parse().ok()?,
        text[5..7].parse().ok()?,
        text[8..10].parse().ok()?,
    ))
}

// `/^(\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?$/`.
fn time_pattern(text: &str) -> Option<(i32, i32, Option<i32>)> {
    let digits = |part: &str| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit());
    let bytes = text.as_bytes();
    if bytes.len() < 5 || bytes[2] != b':' || !digits(&text[0..2]) || !digits(&text[3..5]) {
        return None;
    }
    let (hour, minute) = (text[0..2].parse().ok()?, text[3..5].parse().ok()?);
    let rest = &text[5..];
    if rest.is_empty() {
        return Some((hour, minute, None));
    }
    let rest = rest.strip_prefix(':')?;
    if rest.len() < 2 || !digits(&rest[0..2]) {
        return None;
    }
    let second = rest[0..2].parse().ok()?;
    let fraction = &rest[2..];
    if !fraction.is_empty() {
        let fraction = fraction.strip_prefix('.')?;
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    }
    Some((hour, minute, Some(second)))
}

fn parts_of(value: Option<&str>) -> Option<Parts> {
    let value = value?;
    let (date_part, time_part) = if value.contains('T') {
        let mut split = value.split('T');
        (split.next().unwrap_or(""), split.next().unwrap_or(""))
    } else if time_pattern(value).is_some() {
        ("", value)
    } else {
        (value, "")
    };
    let mut parts = Parts::default();
    if !date_part.is_empty() {
        let (year, month, day) = date_pattern(date_part)?;
        iso_date(year, month, day)?;
        parts.date = Some((year, month, day));
    }
    if !time_part.is_empty() {
        let (hour, minute, second) = time_pattern(time_part)?;
        let second = second.unwrap_or(0);
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        parts.hour = Some(hour);
        parts.minute = Some(minute);
        parts.second = Some(second);
    }
    (parts.date.is_some() || parts.hour.is_some()).then_some(parts)
}

/// The fields an ISO value fills, for the segments an input shows.
pub fn fields_of(value: Option<&str>, format: ValueFormat) -> Fields {
    let mut fields = empty_fields();
    let Some(mut parts) = parts_of(value) else {
        return fields;
    };
    let types = types_of(format);
    if format.kind == Kind::Date {
        let Some((year, month, day)) = parts.date else {
            return fields;
        };
        fields.year = Some(year);
        fields.month = Some(month);
        fields.day = Some(day);
    }
    if types.contains(&EditableType::Hour) {
        if parts.hour.is_none() {
            if format.kind == Kind::Time {
                return fields;
            }
            parts.hour = Some(0);
            parts.minute = Some(0);
            parts.second = Some(0);
        }
        let hour = parts.hour.unwrap_or(0);
        fields.hour = Some(match format.hour_cycle {
            HourCycle::H12 if hour % 12 == 0 => 12,
            HourCycle::H12 => hour % 12,
            HourCycle::H24 => hour,
        });
        if format.hour_cycle == HourCycle::H12 {
            fields.day_period = Some(if hour >= 12 { 1 } else { 0 });
        }
        if types.contains(&EditableType::Minute) {
            fields.minute = parts.minute;
        }
        if types.contains(&EditableType::Second) {
            fields.second = parts.second;
        }
    }
    fields
}

/// A value in the input's own shape: a date and a time cut to its smallest
/// unit, or `None` when it isn't one it can show.
pub fn normalize_value(value: Option<&str>, format: ValueFormat) -> Option<SharedString> {
    value_of(&fields_of(value, format), format)
}

// Sorts as text: `YYYY-MM-DDTHH:mm:ss`, or `HH:mm:ss` for a time.
fn sort_key(parts: &Parts) -> String {
    let time = match parts.hour {
        None => String::new(),
        Some(hour) => format!(
            "{}:{}:{}",
            pad(hour, 2),
            pad(parts.minute.unwrap_or(0), 2),
            pad(parts.second.unwrap_or(0), 2)
        ),
    };
    let Some((year, month, day)) = parts.date else {
        return time;
    };
    let date = format!("{}-{}-{}", pad(year, 4), pad(month, 2), pad(day, 2));
    if time.is_empty() { date } else { format!("{date}T{time}") }
}

/// How a value compares with a bound: less before it, greater after it,
/// equal on it. A bound with a date alone compares dates, so a `max` of
/// `"2026-12-31"` takes any time that day.
pub fn compare_to(value: &str, bound: &str) -> std::cmp::Ordering {
    let (Some(a), Some(b)) = (parts_of(Some(value)), parts_of(Some(bound))) else {
        return std::cmp::Ordering::Equal;
    };
    let mut left = sort_key(&a);
    let mut right = sort_key(&b);
    if a.date.is_some() && b.date.is_some() && (b.hour.is_none() || a.hour.is_none()) {
        left.truncate(10);
        right.truncate(10);
    }
    left.cmp(&right)
}

/// The date part of a value, for matching unavailable dates.
pub fn date_of(value: &str) -> Option<SharedString> {
    let head = value.get(..10)?;
    date_pattern(head).map(|_| SharedString::from(head.to_string()))
}
