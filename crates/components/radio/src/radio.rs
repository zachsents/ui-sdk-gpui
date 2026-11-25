//! Port of `components/radio/radio.ts`: radios that share one value, with
//! one tab stop and arrow keys between them, which check as they move.
//!
//! The group's state and api are `radio_api.rs`'s [`RadioGroupApi`], and its
//! group and radio parts `radio_parts.rs`'s, which Segmented Control and
//! Rating share. What's Radio's own is the indicator: the dot inside a
//! checked radio, which stays in the page while it animates out.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, Context, Entity, SharedString, Window};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentSurface, ChangeCallback, ChangeDetails, ChangeOptions,
    Instance, Part, Reason,
};
use ui_sdk_motion::{Presence, PresenceState, PresenceTiming};

use crate::radio_api::{RadioGroupApi, RadioHost, RadioSlotNames};
use crate::radio_parts::{radio_group, radio_item};

/// Why a radio group's value wants to change: a radio was pressed, or a
/// label naming it (`ItemPress`), an arrow key moved to it
/// (`ListNavigation`), its form reset (`Reset`), or a call
/// (`Imperative`).
pub type RadioGroupValueChangeReason = Reason;

/// Options for every write method.
pub type RadioGroupChangeOptions = ChangeOptions;

/// The options `RadioGroup` takes.
#[derive(Clone)]
pub struct RadioGroupOptions {
    /// The checked radio's value. Pass it to control the group. `Some(None)`
    /// checks nothing.
    pub value: Option<Option<SharedString>>,
    /// The radio checked at first, when uncontrolled. A form reset returns
    /// to it.
    pub default_value: Option<Option<SharedString>>,
    /// Runs when a user or a form reset wants to change the value.
    /// `details.reason` says why (`ItemPress`, `ListNavigation`, `Reset`,
    /// `Imperative`). It runs once the group's update is over, so it may
    /// update any entity, the group too.
    pub on_value_change: Option<ChangeCallback<Option<SharedString>>>,
    /// Runs as the value is asked to change, before it does, inside the
    /// group's update: `details.cancel()` stops it. It may update any entity
    /// but the group.
    pub on_before_value_change: Option<ChangeCallback<Option<SharedString>>>,
    /// Disables every radio like native ones: they can't be focused or
    /// changed, and the value isn't submitted.
    pub disabled: bool,
    /// Keeps the radios focusable and the value submitted, but stops
    /// changes.
    pub read_only: bool,
    /// Blocks form submission until a radio is checked, with the browser's
    /// own message.
    pub required: bool,
    /// The name the value submits under. Without one, the group submits
    /// nothing.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when the group isn't inside that form.
    pub form: Option<SharedString>,
    /// Whether arrow keys wrap from the last radio to the first. Defaults to
    /// `true`.
    pub r#loop: bool,
    /// How indicators animate in and out, as their CSS transition would
    /// declare. The default has none, so they show and hide at once.
    pub indicator_timing: PresenceTiming,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for RadioGroupOptions {
    fn default() -> Self {
        Self {
            value: None,
            default_value: None,
            on_value_change: None,
            on_before_value_change: None,
            disabled: false,
            read_only: false,
            required: false,
            name: None,
            form: None,
            r#loop: true,
            indicator_timing: PresenceTiming::default(),
            debug: false,
        }
    }
}

impl RadioGroupOptions {
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

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
        self
    }

    pub fn indicator_timing(mut self, timing: PresenceTiming) -> Self {
        self.indicator_timing = timing;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `root` part, a radio, takes beside its value, which the
/// group's value becomes when it's checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RadioProps {
    /// Skips the radio, like a disabled native radio: arrow keys pass it and
    /// it can't be focused or checked.
    pub disabled: bool,
}

impl RadioProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The props the `indicator` part takes beside its radio's value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RadioIndicatorProps {
    /// Whether the indicator stays in the page while its radio is unchecked.
    pub keep_mounted: bool,
}

impl RadioIndicatorProps {
    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }
}

/// The state the `group` part hands `with`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadioGroupState {
    /// The checked radio's value.
    pub value: Option<SharedString>,
    /// Whether the group is disabled, by itself, its field or a fieldset.
    pub disabled: bool,
    /// Whether the group is read-only: arrow keys still move focus, but
    /// nothing changes the checked radio.
    pub read_only: bool,
    /// Whether a form won't submit until a radio is checked.
    pub required: bool,
}

/// The state a radio hands `with`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadioState {
    /// Whether this radio is checked.
    pub checked: bool,
    /// Whether this radio is disabled, by itself, its group, a field or a
    /// fieldset.
    pub disabled: bool,
    /// Whether the group is read-only.
    pub read_only: bool,
    /// Whether the group is required.
    pub required: bool,
}

/// The state the `indicator` part hands `with`: its radio's, with the
/// indicator's presence, which the original's `data-starting-style` and
/// `data-ending-style` carry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadioIndicatorState {
    /// Whether its radio is checked.
    pub checked: bool,
    /// Whether its radio is disabled, by itself, its group, a field or a
    /// fieldset.
    pub disabled: bool,
    /// Whether the group is read-only.
    pub read_only: bool,
    /// Whether the group is required.
    pub required: bool,
    /// The indicator's presence: its phase, the starting and ending styles,
    /// and how far it shows.
    pub presence: PresenceState,
}

/// A radio group (`radioGroup`). Its entity is its controller.
pub struct RadioGroup {
    radio: RadioGroupApi,
    // Each indicator's presence, by its radio's value, made as it first
    // animates.
    indicators: HashMap<SharedString, Presence>,
    indicator_timing: PresenceTiming,
    // Each indicator's `keep_mounted`, as it last rendered.
    keep_mounted: RefCell<HashMap<SharedString, bool>>,
}

impl RadioHost for RadioGroup {
    fn radio(&self) -> &RadioGroupApi {
        &self.radio
    }

    fn radio_mut(&mut self) -> &mut RadioGroupApi {
        &mut self.radio
    }

    // What each indicator's subscription does in the original: the checked
    // radio's animates in, and the one it replaces animates out while it's
    // still in the page, which keeps it there until it's done.
    fn value_changed(&mut self, from: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(to) = self.radio.get_value() {
            self.presence(&to, false).animate_in(window, cx);
        }
        let Some(from) = from else {
            return;
        };
        let kept = self.keep_mounted.borrow().get(&from).copied().unwrap_or(false);
        if self.radio.exiting().contains(&from) || kept {
            let presence = self.presence(&from, true);
            presence.animate_out(
                self,
                move |group, _window, cx| group.finish_exit(&from, cx),
                window,
                cx,
            );
        } else if let Some(presence) = self.indicators.get(&from) {
            presence.set_open(false, cx);
        }
    }
}

impl RadioGroup {
    pub fn new(options: RadioGroupOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("radio").with_agent(cx);
        instance.debug = options.debug;
        let radio = RadioGroupApi::new(instance, &options, RadioSlotNames::default(), window, cx);
        Self {
            radio,
            indicators: HashMap::new(),
            indicator_timing: options.indicator_timing,
            keep_mounted: RefCell::new(HashMap::new()),
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A controlled `value` moves the indicators in the same pass.
    pub fn set_options(&mut self, options: RadioGroupOptions, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self.radio.set_options(&options);
        for presence in self.indicators.values() {
            presence.set_timing(options.indicator_timing.clone());
        }
        self.indicator_timing = options.indicator_timing;
        self.sync_radio(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, `Some(None)` to check nothing, or `None`
    /// to let the group keep its own.
    pub fn set_controlled_value(
        &mut self,
        value: Option<Option<SharedString>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.radio.set_controlled_value(value) {
            cx.notify();
        }
        self.sync_radio(window, cx);
    }

    pub fn instance(&self) -> &Instance {
        self.radio.instance()
    }

    /// The group's state and api, which its parts share with Segmented
    /// Control's and Rating's.
    pub fn api(&self) -> &RadioGroupApi {
        &self.radio
    }

    /// What the group part's state holds.
    pub fn state(&self) -> RadioGroupState {
        RadioGroupState {
            value: self.radio.get_value(),
            disabled: self.radio.is_disabled(),
            read_only: self.radio.is_read_only(),
            required: self.radio.is_required(),
        }
    }

    /// The checked radio's value.
    pub fn get_value(&self) -> Option<SharedString> {
        self.radio.get_value()
    }

    /// Checks a radio, as a user would, or with `None` none. Disabled radios
    /// and read-only groups ignore it. Returns false when canceled.
    pub fn set_value(
        &mut self,
        value: Option<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.set_radio_value(value, options, window, cx)
    }

    /// Moves focus to the checked radio, or the first one.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.radio.focus(window, cx);
    }

    /// Whether the value changed since the group was made.
    pub fn is_changed(&self) -> bool {
        self.radio.is_changed()
    }

    // The presence of the indicator for `key`, made at rest, shown or not.
    fn presence(&mut self, key: &SharedString, shown: bool) -> Presence {
        let timing = &self.indicator_timing;
        self.indicators
            .entry(key.clone())
            .or_insert_with(|| Presence::new(shown).with_timing(timing.clone()))
            .clone()
    }

    fn finish_exit(&mut self, key: &SharedString, cx: &mut Context<Self>) {
        self.radio.finish_exit(key);
        cx.notify();
    }

    /// The `group` part, around the radios: a radio group, which a label or
    /// field names, with its form side. Build it before the radios: it
    /// starts their render, so the order they render in is page order.
    pub fn group(this: &Entity<Self>, cx: &App) -> Part<RadioGroupState> {
        radio_group(this, this.read(cx).state(), cx)
    }

    /// The `root` part: one radio, checked when the group's value is
    /// `value`. Values identify the radios, so each is unique. Put its
    /// indicator inside it, and name it with a label around it, a label
    /// pointing at its element, or `aria_label`.
    pub fn root(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: RadioProps,
        cx: &App,
    ) -> Part<RadioState> {
        radio_item(this, value, props, cx)
    }

    /// The `indicator` part inside the radio with `value`, or `None` while
    /// it's out of the page: it's in the page while its radio is checked,
    /// while it animates out, and always with `keep_mounted`.
    pub fn indicator(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: RadioIndicatorProps,
        cx: &App,
    ) -> Option<Part<RadioIndicatorState>> {
        let value = value.into();
        let group = this.read(cx);
        group
            .keep_mounted
            .borrow_mut()
            .insert(value.clone(), props.keep_mounted);
        let radio = &group.radio;
        let item = radio.item_state(&value);
        let present = props.keep_mounted || item.checked || radio.exiting().contains(&value);
        if !present {
            return None;
        }
        let presence = match group.indicators.get(&value) {
            Some(presence) => presence.state(),
            None => Presence::new(item.checked).state(),
        };
        let state = RadioIndicatorState {
            checked: item.checked,
            disabled: item.disabled,
            read_only: item.read_only,
            required: item.required,
            presence,
        };
        Some(Part::new(radio.instance(), "indicator", Some(&value), state))
    }
}

/// `agent: { actions: ["setValue"], closed: true }`.
impl Agent for RadioGroup {
    const SURFACE: AgentSurface = AgentSurface::new().closed(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[AgentAction::new("setValue", |group, args, window, cx| {
        let value = if args.is_missing(0) {
            None
        } else {
            Some(args.string(0)?)
        };
        Ok(group.set_value(value, ChangeOptions::default(), window, cx).into())
    })];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", self.radio.get_value().map(|value| value.to_string()))
            .with("disabled", self.radio.is_disabled())
            .with("readOnly", self.radio.is_read_only())
            .with("required", self.radio.is_required())
    }
}
