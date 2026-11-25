//! Port of `foundation/forms/text-field.ts`: what every native text field
//! shares: Input, Textarea and the control of a composite like
//! PasswordInput. It reports changes with reasons, follows writes, resets and
//! controlled values, and marks whether it's filled.
//!
//! The original drives a native `<input>`, which takes part in its form by
//! itself. Here [`TextFieldApi`] drives a [`TextControl`], like the native
//! crate's text field, and takes part in the form for it: its value submits
//! under its name, `required` validates it, a reset puts its default back,
//! and Enter in a single-line field submits implicitly.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, ClickEvent, Context, SharedString, Subscription, Window};
use ui_sdk_kernel::{ChangeCallback, ChangeOptions, Controllable, InputEvent, Instance, Reason};

use crate::document::{FormEventType, Node, NodeKind};
use crate::form::{
    FormValue, ValueMissingKind, bind_internals, dispatch_change, focus_control, focus_with,
    submit_implicitly, value_missing_message,
};
use crate::form_internals::{Problem, InternalsOptions};
use crate::internals::ElementInternals;
use crate::value::{TextControl, ValueSource, notify_value, write_value};

/// Why a text field's value changed: typing (`Reason::InputChange`), a Clear
/// part (`Reason::ClearPress`), a form reset (`Reason::Reset`) or a
/// controller (`Reason::Imperative`).
pub type InputValueChangeReason = Reason;

/// The options every text field takes.
#[derive(Clone, Default)]
pub struct TextFieldOptions {
    /// The value, for a controlled field.
    pub value: Option<SharedString>,
    /// The value it starts with. A form reset returns here.
    pub default_value: Option<SharedString>,
    /// Called when the user changes the value: typing (`InputChange`), a
    /// Clear part (`ClearPress`), a form reset (`Reset`) or a controller
    /// (`Imperative`). It runs once the component's update is over, so it may
    /// update any entity, the component too.
    pub on_value_change: Option<ChangeCallback<SharedString>>,
    /// Runs as the value is asked to change, before it does, inside the
    /// component's update: `details.cancel()` keeps the value as it was. It
    /// may update any entity but the component.
    pub on_before_value_change: Option<ChangeCallback<SharedString>>,
}

impl TextFieldOptions {
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&SharedString, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&SharedString, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }
}

/// A text field's value, and whether it holds any.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextState {
    /// Changes on every keystroke, and renders nothing. `filled` renders.
    pub value: SharedString,
    pub filled: bool,
}

/// The state a text field starts with.
pub fn initial_text(options: &TextFieldOptions) -> TextState {
    let value = options
        .value
        .clone()
        .or_else(|| options.default_value.clone())
        .unwrap_or_default();
    TextState {
        filled: !value.is_empty(),
        value,
    }
}

/// The state new options set: the controlled value, when there is one.
pub fn sync_text(options: &TextFieldOptions) -> Option<TextState> {
    options.value.clone().map(|value| TextState {
        filled: !value.is_empty(),
        value,
    })
}

/// What a Clear part empties: a text field of ours, which reports the reason
/// `ClearPress`.
pub type ClearTarget = Rc<dyn Fn(Option<InputEvent>, &mut Window, &mut App)>;

thread_local! {
    static TEXT_FIELDS: RefCell<Vec<(u64, ClearTarget)>> = const { RefCell::new(Vec::new()) };
}

/// The text field a control's node belongs to, so a Clear part in its group
/// reaches it (`textFields`).
pub fn text_fields(node: &Node) -> Option<ClearTarget> {
    let key = node.key();
    TEXT_FIELDS.with(|fields| {
        fields
            .borrow()
            .iter()
            .find(|(each, _)| *each == key)
            .map(|(_, clear)| clear.clone())
    })
}

/// What a text field reports to its form besides its value: a native input's
/// attributes and the problems its own constraints find.
#[derive(Clone, Debug, Default)]
pub struct TextFormOptions {
    /// The name it submits under. Without one, it submits nothing.
    pub name: Option<SharedString>,
    /// Blocks submission while it's empty.
    pub required: bool,
    /// Disables it: it's neither submitted nor validated.
    pub disabled: bool,
    /// Makes it read-only, which keeps it out of validation.
    pub read_only: bool,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
    /// A problem its own constraints find, like text that doesn't match its
    /// pattern.
    pub invalid: Option<Problem>,
}

/// The component a text field belongs to, which its callbacks reach it
/// through.
pub trait TextFieldHost: 'static + Sized {
    /// The component's instance and its text field.
    fn text_field(&mut self) -> (&Instance, &mut TextFieldApi);

    /// Runs once a form reset has put the field's default back, or kept its
    /// value when the change was canceled: where a native input checks its
    /// constraints again. It runs inside the component's update, which a
    /// watcher of the field can't update. Nothing by default.
    fn text_field_reset(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
}

/// The value api of a text field, which a component keeps (`textFieldApi`).
/// The component hands it typing from its text control with
/// [`TextFieldApi::typed`], the commit with [`TextFieldApi::committed`],
/// and writes to the control from outside the field with
/// [`TextFieldApi::written`].
pub struct TextFieldApi {
    value: Controllable<SharedString>,
    default_value: Option<SharedString>,
    multiline: bool,
    control: Option<Rc<dyn TextControl>>,
    // Set while the field writes its own value, so typing it reports back
    // is skipped.
    writing: bool,
    // The text a controlled field wrote as the user, which the control shows
    // ahead of the value until the app passes it.
    ahead: Option<SharedString>,
    form: TextFormOptions,
    internals: ElementInternals,
    focus: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl TextFieldApi {
    /// A text field of the component of `cx`. A multi-line one, like a
    /// textarea's, doesn't take part in implicit submission.
    pub fn new<T: TextFieldHost>(options: TextFieldOptions, multiline: bool, cx: &mut Context<T>) -> Self {
        let initial = initial_text(&options);
        let mut value = Controllable::new("value", initial.value);
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());
        let internals = ElementInternals::new(cx);
        internals.set_blocks_implicit_submission(!multiline);
        let reset = internals.on_reset(cx, |host: &mut T, window, cx| {
            let (instance, field) = host.text_field();
            field.reset(instance, window, cx);
            host.text_field_reset(window, cx);
        });
        let key = internals.node().key();
        let entity = cx.weak_entity();
        let clear: ClearTarget = Rc::new(move |event, window, cx| {
            entity
                .update(cx, |host, cx| {
                    let (instance, field) = host.text_field();
                    field.clear_press(instance, event, window, cx);
                })
                .ok();
        });
        TEXT_FIELDS.with(|fields| fields.borrow_mut().push((key, clear)));
        let unregister = Subscription::new(move || {
            TEXT_FIELDS
                .try_with(|fields| fields.borrow_mut().retain(|(each, _)| *each != key))
                .ok();
        });
        let field = Self {
            value,
            default_value: options.default_value,
            multiline,
            control: None,
            writing: false,
            ahead: None,
            form: TextFormOptions::default(),
            internals,
            focus: None,
            _subscriptions: vec![reset, unregister],
        };
        field.sync_form();
        field
    }

    /// Hands it new options, as a framework re-renders with new props.
    /// Returns whether what renders changed. A new controlled value shows in
    /// the control once [`TextFieldApi::show`] runs.
    pub fn set_options(&mut self, options: TextFieldOptions) -> bool {
        let changed = self.value.set_controlled(options.value);
        self.value.set_on_change(options.on_value_change);
        self.value.set_on_before_change(options.on_before_value_change);
        self.default_value = options.default_value;
        if changed {
            self.ahead = None;
            self.sync_form();
        }
        changed
    }

    /// Hands it what it reports to its form besides its value.
    pub fn set_form_options(&mut self, options: TextFormOptions) {
        self.form = options;
        self.sync_form();
    }

    /// Its part in forms. Put them in the text control's element with
    /// `track`, and its name and states with `aria`.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    /// Its value, and whether it holds any.
    pub fn state(&self) -> TextState {
        let value = self.value.get().clone();
        TextState {
            filled: !value.is_empty(),
            value,
        }
    }

    /// The current value.
    pub fn get_value(&self) -> &SharedString {
        self.value.get()
    }

    pub fn is_controlled(&self) -> bool {
        self.value.is_controlled()
    }

    /// Sets the value as if the user typed it, so `Input` goes out. Returns
    /// false when the change was canceled or nothing changed.
    pub fn set_value<T: 'static>(
        &mut self,
        instance: &Instance,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.write(instance, value.into(), reason, options.event, window, cx)
    }

    /// Empties the field, as if the user deleted the text, so `Input` goes
    /// out. Returns false when the change was canceled or nothing changed.
    pub fn clear<T: 'static>(
        &mut self,
        instance: &Instance,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.write(instance, SharedString::default(), reason, options.event, window, cx)
    }

    /// What a Clear part's press does.
    pub fn clear_press<T: 'static>(
        &mut self,
        instance: &Instance,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        self.write(instance, SharedString::default(), Reason::ClearPress, event, window, cx)
    }

    /// Moves focus to the field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(control) = &self.control {
            control.focus(window, cx);
        }
    }

    // A change the user or a controller asked for. The control follows unless
    // it was canceled. A controlled field waits for its new `value`.
    fn write<T: 'static>(
        &mut self,
        instance: &Instance,
        value: SharedString,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let options = ChangeOptions {
            reason: Some(reason),
            event,
            source: None,
        };
        if !self.value.change(instance, value.clone(), options, window, cx) {
            return false;
        }
        self.sync_form();
        cx.notify();
        let Some(control) = self.control.clone() else {
            return true;
        };
        if control.value(cx) == value {
            return true;
        }
        if self.value.is_controlled() {
            self.ahead = Some(value.clone());
        }
        self.writing = true;
        write_value(control.as_ref(), Some(self.internals.node()), value, true, window, cx);
        self.writing = false;
        true
    }

    /// Drives `control` from now on. A field with no text takes the field's
    /// value; otherwise the field takes the control's, unless it's
    /// controlled.
    pub fn bind(&mut self, control: Rc<dyn TextControl>, window: &mut Window, cx: &mut App) {
        let node = self.internals.node().clone();
        if self.value.is_controlled() {
            write_value(control.as_ref(), Some(&node), self.value.get().clone(), false, window, cx);
        } else {
            if control.value(cx).is_empty() {
                write_value(control.as_ref(), Some(&node), self.value.get().clone(), false, window, cx);
            }
            self.value.set_held(control.value(cx));
        }
        let focused = control.clone();
        self.focus = Some(focus_with(&self.internals, move |window, cx| focused.focus(window, cx)));
        self.control = Some(control);
        self.sync_form();
    }

    /// Typing, or an input method finishing its text: the control's text is
    /// now `next`. A canceled change puts the text back, with the caret where
    /// it was, controlled or not. Mid-composition (`composing`) the input
    /// method owns the text, and it goes back once composition ends. Call it
    /// from the text control's event once the control's update is done, like
    /// a subscription to it, since putting the text back writes the control.
    pub fn typed<T: 'static>(
        &mut self,
        instance: &Instance,
        next: SharedString,
        event: Option<InputEvent>,
        composing: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if self.writing {
            return;
        }
        let previous = self.value.get().clone();
        if next == previous {
            return;
        }
        let node = self.internals.node().clone();
        notify_value(&node, ValueSource::Input, window, cx);
        dispatch_change(Some(&node), &[FormEventType::Input], window, cx);
        let options = ChangeOptions {
            reason: Some(Reason::InputChange),
            event,
            source: None,
        };
        if self.value.change(instance, next.clone(), options, window, cx) {
            self.sync_form();
            cx.notify();
            return;
        }
        if composing {
            return;
        }
        let Some(control) = self.control.clone() else {
            return;
        };
        let inserted = next.len() as isize - previous.len() as isize;
        let caret = control.caret(cx).unwrap_or(next.len()) as isize - inserted;
        let mut caret = caret.clamp(0, previous.len() as isize) as usize;
        while !previous.is_char_boundary(caret) {
            caret -= 1;
        }
        self.writing = true;
        control.set_value(previous, window, cx);
        control.set_caret(caret, window, cx);
        self.writing = false;
    }

    /// Code outside the field wrote the text control, like an app emptying a
    /// chat box after sending: an uncontrolled field takes the control's
    /// text as its value, which reports no change, and the field's watchers
    /// hear the write (`watchValue`'s `write`). Call it from the control's
    /// event for a write from code, like the native field's
    /// `TextFieldEvent::Write`, once the control's update is done. Writes
    /// the field made itself leave the control showing its value, and are
    /// skipped.
    pub fn written<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        if self.writing {
            return;
        }
        let Some(control) = self.control.clone() else {
            return;
        };
        let value = control.value(cx);
        if value == *self.value.get() || self.ahead.as_ref() == Some(&value) {
            return;
        }
        self.ahead = None;
        if !self.value.is_controlled() {
            self.value.set_held(value);
            self.sync_form();
            cx.notify();
        }
        notify_value(self.internals.node(), ValueSource::Write, window, cx);
    }

    /// The user is done changing the text, like when the field loses focus
    /// after an edit: `Change` goes out, as from a native input.
    pub fn committed(&self, window: &mut Window, cx: &mut App) {
        dispatch_change(Some(self.internals.node()), &[FormEventType::Change], window, cx);
    }

    /// What Enter does in the field: a single-line field submits its form
    /// implicitly, like a native input. A multi-line one does nothing.
    pub fn submit_implicitly(&self, window: &mut Window, cx: &mut App) {
        if !self.multiline {
            submit_implicitly(self.internals.form().as_ref(), window, cx);
        }
    }

    /// A form reset puts the default back, unless the change is canceled. A
    /// controlled field with no default keeps its value, as the browser
    /// keeps the default a framework moves with a controlled input.
    pub fn reset<T: 'static>(&mut self, instance: &Instance, window: &mut Window, cx: &mut Context<T>) {
        let current = self.value.get().clone();
        let target = self.default_value.clone().unwrap_or_else(|| {
            if self.value.is_controlled() {
                current.clone()
            } else {
                SharedString::default()
            }
        });
        if target == current {
            self.show_value(target, window, cx);
        } else if !self.value.change(instance, target.clone(), ChangeOptions::reason(Reason::Reset), window, cx) {
            self.show_value(current, window, cx);
        } else if !self.value.is_controlled() {
            self.show_value(target, window, cx);
            cx.notify();
        }
        self.sync_form();
        notify_value(self.internals.node(), ValueSource::Reset, window, cx);
    }

    /// Makes the control show the field's value, like after a new controlled
    /// value.
    pub fn show(&mut self, window: &mut Window, cx: &mut App) {
        let value = self.value.get().clone();
        self.show_value(value, window, cx);
    }

    fn show_value(&mut self, value: SharedString, window: &mut Window, cx: &mut App) {
        let Some(control) = self.control.clone() else {
            return;
        };
        self.ahead = None;
        if control.value(cx) != value {
            self.writing = true;
            write_value(control.as_ref(), Some(self.internals.node()), value, false, window, cx);
            self.writing = false;
        }
    }

    // Reports the value and constraints, as a native input does by itself.
    fn sync_form(&self) {
        let value = self.value.get().clone();
        bind_internals(
            &self.internals,
            InternalsOptions {
                value_missing: self.form.required && value.is_empty(),
                message: value_missing_message(ValueMissingKind::Text),
                value: FormValue::Text(value),
                name: self.form.name.clone(),
                invalid: self.form.invalid.clone(),
                anchor: None,
                disabled: self.form.disabled,
                read_only: self.form.read_only,
                required: self.form.required,
                // A read-only text field is barred from validation.
                barred: self.form.read_only,
                form: self.form.form.clone(),
            },
        );
    }
}

/// The field a press on its surroundings focuses: a text control, or the
/// node of a control that takes part in forms, which takes focus the way a
/// user would reach it.
pub trait FocusTarget {
    /// Moves focus to the field.
    fn focus_target(&self, window: &mut Window, cx: &mut App);
}

impl<T: TextControl + ?Sized> FocusTarget for T {
    fn focus_target(&self, window: &mut Window, cx: &mut App) {
        self.focus(window, cx);
    }
}

impl FocusTarget for Node {
    fn focus_target(&self, window: &mut Window, cx: &mut App) {
        focus_control(self, window, cx);
    }
}

/// A press on a field's surroundings, like its group's text and icons,
/// focuses the field the way a label would. Presses on anything interactive
/// inside `container`, like the field itself or a button, keep their own
/// behavior. `field` is its text control, or its node when that's how the
/// container reaches it.
pub fn focus_from_surroundings<F: FocusTarget + ?Sized>(
    event: &ClickEvent,
    container: &Node,
    field: Option<&F>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(field) = field else {
        return;
    };
    if !matches!(event, ClickEvent::Keyboard(_)) {
        let position = event.position();
        let interactive = container.descendants().iter().any(|node| {
            matches!(node.kind(), NodeKind::Control | NodeKind::Label)
                && node.bounds().is_some_and(|bounds| bounds.contains(&position))
        });
        if interactive {
            return;
        }
    }
    field.focus_target(window, cx);
}
