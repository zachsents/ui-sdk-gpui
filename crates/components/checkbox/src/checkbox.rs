//! Port of `components/checkbox/checkbox.ts`: a control checked, unchecked or
//! indeterminate, alone or in a group.
//!
//! The forms crate's [`ToggleControl`] holds the checked state and its form
//! side. Inside a group, the group decides whether it's checked, its name and
//! whether it's disabled, and a parent checkbox stands for the group.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Context, ElementId, Entity, EntityId, FocusHandle, Focusable, SharedString, Subscription,
    WeakEntity, Window,
};
use ui_sdk_forms::{
    CheckboxState, CheckedState, ElementInternals, Form, ToggleControl, ToggleGroup, ToggleHost,
    ToggleOptions, ToggleRole, checkbox_state,
};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentValue, Aria, ChangeCallback, ChangeDetails, ChangeOptions,
    Instance, Part, Reason, held, while_destroying,
};
use ui_sdk_motion::{Presence, PresenceState, PresenceTiming};

use crate::checkbox_group::{CheckboxGroup, Child};

/// The options `Checkbox` takes.
#[derive(Clone, Default)]
pub struct CheckboxOptions {
    /// Whether it's checked: checked, unchecked or indeterminate. Pass it to
    /// control the checkbox.
    pub checked: Option<CheckedState>,
    /// Whether it's checked at first, when uncontrolled. A form reset returns
    /// to it.
    pub default_checked: Option<CheckedState>,
    /// Runs when a user or a form reset wants to change it. `details.reason`
    /// is `Press`, `Reset` or `Imperative`, and `details.cancel()` stops it. It
    /// runs inside the checkbox's update, so it may update any entity but the
    /// checkbox.
    pub on_checked_change: Option<ChangeCallback<CheckedState>>,
    /// Disables it like a native control: it can't be focused or changed, and
    /// isn't submitted.
    pub disabled: bool,
    /// Keeps it focusable and submitted, but stops changes.
    pub read_only: bool,
    /// Blocks form submission until it's checked, with the browser's own
    /// message.
    pub required: bool,
    /// The name it submits under. Without one, it submits nothing.
    pub name: Option<SharedString>,
    /// What it submits while checked, `"on"` by default. In a group, the value
    /// the group's value lists while it's checked.
    pub value: Option<SharedString>,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
    /// Makes it the parent of its group: it checks or unchecks every checkbox
    /// in the group, and shows checked, unchecked or indeterminate from them.
    pub parent: bool,
    /// The group it belongs to, which decides whether it's checked, its form
    /// and whether it's disabled (what adapters find from context).
    pub group: Option<Entity<CheckboxGroup>>,
    /// How the indicator animates in and out, as its CSS transition would
    /// declare. The default has none, so it shows and hides at once.
    pub indicator_timing: PresenceTiming,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl CheckboxOptions {
    pub fn checked(mut self, checked: impl Into<CheckedState>) -> Self {
        self.checked = Some(checked.into());
        self
    }

    pub fn default_checked(mut self, checked: impl Into<CheckedState>) -> Self {
        self.default_checked = Some(checked.into());
        self
    }

    pub fn on_checked_change(
        mut self,
        callback: impl Fn(&CheckedState, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_checked_change = Some(Rc::new(callback));
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

    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn form(mut self, form: impl Into<SharedString>) -> Self {
        self.form = Some(form.into());
        self
    }

    pub fn parent(mut self, parent: bool) -> Self {
        self.parent = parent;
        self
    }

    pub fn group(mut self, group: &Entity<CheckboxGroup>) -> Self {
        self.group = Some(group.clone());
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

/// The props the `indicator` part takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckboxIndicatorProps {
    /// Whether the indicator stays in the page while unchecked, so it can be
    /// styled or animated in both states.
    pub keep_mounted: bool,
}

impl CheckboxIndicatorProps {
    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }
}

/// The state the `indicator` part hands `with`: its checkbox's, with the
/// indicator's presence, which the original's `data-starting-style` and
/// `data-ending-style` carry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CheckboxIndicatorState {
    /// Whether it's checked. False while indeterminate.
    pub checked: bool,
    /// Whether it's indeterminate.
    pub indeterminate: bool,
    /// Whether it's disabled, by itself, its field or a fieldset.
    pub disabled: bool,
    /// Whether it's read-only.
    pub read_only: bool,
    /// Whether a form won't submit until it's checked.
    pub required: bool,
    /// The indicator's presence: its phase, the starting and ending styles,
    /// and how far it shows.
    pub presence: PresenceState,
}

// The group a checkbox joined, and its following of the group's value.
struct Membership {
    group: WeakEntity<CheckboxGroup>,
    value: SharedString,
    parent: bool,
    checkbox: EntityId,
    _follow: Subscription,
}

// The checkbox's side of its group (`toggleControl`'s `group`): the group
// decides, and its value then decides the checkbox.
struct GroupLink {
    group: WeakEntity<CheckboxGroup>,
    internals: ElementInternals,
    value: SharedString,
    parent: bool,
}

impl ToggleGroup for GroupLink {
    fn form_owner(&self, _cx: &App) -> Option<Form> {
        self.internals.form()
    }

    fn write(
        &self,
        control: &mut ToggleControl,
        checked: CheckedState,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(group) = self.group.upgrade() else {
            return control.change_checked(checked, options, window, cx);
        };
        let reason = match options.reason {
            Some(Reason::Press) => Reason::ItemPress,
            reason => reason.unwrap_or(Reason::Imperative),
        };
        let for_group = ChangeOptions {
            reason: Some(reason),
            event: options.event.clone(),
            source: options.source,
        };
        let parent = self.parent;
        let value = self.value.clone();
        let accepted = group.update(cx, |group, cx| {
            if parent {
                // A mixed parent checks the rest. When only disabled ones are
                // left unchecked, there's nothing to check, so it unchecks
                // instead.
                let check = checked == CheckedState::True && !group.all_enabled_checked();
                group.set_all(check, for_group, window, cx)
            } else {
                group.set_child(value, checked == CheckedState::True, for_group, window, cx)
            }
        });
        if !accepted {
            return false;
        }
        let next = {
            let group = group.read(cx);
            if parent {
                group.parent_checked()
            } else {
                group.checked(&self.value).into()
            }
        };
        control.change_checked(next, options, window, cx);
        true
    }
}

/// A checkbox. Its entity is its controller.
pub struct Checkbox {
    control: ToggleControl,
    options: CheckboxOptions,
    membership: Option<Membership>,
    // The indicator is animating out.
    exiting: bool,
    // Changes after mount animate the indicator. The first paint never does.
    changed: bool,
    // The checked state the last change saw.
    previous: CheckedState,
    indicator: Presence,
    // The indicator's `keep_mounted`, as it last rendered.
    keep_mounted: Cell<bool>,
    _subscriptions: Vec<Subscription>,
}

impl ToggleHost for Checkbox {
    fn toggle_control(&self) -> &ToggleControl {
        &self.control
    }

    fn toggle_control_mut(&mut self) -> &mut ToggleControl {
        &mut self.control
    }
}

// What the control takes from options, and inside a group, from the group
// (`fromGroup`, `checkboxSync`).
fn toggle_options(options: &CheckboxOptions, cx: &App) -> ToggleOptions {
    let mut toggle = ToggleOptions {
        checked: options.checked,
        default_checked: options.default_checked,
        on_checked_change: options.on_checked_change.clone(),
        disabled: options.disabled,
        read_only: options.read_only,
        required: options.required,
        name: options.name.clone(),
        value: options.value.clone(),
        form: options.form.clone(),
    };
    let Some(group) = &options.group else {
        return toggle;
    };
    let group = group.read(cx);
    let value = options.value.clone().unwrap_or_else(|| "on".into());
    toggle.checked = Some(if options.parent {
        group.parent_checked()
    } else {
        group.checked(&value).into()
    });
    // The group submits every checked value under its name, so a checkbox
    // in it submits only under a name of its own. A parent stands for the
    // others, so it submits nothing.
    toggle.name = if options.parent { None } else { options.name.clone() };
    toggle.disabled = options.disabled || group.is_disabled();
    toggle.read_only = options.read_only || group.is_read_only();
    toggle.form = options.form.clone().or_else(|| group.form_attribute());
    toggle.value = Some(value);
    toggle
}

impl Checkbox {
    pub fn new(options: CheckboxOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("checkbox").with_agent(cx);
        instance.debug = options.debug;
        let control = ToggleControl::new(instance, toggle_options(&options, cx), cx);
        let previous = control.get_checked();
        let indicator = Presence::new(previous != CheckedState::False).with_timing(options.indicator_timing.clone());
        // A press, a label's or a form reset changes the control in place:
        // the indicator hears it before the next frame.
        let this = cx.entity();
        let changes = cx.observe_in(&this, window, |checkbox, _, window, cx| checkbox.after_change(window, cx));
        let release = cx.on_release(|checkbox, cx| while_destroying(|| checkbox.leave_group(cx)));
        let mut checkbox = Self {
            control,
            options,
            membership: None,
            exiting: false,
            changed: false,
            previous,
            indicator,
            keep_mounted: Cell::new(false),
            _subscriptions: vec![changes, release],
        };
        checkbox.join_group(window, cx);
        checkbox
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A controlled `checked` starts the indicator's exit in the
    /// same pass.
    pub fn set_options(&mut self, options: CheckboxOptions, window: &mut Window, cx: &mut Context<Self>) {
        let rejoin = self.options.group != options.group
            || self.options.parent != options.parent
            || self.options.value != options.value;
        if rejoin {
            self.leave_group(cx);
        }
        self.options = options;
        if rejoin {
            self.join_group(window, cx);
        }
        let changed = self.control.set_options(toggle_options(&self.options, cx));
        self.indicator.set_timing(self.options.indicator_timing.clone());
        self.control.instance_mut().debug = self.options.debug;
        self.after_change(window, cx);
        if changed || rejoin {
            cx.notify();
        }
    }

    /// Passes the controlled state, or `None` to let the checkbox keep its
    /// own. Inside a group, the group's value decides.
    pub fn set_controlled_checked(&mut self, checked: Option<CheckedState>, window: &mut Window, cx: &mut Context<Self>) {
        self.options.checked = checked;
        if self.control.set_options(toggle_options(&self.options, cx)) {
            cx.notify();
        }
        self.after_change(window, cx);
    }

    pub fn instance(&self) -> &Instance {
        self.control.instance()
    }

    /// What the root part's state holds.
    pub fn state(&self) -> CheckboxState {
        checkbox_state(&self.control)
    }

    /// Its checked state and form side.
    pub fn control(&self) -> &ToggleControl {
        &self.control
    }

    /// Whether it's checked: checked, unchecked or indeterminate.
    pub fn get_checked(&self) -> CheckedState {
        self.control.get_checked()
    }

    /// Sets whether it's checked, as a user would. Disabled and read-only
    /// checkboxes ignore it. Returns false when canceled.
    pub fn set_checked(
        &mut self,
        checked: impl Into<CheckedState>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let done = self.control.set_checked(checked.into(), options, window, cx);
        self.after_change(window, cx);
        done
    }

    /// Checks it, or unchecks it. An indeterminate checkbox becomes checked.
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let done = self.control.toggle(options, window, cx);
        self.after_change(window, cx);
        done
    }

    /// Moves focus to it.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.control.focus(window, cx);
    }

    /// The form it belongs to, or its group's.
    pub fn form(&self, cx: &App) -> Option<Form> {
        self.control.form(cx)
    }

    /// Whether it changed since it was made, which animates its indicator.
    pub fn is_changed(&self) -> bool {
        self.changed
    }

    // Joins the group, and follows its value from then on (`joinGroup`).
    fn join_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(group) = self.options.group.clone() else {
            self.control.set_group(None);
            return;
        };
        let value = self.options.value.clone().unwrap_or_else(|| "on".into());
        let parent = self.options.parent;
        let internals = group.read(cx).internals().clone();
        self.control.set_group(Some(Rc::new(GroupLink {
            group: group.downgrade(),
            internals,
            value: value.clone(),
            parent,
        })));
        let follow = cx.observe_in(&group, window, |checkbox, _, window, cx| checkbox.follow(window, cx));
        let checkbox = cx.entity_id();
        if !parent {
            let child = Child {
                checkbox,
                internals: self.control.internals().clone(),
                element: self.control.instance().element_id("root", None),
                focus: self.control.focus_handle().clone(),
            };
            group.update(cx, |group, cx| group.register(value.clone(), child, cx));
        }
        self.membership = Some(Membership {
            group: group.downgrade(),
            value,
            parent,
            checkbox,
            _follow: follow,
        });
    }

    fn leave_group(&mut self, cx: &mut App) {
        let Some(membership) = self.membership.take() else {
            return;
        };
        if membership.parent {
            return;
        }
        if let Some(group) = membership.group.upgrade() {
            group.update(cx, |group, cx| group.unregister(&membership.value, membership.checkbox, cx));
        }
    }

    // The group's value changed, or what it lends its checkboxes.
    fn follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self.control.set_options(toggle_options(&self.options, cx));
        self.after_change(window, cx);
        // A parent points at the checkboxes it controls, which the group
        // knows.
        if changed || self.options.parent {
            cx.notify();
        }
    }

    // What the original's store subscription and the indicator's do: hears
    // the checked state change from any source, before anything renders it.
    fn after_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let checked = self.control.get_checked();
        if checked == self.previous {
            return;
        }
        let was = std::mem::replace(&mut self.previous, checked);
        self.changed = true;
        // Unchecking keeps the indicator while it animates out, in the same
        // write, so it never disappears for a frame first.
        self.exiting = checked == CheckedState::False
            && was != CheckedState::False
            && held(self.control.instance(), "indicator", None);
        let shown = checked != CheckedState::False;
        if shown != (was != CheckedState::False) {
            let indicator = self.indicator.clone();
            if shown {
                indicator.animate_in(window, cx);
            } else if self.exiting || self.keep_mounted.get() {
                indicator.animate_out(self, |checkbox, _window, cx| checkbox.finish_exit(cx), window, cx);
            } else {
                indicator.set_open(false, cx);
            }
        }
        cx.notify();
    }

    fn finish_exit(&mut self, cx: &mut Context<Self>) {
        self.exiting = false;
        cx.notify();
    }

    // The element ids of the checkboxes a parent controls.
    fn controls(&self, cx: &App) -> Vec<ElementId> {
        let Some(membership) = &self.membership else {
            return Vec::new();
        };
        if !membership.parent {
            return Vec::new();
        }
        membership
            .group
            .upgrade()
            .map(|group| group.read(cx).child_ids().to_vec())
            .unwrap_or_default()
    }

    /// The `root` part: the checkbox itself, focusable, which a press, Space
    /// or a label press toggles, and Enter submits its form from. A parent
    /// points at the checkboxes it controls.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<CheckboxState> {
        let checkbox = this.read(cx);
        let aria = checkbox
            .controls(cx)
            .into_iter()
            .fold(Aria::new(), |aria, id| aria.controls(id));
        ToggleControl::root(this, ToggleRole::Checkbox, checkbox_state, cx).aria(aria)
    }

    /// The `indicator` part: the check inside the checkbox, or `None` while
    /// it's out of the page. It's in the page while checked or indeterminate,
    /// while it animates out, and always with `keep_mounted`.
    pub fn indicator(this: &Entity<Self>, props: CheckboxIndicatorProps, cx: &App) -> Option<Part<CheckboxIndicatorState>> {
        let checkbox = this.read(cx);
        checkbox.keep_mounted.set(props.keep_mounted);
        let present = props.keep_mounted || checkbox.control.get_checked() != CheckedState::False || checkbox.exiting;
        if !present {
            return None;
        }
        let state = checkbox_state(&checkbox.control);
        let state = CheckboxIndicatorState {
            checked: state.checked,
            indeterminate: state.indeterminate,
            disabled: state.disabled,
            read_only: state.read_only,
            required: state.required,
            presence: checkbox.indicator.state(),
        };
        Some(Part::new(checkbox.control.instance(), "indicator", None, state))
    }
}

impl Focusable for Checkbox {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.control.focus_handle().clone()
    }
}

fn checked_value(checked: CheckedState) -> AgentValue {
    match checked {
        CheckedState::True => AgentValue::Bool(true),
        CheckedState::False => AgentValue::Bool(false),
        CheckedState::Indeterminate => AgentValue::String("indeterminate".into()),
    }
}

/// `agent: { actions: ["setChecked"] }` and `controlled: ["checked"]`, with
/// `toggle`, which `setChecked` covers, unlisted.
impl Agent for Checkbox {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setChecked", |checkbox, args, window, cx| {
            let checked = match args.value(0) {
                AgentValue::String(text) if text == "indeterminate" => CheckedState::Indeterminate,
                _ => CheckedState::from(args.boolean(0)?),
            };
            Ok(checkbox.set_checked(checked, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |checkbox, _args, window, cx| {
            Ok(checkbox.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["checked"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("checked", checked_value(self.control.get_checked()))
            .with("disabled", self.control.is_disabled())
            .with("readOnly", self.control.is_read_only())
            .with("required", self.control.is_required())
    }
}
