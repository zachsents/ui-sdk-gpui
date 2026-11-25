//! Port of `components/segmented-control/segmented-control.ts`: one of a few
//! options picked from a row of buttons, with an indicator that slides to
//! the choice.
//!
//! A segmented control is a radio group, on `ui-sdk-radio`'s api and parts:
//! one value, one tab stop, arrow keys on both axes that move and check,
//! Space to check, Enter to submit its form. What's its own is the
//! orientation, for styling and screen readers, and the indicator, which
//! measures the checked item and says which way it last moved.

use std::rc::Rc;

use gpui::{App, Context, Entity, SharedString, Window};
use ui_sdk_focus::{Direction, Orientation};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentSurface, ChangeCallback, ChangeDetails, ChangeOptions, Instance,
    Part, Reason,
};
use ui_sdk_platform::{LayoutBox, layout_box};
use ui_sdk_radio::{
    RadioGroupApi, RadioGroupOptions, RadioHost, RadioProps, RadioSlotNames, RadioState, radio_group,
    radio_item,
};
use ui_sdk_selection::{Indicator, IndicatorOptions, bind_indicator, direction_between};

/// Which way the items run.
pub type SegmentedControlOrientation = Orientation;

/// Why a segmented control's value wants to change: an item was pressed,
/// or a label naming it (`ItemPress`), an arrow key moved to it
/// (`ListNavigation`), its form reset (`Reset`), or a call (`Imperative`).
pub type SegmentedControlValueChangeReason = Reason;

/// Options for every write method.
pub type SegmentedControlChangeOptions = ChangeOptions;

/// Which way the checked item moved, on screen.
pub type SegmentedControlDirection = Direction;

/// The props the `item` part takes beside its value, which the control's
/// value becomes when it's checked: `disabled` skips the item, like a
/// disabled native radio, so arrow keys pass it and it can't be focused or
/// checked.
pub type SegmentedControlItemProps = RadioProps;

/// What an item's `with` reads: whether it's checked, which the indicator
/// sits behind, and whether it's disabled, by itself, the control, a field
/// or a fieldset, read-only or required.
pub type SegmentedControlItemState = RadioState;

/// The options `SegmentedControl` takes.
#[derive(Clone)]
pub struct SegmentedControlOptions {
    /// The checked item's value. Pass it to control the segmented control.
    /// `Some(None)` checks nothing.
    pub value: Option<Option<SharedString>>,
    /// The item checked at first, when uncontrolled. A form reset returns to
    /// it.
    pub default_value: Option<Option<SharedString>>,
    /// Runs when a user or a form reset wants to change the value.
    /// `details.reason` says why (`ItemPress`, `ListNavigation`, `Reset`,
    /// `Imperative`). It runs once the control's update is over, so it may
    /// update any entity, the control too.
    pub on_value_change: Option<ChangeCallback<Option<SharedString>>>,
    /// Runs as the value is asked to change, before it does, inside the
    /// control's update: `details.cancel()` stops it. It may update any
    /// entity but the control.
    pub on_before_value_change: Option<ChangeCallback<Option<SharedString>>>,
    /// Disables every item: they can't be focused or changed, and the value
    /// isn't submitted.
    pub disabled: bool,
    /// Keeps the items focusable and the value submitted, but stops changes.
    pub read_only: bool,
    /// Blocks form submission until an item is checked, with the browser's
    /// own message.
    pub required: bool,
    /// The name the value submits under. Without one, the segmented control
    /// submits nothing.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when the segmented control isn't
    /// inside that form.
    pub form: Option<SharedString>,
    /// Whether arrow keys wrap from the last item to the first. Defaults to
    /// `true`.
    pub r#loop: bool,
    /// Which way the items run, for styling and for screen readers. Arrow
    /// keys on both axes move between items either way, as in any radio
    /// group. Defaults to horizontal.
    pub orientation: SegmentedControlOrientation,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for SegmentedControlOptions {
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
            orientation: Orientation::Horizontal,
            debug: false,
        }
    }
}

impl SegmentedControlOptions {
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

    pub fn orientation(mut self, orientation: SegmentedControlOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentedControlState {
    /// The checked item's value.
    pub value: Option<SharedString>,
    /// Whether the segmented control is disabled, by itself, its field or a
    /// fieldset.
    pub disabled: bool,
    /// Whether it's read-only: arrow keys still move focus, but nothing
    /// changes the checked item.
    pub read_only: bool,
    /// Whether a form won't submit until an item is checked.
    pub required: bool,
    /// Which way the items run.
    pub orientation: SegmentedControlOrientation,
}

/// What the indicator's `with` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentedControlIndicatorState {
    /// The checked item's value, which the indicator sits behind.
    pub value: Option<SharedString>,
    /// Which way the items run.
    pub orientation: SegmentedControlOrientation,
    /// Which way the checked item last moved on screen, `None` before it
    /// first moves (`data-activation-direction`). Skip the indicator's
    /// animation while it's `None`, so it doesn't slide in on its first
    /// placement.
    pub direction: SegmentedControlDirection,
    /// The checked item's box inside the control
    /// (`--segmented-control-indicator-*`), or `None` while nothing is
    /// checked or before the items lay out. Place the indicator with it.
    pub item_box: Option<LayoutBox>,
}

const SLOT_NAMES: RadioSlotNames = RadioSlotNames {
    group: "root",
    item: "item",
};

// A segmented control's options, as the radio group it is takes them.
fn radio_options(options: &SegmentedControlOptions) -> RadioGroupOptions {
    RadioGroupOptions {
        value: options.value.clone(),
        default_value: options.default_value.clone(),
        on_value_change: options.on_value_change.clone(),
        on_before_value_change: options.on_before_value_change.clone(),
        disabled: options.disabled,
        read_only: options.read_only,
        required: options.required,
        name: options.name.clone(),
        form: options.form.clone(),
        r#loop: options.r#loop,
        debug: options.debug,
        ..RadioGroupOptions::default()
    }
}

/// A segmented control (`segmentedControl`). Its entity is its controller.
pub struct SegmentedControl {
    radio: RadioGroupApi,
    // Measures the control and its items, for the indicator over the
    // checked one.
    indicator: Indicator<SharedString>,
}

impl RadioHost for SegmentedControl {
    fn radio(&self) -> &RadioGroupApi {
        &self.radio
    }

    fn radio_mut(&mut self) -> &mut RadioGroupApi {
        &mut self.radio
    }

    // What the indicator's subscription does in the original: which way the
    // checked item moved on screen, set ahead of the render, so styles that
    // skip the animation while the direction is `None` animate the first
    // move too.
    fn value_changed(&mut self, from: Option<SharedString>, _window: &mut Window, cx: &mut Context<Self>) {
        let to = self.radio.get_value();
        let direction = direction_between(
            self.item_box(from.as_ref()).as_ref(),
            self.item_box(to.as_ref()).as_ref(),
            self.orientation(),
        );
        if self.radio.set_direction(Some(direction)) {
            cx.notify();
        }
    }
}

impl SegmentedControl {
    pub fn new(options: SegmentedControlOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("segmented-control").with_agent(cx);
        instance.debug = options.debug;
        let mut radio = RadioGroupApi::new(instance, &radio_options(&options), SLOT_NAMES, window, cx);
        radio.set_orientation(Some(options.orientation));
        radio.set_direction(Some(Direction::None));
        Self {
            radio,
            indicator: bind_indicator(IndicatorOptions::new(), cx),
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A controlled `value` moves the indicator in the same pass.
    pub fn set_options(
        &mut self,
        options: SegmentedControlOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut changed = self.radio.set_options(&radio_options(&options));
        changed |= self.radio.set_orientation(Some(options.orientation));
        self.sync_radio(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, `Some(None)` to check nothing, or `None`
    /// to let the control keep its own.
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

    /// The control's radio group, which its parts run on.
    pub fn api(&self) -> &RadioGroupApi {
        &self.radio
    }

    /// Which way the items run.
    pub fn orientation(&self) -> SegmentedControlOrientation {
        self.radio.orientation().unwrap_or(Orientation::Horizontal)
    }

    /// What the root part's state holds.
    pub fn state(&self) -> SegmentedControlState {
        SegmentedControlState {
            value: self.radio.get_value(),
            disabled: self.radio.is_disabled(),
            read_only: self.radio.is_read_only(),
            required: self.radio.is_required(),
            orientation: self.orientation(),
        }
    }

    /// The checked item's value.
    pub fn get_value(&self) -> Option<SharedString> {
        self.radio.get_value()
    }

    /// Checks an item, as a user would, or with `None` none. Disabled items
    /// and read-only controls ignore it. Returns false when canceled.
    pub fn set_value(
        &mut self,
        value: Option<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.set_radio_value(value, options, window, cx)
    }

    /// Moves focus to the checked item, or the first one.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.radio.focus(window, cx);
    }

    // An item's box inside the control, as of the last layout (`rect`).
    fn item_box(&self, key: Option<&SharedString>) -> Option<LayoutBox> {
        let item = self.radio.radio_bounds(key?)?;
        let group = self.radio.group_bounds()?;
        Some(layout_box(item, group, None))
    }

    /// The `root` part, around the items: a radio group, which `aria_label`,
    /// a label or a field names, with its form side. Build it before the
    /// items: it starts their render, so the order they render in is page
    /// order. The indicator's box is relative to it.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<SegmentedControlState> {
        let control = this.read(cx);
        let part = radio_group(this, control.state(), cx);
        control.indicator.list(part)
    }

    /// The `item` part: one choice, checked when the control's value is
    /// `value`. Values identify the items, so each is unique. Name it with
    /// `aria_label`.
    pub fn item(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: SegmentedControlItemProps,
        cx: &App,
    ) -> Part<SegmentedControlItemState> {
        let value = value.into();
        let part = radio_item(this, value.clone(), props, cx);
        this.read(cx).indicator.item(value, part)
    }

    /// The `indicator` part: the background that slides to the checked item,
    /// which screen readers skip. Render it inside the root, ahead of the
    /// items so they paint over it, and place it from its state's
    /// `item_box`.
    pub fn indicator(this: &Entity<Self>, cx: &App) -> Part<SegmentedControlIndicatorState> {
        let control = this.read(cx);
        let value = control.radio.get_value();
        let state = SegmentedControlIndicatorState {
            item_box: control.indicator.layout_box(value.as_ref()),
            value,
            orientation: control.orientation(),
            direction: control.radio.direction().unwrap_or(Direction::None),
        };
        Part::new(control.radio.instance(), "indicator", None, state).aria_role(Role::Presentation)
    }
}

/// `agent: { actions: ["setValue"], closed: true }`.
impl Agent for SegmentedControl {
    const SURFACE: AgentSurface = AgentSurface::new().closed(true);

    const ACTIONS: &'static [AgentAction<Self>] =
        &[AgentAction::new("setValue", |control, args, window, cx| {
            let value = if args.is_missing(0) {
                None
            } else {
                Some(args.string(0)?)
            };
            Ok(control
                .set_value(value, ChangeOptions::default(), window, cx)
                .into())
        })];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", self.radio.get_value().map(|value| value.to_string()))
            .with("disabled", self.radio.is_disabled())
            .with("readOnly", self.radio.is_read_only())
            .with("required", self.radio.is_required())
    }
}
