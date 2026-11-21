//! Port of `components/number-input/number-input.ts`: a text field for a
//! number, stepped with the keyboard, its buttons, the wheel or a scrub, and
//! formatted for a locale.
//!
//! The input's state and what it does are `number_input_api.rs`'s. Here are
//! the options, what each part's state holds, and the parts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    AnyWindowHandle, App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    KeystrokeEvent, ParentElement, Pixels, Point, SharedString, Styled, Subscription, Task, Window,
};
use ui_sdk_direct_manipulation::{RepeatPresses, bind_repeat_presses};
use ui_sdk_drag_and_drop::{DragSession, create_drag_session, on_pointer_down};
use ui_sdk_forms::{Association, ElementInternals, associate, submit_implicitly};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentArgs, AgentState, AgentValue, Aria, ChangeCallback, ChangeDetails,
    ChangeOptions, CommitCallback, Controllable, Instance, Key, Need, Part, Reason, track_input,
};
use ui_sdk_native::{
    BeforeEdit, FieldHook, FieldNotice, TextField, TextFieldEvent, TextFieldKind,
    TextFieldOptions as FieldOptions, TextFieldStyle,
};
use ui_sdk_number::NumberFormatOptions;
use ui_sdk_platform::is_apple;
use ui_sdk_text::{bind_page_locale, page_locale};

use crate::number_input_api::{Pending, Reading, Settings, before_input, display, settings};

/// Why a number input's value wants to change: typing a valid number
/// (`InputChange`), Enter or leaving the field (`InputCommit`), a key
/// (`StepKey`), an increment or decrement button pressed or held
/// (`IncrementPress`, `DecrementPress`), the wheel (`Wheel`), a drag on a
/// scrub area (`Scrub`), a screen reader's adjustment of the spinbutton
/// (`AccessibilityAdjust`), a form reset (`Reset`), or a call
/// (`Imperative`).
pub type NumberInputValueChangeReason = Reason;

/// Options for every write method: the reason and event `on_value_change`
/// receives.
pub type NumberInputChangeOptions = ChangeOptions;

/// The options `NumberInput` takes.
#[derive(Clone, Default)]
pub struct NumberInputOptions {
    /// The value, or `Some(None)` for an empty field. Pass it to control the
    /// field.
    pub value: Option<Option<f64>>,
    /// The value at first, when uncontrolled. A form reset returns to it.
    /// Empty by default.
    pub default_value: Option<f64>,
    /// Runs when the value changes: as a typed number becomes valid, and on
    /// every step. `details.cancel()` stops it. It runs inside the input's
    /// update, so it may update any entity but the input.
    pub on_value_change: Option<ChangeCallback<Option<f64>>>,
    /// Runs once an interaction that changed the value ends: typing is
    /// committed, a key or a button is released, or a scrub ends. For
    /// saving.
    pub on_value_commit: Option<CommitCallback<Option<f64>>>,
    /// The lowest value. Typed values below it are raised to it when
    /// committed.
    pub min: Option<f64>,
    /// The highest value. Typed values above it are lowered to it when
    /// committed.
    pub max: Option<f64>,
    /// How far one step moves. Committed values snap to steps from `min`, or
    /// from 0 without one. Defaults to 1, or the smallest percent a percent
    /// format shows, like 0.01.
    pub step: Option<f64>,
    /// How far Page Up, Page Down, and Shift with an arrow key, a button, the
    /// wheel or a scrub move. Defaults to 10 steps.
    pub large_step: Option<f64>,
    /// How far Alt (Option on a Mac) with an arrow key, the wheel or a scrub
    /// moves, for fine changes. Values it reaches snap to it rather than to
    /// `step`. Defaults to a tenth of a step.
    pub small_step: Option<f64>,
    /// How the value shows: `Intl.NumberFormat` options, like a currency.
    /// Typing accepts the same format in the locale, and percents are
    /// fractions: 50% is 0.5.
    pub format: Option<NumberFormatOptions>,
    /// The locale for formatting and reading typed numbers. Defaults to the
    /// page's language.
    pub locale: Option<SharedString>,
    /// Lets the mouse wheel step the value while the input has focus.
    pub wheel: bool,
    /// Disables it like a native control: it can't be focused or changed,
    /// and isn't submitted.
    pub disabled: bool,
    /// Keeps it focusable and submitted, but stops changes.
    pub read_only: bool,
    /// Blocks form submission while it's empty, with the browser's own
    /// message.
    pub required: bool,
    /// The name it submits under. The number is submitted as is, like
    /// `1234.5`, never formatted.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
    /// The control's placeholder, an attribute the original passes through
    /// to its native input.
    pub placeholder: Option<SharedString>,
    /// The colors the control's text, caret, selection and placeholder paint
    /// with, and its alignment.
    pub field_style: TextFieldStyle,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl NumberInputOptions {
    /// The value, or `None` for an empty field.
    pub fn value(mut self, value: impl Into<Option<f64>>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn default_value(mut self, value: f64) -> Self {
        self.default_value = Some(value);
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&Option<f64>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_value_commit(
        mut self,
        callback: impl Fn(&Option<f64>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_commit = Some(Rc::new(callback));
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

    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    pub fn large_step(mut self, step: f64) -> Self {
        self.large_step = Some(step);
        self
    }

    pub fn small_step(mut self, step: f64) -> Self {
        self.small_step = Some(step);
        self
    }

    pub fn format(mut self, format: NumberFormatOptions) -> Self {
        self.format = Some(format);
        self
    }

    pub fn locale(mut self, locale: impl Into<SharedString>) -> Self {
        self.locale = Some(locale.into());
        self
    }

    pub fn wheel(mut self, wheel: bool) -> Self {
        self.wheel = wheel;
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

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
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
}

/// Which way to drag a scrub area: right and up increase.
pub type NumberInputScrubAreaOrientation = gpui::Orientation;

/// The props the `scrub-area` part takes.
#[derive(Clone, Debug, PartialEq)]
pub struct NumberInputScrubAreaProps {
    /// Which way to drag: right and up increase. Horizontal by default.
    pub orientation: NumberInputScrubAreaOrientation,
    /// How far the pointer moves for one step, in pixels. 2 by default.
    pub pixels_per_step: Option<f64>,
    /// Tells one of several scrub areas apart, like a label and an icon
    /// that both drive the input, since each part needs an id of its own.
    pub key: Option<SharedString>,
}

impl Default for NumberInputScrubAreaProps {
    fn default() -> Self {
        Self {
            orientation: NumberInputScrubAreaOrientation::Horizontal,
            pixels_per_step: None,
            key: None,
        }
    }
}

impl NumberInputScrubAreaProps {
    pub fn key(mut self, key: impl Into<SharedString>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn orientation(mut self, orientation: NumberInputScrubAreaOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    pub fn pixels_per_step(mut self, pixels: f64) -> Self {
        self.pixels_per_step = Some(pixels);
        self
    }
}

/// The props the `increment` and `decrement` parts take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NumberInputButtonProps {
    /// Turns off this button alone. Pressing, clicking or holding it doesn't
    /// step.
    pub disabled: bool,
}

impl NumberInputButtonProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// What the root, group and control parts' `with` and the app read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberInputState {
    /// The value, or `None` when empty.
    pub value: Option<f64>,
    /// Whether it's disabled, by itself or by a fieldset.
    pub disabled: bool,
    /// Whether it's read-only: it stays focusable and submitted, but typing,
    /// steps and scrubs don't change it.
    pub read_only: bool,
    /// Whether a form won't submit while it's empty.
    pub required: bool,
    /// Whether a scrub is moving the value.
    pub scrubbing: bool,
}

/// What the `increment` and `decrement` parts' `with` reads: the field's
/// state, with whether the button can step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberInputButtonState {
    pub value: Option<f64>,
    /// Whether the button can't step, because it or the field is disabled,
    /// the field is read-only, or the value is at its bound.
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub scrubbing: bool,
    /// Whether the value is at the bound this button steps toward, like the
    /// maximum for the increment.
    pub at_bound: bool,
}

/// What a `scrub-area` part's `with` reads: the field's state, with the
/// way it drags (`data-orientation`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberInputScrubAreaState {
    pub value: Option<f64>,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub scrubbing: bool,
    pub orientation: NumberInputScrubAreaOrientation,
}

/// What the `scrub-area-cursor` part's `with` reads: the field's state,
/// with where the cursor it stands in for is drawn while the pointer is
/// locked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberInputScrubAreaCursorState {
    pub value: Option<f64>,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub scrubbing: bool,
    /// Where the cursor is, in window coordinates. gpui can't lock the
    /// pointer, so it's always `None`, and the part is hidden.
    pub cursor: Option<Point<Pixels>>,
}

/// The keys the control handles, besides typing.
const CONTROL_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "shift+ArrowUp",
    "shift+ArrowDown",
    "alt+ArrowUp",
    "alt+ArrowDown",
    "PageUp",
    "PageDown",
    "Home",
    "End",
    "Enter",
];

/// A number input (`numberInput`). Its entity is its controller.
pub struct NumberInput {
    pub(crate) instance: Instance,
    pub(crate) value: Controllable<Option<f64>>,
    // What a form reset returns to.
    pub(crate) initial: Option<f64>,
    // Whether the options pass a `default_value`, which a controlled input
    // resets to.
    pub(crate) has_default: bool,
    // What the input shows. Typing changes it before the value.
    pub(crate) text: SharedString,
    pub(crate) settings: Settings,
    // The page's language, followed as it changes.
    pub(crate) page_locale: Option<SharedString>,
    pub(crate) scrubbing: bool,
    // Where the scrub cursor is drawn, while the pointer is locked.
    pub(crate) cursor: Option<Point<Pixels>>,
    pub(crate) placeholder: Option<SharedString>,
    pub(crate) field_style: TextFieldStyle,
    // The native field the control is, and the text it started with.
    pub(crate) field: Entity<TextField>,
    field_default: SharedString,
    pub(crate) internals: ElementInternals,
    pub(crate) drags: DragSession<Self>,
    pub(crate) increment: RepeatPresses<Self>,
    pub(crate) decrement: RepeatPresses<Self>,
    // Each button's own `disabled`, as it last rendered.
    pub(crate) increment_disabled: Cell<bool>,
    pub(crate) decrement_disabled: Cell<bool>,
    // The value when the current interaction began, so a commit only reports
    // a real change. A controlled value or a call can move it in between.
    pub(crate) start_value: Option<f64>,
    // What changed the value since, reported at the next commit.
    pub(crate) pending: Option<Pending>,
    pub(crate) wheel_timer: Option<Task<()>>,
    // Whether the text was typed since the last commit. Untouched text keeps
    // the exact value, even when it shows fewer decimals.
    pub(crate) edited: bool,
    // The value and settings the text last followed.
    pub(crate) shown: Option<f64>,
    pub(crate) shown_format: (Option<NumberFormatOptions>, SharedString, f64),
    pub(crate) reading: Rc<RefCell<Reading>>,
    before_input: FieldHook<BeforeEdit>,
    pub(crate) window: AnyWindowHandle,
    pub(crate) write_queued: bool,
    _subscriptions: Vec<Subscription>,
}

impl NumberInput {
    pub fn new(options: NumberInputOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A change's source follows how the window was used.
        track_input(window, cx);
        let settings = settings(&options);
        let (controlled, default_value) = (number_of(options.value), finite(options.default_value));
        let start = controlled.unwrap_or(default_value);
        let mut value = Controllable::new("value", start);
        value.set_controlled(controlled);
        value.set_on_change(options.on_value_change.clone());
        value.set_on_commit(options.on_value_commit.clone());
        let mut instance = Instance::new("number-input").with_agent(cx);
        instance.debug = options.debug;
        let page_locale = page_locale(cx);
        let locale = ui_sdk_text::locale_from(settings.locale.as_ref(), page_locale.as_ref());
        let text = display(start, &settings, &locale);
        let internals = ElementInternals::new(cx);
        // Enter in it submits a form with no other text field, like a text input.
        internals.set_blocks_implicit_submission(true);
        let reading = Rc::new(RefCell::new(Reading::of(&settings, &locale)));
        let hook = before_input(reading.clone());
        let disabled = settings.disabled || internals.form_disabled() || internals.control_attrs().disabled;
        let field_options = native_options(
            &settings,
            disabled,
            options.placeholder.clone(),
            options.field_style,
            text.clone(),
            hook.clone(),
            &internals,
        );
        let field = cx.new(|cx| TextField::new(field_options, window, cx));
        internals.set_focus_handle(Some(field.read(cx).focus_handle(cx)));
        let this = cx.entity();
        let subscriptions = vec![
            cx.subscribe_in(&field, window, |input: &mut Self, _, event, window, cx| {
                input.field_event(event, window, cx)
            }),
            cx.observe_keystrokes(Self::keystroke),
            associate(
                &internals,
                Association::new()
                    .reset(cx, |input: &mut Self, window, cx| input.reset(window, cx))
                    .disabled(cx, |input: &mut Self, _disabled, _window, cx| {
                        input.sync_field(cx);
                        cx.notify();
                    }),
            ),
            bind_page_locale(cx, |input: &mut Self, locale, cx| {
                input.page_locale = locale;
                input.sync_reading();
                input.follow();
                input.sync_form();
                input.queue_write(cx);
                cx.notify();
            }),
            // Its field, or a fieldset around it, notifies it when they
            // disable it, which the native field follows.
            cx.observe_in(&this, window, |input: &mut Self, _, _window, cx| {
                input.sync_field(cx)
            }),
        ];
        let input = Self {
            instance,
            value,
            initial: default_value.or(start),
            has_default: default_value.is_some(),
            text: text.clone(),
            shown_format: (settings.format.clone(), locale, settings.step),
            settings,
            page_locale,
            scrubbing: false,
            cursor: None,
            placeholder: options.placeholder,
            field_style: options.field_style,
            field,
            field_default: text,
            internals,
            drags: create_drag_session(cx),
            increment: bind_repeat_presses(None, Self::button_options(1.0), cx),
            decrement: bind_repeat_presses(None, Self::button_options(-1.0), cx),
            increment_disabled: Cell::new(false),
            decrement_disabled: Cell::new(false),
            start_value: start,
            pending: None,
            wheel_timer: None,
            edited: false,
            shown: start,
            reading,
            before_input: hook,
            window: window.window_handle(),
            write_queued: false,
            _subscriptions: subscriptions,
        };
        input.sync_form();
        input
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A new controlled value shows in the field at once, unless
    /// the text typed so far already reads as it.
    pub fn set_options(&mut self, options: NumberInputOptions, window: &mut Window, cx: &mut Context<Self>) {
        let settings = settings(&options);
        let mut changed = settings != self.settings
            || self.placeholder != options.placeholder
            || self.field_style != options.field_style;
        self.settings = settings;
        self.placeholder = options.placeholder;
        self.field_style = options.field_style;
        let default_value = finite(options.default_value);
        if let Some(default) = default_value {
            self.initial = Some(default);
        }
        self.has_default = default_value.is_some();
        self.value.set_on_change(options.on_value_change);
        self.value.set_on_commit(options.on_value_commit);
        self.instance.debug = options.debug;
        changed |= self.arrive(number_of(options.value));
        self.sync_reading();
        self.follow();
        self.sync_field(cx);
        self.sync_form();
        self.write_text(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, `Some(None)` for an empty field, or
    /// `None` to let the input keep its own.
    pub fn set_controlled_value(
        &mut self,
        value: Option<Option<f64>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.arrive(number_of(value)) {
            self.follow();
            self.sync_form();
            self.write_text(window, cx);
            cx.notify();
        }
    }

    // A value from options shows in the same pass, unless the text typed so
    // far already reads as it (`deriveFromOptions`).
    fn arrive(&mut self, value: Option<Option<f64>>) -> bool {
        let before = *self.value.get();
        let changed = self.value.set_controlled(value);
        if let Some(value) = value
            && value != before
            && self.typed() != value
        {
            self.text = self.display(value);
        }
        changed
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The native field the control renders, like to read where its caret
    /// is.
    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// Its part in forms, on the control.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    /// What the input shows, which typing changes before the value.
    pub fn text(&self) -> &SharedString {
        &self.text
    }

    /// What the root, group and control parts' state holds.
    pub fn state(&self) -> NumberInputState {
        NumberInputState {
            value: *self.value.get(),
            disabled: self.is_disabled(),
            read_only: self.settings.read_only,
            required: self.settings.required,
            scrubbing: self.scrubbing,
        }
    }

    // What the field hears, once its update is over: the text it was
    // typed, pasted or composed into.
    fn field_event(&mut self, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Input(edit) => {
                let text = edit.value.clone();
                // Typing reports how it was typed, though no event explains it.
                ui_sdk_kernel::with_source(edit.source, || self.type_text(text, None, window, cx));
            }
            // Browsers differ on whether an `input` follows the end of a composition.
            TextFieldEvent::CompositionEnd(_) => {
                let text = self.field.read(cx).value().clone();
                if text != self.text {
                    self.type_text(text, None, window, cx);
                }
            }
            // Its own blur comes after what an input method composed as focus
            // left, which it commits (`focusout`).
            TextFieldEvent::Blur => self.blurred(window, cx),
            _ => {}
        }
    }

    // Enter in the field submits its form implicitly, the browser's own
    // action for a native input, once every listener had the key and none
    // prevented it, after the text committed.
    fn keystroke(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.action.is_some() || window.default_prevented() || Key::of(&event.keystroke) != Key::Enter {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        let plain = if is_apple(cx) {
            !modifiers.control && !modifiers.platform
        } else {
            !modifiers.control && !modifiers.alt && !modifiers.platform
        };
        let field = self.field.read(cx);
        if !plain || !field.is_focused(window) || field.is_composing() || field.is_disabled() {
            return;
        }
        submit_implicitly(self.internals.form().as_ref(), window, cx);
    }

    // Hands the native field its attributes, and what disables it from
    // outside.
    pub(crate) fn sync_field(&self, cx: &mut App) {
        let options = native_options(
            &self.settings,
            self.is_disabled(),
            self.placeholder.clone(),
            self.field_style,
            self.field_default.clone(),
            self.before_input.clone(),
            &self.internals,
        );
        self.field.update(cx, |field, cx| field.set_options(options, cx));
    }

    // The state a part of the field renders with.
    fn button_state(&self, own: bool, direction: f64) -> NumberInputButtonState {
        let state = self.state();
        NumberInputButtonState {
            value: state.value,
            disabled: self.is_blocked(own, direction),
            read_only: state.read_only,
            required: state.required,
            scrubbing: state.scrubbing,
            at_bound: self.at_bound(direction, self.settings.step),
        }
    }

    /// The `root` part, around everything. It follows a scrub's pointer, so
    /// render it on every frame.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<NumberInputState> {
        let input = this.read(cx);
        Part::new(&input.instance, "root", None, input.state()).child(input.drags.listener())
    }

    /// The `group` part, which holds the control and its buttons.
    pub fn group(this: &Entity<Self>, cx: &App) -> Part<NumberInputState> {
        let input = this.read(cx);
        Part::new(&input.instance, "group", None, input.state()).aria_role(Role::Group)
    }

    /// The `control` part: the native field, a spinbutton with the value,
    /// bounds and step. Name it with `aria_label` or a field's label. The
    /// text draws with the part's text style and `field_style`.
    pub fn control(this: &Entity<Self>, cx: &App) -> Part<NumberInputState> {
        let input = this.read(cx);
        let state = input.state();
        let field = input.field.read(cx);
        let focus = field.focus_handle(cx);
        let disabled = field.is_disabled();
        let own = Aria::new()
            .disabled(disabled)
            .read_only(field.is_read_only())
            .required(field.is_required());
        let internals = input.internals.clone();
        let aria = internals.aria();
        let (el, _) = TextField::root(&input.field, cx).into_element_parts();
        let keys = this.downgrade();
        let ups = this.downgrade();
        let wheels = this.downgrade();
        Part::new(&input.instance, "control", None, state)
            .map_el(move |_| internals.track(el))
            .aria_role(Role::Textbox)
            .focus_mode(FocusMode::Native, Some(&focus), disabled)
            .aria(own)
            .aria(aria)
            .on_keys(CONTROL_KEYS, move |event, window, cx| {
                keys.update(cx, |input, cx| input.keydown(event, window, cx)).ok();
            })
            .on_key_up(move |event, window, cx| {
                ups.update(cx, |input, cx| input.keyup(event, window, cx)).ok();
            })
            // Listens before the field's own scrolling, which a step stops.
            .on_scroll(move |event, window, cx| {
                wheels.update(cx, |input, cx| input.wheel(event, window, cx)).ok();
            })
    }

    /// The `increment` part, a button that steps the value up, and repeats
    /// while held. Name it with `aria_label`, like "Increase". It's named
    /// apart from the api's `increment`, which steps from code.
    pub fn increment_button(
        this: &Entity<Self>,
        props: NumberInputButtonProps,
        cx: &App,
    ) -> Part<NumberInputButtonState> {
        Self::button(this, props, 1.0, cx)
    }

    /// The `decrement` part, a button that steps the value down, and repeats
    /// while held. Name it with `aria_label`, like "Decrease". It's named
    /// apart from the api's `decrement`, which steps from code.
    pub fn decrement_button(
        this: &Entity<Self>,
        props: NumberInputButtonProps,
        cx: &App,
    ) -> Part<NumberInputButtonState> {
        Self::button(this, props, -1.0, cx)
    }

    fn button(
        this: &Entity<Self>,
        props: NumberInputButtonProps,
        direction: f64,
        cx: &App,
    ) -> Part<NumberInputButtonState> {
        let input = this.read(cx);
        input.button_prop(direction).set(props.disabled);
        let (slot, presses) = if direction > 0.0 {
            ("increment", &input.increment)
        } else {
            ("decrement", &input.decrement)
        };
        let state = input.button_state(props.disabled, direction);
        let native = input.is_natively_disabled(props.disabled, direction);
        let entity = this.downgrade();
        let part = Part::new(&input.instance, slot, None, state)
            .aria_role(Role::Button)
            .unneeded(
                Need::Focus,
                "The input keeps focus: the buttons are out of the tab order, and a press doesn't take focus.",
            )
            .aria(
                Aria::new()
                    .controls(input.instance.element_id("control", None))
                    .disabled(state.disabled),
            )
            .on_press(move |event, window, cx| {
                if native {
                    return;
                }
                entity
                    .update(cx, |input, cx| input.click_button(&event, direction, window, cx))
                    .ok();
            });
        presses.bind(part)
    }

    /// A `scrub-area` part: drag across it to change the value, like a label
    /// in a design tool. Any number of them, anywhere, drive one input.
    pub fn scrub_area(
        this: &Entity<Self>,
        props: NumberInputScrubAreaProps,
        cx: &App,
    ) -> Part<NumberInputScrubAreaState> {
        let input = this.read(cx);
        let state = input.state();
        let area = NumberInputScrubAreaState {
            value: state.value,
            disabled: state.disabled,
            read_only: state.read_only,
            required: state.required,
            scrubbing: state.scrubbing,
            orientation: props.orientation,
        };
        let entity = this.downgrade();
        let part = Part::new(&input.instance, "scrub-area", props.key.as_deref(), area);
        on_pointer_down(part, move |event, window, cx| {
            entity
                .update(cx, |input, cx| input.scrub(event, props.clone(), window, cx))
                .ok();
        })
    }

    /// The `scrub-area-cursor` part, which stands in for the mouse cursor
    /// while the pointer is locked: `None` unless a scrub is under way. gpui
    /// can't lock the pointer, so it stays hidden.
    pub fn scrub_area_cursor(this: &Entity<Self>, cx: &App) -> Option<Part<NumberInputScrubAreaCursorState>> {
        let input = this.read(cx);
        if !input.scrubbing {
            return None;
        }
        let state = input.state();
        let cursor = NumberInputScrubAreaCursorState {
            value: state.value,
            disabled: state.disabled,
            read_only: state.read_only,
            required: state.required,
            scrubbing: state.scrubbing,
            cursor: input.cursor,
        };
        let part = Part::new(&input.instance, "scrub-area-cursor", None, cursor)
            .absolute()
            .top_0()
            .left_0();
        Some(match input.cursor {
            Some(at) => part.ml(at.x).mt(at.y),
            None => part.invisible(),
        })
    }
}

impl Focusable for NumberInput {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.read(cx).focus_handle(cx)
    }
}

// The native field's attributes, from the settings. Its `on_submit`, which
// asks to submit the form it's in, submits the form the forms crate found.
fn native_options(
    settings: &Settings,
    disabled: bool,
    placeholder: Option<SharedString>,
    style: TextFieldStyle,
    default_value: SharedString,
    hook: FieldHook<BeforeEdit>,
    internals: &ElementInternals,
) -> FieldOptions {
    let submits = internals.clone();
    let submit: FieldNotice = Rc::new(move |_field, window, cx| {
        let internals = submits.clone();
        window.defer(cx, move |window, cx| {
            if let Some(form) = internals.form() {
                form.request_submit(None, None, window, cx);
            }
        });
    });
    FieldOptions {
        default_value,
        placeholder,
        kind: TextFieldKind::Text,
        disabled,
        read_only: settings.read_only,
        required: settings.required,
        style,
        on_before_input: Some(hook),
        on_submit: Some(submit),
        ..FieldOptions::default()
    }
}

// A number that isn't one, like NaN, is an empty field, and one way to
// write it, so it never reads as a change from itself.
fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite())
}

fn number_of(value: Option<Option<f64>>) -> Option<Option<f64>> {
    value.map(finite)
}

// Whether a step an agent asks for takes the large step: `{ "large": true }`.
fn large(args: &AgentArgs) -> bool {
    matches!(args.value(0), AgentValue::Object(options) if options.get("large") == Some(&AgentValue::Bool(true)))
}

/// `agent: { actions: ["setValue", "increment", "decrement"] }` and
/// `commits: ["value"]`.
impl Agent for NumberInput {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |input, args, window, cx| {
            let value = if args.is_missing(0) {
                None
            } else {
                Some(args.number(0)?)
            };
            Ok(input
                .set_value(value, ChangeOptions::default(), window, cx)
                .into())
        }),
        AgentAction::new("increment", |input, args, window, cx| {
            Ok(input
                .increment(ChangeOptions::default(), large(args), window, cx)
                .into())
        }),
        AgentAction::new("decrement", |input, args, window, cx| {
            Ok(input
                .decrement(ChangeOptions::default(), large(args), window, cx)
                .into())
        }),
    ];

    const COMMITS: &'static [&'static str] = &["value"];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", *self.value.get())
            .with("disabled", self.is_disabled())
            .with("readOnly", self.settings.read_only)
            .with("required", self.settings.required)
            .with("min", self.settings.min)
            .with("max", self.settings.max)
            .with("step", self.settings.step)
            .with(
                "placeholder",
                self.placeholder
                    .as_ref()
                    .map(|placeholder| placeholder.to_string()),
            )
    }
}
