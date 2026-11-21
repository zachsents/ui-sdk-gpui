//! The elements a browser gives forms, which gpui has none of: `<form>`,
//! `<fieldset>`, `<legend>` and `<label>`, and the `FormData` a form submits.
//!
//! Each is a handle to its node; clones are the same element. An app or a
//! component keeps one and puts it in the element it renders with `track` (a
//! label also with `bind`, which adds its press), or renders the ready-made
//! `root`. Controls rendered inside a form's element belong to it, those
//! inside a disabled fieldset's are disabled, and a label names and activates
//! the control inside it or the one its `for` names.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, Div, ElementId, InteractiveElement, ParentElement, Role, SharedString,
    Stateful, StatefulInteractiveElement, Subscription, Window, div,
};
use ui_sdk_kernel::part::{PartElement, with_aria};
use ui_sdk_kernel::{Aria, InputEvent};

use crate::control::activate_control;
use crate::document::{
    FormEvent, FormEventType, FormListener, Node, NodeKind, dispatch, invalidate, placed_of,
};
use crate::files::FormFile;
use crate::form::{FormValue, focus_control};
use crate::internals::{ButtonType, ElementInternals};

/// What `on_submit` receives besides the data.
#[derive(Clone, Debug, Default)]
pub struct SubmitDetails {
    /// The button that submitted, if one did.
    pub submitter: Option<ElementInternals>,
    /// The gpui event behind the submit, when one is.
    pub event: Option<InputEvent>,
}

/// What a browser shows when a submit finds a control invalid: the message,
/// beside the control. Headless, the form hands it to `on_report`.
#[derive(Clone, Debug)]
pub struct ValidityReport {
    /// The first invalid control nothing handled. Focus is already on it.
    pub control: ElementInternals,
    /// The element the message points at, whose bounds place it.
    pub anchor: Node,
    /// The control's validation message.
    pub message: SharedString,
}

/// `on_submit`: what the form submits, and the button it came from.
pub type SubmitCallback = Rc<dyn Fn(&FormData, &SubmitDetails, &mut Window, &mut App)>;
/// `on_report`: the invalid control a browser would point its message at.
pub type ReportCallback = Rc<dyn Fn(&ValidityReport, &mut Window, &mut App)>;

/// The options a form takes.
#[derive(Clone, Default)]
pub struct FormOptions {
    /// Its `id`, which a control outside it names in its `form` option to
    /// belong to it.
    pub id: Option<SharedString>,
    /// Submits without validating its controls first (`novalidate`).
    pub no_validate: bool,
    /// Runs when it submits, with what its controls submit, in page order.
    /// It runs inside whatever update submitted, so it may update any entity
    /// but the one whose press or key submitted.
    pub on_submit: Option<SubmitCallback>,
    /// Runs when it's about to reset. `event.prevent_default()` stops the
    /// reset.
    pub on_reset: Option<FormListener>,
    /// Runs when a submit, or `report_validity`, finds a control invalid and
    /// nothing canceled its `Invalid`: where a browser shows its message.
    pub on_report: Option<ReportCallback>,
}

impl FormOptions {
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn no_validate(mut self, no_validate: bool) -> Self {
        self.no_validate = no_validate;
        self
    }

    pub fn on_submit(
        mut self,
        callback: impl Fn(&FormData, &SubmitDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_submit = Some(Rc::new(callback));
        self
    }

    pub fn on_reset(mut self, callback: impl Fn(&FormEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_reset = Some(Rc::new(callback));
        self
    }

    pub fn on_report(
        mut self,
        callback: impl Fn(&ValidityReport, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_report = Some(Rc::new(callback));
        self
    }
}

#[derive(Default)]
pub(crate) struct FormFacts {
    options: FormOptions,
    // Submitting now, so a submit from inside it does nothing.
    submitting: bool,
    // A control of the current submission took focus (`claimInvalidFocus`).
    pub(crate) claimed: bool,
}

/// A form, standing in for `<form>`. The controls rendered inside its element,
/// or naming its id in their `form` option, belong to it.
#[derive(Clone, PartialEq)]
pub struct Form {
    node: Node,
}

impl Form {
    /// A form, which its controls join once it renders with them inside.
    pub fn new(options: FormOptions) -> Self {
        let form = Self {
            node: Node::with_kind(NodeKind::Form),
        };
        form.node.set_id_quiet(options.id.clone());
        form.facts_mut().options = options;
        form
    }

    pub(crate) fn from_node(node: Node) -> Self {
        Self { node }
    }

    /// The form a node stands for, if it stands for one.
    pub fn of(node: &Node) -> Option<Self> {
        (node.kind() == NodeKind::Form).then(|| Self { node: node.clone() })
    }

    fn facts(&self) -> std::cell::Ref<'_, FormFacts> {
        self.node.0.form.as_ref().expect("a form node holds form facts").borrow()
    }

    pub(crate) fn facts_mut(&self) -> std::cell::RefMut<'_, FormFacts> {
        self.node
            .0
            .form
            .as_ref()
            .expect("a form node holds form facts")
            .borrow_mut()
    }

    /// Hands it new options, as a framework re-renders with new props.
    pub fn set_options(&self, options: FormOptions, cx: &mut App) {
        let id = options.id.clone();
        self.facts_mut().options = options;
        self.node.set_id(id, cx);
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    pub fn id(&self) -> Option<SharedString> {
        self.node.id()
    }

    /// Puts the form in `el`, the element it renders as. Call it before
    /// adding children.
    pub fn track<E: ParentElement + StatefulInteractiveElement>(&self, el: E) -> E {
        self.node.track(el)
    }

    /// A div that is the form. Style it and add its controls.
    pub fn root(&self) -> Stateful<Div> {
        self.track(div().id(ElementId::NamedInteger("ui-sdk-form".into(), self.node.key())))
    }

    /// Calls `listener` for each event of `kind` that reaches the form: its
    /// own `Submit` and `Reset`, and its controls' `Input` and `Change`.
    pub fn listen(
        &self,
        kind: FormEventType,
        listener: impl Fn(&FormEvent, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        self.node.listen(kind, false, listener)
    }

    /// Its controls, in page order, as of the last frame (`elements`).
    pub fn elements(&self) -> Vec<ElementInternals> {
        placed_of(NodeKind::Control)
            .iter()
            .filter_map(ElementInternals::of)
            .filter(|control| control.form().as_ref() == Some(self))
            .collect()
    }

    /// What it submits now, with no submitter: each enabled control's entries
    /// under its name, in page order (`new FormData(form)`).
    pub fn data(&self) -> FormData {
        self.data_with(None)
    }

    /// What it submits from `submitter`, which adds the button's own entry.
    pub fn data_with(&self, submitter: Option<&ElementInternals>) -> FormData {
        let mut data = FormData::default();
        for control in self.elements() {
            if control.is_disabled() {
                continue;
            }
            // Only the button that submitted submits its value.
            if control.button().is_some() && Some(&control) != submitter {
                continue;
            }
            let name = control.name();
            match control.value() {
                FormValue::None => {}
                FormValue::Text(value) => {
                    if let Some(name) = name {
                        data.append(name, value);
                    }
                }
                FormValue::List(values) => {
                    if let Some(name) = name {
                        for value in values {
                            data.append(name.clone(), value);
                        }
                    }
                }
                FormValue::Data(entries) => {
                    for (name, value) in entries.entries {
                        data.append(name, value);
                    }
                }
            }
        }
        data
    }

    /// Fires `Invalid` at each invalid control and returns whether none is,
    /// `checkValidity`.
    pub fn check_validity(&self, window: &mut Window, cx: &mut App) -> bool {
        self.validate(false, window, cx)
    }

    /// Like [`Form::check_validity`], and focuses the first invalid control
    /// whose `Invalid` nothing canceled and reports it, `reportValidity`.
    pub fn report_validity(&self, window: &mut Window, cx: &mut App) -> bool {
        self.validate(true, window, cx)
    }

    fn validate(&self, report: bool, window: &mut Window, cx: &mut App) -> bool {
        let invalid: Vec<ElementInternals> = self
            .elements()
            .into_iter()
            .filter(|control| control.will_validate() && !control.validity().valid())
            .collect();
        let mut unhandled = Vec::new();
        for control in &invalid {
            let event = FormEvent::new(FormEventType::Invalid, control.node().clone());
            dispatch(&event, window, cx);
            if !event.default_prevented() {
                unhandled.push(control.clone());
            }
        }
        if report && let Some(first) = unhandled.first() {
            focus_control(first.node(), window, cx);
            self.report(first, window, cx);
        }
        invalid.is_empty()
    }

    pub(crate) fn report(&self, control: &ElementInternals, window: &mut Window, cx: &mut App) {
        let on_report = self.facts().options.on_report.clone();
        if let Some(on_report) = on_report {
            let report = ValidityReport {
                control: control.clone(),
                anchor: control.anchor(),
                message: control.validation_message(),
            };
            on_report(&report, window, cx);
        }
    }

    /// Submits as a submit button or Enter would, `requestSubmit`: validates
    /// its controls unless `no_validate` is set, reporting the first invalid
    /// one, then fires `Submit`, and unless that's canceled, hands what it
    /// submits to `on_submit`. `submitter` is the submit button pressed, which
    /// must belong to the form.
    pub fn request_submit(
        &self,
        submitter: Option<&ElementInternals>,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(submitter) = submitter
            && (submitter.button() != Some(ButtonType::Submit) || submitter.form().as_ref() != Some(self))
        {
            if cfg!(debug_assertions) {
                log::warn!("[ui-sdk] request_submit: the submitter is not a submit button of this form");
            }
            return;
        }
        if std::mem::replace(&mut self.facts_mut().submitting, true) {
            return;
        }
        let no_validate = self.facts().options.no_validate;
        if no_validate || self.validate(true, window, cx) {
            let submit = FormEvent::new(FormEventType::Submit, self.node.clone())
                .with_submitter(submitter.map(|submitter| submitter.node().clone()))
                .with_input(event.clone());
            dispatch(&submit, window, cx);
            if !submit.default_prevented() {
                self.deliver(submitter, event, window, cx);
            }
        }
        self.facts_mut().submitting = false;
    }

    /// Submits without validating or firing `Submit`, `submit()`.
    pub fn submit(&self, window: &mut Window, cx: &mut App) {
        if std::mem::replace(&mut self.facts_mut().submitting, true) {
            return;
        }
        self.deliver(None, None, window, cx);
        self.facts_mut().submitting = false;
    }

    fn deliver(
        &self,
        submitter: Option<&ElementInternals>,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let on_submit = self.facts().options.on_submit.clone();
        if let Some(on_submit) = on_submit {
            let data = self.data_with(submitter);
            let details = SubmitDetails {
                submitter: submitter.cloned(),
                event,
            };
            on_submit(&data, &details, window, cx);
        }
    }

    /// Resets it, `reset()`: fires `Reset`, and unless that's canceled, puts
    /// every control back to its default, in page order, once the current
    /// update is done. What `after_reset` queued runs after them.
    pub fn reset(&self, window: &mut Window, cx: &mut App) {
        let event = FormEvent::new(FormEventType::Reset, self.node.clone());
        let on_reset = self.facts().options.on_reset.clone();
        if let Some(on_reset) = on_reset {
            on_reset(&event, window, cx);
        }
        dispatch(&event, window, cx);
        if event.default_prevented() {
            return;
        }
        let after = std::mem::take(&mut *event.after.borrow_mut());
        let form = self.clone();
        window.defer(cx, move |window, cx| {
            for control in form.elements() {
                for reset in control.reset_callbacks() {
                    reset(window, cx);
                }
            }
            for run in after {
                run(window, cx);
            }
        });
    }
}

impl std::fmt::Debug for Form {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Form").field("node", &self.node).finish()
    }
}

/// One value of a `FormData` entry: text, or a file.
#[derive(Clone, Debug, PartialEq)]
pub enum FormDataEntryValue {
    Text(SharedString),
    File(FormFile),
}

impl FormDataEntryValue {
    pub fn as_text(&self) -> Option<&SharedString> {
        match self {
            FormDataEntryValue::Text(text) => Some(text),
            FormDataEntryValue::File(_) => None,
        }
    }

    pub fn as_file(&self) -> Option<&FormFile> {
        match self {
            FormDataEntryValue::Text(_) => None,
            FormDataEntryValue::File(file) => Some(file),
        }
    }
}

impl From<&str> for FormDataEntryValue {
    fn from(value: &str) -> Self {
        FormDataEntryValue::Text(SharedString::from(value.to_string()))
    }
}

impl From<String> for FormDataEntryValue {
    fn from(value: String) -> Self {
        FormDataEntryValue::Text(value.into())
    }
}

impl From<SharedString> for FormDataEntryValue {
    fn from(value: SharedString) -> Self {
        FormDataEntryValue::Text(value)
    }
}

impl From<FormFile> for FormDataEntryValue {
    fn from(file: FormFile) -> Self {
        FormDataEntryValue::File(file)
    }
}

/// Name and value pairs, in the order a form's controls submit them, like
/// the browser's `FormData`. A name may appear more than once, like a
/// checkbox group's or a multiple select's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormData {
    entries: Vec<(SharedString, FormDataEntryValue)>,
}

impl FormData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an entry after the others.
    pub fn append(&mut self, name: impl Into<SharedString>, value: impl Into<FormDataEntryValue>) {
        self.entries.push((name.into(), value.into()));
    }

    /// Replaces the first entry under `name` and removes the rest, or adds one.
    pub fn set(&mut self, name: impl Into<SharedString>, value: impl Into<FormDataEntryValue>) {
        let name = name.into();
        let value = value.into();
        match self.entries.iter().position(|(each, _)| *each == name) {
            Some(first) => {
                self.entries[first].1 = value;
                let mut index = 0;
                self.entries.retain(|(each, _)| {
                    let keep = index <= first || *each != name;
                    index += 1;
                    keep
                });
            }
            None => self.entries.push((name, value)),
        }
    }

    pub fn delete(&mut self, name: &str) {
        self.entries.retain(|(each, _)| each.as_ref() != name);
    }

    /// The first value under `name`.
    pub fn get(&self, name: &str) -> Option<&FormDataEntryValue> {
        self.entries
            .iter()
            .find(|(each, _)| each.as_ref() == name)
            .map(|(_, value)| value)
    }

    /// Every value under `name`, in order.
    pub fn get_all(&self, name: &str) -> Vec<&FormDataEntryValue> {
        self.entries
            .iter()
            .filter(|(each, _)| each.as_ref() == name)
            .map(|(_, value)| value)
            .collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.entries.iter().any(|(each, _)| each.as_ref() == name)
    }

    pub fn entries(&self) -> &[(SharedString, FormDataEntryValue)] {
        &self.entries
    }

    pub fn keys(&self) -> impl Iterator<Item = &SharedString> {
        self.entries.iter().map(|(name, _)| name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries as a browser posts them by default,
    /// `application/x-www-form-urlencoded`. A file goes as its name.
    pub fn to_url_encoded(&self) -> String {
        let mut out = String::new();
        for (index, (name, value)) in self.entries.iter().enumerate() {
            if index > 0 {
                out.push('&');
            }
            let value = match value {
                FormDataEntryValue::Text(text) => text.to_string(),
                FormDataEntryValue::File(file) => file.name.to_string(),
            };
            url_encode(&normalize_newlines(name), &mut out);
            out.push('=');
            url_encode(&normalize_newlines(&value), &mut out);
        }
        out
    }
}

// Line breaks in a submitted entry go as CR LF.
fn normalize_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("\r\n");
            }
            '\n' => out.push_str("\r\n"),
            c => out.push(c),
        }
    }
    out
}

// The urlencoded byte serializer: spaces as `+`, and bytes other than
// letters, digits and `*-._` percent-encoded.
fn url_encode(text: &str, out: &mut String) {
    for byte in text.bytes() {
        match byte {
            b' ' => out.push('+'),
            b'*' | b'-' | b'.' | b'_' => out.push(byte as char),
            byte if byte.is_ascii_alphanumeric() => out.push(byte as char),
            byte => out.push_str(&format!("%{byte:02X}")),
        }
    }
}

/// A fieldset, standing in for `<fieldset>`. Its `disabled` disables every
/// control inside it, except those in its first legend, and leaves them out
/// of the form. A fieldset inside a disabled one is disabled too.
#[derive(Clone, PartialEq, Debug)]
pub struct Fieldset {
    node: Node,
}

impl Default for Fieldset {
    fn default() -> Self {
        Self::new()
    }
}

impl Fieldset {
    pub fn new() -> Self {
        Self {
            node: Node::with_kind(NodeKind::Fieldset),
        }
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    /// Disables or enables the controls inside it. They hear it once the
    /// current update is done.
    pub fn set_disabled(&self, disabled: bool, cx: &mut App) {
        if self.node.set_fieldset_disabled_flag(disabled) {
            invalidate(cx);
        }
    }

    /// Its own `disabled`.
    pub fn disabled(&self) -> bool {
        self.node.fieldset_disabled_flag()
    }

    /// Whether it's disabled, by itself or a disabled fieldset around it
    /// (`:disabled`).
    pub fn is_disabled(&self) -> bool {
        self.disabled() || self.node.form_disabled()
    }

    /// Its first legend, which names it and whose controls stay enabled.
    pub fn legend(&self) -> Option<Legend> {
        placed_of(NodeKind::Legend)
            .into_iter()
            .find(|legend| {
                legend
                    .ancestors()
                    .into_iter()
                    .find(|node| node.kind() == NodeKind::Fieldset)
                    .is_some_and(|nearest| nearest == self.node)
            })
            .map(|node| Legend { node })
    }

    /// Puts the fieldset in `el`, the element it renders as. Call it before
    /// adding children.
    pub fn track<E: ParentElement + StatefulInteractiveElement>(&self, el: E) -> E {
        self.node.track(el)
    }

    /// A group that is the fieldset, named by its legend.
    pub fn root(&self) -> PartElement<Stateful<Div>> {
        let el = self.track(
            div()
                .id(ElementId::NamedInteger("ui-sdk-fieldset".into(), self.node.key()))
                .role(Role::Group),
        );
        let mut aria = Aria::new();
        if let Some(legend) = self.legend() {
            aria.labelled_by = legend.node.element_id().into_iter().collect();
            if !legend.text().is_empty() {
                aria = aria.fallback_label(legend.text());
            }
        }
        with_aria(el, aria)
    }
}

/// A legend, standing in for `<legend>`. The first one in a fieldset names
/// it, and the controls inside it stay enabled when the fieldset is disabled.
#[derive(Clone, PartialEq, Debug)]
pub struct Legend {
    node: Node,
}

impl Legend {
    /// A legend that reads as `text`.
    pub fn new(text: impl Into<SharedString>) -> Self {
        let node = Node::with_kind(NodeKind::Legend);
        node.set_text(text.into());
        Self { node }
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    pub fn text(&self) -> SharedString {
        self.node.text()
    }

    pub fn set_text(&self, text: impl Into<SharedString>, cx: &mut App) {
        if self.node.set_text(text.into()) {
            invalidate(cx);
        }
    }

    /// Puts the legend in `el`, the element it renders as. Call it before
    /// adding children.
    pub fn track<E: ParentElement + StatefulInteractiveElement>(&self, el: E) -> E {
        self.node.track(el)
    }

    /// A div that is the legend, which its fieldset's name points at.
    pub fn root(&self) -> PartElement<Stateful<Div>> {
        with_aria(
            self.track(
                div()
                    .id(ElementId::NamedInteger("ui-sdk-legend".into(), self.node.key()))
                    .role(Role::Legend),
            ),
            Aria::new(),
        )
    }
}

/// The options a label takes.
#[derive(Clone, Debug, Default)]
pub struct LabelOptions {
    /// Its `id`. Without one it has a made one.
    pub id: Option<SharedString>,
    /// The id of the control it names (`for`). Without one, it names the
    /// first control inside it.
    pub for_id: Option<SharedString>,
    /// What it reads as. Its control points at the label's element, which
    /// names it when that's a part in the page, and takes this text when
    /// it's not.
    pub text: SharedString,
}

impl LabelOptions {
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn for_id(mut self, id: impl Into<SharedString>) -> Self {
        self.for_id = Some(id.into());
        self
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = text.into();
        self
    }
}

/// A label, standing in for `<label>`. It names its control, and a press on
/// it activates the control the way a native label does.
#[derive(Clone, PartialEq, Debug)]
pub struct Label {
    node: Node,
}

impl Label {
    /// A label, which names its control once both render.
    pub fn new(options: LabelOptions) -> Self {
        let label = Self {
            node: Node::with_kind(NodeKind::Label),
        };
        label.node.set_id_quiet(options.id);
        label.node.set_for_id(options.for_id);
        label.node.set_text(options.text);
        label
    }

    pub(crate) fn from_node(node: Node) -> Self {
        Self { node }
    }

    /// The label a node stands for, if it stands for one.
    pub fn of(node: &Node) -> Option<Self> {
        (node.kind() == NodeKind::Label).then(|| Self { node: node.clone() })
    }

    /// Hands it new options. Its control hears a new text or `for` once the
    /// current update is done.
    pub fn set_options(&self, options: LabelOptions, cx: &mut App) {
        let changed = self.node.set_for_id(options.for_id) | self.node.set_text(options.text);
        self.node.set_id(options.id, cx);
        if changed {
            invalidate(cx);
        }
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    /// Its id: its own, or one made for it.
    pub fn id(&self) -> SharedString {
        self.node
            .id()
            .unwrap_or_else(|| format!("ui-sdk-label-{}", self.node.key()).into())
    }

    pub fn text(&self) -> SharedString {
        self.node.text()
    }

    pub fn for_id(&self) -> Option<SharedString> {
        self.node.for_id()
    }

    /// The id of the element it renders as, which its control's
    /// `labelled_by` names, as of the last frame.
    pub fn element_id(&self) -> Option<ElementId> {
        self.node.element_id()
    }

    /// The control it names, as of the last frame.
    pub fn control(&self) -> Option<Node> {
        self.node.labeled_control()
    }

    /// What a press on it does: activates its control, unless the control is
    /// disabled or the press landed on the control or another control inside
    /// the label, which handle their own presses.
    pub fn press(&self, event: &ClickEvent, window: &mut Window, cx: &mut App) {
        let Some(control) = self.control() else {
            return;
        };
        if control.is_disabled() || presses_inside(event, &self.node, &control) {
            return;
        }
        activate_control(&control, Some(InputEvent::from(event)), window, cx);
    }

    /// Puts the label in `el`, the element it renders as. Call it before
    /// adding children.
    pub fn track<E: ParentElement + StatefulInteractiveElement>(&self, el: E) -> E {
        self.node.track(el)
    }

    /// Puts the label in `el` and gives it its press.
    pub fn bind<E: ParentElement + StatefulInteractiveElement>(&self, el: E) -> E {
        let label = self.clone();
        self.track(el)
            .on_click(move |event, window, cx| label.press(event, window, cx))
    }

    /// A div that is the label, which its control's name points at.
    pub fn root(&self) -> PartElement<Stateful<Div>> {
        with_aria(
            self.bind(
                div()
                    .id(ElementId::NamedInteger("ui-sdk-label".into(), self.node.key()))
                    .role(Role::Label),
            ),
            Aria::new(),
        )
    }
}

// A press on the control, or on another control inside the label, is
// theirs, as on a native label.
fn presses_inside(event: &ClickEvent, label: &Node, control: &Node) -> bool {
    if matches!(event, ClickEvent::Keyboard(_)) {
        return false;
    }
    let position = event.position();
    let hit = |node: &Node| node.bounds().is_some_and(|bounds| bounds.contains(&position));
    if hit(control) {
        return true;
    }
    label
        .descendants()
        .iter()
        .any(|node| node.kind() == NodeKind::Control && hit(node))
}
