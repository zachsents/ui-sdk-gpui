//! Port of `components/calendar/calendar.ts`: a calendar: months of days in
//! a grid, one tab stop, and arrow keys that move by day and week across
//! months, as the WAI-ARIA date picker dialog asks. It selects one date, any
//! number, or a range, as ISO strings, with no date library: `ui-sdk-date`
//! does the arithmetic and ICU4X the names.

use std::rc::Rc;

use gpui::{App, Context, Entity, FocusHandle, Focusable, SharedString, Window};
use ui_sdk_date::{ISODate, TextWidth};
use ui_sdk_forms::{Node, watch_form_disabled};
use ui_sdk_kernel::input_source::EventCallback;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentValue, ChangeCallback, ChangeDetails, ChangeOptions,
    InputEvent, Instance, Part, Reason, json,
};

use crate::calendar_api::{self, CalendarHost, CalendarModelState};
use crate::calendar_model::{CalendarCell, CalendarMode, CalendarValue, DateMatchers, ISOMonth, PageBy};

/// Why a calendar's value wants to change: a date was pressed, or Enter or
/// Space on one (`ItemPress`), or an api call (`Imperative`).
pub type CalendarValueChangeReason = Reason;

/// Why a calendar's visible month wants to change: Previous or Next
/// (`PreviousPress`, `NextPress`), a key moving focus to another month
/// (`ListNavigation`), a press on a day of another month (`ItemPress`), a
/// month or year select (`SelectChange`), or an api call (`Imperative`).
pub type CalendarMonthChangeReason = Reason;

/// Why focus wants to move to another day: a key (`ListNavigation`), a press
/// (`ItemPress`), focus arriving another way, like a screen reader's
/// (`FocusIn`), or `focus` (`Imperative`).
pub type CalendarFocusedChangeReason = Reason;

/// Why a range's start wants to change: a press (`ItemPress`), or Escape
/// dropping it (`EscapeKey`).
pub type CalendarAnchorChangeReason = Reason;

/// Options for every write method.
pub type CalendarChangeOptions = ChangeOptions;

/// The options `Calendar` takes.
#[derive(Clone)]
pub struct CalendarOptions {
    /// How many dates it selects: one, any number, or a range from a start
    /// to an end. `Single` by default.
    pub mode: CalendarMode,
    /// The selected value: an ISO date for `Single`, a list for `Multiple`,
    /// a start and end for `Range`, or `Empty`. Pass it to control the
    /// calendar.
    pub value: Option<CalendarValue>,
    /// The value it starts with, when uncontrolled.
    pub default_value: Option<CalendarValue>,
    /// Runs when a date is pressed (`ItemPress`), with Enter or Space too, or
    /// an api call sets the value (`Imperative`). A range changes once both
    /// ends are picked. It runs once the calendar's update is over, so it may
    /// update any entity, the calendar too.
    pub on_value_change: Option<ChangeCallback<CalendarValue>>,
    /// Runs as the value is asked to change, before it does, inside the
    /// calendar's update: `details.cancel()` keeps the value. It may update
    /// any entity but the calendar.
    pub on_before_value_change: Option<ChangeCallback<CalendarValue>>,
    /// The first month shown, like `"2026-10"`. Pass it to control the
    /// calendar.
    pub month: Option<ISOMonth>,
    /// The first month it shows, when uncontrolled. Defaults to the selected
    /// date's month, else today's.
    pub default_month: Option<ISOMonth>,
    /// Runs when the months shown want to change: Previous or Next, a key
    /// moving focus to another month, a press on a day of another month, a
    /// month or year select, or an api call. It runs once the calendar's
    /// update is over.
    pub on_month_change: Option<ChangeCallback<ISOMonth>>,
    /// Runs as the months shown are asked to change, before they do, inside
    /// the calendar's update: `details.cancel()` keeps them. It may update
    /// any entity but the calendar.
    pub on_before_month_change: Option<ChangeCallback<ISOMonth>>,
    /// The earliest date that can be picked. Keys stop there, and Previous
    /// stops at its month.
    pub min: Option<ISODate>,
    /// The latest date that can be picked.
    pub max: Option<ISODate>,
    /// Dates that can't be picked, like booked days, as data: ISO dates,
    /// ranges, weekdays, `Before`, `After`, or a function. They stay
    /// focusable, so keyboard users hear them.
    pub unavailable: Option<DateMatchers>,
    /// Whether nothing can be picked or focused.
    pub disabled: bool,
    /// Whether dates can be focused but not picked.
    pub read_only: bool,
    /// Whether pressing the selected date again clears it, in `Single` mode.
    pub deselectable: bool,
    /// Runs when keys, a press or `focus` moved focus to another day, once
    /// the calendar's update is over.
    pub on_focused_change: Option<EventCallback<ISODate>>,
    /// Runs before keys, a press or `focus` move focus to another day, inside
    /// the calendar's update: `details.cancel()` keeps focus where it was. It
    /// may update any entity but the calendar.
    pub on_before_focused_change: Option<EventCallback<ISODate>>,
    /// Runs when a press started a range, or ended it or Escape dropped it,
    /// with `None`, once the calendar's update is over.
    pub on_anchor_change: Option<EventCallback<Option<ISODate>>>,
    /// Runs before a press starts a range, or ends it or Escape drops it,
    /// with `None`, inside the calendar's update: `details.cancel()` keeps
    /// the start as it was. It may update any entity but the calendar.
    pub on_before_anchor_change: Option<EventCallback<Option<ISODate>>>,
    /// How many months show side by side, each in its own grid. 1 by default.
    pub months: usize,
    /// Whether Previous, Next and keys turn one month, or all the months
    /// shown. `Month` by default.
    pub page_by: PageBy,
    /// The language of the names and labels. Defaults to the page's
    /// (`ui_sdk_text::set_page_locale`), then the system's.
    pub locale: Option<SharedString>,
    /// The first day of the week, 0 for Sunday. Defaults to the locale's.
    pub week_starts_on: Option<i64>,
    /// How weekday headers are written. `Short` by default.
    pub weekday_format: TextWidth,
    /// How month names are written, in the heading and the month select.
    /// `Long` by default.
    pub month_format: TextWidth,
    /// Whether every month shows six weeks, so the calendar doesn't change
    /// height.
    pub fixed_weeks: bool,
    /// Whether the days of the months before and after show at the edges.
    /// On by default.
    pub show_outside_days: bool,
    /// Today's date, for `aria-current` and the starting month. Defaults to
    /// today in `time_zone`.
    pub today: Option<ISODate>,
    /// The time zone that says what day today is, like `"America/New_York"`.
    /// Defaults to the system's.
    pub time_zone: Option<SharedString>,
    /// The fewest days a range can span, counting both ends.
    pub min_days: Option<f64>,
    /// The most days a range can span, counting both ends.
    pub max_days: Option<f64>,
    /// Whether a range may hold unavailable dates, like a stay over a booked
    /// night.
    pub allow_unavailable: bool,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for CalendarOptions {
    fn default() -> Self {
        Self {
            mode: CalendarMode::Single,
            value: None,
            default_value: None,
            on_value_change: None,
            on_before_value_change: None,
            month: None,
            default_month: None,
            on_month_change: None,
            on_before_month_change: None,
            min: None,
            max: None,
            unavailable: None,
            disabled: false,
            read_only: false,
            deselectable: false,
            on_focused_change: None,
            on_before_focused_change: None,
            on_anchor_change: None,
            on_before_anchor_change: None,
            months: 1,
            page_by: PageBy::Month,
            locale: None,
            week_starts_on: None,
            weekday_format: TextWidth::Short,
            month_format: TextWidth::Long,
            fixed_weeks: false,
            show_outside_days: true,
            today: None,
            time_zone: None,
            min_days: None,
            max_days: None,
            allow_unavailable: false,
            debug: false,
        }
    }
}

impl CalendarOptions {
    pub fn mode(mut self, mode: CalendarMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn value(mut self, value: CalendarValue) -> Self {
        self.value = Some(value);
        self
    }

    pub fn default_value(mut self, value: CalendarValue) -> Self {
        self.default_value = Some(value);
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&CalendarValue, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&CalendarValue, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn month(mut self, month: impl Into<ISOMonth>) -> Self {
        self.month = Some(month.into());
        self
    }

    pub fn default_month(mut self, month: impl Into<ISOMonth>) -> Self {
        self.default_month = Some(month.into());
        self
    }

    pub fn on_month_change(
        mut self,
        callback: impl Fn(&ISOMonth, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_month_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_month_change(
        mut self,
        callback: impl Fn(&ISOMonth, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_month_change = Some(Rc::new(callback));
        self
    }

    pub fn min(mut self, min: impl Into<ISODate>) -> Self {
        self.min = Some(min.into());
        self
    }

    pub fn max(mut self, max: impl Into<ISODate>) -> Self {
        self.max = Some(max.into());
        self
    }

    pub fn unavailable(mut self, unavailable: impl Into<DateMatchers>) -> Self {
        self.unavailable = Some(unavailable.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn deselectable(mut self, deselectable: bool) -> Self {
        self.deselectable = deselectable;
        self
    }

    pub fn on_focused_change(
        mut self,
        callback: impl Fn(&ISODate, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_focused_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_focused_change(
        mut self,
        callback: impl Fn(&ISODate, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_focused_change = Some(Rc::new(callback));
        self
    }

    pub fn on_anchor_change(
        mut self,
        callback: impl Fn(&Option<ISODate>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_anchor_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_anchor_change(
        mut self,
        callback: impl Fn(&Option<ISODate>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_anchor_change = Some(Rc::new(callback));
        self
    }

    pub fn months(mut self, months: usize) -> Self {
        self.months = months;
        self
    }

    pub fn page_by(mut self, page_by: PageBy) -> Self {
        self.page_by = page_by;
        self
    }

    pub fn locale(mut self, locale: impl Into<SharedString>) -> Self {
        self.locale = Some(locale.into());
        self
    }

    pub fn week_starts_on(mut self, day: i64) -> Self {
        self.week_starts_on = Some(day);
        self
    }

    pub fn weekday_format(mut self, format: TextWidth) -> Self {
        self.weekday_format = format;
        self
    }

    pub fn month_format(mut self, format: TextWidth) -> Self {
        self.month_format = format;
        self
    }

    pub fn fixed_weeks(mut self, fixed_weeks: bool) -> Self {
        self.fixed_weeks = fixed_weeks;
        self
    }

    pub fn show_outside_days(mut self, show: bool) -> Self {
        self.show_outside_days = show;
        self
    }

    pub fn today(mut self, today: impl Into<ISODate>) -> Self {
        self.today = Some(today.into());
        self
    }

    pub fn time_zone(mut self, time_zone: impl Into<SharedString>) -> Self {
        self.time_zone = Some(time_zone.into());
        self
    }

    pub fn min_days(mut self, days: f64) -> Self {
        self.min_days = Some(days);
        self
    }

    pub fn max_days(mut self, days: f64) -> Self {
        self.max_days = Some(days);
        self
    }

    pub fn allow_unavailable(mut self, allow: bool) -> Self {
        self.allow_unavailable = allow;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarState {
    /// The selected value.
    pub value: CalendarValue,
    /// The first month shown.
    pub month: ISOMonth,
    /// How it selects.
    pub mode: CalendarMode,
    /// Whether nothing can be picked or focused, by itself or a fieldset.
    pub disabled: bool,
    /// Whether dates can be focused but not picked.
    pub read_only: bool,
    /// The date with the tab stop, `None` until days render.
    pub focused: Option<ISODate>,
    /// A range's start while its end is being picked.
    pub anchor: Option<ISODate>,
}

/// What the heading's `with` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarHeadingState {
    /// The first month shown, to format the heading yourself.
    pub month: ISOMonth,
    /// The months shown in `month_format`, like "October 2026", or
    /// "October – November 2026".
    pub label: SharedString,
}

/// A month or a year a select offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarSelectOption {
    /// The month, 1 to 12, or the year.
    pub value: i32,
    /// Its name in the locale, like "October" or "2026".
    pub label: SharedString,
    /// Whether it lies outside `min` and `max`.
    pub disabled: bool,
}

/// Which select: the months', or the years'.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CalendarSelectKind {
    Month,
    Year,
}

impl CalendarSelectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CalendarSelectKind::Month => "month",
            CalendarSelectKind::Year => "year",
        }
    }
}

/// What the month and year selects' `with` reads. The options can feed any
/// select.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarSelectState {
    /// The first month shown's month, 1 to 12, or its year.
    pub value: i32,
    /// Every month, or the years from `min` to `max` (10 either side of the
    /// year shown without them).
    pub options: Vec<CalendarSelectOption>,
    /// Whether the calendar is disabled, by its option or a fieldset.
    pub disabled: bool,
}

/// What Previous's and Next's `with` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarStepState {
    /// The first month it shows.
    pub month: ISOMonth,
    /// Whether it can't go further, past `min` or `max`.
    pub disabled: bool,
}

/// A cell of a grid's week, as the grid's state lists it: its date, which
/// the day part takes, whether it belongs to the month before or after, and
/// whether it shows nothing, an outside day that's hidden or one another
/// grid shows, which renders an empty cell.
pub type CalendarGridCell = CalendarCell;

/// What a grid's `with` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarGridState {
    /// The month it shows: the first month shown, plus its offset.
    pub month: ISOMonth,
    /// The month's name and year in `month_format`, like "October 2026".
    pub label: SharedString,
    /// The weekday of each column, 0 for Sunday, in the order they show.
    pub weekdays: Vec<u32>,
    /// The weeks, each seven cells. Render a day for each cell that isn't
    /// hidden.
    pub weeks: Vec<[CalendarGridCell; 7]>,
}

/// What a weekday header's `with` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarWeekdayState {
    /// The weekday, 0 for Sunday.
    pub weekday: u32,
    /// Its name, in `weekday_format`.
    pub label: SharedString,
    /// Its full name, which screen readers hear.
    pub name: SharedString,
}

/// What a day's `with` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarDayState {
    /// Its ISO date (`data-value`).
    pub date: ISODate,
    /// The day of the month in the locale's digits, like "16".
    pub label: SharedString,
    /// Whether it's selected. In a range, every date from start to end is.
    pub selected: bool,
    /// Whether it's the `today` option's date, or else today in `time_zone`.
    pub today: bool,
    /// Whether it belongs to the month before or after its grid's.
    pub outside: bool,
    /// Whether it can't be picked: outside `min` and `max`, unavailable,
    /// unable to end the range being picked, or the calendar is disabled.
    pub disabled: bool,
    /// Whether `unavailable` names it.
    pub unavailable: bool,
    /// Whether a range starts here, including one being picked.
    pub range_start: bool,
    /// Whether a range ends here, including one being picked.
    pub range_end: bool,
    /// Whether it's inside a range, ends included, including one being picked.
    pub in_range: bool,
    /// Whether it's in the range being picked (`data-preview`).
    pub preview: bool,
}

impl CalendarDayState {
    /// Whether it's inside a range but neither end (`data-range-middle`).
    pub fn range_middle(&self) -> bool {
        self.in_range && !self.range_start && !self.range_end
    }
}

/// A calendar. Its entity is its controller.
pub struct Calendar {
    instance: Instance,
    model: CalendarModelState,
    // The root's place in the page, for the fieldsets that disable it.
    form: Node,
}

impl Calendar {
    pub fn new(options: CalendarOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("calendar").with_agent(cx);
        CalendarValue::register_data();
        instance.debug = options.debug;
        let model = CalendarModelState::new(&options, window, cx);
        Self {
            instance,
            model,
            form: watch_form_disabled(cx),
        }
    }

    /// Hands it new options, as a framework re-renders with new props (`sync`).
    pub fn set_options(&mut self, options: CalendarOptions, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        if self.model.sync(&options) {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the calendar keep its own.
    pub fn set_controlled_value(&mut self, value: Option<CalendarValue>, cx: &mut Context<Self>) {
        if self.model.set_controlled_value(value) {
            cx.notify();
        }
    }

    /// Passes the controlled first month, or `None` to let the calendar keep
    /// its own.
    pub fn set_controlled_month(&mut self, month: Option<ISOMonth>, cx: &mut Context<Self>) {
        if self.model.set_controlled_month(month) {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The calendar's state, which Date Picker shares.
    pub fn model(&self) -> &CalendarModelState {
        &self.model
    }

    pub fn is_disabled(&self) -> bool {
        calendar_api::is_disabled(self)
    }

    pub fn state(&self) -> CalendarState {
        CalendarState {
            value: self.model.value().clone(),
            month: self.model.month().clone(),
            mode: self.model.mode(),
            disabled: self.is_disabled(),
            read_only: self.model.is_read_only(),
            focused: self.model.focused(),
            anchor: self.model.anchor().cloned(),
        }
    }

    /// The selected value.
    pub fn get_value(&self) -> &CalendarValue {
        self.model.value()
    }

    /// Selects a value. Returns false when canceled, unchanged, disabled,
    /// read-only, or holding a date people can't pick, like one past `max`.
    pub fn set_value(
        &mut self,
        value: CalendarValue,
        options: CalendarChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        calendar_api::set_value(self, value, options, window, cx)
    }

    /// The first month shown, like `"2026-10"`.
    pub fn get_month(&self) -> &ISOMonth {
        self.model.month()
    }

    /// Shows a month first. Returns false when canceled or unchanged.
    pub fn set_month(
        &mut self,
        month: impl Into<ISOMonth>,
        options: CalendarChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        calendar_api::set_month(self, month.into(), options, window, cx)
    }

    /// Shows the months before, as the Previous part does.
    pub fn previous(&mut self, options: CalendarChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        calendar_api::previous(self, options, window, cx)
    }

    /// Shows the months after, as the Next part does.
    pub fn next(&mut self, options: CalendarChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        calendar_api::next(self, options, window, cx)
    }

    /// Focuses a date, turning the months to show it, or with `None` the
    /// date with the tab stop.
    pub fn focus(&mut self, date: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        calendar_api::focus(self, date, window, cx);
    }

    /// Shows the month a month or year select picked, as its change does:
    /// `value` is the month, 1 to 12, or the year. It's kept between `min`
    /// and `max`. Returns false when canceled or unchanged.
    pub fn select(
        &mut self,
        kind: CalendarSelectKind,
        value: i32,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        calendar_api::select(self, kind, value, event, window, cx)
    }

    /// The `root` part, around the calendar's other parts.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<CalendarState> {
        let calendar = this.read(cx);
        calendar.model.roving().begin(cx);
        let form = calendar.form.clone();
        Part::new(&calendar.instance, "root", None, calendar.state()).map_el(|el| form.track(el))
    }

    /// The `heading` part: the months shown, like "October 2026", which
    /// screen readers hear change.
    pub fn heading(this: &Entity<Self>, cx: &App) -> Part<CalendarHeadingState> {
        calendar_api::heading(this, cx)
    }

    /// The `previous` part: a button that shows the months before.
    pub fn previous_part(this: &Entity<Self>, cx: &App) -> Part<CalendarStepState> {
        calendar_api::previous_part(this, cx)
    }

    /// The `next` part: a button that shows the months after.
    pub fn next_part(this: &Entity<Self>, cx: &App) -> Part<CalendarStepState> {
        calendar_api::next_part(this, cx)
    }

    /// The `month-select` part, where a select of the months goes. It calls
    /// [`Calendar::select`] with the month picked.
    pub fn month_select(this: &Entity<Self>, cx: &App) -> Part<CalendarSelectState> {
        calendar_api::month_select(this, cx)
    }

    /// The `year-select` part, where a select of the years goes.
    pub fn year_select(this: &Entity<Self>, cx: &App) -> Part<CalendarSelectState> {
        calendar_api::year_select(this, cx)
    }

    /// A `grid` part: the month at `offset` from the first shown, from 0.
    pub fn grid(this: &Entity<Self>, offset: usize, cx: &App) -> Part<CalendarGridState> {
        calendar_api::grid(this, offset, cx)
    }

    /// The `weekdays` part: the header row of the grid at `offset`.
    pub fn weekdays(this: &Entity<Self>, offset: usize, cx: &App) -> Part {
        calendar_api::weekdays(this, offset, cx)
    }

    /// A `weekday` part: the header of column `index` in the grid at `offset`.
    pub fn weekday(this: &Entity<Self>, offset: usize, index: usize, cx: &App) -> Part<CalendarWeekdayState> {
        calendar_api::weekday(this, offset, index, cx)
    }

    /// A `week` part: the row of week `index` in the grid at `offset`.
    pub fn week(this: &Entity<Self>, offset: usize, index: usize, cx: &App) -> Part {
        calendar_api::week(this, offset, index, cx)
    }

    /// A `day` part: the cell of `date` in the grid at `offset`. Pass each
    /// cell of the grid's weeks that isn't hidden.
    pub fn day(this: &Entity<Self>, date: impl Into<ISODate>, offset: usize, cx: &App) -> Part<CalendarDayState> {
        calendar_api::day(this, date, offset, cx)
    }
}

impl CalendarHost for Calendar {
    fn calendar(&self) -> &CalendarModelState {
        &self.model
    }

    fn calendar_mut(&mut self) -> &mut CalendarModelState {
        &mut self.model
    }

    fn calendar_instance(&self) -> &Instance {
        &self.instance
    }

    fn calendar_split(&mut self) -> (&Instance, &mut CalendarModelState) {
        (&self.instance, &mut self.model)
    }

    fn calendar_form_disabled(&self) -> bool {
        self.form.form_disabled()
    }
}

/// Focus goes to the day with the tab stop, or before days render, the one
/// that will hold it.
impl Focusable for Calendar {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        let date = self.model.focused().unwrap_or_else(|| self.model.preferred_date());
        self.model.roving().focus_handle(&date, cx)
    }
}

// A value as data: `null`, a date, or a list of them.
fn value_to_json(value: &CalendarValue) -> AgentValue {
    match value {
        CalendarValue::Empty => AgentValue::Null,
        CalendarValue::Single(date) => json!(date.to_string()),
        CalendarValue::Multiple(dates) => json!(dates.iter().map(ToString::to_string).collect::<Vec<_>>()),
        CalendarValue::Range(start, end) => json!([start.to_string(), end.to_string()]),
    }
}

// A value from data, read for the mode as the api reads one.
fn value_from_json(value: &AgentValue) -> Option<CalendarValue> {
    match value {
        AgentValue::Null => Some(CalendarValue::Empty),
        AgentValue::String(date) => Some(CalendarValue::Single(date.clone().into())),
        AgentValue::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(|date| ISODate::from(date.to_string())))
            .collect::<Option<Vec<_>>>()
            .map(CalendarValue::Multiple),
        _ => None,
    }
}

/// `agent: { actions: ["setValue", "setMonth", "previous", "next"] }` and
/// `controlled: ["value", "month"]`.
impl Agent for Calendar {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |calendar, args, window, cx| {
            let Some(value) = value_from_json(args.value(0)) else {
                return Ok(false.into());
            };
            Ok(calendar.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setMonth", |calendar, args, window, cx| {
            let month = args.string(0)?;
            Ok(calendar.set_month(month, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("previous", |calendar, _args, window, cx| {
            Ok(calendar.previous(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("next", |calendar, _args, window, cx| {
            Ok(calendar.next(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["value", "month"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        let model = &self.model;
        AgentState::new()
            .with("value", value_to_json(model.value()))
            .with("month", model.month().to_string())
            .with("disabled", self.is_disabled())
            .with("readOnly", model.is_read_only())
            .with("min", model.min().map(ToString::to_string))
            .with("max", model.max().map(ToString::to_string))
    }
}
