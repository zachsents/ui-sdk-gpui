//! Port of `components/otp-input/otp-input.ts`: the input's definition, its
//! parts and their state, and what an agent reads. Its behavior is
//! `otp_input_api.rs`, and the pure side `otp_input_model.rs`.

use gpui::prelude::*;
use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, KeystrokeEvent, MouseButton, Role as AccessRole,
    SharedString, Subscription, WeakEntity, Window,
};
use regex::Regex;
use ui_sdk_forms::{ElementInternals, submit_implicitly};
use ui_sdk_kernel::dev::{self, Origin};
use ui_sdk_kernel::part::{FocusMode, with_role};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentSurface, Aria, ChangeOptions, Controllable, InputEvent, Instance, Key, Part,
    Reason, track_input,
};
use ui_sdk_native::{TextField, TextFieldOptions as FieldOptions};
use ui_sdk_platform::{is_apple, is_rtl};

use crate::otp_input_api::{OtpInputOptions, OtpInputTextAlign, Settings, clean_code, settings};
use crate::otp_input_model::{OtpInputPattern, OtpInputSlotData, Selection, slots_of};

/// Reads a pattern from text (`readPattern`), like a custom element's
/// attribute: "digits", "letters" and "alphanumeric" are the presets, and
/// anything else is a regular expression each character must match, like
/// "[A-F0-9]". One that doesn't compile falls back to digits rather than
/// break the input.
pub fn read_pattern(attribute: &str) -> OtpInputPattern {
    match attribute {
        "digits" => OtpInputPattern::Digits,
        "letters" => OtpInputPattern::Letters,
        "alphanumeric" => OtpInputPattern::Alphanumeric,
        _ => match Regex::new(attribute) {
            Ok(regex) => OtpInputPattern::Regex(regex),
            Err(_) => {
                dev::warn(
                    &format!("pattern \"{attribute}\" isn't a valid regular expression, so it takes digits."),
                    Some(Origin {
                        component: "otp-input",
                        id: None,
                    }),
                );
                OtpInputPattern::Digits
            }
        },
    }
}

/// What the `root`, `control`, `group` and `separator` parts hand `with`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OtpInputState {
    /// The code.
    pub value: SharedString,
    /// One entry per slot: its character, whether it's active, and whether
    /// it draws a caret.
    pub slots: Vec<OtpInputSlotData>,
    /// Whether the input has focus.
    pub focused: bool,
    /// Whether every slot is filled.
    pub complete: bool,
    /// Whether the code has any character.
    pub filled: bool,
    /// Whether it's disabled, by itself or by a fieldset.
    pub disabled: bool,
}

/// The state the `slot` part hands `with`: its slot's data, with which slot
/// it is and the input's state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OtpInputSlotState {
    /// The character in the slot, or `None` when it's empty.
    pub char: Option<char>,
    /// Whether the slot is where typing goes.
    pub active: bool,
    /// Whether the slot should draw a caret: it's active and empty.
    pub has_fake_caret: bool,
    /// Which character of the code the slot shows, from 0.
    pub index: usize,
    /// Whether the input has focus.
    pub focused: bool,
    /// Whether it's disabled, by itself or by a fieldset.
    pub disabled: bool,
}

/// A one-time code input: one native field for the whole code, drawn as a
/// slot per character. Its entity is its controller.
pub struct OtpInput {
    pub(crate) instance: Instance,
    pub(crate) this: WeakEntity<Self>,
    // The real input, which takes focus, holds the code and submits it.
    pub(crate) field: Entity<TextField>,
    pub(crate) internals: ElementInternals,
    // The code, with `on_value_change` and `on_value_commit`.
    pub(crate) value: Controllable<SharedString>,
    // What a form reset returns to.
    pub(crate) initial: SharedString,
    // Whether `default_value` was passed, which a controlled code resets to.
    pub(crate) has_default: bool,
    pub(crate) settings: Settings,
    // The code is flagged as too short, which only a partial code the user
    // typed is. The pattern then leaves the length to `minlength`.
    pub(crate) too_short: bool,
    pub(crate) focused: bool,
    pub(crate) selection: Selection,
    // The paste or several characters being written, so its change reports
    // it.
    pub(crate) inserting: Option<(Reason, Option<InputEvent>)>,
    // Set from a controlled change until its owner has had a chance to pass
    // it back, which `owner_wait` counts.
    pub(crate) awaiting_owner: bool,
    pub(crate) owner_wait: u64,
    // Whether the last edit deleted, until the caret moves some other way.
    pub(crate) after_delete: bool,
    // A mouse press focused the field, so the focus keeps its slot.
    pub(crate) pressed: bool,
    // Whether the code last changed through the user's edit.
    pub(crate) user_edited: bool,
    pub(crate) pattern_cache: Option<(String, Option<Regex>)>,
    _subscriptions: Vec<Subscription>,
}

impl OtpInput {
    pub fn new(options: OtpInputOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        track_input(window, cx);
        let mut instance = Instance::new("otp-input").with_agent(cx);
        instance.debug = options.debug;
        let base = settings(&options);
        let first = options.value.as_ref().or(options.default_value.as_ref());
        let first = clean_code(first.map_or("", |value| value.as_ref()), &base.pattern, base.max_length);
        let initial = clean_code(
            options.default_value.as_ref().map_or("", |value| value.as_ref()),
            &base.pattern,
            base.max_length,
        );
        let mut value = Controllable::new("value", first);
        value.set_controlled(
            options
                .value
                .as_ref()
                .map(|value| clean_code(value, &base.pattern, base.max_length)),
        );
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());
        value.set_on_commit(options.on_value_commit.clone());
        let internals = ElementInternals::new(cx);
        internals.set_id(base.id.clone(), cx);
        // Enter in it submits a form with no other text field, like any.
        internals.set_blocks_implicit_submission(true);
        let field = cx.new(|cx| TextField::new(FieldOptions::default(), window, cx));
        internals.set_focus_handle(Some(field.read(cx).focus_handle(cx)));
        let this = cx.entity();
        let subscriptions = vec![
            cx.subscribe_in(&field, window, |otp: &mut Self, _, event, window, cx| {
                otp.field_event(event, window, cx)
            }),
            cx.observe_keystrokes(Self::keystroke),
            // A fieldset, or a Field writing the input's `disabled`, notifies
            // it, and the field follows.
            cx.observe_in(&this, window, |otp: &mut Self, _, _window, cx| otp.sync_field(cx)),
            internals.on_reset(cx, |otp: &mut Self, window, cx| otp.reset(window, cx)),
        ];
        let mut otp = Self {
            instance,
            this: cx.weak_entity(),
            field,
            internals,
            value,
            initial,
            has_default: options.default_value.is_some(),
            settings: base,
            too_short: false,
            focused: false,
            selection: Selection::default(),
            inserting: None,
            awaiting_owner: false,
            owner_wait: 0,
            after_delete: false,
            pressed: false,
            user_edited: false,
            pattern_cache: None,
            _subscriptions: subscriptions,
        };
        otp.sync_field(cx);
        otp.sync_element(window, cx);
        otp.sync_form();
        otp
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A new controlled code shows in the field at once.
    pub fn set_options(&mut self, options: OtpInputOptions, window: &mut Window, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        let base = settings(&options);
        let mut changed = base != self.settings;
        if base.id != self.settings.id {
            self.internals.set_id(base.id.clone(), cx);
        }
        if let Some(default) = &options.default_value {
            self.initial = clean_code(default, &base.pattern, base.max_length);
        }
        self.has_default = options.default_value.is_some();
        let controlled = options
            .value
            .as_ref()
            .map(|value| clean_code(value, &base.pattern, base.max_length));
        self.settings = base;
        changed |= self.value.set_controlled(controlled);
        self.value.set_on_change(options.on_value_change.clone());
        self.value.set_on_before_change(options.on_before_value_change.clone());
        self.value.set_on_commit(options.on_value_commit);
        self.sync_element(window, cx);
        self.sync_field(cx);
        self.sync_form();
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled code, or `None` to let the input keep its own.
    /// It's cleaned the way a typed code is.
    pub fn set_controlled_value(&mut self, value: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let controlled = value.map(|value| clean_code(&value, &self.settings.pattern, self.settings.max_length));
        if self.value.set_controlled(controlled) {
            self.sync_element(window, cx);
            self.sync_form();
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The native field that holds the code, to read. Write the code through
    /// the input, which reports it.
    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// Its part in forms.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    /// How many characters the code has.
    pub fn max_length(&self) -> usize {
        self.settings.max_length
    }

    /// The slots rendered from the code, and the selection while focused.
    pub fn slots(&self) -> Vec<OtpInputSlotData> {
        slots_of(
            self.value.get(),
            self.settings.max_length,
            self.focused.then_some(self.selection),
        )
    }

    /// What the root, control, group and separator parts hand `with`.
    pub fn state(&self) -> OtpInputState {
        let value = self.value.get().clone();
        OtpInputState {
            slots: self.slots(),
            focused: self.focused,
            complete: value.chars().count() == self.settings.max_length,
            filled: !value.is_empty(),
            disabled: self.is_disabled(),
            value,
        }
    }

    // Enter in the field submits its form implicitly, the browser's own
    // action for a native input, after the field reports its `change`.
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
        cx.defer_in(window, |otp, window, cx| {
            submit_implicitly(otp.internals.form().as_ref(), window, cx)
        });
    }

    /// The `root` part, which holds the slots and the field over them.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<OtpInputState> {
        let otp = this.read(cx);
        Part::new(&otp.instance, "root", None, otp.state()).relative()
    }

    /// The `control` part: the real input, which takes focus, holds the code
    /// and submits it. It's invisible and fills the root, over every slot,
    /// so a press anywhere focuses it; render it after the slots.
    pub fn control(this: &Entity<Self>, cx: &App) -> Part<OtpInputState> {
        let otp = this.read(cx);
        let field = otp.field.read(cx);
        let disabled = field.is_disabled();
        let own = Aria::new()
            .disabled(disabled)
            .read_only(field.is_read_only())
            .required(field.is_required());
        let focus = field.focus_handle(cx);
        let internals = &otp.internals;
        let (el, _) = TextField::root(&otp.field, cx).into_element_parts();
        let (keys, presses, taps) = (this.downgrade(), this.downgrade(), this.downgrade());
        let part = Part::new(&otp.instance, "control", None, otp.state())
            .map_el(move |_| internals.track(el))
            .aria_role(Role::Textbox)
            .focus_mode(FocusMode::Native, Some(&focus), disabled)
            .aria(own)
            .aria(internals.aria())
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .m_0()
            .p_0()
            .border_0()
            .on_key_down(move |event, window, cx| {
                keys.update(cx, |otp, cx| otp.keydown(event, window, cx)).ok();
            })
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                presses
                    .update(cx, |otp, cx| otp.press_slot(event.position, true, window, cx))
                    .ok();
            })
            .on_click(move |event, window, cx| {
                if let ClickEvent::Touch(_) = event {
                    taps.update(cx, |otp, cx| otp.press_slot(event.position(), false, window, cx))
                        .ok();
                }
            });
        match (otp.settings.text_align, is_rtl(cx)) {
            (OtpInputTextAlign::Center, _) => part.text_center(),
            (OtpInputTextAlign::Start, false) | (OtpInputTextAlign::End, true) => part.text_left(),
            (OtpInputTextAlign::Start, true) | (OtpInputTextAlign::End, false) => part.text_right(),
        }
    }

    /// A `group` part, which lays out a run of slots. Like the slots, it's
    /// only drawn: the field is what's announced. `index` tells groups apart.
    pub fn group(this: &Entity<Self>, index: usize, cx: &App) -> Part<OtpInputState> {
        let otp = this.read(cx);
        Part::new(&otp.instance, "group", Some(&index.to_string()), otp.state())
            .map_el(|el| with_role(el, AccessRole::Group))
            .aria(Aria::new().hidden(true))
    }

    /// The `slot` part for character `index` of the code, from 0: one
    /// character's box. Keep it `relative`, so a caret drawn in it stays
    /// inside it.
    pub fn slot(this: &Entity<Self>, index: usize, cx: &App) -> Part<OtpInputSlotState> {
        let otp = this.read(cx);
        let data = otp.slots().get(index).copied().unwrap_or_default();
        let state = OtpInputSlotState {
            char: data.char,
            active: data.active,
            has_fake_caret: data.has_fake_caret,
            index,
            focused: otp.focused,
            disabled: otp.is_disabled(),
        };
        Part::new(&otp.instance, "slot", Some(&index.to_string()), state)
            .map_el(|el| with_role(el, AccessRole::Group))
            .aria(Aria::new().hidden(true))
    }

    /// A `separator` part, drawn between groups like a dash. `index` tells
    /// separators apart.
    pub fn separator(this: &Entity<Self>, index: usize, cx: &App) -> Part<OtpInputState> {
        let otp = this.read(cx);
        Part::new(&otp.instance, "separator", Some(&index.to_string()), otp.state())
            .map_el(|el| with_role(el, AccessRole::Group))
            .aria(Aria::new().hidden(true))
    }
}

impl Focusable for OtpInput {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.read(cx).focus_handle(cx)
    }
}

/// `agent: { actions: ["setValue", "clear"], secret: true }`, with `value`
/// controlled.
impl Agent for OtpInput {
    const SURFACE: AgentSurface = AgentSurface::new().secret(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |otp, args, window, cx| {
            let value = args.string(0)?;
            Ok(otp.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |otp, _args, window, cx| {
            Ok(otp.clear(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", self.value.get().to_string())
            .with("disabled", self.is_disabled())
            .with("readOnly", self.settings.read_only)
            .with("required", self.settings.required)
    }
}
