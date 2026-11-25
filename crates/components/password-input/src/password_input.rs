//! Port of `components/password-input/password-input.ts`: a native password
//! field with a button that shows and hides what's typed.
//!
//! The `control` part is the native crate's [`TextField`], which switches
//! between a password field and a text field, and the forms crate's
//! [`TextFieldApi`] is the original's `textFieldApi` over it. The password
//! hides again when its form submits or resets and when the window is
//! hidden.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, KeystrokeEvent, MouseButton,
    SharedString, Subscription, TextInputConfiguration, WeakEntity, Window, WindowVisibility,
};
use regex::Regex;
use ui_sdk_forms::{
    ElementInternals, FormEventType, Node, Problem, TextControl, TextFieldApi, TextFieldHost,
    TextFieldOptions, TextFormOptions, ValidityFlag, ValidityState, after_reset,
    focus_from_surroundings, listen_document,
};
use ui_sdk_kernel::dev::{self, Origin};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentSurface, Aria, ChangeCallback, ChangeDetails,
    ChangeOptions, Controllable, InputEvent, Instance, Key, Part, Reason, track_input,
    with_source,
};
use ui_sdk_native::{
    FieldNotice, SelectionDirection, TextField, TextFieldEvent, TextFieldKind,
    TextFieldOptions as FieldOptions, TextFieldStyle,
};
use ui_sdk_platform::{ButtonElement, ButtonElementOptions, button_element_attrs, button_role, is_apple};

/// Why a password input's password wants to be shown or hidden: its toggle
/// (`TogglePress`), submitting or resetting its form (`FormSubmit`,
/// `Reset`), the window being hidden (`PageHide`) or a call (`Imperative`).
pub type PasswordInputVisibleChangeReason = Reason;

/// The options `PasswordInput` takes: the value's, the visibility's, the
/// form's, and the native attributes the original puts on its control.
#[derive(Clone, Default)]
pub struct PasswordInputOptions {
    /// The password, for a controlled field.
    pub value: Option<SharedString>,
    /// The password it starts with. A form reset returns here.
    pub default_value: Option<SharedString>,
    /// Called when the user changes the password: typing (`InputChange`), a
    /// Clear part (`ClearPress`), a form reset (`Reset`) or a call
    /// (`Imperative`). It runs once the password input's update is over, so
    /// it may update any entity, the password input too.
    pub on_value_change: Option<ChangeCallback<SharedString>>,
    /// Runs as the password is asked to change, before it does, inside the
    /// password input's update: `details.cancel()` keeps it as it was. It may
    /// update any entity but the password input.
    pub on_before_value_change: Option<ChangeCallback<SharedString>>,
    /// Whether the password is shown. Pass it to control it.
    pub visible: Option<bool>,
    /// Whether the password is shown at first, when uncontrolled.
    pub default_visible: bool,
    /// Runs when the password wants to be shown or hidden: the toggle
    /// (`TogglePress`), submitting or resetting its form (`FormSubmit`,
    /// `Reset`), the window being hidden (`PageHide`) or a call
    /// (`Imperative`). `details.cancel()` keeps it as it was; the field is a
    /// password field while its form reads a submit either way. It runs
    /// inside the password input's update, so it may update any entity but
    /// it.
    pub on_visible_change: Option<ChangeCallback<bool>>,
    /// The name the password submits under. Inside a Field, the Field's name.
    pub name: Option<SharedString>,
    /// The id of the form the field belongs to, when it isn't inside it.
    pub form: Option<SharedString>,
    /// Whether a password is required to submit the form.
    pub required: bool,
    /// Disables the field and the toggle. Inside a Field or a disabled
    /// fieldset, they follow it.
    pub disabled: bool,
    /// Makes the field read-only. The toggle still shows and hides it.
    pub read_only: bool,
    /// The control's `id`, which a label's `for` names.
    pub id: Option<SharedString>,
    /// The control's placeholder.
    pub placeholder: Option<SharedString>,
    /// The most characters typing leaves, the control's `maxlength`.
    pub max_length: Option<usize>,
    /// The fewest characters a typed password may have, `minlength`.
    pub min_length: Option<usize>,
    /// A regular expression the whole password must match, `pattern`.
    pub pattern: Option<SharedString>,
    /// How many characters wide the control is when nothing sets its width.
    pub size: Option<usize>,
    /// The colors the control's text, caret, selection and placeholder
    /// paint with.
    pub field_style: TextFieldStyle,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl PasswordInputOptions {
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
        callback: impl Fn(&SharedString, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&SharedString, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = Some(visible);
        self
    }

    pub fn default_visible(mut self, visible: bool) -> Self {
        self.default_visible = visible;
        self
    }

    pub fn on_visible_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_visible_change = Some(Rc::new(callback));
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

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
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

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn max_length(mut self, max_length: usize) -> Self {
        self.max_length = Some(max_length);
        self
    }

    pub fn min_length(mut self, min_length: usize) -> Self {
        self.min_length = Some(min_length);
        self
    }

    pub fn pattern(mut self, pattern: impl Into<SharedString>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    pub fn size(mut self, size: usize) -> Self {
        self.size = Some(size);
        self
    }

    pub fn field_style(mut self, style: TextFieldStyle) -> Self {
        self.field_style = style;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    fn text(&self) -> TextFieldOptions {
        TextFieldOptions {
            value: self.value.clone(),
            default_value: self.default_value.clone(),
            on_value_change: self.on_value_change.clone(),
            on_before_value_change: self.on_before_value_change.clone(),
        }
    }

    fn settings(&self) -> Settings {
        Settings {
            disabled: self.disabled,
            read_only: self.read_only,
            required: self.required,
            name: self.name.clone(),
            form: self.form.clone(),
            id: self.id.clone(),
            placeholder: self.placeholder.clone(),
            max_length: self.max_length,
            min_length: self.min_length,
            pattern: self.pattern.clone(),
            size: self.size,
            field_style: self.field_style,
        }
    }
}

/// What every part's `with` and the app read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PasswordInputState {
    /// Whether the password is shown.
    pub visible: bool,
    /// Whether Caps Lock is on while the field has focus.
    pub caps_lock: bool,
    /// Whether the field holds any text.
    pub filled: bool,
    /// Whether the field is disabled, by itself, its Field or a fieldset.
    pub disabled: bool,
    /// Whether the field is read-only.
    pub read_only: bool,
    /// Whether a form won't submit while the field is empty.
    pub required: bool,
}

// The options that aren't the value's or the visibility's (`settings`).
#[derive(Clone, Default, PartialEq)]
struct Settings {
    disabled: bool,
    read_only: bool,
    required: bool,
    name: Option<SharedString>,
    form: Option<SharedString>,
    id: Option<SharedString>,
    placeholder: Option<SharedString>,
    max_length: Option<usize>,
    min_length: Option<usize>,
    pattern: Option<SharedString>,
    size: Option<usize>,
    field_style: TextFieldStyle,
}

// What the toggle renders as: a native button, as adapters render it.
const TOGGLE_ELEMENT: ButtonElement = ButtonElement::Button;

// The native field as the forms crate drives it.
struct FieldControl(WeakEntity<TextField>);

impl TextControl for FieldControl {
    fn value(&self, cx: &App) -> SharedString {
        self.0
            .upgrade()
            .map(|field| field.read(cx).value().clone())
            .unwrap_or_default()
    }

    fn set_value(&self, value: SharedString, window: &mut Window, cx: &mut App) {
        self.0.update(cx, |field, cx| field.set_value(value, window, cx)).ok();
    }

    fn caret(&self, cx: &App) -> Option<usize> {
        self.0.upgrade().map(|field| field.read(cx).selection_start())
    }

    fn set_caret(&self, caret: usize, window: &mut Window, cx: &mut App) {
        self.0
            .update(cx, |field, cx| {
                field.set_selection_range(caret, caret, SelectionDirection::None, window, cx)
            })
            .ok();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.0.update(cx, |field, cx| field.focus(window, cx)).ok();
    }
}

/// A password input. Its entity is its controller.
pub struct PasswordInput {
    instance: Instance,
    field: Entity<TextField>,
    text: TextFieldApi,
    text_options: TextFieldOptions,
    visible: Controllable<bool>,
    // The visibility the field last followed from the state (`shown`).
    shown: bool,
    // Whether the field shows the password now, as a text field. `hide`
    // makes it a password field ahead of the state.
    showing: bool,
    caps_lock: bool,
    settings: Settings,
    pattern: Option<Regex>,
    // Whether the value last changed through the user's edit. Only those
    // are checked against the lengths.
    user_edited: bool,
    // Where the caret was when the field last lost focus, for a toggle press
    // that brings focus back.
    selection: Option<(usize, usize, SelectionDirection)>,
    // The root's element, whose own text and icons focus the field.
    root_node: Node,
    toggle_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl TextFieldHost for PasswordInput {
    fn text_field(&mut self) -> (&Instance, &mut TextFieldApi) {
        (&self.instance, &mut self.text)
    }

    // A reset puts the default back, which nobody typed.
    fn text_field_reset(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.user_edited = false;
        self.sync_form();
    }
}

impl PasswordInput {
    pub fn new(options: PasswordInputOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        track_input(window, cx);
        let mut instance = Instance::new("password-input").with_agent(cx);
        instance.debug = options.debug;
        let text = TextFieldApi::new(options.text(), false, cx);
        let internals = text.internals().clone();
        internals.set_id(options.id.clone(), cx);
        let mut visible = Controllable::new("visible", options.visible.unwrap_or(options.default_visible));
        visible.set_controlled(options.visible);
        visible.set_on_change(options.on_visible_change.clone());
        let shown = *visible.get();
        let settings = options.settings();
        let field_options = field_options(&settings, shown, &internals);
        let field = cx.new(|cx| TextField::new(field_options, window, cx));
        internals.set_focus_handle(Some(field.read(cx).focus_handle(cx)));
        let this = cx.entity();
        let weak = cx.weak_entity();
        let subscriptions = vec![
            cx.subscribe_in(&field, window, Self::field_event),
            cx.observe_keystrokes(Self::keystroke),
            // Only a change of visibility, not every keystroke, which also
            // notifies, shows or hides the password. A fieldset or a Field
            // disabling the control notifies too.
            cx.observe_in(&this, window, |password: &mut Self, _, _window, cx| password.follow(cx)),
            // Captured, so the field is a password field before the page's
            // own submit listeners run, and before the form reads it.
            listen_document(FormEventType::Submit, true, {
                let weak = weak.clone();
                let internals = internals.clone();
                move |event, window, cx| {
                    if internals.form().is_some_and(|form| form.node() == event.target()) {
                        let input = event.input().cloned();
                        weak.update(cx, |password, cx| password.hide(Reason::FormSubmit, input, window, cx))
                            .ok();
                    }
                }
            }),
            listen_document(FormEventType::Reset, true, {
                let weak = weak.clone();
                let internals = internals.clone();
                move |event, _window, _cx| {
                    if internals.form().is_some_and(|form| form.node() == event.target()) {
                        let weak = weak.clone();
                        after_reset(event, move |window, cx| {
                            weak.update(cx, |password, cx| password.hide(Reason::Reset, None, window, cx))
                                .ok();
                        });
                    }
                }
            }),
            // A hidden window hides the password, as browsers keep a text
            // field's value when a page is hidden, but never a password's.
            cx.observe_window_visibility(window, |password: &mut Self, visibility, window, cx| {
                if visibility == WindowVisibility::Hidden {
                    password.hide(Reason::PageHide, None, window, cx);
                }
            }),
        ];
        let pattern = compile_pattern(settings.pattern.as_deref(), &instance);
        let mut password = Self {
            instance,
            field,
            text,
            text_options: options.text(),
            visible,
            shown,
            showing: shown,
            caps_lock: false,
            pattern,
            settings,
            user_edited: false,
            selection: None,
            root_node: Node::element(),
            toggle_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        let control = Rc::new(FieldControl(password.field.downgrade()));
        password.text.bind(control, window, cx);
        password.sync_form();
        password
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A new controlled value shows in the field at once.
    pub fn set_options(&mut self, options: PasswordInputOptions, window: &mut Window, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        let text = options.text();
        let mut changed = self.text.set_options(text.clone());
        self.text_options = text;
        if changed {
            self.text.show(window, cx);
        }
        changed |= self.visible.set_controlled(options.visible);
        self.visible.set_on_change(options.on_visible_change.clone());
        let settings = options.settings();
        if settings.pattern != self.settings.pattern {
            self.pattern = compile_pattern(settings.pattern.as_deref(), &self.instance);
        }
        if settings.id != self.settings.id {
            self.text.internals().set_id(settings.id.clone(), cx);
        }
        changed |= settings != self.settings;
        self.settings = settings;
        self.follow(cx);
        self.sync_form();
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled password, or `None` to let it keep its own.
    pub fn set_controlled_value(
        &mut self,
        value: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = TextFieldOptions {
            value,
            ..self.text_options.clone()
        };
        let changed = self.text.set_options(text.clone());
        self.text_options = text;
        if changed {
            self.text.show(window, cx);
            self.sync_form();
            cx.notify();
        }
    }

    /// Passes the controlled visibility, or `None` to let it keep its own.
    pub fn set_controlled_visible(&mut self, visible: Option<bool>, cx: &mut Context<Self>) {
        if self.visible.set_controlled(visible) {
            self.follow(cx);
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The native field it renders. A value code writes to it becomes an
    /// uncontrolled password input's own, with no change reported, where
    /// `set_value` reports one.
    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// Its value api and part in forms.
    pub fn text_field_api(&self) -> &TextFieldApi {
        &self.text
    }

    /// What every part's state holds.
    pub fn state(&self) -> PasswordInputState {
        PasswordInputState {
            visible: *self.visible.get(),
            caps_lock: self.caps_lock,
            filled: self.text.state().filled,
            disabled: self.is_disabled(),
            read_only: self.settings.read_only,
            required: self.settings.required,
        }
    }

    // Disabled by its own option, a fieldset around it, or a Field that
    // writes the control's own `disabled`.
    fn is_disabled(&self) -> bool {
        let internals = self.text.internals();
        self.settings.disabled || internals.form_disabled() || internals.control_attrs().disabled
    }

    /// The current password.
    pub fn get_value(&self) -> &SharedString {
        self.text.get_value()
    }

    /// Sets the password as if the user typed it, so `Input` goes out to its
    /// form. Returns false when the change was canceled or nothing changed.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let done = self.text.set_value(&self.instance, value, options, window, cx);
        self.sync_form();
        done
    }

    /// Empties the field, as if the user deleted the text, so `Input` goes
    /// out to its form. Returns false when the change was canceled or
    /// nothing changed.
    pub fn clear(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let done = self.text.clear(&self.instance, options, window, cx);
        self.sync_form();
        done
    }

    /// Whether the password is shown.
    pub fn is_visible(&self) -> bool {
        *self.visible.get()
    }

    /// Shows or hides the password. Returns false when canceled.
    pub fn set_visible(
        &mut self,
        visible: bool,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let changed = self.visible.change(&self.instance, visible, options, window, cx);
        if changed {
            cx.notify();
        }
        changed
    }

    /// Shows the password if it's hidden, and hides it if it's shown.
    /// Returns false when canceled.
    pub fn toggle_visible(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let visible = !*self.visible.get();
        self.set_visible(visible, options, window, cx)
    }

    /// Moves focus to the field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.text.focus(window, cx);
    }

    // Hides the password before the form is read or the window is hidden.
    // The field changes at once, before anything renders, and follows the
    // state again a task later, in case the change was canceled or is
    // controlled.
    fn hide(&mut self, reason: Reason, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if !*self.visible.get() {
            return;
        }
        self.show_text(false, cx);
        let options = ChangeOptions {
            reason: Some(reason),
            event,
            source: None,
        };
        self.set_visible(false, options, window, cx);
        cx.defer_in(window, |password, _window, cx| {
            let visible = *password.visible.get();
            password.show_text(visible, cx);
        });
    }

    // Switches the field between a text field and a password field. It keeps
    // its caret and selection through the switch.
    fn show_text(&mut self, visible: bool, cx: &mut App) {
        if self.showing != visible {
            self.showing = visible;
            self.sync_field(cx);
        }
    }

    // What the original's store subscription does: follows a change of
    // visibility, and what disables the control from outside.
    fn follow(&mut self, cx: &mut App) {
        let visible = *self.visible.get();
        if visible != self.shown {
            self.shown = visible;
            self.showing = visible;
        }
        self.sync_field(cx);
    }

    fn sync_field(&self, cx: &mut App) {
        let options = field_options(&self.settings, self.showing, self.text.internals());
        self.field.update(cx, |field, cx| field.set_options(options, cx));
    }

    // Reports the password's own constraints, as a native field does by
    // itself.
    fn sync_form(&mut self) {
        let value = self.text.get_value().clone();
        let invalid = constraint_problem(&value, &self.settings, self.pattern.as_ref(), self.user_edited);
        self.text.set_form_options(TextFormOptions {
            name: self.settings.name.clone(),
            required: self.settings.required,
            disabled: self.settings.disabled,
            read_only: self.settings.read_only,
            form: self.settings.form.clone(),
            invalid,
        });
    }

    fn set_caps_lock(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.caps_lock != on {
            self.caps_lock = on;
            cx.notify();
        }
    }

    // What the field tells once its update is over, which the original hears
    // with listeners bound on the control.
    fn field_event(
        &mut self,
        _field: &Entity<TextField>,
        event: &TextFieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextFieldEvent::Input(edit) => {
                self.user_edited = edit.user;
                let field = self.field.read(cx);
                let value = field.value().clone();
                let composing = field.is_composing();
                let (instance, text) = (&self.instance, &mut self.text);
                with_source(edit.source, || text.typed(instance, value, None, composing, window, cx));
                self.sync_form();
            }
            // Browsers differ on whether an `input` follows the end of a composition.
            TextFieldEvent::CompositionEnd(_) => {
                let value = self.field.read(cx).value().clone();
                self.text.typed(&self.instance, value, None, false, window, cx);
                self.sync_form();
            }
            TextFieldEvent::Change(_) => self.text.committed(window, cx),
            // A write from code isn't the user's edit, so the lengths let it
            // be. One from outside the component becomes the value.
            TextFieldEvent::Write(_) => {
                self.user_edited = false;
                self.text.written(window, cx);
                self.sync_form();
            }
            // Caps Lock shows only while the field has focus, and the caret
            // waits for a toggle press that brings focus back.
            TextFieldEvent::Blur => {
                let field = self.field.read(cx);
                self.selection = Some((field.selection_start(), field.selection_end(), field.selection_direction()));
                self.set_caps_lock(false, cx);
            }
            _ => {}
        }
    }

    // Enter in the field submits its form implicitly, the browser's own
    // action for a native input, after the field reports its `change`. It
    // never presses the toggle.
    fn keystroke(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.action.is_some() || window.default_prevented() || Key::of(&event.keystroke) != Key::Enter {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        let commits = if is_apple(cx) {
            !modifiers.control && !modifiers.platform
        } else {
            !modifiers.control && !modifiers.alt && !modifiers.platform
        };
        let field = self.field.read(cx);
        if !commits || !field.is_focused(window) || field.is_composing() || field.is_disabled() {
            return;
        }
        cx.defer_in(window, |password, window, cx| password.text.submit_implicitly(window, cx));
    }

    // A press from a pointer brings focus back to the field, with its caret,
    // so typing goes on. Keys and screen readers keep focus on the toggle,
    // which says whether it's pressed.
    fn toggle_press(&mut self, event: InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_disabled() {
            return;
        }
        let visible = !*self.visible.get();
        self.set_visible(visible, ChangeOptions::event_of(Reason::TogglePress, event.clone()), window, cx);
        let pointer = matches!(event, InputEvent::Click(ClickEvent::Mouse(_) | ClickEvent::Touch(_)));
        if !pointer || self.field.read(cx).is_focused(window) {
            return;
        }
        let selection = self.selection;
        self.field.update(cx, |field, cx| {
            field.focus(window, cx);
            if let Some((start, end, direction)) = selection {
                field.set_selection_range(start, end, direction, window, cx);
            }
        });
    }

    // Presses on the root's own text and icons focus the field, like a label
    // would.
    fn press_root(this: &Entity<Self>, event: &InputEvent, window: &mut Window, cx: &mut App) {
        let password = this.read(cx);
        let node = password.root_node.clone();
        let control = FieldControl(password.field.downgrade());
        match event {
            InputEvent::Click(click) => focus_from_surroundings(click, &node, Some(&control), window, cx),
            _ => control.focus(window, cx),
        }
    }

    /// The `root` part, which holds the control and the toggle: the field's
    /// box.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<PasswordInputState> {
        let password = this.read(cx);
        let presses = this.downgrade();
        Part::new(&password.instance, "root", None, password.state())
            .map_el(|el| password.root_node.track(el))
            .on_press(move |event, window, cx| {
                if let Some(password) = presses.upgrade() {
                    PasswordInput::press_root(&password, &event, window, cx);
                }
            })
    }

    /// The `control` part: the native field itself, a password field while
    /// the password is hidden and a text field while it's shown.
    pub fn control(this: &Entity<Self>, cx: &App) -> Part<PasswordInputState> {
        let password = this.read(cx);
        let field = password.field.read(cx);
        let disabled = field.is_disabled();
        let own = Aria::new()
            .disabled(disabled)
            .read_only(field.is_read_only())
            .required(field.is_required());
        let focus = field.focus_handle(cx);
        let internals = password.text.internals();
        let (el, _) = TextField::root(&password.field, cx).into_element_parts();
        // Mouse events carry modifier state too, so a click into the field
        // shows the warning before any key.
        let caps = |weak: WeakEntity<Self>| {
            move |window: &mut Window, cx: &mut App| {
                let on = window.capslock().on;
                weak.update(cx, |password, cx| password.set_caps_lock(on, cx)).ok();
            }
        };
        let (down, up, press, modifiers) = (
            caps(this.downgrade()),
            caps(this.downgrade()),
            caps(this.downgrade()),
            caps(this.downgrade()),
        );
        Part::new(&password.instance, "control", None, password.state())
            .map_el(move |_| internals.track(el))
            .aria_role(Role::Textbox)
            .focus_mode(FocusMode::Native, Some(&focus), disabled)
            .aria(own)
            .aria(internals.aria())
            .on_key_down(move |_, window, cx| down(window, cx))
            .on_key_up(move |_, window, cx| up(window, cx))
            .on_any_mouse_down(move |_, window, cx| press(window, cx))
            .on_modifiers_changed(move |_, window, cx| modifiers(window, cx))
    }

    /// The `toggle` part: a button that shows and hides the password. It's
    /// never a submit button, so Enter in the field submits the form rather
    /// than pressing it.
    pub fn toggle(this: &Entity<Self>, cx: &App) -> Part<PasswordInputState> {
        let password = this.read(cx);
        let state = password.state();
        let attrs = button_element_attrs(
            TOGGLE_ELEMENT,
            ButtonElementOptions {
                button_type: Some("button".into()),
                disabled: state.disabled,
                focusable: false,
                tab_index: None,
            },
        );
        let unfocusable = attrs.disabled || attrs.tab_index == Some(-1);
        let aria = Aria::new()
            .pressed(state.visible)
            .disabled(attrs.disabled || attrs.aria_disabled)
            .controls(password.instance.element_id("control", None));
        let presses = this.downgrade();
        Part::new(&password.instance, "toggle", None, state)
            .aria_role(button_role(TOGGLE_ELEMENT))
            .focus_mode(FocusMode::Native, Some(&password.toggle_focus), unfocusable)
            .aria(aria)
            // Focus stays in the field, so the caret, the selection and the
            // on-screen keyboard stay where they are.
            .on_mouse_down(MouseButton::Left, |_, window, _cx| window.prevent_default())
            .on_press(move |event, window, cx| {
                // The press is the toggle's, not the root's around it.
                cx.stop_propagation();
                presses
                    .update(cx, |password, cx| password.toggle_press(event, window, cx))
                    .ok();
            })
    }
}

impl Focusable for PasswordInput {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.read(cx).focus_handle(cx)
    }
}

/// `agent: { actions: ["setValue", "clear", "setVisible",
/// "toggleVisible"], secret: true }` and `controlled: ["value", "visible"]`.
impl Agent for PasswordInput {
    const SURFACE: AgentSurface = AgentSurface::new().secret(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |password, args, window, cx| {
            let value = args.string(0)?;
            Ok(password.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |password, _args, window, cx| {
            Ok(password.clear(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setVisible", |password, args, window, cx| {
            let visible = args.boolean(0)?;
            Ok(password.set_visible(visible, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("toggleVisible", |password, _args, window, cx| {
            Ok(password.toggle_visible(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["value", "visible"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", self.text.get_value().to_string())
            .with("visible", self.is_visible())
            .with("disabled", self.is_disabled())
            .with("readOnly", self.settings.read_only)
            .with("required", self.settings.required)
    }
}

// The control's options. A shown password is plain text: it's never sent
// to the platform's spelling services, and the keyboard never corrects or
// capitalizes it. Its `on_submit` submits the form the forms crate found.
fn field_options(settings: &Settings, showing: bool, internals: &ElementInternals) -> FieldOptions {
    let disabled = settings.disabled || internals.form_disabled() || internals.control_attrs().disabled;
    FieldOptions {
        placeholder: settings.placeholder.clone(),
        cols: settings.size,
        kind: if showing {
            TextFieldKind::Text
        } else {
            TextFieldKind::Password
        },
        disabled,
        read_only: settings.read_only,
        required: settings.required,
        max_length: settings.max_length,
        configuration: TextInputConfiguration::default(),
        style: settings.field_style,
        on_submit: Some(submit_hook(internals.clone())),
        ..FieldOptions::default()
    }
}

// `form.requestSubmit()` from the field, once its update is over.
fn submit_hook(internals: ElementInternals) -> FieldNotice {
    Rc::new(move |_field, window, cx| {
        let internals = internals.clone();
        window.defer(cx, move |window, cx| {
            if let Some(form) = internals.form() {
                form.request_submit(None, None, window, cx);
            }
        });
    })
}

// What the browser checks of a password field's value by itself, which the
// native field leaves to its component, with Chromium's English messages:
// the pattern, then the lengths of a typed password.

// The control's `pattern`, which the whole password must match. One that
// doesn't compile is ignored, as browsers do.
fn compile_pattern(pattern: Option<&str>, instance: &Instance) -> Option<Regex> {
    let pattern = pattern?;
    match Regex::new(&format!("^(?:{pattern})$")) {
        Ok(regex) => Some(regex),
        Err(_) => {
            dev::warn(
                &format!("pattern \"{pattern}\" isn't a valid regular expression, so it's ignored."),
                Some(Origin::new(instance.label(), instance.id())),
            );
            None
        }
    }
}

fn constraint_problem(value: &str, settings: &Settings, pattern: Option<&Regex>, user_edited: bool) -> Option<Problem> {
    if value.is_empty() {
        return None;
    }
    let mut flags = ValidityState::default();
    let mut message: Option<String> = None;
    let mut fail = |flag: ValidityFlag, text: String| {
        flags.set(flag, true);
        message.get_or_insert(text);
    };
    if let Some(pattern) = pattern
        && !pattern.is_match(value)
    {
        fail(ValidityFlag::PatternMismatch, "Please match the requested format.".into());
    }
    let length = value.chars().count();
    if user_edited {
        if let Some(max) = settings.max_length
            && length > max
        {
            fail(
                ValidityFlag::TooLong,
                format!(
                    "Please shorten this text to {max} {} or less (you are currently using {length} {}).",
                    characters(max),
                    characters(length)
                ),
            );
        }
        if let Some(min) = settings.min_length
            && length < min
        {
            fail(
                ValidityFlag::TooShort,
                format!(
                    "Please lengthen this text to {min} {} or more (you are currently using {length} {}).",
                    characters(min),
                    characters(length)
                ),
            );
        }
    }
    let message = message?;
    Some(Problem {
        flags,
        message: message.into(),
        anchor: None,
    })
}

fn characters(count: usize) -> &'static str {
    if count == 1 { "character" } else { "characters" }
}
