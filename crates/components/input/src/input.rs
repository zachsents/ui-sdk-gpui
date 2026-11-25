//! Port of `components/input/input.ts`: Input, a single-line native text
//! field; Textarea, a multi-line one; and InputGroup, text or buttons around
//! an input, with a button that clears it.
//!
//! Input and Textarea each wrap the native crate's [`TextField`], the way the
//! original renders an `<input>` or a `<textarea>`: the field's element is
//! the `root` part, with the component's slot, state and ARIA on it, and the
//! forms crate's [`TextFieldApi`] is the original's `textFieldApi` over it.
//! What a browser does for a native input by itself, the components do here:
//! Enter submits the form implicitly, a disabled fieldset disables the field,
//! and the value's own constraints (`pattern`, the lengths, the type, a
//! number's range and step) reach the form as a [`Problem`].

use std::rc::Rc;
use std::sync::LazyLock;

use gpui::prelude::*;
use gpui::{
    App, Autocapitalize, Context, Entity, FocusHandle, Focusable, KeyDownEvent, KeystrokeEvent,
    SharedString, Subscription, TextInputAction, TextInputConfiguration, WeakEntity, Window,
};
use regex::Regex;
use ui_sdk_forms::{
    AutoCapitalize, ElementInternals, EnterKeyHint, FormValue, InputHints, Node, Problem,
    TextControl, TextFieldApi, TextFieldHost, TextFieldOptions, TextFormOptions, ValidityFlag,
    ValidityState, WeakNode, focus_control, focus_from_surroundings, text_fields,
};
use ui_sdk_kernel::dev::{self, Origin};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentValue, Aria, ChangeCallback, ChangeDetails,
    ChangeOptions, InputEvent, Instance, Key, Part, is_composing, track_input, with_source,
};
use ui_sdk_native::{
    FieldNotice, SelectionDirection, TextField, TextFieldEvent, TextFieldKind,
    TextFieldOptions as FieldOptions, TextFieldStyle,
};
use ui_sdk_platform::is_apple;

/// A number field's `step`: how far its arrows step it, which also makes the
/// valid values those a whole number of steps from `min`, or `Any`, which
/// takes any value and steps by 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputStep {
    Value(f64),
    Any,
}

impl From<f64> for InputStep {
    fn from(step: f64) -> Self {
        InputStep::Value(step)
    }
}

/// The options `Input` takes: the value's, the root's props (`disabled`,
/// `read_only`, `required`), and the native `<input>`'s attributes, which the
/// original passes through to it.
#[derive(Clone, Default)]
pub struct InputOptions {
    /// The value, for a controlled field.
    pub value: Option<SharedString>,
    /// The value it starts with. A form reset returns here.
    pub default_value: Option<SharedString>,
    /// Called when the user changes the value: typing (`InputChange`), a
    /// Clear part (`ClearPress`), a form reset (`Reset`) or a call
    /// (`Imperative`). It runs once the input's update is over, so it may
    /// update any entity, the input too.
    pub on_value_change: Option<ChangeCallback<SharedString>>,
    /// Runs as the value is asked to change, before it does, inside the
    /// input's update: `details.cancel()` keeps the value as it was, with the
    /// text and the caret back where they were. It may update any entity but
    /// the input.
    pub on_before_value_change: Option<ChangeCallback<SharedString>>,
    /// Disables the field.
    pub disabled: bool,
    /// Makes the field read-only.
    pub read_only: bool,
    /// Makes the field required.
    pub required: bool,
    /// What it holds, `type`: text, search, email, tel, url, number or a
    /// password.
    pub kind: TextFieldKind,
    /// Its `id`, which a label's `for` names.
    pub id: Option<SharedString>,
    /// The name it submits under. Inside a Field, the Field's name.
    pub name: Option<SharedString>,
    /// The id of the form it belongs to, when it isn't inside it.
    pub form: Option<SharedString>,
    pub placeholder: Option<SharedString>,
    /// The most characters typing leaves, `maxlength`.
    pub max_length: Option<usize>,
    /// The fewest characters typed text may have, `minlength`.
    pub min_length: Option<usize>,
    /// A regular expression the whole value must match, `pattern`.
    pub pattern: Option<SharedString>,
    /// A number field's lowest value, `min`.
    pub min: Option<f64>,
    /// A number field's highest value, `max`.
    pub max: Option<f64>,
    /// A number field's `step`, 1 by default.
    pub step: Option<InputStep>,
    /// How many characters wide it is when nothing sets its width, `size`.
    pub size: Option<usize>,
    /// What a phone's keyboard and the platform's text services offer, like
    /// `input_hints(InputHintKind::Email, ..)`.
    pub input_hints: InputHints,
    /// The colors its text, caret, selection and placeholder paint with.
    pub field_style: TextFieldStyle,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl InputOptions {
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

    pub fn kind(mut self, kind: TextFieldKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
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

    pub fn min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    pub fn step(mut self, step: impl Into<InputStep>) -> Self {
        self.step = Some(step.into());
        self
    }

    pub fn size(mut self, size: usize) -> Self {
        self.size = Some(size);
        self
    }

    pub fn input_hints(mut self, hints: InputHints) -> Self {
        self.input_hints = hints;
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

    fn native(&self) -> NativeAttributes {
        NativeAttributes {
            disabled: self.disabled,
            read_only: self.read_only,
            required: self.required,
            kind: self.kind,
            id: self.id.clone(),
            name: self.name.clone(),
            form: self.form.clone(),
            placeholder: self.placeholder.clone(),
            max_length: self.max_length,
            min_length: self.min_length,
            pattern: self.pattern.clone(),
            min: self.min,
            max: self.max,
            step: self.step,
            rows: None,
            cols: self.size,
            auto_resize: false,
            input_hints: self.input_hints.clone(),
            field_style: self.field_style,
        }
    }
}

/// What `on_submit` of `Textarea` receives besides the value.
#[derive(Clone, Debug, Default)]
pub struct TextareaSubmitDetails {
    /// The key press that submitted.
    pub event: Option<InputEvent>,
}

/// `on_submit` of `Textarea`: the value, and the key press that submitted.
pub type TextareaSubmitCallback = Rc<dyn Fn(&SharedString, &TextareaSubmitDetails, &mut Window, &mut App)>;

/// Which keys submit a textarea (`submitShortcut`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextareaSubmitShortcut {
    /// `"enter"`: Enter submits, and Shift+Enter starts a new line.
    Enter,
    /// `"mod+enter"`: Command+Enter on a Mac, Control+Enter elsewhere, and
    /// Enter starts a new line.
    ModEnter,
}

/// The options `Textarea` takes: an input's, without the attributes a
/// `<textarea>` has none of, and its own.
#[derive(Clone, Default)]
pub struct TextareaOptions {
    /// The value, for a controlled field.
    pub value: Option<SharedString>,
    /// The value it starts with. A form reset returns here.
    pub default_value: Option<SharedString>,
    /// Called when the user changes the value: typing (`InputChange`), a
    /// Clear part (`ClearPress`), a form reset (`Reset`) or a call
    /// (`Imperative`). It runs once the textarea's update is over, so it may
    /// update any entity, the textarea too.
    pub on_value_change: Option<ChangeCallback<SharedString>>,
    /// Runs as the value is asked to change, before it does, inside the
    /// textarea's update: `details.cancel()` keeps the value as it was. It
    /// may update any entity but the textarea.
    pub on_before_value_change: Option<ChangeCallback<SharedString>>,
    /// Disables the field.
    pub disabled: bool,
    /// Makes the field read-only.
    pub read_only: bool,
    /// Makes the field required.
    pub required: bool,
    /// Its `id`, which a label's `for` names.
    pub id: Option<SharedString>,
    /// The name it submits under. Inside a Field, the Field's name.
    pub name: Option<SharedString>,
    /// The id of the form it belongs to, when it isn't inside it.
    pub form: Option<SharedString>,
    pub placeholder: Option<SharedString>,
    /// The most characters typing leaves, `maxlength`.
    pub max_length: Option<usize>,
    /// The fewest characters typed text may have, `minlength`.
    pub min_length: Option<usize>,
    /// How many lines tall it is when nothing sets its height, `rows`.
    pub rows: Option<usize>,
    /// How many characters wide it is when nothing sets its width, `cols`.
    pub cols: Option<usize>,
    /// What a phone's keyboard and the platform's text services offer.
    pub input_hints: InputHints,
    /// The colors its text, caret, selection and placeholder paint with.
    pub field_style: TextFieldStyle,
    /// Grows and shrinks with the text. Bound it with `min_h` and `max_h` on
    /// the root; past the maximum, it scrolls.
    pub auto_resize: bool,
    /// Runs on the submit shortcut, unless an input method is composing.
    /// Without it, the shortcut submits the form the textarea is in. With
    /// neither, the key is left alone, so Enter adds a line. It runs outside
    /// the textarea's update, so it may update the textarea, like clearing it
    /// once it's sent.
    pub on_submit: Option<TextareaSubmitCallback>,
    /// Which keys submit: `Enter`, with Shift+Enter for a new line, or
    /// `ModEnter`, with Enter for a new line. Without it, Enter always starts
    /// a new line.
    pub submit_shortcut: Option<TextareaSubmitShortcut>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl TextareaOptions {
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

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
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

    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = Some(rows);
        self
    }

    pub fn cols(mut self, cols: usize) -> Self {
        self.cols = Some(cols);
        self
    }

    pub fn input_hints(mut self, hints: InputHints) -> Self {
        self.input_hints = hints;
        self
    }

    pub fn field_style(mut self, style: TextFieldStyle) -> Self {
        self.field_style = style;
        self
    }

    pub fn auto_resize(mut self, auto_resize: bool) -> Self {
        self.auto_resize = auto_resize;
        self
    }

    pub fn on_submit(
        mut self,
        callback: impl Fn(&SharedString, &TextareaSubmitDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_submit = Some(Rc::new(callback));
        self
    }

    pub fn submit_shortcut(mut self, shortcut: TextareaSubmitShortcut) -> Self {
        self.submit_shortcut = Some(shortcut);
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

    fn native(&self) -> NativeAttributes {
        NativeAttributes {
            disabled: self.disabled,
            read_only: self.read_only,
            required: self.required,
            kind: TextFieldKind::Text,
            id: self.id.clone(),
            name: self.name.clone(),
            form: self.form.clone(),
            placeholder: self.placeholder.clone(),
            max_length: self.max_length,
            min_length: self.min_length,
            pattern: None,
            min: None,
            max: None,
            step: None,
            rows: self.rows,
            cols: self.cols,
            auto_resize: self.auto_resize,
            input_hints: self.input_hints.clone(),
            field_style: self.field_style,
        }
    }
}

/// What the root part's `with` and the app read, for an Input and a
/// Textarea.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputState {
    /// Whether the field holds any text.
    pub filled: bool,
    /// Whether the field is disabled.
    pub disabled: bool,
    /// Whether the field is read-only.
    pub read_only: bool,
    /// Whether the field is required.
    pub required: bool,
}

/// The state the `root` part of a textarea hands `with`.
pub type TextareaState = InputState;

// The native attributes an Input or a Textarea puts on its field.
#[derive(Clone, Default, PartialEq)]
struct NativeAttributes {
    disabled: bool,
    read_only: bool,
    required: bool,
    kind: TextFieldKind,
    id: Option<SharedString>,
    name: Option<SharedString>,
    form: Option<SharedString>,
    placeholder: Option<SharedString>,
    max_length: Option<usize>,
    min_length: Option<usize>,
    pattern: Option<SharedString>,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<InputStep>,
    rows: Option<usize>,
    cols: Option<usize>,
    auto_resize: bool,
    input_hints: InputHints,
    field_style: TextFieldStyle,
}

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

// What Input and Textarea share: the native field, its value api, and what
// the browser checks of its value.
struct TextBox {
    instance: Instance,
    field: Entity<TextField>,
    text: TextFieldApi,
    options: TextFieldOptions,
    native: NativeAttributes,
    multi_line: bool,
    // The native `pattern`, compiled, while it compiles.
    pattern: Option<Regex>,
    // Whether the value last changed through the user's edit. Only those
    // are checked against the lengths (`lastChangeWasUserEdit`).
    user_edited: bool,
}

impl TextBox {
    fn new<T: TextFieldHost + Agent>(
        name: &'static str,
        options: TextFieldOptions,
        native: NativeAttributes,
        multi_line: bool,
        debug: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Self {
        // A change's source follows how the window was used (`inputSource`).
        track_input(window, cx);
        let mut instance = Instance::new(name).with_agent(cx);
        instance.debug = debug;
        let text = TextFieldApi::new(options.clone(), multi_line, cx);
        let internals = text.internals().clone();
        internals.set_id(native.id.clone(), cx);
        let field_options = field_options(&native, multi_line, &internals);
        let field = cx.new(|cx| TextField::new(field_options, window, cx));
        internals.set_focus_handle(Some(field.read(cx).focus_handle(cx)));
        let pattern = compile_pattern(native.pattern.as_deref(), &instance);
        let mut text_box = Self {
            instance,
            text,
            options,
            pattern,
            native,
            multi_line,
            user_edited: false,
            field,
        };
        let control = Rc::new(FieldControl(text_box.field.downgrade()));
        text_box.text.bind(control, window, cx);
        text_box.sync_form();
        text_box
    }

    fn set_options<T: 'static>(
        &mut self,
        options: TextFieldOptions,
        native: NativeAttributes,
        debug: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        self.instance.debug = debug;
        let mut changed = self.text.set_options(options.clone());
        self.options = options;
        if changed {
            self.text.show(window, cx);
        }
        if native.pattern != self.native.pattern {
            self.pattern = compile_pattern(native.pattern.as_deref(), &self.instance);
        }
        if native.id != self.native.id {
            self.text.internals().set_id(native.id.clone(), cx);
        }
        changed |= native != self.native;
        self.native = native;
        self.sync_field(cx);
        self.sync_form();
        changed
    }

    fn set_controlled_value<T: 'static>(
        &mut self,
        value: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let options = TextFieldOptions {
            value,
            ..self.options.clone()
        };
        let changed = self.text.set_options(options.clone());
        self.options = options;
        if changed {
            self.text.show(window, cx);
            self.sync_form();
        }
        changed
    }

    fn state(&self) -> InputState {
        InputState {
            filled: self.text.state().filled,
            disabled: self.native.disabled,
            read_only: self.native.read_only,
            required: self.native.required,
        }
    }

    // Disabled the way a person meets it: by itself, by its Field, or by a
    // fieldset around it, which disables a native field by itself.
    fn is_disabled(&self) -> bool {
        native_disabled(self.native.disabled, self.text.internals())
    }

    fn set_value<T: 'static>(
        &mut self,
        value: SharedString,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let done = self.text.set_value(&self.instance, value, options, window, cx);
        self.sync_form();
        done
    }

    fn clear<T: 'static>(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<T>) -> bool {
        let done = self.text.clear(&self.instance, options, window, cx);
        self.sync_form();
        done
    }

    // Hands the field its attributes, and what disables it from outside.
    fn sync_field(&self, cx: &mut App) {
        let options = field_options(&self.native, self.multi_line, self.text.internals());
        self.field.update(cx, |field, cx| field.set_options(options, cx));
    }

    // Reports the value's own constraints, as a native field does by itself.
    fn sync_form(&mut self) {
        let value = self.text.get_value().clone();
        let invalid = constraint_problem(&value, &self.native, self.pattern.as_ref(), self.user_edited);
        self.text.set_form_options(TextFormOptions {
            name: self.native.name.clone(),
            required: self.native.required,
            disabled: self.native.disabled,
            read_only: self.native.read_only,
            form: self.native.form.clone(),
            invalid,
        });
    }

    // What the field tells once its update is over, which the original hears
    // with listeners bound on the element.
    fn field_event<T: 'static>(&mut self, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<T>) {
        match event {
            TextFieldEvent::Input(edit) => {
                self.user_edited = edit.user;
                let field = self.field.read(cx);
                let value = field.value().clone();
                let composing = field.is_composing();
                let (instance, text) = (&self.instance, &mut self.text);
                // Typing reports how it was typed, though no event explains it.
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
            _ => {}
        }
    }

    // The field's element as the component's part: its slot, state and
    // ARIA, with the forms crate's node in it.
    fn part<S: 'static>(&self, state: S, cx: &App) -> Part<S> {
        let field = self.field.read(cx);
        let role = if field.kind() == TextFieldKind::Search && !field.is_multi_line() {
            Role::Searchbox
        } else {
            Role::Textbox
        };
        let disabled = field.is_disabled();
        let own = Aria::new()
            .disabled(disabled)
            .read_only(field.is_read_only())
            .required(field.is_required());
        let focus = field.focus_handle(cx);
        let internals = self.text.internals();
        // The `type` and `autocomplete` the original's `<input>` carries,
        // which tell the agent bridge a password or a one-time code. A
        // textarea has no type.
        let input_type = (!self.multi_line).then(|| self.native.kind.as_str());
        let autocomplete = self.native.input_hints.auto_complete;
        let (el, _) = TextField::root(&self.field, cx).into_element_parts();
        Part::new(&self.instance, "root", None, state)
            .map_el(move |_| internals.track(el))
            .input_attrs(input_type, autocomplete)
            .aria_role(role)
            .focus_mode(FocusMode::Native, Some(&focus), disabled)
            .aria(own)
            .aria(internals.aria())
    }
}

// Disabled by its own option, its Field or a fieldset around it.
fn native_disabled(disabled: bool, internals: &ElementInternals) -> bool {
    disabled || internals.form_disabled() || internals.control_attrs().disabled
}

// The field's options from the native attributes. Its `on_submit`, which
// asks to submit the form it's in, submits the form the forms crate found.
fn field_options(native: &NativeAttributes, multi_line: bool, internals: &ElementInternals) -> FieldOptions {
    FieldOptions {
        placeholder: native.placeholder.clone(),
        multi_line,
        rows: native.rows,
        cols: native.cols,
        auto_resize: native.auto_resize,
        kind: native.kind,
        disabled: native_disabled(native.disabled, internals),
        read_only: native.read_only,
        required: native.required,
        max_length: native.max_length,
        min: native.min,
        max: native.max,
        step: match native.step {
            Some(InputStep::Value(step)) => Some(step),
            _ => None,
        },
        configuration: configuration_of(&native.input_hints),
        style: native.field_style,
        on_submit: Some(submit_hook(internals.clone())),
        ..FieldOptions::default()
    }
}

// `form.requestSubmit()` from the field, once its update is over, since a
// submit reads and focuses fields.
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

// What the hints ask of the platform's text input. Hints left unset leave
// the field's own, which offer nothing.
fn configuration_of(hints: &InputHints) -> TextInputConfiguration {
    TextInputConfiguration {
        autocorrect: hints.auto_correct.unwrap_or(false),
        autocapitalize: match hints.auto_capitalize {
            Some(AutoCapitalize::Sentences) => Autocapitalize::Sentences,
            Some(AutoCapitalize::Words) => Autocapitalize::Words,
            Some(AutoCapitalize::Characters) => Autocapitalize::Characters,
            Some(AutoCapitalize::None) | None => Autocapitalize::None,
        },
        suggestions: hints.spell_check.unwrap_or(false),
        input_action: match hints.enter_key_hint {
            Some(EnterKeyHint::Enter) => TextInputAction::Enter,
            Some(EnterKeyHint::Done) => TextInputAction::Done,
            Some(EnterKeyHint::Go) => TextInputAction::Go,
            Some(EnterKeyHint::Next) => TextInputAction::Next,
            Some(EnterKeyHint::Previous) => TextInputAction::Previous,
            Some(EnterKeyHint::Search) => TextInputAction::Search,
            Some(EnterKeyHint::Send) => TextInputAction::Send,
            None => TextInputAction::Unspecified,
        },
    }
}

// Whether a key commits a single-line field, as the native field reads it:
// Enter, without Control or Command on a Mac, and without Control, Alt or
// the Windows key elsewhere.
fn commits(event: &KeystrokeEvent, cx: &App) -> bool {
    if Key::of(&event.keystroke) != Key::Enter {
        return false;
    }
    let modifiers = event.keystroke.modifiers;
    if is_apple(cx) {
        !modifiers.control && !modifiers.platform
    } else {
        !modifiers.control && !modifiers.alt && !modifiers.platform
    }
}

/// A single-line native text field. Its entity is its controller.
pub struct Input {
    core: TextBox,
    _subscriptions: Vec<Subscription>,
}

impl TextFieldHost for Input {
    fn text_field(&mut self) -> (&Instance, &mut TextFieldApi) {
        (&self.core.instance, &mut self.core.text)
    }

    // A reset puts the default back, which nobody typed.
    fn text_field_reset(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.core.user_edited = false;
        self.core.sync_form();
    }
}

impl Input {
    pub fn new(options: InputOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let core = TextBox::new("input", options.text(), options.native(), false, options.debug, window, cx);
        let this = cx.entity();
        let subscriptions = vec![
            cx.subscribe_in(&core.field, window, |input: &mut Self, _, event, window, cx| {
                input.core.field_event(event, window, cx)
            }),
            cx.observe_keystrokes(Self::keystroke),
            // Its Field, or a fieldset around it, notifies it when they
            // disable it, which the native field follows.
            cx.observe_in(&this, window, |input: &mut Self, _, _window, cx| input.core.sync_field(cx)),
        ];
        Self {
            core,
            _subscriptions: subscriptions,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A new controlled value shows in the field at once.
    pub fn set_options(&mut self, options: InputOptions, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self
            .core
            .set_options(options.text(), options.native(), options.debug, window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the input keep its own.
    pub fn set_controlled_value(
        &mut self,
        value: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.core.set_controlled_value(value, window, cx) {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.core.instance
    }

    /// The native field it renders, like to read where its caret is. A value
    /// code writes to it becomes an uncontrolled input's own, with no change
    /// reported, where `set_value` reports one.
    pub fn field(&self) -> &Entity<TextField> {
        &self.core.field
    }

    /// Its value api and part in forms.
    pub fn text_field_api(&self) -> &TextFieldApi {
        &self.core.text
    }

    /// What the root part's state holds.
    pub fn state(&self) -> InputState {
        self.core.state()
    }

    /// The current value.
    pub fn get_value(&self) -> &SharedString {
        self.core.text.get_value()
    }

    /// Sets the value as if the user typed it, so `Input` goes out to its
    /// form. Returns false when the change was canceled or nothing changed.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.core.set_value(value.into(), options, window, cx)
    }

    /// Empties the field, as if the user deleted the text, so `Input` goes
    /// out to its form. Returns false when the change was canceled or
    /// nothing changed.
    pub fn clear(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.core.clear(options, window, cx)
    }

    /// Moves focus to the field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.core.text.focus(window, cx);
    }

    // Enter in the field submits its form implicitly, the browser's own
    // action for a native input: once every listener had the key and none
    // prevented it, and after the field reports its `change`.
    fn keystroke(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.action.is_some() || window.default_prevented() || !commits(event, cx) {
            return;
        }
        let field = self.core.field.read(cx);
        if !field.is_focused(window) || field.is_composing() || field.is_disabled() {
            return;
        }
        cx.defer_in(window, |input, window, cx| input.core.text.submit_implicitly(window, cx));
    }

    /// The `root` part: the native field itself, which the app styles as the
    /// input. The text inside draws with the part's text style and
    /// `field_style`.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<InputState> {
        let input = this.read(cx);
        input.core.part(input.state(), cx)
    }
}

impl Focusable for Input {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.core.field.read(cx).focus_handle(cx)
    }
}

/// `agent: { actions: ["setValue", "clear"] }`, with `value` controlled.
impl Agent for Input {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |input, args, window, cx| {
            let value = args.string(0)?;
            Ok(input.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |input, _args, window, cx| {
            Ok(input.clear(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        agent_state(&self.core)
    }
}

fn agent_state(core: &TextBox) -> AgentState {
    AgentState::new()
        .with("value", core.text.get_value().to_string())
        .with("disabled", core.is_disabled())
        .with("readOnly", core.native.read_only)
        .with("required", core.native.required)
}

/// A multi-line native text field. Its entity is its controller.
pub struct Textarea {
    core: TextBox,
    on_submit: Option<TextareaSubmitCallback>,
    submit_shortcut: Option<TextareaSubmitShortcut>,
    _subscriptions: Vec<Subscription>,
}

impl TextFieldHost for Textarea {
    fn text_field(&mut self) -> (&Instance, &mut TextFieldApi) {
        (&self.core.instance, &mut self.core.text)
    }

    // A reset puts the default back, which nobody typed.
    fn text_field_reset(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.core.user_edited = false;
        self.core.sync_form();
    }
}

impl Textarea {
    pub fn new(options: TextareaOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let core = TextBox::new("textarea", options.text(), options.native(), true, options.debug, window, cx);
        let this = cx.entity();
        let subscriptions = vec![
            cx.subscribe_in(&core.field, window, |textarea: &mut Self, _, event, window, cx| {
                textarea.core.field_event(event, window, cx)
            }),
            cx.observe_in(&this, window, |textarea: &mut Self, _, _window, cx| textarea.core.sync_field(cx)),
        ];
        Self {
            core,
            on_submit: options.on_submit,
            submit_shortcut: options.submit_shortcut,
            _subscriptions: subscriptions,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). `auto_resize` reaches the field at once.
    pub fn set_options(&mut self, options: TextareaOptions, window: &mut Window, cx: &mut Context<Self>) {
        self.on_submit = options.on_submit.clone();
        let mut changed = self.submit_shortcut != options.submit_shortcut;
        self.submit_shortcut = options.submit_shortcut;
        changed |= self
            .core
            .set_options(options.text(), options.native(), options.debug, window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the textarea keep its
    /// own.
    pub fn set_controlled_value(
        &mut self,
        value: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.core.set_controlled_value(value, window, cx) {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.core.instance
    }

    /// The native field it renders. A value code writes to it becomes an
    /// uncontrolled textarea's own, with no change reported, where
    /// `set_value` reports one.
    pub fn field(&self) -> &Entity<TextField> {
        &self.core.field
    }

    /// Its value api and part in forms.
    pub fn text_field_api(&self) -> &TextFieldApi {
        &self.core.text
    }

    /// What the root part's state holds.
    pub fn state(&self) -> TextareaState {
        self.core.state()
    }

    /// The current value.
    pub fn get_value(&self) -> &SharedString {
        self.core.text.get_value()
    }

    /// Sets the value as if the user typed it, so `Input` goes out to its
    /// form. Returns false when the change was canceled or nothing changed.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.core.set_value(value.into(), options, window, cx)
    }

    /// Empties the field, as if the user deleted the text, so `Input` goes
    /// out to its form. Returns false when the change was canceled or
    /// nothing changed.
    pub fn clear(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.core.clear(options, window, cx)
    }

    /// Moves focus to the field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.core.text.focus(window, cx);
    }

    // The submit shortcut (`submitKey`): runs `on_submit`, or submits the
    // form the textarea is in, as its submit button would. With neither, the
    // key is left alone.
    fn submit_key(this: &Entity<Self>, event: &KeyDownEvent, window: &mut Window, cx: &mut App) {
        if is_composing(window, cx) || Key::of(&event.keystroke) != Key::Enter {
            return;
        }
        let textarea = this.read(cx);
        let Some(shortcut) = textarea.submit_shortcut else {
            return;
        };
        let modifiers = event.keystroke.modifiers;
        let command = if is_apple(cx) {
            modifiers.platform
        } else {
            modifiers.control
        };
        let submits = match shortcut {
            TextareaSubmitShortcut::Enter => !modifiers.shift && !command && !modifiers.alt,
            TextareaSubmitShortcut::ModEnter => command && !modifiers.shift,
        };
        if !submits {
            return;
        }
        let on_submit = textarea.on_submit.clone();
        let form = textarea.core.text.internals().form();
        if on_submit.is_none() && form.is_none() {
            return;
        }
        let value = textarea.core.text.get_value().clone();
        window.prevent_default();
        let event = InputEvent::KeyDown(event.clone());
        match (on_submit, form) {
            (Some(on_submit), _) => on_submit(&value, &TextareaSubmitDetails { event: Some(event) }, window, cx),
            (None, Some(form)) => form.request_submit(None, Some(event), window, cx),
            (None, None) => {}
        }
    }

    /// The `root` part: the native field itself, which the app styles as the
    /// textarea. With `submit_shortcut`, its key submits.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<TextareaState> {
        let textarea = this.read(cx);
        let keys = this.downgrade();
        textarea.core.part(textarea.state(), cx).on_key_down(move |event, window, cx| {
            if let Some(textarea) = keys.upgrade() {
                Textarea::submit_key(&textarea, event, window, cx);
            }
        })
    }
}

impl Focusable for Textarea {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.core.field.read(cx).focus_handle(cx)
    }
}

/// `agent: { actions: ["setValue", "clear"] }`, with `value` controlled.
impl Agent for Textarea {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |textarea, args, window, cx| {
            let value = args.string(0)?;
            Ok(textarea.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |textarea, _args, window, cx| {
            Ok(textarea.clear(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        agent_state(&self.core)
    }
}

/// What the `group` and `clear` parts of an InputGroup hand `with`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputGroupState {
    /// Whether the field in the group holds any text.
    pub filled: bool,
    /// Whether the field in the group is disabled, by itself or by a
    /// fieldset.
    pub disabled: bool,
    /// Whether the field in the group is read-only.
    pub read_only: bool,
}

/// The options `InputGroup` takes.
#[derive(Clone, Debug, Default)]
pub struct InputGroupOptions {
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl InputGroupOptions {
    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// Text or buttons around an input, and a button that clears it. Its parts
/// are `input-group` and `input-clear`.
pub struct InputGroup {
    instance: Instance,
    // The group's element, which the field sits in.
    node: Node,
    // The first text field in the group, as of the last frame.
    field: Option<WeakNode>,
    // What `update` set, which shows while no field is in the group.
    held: InputGroupState,
    clear_focus: FocusHandle,
}

impl InputGroup {
    pub fn new(options: InputGroupOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A clear press is input the field's change reports.
        track_input(window, cx);
        let mut instance = Instance::new("input").with_agent(cx);
        instance.debug = options.debug;
        let node = Node::element();
        node.set_observer(Some(cx.entity_id()));
        Self {
            instance,
            node,
            field: None,
            held: InputGroupState::default(),
            clear_focus: cx.focus_handle(),
        }
    }

    /// Hands it new options, as a framework re-renders with new props.
    pub fn set_options(&mut self, options: InputGroupOptions, _cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The text field in the group: the first one its element holds, as of
    /// the last frame, by its node in the forms crate's page (`control`).
    pub fn control(&self) -> Option<Node> {
        self.field
            .as_ref()
            .and_then(WeakNode::upgrade)
            .filter(Node::is_connected)
    }

    /// The state the group shows: its field's, or what `update` set while
    /// no field is in it.
    pub fn state(&self) -> InputGroupState {
        match self.control().and_then(|node| ElementInternals::of(&node)) {
            Some(internals) => InputGroupState {
                filled: matches!(internals.value(), FormValue::Text(value) if !value.is_empty()),
                disabled: internals.is_disabled(),
                read_only: internals.aria().read_only,
            },
            None => self.held,
        }
    }

    /// Sets the state the group shows, like the docs' attribute tables do
    /// without a field. A field in the group shows its own.
    pub fn update(&mut self, state: InputGroupState, cx: &mut Context<Self>) {
        if self.held != state {
            self.held = state;
            cx.notify();
        }
    }

    /// Empties the field the way typing would, then returns focus to it. A
    /// disabled or read-only field keeps its text.
    pub fn clear(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(node) = self.control() else {
            return;
        };
        let state = self.state();
        if state.disabled || state.read_only {
            return;
        }
        if let Some(clear) = text_fields(&node) {
            clear(event, window, cx);
        }
        focus_control(&node, window, cx);
    }

    // Frameworks may render the field after the group, or swap it, so the
    // group looks for it again after every frame it draws.
    fn find(&mut self, cx: &mut Context<Self>) {
        let next = self
            .node
            .descendants()
            .into_iter()
            .find(|node| text_fields(node).is_some());
        if next != self.control() {
            self.field = next.map(|node| node.downgrade());
            cx.notify();
        }
    }

    // Presses on the group's own text and icons focus the field, like a
    // label would (`focusFromSurroundings`). Presses on a control or a
    // label inside it keep their own behavior.
    fn press(this: &Entity<Self>, event: &InputEvent, window: &mut Window, cx: &mut App) {
        let group = this.read(cx);
        let (Some(field), node) = (group.control(), group.node.clone()) else {
            return;
        };
        match event {
            InputEvent::Click(click) => focus_from_surroundings(click, &node, Some(&field), window, cx),
            _ => focus_control(&field, window, cx),
        }
    }

    /// The `group` part, which holds the field and the text or buttons
    /// around it.
    pub fn group(this: &Entity<Self>, cx: &App) -> Part<InputGroupState> {
        let group = this.read(cx);
        let presses = this.downgrade();
        let finds = this.downgrade();
        Part::new(&group.instance, "group", None, group.state())
            .map_el(|el| group.node.track(el))
            .on_press(move |event, window, cx| {
                if let Some(group) = presses.upgrade() {
                    InputGroup::press(&group, &event, window, cx);
                }
            })
            .on_props(move |_window, cx| {
                finds.update(cx, |group, cx| group.find(cx)).ok();
            })
    }

    /// The `clear` part, a button that empties the field and returns focus
    /// to it. `None` while the field has no text it can clear.
    pub fn clear_part(this: &Entity<Self>, cx: &App) -> Option<Part<InputGroupState>> {
        let group = this.read(cx);
        let state = group.state();
        if !state.filled || state.disabled || state.read_only {
            return None;
        }
        let presses = this.downgrade();
        Some(
            Part::new(&group.instance, "clear", None, state)
                .aria_role(Role::Button)
                .focus_mode(FocusMode::Native, Some(&group.clear_focus), false)
                .on_press(move |event, window, cx| {
                    // The press is the button's, not the group's around it.
                    cx.stop_propagation();
                    presses.update(cx, |group, cx| group.clear(Some(event), window, cx)).ok();
                }),
        )
    }
}

/// `agent: { shown: () => false }`: the Input inside is the component an
/// agent reads and clears.
impl Agent for InputGroup {
    const ACTIONS: &'static [AgentAction<Self>] = &[AgentAction::new("clear", |group, _args, window, cx| {
        group.clear(None, window, cx);
        Ok(AgentValue::Null)
    })];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&[]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        let state = self.state();
        AgentState::new()
            .with("filled", state.filled)
            .with("disabled", state.disabled)
            .with("readOnly", state.read_only)
    }

    fn agent_shown(&self, _cx: &App) -> bool {
        false
    }
}

// What the browser checks of a native field's value by itself, its
// constraint validation, which the native field leaves to its component.
// Messages are Chromium's, in English, and the first failed constraint, in
// the order Chromium checks them, gives the message.

// The native `pattern`, which the whole value must match. One that doesn't
// compile is ignored, as browsers do.
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

// The constraints a value fails, with the message for the first. An empty
// value fails none of these; `required` is the value api's.
fn constraint_problem(
    value: &str,
    native: &NativeAttributes,
    pattern: Option<&Regex>,
    user_edited: bool,
) -> Option<Problem> {
    if value.is_empty() {
        return None;
    }
    let mut flags = ValidityState::default();
    let mut message: Option<String> = None;
    let mut fail = |flag: ValidityFlag, text: String| {
        flags.set(flag, true);
        message.get_or_insert(text);
    };
    let number = native.kind == TextFieldKind::Number;
    let parsed = if number { parse_number(value) } else { None };
    if number && parsed.is_none() {
        fail(ValidityFlag::BadInput, "Please enter a number.".into());
    }
    match native.kind {
        TextFieldKind::Email => {
            // Email and URL fields hold their value without the spaces around it.
            let address = value.trim_matches(is_html_space);
            if !email_valid(address) {
                fail(ValidityFlag::TypeMismatch, email_message(address));
            }
        }
        TextFieldKind::Url => {
            if url::Url::parse(value.trim_matches(is_html_space)).is_err() {
                fail(ValidityFlag::TypeMismatch, "Please enter a URL.".into());
            }
        }
        _ => {}
    }
    if !number
        && let Some(pattern) = pattern
        && !pattern.is_match(value)
    {
        fail(ValidityFlag::PatternMismatch, "Please match the requested format.".into());
    }
    // A field's lengths judge only what its user typed.
    let length = value.chars().count();
    if user_edited {
        if let Some(max) = native.max_length
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
        if let Some(min) = native.min_length
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
    if let Some(number) = parsed {
        if let Some(min) = native.min
            && number < min
        {
            fail(
                ValidityFlag::RangeUnderflow,
                format!("Value must be greater than or equal to {}.", format_number(min, 0)),
            );
        }
        if let Some(max) = native.max
            && number > max
        {
            fail(
                ValidityFlag::RangeOverflow,
                format!("Value must be less than or equal to {}.", format_number(max, 0)),
            );
        }
        if let Some(text) = step_mismatch(number, native) {
            fail(ValidityFlag::StepMismatch, text);
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

// ASCII whitespace, which a value's sanitizing strips.
fn is_html_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{0C}' | '\r' | ' ')
}

// A valid floating-point number as HTML writes them, like `-1.5e3`.
static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^-?(?:[0-9]+(?:\.[0-9]+)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$").expect("a valid number pattern")
});

fn parse_number(text: &str) -> Option<f64> {
    if !NUMBER.is_match(text) {
        return None;
    }
    text.parse::<f64>().ok().filter(|number| number.is_finite())
}

// A value off the step's grid, which starts at `min` or 0: the message names
// the nearest values on it that the range allows.
fn step_mismatch(number: f64, native: &NativeAttributes) -> Option<String> {
    let step = match native.step {
        Some(InputStep::Any) => return None,
        Some(InputStep::Value(step)) if step > 0. && step.is_finite() => step,
        _ => 1.,
    };
    let base = native.min.unwrap_or(0.);
    let steps = (number - base) / step;
    if (steps - steps.round()).abs() <= 1e-9 * steps.abs().max(1.) {
        return None;
    }
    let lower = base + steps.floor() * step;
    let upper = lower + step;
    let allowed = |candidate: f64| {
        native.min.is_none_or(|min| candidate >= min) && native.max.is_none_or(|max| candidate <= max)
    };
    let decimals = decimals_of(step).max(decimals_of(base));
    let (lower_text, upper_text) = (format_number(lower, decimals), format_number(upper, decimals));
    Some(match (allowed(lower), allowed(upper)) {
        (true, true) => format!(
            "Please enter a valid value. The two nearest valid values are {lower_text} and {upper_text}."
        ),
        (true, false) => format!("Please enter a valid value. The nearest valid value is {lower_text}."),
        (false, true) => format!("Please enter a valid value. The nearest valid value is {upper_text}."),
        (false, false) => "Please enter a valid value.".into(),
    })
}

// How many decimals a number needs to be written exactly.
fn decimals_of(value: f64) -> usize {
    let text = format!("{value}");
    text.split_once('.').map_or(0, |(_, decimals)| decimals.len().min(12))
}

// A number as a message writes it, without trailing zeros.
fn format_number(value: f64, decimals: usize) -> String {
    let mut text = if decimals == 0 && value.fract() == 0. {
        format!("{value}")
    } else {
        format!("{value:.decimals$}")
    };
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if text == "-0" {
        text = "0".into();
    }
    text
}

// The characters an e-mail address's local part may hold besides letters
// and digits.
const LOCAL_SYMBOLS: &str = ".!#$%&'*+/=?^_`{|}~-";

fn local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || LOCAL_SYMBOLS.contains(c)
}

// A valid e-mail address as HTML defines one: a local part, `@`, and a
// domain of labels that start and end with a letter or a digit.
fn email_valid(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.chars().all(local_char)
        && !domain.is_empty()
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

// Why an address isn't one, as Chromium says it.
fn email_message(address: &str) -> String {
    let Some((local, domain)) = address.split_once('@') else {
        return format!("Please include an '@' in the email address. '{address}' is missing an '@'.");
    };
    if local.is_empty() {
        return format!("Please enter a part followed by '@'. '{address}' is incomplete.");
    }
    if domain.is_empty() {
        return format!("Please enter a part following '@'. '{address}' is incomplete.");
    }
    if let Some(symbol) = local.chars().find(|c| !local_char(*c)) {
        return format!("A part followed by '@' should not contain the symbol '{symbol}'.");
    }
    if let Some(symbol) = domain
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '.'))
    {
        return format!("A part following '@' should not contain the symbol '{symbol}'.");
    }
    if domain.starts_with('.') || domain.contains("..") {
        return format!("'.' is used at a wrong position in '{domain}'.");
    }
    "Please enter an email address.".into()
}
