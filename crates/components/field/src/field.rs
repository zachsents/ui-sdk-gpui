//! Port of `components/field/field.ts`: ties a control to its label,
//! description and errors.
//!
//! Field shows the browser's own validation once a submit finds the control
//! invalid, or the invalid state your code passes. Values, custom rules and
//! submitting stay with the control and whatever owns the form.
//!
//! The control is the first one inside the field's root, not counting
//! nested fields or plain buttons, or the field's own `control` part around
//! a control of the app's own. Once a frame has drawn, the field finds it, reads the validity
//! of the controls inside it, and writes on the control what names and
//! describes it ([`set_control_attrs`](ui_sdk_forms::set_control_attrs)), which every control renders through
//! its internals' ARIA.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{
    App, ClickEvent, Context, ElementId, Entity, FocusHandle, InteractiveElement, MouseButton,
    Role, SharedString, StatefulInteractiveElement, Subscription, Window,
};
use ui_sdk_forms::{
    ElementInternals, Form, FormEvent, FormEventType, LabelOptions, Node, NodeKind,
    activate_control, after_reset, claim_invalid_focus, component_flag_of, field_control_id,
    find_control, invalid_focus_claimed, is_labelable, listen_document, on_validity_change,
    watch_fieldsets,
};
use ui_sdk_kernel::bound_keys::bound;
use ui_sdk_kernel::focus_events::{on_focus_in, on_focus_out};
use ui_sdk_kernel::part::with_role;
use ui_sdk_kernel::{Agent, AgentState, InputEvent, Instance, Part, key_while_bound, while_destroying};
use ui_sdk_platform::announce;

use crate::field_attrs::{ControlBinding, FieldStore, error_shown, error_state, field_state};
use crate::field_control::{
    ControlValidity, Validity, ValidityKey, controls_in, is_required, valid, validity_of,
};
use crate::field_set::FieldSetState;

/// The options `Field` takes.
#[derive(Clone, Debug, Default)]
pub struct FieldOptions {
    /// The name the control submits under, when the control doesn't set its
    /// own.
    pub name: Option<SharedString>,
    /// Disables the control, and marks every part.
    pub disabled: bool,
    /// Whether the field shows as invalid, when your code decides, like a
    /// form library or an error from the server. `Some(true)` shows its
    /// errors now, `Some(false)` shows none whatever the browser thinks.
    /// Leave it `None` to show the browser's own validation once a submit
    /// finds the field invalid.
    pub invalid: Option<bool>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl FieldOptions {
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = Some(invalid);
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// When an error shows (`match`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldErrorMatch {
    /// Always (`true`).
    Always,
    /// Never (`false`).
    Never,
    /// Only for this problem of the browser's own validation, like
    /// `ValueMissing`.
    Flag(ValidityKey),
}

impl From<bool> for FieldErrorMatch {
    fn from(shows: bool) -> Self {
        if shows {
            FieldErrorMatch::Always
        } else {
            FieldErrorMatch::Never
        }
    }
}

impl From<ValidityKey> for FieldErrorMatch {
    fn from(flag: ValidityKey) -> Self {
        FieldErrorMatch::Flag(flag)
    }
}

/// The props the `error` part takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldErrorProps {
    /// When this error shows. `Always` shows it always and `Never` never. A
    /// validity flag, like `ValueMissing`, shows it only for that problem of
    /// the browser's own validation. Without it, any problem shows it.
    pub r#match: Option<FieldErrorMatch>,
    /// What the error says, as the part renders it: its name for assistive
    /// technology, and what the field announces when it appears or changes.
    /// Without it, the browser's message for the current problem, which an
    /// error with no text of its own shows.
    pub text: Option<SharedString>,
}

impl FieldErrorProps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn r#match(mut self, r#match: impl Into<FieldErrorMatch>) -> Self {
        self.r#match = Some(r#match.into());
        self
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = Some(text.into());
        self
    }
}

/// What every part's `with` and the app read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldState {
    /// Whether the field is disabled, by itself or by a fieldset.
    pub disabled: bool,
    /// Whether the control is required, as it reports.
    pub required: bool,
    /// Whether focus is inside the field.
    pub focused: bool,
    /// Whether the field shows as invalid: `invalid` when you pass it, else
    /// the browser's validation once shown.
    pub invalid: bool,
    /// Whether the browser's validation passes, once shown. Always false
    /// while you pass `invalid`.
    pub valid: bool,
}

/// What a description reads: its field's state, or only `disabled` when it
/// describes a field set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldDescriptionState {
    pub disabled: bool,
    pub required: Option<bool>,
    pub focused: Option<bool>,
    pub invalid: Option<bool>,
    pub valid: Option<bool>,
}

impl From<FieldState> for FieldDescriptionState {
    fn from(state: FieldState) -> Self {
        Self {
            disabled: state.disabled,
            required: Some(state.required),
            focused: Some(state.focused),
            invalid: Some(state.invalid),
            valid: Some(state.valid),
        }
    }
}

impl From<FieldSetState> for FieldDescriptionState {
    fn from(state: FieldSetState) -> Self {
        Self {
            disabled: state.disabled,
            ..Default::default()
        }
    }
}

/// What the `error` part's `with` reads: the field's state and the message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldErrorState {
    pub disabled: bool,
    pub required: bool,
    pub focused: bool,
    pub invalid: bool,
    pub valid: bool,
    /// The browser's message for the current problem. Text you give shows
    /// instead.
    pub message: SharedString,
}

// An error the app's last render asked for, shown or not, and the render
// that asked.
struct Held {
    props: FieldErrorProps,
    pass: u64,
}

/// A field. Its entity is its controller.
pub struct Field {
    instance: Instance,
    store: FieldStore,
    // The root's place in the page, which says what's inside the field and
    // around it.
    root: Node,
    // Tracked by the root, so the field hears focus come and go inside it.
    focus_handle: FocusHandle,
    // What the label part is to the forms code: a label for the control,
    // whose `for` names it when it can take one.
    label: ui_sdk_forms::Label,
    // What a control of the app's own takes part in forms with, through the
    // `control` part.
    control_internals: ElementInternals,
    binding: ControlBinding,
    // Errors the last render asked for, which `match` an invalid submit
    // reads.
    held: RefCell<HashMap<SharedString, Held>>,
    pass: u64,
    // What each error in the page last said.
    said: HashMap<SharedString, SharedString>,
    // Errors in the first render, or revealed by a submit, aren't announced:
    // the page loading or focus moving to the invalid control reads them.
    // Errors that join before this settle are quiet.
    quiet_until: u64,
    settles: u64,
    queued: bool,
    _subscriptions: Vec<Subscription>,
}

impl Field {
    pub fn new(options: FieldOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("field").with_agent(cx);
        instance.debug = options.debug;
        let root = Node::tagged("field");
        let focus_handle = cx.focus_handle();
        let weak = cx.weak_entity();
        let mut subscriptions = vec![
            watch_fieldsets(&root, {
                let weak = weak.clone();
                move |disabled, _window, cx| {
                    weak.update(cx, |field, cx| {
                        if field.store.form_disabled != disabled {
                            field.store.form_disabled = disabled;
                            cx.notify();
                        }
                    })
                    .ok();
                }
            }),
            on_focus_in(&focus_handle, window, cx, |field, _window, cx| {
                field.set_focused(true, cx)
            }),
            on_focus_out(&focus_handle, window, cx, |field, _event, _window, cx| {
                field.set_focused(false, cx)
            }),
            // A submit, or `report_validity`, found the field invalid. It
            // doesn't travel back out, so the field hears it on the way in.
            root.listen(FormEventType::Invalid, true, {
                let weak = weak.clone();
                move |event, window, cx| {
                    weak.update(cx, |field, cx| field.invalid(event, window, cx)).ok();
                }
            }),
            // A form reset hides the browser's validation again.
            listen_document(FormEventType::Reset, true, {
                let weak = weak.clone();
                move |event, _window, cx| {
                    let Some(field) = weak.upgrade() else {
                        return;
                    };
                    let ours = field
                        .read(cx)
                        .form()
                        .is_some_and(|form| form.node() == event.target());
                    if !ours {
                        return;
                    }
                    let weak = weak.clone();
                    after_reset(event, move |_window, cx| {
                        weak.update(cx, |field, cx| {
                            field.store.revealed = false;
                            field.read_validity(controls_in(&field.root));
                            cx.notify();
                        })
                        .ok();
                    });
                }
            }),
            // A component's own check, like a date picker's text that isn't a
            // date, changes its control's validity without an event.
            on_validity_change({
                let weak = weak.clone();
                let root = root.downgrade();
                move |control, _window, cx| {
                    if root.upgrade().is_some_and(|root| root.contains(control)) {
                        weak.update(cx, |field, cx| field.check(cx)).ok();
                    }
                }
            }),
        ];
        // A user's change to a control inside, once the control took it.
        for kind in [FormEventType::Input, FormEventType::Change] {
            let weak = weak.clone();
            let root = root.downgrade();
            subscriptions.push(listen_document(kind, false, move |event, _window, cx| {
                if root.upgrade().is_some_and(|root| root.contains(event.target())) {
                    weak.update(cx, |field, cx| field.check(cx)).ok();
                }
            }));
        }
        subscriptions.push(cx.on_release(|field, cx| while_destroying(|| field.binding.release(cx))));
        Self {
            instance,
            store: FieldStore {
                disabled: options.disabled,
                name: options.name,
                external: options.invalid,
                validity: valid(),
                labelable: true,
                ..FieldStore::default()
            },
            root,
            focus_handle,
            label: ui_sdk_forms::Label::new(LabelOptions::default()),
            control_internals: ElementInternals::new(cx),
            binding: ControlBinding::default(),
            held: RefCell::new(HashMap::new()),
            pass: 0,
            said: HashMap::new(),
            quiet_until: 1,
            settles: 0,
            queued: false,
            _subscriptions: subscriptions,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). Passing `invalid`, or no longer passing it, starts the
    /// browser's validation over.
    pub fn set_options(&mut self, options: FieldOptions, cx: &mut Context<Self>) {
        let restart = self.store.external.is_none() != options.invalid.is_none();
        let changed = self.store.disabled != options.disabled
            || self.store.name != options.name
            || self.store.external != options.invalid;
        self.store.disabled = options.disabled;
        self.store.name = options.name;
        self.store.external = options.invalid;
        self.instance.debug = options.debug;
        if restart {
            self.store.revealed = false;
            self.check(cx);
        }
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The field's id, which the id it gives its control starts with.
    pub fn id(&self) -> &SharedString {
        self.instance.id()
    }

    /// The name a control of the app's own submits under, when it doesn't
    /// set its own.
    pub fn name(&self) -> Option<&SharedString> {
        self.store.name.as_ref()
    }

    pub fn state(&self) -> FieldState {
        field_state(&self.store)
    }

    /// Whether errors show: what you pass, or the browser's once a submit
    /// found the field invalid.
    pub fn shown(&self) -> bool {
        self.store.shown()
    }

    /// Whether it's disabled, by itself or by a fieldset.
    pub fn is_disabled(&self) -> bool {
        self.store.is_disabled()
    }

    /// Whether its control is invalid: `invalid` when you pass it, else the
    /// browser's validation, shown or not.
    pub fn is_invalid(&self) -> bool {
        self.store.invalid()
    }

    /// The validity of its controls as it last read it.
    pub fn validity(&self) -> &Validity {
        &self.store.validity
    }

    /// The browser's message for the current problem, which an error with
    /// no text of its own shows.
    pub fn message(&self) -> &SharedString {
        &self.store.validity.message
    }

    /// What a control of the app's own takes part in forms with: put it in
    /// the control's element with the `control` part, and report the
    /// control's value and validity through it, like with
    /// `ui_sdk_forms::bind_value_input`.
    pub fn control_internals(&self) -> &ElementInternals {
        &self.control_internals
    }

    /// The control its label, descriptions and errors are wired to: its
    /// `control` part while that's in the page, else the first control
    /// inside it, not counting nested fields or plain buttons (`control` in
    /// the original's api).
    pub fn control_node(&self) -> Option<Node> {
        let own = self.control_internals.node();
        if own.is_connected() {
            return Some(own.clone());
        }
        let root = &self.root;
        find_control(root, |node| owns(root, node))
    }

    /// Activates the control the way its label does: checks a checkbox or a
    /// switch, presses a plain button, and focuses anything else. A disabled
    /// field does nothing. Call it outside the field's own update.
    pub fn activate(this: &Entity<Self>, event: Option<InputEvent>, window: &mut Window, cx: &mut App) {
        let Some(control) = this.read(cx).activation_target() else {
            return;
        };
        activate_control(&control, event, window, cx);
    }

    fn activation_target(&self) -> Option<Node> {
        let control = self.control_node()?;
        (!self.store.is_disabled()).then_some(control)
    }

    // What a press on the label activates. A label whose `for` names its
    // control does what a native one does: nothing when the control is
    // disabled, or when the press landed on it or on another control inside
    // the label. Any other activates the field's control all the same.
    fn label_target(&self, event: &InputEvent) -> Option<Node> {
        if let Some(control) = self.label.control() {
            if control.is_disabled() || presses_inside(event, self.label.node(), &control) {
                return None;
            }
            return Some(control);
        }
        self.activation_target()
    }

    fn set_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.store.focused != focused {
            self.store.focused = focused;
            cx.notify();
        }
    }

    // The form the field's controls belong to.
    fn form(&self) -> Option<Form> {
        controls_in(&self.root)
            .first()
            .and_then(ElementInternals::form)
            .or_else(|| self.root.form())
    }

    // Errors that join the page by the next settle aren't announced.
    fn hush(&mut self) {
        self.quiet_until = self.settles + 1;
    }

    // Reads the controls' validity once the current update is done, however
    // often it's asked (`check`).
    fn check(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.queued, true) {
            return;
        }
        let weak = cx.weak_entity();
        cx.defer(move |cx| {
            weak.update(cx, |field, cx| {
                field.queued = false;
                if field.read_validity(controls_in(&field.root)) {
                    cx.notify();
                }
            })
            .ok();
        });
    }

    // Reads its controls' validity: the first one that fails, with a
    // component's own flag in place of `CustomError`, and whether any is
    // required. Returns whether either changed.
    fn read_validity(&mut self, controls: Vec<ElementInternals>) -> bool {
        let mut next = valid();
        for control in &controls {
            let ControlValidity {
                will_validate,
                validity,
                message,
            } = validity_of(control);
            if !will_validate || validity.valid() {
                continue;
            }
            let mut flags = validity;
            if let Some(flag) = component_flag_of(control.node())
                && flags.custom_error
            {
                flags.custom_error = false;
                flags.set(flag, true);
            }
            next = Validity {
                valid: false,
                flags,
                message,
            };
            break;
        }
        let required = controls.iter().any(is_required);
        let changed = required != self.store.required || next != self.store.validity;
        self.store.required = required;
        self.store.validity = next;
        changed
    }

    // Whether an error the app renders shows now, with its current `match`,
    // in place of the browser's message. Only errors the last render asked
    // for count, so one that just left never does.
    fn error_in_page(&self) -> bool {
        self.held
            .borrow()
            .values()
            .any(|held| error_shown(&self.store, &held.props))
    }

    // A submit, or `report_validity`, found the field invalid.
    fn invalid(&mut self, event: &FormEvent, window: &mut Window, cx: &mut Context<Self>) {
        let target = event.target().clone();
        self.hush();
        self.store.revealed = true;
        self.read_validity(controls_in(&self.root));
        cx.notify();
        let form = self.form();
        // An error shown in the page replaces the browser's own message, and
        // the first invalid field of the submission takes focus. Without
        // one, the browser keeps its message, and a later field doesn't take
        // focus from the first.
        if !self.error_in_page() {
            if invalid_focus_claimed(&target, form.as_ref()) {
                event.prevent_default();
            }
            return;
        }
        event.prevent_default();
        claim_invalid_focus(&target, form.as_ref(), window, cx);
    }

    // Once a frame has drawn the field: the parts in the page, the
    // controls' validity, the control and what the field writes on it, and
    // what its errors say. The original does this as its parts bind and its
    // store changes, a microtask later.
    fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let instance = &self.instance;
        self.store.labels = if bound(instance, "label") {
            vec![instance.element_id("label", None)]
        } else {
            Vec::new()
        };
        self.store.descriptions = parts_of(instance, "description");
        self.store.errors = parts_of(instance, "error");
        let pass = self.pass;
        self.held.get_mut().retain(|_, held| held.pass == pass);
        self.pass += 1;
        // Controls that become required, disabled or enabled change their
        // validity without an event, and so does a control that leaves.
        let changed = self.read_validity(controls_in(&self.root));
        self.wire(cx);
        self.announce_errors(window, cx);
        self.settles += 1;
        if changed {
            cx.notify();
        }
    }

    // Finds the control, adopts it, writes its attributes, and points the
    // label's `for` at it (`bindFoundControl`).
    fn wire(&mut self, cx: &mut Context<Self>) {
        let own = self.control_internals.node().is_connected();
        let next = if own {
            Some(self.control_internals.clone())
        } else {
            self.control_node().as_ref().and_then(ElementInternals::of)
        };
        self.binding.find(next, cx);
        if let Some(control) = self.binding.current().cloned() {
            self.adopt(&control);
        }
        let fallback = field_control_id(self.instance.id());
        self.binding.write(&self.store, &self.root, own, fallback.clone(), cx);
        let for_id = self
            .store
            .labelable
            .then(|| self.store.control_id.clone().unwrap_or(fallback));
        if self.label.for_id() != for_id {
            self.label.set_options(
                LabelOptions {
                    for_id,
                    ..LabelOptions::default()
                },
                cx,
            );
        }
    }

    // Labels point at the control's id, or the one the field gives it when
    // it has none (`adopt`).
    fn adopt(&mut self, control: &ElementInternals) {
        // A plain button, like a file input's trigger, is named by the label
        // and its own text. Some name computations stop at a label's `for`
        // and drop the button's text, so the label activates it itself.
        let named_by_content = control.named_by_content().is_some();
        self.store.control_id = control.id();
        self.store.labelable = is_labelable(control.node()) && !named_by_content;
        self.store.named_by_content = named_by_content;
    }

    // Screen readers don't read an error that appears or changes while the
    // person stays where they are, so its text goes to the shared announcer.
    fn announce_errors(&mut self, window: &mut Window, cx: &mut App) {
        let quiet = self.settles < self.quiet_until;
        let mut before = std::mem::take(&mut self.said);
        let mut said = HashMap::new();
        let mut say = Vec::new();
        {
            let held = self.held.borrow();
            for key in key_while_bound(&self.instance, "error") {
                let text = held
                    .get(&key)
                    .map(|held| collapse(&error_text(&held.props, &self.store.validity.message)))
                    .unwrap_or_default();
                let last = match before.remove(&key) {
                    Some(last) => last,
                    // One that joins quietly says nothing until it changes.
                    None if quiet => text.clone(),
                    None => SharedString::default(),
                };
                if !text.is_empty() && text != last {
                    say.push(text.clone());
                    said.insert(key, text);
                } else {
                    said.insert(key, last);
                }
            }
        }
        self.said = said;
        for text in say {
            announce(&text, window, cx);
        }
    }

    /// The `root` part: the field, around its control and its other parts.
    /// It hears focus come and go inside it, and never takes focus itself:
    /// a press on it leaves focus where it is.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<FieldState> {
        let field = this.read(cx);
        let entity = this.downgrade();
        Part::new(&field.instance, "root", None, field.state())
            .map_el(|el| field.root.track(el).track_focus(&field.focus_handle))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_props(move |window, cx| {
                entity.update(cx, |field, cx| field.settle(window, cx)).ok();
            })
    }

    /// The `label` part, which names the control, as a native `<label for>`
    /// does. A press on it activates the control: checks a checkbox or a
    /// switch, presses a plain button, focuses anything else. A press on the
    /// control itself, or on another control inside the label, is theirs.
    /// Give it its text as an `aria_label`, which is the control's name.
    pub fn label(this: &Entity<Self>, cx: &App) -> Part<FieldState> {
        let field = this.read(cx);
        let entity = this.downgrade();
        Part::new(&field.instance, "label", None, field.state())
            .map_el(|el| with_role(field.label.track(el), Role::Label))
            .on_press(move |event, window, cx| {
                let Some(field) = entity.upgrade() else {
                    return;
                };
                if let Some(control) = field.read(cx).label_target(&event) {
                    activate_control(&control, Some(event), window, cx);
                }
            })
    }

    /// A `description` part, by a key of the app's own: text that describes
    /// the control, after the control's own description, while it's in the
    /// page. Give it its text as an `aria_label`.
    pub fn description(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part<FieldState> {
        let field = this.read(cx);
        let key: SharedString = key.into();
        Part::new(&field.instance, "description", Some(&key), field.state())
            .map_el(|el| with_role(el, Role::Label))
    }

    /// An `error` part, by a key of the app's own, while it shows: once a
    /// submit finds the field invalid, or while you pass `invalid`, for the
    /// problems its `match` names. `None` while it doesn't. It describes the
    /// control after the descriptions, and the field announces it when it
    /// appears or changes while the person stays where they are. Render
    /// `state.message` for the browser's message, or text of your own given
    /// as `text`.
    pub fn error(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: FieldErrorProps,
        cx: &App,
    ) -> Option<Part<FieldErrorState>> {
        let field = this.read(cx);
        let key: SharedString = key.into();
        field.held.borrow_mut().insert(
            key.clone(),
            Held {
                props: props.clone(),
                pass: field.pass,
            },
        );
        if !error_shown(&field.store, &props) {
            return None;
        }
        let state = error_state(&field.store);
        let text = error_text(&props, &state.message);
        let part = Part::new(&field.instance, "error", Some(&key), state)
            .map_el(|el| with_role(el, Role::Label));
        Some(if text.is_empty() { part } else { part.aria_label(text) })
    }

    /// The `control` part: a control of the app's own, like a native text
    /// field's root, wired like one of ours. It takes the field's control
    /// internals, which name it by the label, describe it by the
    /// descriptions and errors, and mark it invalid and disabled with the
    /// field; `focus` is where a label press and an invalid submit send
    /// focus. Report its value and validity through
    /// [`Field::control_internals`]. A plain button says so there with
    /// `set_named_by_content`, so the label names it and then its own text.
    pub fn control<S: 'static>(
        this: &Entity<Self>,
        part: Part<S>,
        focus: Option<&FocusHandle>,
        cx: &App,
    ) -> Part<S> {
        let field = this.read(cx);
        let internals = &field.control_internals;
        internals.set_focus_handle(focus.cloned());
        part.map_el(|el| internals.track(el)).aria(internals.aria())
    }
}

// Whether a control belongs to this field rather than one nested in it.
fn owns(root: &Node, control: &Node) -> bool {
    control.closest_tagged("field").as_ref() == Some(root)
}

// The element ids of a slot's parts in the page, in the order they joined.
fn parts_of(instance: &Instance, slot: &'static str) -> Vec<ElementId> {
    key_while_bound(instance, slot)
        .iter()
        .map(|key| instance.element_id(slot, Some(key.as_ref())))
        .collect()
}

// What an error says: its own text, or the browser's message.
fn error_text(props: &FieldErrorProps, message: &SharedString) -> SharedString {
    props.text.clone().unwrap_or_else(|| message.clone())
}

// Text as a person hears it, with runs of white space as one space.
fn collapse(text: &str) -> SharedString {
    text.split_whitespace().collect::<Vec<_>>().join(" ").into()
}

// A press on the control, or on another control inside the label, is
// theirs, as on a native label.
fn presses_inside(event: &InputEvent, label: &Node, control: &Node) -> bool {
    let InputEvent::Click(click) = event else {
        return false;
    };
    if matches!(click, ClickEvent::Keyboard(_)) {
        return false;
    }
    let position = click.position();
    let hit = |node: &Node| node.bounds().is_some_and(|bounds| bounds.contains(&position));
    hit(control)
        || label
            .descendants()
            .iter()
            .any(|node| node.kind() == NodeKind::Control && hit(node))
}

/// `agent: { shown }`: agents see a field only while it has errors in the
/// page, since its control carries its label. It takes no actions.
impl Agent for Field {
    fn agent_state(&self, _cx: &App) -> AgentState {
        let held = self.held.borrow();
        let errors: Vec<String> = key_while_bound(&self.instance, "error")
            .iter()
            .filter_map(|key| held.get(key))
            .map(|held| collapse(&error_text(&held.props, &self.store.validity.message)).to_string())
            .collect();
        AgentState::new()
            .with("disabled", self.store.is_disabled())
            .with("required", self.store.required)
            .with("errors", errors)
    }

    fn agent_shown(&self, _cx: &App) -> bool {
        !self.store.errors.is_empty()
    }
}
