//! Port of `components/calendar/calendar-api.ts`: what Calendar and Date
//! Picker share: a calendar's state, its behavior and every part but the
//! root. The public entry is `calendar.rs`; a date picker builds on this.
//!
//! The original shares a state shape, an api built over a primitive's store
//! (`calendarApi`) and slot definitions (`calendarSlots`). Here the state is
//! [`CalendarModelState`], which a component keeps in a field and hands out
//! through [`CalendarHost`]; the api is the functions below, which take the
//! component; and the slots are the part functions, which take its entity.
//! A date picker implements [`CalendarHost`] over its own state, with its own
//! `picked`, and renders these parts in its popup.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, InteractiveElement, KeyDownEvent, MouseButton,
    MouseDownEvent, SharedString, StatefulInteractiveElement, Window,
};
use ui_sdk_date::{
    DateTimeFormatOptions, DateTimeValue, ISODate, MonthWidth, NumericWidth, TextWidth, add_days,
    clamp_date, date_formatter, first_day_of_week, format_date, format_date_range, is_iso_date,
    month_names, today_date, weekday_names,
};
use ui_sdk_focus::{RovingFocus, RovingFocusOptions, RovingMember};
use ui_sdk_kernel::input_source::EventCallback;
use ui_sdk_kernel::modality::focus_event;
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Aria, ChangeOptions, Controllable, Current, InputEvent, Instance, Key, Live, Need, Part, Reason,
    dispatch_then,
};
use ui_sdk_platform::is_rtl;
use ui_sdk_text::locale_of;

use crate::calendar::{
    CalendarDayState, CalendarGridState, CalendarHeadingState, CalendarOptions,
    CalendarSelectKind, CalendarSelectOption, CalendarSelectState, CalendarStepState,
    CalendarWeekdayState,
};
use crate::calendar_model::{
    CalendarMode, CalendarValue, DateMatchers, GridWeeksOptions, ISOMonth, MoveDateOptions, PageBy,
    RangeRules, add_months_to, grid_weeks, is_allowed_range, is_iso_month, matches_date, month_of,
    month_to_show, move_date, normalize_value, press_range, range_to, selected_dates,
    visible_range,
};

/// What a component built on the calendar gives its parts and its api: the
/// calendar's state, the instance its parts belong to, whether a fieldset
/// disables it, and what happens once a value is picked. Calendar implements
/// it over itself; a date picker over its own state.
pub trait CalendarHost: Sized + 'static {
    fn calendar(&self) -> &CalendarModelState;

    fn calendar_mut(&mut self) -> &mut CalendarModelState;

    /// The instance the calendar's parts belong to.
    fn calendar_instance(&self) -> &Instance;

    /// The instance and the state at once, for a change that reports to the
    /// instance.
    fn calendar_split(&mut self) -> (&Instance, &mut CalendarModelState);

    /// Whether an ancestor fieldset disables it, for a component that sits
    /// in a form (`formDisabled`).
    fn calendar_form_disabled(&self) -> bool {
        false
    }

    /// A press or a key completed a value: a date in single mode, a range's
    /// end (`picked`). A date picker closes here.
    fn calendar_picked(&mut self, _event: Option<&InputEvent>, _window: &mut Window, _cx: &mut Context<Self>) {}
}

/// A calendar's state, which Date Picker extends (`CalendarModelState`).
pub struct CalendarModelState {
    pub(crate) mode: CalendarMode,
    pub(crate) value: Controllable<CalendarValue>,
    pub(crate) month: Controllable<ISOMonth>,
    // A range's start while its end is being picked, and the date the end
    // would be.
    pub(crate) anchor: Option<ISODate>,
    pub(crate) hover: Option<ISODate>,
    pub(crate) today: Option<ISODate>,
    pub(crate) min: Option<ISODate>,
    pub(crate) max: Option<ISODate>,
    pub(crate) unavailable: Option<DateMatchers>,
    pub(crate) disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) deselectable: bool,
    pub(crate) months: usize,
    pub(crate) page_by: PageBy,
    pub(crate) locale: Option<SharedString>,
    pub(crate) week_starts_on: Option<u32>,
    pub(crate) weekday_format: TextWidth,
    pub(crate) month_format: TextWidth,
    pub(crate) fixed_weeks: bool,
    pub(crate) show_outside_days: bool,
    pub(crate) min_days: Option<f64>,
    pub(crate) max_days: Option<f64>,
    pub(crate) allow_unavailable: bool,
    on_focused_change: Option<EventCallback<ISODate>>,
    on_before_focused_change: Option<EventCallback<ISODate>>,
    on_anchor_change: Option<EventCallback<Option<ISODate>>>,
    on_before_anchor_change: Option<EventCallback<Option<ISODate>>>,
    // The date with the tab stop is the roving focus's stop (`focused`).
    pub(crate) roving: RovingFocus<ISODate>,
    // A move to `approved` was asked before focus went, so landing there asks nothing.
    approved: Option<ISODate>,
    pointer: Option<InputEvent>,
    // A day a press couldn't focus, which its click then leaves unfocused.
    refused: Option<ISODate>,
    // The day that held focus when focus was last looked at, so focus that
    // lands on another from elsewhere, like a screen reader, is asked about.
    noted: Option<ISODate>,
    previous_focus: FocusHandle,
    next_focus: FocusHandle,
    // A grid's state, and a select's, the same until what they show changes.
    grids: RefCell<HashMap<String, CalendarGridState>>,
    selects: RefCell<HashMap<String, CalendarSelectState>>,
}

fn date_or_null(value: Option<&SharedString>) -> Option<ISODate> {
    value.filter(|value| is_iso_date(value)).cloned()
}

// The month a calendar opens on: the selection's, else today's, within `min` and `max`.
fn initial_month(options: &CalendarOptions, value: &CalendarValue) -> ISOMonth {
    if let Some(month) = options.month.as_ref().filter(|month| is_iso_month(month)) {
        return month.clone();
    }
    if let Some(month) = options.default_month.as_ref().filter(|month| is_iso_month(month)) {
        return month.clone();
    }
    if let Some(selected) = selected_dates(value).first() {
        return month_of(selected);
    }
    let today = date_or_null(options.today.as_ref()).unwrap_or_else(|| today_date(options.time_zone.as_deref()));
    let min = date_or_null(options.min.as_ref());
    let max = date_or_null(options.max.as_ref());
    month_of(&clamp_date(&today, min.as_deref(), max.as_deref()))
}

impl CalendarModelState {
    /// A calendar's state from its options (`calendarState`). Today is read
    /// now, in `time_zone`, unless the `today` option names it.
    pub fn new<H: CalendarHost>(options: &CalendarOptions, window: &mut Window, cx: &mut Context<H>) -> Self {
        let mode = options.mode;
        let first = options
            .value
            .clone()
            .or_else(|| options.default_value.clone())
            .unwrap_or_default();
        let first = normalize_value(mode, &first);
        let mut value = Controllable::new("value", first.clone());
        value.set_controlled(options.value.as_ref().map(|value| normalize_value(mode, value)));
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());
        let mut month = Controllable::new("month", initial_month(options, &first));
        month.set_controlled(options.month.clone().filter(|month| is_iso_month(month)));
        month.set_on_change(options.on_month_change.clone());
        month.set_on_before_change(options.on_before_month_change.clone());
        let today = match &options.today {
            Some(today) => date_or_null(Some(today)),
            None => Some(today_date(options.time_zone.as_deref())),
        };
        let mut state = Self {
            mode,
            value,
            month,
            anchor: None,
            hover: None,
            today,
            min: None,
            max: None,
            unavailable: None,
            disabled: false,
            read_only: false,
            deselectable: false,
            months: 1,
            page_by: PageBy::Month,
            locale: None,
            week_starts_on: None,
            weekday_format: TextWidth::Short,
            month_format: TextWidth::Long,
            fixed_weeks: false,
            show_outside_days: true,
            min_days: None,
            max_days: None,
            allow_unavailable: false,
            on_focused_change: None,
            on_before_focused_change: None,
            on_anchor_change: None,
            on_before_anchor_change: None,
            roving: RovingFocus::new(RovingFocusOptions::default(), window, cx),
            approved: None,
            pointer: None,
            refused: None,
            noted: None,
            previous_focus: cx.focus_handle(),
            next_focus: cx.focus_handle(),
            grids: RefCell::default(),
            selects: RefCell::default(),
        };
        state.apply_settings(options);
        state
    }

    // What every options object sets (`calendarSettings`). Returns whether
    // what renders changed.
    fn apply_settings(&mut self, options: &CalendarOptions) -> bool {
        let min = date_or_null(options.min.as_ref());
        let max = date_or_null(options.max.as_ref());
        let months = options.months.max(1);
        let week_starts_on = options.week_starts_on.map(|day| day.rem_euclid(7) as u32);
        let changed = self.mode != options.mode
            || self.min != min
            || self.max != max
            || self.unavailable != options.unavailable
            || self.disabled != options.disabled
            || self.read_only != options.read_only
            || self.deselectable != options.deselectable
            || self.months != months
            || self.page_by != options.page_by
            || self.locale != options.locale
            || self.week_starts_on != week_starts_on
            || self.weekday_format != options.weekday_format
            || self.month_format != options.month_format
            || self.fixed_weeks != options.fixed_weeks
            || self.show_outside_days != options.show_outside_days
            || self.min_days != options.min_days
            || self.max_days != options.max_days
            || self.allow_unavailable != options.allow_unavailable;
        self.mode = options.mode;
        self.min = min;
        self.max = max;
        self.unavailable = options.unavailable.clone();
        self.disabled = options.disabled;
        self.read_only = options.read_only;
        self.deselectable = options.deselectable;
        self.months = months;
        self.page_by = options.page_by;
        self.locale = options.locale.clone();
        self.week_starts_on = week_starts_on;
        self.weekday_format = options.weekday_format;
        self.month_format = options.month_format;
        self.fixed_weeks = options.fixed_weeks;
        self.show_outside_days = options.show_outside_days;
        self.min_days = options.min_days;
        self.max_days = options.max_days;
        self.allow_unavailable = options.allow_unavailable;
        self.on_focused_change = options.on_focused_change.clone();
        self.on_before_focused_change = options.on_before_focused_change.clone();
        self.on_anchor_change = options.on_anchor_change.clone();
        self.on_before_anchor_change = options.on_before_anchor_change.clone();
        changed
    }

    /// Hands it new options (`calendarSync`): the settings, the controlled
    /// value and month, and today when the option names it. Returns whether
    /// what renders changed.
    pub fn sync(&mut self, options: &CalendarOptions) -> bool {
        let mut changed = self.apply_settings(options);
        changed |= self
            .value
            .set_controlled(options.value.as_ref().map(|value| normalize_value(options.mode, value)));
        self.value.set_on_change(options.on_value_change.clone());
        self.value.set_on_before_change(options.on_before_value_change.clone());
        match &options.month {
            Some(month) if is_iso_month(month) => changed |= self.month.set_controlled(Some(month.clone())),
            Some(_) => {}
            None => changed |= self.month.set_controlled(None),
        }
        self.month.set_on_change(options.on_month_change.clone());
        self.month.set_on_before_change(options.on_before_month_change.clone());
        if let Some(today) = &options.today {
            let today = date_or_null(Some(today));
            changed |= self.today != today;
            self.today = today;
        }
        changed
    }

    /// Passes the controlled value, or `None` to let the calendar keep its
    /// own. Returns whether what renders changed.
    pub fn set_controlled_value(&mut self, value: Option<CalendarValue>) -> bool {
        let mode = self.mode;
        self.value.set_controlled(value.map(|value| normalize_value(mode, &value)))
    }

    /// Passes the controlled first month, or `None` to let the calendar keep
    /// its own. A value that isn't a month is ignored.
    pub fn set_controlled_month(&mut self, month: Option<ISOMonth>) -> bool {
        match month {
            Some(month) if !is_iso_month(&month) => false,
            month => self.month.set_controlled(month),
        }
    }

    pub fn mode(&self) -> CalendarMode {
        self.mode
    }

    /// The selected value.
    pub fn value(&self) -> &CalendarValue {
        self.value.get()
    }

    /// The first month shown.
    pub fn month(&self) -> &ISOMonth {
        self.month.get()
    }

    /// A range's start while its end is being picked.
    pub fn anchor(&self) -> Option<&ISODate> {
        self.anchor.as_ref()
    }

    /// The date a range being picked would end on.
    pub fn hover(&self) -> Option<&ISODate> {
        self.hover.as_ref()
    }

    /// Sets a range's start and the end it previews, with nothing asked,
    /// like a date picker dropping both as it commits typed dates
    /// (`store.set({ anchor, hover })`). Returns whether they changed.
    pub fn set_range_anchor(&mut self, anchor: Option<ISODate>, hover: Option<ISODate>) -> bool {
        let changed = self.anchor != anchor || self.hover != hover;
        self.anchor = anchor;
        self.hover = hover;
        changed
    }

    /// Today's date, for `aria-current`.
    pub fn today(&self) -> Option<&ISODate> {
        self.today.as_ref()
    }

    pub fn min(&self) -> Option<&ISODate> {
        self.min.as_ref()
    }

    pub fn max(&self) -> Option<&ISODate> {
        self.max.as_ref()
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// How many months show.
    pub fn months(&self) -> usize {
        self.months
    }

    /// The date with the tab stop, `None` until days render (`focused`).
    pub fn focused(&self) -> Option<ISODate> {
        self.roving.stop()
    }

    /// The days' roving focus.
    pub fn roving(&self) -> &RovingFocus<ISODate> {
        &self.roving
    }

    /// Forgets the date focus was on, so the tab stop starts over from the
    /// selection, like a date picker opening (`focused: null`).
    pub fn reset_focus(&mut self, cx: &mut App) {
        self.roving.reset(cx);
    }

    /// What the calendar formats in: its `locale`, else the page's.
    pub fn locale(&self, cx: &App) -> SharedString {
        locale_of(self.locale.as_ref(), cx)
    }

    /// The first day of the week, 0 for Sunday: `week_starts_on`, else the
    /// locale's.
    pub fn week_start(&self, cx: &App) -> u32 {
        self.week_starts_on
            .unwrap_or_else(|| first_day_of_week(Some(&self.locale(cx))))
    }

    /// Whether `unavailable` names a date.
    pub fn is_unavailable(&self, date: &str) -> bool {
        matches_date(date, self.unavailable.as_ref())
    }

    /// Whether a date lies before `min` or after `max`.
    pub fn out_of_bounds(&self, date: &str) -> bool {
        self.min.as_ref().is_some_and(|min| date < min.as_ref()) || self.max.as_ref().is_some_and(|max| date > max.as_ref())
    }

    /// A date that can't be picked. The calendar being disabled is checked apart.
    pub fn blocked_date(&self, date: &str) -> bool {
        self.out_of_bounds(date) || self.is_unavailable(date)
    }

    fn grid_month(&self, offset: usize) -> ISOMonth {
        add_months_to(self.month.get(), offset as i64)
    }

    fn is_outside(&self, date: &str, offset: usize) -> bool {
        month_of(date) != self.grid_month(offset)
    }

    /// Runs `run` with the rules a range must follow (`rangeRules`).
    pub fn with_range_rules<R>(&self, run: impl FnOnce(&RangeRules) -> R) -> R {
        let unavailable = |date: &str| self.is_unavailable(date);
        run(&RangeRules {
            min_days: self.min_days,
            max_days: self.max_days,
            allow_unavailable: self.allow_unavailable,
            is_unavailable: &unavailable,
        })
    }

    /// Whether people could pick a value now: the calendar isn't disabled or
    /// read-only, the value fits the mode, no date is blocked, and the rules
    /// allow a range (`isPickable`). `disabled` is the calendar's, a
    /// fieldset's included.
    pub fn is_pickable(&self, value: &CalendarValue, disabled: bool) -> bool {
        if disabled || self.read_only {
            return false;
        }
        let dates = selected_dates(&normalize_value(self.mode, value));
        if value.entries().iter().any(|date| !dates.contains(date)) || dates.iter().any(|date| self.blocked_date(date)) {
            return false;
        }
        self.mode != CalendarMode::Range
            || dates.is_empty()
            || self.with_range_rules(|rules| is_allowed_range(&dates[0], &dates[1], rules))
    }

    fn refused_end(&self, date: &str) -> bool {
        self.mode == CalendarMode::Range
            && self.anchor.as_ref().is_some_and(|anchor| {
                anchor.as_ref() != date && self.with_range_rules(|rules| range_to(anchor, date, rules)).is_none()
            })
    }

    // The range days show: the one being picked, else the selected one.
    fn shown_range(&self) -> Option<(ISODate, ISODate)> {
        if self.mode != CalendarMode::Range {
            return None;
        }
        if let Some(anchor) = &self.anchor {
            let preview = self
                .hover
                .as_ref()
                .and_then(|hover| self.with_range_rules(|rules| range_to(anchor, hover, rules)));
            return Some(preview.unwrap_or_else(|| (anchor.clone(), anchor.clone())));
        }
        self.value.get().as_range().map(|(start, end)| (start.clone(), end.clone()))
    }

    /// The date the tab stop goes to when no day has had focus: the selection
    /// if it shows, else today, else the first date that can be picked
    /// (`preferredDate`).
    pub fn preferred_date(&self) -> ISODate {
        let range = visible_range(self.month.get(), self.months);
        let shows = |date: &str| date >= range.start.as_ref() && date <= range.end.as_ref() && !self.blocked_date(date);
        let found = selected_dates(self.value.get())
            .into_iter()
            .find(|date| shows(date))
            .or_else(|| self.today.clone().filter(|today| shows(today)));
        if let Some(date) = found {
            return date;
        }
        let first = clamp_date(&range.start, self.min.as_deref(), self.max.as_deref());
        let mut day = first.clone();
        while day <= range.end {
            if !self.blocked_date(&day) {
                return day;
            }
            let next = add_days(&day, 1);
            if next == day {
                break;
            }
            day = next;
        }
        first
    }

    fn heading_label(&self, cx: &App) -> String {
        let locale = self.locale(cx);
        let options = DateTimeFormatOptions::default()
            .month(MonthWidth::from(self.month_format))
            .year(NumericWidth::Numeric);
        let first = format!("{}-01", self.month.get());
        if self.months == 1 {
            return format_date(&first, Some(&locale), Some(&options));
        }
        let last = format!("{}-01", add_months_to(self.month.get(), self.months as i64 - 1));
        format_date_range(&first, &last, Some(&locale), Some(&options))
    }

    // The month and year selects: every month, and the years from min to
    // max, or 10 either side of the year shown. The same until those change.
    fn select_state(&self, kind: CalendarSelectKind, disabled: bool, cx: &App) -> CalendarSelectState {
        let locale = self.locale(cx);
        let key = format!(
            "{}|{}|{}|{}|{:?}|{:?}|{}",
            kind.as_str(),
            self.month.get(),
            locale,
            self.month_format.as_str(),
            self.min,
            self.max,
            disabled
        );
        if let Some(cached) = self.selects.borrow().get(&key) {
            return cached.clone();
        }
        let built = self.build_select_state(kind, disabled, &locale);
        let mut selects = self.selects.borrow_mut();
        if selects.len() > 50 {
            selects.clear();
        }
        selects.insert(key, built.clone());
        built
    }

    fn build_select_state(&self, kind: CalendarSelectKind, disabled: bool, locale: &str) -> CalendarSelectState {
        let month = self.month.get();
        let year: i32 = month[..4].parse().unwrap_or(0);
        let first = self.min.as_ref().map(|min| month_of(min));
        let last = self.max.as_ref().map(|max| month_of(max));
        let months = kind == CalendarSelectKind::Month;
        let year_of = |month: &ISOMonth| month[..4].parse::<i32>().unwrap_or(year);
        let from = if months {
            1
        } else {
            year.min(first.as_ref().map_or(year - 10, year_of))
        };
        let to = if months {
            12
        } else {
            year.max(last.as_ref().map_or(year + 10, year_of))
        };
        let names = month_names(Some(locale), self.month_format);
        let format = date_formatter(Some(locale), &DateTimeFormatOptions::default().year(NumericWidth::Numeric));
        let options = (from..=to)
            .enumerate()
            .map(|(index, value)| {
                // A month outside min and max, in the year shown, can't be picked.
                let shown = format!("{year:04}-{value:02}");
                CalendarSelectOption {
                    value,
                    label: if months {
                        names[index].clone().into()
                    } else {
                        format.format(DateTimeValue::new(value, 1, 1)).into()
                    },
                    disabled: months
                        && (first.as_ref().is_some_and(|first| shown.as_str() < first.as_ref())
                            || last.as_ref().is_some_and(|last| shown.as_str() > last.as_ref())),
                }
            })
            .collect();
        CalendarSelectState {
            value: if months { month[5..7].parse().unwrap_or(1) } else { year },
            options,
            disabled,
        }
    }

    // The first month a month or year select picks, kept between min and max.
    fn selected_month(&self, kind: CalendarSelectKind, picked: i32) -> ISOMonth {
        let month = self.month.get();
        let (year, month_part) = (&month[..4], &month[5..7]);
        let target: ISOMonth = match kind {
            CalendarSelectKind::Month => format!("{year}-{picked:02}").into(),
            CalendarSelectKind::Year => format!("{picked:04}-{month_part}").into(),
        };
        if let Some(min) = &self.min
            && target < month_of(min)
        {
            return month_of(min);
        }
        if let Some(max) = &self.max
            && target > month_of(max)
        {
            return month_of(max);
        }
        target
    }

    fn step_target(&self, direction: i64, disabled: bool) -> CalendarStepState {
        let by = match self.page_by {
            PageBy::Page => self.months as i64,
            PageBy::Month => 1,
        };
        let month = add_months_to(self.month.get(), direction * by);
        let disabled = disabled
            || if direction < 0 {
                self.min.as_ref().is_some_and(|min| month_of(min) >= *self.month.get())
            } else {
                self.max
                    .as_ref()
                    .is_some_and(|max| month_of(max) <= add_months_to(self.month.get(), self.months as i64 - 1))
            };
        CalendarStepState { month, disabled }
    }

    // A grid's state, the same until its month or layout changes.
    fn grid_state(&self, offset: usize, cx: &App) -> CalendarGridState {
        let month = self.grid_month(offset);
        let week_starts_on = self.week_start(cx);
        let locale = self.locale(cx);
        let first = offset == 0;
        let last = offset + 1 == self.months;
        let key = format!(
            "{month}|{locale}|{}|{week_starts_on}|{}|{}|{first}|{last}",
            self.month_format.as_str(),
            self.fixed_weeks,
            self.show_outside_days
        );
        if let Some(grid) = self.grids.borrow().get(&key) {
            return grid.clone();
        }
        let options = DateTimeFormatOptions::default()
            .month(MonthWidth::from(self.month_format))
            .year(NumericWidth::Numeric);
        let grid = CalendarGridState {
            label: format_date(&format!("{month}-01"), Some(&locale), Some(&options)).into(),
            weekdays: (0..7).map(|index| (week_starts_on + index) % 7).collect(),
            weeks: grid_weeks(
                &month,
                GridWeeksOptions {
                    week_starts_on,
                    fixed_weeks: self.fixed_weeks,
                    show_outside_days: self.show_outside_days,
                    first,
                    last,
                },
            ),
            month,
        };
        let mut grids = self.grids.borrow_mut();
        if grids.len() > 50 {
            grids.clear();
        }
        grids.insert(key, grid.clone());
        grid
    }

    fn day_state(&self, date: &ISODate, offset: usize, disabled: bool, cx: &App) -> CalendarDayState {
        let range = self.shown_range();
        let selected = match &range {
            Some((start, end)) => date >= start && date <= end,
            None => match self.value.get() {
                CalendarValue::Multiple(dates) => dates.contains(date),
                value => value.as_single() == Some(date),
            },
        };
        let day: i32 = date.get(8..).and_then(|day| day.parse().ok()).unwrap_or(1);
        let label = date_formatter(
            Some(&self.locale(cx)),
            &DateTimeFormatOptions::default().day(NumericWidth::Numeric),
        )
        .format(DateTimeValue::new(2000, 1, day));
        CalendarDayState {
            date: date.clone(),
            label: label.into(),
            selected,
            today: self.today.as_ref() == Some(date),
            outside: self.is_outside(date, offset),
            disabled: disabled || self.blocked_date(date) || self.refused_end(date),
            unavailable: self.is_unavailable(date),
            range_start: range.as_ref().is_some_and(|(start, _)| start == date),
            range_end: range.as_ref().is_some_and(|(_, end)| end == date),
            in_range: selected && range.is_some(),
            preview: range.is_some() && self.anchor.is_some() && selected,
        }
    }
}

/// Whether the calendar can't be used: its `disabled` option, or a fieldset
/// around the component (`isDisabled`).
pub fn is_disabled<H: CalendarHost>(host: &H) -> bool {
    host.calendar().disabled || host.calendar_form_disabled()
}

/// The calendar's state as the heading shows it.
pub fn heading_state<H: CalendarHost>(host: &H, cx: &App) -> CalendarHeadingState {
    let model = host.calendar();
    CalendarHeadingState {
        month: model.month.get().clone(),
        label: model.heading_label(cx).into(),
    }
}

/// What a month or year select shows.
pub fn select_state<H: CalendarHost>(host: &H, kind: CalendarSelectKind, cx: &App) -> CalendarSelectState {
    host.calendar().select_state(kind, is_disabled(host), cx)
}

/// What Previous (`direction` -1) or Next (1) shows.
pub fn step_state<H: CalendarHost>(host: &H, direction: i64) -> CalendarStepState {
    host.calendar().step_target(direction, is_disabled(host))
}

/// What the grid at `offset` shows.
pub fn grid_state<H: CalendarHost>(host: &H, offset: usize, cx: &App) -> CalendarGridState {
    host.calendar().grid_state(offset, cx)
}

/// What a day shows.
pub fn day_state<H: CalendarHost>(host: &H, date: &ISODate, offset: usize, cx: &App) -> CalendarDayState {
    host.calendar().day_state(date, offset, is_disabled(host), cx)
}

// Asks before focus moves to another day. Returns whether it may.
fn move_focus<H: CalendarHost>(
    host: &mut H,
    date: &ISODate,
    reason: Reason,
    event: Option<InputEvent>,
    window: &mut Window,
    cx: &mut Context<H>,
) -> bool {
    let model = host.calendar();
    if model.roving.stop().as_ref() == Some(date) {
        return true;
    }
    let (before, after) = (model.on_before_focused_change.clone(), model.on_focused_change.clone());
    let options = ChangeOptions {
        reason: Some(reason),
        event,
        source: None,
    };
    if dispatch_then(before.as_ref(), after.as_ref(), date, options, window, cx) {
        return false;
    }
    host.calendar_mut().approved = Some(date.clone());
    true
}

// Focus a pointer or a script put on a day, past `move_focus`. A stop the
// calendar settles on itself, on load or as the months turn, asks nothing.
// Returns whether focus may stay there; when it may not, focus goes back to
// the day with the tab stop.
fn focus_landed<H: CalendarHost>(
    host: &mut H,
    date: &ISODate,
    press: Option<InputEvent>,
    event: InputEvent,
    window: &mut Window,
    cx: &mut Context<H>,
) -> bool {
    let model = host.calendar_mut();
    if model.approved.as_ref() == Some(date) {
        model.approved = None;
        return true;
    }
    let Some(from) = model.roving.stop() else {
        return true;
    };
    if &from == date {
        return true;
    }
    let (before, after) = (model.on_before_focused_change.clone(), model.on_focused_change.clone());
    let options = match &press {
        Some(press) => ChangeOptions::event_of(Reason::ItemPress, press.clone()),
        None => ChangeOptions::event_of(Reason::FocusIn, event),
    };
    if !dispatch_then(before.as_ref(), after.as_ref(), date, options, window, cx) {
        return true;
    }
    let model = host.calendar_mut();
    if press.is_some() {
        model.refused = Some(date.clone());
    }
    model.noted = Some(from.clone());
    let roving = model.roving.clone();
    roving.focus(Some(&from), window, cx);
    false
}

/// Asks about focus that landed on a day from elsewhere since it was last
/// looked at, like a screen reader moving it, as the original's focus
/// listener on each day does. Grids run it once each frame they render has
/// been drawn.
pub fn check_focus<H: CalendarHost>(host: &mut H, window: &mut Window, cx: &mut Context<H>) {
    let model = host.calendar_mut();
    let now = model.roving.focused(window);
    if now == model.noted {
        return;
    }
    model.noted = now.clone();
    let Some(date) = now else {
        return;
    };
    let press = model.pointer.take();
    focus_landed(host, &date, press, focus_event(window), window, cx);
}

fn set_anchor<H: CalendarHost>(
    host: &mut H,
    anchor: Option<ISODate>,
    hover: Option<ISODate>,
    options: ChangeOptions,
    window: &mut Window,
    cx: &mut Context<H>,
) -> bool {
    let model = host.calendar();
    if anchor != model.anchor {
        let (before, after) = (model.on_before_anchor_change.clone(), model.on_anchor_change.clone());
        if dispatch_then(before.as_ref(), after.as_ref(), &anchor, options, window, cx) {
            return false;
        }
    }
    if host.calendar_mut().set_range_anchor(anchor, hover) {
        cx.notify();
    }
    true
}

/// Shows a month first, with no check that people could (`internal.setMonth`).
/// Returns false when it isn't a month, canceled or unchanged.
pub fn change_month<H: CalendarHost>(
    host: &mut H,
    month: ISOMonth,
    options: ChangeOptions,
    window: &mut Window,
    cx: &mut Context<H>,
) -> bool {
    if !is_iso_month(&month) {
        return false;
    }
    let (instance, model) = host.calendar_split();
    let changed = model.month.change(instance, month, options, window, cx);
    if changed {
        cx.notify();
    }
    changed
}

/// Selects a value read for the mode, with no check that people could pick
/// it (`internal.setValue`). Returns false when canceled or unchanged.
pub fn change_value<H: CalendarHost>(
    host: &mut H,
    value: CalendarValue,
    options: ChangeOptions,
    window: &mut Window,
    cx: &mut Context<H>,
) -> bool {
    let (instance, model) = host.calendar_split();
    let value = normalize_value(model.mode, &value);
    let changed = model.value.change(instance, value, options, window, cx);
    if changed {
        cx.notify();
    }
    changed
}

/// Turns the months to show `date`, unless they do (`reveal`).
pub fn reveal<H: CalendarHost>(
    host: &mut H,
    date: &str,
    options: ChangeOptions,
    window: &mut Window,
    cx: &mut Context<H>,
) {
    let model = host.calendar();
    let first = month_to_show(date, model.month.get(), model.months, model.page_by);
    if first != *model.month.get() {
        change_month(host, first, options, window, cx);
    }
}

fn step<H: CalendarHost>(
    host: &mut H,
    direction: i64,
    options: ChangeOptions,
    window: &mut Window,
    cx: &mut Context<H>,
) -> bool {
    let target = step_state(host, direction);
    !target.disabled && change_month(host, target.month, options, window, cx)
}

// A press on a date, or Enter or Space on one: what it does to the value.
fn pick<H: CalendarHost>(
    host: &mut H,
    date: &ISODate,
    event: Option<InputEvent>,
    window: &mut Window,
    cx: &mut Context<H>,
) {
    let disabled = is_disabled(host);
    let model = host.calendar();
    if disabled || model.read_only || model.blocked_date(date) {
        return;
    }
    let options = || ChangeOptions {
        reason: Some(Reason::ItemPress),
        event: event.clone(),
        source: None,
    };
    match model.mode {
        CalendarMode::Multiple => {
            let mut value = match model.value.get() {
                CalendarValue::Multiple(dates) => dates.clone(),
                other => selected_dates(other),
            };
            if value.contains(date) {
                value.retain(|selected| selected != date);
            } else {
                value.push(date.clone());
            }
            change_value(host, CalendarValue::Multiple(value), options(), window, cx);
        }
        CalendarMode::Range => {
            let pressed = model.with_range_rules(|rules| press_range(model.anchor.as_deref(), date, rules));
            let hover = pressed.anchor.is_some().then(|| date.clone());
            if !set_anchor(host, pressed.anchor.clone(), hover, options(), window, cx) {
                return;
            }
            if let Some((start, end)) = pressed.value {
                let value = CalendarValue::Range(start, end);
                if change_value(host, value.clone(), options(), window, cx)
                    || host.calendar().value.get().key() == value.key()
                {
                    host.calendar_picked(event.as_ref(), window, cx);
                }
            }
        }
        CalendarMode::Single => {
            if model.deselectable && model.value.get().as_single() == Some(date) {
                change_value(host, CalendarValue::Empty, options(), window, cx);
            } else if change_value(host, CalendarValue::Single(date.clone()), options(), window, cx)
                || host.calendar().value.get().as_single() == Some(date)
            {
                host.calendar_picked(event.as_ref(), window, cx);
            }
        }
    }
}

/// Selects a value, as a person could: false when canceled, unchanged,
/// disabled, read-only, or holding a date people can't pick, like one past
/// `max` (`setValue`).
pub fn set_value<H: CalendarHost>(
    host: &mut H,
    value: CalendarValue,
    options: ChangeOptions,
    window: &mut Window,
    cx: &mut Context<H>,
) -> bool {
    host.calendar().is_pickable(&value, is_disabled(host)) && change_value(host, value, options, window, cx)
}

/// Shows a month first (`setMonth`). False when canceled or unchanged.
pub fn set_month<H: CalendarHost>(
    host: &mut H,
    month: ISOMonth,
    options: ChangeOptions,
    window: &mut Window,
    cx: &mut Context<H>,
) -> bool {
    change_month(host, month, options, window, cx)
}

/// Shows the months before, as the Previous part does (`previous`).
pub fn previous<H: CalendarHost>(host: &mut H, options: ChangeOptions, window: &mut Window, cx: &mut Context<H>) -> bool {
    step(host, -1, options, window, cx)
}

/// Shows the months after, as the Next part does (`next`).
pub fn next<H: CalendarHost>(host: &mut H, options: ChangeOptions, window: &mut Window, cx: &mut Context<H>) -> bool {
    step(host, 1, options, window, cx)
}

/// Focuses a date, turning the months to show it, or with `None` the date
/// with the tab stop (`focus`). A date past `min` or `max` focuses the bound.
pub fn focus<H: CalendarHost>(host: &mut H, date: Option<&str>, window: &mut Window, cx: &mut Context<H>) {
    let Some(date) = date else {
        let model = host.calendar();
        let roving = model.roving.clone();
        let stop = model.roving.stop();
        roving.focus(stop.as_ref(), window, cx);
        return;
    };
    if !is_iso_date(date) {
        return;
    }
    let model = host.calendar();
    let target = clamp_date(date, model.min.as_deref(), model.max.as_deref());
    if !move_focus(host, &target, Reason::Imperative, None, window, cx) {
        return;
    }
    reveal(host, &target, ChangeOptions::reason(Reason::Imperative), window, cx);
    let roving = host.calendar().roving.clone();
    roving.focus(Some(&target), window, cx);
}

/// What a month or year select's change does: shows the month it picks,
/// kept between `min` and `max`, with `SelectChange` (`internal.select`).
pub fn select<H: CalendarHost>(
    host: &mut H,
    kind: CalendarSelectKind,
    value: i32,
    event: Option<InputEvent>,
    window: &mut Window,
    cx: &mut Context<H>,
) -> bool {
    let month = host.calendar().selected_month(kind, value);
    let options = ChangeOptions {
        reason: Some(Reason::SelectChange),
        event,
        source: None,
    };
    change_month(host, month, options, window, cx)
}

// Whether a click came from a pointer that pressed, the original's
// `event.detail`: a screen reader's and a key's clicks count none.
fn counts_clicks(event: &InputEvent) -> bool {
    matches!(event, InputEvent::Click(ClickEvent::Mouse(_) | ClickEvent::Touch(_)))
}

/// A press on a day, or Enter or Space on one. A day of another month turns
/// the months to it (`internal.press`).
pub fn press<H: CalendarHost>(
    host: &mut H,
    date: &ISODate,
    offset: usize,
    event: InputEvent,
    window: &mut Window,
    cx: &mut Context<H>,
) {
    if is_disabled(host) {
        return;
    }
    let model = host.calendar();
    let outside = model.is_outside(date, offset);
    let out_of_bounds = model.out_of_bounds(date);
    // A press that didn't focus its day, like a screen reader's, focuses it.
    // Only the refused press's own click counts clicks; a click from code on
    // that day still focuses it.
    if !outside && !out_of_bounds {
        let handle = model.roving.focus_handle(date, cx);
        if !handle.is_focused(window) && (model.refused.as_ref() != Some(date) || !counts_clicks(&event)) {
            let press = host.calendar_mut().pointer.take();
            if focus_landed(host, date, press, focus_event(window), window, cx) {
                host.calendar_mut().noted = Some(date.clone());
                window.focus(&handle, cx);
            }
        }
    }
    let model = host.calendar_mut();
    model.pointer = None;
    model.refused = None;
    pick(host, date, Some(event.clone()), window, cx);
    if host.calendar().is_outside(date, offset)
        && !out_of_bounds
        && move_focus(host, date, Reason::ItemPress, Some(event.clone()), window, cx)
    {
        reveal(host, date, ChangeOptions::event_of(Reason::ItemPress, event), window, cx);
        let roving = host.calendar().roving.clone();
        roving.focus(Some(date), window, cx);
    }
}

/// Previous (`direction` -1) or Next (1) pressed (`internal.step`).
pub fn step_press<H: CalendarHost>(
    host: &mut H,
    direction: i64,
    event: InputEvent,
    window: &mut Window,
    cx: &mut Context<H>,
) -> bool {
    let reason = if direction < 0 {
        Reason::PreviousPress
    } else {
        Reason::NextPress
    };
    step(host, direction, ChangeOptions::event_of(reason, event), window, cx)
}

/// A key pressed on a day (`internal.keydown`): Shift with Enter or Space
/// picks it, Escape drops a range's start, and the grid's keys move focus,
/// turning the months when the date they reach doesn't show. Enter and Space
/// alone come as a keyboard click, which presses the day.
pub fn keydown<H: CalendarHost>(
    host: &mut H,
    date: &ISODate,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut Context<H>,
) {
    if window.default_prevented() {
        return;
    }
    let key = Key::of(&event.keystroke);
    let modifiers = event.keystroke.modifiers;
    let input = InputEvent::from(event);
    if matches!(key, Key::Enter | Key::Space) && !modifiers.alt && !modifiers.control && !modifiers.platform {
        if modifiers.shift {
            window.prevent_default();
            pick(host, date, Some(input), window, cx);
        }
        return;
    }
    // Escape drops the start of a range being picked, before it closes anything.
    if key == Key::Escape
        && host.calendar().anchor.is_some()
        && set_anchor(host, None, None, ChangeOptions::event_of(Reason::EscapeKey, input.clone()), window, cx)
    {
        window.prevent_default();
        return;
    }
    // The roving focus's own checks, ahead of the move, which asks before
    // focus goes.
    if modifiers.alt {
        return;
    }
    let model = host.calendar();
    let roving = model.roving.clone();
    if !roving.items().iter().any(|item| &item.key == date) {
        return;
    }
    if modifiers.control || modifiers.platform {
        return;
    }
    let options = MoveDateOptions {
        rtl: is_rtl(cx),
        week_starts_on: model.week_start(cx),
        min: model.min.as_deref(),
        max: model.max.as_deref(),
    };
    let Some(to) = move_date(&key, modifiers.shift, date, &options) else {
        return;
    };
    let target = if move_focus(host, &to, Reason::ListNavigation, Some(input.clone()), window, cx) {
        to
    } else {
        date.clone()
    };
    let Some(moved) = roving.keydown_with(event, date, move |_, _, _| Some(target), window, cx) else {
        return;
    };
    if moved.key == moved.from {
        return;
    }
    reveal(host, &moved.key, ChangeOptions::event_of(Reason::ListNavigation, input), window, cx);
    let model = host.calendar_mut();
    if model.anchor.is_some() {
        model.hover = Some(moved.key);
        cx.notify();
    }
}

/// A pointer pressed on a day (`internal.pointerdown`). gpui focuses a day
/// that takes focus once this returns, so focus landing there is asked about
/// now, and a refusal keeps gpui from moving it.
pub fn pointerdown<H: CalendarHost>(
    host: &mut H,
    date: &ISODate,
    offset: usize,
    event: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<H>,
) {
    let disabled = is_disabled(host);
    let model = host.calendar_mut();
    model.pointer = Some(InputEvent::from(event));
    model.refused = None;
    if disabled || model.is_outside(date, offset) || model.out_of_bounds(date) {
        return;
    }
    if model.roving.focus_handle(date, cx).is_focused(window) {
        return;
    }
    let press = model.pointer.take();
    if focus_landed(host, date, press, focus_event(window), window, cx) {
        host.calendar_mut().noted = Some(date.clone());
    } else {
        window.prevent_default();
    }
}

/// The pointer entered a day: a range being picked previews the end there
/// (`internal.hover`).
pub fn hover<H: CalendarHost>(host: &mut H, date: &ISODate, cx: &mut Context<H>) {
    let model = host.calendar_mut();
    if model.anchor.is_some() && model.hover.as_ref() != Some(date) {
        model.hover = Some(date.clone());
        cx.notify();
    }
}

/// The `heading` part: the months shown, like "October 2026". Screen readers
/// hear the month change after Previous, Next or a key turns it. A heading,
/// as the adapters' `h2`, named by its label.
pub fn heading<H: CalendarHost>(this: &Entity<H>, cx: &App) -> Part<CalendarHeadingState> {
    let host = this.read(cx);
    let state = heading_state(host, cx);
    let label = state.label.clone();
    Part::new(host.calendar_instance(), "heading", None, state)
        .aria_role(Role::Heading)
        .aria_level(2)
        .aria_label(label)
        .aria(Aria::new().live(Live::Polite))
}

fn step_part<H: CalendarHost>(this: &Entity<H>, direction: i64, cx: &App) -> Part<CalendarStepState> {
    let host = this.read(cx);
    let model = host.calendar();
    let state = step_state(host, direction);
    let (slot, label, handle) = if direction < 0 {
        ("previous", "Previous month", &model.previous_focus)
    } else {
        ("next", "Next month", &model.next_focus)
    };
    let disabled = state.disabled;
    let entity = this.downgrade();
    Part::new(host.calendar_instance(), slot, None, state)
        .aria_role(Role::Button)
        .focus_mode(FocusMode::Native, Some(handle), false)
        .aria_label(label)
        .aria(Aria::new().disabled(disabled))
        .on_press(move |event, window, cx| {
            entity
                .update(cx, |host, cx| {
                    step_press(host, direction, event, window, cx);
                })
                .ok();
        })
}

/// The `previous` part: a button that shows the months before. Named
/// "Previous month"; give it an `aria_label` in another language. Disabled
/// at `min`, where it stays focusable.
pub fn previous_part<H: CalendarHost>(this: &Entity<H>, cx: &App) -> Part<CalendarStepState> {
    step_part(this, -1, cx)
}

/// The `next` part: a button that shows the months after. Named "Next
/// month". Disabled at `max`.
pub fn next_part<H: CalendarHost>(this: &Entity<H>, cx: &App) -> Part<CalendarStepState> {
    step_part(this, 1, cx)
}

fn select_part<H: CalendarHost>(this: &Entity<H>, kind: CalendarSelectKind, cx: &App) -> Part<CalendarSelectState> {
    let host = this.read(cx);
    let state = select_state(host, kind, cx);
    let disabled = state.disabled;
    let (slot, label) = match kind {
        CalendarSelectKind::Month => ("month-select", "Month"),
        CalendarSelectKind::Year => ("year-select", "Year"),
    };
    // A group, since gpui gives a part without a role no node for its name
    // and disabled state to reach.
    Part::new(host.calendar_instance(), slot, None, state)
        .aria_role(Role::Group)
        .aria_label(label)
        .aria(Aria::new().disabled(disabled))
}

/// The `month-select` part: where a select of the months goes, which turns
/// the first month shown. Its state holds the options; the select inside
/// calls [`select`] with the month picked.
pub fn month_select<H: CalendarHost>(this: &Entity<H>, cx: &App) -> Part<CalendarSelectState> {
    select_part(this, CalendarSelectKind::Month, cx)
}

/// The `year-select` part: where a select of the years from `min` to `max`,
/// or 10 either side of the year shown, goes.
pub fn year_select<H: CalendarHost>(this: &Entity<H>, cx: &App) -> Part<CalendarSelectState> {
    select_part(this, CalendarSelectKind::Year, cx)
}

/// A `grid` part: the month at `offset` from the first shown, from 0, named
/// by its month. Render its weekday headers and weeks inside it.
pub fn grid<H: CalendarHost>(this: &Entity<H>, offset: usize, cx: &App) -> Part<CalendarGridState> {
    let host = this.read(cx);
    let model = host.calendar();
    model.roving.begin(cx);
    let disabled = is_disabled(host);
    let state = grid_state(host, offset, cx);
    let label = format_date(
        &format!("{}-01", state.month),
        Some(&model.locale(cx)),
        Some(
            &DateTimeFormatOptions::default()
                .month(MonthWidth::Long)
                .year(NumericWidth::Numeric),
        ),
    );
    let aria = Aria::new()
        .multiselectable(model.mode != CalendarMode::Single)
        .read_only(model.read_only)
        .disabled(disabled);
    let entity = this.downgrade();
    Part::new(host.calendar_instance(), "grid", Some(&offset.to_string()), state)
        .aria_role(Role::Grid)
        .unneeded(Need::Keys, "Focus stays on a day, whose key handler handles them.")
        .aria_label(label)
        .aria(aria)
        .on_props(move |window, cx| {
            entity.update(cx, |host, cx| check_focus(host, window, cx)).ok();
        })
}

/// The `weekdays` part: the row of weekday headers in the grid at `offset`.
pub fn weekdays<H: CalendarHost>(this: &Entity<H>, offset: usize, cx: &App) -> Part {
    let host = this.read(cx);
    Part::new(host.calendar_instance(), "weekdays", Some(&offset.to_string()), ()).aria_role(Role::Row)
}

/// A `weekday` part: the header of column `index`, from 0, in the grid at
/// `offset`. Named by the weekday's full name.
pub fn weekday<H: CalendarHost>(this: &Entity<H>, offset: usize, index: usize, cx: &App) -> Part<CalendarWeekdayState> {
    let host = this.read(cx);
    let model = host.calendar();
    let week_starts_on = model.week_start(cx);
    let locale = model.locale(cx);
    let index = index % 7;
    let label = weekday_names(Some(&locale), model.weekday_format, week_starts_on)[index].clone();
    let name = weekday_names(Some(&locale), TextWidth::Long, week_starts_on)[index].clone();
    let state = CalendarWeekdayState {
        weekday: (week_starts_on + index as u32) % 7,
        label: label.into(),
        name: name.clone().into(),
    };
    Part::new(host.calendar_instance(), "weekday", Some(&format!("{offset}-{index}")), state)
        .aria_role(Role::ColumnHeader)
        .aria_label(name)
}

/// A `week` part: the row of week `index`, from 0, in the grid at `offset`.
pub fn week<H: CalendarHost>(this: &Entity<H>, offset: usize, index: usize, cx: &App) -> Part {
    let host = this.read(cx);
    Part::new(host.calendar_instance(), "week", Some(&format!("{offset}-{index}")), ()).aria_role(Role::Row)
}

/// A `day` part: the cell of `date` in the grid at `offset`, named by its
/// full date, like "Friday, October 16, 2026". The grid's own days share one
/// tab stop; days of the months around it show but take no focus, and
/// neither do days past `min` or `max`. Unavailable days stay focusable, so
/// keyboard users hear them.
pub fn day<H: CalendarHost>(
    this: &Entity<H>,
    date: impl Into<ISODate>,
    offset: usize,
    cx: &App,
) -> Part<CalendarDayState> {
    let host = this.read(cx);
    let date: ISODate = date.into();
    let disabled = is_disabled(host);
    let model = host.calendar();
    let state = day_state(host, &date, offset, cx);
    let out_of_bounds = model.out_of_bounds(&date);
    let focusable = !disabled && !state.outside && !out_of_bounds;
    let preferred = model.preferred_date() == date;
    let handle = model.roving.focus_handle(&date, cx);
    let member = RovingMember::new()
        .disabled(model.blocked_date(&date))
        .focusable(focusable)
        .preferred(preferred)
        .claims(!state.outside && preferred);
    let tab_stop = model.roving.bind(date.clone(), &handle, member, cx);
    let aria = Aria::new()
        .disabled(state.disabled)
        .current(state.today.then_some(Current::Date));
    let label = format_date(&date, Some(&model.locale(cx)), None);
    let outside = state.outside;
    let selected = state.selected;
    let part = Part::new(host.calendar_instance(), "day", Some(&date), state)
        .aria_role(Role::GridCell)
        .aria_label(label)
        .aria(aria);
    let part = if outside { part } else { part.aria_selected(selected) };
    let part = if focusable {
        part.focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
    } else if outside {
        part.unneeded(
            Need::Focus,
            "Days of the months around show here, but focus moves through each grid's own.",
        )
    } else {
        part.unneeded(Need::Focus, "A day past min or max, or in a disabled calendar, takes no focus.")
    };
    let pressed = this.downgrade();
    let keyed = this.downgrade();
    let pointed = this.downgrade();
    let hovered = this.downgrade();
    let (press_date, key_date, point_date, hover_date) = (date.clone(), date.clone(), date.clone(), date);
    part.on_press(move |event, window, cx| {
        pressed
            .update(cx, |host, cx| press(host, &press_date, offset, event, window, cx))
            .ok();
    })
    .on_keys(DAY_KEYS, move |event, window, cx| {
        keyed
            .update(cx, |host, cx| keydown(host, &key_date, event, window, cx))
            .ok();
    })
    .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, window, cx| {
        pointed
            .update(cx, |host, cx| pointerdown(host, &point_date, offset, event, window, cx))
            .ok();
    })
    .on_hover(move |hovering: &bool, _window, cx| {
        if *hovering {
            hovered.update(cx, |host, cx| hover(host, &hover_date, cx)).ok();
        }
    })
}

/// The keys a day handles.
pub const DAY_KEYS: &[&str] = &[
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "ArrowDown",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "shift+Enter",
    "shift+ ",
    "Escape",
];
