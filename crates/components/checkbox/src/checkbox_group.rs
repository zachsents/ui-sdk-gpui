//! Port of `components/checkbox/checkbox-group.ts`: checkboxes that share a
//! name and one list of checked values, and a parent checkbox that checks or
//! unchecks them all.
//!
//! Each checkbox joins its group through its `group` option. The group keeps
//! them in page order, as of the frame that drew them, and takes part in
//! forms through its own internals, as the original's form-associated group
//! element does: it submits every checked value under its name.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, Context, ElementId, Entity, EntityId, FocusHandle, SharedString, Subscription, Window};
use ui_sdk_focus::in_order;
use ui_sdk_forms::{
    CheckedState, ElementInternals, Form, FormValue, InternalsOptions, Node, ValueMissingKind,
    bind_internals, document_order, focus_with, is_disabled, keeps_value_on_reset,
    value_missing_message,
};
use ui_sdk_kernel::dev::{self, Origin};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, ChangeCallback, ChangeDetails, ChangeOptions, Controllable,
    Instance, Part, Reason,
};

/// The state of a checkbox that controls a group: checked when every value is
/// selected, unchecked when none is, and indeterminate in between.
pub fn group_checked(selected: &[SharedString], all: &[SharedString]) -> CheckedState {
    let count = all.iter().filter(|value| selected.contains(value)).count();
    if count == 0 {
        return CheckedState::False;
    }
    if count == all.len() {
        CheckedState::True
    } else {
        CheckedState::Indeterminate
    }
}

/// Why a checkbox group's value wants to change: a checkbox in it was pressed
/// (`ItemPress`), a form reset (`Reset`), or a controller or api call
/// (`Imperative`).
pub type CheckboxGroupValueChangeReason = Reason;

/// Options for every write method.
pub type CheckboxGroupChangeOptions = ChangeOptions;

/// The options `CheckboxGroup` takes.
#[derive(Clone, Default)]
pub struct CheckboxGroupOptions {
    /// The checked checkboxes' values. Pass it to control the group.
    pub value: Option<Vec<SharedString>>,
    /// The values checked at first, when uncontrolled. A form reset returns
    /// to them.
    pub default_value: Option<Vec<SharedString>>,
    /// Runs when a user or a form reset wants to change the value.
    /// `details.reason` is `ItemPress`, `Reset` or `Imperative`. It runs once
    /// the group's update is over, so it may update any entity, the group and
    /// its checkboxes too.
    pub on_value_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Runs as the value is asked to change, before it does, inside the
    /// group's update: `details.cancel()` stops it. It may update any entity
    /// but the group and its checkboxes, which follow the group.
    pub on_before_value_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Every value a parent checkbox checks. Without it, the parent checks the
    /// checkboxes in the group, once a frame has drawn them.
    pub all_values: Option<Vec<SharedString>>,
    /// Disables every checkbox in the group, and leaves them out of the form.
    pub disabled: bool,
    /// Makes every checkbox in the group read-only: focusable and submitted,
    /// but presses don't change it.
    pub read_only: bool,
    /// Blocks form submission until at least one checkbox is checked, with the
    /// browser's own message.
    pub required: bool,
    /// The name every checked value in the group submits under.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when the group isn't inside that form.
    pub form: Option<SharedString>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl CheckboxGroupOptions {
    pub fn value(mut self, value: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.value = Some(value.into_iter().map(Into::into).collect());
        self
    }

    pub fn default_value(mut self, value: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.default_value = Some(value.into_iter().map(Into::into).collect());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn all_values(mut self, values: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.all_values = Some(values.into_iter().map(Into::into).collect());
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

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the `group` part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckboxGroupState {
    /// The checked values.
    pub value: Vec<SharedString>,
    /// Whether the group is disabled, by itself, its field or a fieldset.
    pub disabled: bool,
    /// Whether the group is read-only: its checkboxes stay focusable and
    /// submitted, but presses don't change them.
    pub read_only: bool,
    /// Whether a form won't submit until at least one of its checkboxes is
    /// checked.
    pub required: bool,
}

// A checkbox in the group: its control's form side, its root's element id and
// its focus.
#[derive(Clone)]
pub(crate) struct Child {
    pub(crate) checkbox: EntityId,
    pub(crate) internals: ElementInternals,
    pub(crate) element: ElementId,
    pub(crate) focus: FocusHandle,
}

impl Child {
    fn disabled(&self) -> bool {
        self.internals.is_disabled()
    }

    fn connected(&self) -> bool {
        self.internals.node().is_connected()
    }
}

/// A checkbox group. Its entity is its controller.
pub struct CheckboxGroup {
    instance: Instance,
    value: Controllable<Vec<SharedString>>,
    // What a form reset returns to: `default_value`, or the first value.
    initial: Vec<SharedString>,
    // Whether `default_value` was passed, which a controlled group resets to.
    has_default: bool,
    all_values: Option<Vec<SharedString>>,
    disabled: bool,
    read_only: bool,
    required: bool,
    name: Option<SharedString>,
    form: Option<SharedString>,
    internals: ElementInternals,
    // Each checkbox in the group, by value.
    children: HashMap<SharedString, Child>,
    // Values of the checkboxes in the group, in page order.
    ordered: Vec<SharedString>,
    // The ones that aren't disabled.
    enabled: Vec<SharedString>,
    // Their element ids, for a parent's `aria-controls`.
    ids: Vec<ElementId>,
    _subscriptions: Vec<Subscription>,
}

impl CheckboxGroup {
    pub fn new(options: CheckboxGroupOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first = options
            .value
            .clone()
            .or_else(|| options.default_value.clone())
            .unwrap_or_default();
        let mut value = Controllable::new("value", first.clone()).after_update();
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());
        let mut instance = Instance::new("checkbox").with_agent(cx);
        instance.set_label("checkbox-group");
        instance.debug = options.debug;
        let internals = ElementInternals::new(cx);
        let reset = internals.on_reset(cx, |group: &mut Self, window, cx| group.reset(window, cx));
        // The browser focuses an invalid group's first checkbox on submit.
        let entity = cx.weak_entity();
        let focus = focus_with(&internals, move |window, cx| {
            let first = entity.upgrade().and_then(|group| group.read(cx).first_enabled());
            if let Some(first) = first {
                window.focus(&first, cx);
            }
        });
        let group = Self {
            instance,
            value,
            initial: first,
            has_default: options.default_value.is_some(),
            all_values: options.all_values,
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            name: options.name,
            form: options.form,
            internals,
            children: HashMap::new(),
            ordered: Vec::new(),
            enabled: Vec::new(),
            ids: Vec::new(),
            _subscriptions: vec![reset, focus],
        };
        group.report();
        group
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: CheckboxGroupOptions, cx: &mut Context<Self>) {
        let mut changed = self.value.set_controlled(options.value);
        self.value.set_on_change(options.on_value_change);
        self.value.set_on_before_change(options.on_before_value_change);
        // Like the native `defaultChecked`, changing it changes what a reset
        // returns to.
        if let Some(default) = &options.default_value {
            self.initial = default.clone();
        }
        self.has_default = options.default_value.is_some();
        changed |= self.all_values != options.all_values
            || self.disabled != options.disabled
            || self.read_only != options.read_only
            || self.required != options.required
            || self.name != options.name
            || self.form != options.form;
        self.all_values = options.all_values;
        self.disabled = options.disabled;
        self.read_only = options.read_only;
        self.required = options.required;
        self.name = options.name;
        self.form = options.form;
        self.instance.debug = options.debug;
        self.report();
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the group keep its own.
    pub fn set_controlled_value(&mut self, value: Option<Vec<SharedString>>, cx: &mut Context<Self>) {
        if self.value.set_controlled(value) {
            self.report();
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// What the `group` part's state holds.
    pub fn state(&self) -> CheckboxGroupState {
        CheckboxGroupState {
            value: self.value.get().clone(),
            disabled: self.is_disabled(),
            read_only: self.read_only,
            required: self.required,
        }
    }

    /// Its part in forms.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    /// The checked values.
    pub fn get_value(&self) -> &[SharedString] {
        self.value.get()
    }

    /// Sets the checked values, as a user would. A disabled group ignores
    /// it. Returns false when canceled.
    pub fn set_value(
        &mut self,
        value: Vec<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        !self.is_disabled() && self.write(value, options, window, cx)
    }

    /// Moves focus to the first enabled checkbox.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(first) = self.first_enabled() {
            window.focus(&first, cx);
        }
    }

    /// The name every checkbox in the group submits under.
    pub fn name(&self) -> Option<&SharedString> {
        self.name.as_ref()
    }

    /// The id of the form its `form` option names.
    pub fn form_attribute(&self) -> Option<SharedString> {
        self.form.clone()
    }

    /// Whether the group is disabled, by itself, its field or a fieldset. A
    /// field disables the group it's the control of, as the original's
    /// adapters pass the field's `disabled` into it.
    pub fn is_disabled(&self) -> bool {
        is_disabled(
            self.disabled || self.internals.control_attrs().disabled,
            self.internals.form_disabled(),
        )
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether `value` is checked.
    pub fn checked(&self, value: &str) -> bool {
        self.value.get().iter().any(|item| item.as_ref() == value)
    }

    /// A parent checkbox's state: checked when all are, indeterminate when
    /// some are.
    pub fn parent_checked(&self) -> CheckedState {
        group_checked(self.value.get(), &self.all())
    }

    /// Checks or unchecks one checkbox, the change a checkbox in the group
    /// asks for. A disabled group refuses it.
    pub fn set_child(
        &mut self,
        value: SharedString,
        checked: bool,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_disabled() {
            return false;
        }
        let mut next: Vec<SharedString> = self.value.get().iter().filter(|item| **item != value).cloned().collect();
        if checked {
            next.push(value);
        }
        self.write(next, options, window, cx)
    }

    /// Whether every enabled checkbox is checked, so a parent's press can only
    /// uncheck.
    pub fn all_enabled_checked(&self) -> bool {
        let current = self.value.get();
        self.all()
            .iter()
            .all(|value| self.children.get(value).is_some_and(Child::disabled) || current.contains(value))
    }

    /// The form the group belongs to, for its checkboxes.
    pub fn form_owner(&self) -> Option<Form> {
        self.internals.form()
    }

    /// Checks or unchecks every enabled checkbox. Disabled ones keep their
    /// state.
    pub fn set_all(&mut self, checked: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.is_disabled() {
            return false;
        }
        let locked = |value: &SharedString| self.children.get(value).is_some_and(Child::disabled);
        let current = self.value.get().clone();
        let next = if checked {
            let mut next = current;
            for value in self.all() {
                if !locked(&value) && !next.contains(&value) {
                    next.push(value);
                }
            }
            next
        } else {
            current.into_iter().filter(|value| locked(value)).collect()
        };
        self.write(next, options, window, cx)
    }

    /// The element ids of the checkboxes in the group, in page order, for a
    /// parent's `aria-controls`.
    pub fn child_ids(&self) -> &[ElementId] {
        &self.ids
    }

    // Every value a parent checks: `all_values`, or the checkboxes in the
    // group.
    fn all(&self) -> Vec<SharedString> {
        self.all_values.clone().unwrap_or_else(|| self.ordered.clone())
    }

    fn write(&mut self, value: Vec<SharedString>, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if value == *self.value.get() {
            return true;
        }
        // Keep the page order, so forms submit values the same way every time.
        let ordered = in_order(&value, &self.all());
        let changed = self.value.change(&self.instance, ordered, options, window, cx);
        if changed {
            self.report();
            cx.notify();
        }
        changed
    }

    // What the form gets, like native checkboxes: the checked ones in the
    // page that aren't disabled.
    fn submitted(&self) -> Vec<SharedString> {
        let value = self.value.get();
        self.enabled.iter().filter(|item| value.contains(item)).cloned().collect()
    }

    // In page order, not the order they joined, since one can render before
    // the others later.
    fn first_enabled(&self) -> Option<FocusHandle> {
        self.first_enabled_child().map(|child| child.focus.clone())
    }

    fn first_enabled_child(&self) -> Option<&Child> {
        self.ordered
            .iter()
            .filter_map(|value| self.children.get(value))
            .find(|child| child.connected() && !child.disabled())
    }

    // Where the browser points its validation message.
    fn anchor(&self) -> Option<Node> {
        self.first_enabled_child().map(|child| child.internals.node().clone())
    }

    // Reports the group's value and validity to its form (`bindInternals`).
    fn report(&self) {
        let submitted = self.submitted();
        bind_internals(
            &self.internals,
            InternalsOptions {
                value_missing: self.required && submitted.is_empty(),
                value: FormValue::List(submitted),
                name: self.name.clone(),
                message: value_missing_message(ValueMissingKind::Checkbox),
                anchor: self.anchor(),
                disabled: self.disabled,
                read_only: self.read_only,
                required: self.required,
                form: self.form.clone(),
                ..InternalsOptions::default()
            },
        );
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let controlled = self.value.is_controlled().then_some(());
        let default = self.has_default.then_some(());
        if keeps_value_on_reset(controlled.as_ref(), default.as_ref()) {
            return;
        }
        let initial = self.initial.clone();
        if self
            .value
            .change(&self.instance, initial, ChangeOptions::reason(Reason::Reset), window, cx)
        {
            cx.notify();
        }
        self.report();
    }

    // A checkbox joins as it's made. Checkboxes join by value, so two with
    // the same one, like two without a `value` ("on"), replace each other.
    pub(crate) fn register(&mut self, value: SharedString, child: Child, cx: &mut Context<Self>) {
        if dev::DEV
            && let Some(other) = self.children.get(&value)
            && other.checkbox != child.checkbox
            && other.connected()
        {
            dev::warn(
                &format!("two checkboxes in a group share the value \"{value}\". Give each its own `value`."),
                Some(Origin::new(self.instance.label(), self.instance.id())),
            );
        }
        self.children.insert(value, child);
        self.refresh_children(cx);
    }

    pub(crate) fn unregister(&mut self, value: &SharedString, checkbox: EntityId, cx: &mut Context<Self>) {
        if self.children.get(value).is_some_and(|child| child.checkbox == checkbox) {
            self.children.remove(value);
        }
        self.refresh_children(cx);
    }

    // The checkboxes in the page, in page order, as of the last frame.
    fn refresh_children(&mut self, cx: &mut Context<Self>) {
        let mut ordered: Vec<(&SharedString, &Child)> =
            self.children.iter().filter(|(_, child)| child.connected()).collect();
        ordered.sort_by(|(_, a), (_, b)| document_order(a.internals.node(), b.internals.node()));
        let enabled: Vec<SharedString> = ordered
            .iter()
            .filter(|(_, child)| !child.disabled())
            .map(|(value, _)| (*value).clone())
            .collect();
        let ids: Vec<ElementId> = ordered.iter().map(|(_, child)| child.element.clone()).collect();
        let values: Vec<SharedString> = ordered.into_iter().map(|(value, _)| value.clone()).collect();
        if values == self.ordered && enabled == self.enabled && ids == self.ids {
            return;
        }
        self.ordered = values;
        self.enabled = enabled;
        self.ids = ids;
        self.report();
        cx.notify();
    }

    /// The `group` part, around the checkboxes. It takes part in forms: it
    /// submits every checked value under its name, and blocks a submit while
    /// it's required and nothing is checked. Name it with `aria_label`, a
    /// label pointing at it, or a field.
    pub fn group(this: &Entity<Self>, cx: &App) -> Part<CheckboxGroupState> {
        let group = this.read(cx);
        group.report();
        let state = group.state();
        let internals = group.internals.clone();
        let mut aria = internals.aria();
        // A group takes no required or read-only state of its own.
        aria.required = false;
        aria.read_only = false;
        let entity = this.downgrade();
        Part::new(&group.instance, "group", None, state)
            .aria_role(Role::Group)
            .aria(aria)
            .map_el(|el| internals.track(el))
            // Checkboxes joining, leaving or moving show once a frame drew
            // them.
            .on_props(move |_window, cx| {
                entity.update(cx, |group, cx| group.refresh_children(cx)).ok();
            })
    }
}

/// `agent: { actions: ["setValue", "setAll"] }`.
impl Agent for CheckboxGroup {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |group, args, window, cx| {
            let value = args.list(0)?;
            Ok(group.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setAll", |group, args, window, cx| {
            let checked = args.boolean(0)?;
            Ok(group.set_all(checked, ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        let value: Vec<String> = self.value.get().iter().map(ToString::to_string).collect();
        AgentState::new()
            .with("value", value)
            .with("disabled", self.is_disabled())
            .with("readOnly", self.read_only)
            .with("required", self.required)
    }
}
