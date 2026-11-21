//! Port of `foundation/forms/form.ts`: native form participation, shared by
//! every form control.
//!
//! A control takes part in a form through its [`ElementInternals`]. It
//! reports what it submits and its validity through the binders here, hears
//! its form's resets and the fieldsets that disable it, and is named by the
//! labels around it or pointing at it. The original either keeps a visually
//! hidden native input in step with the control or reports through a custom
//! element's internals; gpui has neither, so every control reports to its
//! internals, the way a form-associated custom element does.
//!
//! gpui has no store to subscribe to, so a binder is the original's `sync`:
//! call it each time the control renders, and again after a change that
//! something may read before the next frame, like a reset.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{App, Context, SharedString, Subscription, Window};
use ui_sdk_kernel::{Aria, InputEvent};

use crate::document::{FormEvent, FormEventType, Node, dispatch, invalidate, mark_dirty};
use crate::elements::{Form, FormData, Label};
use crate::internals::{ButtonType, ControlAttrs, ElementInternals, Held, ValidityState};
use crate::value::writing_from_script;

/// What a control submits: one value under its name, several under its name,
/// entries of its own like files, or nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum FormValue {
    #[default]
    None,
    Text(SharedString),
    List(Vec<SharedString>),
    Data(FormData),
}

impl From<&str> for FormValue {
    fn from(value: &str) -> Self {
        FormValue::Text(SharedString::from(value.to_string()))
    }
}

impl From<String> for FormValue {
    fn from(value: String) -> Self {
        FormValue::Text(value.into())
    }
}

impl From<SharedString> for FormValue {
    fn from(value: SharedString) -> Self {
        FormValue::Text(value)
    }
}

impl From<Option<SharedString>> for FormValue {
    fn from(value: Option<SharedString>) -> Self {
        value.map_or(FormValue::None, FormValue::Text)
    }
}

impl From<Vec<SharedString>> for FormValue {
    fn from(values: Vec<SharedString>) -> Self {
        FormValue::List(values)
    }
}

impl From<FormData> for FormValue {
    fn from(data: FormData) -> Self {
        FormValue::Data(data)
    }
}

/// Where focus goes for a control that isn't focusable itself, like a group's
/// tab stop. A group isn't focusable, so when a submit finds it invalid it
/// claims focus for its first item itself. Dropping the subscription takes
/// both back.
pub fn focus_with(
    internals: &ElementInternals,
    focus: impl Fn(&mut Window, &mut App) + 'static,
) -> Subscription {
    let id = internals.add_focus_with(Rc::new(focus));
    let node = internals.node().downgrade();
    let invalid = internals.node().listen(FormEventType::Invalid, false, move |event, window, cx| {
        let Some(node) = node.upgrade() else {
            return;
        };
        if event.target() == &node {
            claim_invalid_focus(&node, node.form().as_ref(), window, cx);
        }
    });
    let internals = internals.clone();
    Subscription::new(move || {
        drop(invalid);
        internals.remove_focus_with(id);
    })
}

/// Focuses a control the way a user would reach it. A group focuses its tab
/// stop.
pub fn focus_control(node: &Node, window: &mut Window, cx: &mut App) {
    let Some(internals) = ElementInternals::of(node) else {
        return;
    };
    if let Some(focus) = internals.focus_with() {
        focus(window, cx);
    } else if let Some(handle) = internals.focus_handle() {
        window.focus(&handle, cx);
    }
}

thread_local! {
    // A control with no form took focus in the current submission.
    static DOCUMENT_CLAIMED: Cell<bool> = const { Cell::new(false) };
}

/// The first invalid control of a submission takes focus, and later ones
/// don't steal it. The claim lasts until the current update is done, which
/// every `Invalid` of one submission runs in.
pub fn claim_invalid_focus(node: &Node, form: Option<&Form>, window: &mut Window, cx: &mut App) {
    if invalid_focus_claimed(node, form) {
        return;
    }
    set_claimed(form, true);
    let form = form.cloned();
    window.defer(cx, move |_, _| set_claimed(form.as_ref(), false));
    focus_control(node, window, cx);
}

/// Whether an earlier control of this submission already took focus.
pub fn invalid_focus_claimed(_node: &Node, form: Option<&Form>) -> bool {
    match form {
        Some(form) => form.facts_mut().claimed,
        None => DOCUMENT_CLAIMED.with(Cell::get),
    }
}

fn set_claimed(form: Option<&Form>, claimed: bool) {
    match form {
        Some(form) => form.facts_mut().claimed = claimed,
        None => DOCUMENT_CLAIMED.with(|cell| cell.set(claimed)),
    }
}

/// The internals of a control's node, when it takes part in forms
/// (`formInternals`).
pub fn form_internals(node: Option<&Node>) -> Option<ElementInternals> {
    node.and_then(ElementInternals::of)
}

/// The kinds of control a browser has its own value-missing message for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueMissingKind {
    Checkbox,
    Radio,
    Select,
    File,
    Text,
}

thread_local! {
    static MESSAGES: RefCell<Vec<(ValueMissingKind, SharedString)>> = const { RefCell::new(Vec::new()) };
}

/// The browser's own message for a missing value: Chrome's and Firefox's in
/// English, or the one [`set_value_missing_message`] set for the app's
/// language.
pub fn value_missing_message(kind: ValueMissingKind) -> SharedString {
    let set = MESSAGES.with(|messages| {
        messages
            .borrow()
            .iter()
            .find(|(each, _)| *each == kind)
            .map(|(_, message)| message.clone())
    });
    set.unwrap_or_else(|| {
        match kind {
            ValueMissingKind::Checkbox => "Please check this box if you want to proceed.",
            ValueMissingKind::Radio => "Please select one of these options.",
            ValueMissingKind::Select => "Please select an item in the list.",
            ValueMissingKind::File => "Please select a file.",
            ValueMissingKind::Text => "Please fill out this field.",
        }
        .into()
    })
}

/// Sets the value-missing message for `kind`, in the app's language.
pub fn set_value_missing_message(kind: ValueMissingKind, message: impl Into<SharedString>) {
    let message = message.into();
    MESSAGES.with(|messages| {
        let mut messages = messages.borrow_mut();
        messages.retain(|(each, _)| *each != kind);
        messages.push((kind, message));
    });
}

/// Tells fields around a control that its validity changed, once the frame
/// has drawn.
pub fn validity_changed(node: &Node) {
    node.mark_validity_changed();
}

pub use crate::document::on_validity_change;

/// The constraints a control can fail, as `ValidityState` names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValidityFlag {
    ValueMissing,
    TypeMismatch,
    PatternMismatch,
    TooLong,
    TooShort,
    RangeUnderflow,
    RangeOverflow,
    StepMismatch,
    BadInput,
    CustomError,
}

impl ValidityFlag {
    /// Every flag, in the order `ValidityState` lists them.
    pub const ALL: [ValidityFlag; 10] = [
        ValidityFlag::ValueMissing,
        ValidityFlag::TypeMismatch,
        ValidityFlag::PatternMismatch,
        ValidityFlag::TooLong,
        ValidityFlag::TooShort,
        ValidityFlag::RangeUnderflow,
        ValidityFlag::RangeOverflow,
        ValidityFlag::StepMismatch,
        ValidityFlag::BadInput,
        ValidityFlag::CustomError,
    ];

    /// The name `ValidityState` gives it, like `"valueMissing"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ValidityFlag::ValueMissing => "valueMissing",
            ValidityFlag::TypeMismatch => "typeMismatch",
            ValidityFlag::PatternMismatch => "patternMismatch",
            ValidityFlag::TooLong => "tooLong",
            ValidityFlag::TooShort => "tooShort",
            ValidityFlag::RangeUnderflow => "rangeUnderflow",
            ValidityFlag::RangeOverflow => "rangeOverflow",
            ValidityFlag::StepMismatch => "stepMismatch",
            ValidityFlag::BadInput => "badInput",
            ValidityFlag::CustomError => "customError",
        }
    }
}

/// Sets a control's custom validity from a component's own check, like a
/// date picker's text that isn't a date, and tells fields around it. An empty
/// message gives the control back the message the app had set, unless the
/// app set another one since. A control barred from validation, like a
/// disabled one, keeps holding the component's.
pub fn set_component_validity(internals: &ElementInternals, message: &str, flag: Option<ValidityFlag>) {
    let mut data = internals.data_mut();
    let held = data.held.as_ref();
    if held.map_or("", |held| held.message.as_ref()) == message && held.and_then(|held| held.flag) == flag {
        return;
    }
    if !message.is_empty() {
        let message: SharedString = message.to_string().into();
        let app = data.custom.clone();
        match &mut data.held {
            Some(held) => {
                held.message = message.clone();
                held.flag = flag;
            }
            None => {
                data.held = Some(Held {
                    message: message.clone(),
                    app,
                    flag,
                })
            }
        }
        data.custom = message;
    } else {
        let Some(held) = data.held.take() else {
            return;
        };
        if data.custom == held.message {
            data.custom = held.app;
        }
    }
    drop(data);
    internals.touched();
    validity_changed(internals.node());
}

/// The flag a component's message stands for, like `RangeUnderflow`, which a
/// custom validity can't set on a control.
pub fn component_flag_of(node: &Node) -> Option<ValidityFlag> {
    let internals = ElementInternals::of(node)?;
    let data = internals.data();
    let held = data.held.as_ref()?;
    (data.custom == held.message).then_some(held.flag).flatten()
}

/// Enter on a checkbox, switch, radio or text field submits its form, like it
/// does on a native one: it clicks the form's first submit button, or submits
/// a form without one when no other text field claims Enter. It runs once the
/// key's update is done, so the control that took the key may be read.
pub fn submit_implicitly(form: Option<&Form>, window: &mut Window, cx: &mut App) {
    let Some(form) = form.cloned() else {
        return;
    };
    window.defer(cx, move |window, cx| {
        let elements = form.elements();
        if let Some(button) = elements
            .iter()
            .find(|control| control.button() == Some(ButtonType::Submit))
        {
            if !button.is_disabled() {
                click_button(button, window, cx);
            }
            return;
        }
        let fields = elements
            .iter()
            .filter(|control| {
                let data = control.data();
                data.blocks_implicit_submission && !data.segment
            })
            .count();
        if fields <= 1 {
            form.request_submit(None, None, window, cx);
        }
    });
}

// A click on a button from code: what its press does, else what a native
// button does.
fn click_button(button: &ElementInternals, window: &mut Window, cx: &mut App) {
    match button.activation() {
        Some(activate) => activate(None, window, cx),
        None => activate_button(button, None, window, cx),
    }
}

/// What a native button does once nothing prevents its press: a submit button
/// submits its form, from itself, and a reset button resets it.
pub fn activate_button(
    button: &ElementInternals,
    event: Option<InputEvent>,
    window: &mut Window,
    cx: &mut App,
) {
    if button.is_disabled() {
        return;
    }
    let Some(form) = button.form() else {
        return;
    };
    match button.button() {
        Some(ButtonType::Submit) => form.request_submit(Some(button), event, window, cx),
        Some(ButtonType::Reset) => form.reset(window, cx),
        _ => {}
    }
}

/// Marks a Date Input's or Time Input's segment. Each one is a text field, but
/// a form counts them as none when it decides whether Enter submits, as it
/// does a native date input, so Enter in a form whose only text field sits
/// beside segments, or in a segment of a form with no other, submits it. A
/// segment calls [`submit_implicitly`] on Enter like any text field.
/// Dropping the subscription unmarks it.
pub fn bind_segment_submit(internals: &ElementInternals) -> Subscription {
    {
        let mut data = internals.data_mut();
        data.segment = true;
        data.blocks_implicit_submission = true;
    }
    let internals = internals.clone();
    Subscription::new(move || internals.data_mut().segment = false)
}

/// Sends `Input` and `Change` from the control that takes part in the form,
/// after a user changes its value, so form listeners and fields hear it.
/// Controls with a continuous value send `Input` as it changes and `Change`
/// when the interaction ends, like a native range or text input.
pub fn dispatch_change(node: Option<&Node>, types: &[FormEventType], window: &mut Window, cx: &mut App) {
    let Some(node) = node.cloned() else {
        return;
    };
    if writing_from_script(&node) {
        return;
    }
    let types: Vec<FormEventType> = types.to_vec();
    // Controlled components take the new value first.
    window.defer(cx, move |window, cx| {
        if !node.is_connected() {
            return;
        }
        for kind in [FormEventType::Input, FormEventType::Change] {
            if types.contains(&kind) {
                dispatch(&FormEvent::new(kind, node.clone()), window, cx);
            }
        }
    });
}

/// `Input` then `Change`, what [`dispatch_change`] sends by default.
pub const INPUT_AND_CHANGE: &[FormEventType] = &[FormEventType::Input, FormEventType::Change];

/// Whether a disabled fieldset around the node disables it, as of the last
/// frame, except one in the fieldset's first legend.
pub fn fieldset_disabled(node: &Node) -> bool {
    node.form_disabled()
}

/// What a control does when its form resets.
pub type ResetFn = Rc<dyn Fn(&mut Window, &mut App)>;
/// What a control does when fieldsets disable or enable it.
pub type DisabledFn = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// What a control hears from its form.
#[derive(Clone, Default)]
pub struct Association {
    /// Runs after a form reset, once the form has reset the controls before
    /// it. Report the reset state with a binder before it returns, since
    /// fields read validity right after.
    pub reset: Option<ResetFn>,
    /// An ancestor fieldset disabled the control, or enabled it again.
    pub disabled: Option<DisabledFn>,
}

impl Association {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resets the component of `cx`.
    pub fn reset<T: 'static>(
        mut self,
        cx: &Context<T>,
        reset: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        let entity = cx.weak_entity();
        self.reset = Some(Rc::new(move |window, cx| {
            entity.update(cx, |this, cx| reset(this, window, cx)).ok();
        }));
        self
    }

    /// Tells the component of `cx` that a fieldset disabled or enabled it.
    pub fn disabled<T: 'static>(
        mut self,
        cx: &Context<T>,
        disabled: impl Fn(&mut T, bool, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        let entity = cx.weak_entity();
        self.disabled = Some(Rc::new(move |value, window, cx| {
            entity
                .update(cx, |this, cx| disabled(this, value, window, cx))
                .ok();
        }));
        self
    }
}

/// The form side of a control: its form's resets and the fieldsets that
/// disable it. Where focus goes when the form focuses it, like on an invalid
/// submit, is the internals' focus handle or [`focus_with`]. Dropping the
/// subscription stops both.
pub fn associate(internals: &ElementInternals, association: Association) -> Subscription {
    let mut subscriptions = Vec::new();
    if let Some(reset) = association.reset {
        subscriptions.push(internals.add_reset(reset));
    }
    if let Some(disabled) = association.disabled {
        subscriptions.push(watch_fieldsets(internals.node(), move |value, window, cx| {
            disabled(value, window, cx)
        }));
    }
    Subscription::new(move || drop(subscriptions))
}

/// Calls `on_change` whenever whether ancestor fieldsets disable `node`
/// changes, once the frame that changed it has drawn. Read
/// [`Node::form_disabled`] for whether they do now. Dropping the subscription
/// stops it.
pub fn watch_fieldsets(
    node: &Node,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> Subscription {
    node.watch_disabled(Rc::new(on_change))
}

/// Tells what watches `node` that its element moved. Moves show in the next
/// frame anyway; this works things out again now.
pub fn element_moved(_node: &Node, cx: &mut App) {
    invalidate(cx);
}

/// A control's own `disabled` (or its field's), or a disabled fieldset around
/// it as `watch_fieldsets` reports.
pub fn is_disabled(disabled: bool, form_disabled: bool) -> bool {
    disabled || form_disabled
}

/// A node for a component's root whose disabled fieldsets the component
/// follows: track it in the root's element, and read
/// [`Node::form_disabled`]. The component is notified when it changes.
pub fn watch_form_disabled<T: 'static>(cx: &mut Context<T>) -> Node {
    let node = Node::element();
    node.set_observer(Some(cx.entity_id()));
    node
}

/// Whether a person can't use the node's element: disabled by itself or a
/// fieldset, or marked `aria-disabled`.
pub fn is_disabled_element(node: &Node) -> bool {
    node.is_disabled()
}

/// What a checkbox-like control reports: a native checkbox's value,
/// checkedness and constraints.
#[derive(Clone, Debug)]
pub struct HiddenInputOptions {
    /// Whether it's checked. It submits only then.
    pub checked: bool,
    /// The name it submits under. Without one, it submits nothing.
    pub name: Option<SharedString>,
    /// What it submits while checked.
    pub value: SharedString,
    /// Blocks submission until it's checked.
    pub required: bool,
    /// Its own `disabled`.
    pub disabled: bool,
    /// Whether it's read-only, which it reports to assistive technology.
    pub read_only: bool,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
}

impl Default for HiddenInputOptions {
    fn default() -> Self {
        Self {
            checked: false,
            name: None,
            value: "on".into(),
            required: false,
            disabled: false,
            read_only: false,
            form: None,
        }
    }
}

/// Reports a checkbox-like control the way its hidden checkbox would: its
/// value while checked, and a missing value while required and unchecked.
pub fn bind_hidden_input(internals: &ElementInternals, options: HiddenInputOptions) {
    report(
        internals,
        Report {
            value: if options.checked {
                FormValue::Text(options.value)
            } else {
                FormValue::None
            },
            name: options.name,
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            barred: false,
            form: options.form,
        },
    );
    set_missing(
        internals,
        options.required && !options.checked,
        value_missing_message(ValueMissingKind::Checkbox),
        None,
    );
}

/// What a control that holds a value reports: a text input's value and
/// constraints.
#[derive(Clone, Debug, Default)]
pub struct ValueInputOptions {
    /// What it submits, even when empty.
    pub value: SharedString,
    /// The name it submits under. Without one, it submits nothing.
    pub name: Option<SharedString>,
    /// Blocks submission while the value is empty.
    pub required: bool,
    /// Its own `disabled`.
    pub disabled: bool,
    /// Whether it's read-only, which it reports to assistive technology.
    pub read_only: bool,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
    /// Which message a missing value shows, the text field's by default.
    pub missing: Option<ValueMissingKind>,
}

/// Reports a control that holds a value, like a select, the way its hidden
/// text input would: the value, always, and a missing value while required
/// and empty.
pub fn bind_value_input(internals: &ElementInternals, options: ValueInputOptions) {
    let missing = options.required && options.value.is_empty();
    report(
        internals,
        Report {
            value: FormValue::Text(options.value),
            name: options.name,
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            barred: false,
            form: options.form,
        },
    );
    set_missing(
        internals,
        missing,
        value_missing_message(options.missing.unwrap_or(ValueMissingKind::Text)),
        None,
    );
}

/// Reports a control's value and validity through its internals, the way a
/// form-associated custom element does (`bindInternals`). See
/// `bind_element_internals`.
pub fn bind_internals(internals: &ElementInternals, options: crate::form_internals::InternalsOptions) {
    crate::form_internals::bind_element_internals(internals, options);
}

// What every binder reports besides validity.
pub(crate) struct Report {
    pub(crate) value: FormValue,
    pub(crate) name: Option<SharedString>,
    pub(crate) disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) barred: bool,
    pub(crate) form: Option<SharedString>,
}

pub(crate) fn report(internals: &ElementInternals, report: Report) {
    let mut data = internals.data_mut();
    data.value = report.value;
    data.name = report.name;
    data.disabled = report.disabled;
    data.read_only = report.read_only;
    data.required = report.required;
    data.barred = report.barred;
    if data.form_attribute != report.form {
        data.form_attribute = report.form;
        drop(data);
        mark_dirty();
    }
}

fn set_missing(internals: &ElementInternals, missing: bool, message: SharedString, anchor: Option<&Node>) {
    if missing {
        internals.set_validity(ValidityState::of(ValidityFlag::ValueMissing), message, anchor);
    } else {
        internals.set_validity(ValidityState::default(), "", anchor);
    }
}

/// The id a field gives its control when the control has none.
pub fn field_control_id(field_id: &str) -> SharedString {
    format!("{field_id}-control").into()
}

/// Whether a label can name and activate the node's control: every control
/// that takes part in forms, unless it says otherwise.
pub fn is_labelable(node: &Node) -> bool {
    ElementInternals::of(node).is_some_and(|internals| internals.is_labelable())
}

/// Points the element that names a control at its labels, unless it's named
/// some other way: the labels by element id, with their text when none of
/// them is a part in the page, or the control's own label when it has one
/// (`ElementInternals::set_aria_label`), which labels don't override. `node`
/// is the control the labels are for, and the ARIA goes on whatever element
/// names it, like a select's trigger. Merge it under the app's own with
/// `Part::aria`.
pub fn name_from_labels(node: &Node) -> Aria {
    ElementInternals::of(node).map_or_else(Aria::new, |internals| internals.name_by_labels())
}

/// The labels that name a control, in page order: one around it, and those
/// whose `for` is its id, as of the last frame.
pub fn labels_of(node: &Node) -> Vec<Label> {
    ElementInternals::of(node).map_or_else(Vec::new, |internals| internals.labels())
}

/// Writes the attributes a field puts on its control, or takes them off with
/// `None`. The control is notified, and labels matching the id it gives hear
/// it once the current update is done.
pub fn set_control_attrs(node: &Node, attrs: Option<ControlAttrs>, cx: &mut App) {
    let Some(internals) = ElementInternals::of(node) else {
        return;
    };
    let mut data = internals.data_mut();
    if data.attrs == attrs {
        return;
    }
    let id_changed = data.attrs.as_ref().and_then(|attrs| attrs.id.clone())
        != attrs.as_ref().and_then(|attrs| attrs.id.clone());
    data.attrs = attrs;
    drop(data);
    internals.touched();
    if id_changed {
        invalidate(cx);
    }
    if let Some(observer) = node.observer() {
        cx.notify(observer);
    }
}
