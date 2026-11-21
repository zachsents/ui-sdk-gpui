//! Port of `components/calendar/calendar-model.ts`: the calendar's decisions
//! as pure functions of plain values: which dates a matcher names, what a key
//! press moves to, what a press selects, and which cells a month's grid
//! shows. The component calls them.

use std::fmt;
use std::rc::Rc;

use gpui::SharedString;
use ui_sdk_date::{
    ISODate, MonthGridOptions, add_days, add_months, add_years, clamp_date, days_between,
    end_of_week, is_iso_date, month_grid, start_of_week, weekday,
};
use ui_sdk_kernel::watchers::register_data;
use ui_sdk_kernel::{AgentValue, Key};

/// A month as `YYYY-MM`, like `"2026-10"`, which is what
/// `<input type="month">` submits.
pub type ISOMonth = SharedString;

/// How many dates a calendar selects: one, any number, or a range from a
/// start to an end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CalendarMode {
    #[default]
    Single,
    Multiple,
    Range,
}

impl CalendarMode {
    /// The mode's name, as the original writes it, like `"range"`.
    pub fn as_str(self) -> &'static str {
        match self {
            CalendarMode::Single => "single",
            CalendarMode::Multiple => "multiple",
            CalendarMode::Range => "range",
        }
    }
}

/// A calendar's value for each mode: an ISO date, a sorted list of them, or
/// a `[start, end]` range. `Empty` when nothing is selected, the original's
/// `null`; a `"multiple"` calendar holds an empty list instead.
///
/// `observe` and history read it as the original's value
/// ([`CalendarValue::to_data`]): `null`, a date, or a list of dates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum CalendarValue {
    #[default]
    Empty,
    Single(ISODate),
    Multiple(Vec<ISODate>),
    Range(ISODate, ISODate),
}

impl CalendarValue {
    /// The original's value: `null`, an ISO date, or a list of them, for a
    /// range its start and end.
    pub fn to_data(&self) -> AgentValue {
        let date = |date: &ISODate| AgentValue::from(date.as_str());
        match self {
            CalendarValue::Empty => AgentValue::Null,
            CalendarValue::Single(single) => date(single),
            CalendarValue::Multiple(dates) => AgentValue::Array(dates.iter().map(date).collect()),
            CalendarValue::Range(start, end) => AgentValue::Array(vec![date(start), date(end)]),
        }
    }

    /// Lets `observe` and history read calendar values as data. Calendars and
    /// date pickers call it as they're made.
    pub fn register_data() {
        register_data::<CalendarValue>(CalendarValue::to_data);
    }

    /// One date, for `"single"`.
    pub fn single(date: impl Into<ISODate>) -> Self {
        CalendarValue::Single(date.into())
    }

    /// Any number of dates, for `"multiple"`.
    pub fn multiple(dates: impl IntoIterator<Item = impl Into<ISODate>>) -> Self {
        CalendarValue::Multiple(dates.into_iter().map(Into::into).collect())
    }

    /// A range from `start` to `end`, for `"range"`.
    pub fn range(start: impl Into<ISODate>, end: impl Into<ISODate>) -> Self {
        CalendarValue::Range(start.into(), end.into())
    }

    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        match self {
            CalendarValue::Empty => true,
            CalendarValue::Multiple(dates) => dates.is_empty(),
            _ => false,
        }
    }

    /// Every date the value holds, as given: what `[value].flat()` reads.
    pub fn entries(&self) -> Vec<ISODate> {
        match self {
            CalendarValue::Empty => Vec::new(),
            CalendarValue::Single(date) => vec![date.clone()],
            CalendarValue::Multiple(dates) => dates.clone(),
            CalendarValue::Range(start, end) => vec![start.clone(), end.clone()],
        }
    }

    /// The single date, in `"single"` mode.
    pub fn as_single(&self) -> Option<&ISODate> {
        match self {
            CalendarValue::Single(date) => Some(date),
            _ => None,
        }
    }

    /// The range's ends, in `"range"` mode.
    pub fn as_range(&self) -> Option<(&ISODate, &ISODate)> {
        match self {
            CalendarValue::Range(start, end) => Some((start, end)),
            _ => None,
        }
    }

    /// The value as `String(value)` writes it, which tells values apart
    /// (`sameDates`): `"null"`, a date, or dates joined by commas.
    pub fn key(&self) -> String {
        match self {
            CalendarValue::Empty => "null".to_string(),
            CalendarValue::Single(date) => date.to_string(),
            CalendarValue::Multiple(dates) => dates.iter().map(|date| date.as_ref()).collect::<Vec<_>>().join(","),
            CalendarValue::Range(start, end) => format!("{start},{end}"),
        }
    }
}

/// Dates a calendar shouldn't let anyone pick, as data or a function: an ISO
/// date, `Range { from, to }` for the dates between two, inclusive,
/// `Weekday(vec![0, 6])` for weekends (0 is Sunday), `Before(date)`,
/// `After(date)`, or a function. Data can be generated and sent over the
/// wire; a function can't.
#[derive(Clone)]
pub enum DateMatcher {
    Date(ISODate),
    Range { from: ISODate, to: ISODate },
    Weekday(Vec<u32>),
    Before(ISODate),
    After(ISODate),
    Fn(Rc<dyn Fn(&str) -> bool>),
}

impl DateMatcher {
    pub fn date(date: impl Into<ISODate>) -> Self {
        DateMatcher::Date(date.into())
    }

    pub fn range(from: impl Into<ISODate>, to: impl Into<ISODate>) -> Self {
        DateMatcher::Range {
            from: from.into(),
            to: to.into(),
        }
    }

    pub fn weekday(days: impl IntoIterator<Item = u32>) -> Self {
        DateMatcher::Weekday(days.into_iter().collect())
    }

    pub fn before(date: impl Into<ISODate>) -> Self {
        DateMatcher::Before(date.into())
    }

    pub fn after(date: impl Into<ISODate>) -> Self {
        DateMatcher::After(date.into())
    }

    pub fn function(matches: impl Fn(&str) -> bool + 'static) -> Self {
        DateMatcher::Fn(Rc::new(matches))
    }
}

impl PartialEq for DateMatcher {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (DateMatcher::Date(a), DateMatcher::Date(b)) => a == b,
            (DateMatcher::Range { from: a, to: b }, DateMatcher::Range { from: c, to: d }) => a == c && b == d,
            (DateMatcher::Weekday(a), DateMatcher::Weekday(b)) => a == b,
            (DateMatcher::Before(a), DateMatcher::Before(b)) => a == b,
            (DateMatcher::After(a), DateMatcher::After(b)) => a == b,
            (DateMatcher::Fn(a), DateMatcher::Fn(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl fmt::Debug for DateMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DateMatcher::Date(date) => f.debug_tuple("Date").field(date).finish(),
            DateMatcher::Range { from, to } => f.debug_struct("Range").field("from", from).field("to", to).finish(),
            DateMatcher::Weekday(days) => f.debug_tuple("Weekday").field(days).finish(),
            DateMatcher::Before(date) => f.debug_tuple("Before").field(date).finish(),
            DateMatcher::After(date) => f.debug_tuple("After").field(date).finish(),
            DateMatcher::Fn(_) => f.write_str("Fn(..)"),
        }
    }
}

/// One or more `DateMatcher`s.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DateMatchers(pub Vec<DateMatcher>);

impl From<DateMatcher> for DateMatchers {
    fn from(matcher: DateMatcher) -> Self {
        DateMatchers(vec![matcher])
    }
}

impl From<Vec<DateMatcher>> for DateMatchers {
    fn from(matchers: Vec<DateMatcher>) -> Self {
        DateMatchers(matchers)
    }
}

impl<const N: usize> From<[DateMatcher; N]> for DateMatchers {
    fn from(matchers: [DateMatcher; N]) -> Self {
        DateMatchers(matchers.into())
    }
}

/// Whether `date` is one of the dates `matchers` name.
pub fn matches_date(date: &str, matchers: Option<&DateMatchers>) -> bool {
    let Some(matchers) = matchers else {
        return false;
    };
    matchers.0.iter().any(|matcher| matches_one(date, matcher))
}

fn matches_one(date: &str, matcher: &DateMatcher) -> bool {
    match matcher {
        DateMatcher::Date(day) => date == day.as_ref(),
        DateMatcher::Fn(matches) => matches(date),
        DateMatcher::Range { from, to } => date >= from.as_ref() && date <= to.as_ref(),
        DateMatcher::Weekday(days) => days.contains(&weekday(date)),
        DateMatcher::Before(day) => date < day.as_ref(),
        DateMatcher::After(day) => date > day.as_ref(),
    }
}

/// The month a date is in.
pub fn month_of(date: &str) -> ISOMonth {
    date.chars().take(7).collect::<String>().into()
}

/// The month `months` months after `month`, or before when negative.
pub fn add_months_to(month: &str, months: i64) -> ISOMonth {
    month_of(&add_months(&format!("{month}-01"), months))
}

/// Whether a value is a month, like `"2026-10"`.
pub fn is_iso_month(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || byte.is_ascii_digit())
        && is_iso_date(&format!("{value}-01"))
}

/// The dates a value selects, sorted. A range gives its two ends.
pub fn selected_dates(value: &CalendarValue) -> Vec<ISODate> {
    match value {
        CalendarValue::Empty => Vec::new(),
        CalendarValue::Single(date) => {
            if is_iso_date(date) {
                vec![date.clone()]
            } else {
                Vec::new()
            }
        }
        CalendarValue::Multiple(_) | CalendarValue::Range(..) => {
            let mut dates: Vec<ISODate> = value.entries().into_iter().filter(|date| is_iso_date(date)).collect();
            dates.sort();
            dates
        }
    }
}

/// A value read for a mode, so a controlled value of the wrong shape can't
/// break the calendar.
pub fn normalize_value(mode: CalendarMode, value: &CalendarValue) -> CalendarValue {
    let dates = selected_dates(value);
    match mode {
        CalendarMode::Multiple => {
            let mut unique: Vec<ISODate> = Vec::with_capacity(dates.len());
            for date in dates {
                if !unique.contains(&date) {
                    unique.push(date);
                }
            }
            CalendarValue::Multiple(unique)
        }
        CalendarMode::Range => match dates.as_slice() {
            [start, end] => CalendarValue::Range(start.clone(), end.clone()),
            _ => CalendarValue::Empty,
        },
        CalendarMode::Single => dates.into_iter().next().map_or(CalendarValue::Empty, CalendarValue::Single),
    }
}

/// The rules a range must follow, from the calendar's options.
#[derive(Clone, Copy)]
pub struct RangeRules<'a> {
    pub min_days: Option<f64>,
    pub max_days: Option<f64>,
    /// Whether a range may hold unavailable dates.
    pub allow_unavailable: bool,
    pub is_unavailable: &'a dyn Fn(&str) -> bool,
}

/// Whether `start` to `end` is a range the rules allow. Lengths count both
/// ends: a day to itself is 1.
pub fn is_allowed_range(start: &str, end: &str, rules: &RangeRules) -> bool {
    let length = (days_between(start, end) + 1) as f64;
    if rules.min_days.is_some_and(|min| length < min) {
        return false;
    }
    if rules.max_days.is_some_and(|max| length > max) {
        return false;
    }
    if rules.allow_unavailable {
        return true;
    }
    let mut date: ISODate = start.to_string().into();
    while date.as_ref() <= end {
        if (rules.is_unavailable)(&date) {
            return false;
        }
        let next = add_days(&date, 1);
        // The last date there is has no next one.
        if next == date {
            break;
        }
        date = next;
    }
    true
}

/// The range from `anchor` to `date`, whichever comes first, or `None` when
/// the rules don't allow it.
pub fn range_to(anchor: &str, date: &str, rules: &RangeRules) -> Option<(ISODate, ISODate)> {
    let (start, end) = if date < anchor { (date, anchor) } else { (anchor, date) };
    is_allowed_range(start, end, rules).then(|| (start.to_string().into(), end.to_string().into()))
}

/// What a press does to a range: the start it leaves, and the range it ends,
/// if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PressedRange {
    pub anchor: Option<ISODate>,
    pub value: Option<(ISODate, ISODate)>,
}

/// What a press on `date` does to a range: with no start yet, it starts one.
/// With a start, any date ends it, the earlier of the two starting it, if the
/// rules allow; one they don't allow keeps the start.
pub fn press_range(anchor: Option<&str>, date: &str, rules: &RangeRules) -> PressedRange {
    let Some(anchor) = anchor else {
        return PressedRange {
            anchor: Some(date.to_string().into()),
            value: None,
        };
    };
    match range_to(anchor, date, rules) {
        Some(value) => PressedRange {
            anchor: None,
            value: Some(value),
        },
        None => PressedRange {
            anchor: Some(anchor.to_string().into()),
            value: None,
        },
    }
}

/// What a key's move reads besides the key and the date it moves from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MoveDateOptions<'a> {
    pub rtl: bool,
    pub week_starts_on: u32,
    pub min: Option<&'a str>,
    pub max: Option<&'a str>,
}

// Where a key moves focus from `from`, before clamping, or `None` for a key
// the grid doesn't use.
fn date_for_key(key: &Key, shift: bool, from: &str, options: &MoveDateOptions) -> Option<ISODate> {
    let back = if options.rtl { 1 } else { -1 };
    Some(match key {
        Key::ArrowLeft => add_days(from, back),
        Key::ArrowRight => add_days(from, -back),
        Key::ArrowUp => add_days(from, -7),
        Key::ArrowDown => add_days(from, 7),
        Key::Home => start_of_week(from, options.week_starts_on),
        Key::End => end_of_week(from, options.week_starts_on),
        Key::PageUp if shift => add_years(from, -1),
        Key::PageUp => add_months(from, -1),
        Key::PageDown if shift => add_years(from, 1),
        Key::PageDown => add_months(from, 1),
        _ => return None,
    })
}

/// A key's move, kept within `min` and `max`. `None` when the key isn't the
/// grid's.
pub fn move_date(key: &Key, shift: bool, from: &str, options: &MoveDateOptions) -> Option<ISODate> {
    let to = date_for_key(key, shift, from, options)?;
    Some(clamp_date(&to, options.min, options.max))
}

/// Whether Previous, Next and keys turn one month, or all the months shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PageBy {
    #[default]
    Month,
    Page,
}

/// The first visible month that shows `date`, given the months shown now:
/// unchanged when it's already in view, else moved by whole pages when
/// `page_by` is `Page`, or just far enough when it's `Month`.
pub fn month_to_show(date: &str, first: &str, count: usize, page_by: PageBy) -> ISOMonth {
    let count = count.max(1) as i64;
    let target = month_of(date);
    let last = add_months_to(first, count - 1);
    if target.as_ref() >= first && target <= last {
        return first.to_string().into();
    }
    let offset = months_between(first, &target);
    if page_by == PageBy::Page {
        return add_months_to(first, offset.div_euclid(count) * count);
    }
    if target.as_ref() < first {
        target
    } else {
        add_months_to(&target, -(count - 1))
    }
}

// Whole months from `from` to `to`.
fn months_between(from: &str, to: &str) -> i64 {
    let parts = |month: &str| -> (i64, i64) {
        let mut split = month.split('-').map(|part| part.parse::<i64>().unwrap_or(0));
        (split.next().unwrap_or(0), split.next().unwrap_or(0))
    };
    let (from_year, from_month) = parts(from);
    let (to_year, to_month) = parts(to);
    (to_year - from_year) * 12 + to_month - from_month
}

/// A cell of a month grid: its date, and whether it belongs to the months
/// around the grid's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CalendarCell {
    /// The cell's ISO date.
    pub date: ISODate,
    /// Whether the date is in the month before or after the grid's.
    pub outside: bool,
    /// Whether the cell shows nothing: a date outside the grid's month when
    /// outside days are hidden, or one another visible month already shows.
    pub hidden: bool,
}

/// What [`grid_weeks`] lays a month out with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GridWeeksOptions {
    pub week_starts_on: u32,
    pub fixed_weeks: bool,
    pub show_outside_days: bool,
    /// Whether it's the first month shown.
    pub first: bool,
    /// Whether it's the last month shown.
    pub last: bool,
}

/// The weeks of one visible month. Days of the months around it show only
/// at the outer edges of the visible months, since a date appears once on
/// screen: the first month's leading days and the last month's trailing ones.
pub fn grid_weeks(month: &str, options: GridWeeksOptions) -> Vec<[CalendarCell; 7]> {
    let first_day = format!("{month}-01");
    let grid = MonthGridOptions::default()
        .week_starts_on(options.week_starts_on)
        .fixed_weeks(options.fixed_weeks);
    month_grid(&first_day, grid)
        .into_iter()
        .map(|week| {
            week.map(|date| {
                let outside = month_of(&date).as_ref() != month;
                let hidden = outside
                    && (!options.show_outside_days
                        || if date.as_ref() < first_day.as_str() {
                            !options.first
                        } else {
                            !options.last
                        });
                CalendarCell { date, outside, hidden }
            })
        })
        .collect()
}

/// The first and last dates a calendar shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VisibleRange {
    pub start: ISODate,
    pub end: ISODate,
}

/// The first and last dates a calendar shows.
pub fn visible_range(first: &str, count: usize) -> VisibleRange {
    VisibleRange {
        start: format!("{first}-01").into(),
        end: add_days(&format!("{}-01", add_months_to(first, count as i64)), -1),
    }
}
