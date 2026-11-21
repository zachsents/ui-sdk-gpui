//! Port of `foundation/forms/toggle-control.ts`: what Checkbox and Switch
//! share. [`ToggleControl`] holds the checked state and its form side, and
//! [`ToggleControl::root`] builds the control's part.

use std::rc::Rc;

use gpui::{App, Context, Entity, FocusHandle, InteractiveElement, SharedString, Subscription, Window};
use ui_sdk_kernel::prelude::AriaRole;
use ui_sdk_kernel::{
    ChangeCallback, ChangeDetails, ChangeOptions, Checked, Controllable, FocusMode, Instance, Key,
    Part, Reason,
};

use crate::elements::Form;
use crate::form::{HiddenInputOptions, INPUT_AND_CHANGE, bind_hidden_input, dispatch_change, is_disabled, submit_implicitly};
use crate::internals::ElementInternals;
use crate::value::keeps_value_on_reset;

/// Checked, unchecked, or indeterminate, for a checkbox that stands for a
/// partly checked group: the kernel's [`Checked`], which every part with a
/// checked state shares.
pub type CheckedState = Checked;

/// Why a checkbox or switch wants to change: a `Press`, a form `Reset`, or an
/// `Imperative` call.
pub type CheckboxCheckedChangeReason = Reason;

/// Options for every write method of a checkbox or switch.
pub type CheckboxChangeOptions = ChangeOptions;

/// The options a checkbox or switch takes.
#[derive(Clone, Default)]
pub struct ToggleOptions {
    /// Whether it's checked. Pass it to control the checkbox.
    pub checked: Option<CheckedState>,
    /// Whether it's checked at first, when uncontrolled. A form reset returns
    /// to it.
    pub default_checked: Option<CheckedState>,
    /// Runs when a user or a form reset wants to change it. `details.reason`
    /// says why (`Press`, `Reset`, `Imperative`), and `details.cancel()` stops
    /// it. It runs inside the component's update, so it may update any entity
    /// but the component.
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
    /// What it submits while checked, `"on"` by default.
    pub value: Option<SharedString>,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
}

impl ToggleOptions {
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
}

/// What a checkbox's parts and the app read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckboxState {
    /// Whether it's checked. False while indeterminate.
    pub checked: bool,
    /// Whether it's indeterminate, for a partly checked group. A press makes
    /// it checked.
    pub indeterminate: bool,
    /// Whether it's disabled, by itself, its field or a fieldset.
    pub disabled: bool,
    /// Whether it's read-only: it stays focusable and submitted, but presses
    /// don't change it.
    pub read_only: bool,
    /// Whether a form won't submit until it's checked. Indeterminate doesn't
    /// count.
    pub required: bool,
}

/// What a switch's parts and the app read: a checkbox's state, never
/// indeterminate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchState {
    pub checked: bool,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
}

/// The group a checkbox belongs to. It takes the checkbox's changes and
/// resets, and lends it its form.
pub trait ToggleGroup {
    /// The form the group belongs to.
    fn form_owner(&self, cx: &App) -> Option<Form>;
    /// Takes a change to the checkbox: the group decides, and its value then
    /// decides the checkbox, through [`ToggleControl::change_checked`].
    /// Returns whether it went through.
    fn write(
        &self,
        control: &mut ToggleControl,
        checked: CheckedState,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> bool;
}

/// The component a toggle control belongs to, which its parts and callbacks
/// reach it through.
pub trait ToggleHost: 'static + Sized {
    fn toggle_control(&self) -> &ToggleControl;
    fn toggle_control_mut(&mut self) -> &mut ToggleControl;
}

/// Which role the control's root takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleRole {
    Checkbox,
    Switch,
}

/// A checkbox's or switch's checked state and form side (`toggleControl`).
pub struct ToggleControl {
    instance: Instance,
    checked: Controllable<CheckedState>,
    // What a form reset returns to: `default_checked`, or the first value.
    initial: CheckedState,
    // Whether `default_checked` was passed, which a controlled one resets to.
    has_default: bool,
    disabled: bool,
    read_only: bool,
    required: bool,
    name: Option<SharedString>,
    value: SharedString,
    form: Option<SharedString>,
    group: Option<Rc<dyn ToggleGroup>>,
    internals: ElementInternals,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ToggleControl {
    /// The control of the component of `cx`, which owns `instance`, like
    /// `Instance::new("checkbox")`, and reaches the control through
    /// [`ToggleHost`].
    pub fn new<T: ToggleHost>(instance: Instance, options: ToggleOptions, cx: &mut Context<T>) -> Self {
        let first = options.checked.or(options.default_checked).unwrap_or_default();
        let mut checked = Controllable::new("checked", first);
        checked.set_controlled(options.checked);
        checked.set_on_change(options.on_checked_change.clone());
        let internals = ElementInternals::new(cx);
        let focus_handle = cx.focus_handle();
        internals.set_focus_handle(Some(focus_handle.clone()));
        // A label press toggles it, like a click.
        internals.on_activate(cx, |host: &mut T, event, window, cx| {
            let options = ChangeOptions {
                reason: Some(Reason::Press),
                event,
                source: None,
            };
            host.toggle_control_mut().toggle(options, window, cx);
        });
        let reset = internals.on_reset(cx, |host: &mut T, window, cx| {
            host.toggle_control_mut().reset(window, cx);
        });
        let control = Self {
            instance,
            checked,
            initial: first,
            has_default: options.default_checked.is_some(),
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            name: options.name,
            value: options.value.unwrap_or_else(|| "on".into()),
            form: options.form,
            group: None,
            internals,
            focus_handle,
            _subscriptions: vec![reset],
        };
        control.sync_form();
        control
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`syncToggle`). Returns whether what renders changed.
    pub fn set_options(&mut self, options: ToggleOptions) -> bool {
        let mut changed = self.checked.set_controlled(options.checked);
        self.checked.set_on_change(options.on_checked_change);
        // Like the native `defaultChecked`, changing it changes what a reset
        // returns to.
        if let Some(default) = options.default_checked {
            self.initial = default;
        }
        self.has_default = options.default_checked.is_some();
        let value = options.value.unwrap_or_else(|| "on".into());
        changed |= self.disabled != options.disabled
            || self.read_only != options.read_only
            || self.required != options.required
            || self.name != options.name
            || self.value != value
            || self.form != options.form;
        self.disabled = options.disabled;
        self.read_only = options.read_only;
        self.required = options.required;
        self.name = options.name;
        self.value = value;
        self.form = options.form;
        self.sync_form();
        changed
    }

    /// Passes the controlled state, or `None` to let it keep its own.
    pub fn set_controlled_checked(&mut self, checked: Option<CheckedState>) -> bool {
        let changed = self.checked.set_controlled(checked);
        self.sync_form();
        changed
    }

    /// The instance of the component it belongs to.
    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub fn instance_mut(&mut self) -> &mut Instance {
        &mut self.instance
    }

    /// Its part in forms.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// The group that takes its changes, like a checkbox group.
    pub fn set_group(&mut self, group: Option<Rc<dyn ToggleGroup>>) {
        self.group = group;
    }

    /// The group that takes its changes, if it's in one.
    pub fn group(&self) -> Option<&Rc<dyn ToggleGroup>> {
        self.group.as_ref()
    }

    /// Whether it's checked: checked, unchecked or indeterminate.
    pub fn get_checked(&self) -> CheckedState {
        *self.checked.get()
    }

    pub fn is_controlled(&self) -> bool {
        self.checked.is_controlled()
    }

    /// Whether it's disabled, by itself, its field or a fieldset. A field
    /// disables its control as the original's adapters pass the field's
    /// `disabled` into it.
    pub fn is_disabled(&self) -> bool {
        is_disabled(
            self.disabled || self.internals.control_attrs().disabled,
            self.internals.form_disabled(),
        )
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn is_required(&self) -> bool {
        self.required
    }

    pub fn name(&self) -> Option<&SharedString> {
        self.name.as_ref()
    }

    pub fn value(&self) -> &SharedString {
        &self.value
    }

    /// What a form reset returns to.
    pub fn initial(&self) -> CheckedState {
        self.initial
    }

    /// Sets any state, as a user would. Disabled and read-only controls
    /// ignore it. A change goes out to its form as `Input` and `Change`.
    /// Returns false when nothing changed or the change was canceled.
    pub fn set_checked<T: 'static>(
        &mut self,
        checked: CheckedState,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        if self.is_disabled() || self.read_only {
            return false;
        }
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        let done = match self.group.clone() {
            Some(group) => group.write(self, checked, options, window, cx),
            None => self.change_checked(checked, options, window, cx),
        };
        if done {
            self.sync_form();
            cx.notify();
            dispatch_change(Some(self.internals.node()), INPUT_AND_CHANGE, window, cx);
        }
        done
    }

    /// Checks it, or unchecks it. An indeterminate checkbox becomes checked.
    pub fn toggle<T: 'static>(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<T>) -> bool {
        let next = CheckedState::from(self.get_checked() != CheckedState::True);
        self.set_checked(next, options, window, cx)
    }

    /// Asks for `checked` with no check of its own, the change a group makes
    /// once it decided.
    pub fn change_checked(
        &mut self,
        checked: CheckedState,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let changed = self.checked.change(&self.instance, checked, options, window, cx);
        if changed {
            self.sync_form();
        }
        changed
    }

    /// Moves focus to it.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus_handle, cx);
    }

    /// The form it belongs to, or its group's.
    pub fn form(&self, cx: &App) -> Option<Form> {
        self.internals
            .form()
            .or_else(|| self.group.as_ref().and_then(|group| group.form_owner(cx)))
    }

    /// What a form reset does to it. A group resets its checkboxes itself,
    /// and a controlled one with no default keeps its state.
    pub fn reset<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        let controlled = self.checked.is_controlled().then_some(());
        let default = self.has_default.then_some(());
        if self.group.is_some() || keeps_value_on_reset(controlled.as_ref(), default.as_ref()) {
            return;
        }
        let initial = self.initial;
        if self
            .checked
            .change(&self.instance, initial, ChangeOptions::reason(Reason::Reset), window, cx)
        {
            cx.notify();
        }
        self.sync_form();
    }

    /// Reports its value and validity to its form, as its hidden checkbox
    /// would. The parts call it as they render; changes call it themselves.
    pub fn sync_form(&self) {
        bind_hidden_input(
            &self.internals,
            HiddenInputOptions {
                checked: self.get_checked().is_checked(),
                name: self.name.clone(),
                value: self.value.clone(),
                required: self.required,
                disabled: self.disabled,
                read_only: self.read_only,
                form: self.form.clone(),
            },
        );
    }

    /// The control itself (`toggleSlots`' `root`): a focusable checkbox or
    /// switch that a press, Space, or a label press toggles, and that Enter
    /// submits its form from, never toggling, like a native checkbox.
    /// `state` makes the slot's state from the control, like
    /// [`checkbox_state`] or [`switch_state`].
    pub fn root<T: ToggleHost, S: 'static>(
        this: &Entity<T>,
        role: ToggleRole,
        state: impl FnOnce(&ToggleControl) -> S,
        cx: &App,
    ) -> Part<S> {
        let control = this.read(cx).toggle_control();
        control.sync_form();
        let disabled = control.is_disabled();
        let aria = control.internals.aria().checked(control.get_checked());
        let keys = this.downgrade();
        let presses = this.downgrade();
        Part::new(&control.instance, "root", None, state(control))
            .aria_role(match role {
                ToggleRole::Checkbox => AriaRole::Checkbox,
                ToggleRole::Switch => AriaRole::Switch,
            })
            .focus_mode(FocusMode::Native, Some(&control.focus_handle), disabled)
            .aria(aria)
            .map_el(|el| {
                control.internals.track(el).capture_key_down(move |event, window, cx| {
                    if Key::of(&event.keystroke) != Key::Enter {
                        return;
                    }
                    // Enter submits the form, like on a native checkbox, and
                    // never toggles: canceled here, gpui makes no click of it.
                    window.prevent_default();
                    if event.is_held {
                        return;
                    }
                    let form = keys
                        .upgrade()
                        .and_then(|host| host.read(cx).toggle_control().form(cx));
                    submit_implicitly(form.as_ref(), window, cx);
                })
            })
            .on_press(move |event, window, cx| {
                presses
                    .update(cx, |host, cx| {
                        let options = ChangeOptions::event_of(Reason::Press, event);
                        host.toggle_control_mut().toggle(options, window, cx);
                    })
                    .ok();
            })
    }
}

/// A switch's state (`switchState`).
pub fn switch_state(control: &ToggleControl) -> SwitchState {
    SwitchState {
        checked: control.get_checked().is_checked(),
        disabled: control.is_disabled(),
        read_only: control.read_only,
        required: control.required,
    }
}

/// A checkbox's state.
pub fn checkbox_state(control: &ToggleControl) -> CheckboxState {
    let switch = switch_state(control);
    CheckboxState {
        checked: switch.checked,
        indeterminate: control.get_checked() == CheckedState::Indeterminate,
        disabled: switch.disabled,
        read_only: switch.read_only,
        required: switch.required,
    }
}
