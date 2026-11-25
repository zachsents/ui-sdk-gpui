//! Port of `components/date-input/date-input.ts`: a date, or a date and
//! time, typed in segments the locale orders. Values are the ISO strings
//! `<input type="date">` and `datetime-local` submit. Time Input, in
//! `time_input.rs`, is the same input with a time alone.

use std::rc::Rc;

use gpui::{App, Context, Entity, FocusHandle, Focusable, SharedString, Window};
use ui_sdk_calendar::DateMatchers;
use ui_sdk_kernel::{Agent, AgentAction, AgentState, ChangeDetails, ChangeOptions, Instance, Part};
use ui_sdk_native::TextFieldStyle;

use crate::date_input_api::{
    DateInputChangeOptions, DateInputCore, DateInputFieldStyle, DateInputHost, SharedOptions,
};
use crate::date_input_model::{
    DateInputGranularity, DateInputSegmentData, DateInputSegmentType, HourCycle, Kind,
};
use crate::date_input_parts::{self, DateInputSegmentState, DateInputState};

/// The options `DateInput` takes.
#[derive(Clone, Default)]
pub struct DateInputOptions {
    /// The value: an ISO date like `"2026-11-02"`, with a time granularity a
    /// date and time like `"2026-11-02T14:30"`, or `Some(None)` for none.
    /// Pass it to control the input.
    pub value: Option<Option<SharedString>>,
    /// The value it starts with, when uncontrolled, and goes back to when
    /// its form resets.
    pub default_value: Option<Option<SharedString>>,
    /// Runs when the value wants to change: once every segment holds
    /// something, and again with `None` when one is emptied. Its reasons are
    /// `InputChange`, `StepKey`, `Paste`, `Reset` and `Imperative`. It runs
    /// once the input's update is over, so it may update any entity, the
    /// input too.
    pub on_value_change: Option<Rc<dyn Fn(&Option<SharedString>, &ChangeDetails, &mut Window, &mut App)>>,
    /// Runs as the value is asked to change, before it does, inside the
    /// input's update: `details.cancel()` keeps the value. It may update any
    /// entity but the input.
    pub on_before_value_change: Option<Rc<dyn Fn(&Option<SharedString>, &ChangeDetails, &mut Window, &mut App)>>,
    /// The smallest unit it shows. `Day` shows a date, and `Hour`, `Minute`
    /// or `Second` add the time down to it. `Day` by default.
    pub granularity: Option<DateInputGranularity>,
    /// A 12 or 24 hour clock for the time. Defaults to the locale's.
    pub hour_cycle: Option<HourCycle>,
    /// The earliest value, as an ISO date or date and time. A value before
    /// it is invalid.
    pub min: Option<SharedString>,
    /// The latest value. A date alone takes any time that day. A value after
    /// it is invalid.
    pub max: Option<SharedString>,
    /// Dates that can't be chosen, as in Calendar. A value on one is invalid.
    pub unavailable: Option<DateMatchers>,
    /// The language whose order, separators and clock it shows. Defaults to
    /// the page's.
    pub locale: Option<SharedString>,
    /// Whether it can't be used.
    pub disabled: bool,
    /// Whether the value can be seen but not changed.
    pub read_only: bool,
    /// Whether a value must be entered before its form submits.
    pub required: bool,
    /// The name the value submits under.
    pub name: Option<SharedString>,
    /// The id of the form it belongs to, when it isn't inside it.
    pub form: Option<SharedString>,
    /// The style each segment's text field paints with, by the segment's
    /// type, as a stylesheet reaches each segment's `<input>`: its colors,
    /// alignment and width in `ch`.
    pub field_style: Option<DateInputFieldStyle>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl DateInputOptions {
    pub fn value(mut self, value: Option<impl Into<SharedString>>) -> Self {
        self.value = Some(value.map(Into::into));
        self
    }

    pub fn default_value(mut self, value: Option<impl Into<SharedString>>) -> Self {
        self.default_value = Some(value.map(Into::into));
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&Option<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&Option<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn granularity(mut self, granularity: DateInputGranularity) -> Self {
        self.granularity = Some(granularity);
        self
    }

    pub fn hour_cycle(mut self, hour_cycle: HourCycle) -> Self {
        self.hour_cycle = Some(hour_cycle);
        self
    }

    pub fn min(mut self, min: impl Into<SharedString>) -> Self {
        self.min = Some(min.into());
        self
    }

    pub fn max(mut self, max: impl Into<SharedString>) -> Self {
        self.max = Some(max.into());
        self
    }

    pub fn unavailable(mut self, unavailable: impl Into<DateMatchers>) -> Self {
        self.unavailable = Some(unavailable.into());
        self
    }

    pub fn locale(mut self, locale: impl Into<SharedString>) -> Self {
        self.locale = Some(locale.into());
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

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn form(mut self, form: impl Into<SharedString>) -> Self {
        self.form = Some(form.into());
        self
    }

    pub fn field_style(mut self, style: impl Fn(DateInputSegmentType) -> TextFieldStyle + 'static) -> Self {
        self.field_style = Some(Rc::new(style));
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    fn shared(self) -> SharedOptions {
        SharedOptions {
            value: self.value,
            default_value: self.default_value,
            on_value_change: self.on_value_change,
            on_before_value_change: self.on_before_value_change,
            granularity: self.granularity,
            hour_cycle: self.hour_cycle,
            min: self.min,
            max: self.max,
            unavailable: self.unavailable,
            locale: self.locale,
            disabled: self.disabled,
            read_only: self.read_only,
            required: self.required,
            name: self.name,
            form: self.form,
            field_style: self.field_style,
            debug: self.debug,
        }
    }
}

/// A date input. Its entity is its controller.
pub struct DateInput {
    core: DateInputCore,
}

impl DateInput {
    pub fn new(options: DateInputOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            core: DateInputCore::new("date-input", Kind::Date, options.shared(), window, cx),
        }
    }

    /// Hands it new options, as a framework re-renders with new props (`sync`).
    pub fn set_options(&mut self, options: DateInputOptions, cx: &mut Context<Self>) {
        self.core.sync(&options.shared(), cx);
    }

    /// Passes the controlled value, `Some(None)` for none, or `None` to let
    /// the input keep its own.
    pub fn set_controlled_value(&mut self, value: Option<Option<SharedString>>, cx: &mut Context<Self>) {
        self.core.set_controlled_value(value, cx);
    }

    pub fn instance(&self) -> &Instance {
        self.core.instance()
    }

    /// Its state and api, which Time Input shares.
    pub fn core(&self) -> &DateInputCore {
        &self.core
    }

    pub fn state(&self, cx: &App) -> DateInputState {
        self.core.state(cx)
    }

    /// The value, or `None` while empty or partly filled.
    pub fn get_value(&self) -> Option<&SharedString> {
        self.core.get_value()
    }

    /// Sets the value. Returns false when it isn't a value the input can
    /// show, or when canceled.
    pub fn set_value(
        &mut self,
        value: Option<SharedString>,
        options: DateInputChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.core.set_value(value, options, window, cx)
    }

    /// Empties every segment.
    pub fn clear(&mut self, options: DateInputChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.core.clear(options, window, cx)
    }

    /// Focuses a segment, like `"month"`, or the first one.
    pub fn focus(&mut self, segment: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.core.focus(segment, window, cx);
    }

    /// The `root` part: a group around the segments.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<DateInputState> {
        date_input_parts::root(this, cx)
    }

    /// A `segment` part, for one of the root state's `segments`.
    pub fn segment(this: &Entity<Self>, segment: &DateInputSegmentData, cx: &App) -> Part<DateInputSegmentState> {
        date_input_parts::segment(this, segment, cx)
    }
}

impl DateInputHost for DateInput {
    fn date_input(&self) -> &DateInputCore {
        &self.core
    }

    fn date_input_mut(&mut self) -> &mut DateInputCore {
        &mut self.core
    }
}

/// Focus goes to the first segment.
impl Focusable for DateInput {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.core.focus_handle(cx)
    }
}

/// `agent: { actions: ["setValue", "clear"] }`.
impl Agent for DateInput {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |input, args, window, cx| {
            let value = if args.is_missing(0) { None } else { Some(args.string(0)?) };
            Ok(input.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |input, _args, window, cx| {
            Ok(input.clear(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        self.core.agent_state()
    }
}
