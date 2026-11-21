//! Port of `components/date-picker/date-picker.ts`: a date picker: a text
//! input that reads a date typed in the app's language, and a button that
//! opens a calendar in a popup. The grid is Calendar's, with its keys and
//! range picking, and the popup is the one Popover uses. Values are ISO
//! strings, as in Calendar. This module is the public api and the primitive
//! itself. The state and the api live in `date_picker_api.rs`, and the parts
//! in `date_picker_parts.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, Entity, FocusHandle, Focusable, Hsla, Pixels, SharedString, Subscription, Window};
use ui_sdk_calendar::{CalendarMode, CalendarModelState, CalendarValue, DateMatchers, ISOMonth, PageBy};
use ui_sdk_date::{ISODate, TextWidth};
use ui_sdk_forms::{ElementInternals, Node};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentArgs, AgentError, AgentItem, AgentState, AgentValue, ChangeCallback,
    ChangeOptions, DismissAttemptReason, Instance, PENDING, Parts, Reason, json,
};
use ui_sdk_motion::{PresenceState, PresenceTiming};
use ui_sdk_native::{TextField, TextFieldOptions, TextFieldStyle};
use ui_sdk_platform::{Align, Anchor, Placement, PositionProps, Region, Side};
use ui_sdk_popups::{
    DismissAttemptCallback, OpenChangeCompleteCallback, Popup, PopupCloseOptions, PopupContentProps,
    PopupFocusTarget, PopupOpenOptions, Presentation, PresentationSetting, SheetPartState,
};

/// How many dates a date picker selects: one, or a range from a start to an
/// end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DatePickerMode {
    #[default]
    Single,
    Range,
}

impl DatePickerMode {
    /// The mode's name, as the original writes it, like `"range"`.
    pub fn as_str(self) -> &'static str {
        match self {
            DatePickerMode::Single => "single",
            DatePickerMode::Range => "range",
        }
    }

    /// The calendar mode it picks with.
    pub fn calendar_mode(self) -> CalendarMode {
        match self {
            DatePickerMode::Single => CalendarMode::Single,
            DatePickerMode::Range => CalendarMode::Range,
        }
    }

    pub(crate) fn of(mode: CalendarMode) -> Self {
        if mode == CalendarMode::Range {
            DatePickerMode::Range
        } else {
            DatePickerMode::Single
        }
    }
}

/// A date picker's value: an ISO date (`Single`), a start and an end
/// (`Range`), or `Empty`, the original's `null`. It's Calendar's value; any
/// other shape is read for the mode, as a controlled value of the wrong
/// shape is.
pub type DatePickerValue = CalendarValue;

/// Why a date picker's value wants to change: a date picked in the calendar
/// (`ItemPress`), a typed date committed on Enter or blur (`InputCommit`), a
/// preset or Clear pressed (`PresetPress`, `ClearPress`), a form reset
/// (`Reset`), or an api call (`Imperative`). The original's `restore` has no
/// meaning here.
pub type DatePickerValueChangeReason = Reason;

/// Why the months a date picker's calendar shows want to change: Calendar's
/// reasons, and a typed date or a preset turning them (`InputCommit`,
/// `PresetPress`).
pub type DatePickerMonthChangeReason = Reason;

/// Why a date picker's popup opened or closed: the trigger (`TriggerPress`),
/// a picked date or a preset that closes it (`ItemPress`, `PresetPress`), a
/// dismissal (`OutsidePress`, `EscapeKey`, `FocusOut`, `WindowBlur`, `Back`,
/// `Swipe`), a Close part (`ClosePress`), or an api call (`Imperative`).
pub type DatePickerOpenChangeReason = Reason;

/// The dismissals `dismissible` refuses that `on_dismiss_attempt` hears
/// about: Escape, an outside press, back and a swipe.
pub type DatePickerDismissReason = DismissAttemptReason;

/// Options for every write method.
pub type DatePickerChangeOptions = ChangeOptions;

/// The options `DatePicker` takes: Calendar's, but for its mode, value,
/// focus and range callbacks, with the date picker's own, and the attributes
/// of the native inputs it makes, which the original passes through to
/// them.
#[derive(Clone)]
pub struct DatePickerOptions {
    /// How many dates it selects: one, or a range from a start to an end,
    /// with an input for each. `Single` by default.
    pub mode: DatePickerMode,
    /// The selected value: an ISO date, a start and an end in range mode, or
    /// `Empty`. Pass it to control the date picker.
    pub value: Option<DatePickerValue>,
    /// The value it starts with, when uncontrolled, and goes back to when its
    /// form resets.
    pub default_value: Option<DatePickerValue>,
    /// Runs when a date is picked in the calendar (`ItemPress`), a typed date
    /// is committed on Enter or blur (`InputCommit`), a preset or Clear is
    /// pressed (`PresetPress`, `ClearPress`), the form resets (`Reset`), or an
    /// api call sets it (`Imperative`). A range changes once both ends are
    /// set. `details.cancel()` keeps the value. It runs inside the date
    /// picker's update, so it may update any entity but the date picker.
    pub on_value_change: Option<ChangeCallback<DatePickerValue>>,
    /// The first month the calendar shows, like `"2026-10"`. Pass it to
    /// control the calendar.
    pub month: Option<ISOMonth>,
    /// The first month it shows, when uncontrolled. Defaults to the selected
    /// date's month, else today's.
    pub default_month: Option<ISOMonth>,
    /// Runs when the months the calendar shows want to change, as in
    /// Calendar, or a typed date or a preset turns them. It runs inside the
    /// date picker's update.
    pub on_month_change: Option<ChangeCallback<ISOMonth>>,
    /// The earliest date that can be typed or picked. Keys stop there, and
    /// Previous stops at its month.
    pub min: Option<ISODate>,
    /// The latest date that can be typed or picked.
    pub max: Option<ISODate>,
    /// Dates that can't be typed or picked, like booked days, as data. They
    /// stay focusable in the calendar, so keyboard users hear them.
    pub unavailable: Option<DateMatchers>,
    /// Whether it can't be used: no typing, opening or picking.
    pub disabled: bool,
    /// Whether the dates can be seen, and the calendar opened, but not
    /// changed.
    pub read_only: bool,
    /// How many months the calendar shows side by side, each in its own
    /// grid. 1 by default.
    pub months: usize,
    /// Whether Previous, Next and keys turn one month, or all the months
    /// shown. `Month` by default.
    pub page_by: PageBy,
    /// The language the inputs read and write dates in, and of the hint, the
    /// month names and the date labels. Defaults to the app's
    /// (`ui_sdk_text::set_page_locale`), then the system's.
    pub locale: Option<SharedString>,
    /// The first day of the week, 0 for Sunday. Defaults to the locale's.
    pub week_starts_on: Option<i64>,
    /// How weekday headers are written. `Short` by default.
    pub weekday_format: TextWidth,
    /// How month names are written. `Long` by default.
    pub month_format: TextWidth,
    /// Whether every month shows six weeks, so the calendar doesn't change
    /// height.
    pub fixed_weeks: bool,
    /// Whether the days of the months before and after show at the edges.
    /// On by default.
    pub show_outside_days: bool,
    /// Today's date, for `aria-current`, the starting month and the year of
    /// a date typed without one. Defaults to today in `time_zone`.
    pub today: Option<ISODate>,
    /// The time zone that says what day today is, like `"America/New_York"`.
    /// Defaults to the system's.
    pub time_zone: Option<SharedString>,
    /// The fewest days a range can span, counting both ends.
    pub min_days: Option<f64>,
    /// The most days a range can span, counting both ends.
    pub max_days: Option<f64>,
    /// Whether a range may hold unavailable dates, but never as an end.
    pub allow_unavailable: bool,
    /// Whether the popup is open. Pass it to control the popup.
    pub open: Option<bool>,
    /// Whether the popup starts open, when uncontrolled.
    pub default_open: bool,
    /// Runs when the popup wants to open or close. `details.reason` says why,
    /// and `details.cancel()` stops it. It runs inside the date picker's
    /// update.
    pub on_open_change: Option<ChangeCallback<bool>>,
    /// Runs once the popup has finished opening or closing, after any
    /// animation.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Whether picking a date, or a range's end, or pressing a preset closes
    /// the popup. On by default.
    pub close_on_select: bool,
    /// How the popup presents. `Sheet` slides it up from the bottom of the
    /// window, and `Auto` does that on small touch screens. `Default`, a
    /// popover, by default.
    pub presentation: PresentationSetting,
    /// The name the date submits under, as an ISO date. In range mode, both
    /// ends submit under it unless `start_name` and `end_name` are set.
    /// Inside a Field, the Field's name when it has none.
    pub name: Option<SharedString>,
    /// The name a range's start submits under.
    pub start_name: Option<SharedString>,
    /// The name a range's end submits under.
    pub end_name: Option<SharedString>,
    /// The id of the form it belongs to, when it isn't inside it.
    pub form: Option<SharedString>,
    /// Whether a date must be entered before its form submits.
    pub required: bool,
    /// Whether Escape, an outside press, focus or the window leaving, the
    /// back gesture and a sheet's swipe can close the popup. The trigger and
    /// the api always can. On by default.
    pub dismissible: bool,
    /// Runs when `dismissible` refused a dismissal made on purpose: Escape,
    /// an outside press, back or a swipe. Focus leaving is refused quietly.
    pub on_dismiss_attempt: Option<DismissAttemptCallback>,
    /// The id a label's `for` names, which names the input that holds the
    /// start, or the date.
    pub id: Option<SharedString>,
    /// What the input that holds the start, or the date, shows while empty.
    pub placeholder: Option<SharedString>,
    /// What a range's end input shows while empty.
    pub end_placeholder: Option<SharedString>,
    /// The colors the inputs' text, caret, selection and placeholder paint
    /// with, their alignment and their width in `ch`.
    pub field_style: TextFieldStyle,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for DatePickerOptions {
    fn default() -> Self {
        Self {
            mode: DatePickerMode::Single,
            value: None,
            default_value: None,
            on_value_change: None,
            month: None,
            default_month: None,
            on_month_change: None,
            min: None,
            max: None,
            unavailable: None,
            disabled: false,
            read_only: false,
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
            open: None,
            default_open: false,
            on_open_change: None,
            on_open_change_complete: None,
            close_on_select: true,
            presentation: PresentationSetting::Default,
            name: None,
            start_name: None,
            end_name: None,
            form: None,
            required: false,
            dismissible: true,
            on_dismiss_attempt: None,
            id: None,
            placeholder: None,
            end_placeholder: None,
            field_style: TextFieldStyle::default(),
            debug: false,
        }
    }
}

impl DatePickerOptions {
    pub fn mode(mut self, mode: DatePickerMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn value(mut self, value: DatePickerValue) -> Self {
        self.value = Some(value);
        self
    }

    pub fn default_value(mut self, value: DatePickerValue) -> Self {
        self.default_value = Some(value);
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&DatePickerValue, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
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
        callback: impl Fn(&ISOMonth, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_month_change = Some(Rc::new(callback));
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

    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    pub fn on_open_change(
        mut self,
        callback: impl Fn(&bool, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn close_on_select(mut self, close: bool) -> Self {
        self.close_on_select = close;
        self
    }

    pub fn presentation(mut self, presentation: PresentationSetting) -> Self {
        self.presentation = presentation;
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn start_name(mut self, name: impl Into<SharedString>) -> Self {
        self.start_name = Some(name.into());
        self
    }

    pub fn end_name(mut self, name: impl Into<SharedString>) -> Self {
        self.end_name = Some(name.into());
        self
    }

    pub fn form(mut self, form: impl Into<SharedString>) -> Self {
        self.form = Some(form.into());
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(
        mut self,
        callback: impl Fn(&ui_sdk_kernel::DismissAttemptDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn end_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.end_placeholder = Some(placeholder.into());
        self
    }

    pub fn field_style(mut self, style: TextFieldStyle) -> Self {
        self.field_style = style;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatePickerState {
    /// The selected value.
    pub value: DatePickerValue,
    /// How it selects (`data-mode`).
    pub mode: DatePickerMode,
    /// Whether the popup is open (`data-open`).
    pub open: bool,
    /// Whether it can't be used, by itself, its field or a fieldset.
    pub disabled: bool,
    /// Whether dates can be seen but not changed.
    pub read_only: bool,
    /// Whether an input holds text that isn't a date that can be picked.
    pub invalid: bool,
}

/// What an input's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatePickerInputState {
    /// Whether its text isn't a date that can be picked, after Enter or
    /// blur.
    pub invalid: bool,
    /// Whether it can't be used.
    pub disabled: bool,
    /// Whether its text can't be changed.
    pub read_only: bool,
}

/// What the trigger's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatePickerTriggerState {
    /// Whether the popup is open.
    pub open: bool,
    /// Whether it can't be pressed.
    pub disabled: bool,
}

/// What the content's `with` reads, with what the original writes as its
/// attributes and variables: its presence, and where it's placed.
#[derive(Clone, Debug, PartialEq)]
pub struct DatePickerContentState {
    /// Whether the popup is open. False while it animates out.
    pub open: bool,
    /// How it presents this time it's open, a popover or a sheet
    /// (`data-presentation`).
    pub presentation: Presentation,
    /// Its enter and exit (`data-starting-style`, `data-ending-style`).
    pub presence: PresenceState,
    /// Where it's placed beside the input, as of the last frame: its side,
    /// alignment, room and transform origin (`data-side`,
    /// `--available-height`, ..).
    pub placement: Option<Placement>,
    /// Kept in the page while closed, it renders hidden.
    pub hidden: bool,
    /// The sheet it presents as, while it does: its values, opacity and
    /// drag.
    pub sheet: Option<SheetPartState>,
}

/// What the format hint's `with` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatePickerHintState {
    /// The format, with a letter for each digit, like "MM/DD/YYYY", or
    /// "TT.MM.JJJJ" in German.
    pub placeholder: SharedString,
    /// December 31 of this year in the format, like "12/31/2026".
    pub example: SharedString,
}

/// What a preset's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatePickerPresetState {
    /// Whether its value is the one selected (`data-selected`).
    pub selected: bool,
    /// Whether it can't be pressed.
    pub disabled: bool,
}

/// What Clear's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatePickerClearState {
    /// Whether there's nothing to clear (`data-empty`).
    pub empty: bool,
    /// Whether it can't be pressed.
    pub disabled: bool,
}

/// The props the `content` slot takes.
#[derive(Clone, Default)]
pub struct DatePickerContentProps {
    /// Where the popup goes beside the input: the side (`Bottom` by default),
    /// how it lines up along that side (`Start` by default), the gap (4 by
    /// default), the shift along the input (0 by default), the collision and
    /// arrow padding, and an anchor of its own.
    pub position: PositionProps,
    /// Where focus goes when the popup closes while focus is inside it.
    /// `Trigger` by default.
    pub final_focus: Option<PopupFocusTarget>,
    /// Whether the popup stays in the page, hidden, while closed. Its
    /// content keeps its state between opens.
    pub keep_mounted: bool,
    /// How it animates in and out, the transitions its CSS would declare.
    pub timing: Option<PresenceTiming>,
    /// The dim behind it while it presents as a sheet, at its darkest.
    /// Black at 0.2 by default.
    pub sheet_backdrop: Option<Hsla>,
}

impl DatePickerContentProps {
    pub fn side(mut self, side: Side) -> Self {
        self.position.side = Some(side);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.position.align = Some(align);
        self
    }

    pub fn side_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.position.side_offset = Some(offset.into());
        self
    }

    pub fn align_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.position.align_offset = Some(offset.into());
        self
    }

    pub fn collision_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.position.collision_padding = Some(padding.into());
        self
    }

    pub fn arrow_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.position.arrow_padding = Some(padding.into());
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.position.anchor = Some(anchor.into());
        self
    }

    pub fn final_focus(mut self, focus: impl Into<PopupFocusTarget>) -> Self {
        self.final_focus = Some(focus.into());
        self
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = Some(timing);
        self
    }

    pub fn sheet_backdrop(mut self, color: impl Into<Hsla>) -> Self {
        self.sheet_backdrop = Some(color.into());
        self
    }

    // What the popup reads from them. Focus moving in is the date picker's
    // own, so `initial_focus` stays the default.
    pub(crate) fn popup_props(&self) -> PopupContentProps {
        PopupContentProps {
            position: self.position.clone(),
            final_focus: self.final_focus.clone(),
            keep_mounted: self.keep_mounted,
            timing: self.timing.clone(),
            sheet_backdrop: self.sheet_backdrop,
            ..PopupContentProps::default()
        }
    }
}

// An input's native text field, the place in the page a form's message
// points at, and where it laid out, which the popup lines up with.
pub(crate) struct PickerInput {
    pub(crate) field: Entity<TextField>,
    pub(crate) node: Node,
    pub(crate) region: Region,
    pub(crate) options: TextFieldOptions,
}

/// A date picker. Its entity is its controller. The calendar's state is
/// Calendar's ([`CalendarModelState`]), its popup is the popups crate's, and
/// each input is a native text field it makes.
pub struct DatePicker {
    pub(crate) instance: Instance,
    pub(crate) model: CalendarModelState,
    pub(crate) popup: Popup<Self>,
    pub(crate) internals: ElementInternals,
    // Text typed in each input, start then end, while it differs from the
    // value it was typed over, which `draft_for` names. A new value from
    // anywhere else leaves the drafts behind.
    pub(crate) drafts: [Option<SharedString>; 2],
    pub(crate) draft_for: String,
    pub(crate) invalid: [bool; 2],
    // The value a form reset goes back to.
    pub(crate) initial: DatePickerValue,
    // Whether the app passes `value` and `default_value`, which a form reset
    // reads.
    pub(crate) controlled: bool,
    pub(crate) has_default: bool,
    pub(crate) disabled: bool,
    pub(crate) close_on_select: bool,
    pub(crate) name: Option<SharedString>,
    pub(crate) start_name: Option<SharedString>,
    pub(crate) end_name: Option<SharedString>,
    pub(crate) form: Option<SharedString>,
    pub(crate) required: bool,
    pub(crate) id: Option<SharedString>,
    pub(crate) placeholders: [Option<SharedString>; 2],
    pub(crate) field_style: TextFieldStyle,
    // With nothing selected, an open goes back to the month the picker
    // started on, like `default_month`.
    pub(crate) first_month: ISOMonth,
    // A controlled value `set_value` asked for, which turns the calendar once
    // the app passes it back, and the value last seen.
    pub(crate) awaited: bool,
    pub(crate) last: DatePickerValue,
    pub(crate) inputs: [PickerInput; 2],
    pub(crate) trigger_focus: FocusHandle,
    pub(crate) trigger_region: Region,
    pub(crate) clear_focus: FocusHandle,
    pub(crate) presets: Parts<SharedString>,
    // The day focus goes to as the popup opens, as the content last rendered
    // it, which the popup reads while the picker may be mid-update.
    pub(crate) focus_target: Rc<RefCell<Option<ISODate>>>,
    pub(crate) _subscriptions: Vec<Subscription>,
}

/// Focus goes to the input that holds the start, or the date.
impl Focusable for DatePicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.inputs[0].field.read(cx).focus_handle(cx)
    }
}

// A value as data: `null`, a date, or a start and an end.
pub(crate) fn value_to_json(value: &DatePickerValue) -> AgentValue {
    match value {
        CalendarValue::Empty => AgentValue::Null,
        CalendarValue::Single(date) => json!(date.to_string()),
        CalendarValue::Multiple(dates) => json!(dates.iter().map(ToString::to_string).collect::<Vec<_>>()),
        CalendarValue::Range(start, end) => json!([start.to_string(), end.to_string()]),
    }
}

// A value from data, which the api reads for the mode.
fn value_from_json(args: &AgentArgs) -> Result<DatePickerValue, AgentError> {
    let value = args.value(0);
    let read = match value {
        AgentValue::Null => Some(CalendarValue::Empty),
        AgentValue::String(date) => Some(CalendarValue::Single(date.clone().into())),
        AgentValue::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(|date| ISODate::from(date.to_string())))
            .collect::<Option<Vec<_>>>()
            .map(CalendarValue::Multiple),
        _ => None,
    };
    read.ok_or_else(|| AgentError::Takes {
        action: args.action().to_string().into(),
        expected: "an ISO date, a start and an end, or null",
        given: value.clone(),
    })
}

/// `agent: { actions: ["setOpen", "setValue", "clear", "setMonth",
/// "previous", "next"], items: () => [] }` and `controlled: ["value",
/// "month", "open"]`, with `open`, `close` and `toggle`, which `setOpen`
/// covers, unlisted.
impl Agent for DatePicker {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", |picker, args, window, cx| {
            let open = args.boolean(0)?;
            Ok(picker.set_open(open, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setValue", |picker, args, window, cx| {
            let value = value_from_json(args)?;
            Ok(picker.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |picker, _args, window, cx| {
            Ok(picker.clear(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setMonth", |picker, args, window, cx| {
            let month = args.string(0)?;
            Ok(picker.set_month(month, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("previous", |picker, _args, window, cx| {
            Ok(picker.previous(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("next", |picker, _args, window, cx| {
            Ok(picker.next(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("open", |picker, _args, window, cx| {
            picker.open(PopupOpenOptions::default(), window, cx);
            Ok(PENDING.into())
        }),
        AgentAction::unlisted("close", |picker, _args, window, cx| {
            Ok(picker.close(PopupCloseOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |picker, _args, window, cx| {
            Ok(picker.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["value", "month", "open"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", value_to_json(self.model.value()))
            .with("month", self.model.month().to_string())
            .with("open", self.popup.is_open())
            .with("disabled", self.is_disabled())
            .with("readOnly", self.model.is_read_only())
            .with("required", self.required)
            .with("min", self.model.min().map(ToString::to_string))
            .with("max", self.model.max().map(ToString::to_string))
    }

    /// Its value is a date, not an item, so a step never opens the calendar
    /// to look for one.
    fn agent_items(&self, _cx: &App) -> Option<Vec<AgentItem>> {
        Some(Vec::new())
    }
}
