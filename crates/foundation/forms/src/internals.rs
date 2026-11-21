//! What the browser gives a form-associated element through its
//! `ElementInternals`, which every control here uses: the form it belongs to,
//! what it submits, its validity, and the labels that name it.
//!
//! The browser keeps an element's value and validity as the element reports
//! them (`setFormValue`, `setValidity`). So do these internals: a control
//! reports through the binders in `form.rs` each time it renders and after it
//! changes, and the form reads what was reported, never the control's entity,
//! which may be in the middle of an update.

use std::rc::Rc;

use gpui::{
    App, Context, ElementId, FocusHandle, ParentElement, SharedString, StatefulInteractiveElement,
    Subscription, Window,
};
use ui_sdk_kernel::{Aria, InputEvent};

use crate::document::{
    DisabledCallback, FormEvent, FormEventType, Node, NodeKind, WeakNode, dispatch, invalidate,
};
use crate::elements::{Form, Label};
use crate::form::{FormValue, ValidityFlag, focus_control};

/// The browser's `ValidityState`: which constraints a control fails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ValidityState {
    pub value_missing: bool,
    pub type_mismatch: bool,
    pub pattern_mismatch: bool,
    pub too_long: bool,
    pub too_short: bool,
    pub range_underflow: bool,
    pub range_overflow: bool,
    pub step_mismatch: bool,
    pub bad_input: bool,
    pub custom_error: bool,
}

impl ValidityState {
    /// The state with only `flag` set.
    pub fn of(flag: ValidityFlag) -> Self {
        let mut state = Self::default();
        state.set(flag, true);
        state
    }

    /// Whether it fails no constraint.
    pub fn valid(&self) -> bool {
        ValidityFlag::ALL.iter().all(|flag| !self.has(*flag))
    }

    /// Whether it fails `flag`.
    pub fn has(&self, flag: ValidityFlag) -> bool {
        match flag {
            ValidityFlag::ValueMissing => self.value_missing,
            ValidityFlag::TypeMismatch => self.type_mismatch,
            ValidityFlag::PatternMismatch => self.pattern_mismatch,
            ValidityFlag::TooLong => self.too_long,
            ValidityFlag::TooShort => self.too_short,
            ValidityFlag::RangeUnderflow => self.range_underflow,
            ValidityFlag::RangeOverflow => self.range_overflow,
            ValidityFlag::StepMismatch => self.step_mismatch,
            ValidityFlag::BadInput => self.bad_input,
            ValidityFlag::CustomError => self.custom_error,
        }
    }

    /// Sets or clears `flag`.
    pub fn set(&mut self, flag: ValidityFlag, on: bool) {
        let slot = match flag {
            ValidityFlag::ValueMissing => &mut self.value_missing,
            ValidityFlag::TypeMismatch => &mut self.type_mismatch,
            ValidityFlag::PatternMismatch => &mut self.pattern_mismatch,
            ValidityFlag::TooLong => &mut self.too_long,
            ValidityFlag::TooShort => &mut self.too_short,
            ValidityFlag::RangeUnderflow => &mut self.range_underflow,
            ValidityFlag::RangeOverflow => &mut self.range_overflow,
            ValidityFlag::StepMismatch => &mut self.step_mismatch,
            ValidityFlag::BadInput => &mut self.bad_input,
            ValidityFlag::CustomError => &mut self.custom_error,
        };
        *slot = on;
    }

    /// The flags that are set, in the order `ValidityState` lists them.
    pub fn flags(&self) -> impl Iterator<Item = ValidityFlag> + '_ {
        ValidityFlag::ALL.into_iter().filter(|flag| self.has(*flag))
    }
}

/// What a button does in its form, its `type`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ButtonType {
    /// Submits its form.
    Submit,
    /// Resets its form.
    Reset,
    /// Does nothing to its form.
    #[default]
    Button,
}

/// The attributes a field writes on its control: an id for labels to point
/// at, the name to submit under, the parts that name and describe it, and
/// whether it shows as invalid or disabled. The control renders them through
/// [`ElementInternals::aria`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlAttrs {
    /// The id labels point at, when the control has none of its own.
    pub id: Option<SharedString>,
    /// The name it submits under, when it has none of its own.
    pub name: Option<SharedString>,
    /// The parts that name it, by element id (`aria-labelledby`), in place of
    /// the labels the forms code finds and its own label.
    pub labelled_by: Vec<ElementId>,
    /// The parts that describe it, by element id (`aria-describedby`).
    pub described_by: Vec<ElementId>,
    /// Whether it shows as invalid (`aria-invalid`).
    pub invalid: bool,
    /// Whether its field disables it.
    pub disabled: bool,
}

/// What a label press does to a control: like a click, given the press.
pub type ActivateCallback = Rc<dyn Fn(Option<InputEvent>, &mut Window, &mut App)>;
pub(crate) type FocusCallback = Rc<dyn Fn(&mut Window, &mut App)>;
pub(crate) type ResetCallback = Rc<dyn Fn(&mut Window, &mut App)>;

// A component's message held over the app's own (`setComponentValidity`).
pub(crate) struct Held {
    pub(crate) message: SharedString,
    pub(crate) app: SharedString,
    pub(crate) flag: Option<ValidityFlag>,
}

pub(crate) struct ControlData {
    // What the control last reported.
    pub(crate) value: FormValue,
    pub(crate) name: Option<SharedString>,
    pub(crate) disabled: bool,
    pub(crate) aria_disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) barred: bool,
    pub(crate) flags: ValidityState,
    pub(crate) message: SharedString,
    pub(crate) anchor: Option<WeakNode>,
    pub(crate) form_attribute: Option<SharedString>,
    // The element's own custom validity, and a component's message held over it.
    pub(crate) custom: SharedString,
    pub(crate) held: Option<Held>,
    // What the control is.
    pub(crate) focus: Option<FocusHandle>,
    pub(crate) focus_with: Vec<(u64, FocusCallback)>,
    pub(crate) activation: Option<ActivateCallback>,
    pub(crate) labelable: bool,
    pub(crate) control: bool,
    pub(crate) button: Option<ButtonType>,
    // Its own `aria-label`, which names it over its labels.
    pub(crate) aria_label: Option<SharedString>,
    // The element whose own content names it after its labels.
    pub(crate) named_by_content: Option<ElementId>,
    pub(crate) blocks_implicit_submission: bool,
    pub(crate) segment: bool,
    pub(crate) reset_callbacks: Vec<(u64, ResetCallback)>,
    pub(crate) disabled_callbacks: Vec<DisabledCallback>,
    pub(crate) attrs: Option<ControlAttrs>,
    // The validity listeners last heard of, so they hear each change once.
    pub(crate) told: (ValidityState, SharedString, bool),
}

impl Default for ControlData {
    fn default() -> Self {
        Self {
            value: FormValue::None,
            name: None,
            disabled: false,
            aria_disabled: false,
            read_only: false,
            required: false,
            barred: false,
            flags: ValidityState::default(),
            message: SharedString::default(),
            anchor: None,
            form_attribute: None,
            custom: SharedString::default(),
            held: None,
            focus: None,
            focus_with: Vec::new(),
            activation: None,
            labelable: true,
            control: true,
            button: None,
            aria_label: None,
            named_by_content: None,
            blocks_implicit_submission: false,
            segment: false,
            reset_callbacks: Vec::new(),
            disabled_callbacks: Vec::new(),
            attrs: None,
            told: (ValidityState::default(), SharedString::default(), true),
        }
    }
}

impl ControlData {
    // Disabled by itself or its field, or marked `aria-disabled`.
    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled || self.aria_disabled || self.attrs.as_ref().is_some_and(|attrs| attrs.disabled)
    }

    fn validity(&self) -> ValidityState {
        let mut validity = self.flags;
        if !self.custom.is_empty() {
            validity.custom_error = true;
        }
        validity
    }
}

/// A control's part in forms, standing in for the browser's
/// `ElementInternals`. Clones share it.
#[derive(Clone)]
pub struct ElementInternals {
    node: Node,
}

impl ElementInternals {
    /// Internals for the component of `cx`, which is notified when what it
    /// reads from the page changes, like a fieldset that disables it or a
    /// label that names it. Put them in the control's element with
    /// [`ElementInternals::track`].
    pub fn new<T: 'static>(cx: &mut Context<T>) -> Self {
        let internals = Self::detached();
        internals.node.set_observer(Some(cx.entity_id()));
        internals
    }

    /// Internals that notify no entity.
    pub fn detached() -> Self {
        Self {
            node: Node::with_kind(NodeKind::Control),
        }
    }

    /// The internals of a control's node (`formInternals`).
    pub fn of(node: &Node) -> Option<Self> {
        (node.kind() == NodeKind::Control).then(|| Self { node: node.clone() })
    }

    /// The node the control stands for in the page.
    pub fn node(&self) -> &Node {
        &self.node
    }

    pub(crate) fn data(&self) -> std::cell::Ref<'_, ControlData> {
        self.node
            .0
            .control
            .as_ref()
            .expect("a control node holds control data")
            .borrow()
    }

    pub(crate) fn data_mut(&self) -> std::cell::RefMut<'_, ControlData> {
        self.node
            .0
            .control
            .as_ref()
            .expect("a control node holds control data")
            .borrow_mut()
    }

    // Tells validity listeners once the frame has drawn, when what they read
    // changed.
    pub(crate) fn touched(&self) {
        let now = (self.validity(), self.validation_message(), self.will_validate());
        let mut data = self.data_mut();
        if data.told != now {
            data.told = now;
            drop(data);
            self.node.mark_validity_changed();
        }
    }

    /// Puts the control in `el`, its element, so it knows each frame where
    /// it sits: its form, the fieldsets around it, and the labels that name
    /// it. Call it before adding children.
    pub fn track<E: ParentElement + StatefulInteractiveElement>(&self, el: E) -> E {
        self.node.track(el)
    }

    /// The ARIA forms give a control's element: the labels that name it,
    /// by element id, with their text when none of them is a part in the
    /// page, unless its own label names it; its field's labels,
    /// descriptions and invalid state; and whether it's disabled, read-only
    /// and required. Put it on the control's part with `Part::aria`, or on
    /// any element with the kernel's `with_aria`.
    pub fn aria(&self) -> Aria {
        let attrs = self.control_attrs();
        let mut aria = self.name_by_labels();
        // What its field names it by wins over its own label and its labels,
        // as the `aria-labelledby` a field writes does.
        if !attrs.labelled_by.is_empty() {
            aria.labelled_by = attrs.labelled_by;
            aria.fallback_label = self.aria_label();
        }
        aria.described_by = attrs.described_by;
        let data = self.data();
        aria.disabled(data.is_disabled() || self.node.form_disabled())
            .read_only(data.read_only)
            .required(data.required)
            .invalid(attrs.invalid)
    }

    // Points it at its labels, unless it's named some other way (`labelledBy`):
    // their element ids, with their text when none of them is a part in the
    // page. An element with its own label keeps that name, which labels
    // don't override.
    pub(crate) fn name_by_labels(&self) -> Aria {
        if let Some(own) = self.aria_label() {
            return Aria::new().fallback_label(own);
        }
        let mut aria = Aria::new();
        aria.labelled_by = self.labels().iter().filter_map(Label::element_id).collect();
        if let Some(text) = self.label_text() {
            aria = aria.fallback_label(text);
        }
        aria
    }

    /// Its `id`, which labels' `for` point at.
    pub fn set_id(&self, id: Option<SharedString>, cx: &mut App) {
        self.node.set_id(id, cx);
    }

    /// The id labels match: its own, or the one its field gave it.
    pub fn id(&self) -> Option<SharedString> {
        self.node.effective_id()
    }

    /// The id of a form to belong to, when it isn't inside that form (`form`).
    pub fn set_form_attribute(&self, form: Option<SharedString>, cx: &mut App) {
        let mut data = self.data_mut();
        if data.form_attribute == form {
            return;
        }
        data.form_attribute = form;
        drop(data);
        invalidate(cx);
    }

    /// Where focus goes for the control, like when a submit finds it invalid.
    pub fn set_focus_handle(&self, handle: Option<FocusHandle>) {
        self.data_mut().focus = handle;
    }

    pub fn focus_handle(&self) -> Option<FocusHandle> {
        self.data().focus.clone()
    }

    /// What a label press does: like a click, given the press. Without one, a
    /// label press focuses the control (`activateControl`).
    pub fn set_activation(&self, activation: Option<ActivateCallback>) {
        self.data_mut().activation = activation;
    }

    /// Sets the activation to `activate` on the component of `cx`.
    pub fn on_activate<T: 'static>(
        &self,
        cx: &Context<T>,
        activate: impl Fn(&mut T, Option<InputEvent>, &mut Window, &mut Context<T>) + 'static,
    ) {
        let entity = cx.weak_entity();
        self.set_activation(Some(Rc::new(move |event, window, cx| {
            entity
                .update(cx, |this, cx| activate(this, event, window, cx))
                .ok();
        })));
    }

    pub(crate) fn activation(&self) -> Option<ActivateCallback> {
        self.data().activation.clone()
    }

    /// Whether a `<label>` can name and activate it (`isLabelable`). True by
    /// default; a group a label names through `for` alone isn't.
    pub fn set_labelable(&self, labelable: bool) {
        self.data_mut().labelable = labelable;
    }

    pub fn is_labelable(&self) -> bool {
        self.data().labelable
    }

    /// Its own `aria-label`, which names it: the labels that would name it
    /// then don't, as an element named some other way keeps its name
    /// (`labelledBy`). What its field names it by still wins, as a field's
    /// `aria-labelledby` does. [`ElementInternals::aria`] writes it as the
    /// control's label while nothing it's labelled by is in the page and
    /// its part has no `aria_label` of its own. Set it where the original's
    /// element would carry the attribute.
    pub fn set_aria_label(&self, label: Option<SharedString>) {
        self.data_mut().aria_label = label;
    }

    /// Its own `aria-label`, if it has one.
    pub fn aria_label(&self) -> Option<SharedString> {
        self.data().aria_label.clone()
    }

    /// The element whose own content names the control after its labels,
    /// by element id, like a plain button's text, "Browse files": the
    /// control's own element, or the button it renders, like a file input's
    /// trigger. A field names it by its label and then that element, and
    /// points no label's `for` at it, since some name computations stop at
    /// a label's `for` and drop the button's text (`namedByContent`). `None`,
    /// the default, leaves it named by its labels alone.
    pub fn set_named_by_content(&self, element: Option<ElementId>) {
        self.data_mut().named_by_content = element;
    }

    /// The element whose own content names it after its labels, if any.
    pub fn named_by_content(&self) -> Option<ElementId> {
        self.data().named_by_content.clone()
    }

    /// Whether a label looking inside an element for its control stops here
    /// (`findControl`). True by default. A slider with one thumb clears it on
    /// its root, so the thumb is the control.
    pub fn set_control(&self, control: bool, cx: &mut App) {
        let mut data = self.data_mut();
        if data.control == control {
            return;
        }
        data.control = control;
        drop(data);
        invalidate(cx);
    }

    /// Makes it a button of its form, which submits or resets it.
    pub fn set_button(&self, button: Option<ButtonType>) {
        self.data_mut().button = button;
        self.touched();
    }

    /// What it does in its form, when it's a button.
    pub fn button(&self) -> Option<ButtonType> {
        self.data().button
    }

    /// Whether it's a text field that keeps Enter from submitting a form with
    /// other such fields and no submit button, like a text input.
    pub fn set_blocks_implicit_submission(&self, blocks: bool) {
        self.data_mut().blocks_implicit_submission = blocks;
    }

    /// Marks it `aria-disabled`: it stays focusable and submitted, but a
    /// person can't use it.
    pub fn set_aria_disabled(&self, disabled: bool) {
        self.data_mut().aria_disabled = disabled;
    }

    /// The form it belongs to: the one around it, or the one its `form`
    /// option names, as of the last frame.
    pub fn form(&self) -> Option<Form> {
        self.node.form()
    }

    /// The labels that name it, in page order, as of the last frame.
    pub fn labels(&self) -> Vec<Label> {
        self.node.label_nodes().into_iter().map(Label::from_node).collect()
    }

    /// The text of the labels that name it, in page order, as of the last
    /// frame.
    pub fn label_text(&self) -> Option<SharedString> {
        self.node.label_name()
    }

    /// What it submits, as it last reported.
    pub fn value(&self) -> FormValue {
        self.data().value.clone()
    }

    /// Reports what it submits, `setFormValue`.
    pub fn set_form_value(&self, value: FormValue) {
        self.data_mut().value = value;
    }

    /// The name it submits under: its own, or its field's.
    pub fn name(&self) -> Option<SharedString> {
        let data = self.data();
        data.name
            .clone()
            .filter(|name| !name.is_empty())
            .or_else(|| data.attrs.as_ref().and_then(|attrs| attrs.name.clone()))
    }

    /// Whether it's required, as it last reported.
    pub fn required(&self) -> bool {
        self.data().required
    }

    /// Whether a disabled fieldset around it disables it.
    pub fn form_disabled(&self) -> bool {
        self.node.form_disabled()
    }

    /// Whether it's disabled: by itself, its field or a fieldset. Disabled
    /// controls are neither submitted nor validated.
    pub fn is_disabled(&self) -> bool {
        let data = self.data();
        data.disabled || data.attrs.as_ref().is_some_and(|attrs| attrs.disabled) || self.node.form_disabled()
    }

    /// What its field wrote on it.
    pub fn control_attrs(&self) -> ControlAttrs {
        self.data().attrs.clone().unwrap_or_default()
    }

    /// Whether it takes part in constraint validation: not disabled, not a
    /// button that doesn't submit, and not barred by the control itself, like
    /// a read-only text field.
    pub fn will_validate(&self) -> bool {
        let data = self.data();
        let button = matches!(data.button, Some(ButtonType::Reset | ButtonType::Button));
        let barred = data.barred || button;
        drop(data);
        !barred && !self.is_disabled()
    }

    /// The constraints it fails, `validity`.
    pub fn validity(&self) -> ValidityState {
        self.data().validity()
    }

    /// The message for its first problem, or nothing when it's valid or
    /// barred from validation, `validationMessage`.
    pub fn validation_message(&self) -> SharedString {
        if !self.will_validate() {
            return SharedString::default();
        }
        let data = self.data();
        if !data.custom.is_empty() {
            return data.custom.clone();
        }
        if data.validity().valid() {
            return SharedString::default();
        }
        data.message.clone()
    }

    /// Reports its validity, `setValidity`: the constraints it fails, the
    /// message to show, and where the message points. Empty flags make it
    /// valid.
    pub fn set_validity(&self, flags: ValidityState, message: impl Into<SharedString>, anchor: Option<&Node>) {
        let mut data = self.data_mut();
        data.flags = flags;
        data.message = if flags.valid() {
            SharedString::default()
        } else {
            message.into()
        };
        data.anchor = anchor.map(Node::downgrade);
        drop(data);
        self.touched();
    }

    /// Sets the element's own custom validity, as an app's
    /// `setCustomValidity` does on a control. Like a native input's, it stays
    /// through form resets until the app clears it.
    pub fn set_custom_validity(&self, message: impl Into<SharedString>) {
        self.data_mut().custom = message.into();
        self.touched();
    }

    /// Where its validation message points: what it reported, or itself.
    pub fn anchor(&self) -> Node {
        self.data()
            .anchor
            .as_ref()
            .and_then(WeakNode::upgrade)
            .unwrap_or_else(|| self.node.clone())
    }

    /// Fires `Invalid` when it's invalid and returns whether it's valid,
    /// `checkValidity`.
    pub fn check_validity(&self, window: &mut Window, cx: &mut App) -> bool {
        self.validate(false, window, cx)
    }

    /// Like [`ElementInternals::check_validity`], and when nothing cancels
    /// the `Invalid`, focuses it and reports its message to its form,
    /// `reportValidity`.
    pub fn report_validity(&self, window: &mut Window, cx: &mut App) -> bool {
        self.validate(true, window, cx)
    }

    fn validate(&self, report: bool, window: &mut Window, cx: &mut App) -> bool {
        if !self.will_validate() || self.validity().valid() {
            return true;
        }
        let event = FormEvent::new(FormEventType::Invalid, self.node.clone());
        dispatch(&event, window, cx);
        if report && !event.default_prevented() {
            focus_control(&self.node, window, cx);
            if let Some(form) = self.form() {
                form.report(self, window, cx);
            }
        }
        false
    }

    /// Runs `reset` on the component of `cx` after its form resets, once the
    /// form has reset every control before it (`Association.reset`).
    pub fn on_reset<T: 'static>(
        &self,
        cx: &Context<T>,
        reset: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let entity = cx.weak_entity();
        self.add_reset(Rc::new(move |window, cx| {
            entity.update(cx, |this, cx| reset(this, window, cx)).ok();
        }))
    }

    pub(crate) fn add_reset(&self, reset: ResetCallback) -> Subscription {
        let id = next_callback();
        self.data_mut().reset_callbacks.push((id, reset));
        let node = self.node.downgrade();
        Subscription::new(move || {
            if let Some(internals) = node.upgrade().as_ref().and_then(ElementInternals::of) {
                internals.data_mut().reset_callbacks.retain(|(each, _)| *each != id);
            }
        })
    }

    pub(crate) fn reset_callbacks(&self) -> Vec<ResetCallback> {
        self.data().reset_callbacks.iter().map(|(_, reset)| reset.clone()).collect()
    }

    /// Runs `disabled` on the component of `cx` when a fieldset around it
    /// disables it or enables it again (`Association.disabled`). The
    /// component is notified either way.
    pub fn on_disabled<T: 'static>(
        &self,
        cx: &Context<T>,
        disabled: impl Fn(&mut T, bool, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let entity = cx.weak_entity();
        self.node.watch_disabled(Rc::new(move |value, window, cx| {
            entity
                .update(cx, |this, cx| disabled(this, value, window, cx))
                .ok();
        }))
    }

    pub(crate) fn add_focus_with(&self, focus: FocusCallback) -> u64 {
        let id = next_callback();
        self.data_mut().focus_with.push((id, focus));
        id
    }

    pub(crate) fn remove_focus_with(&self, id: u64) {
        self.data_mut().focus_with.retain(|(each, _)| *each != id);
    }

    pub(crate) fn focus_with(&self) -> Option<FocusCallback> {
        self.data().focus_with.last().map(|(_, focus)| focus.clone())
    }
}

impl PartialEq for ElementInternals {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node
    }
}

impl std::fmt::Debug for ElementInternals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElementInternals").field("node", &self.node).finish()
    }
}

fn next_callback() -> u64 {
    thread_local! {
        static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    NEXT.with(|next| {
        next.set(next.get() + 1);
        next.get()
    })
}
