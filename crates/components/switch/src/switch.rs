//! Port of `components/switch/switch.ts`: turns a setting on or off.
//!
//! A switch is a checkbox that is never indeterminate, with the `switch`
//! role and a thumb that slides. The forms crate's [`ToggleControl`] holds
//! its checked state and its form side, and builds its root.

use std::rc::Rc;

use gpui::{App, Context, Entity, FocusHandle, Focusable, SharedString, Window};
use ui_sdk_forms::{
    CheckboxChangeOptions, CheckboxCheckedChangeReason, ElementInternals, Form, ToggleControl,
    ToggleHost, ToggleOptions, ToggleRole, switch_state,
};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, ChangeDetails, ChangeOptions, Checked, Instance, Part,
};

/// Why the switch's checked state wants to change, as `reason` in its change
/// details: `Imperative`, `Reset` or `Press`.
pub type SwitchCheckedChangeReason = CheckboxCheckedChangeReason;

/// `on_checked_change`: the requested state and what explains it.
pub type SwitchCheckedChange = Rc<dyn Fn(&bool, &ChangeDetails, &mut Window, &mut App)>;

/// The options `Switch` takes.
#[derive(Clone, Default)]
pub struct SwitchOptions {
    /// Whether it's on. Pass it to control the switch.
    pub checked: Option<bool>,
    /// Whether it's on at first, when uncontrolled. A form reset returns to
    /// it; without it, a reset leaves a controlled switch as it is.
    pub default_checked: Option<bool>,
    /// Runs when a user, a form reset or a call wants to turn it on or off.
    /// `details.reason` is `Press`, `Reset` or `Imperative`, and
    /// `details.cancel()` stops it. It runs inside the switch's update, so it
    /// may update any entity but the switch.
    pub on_checked_change: Option<SwitchCheckedChange>,
    /// Disables it like a native control: it can't be focused or changed,
    /// and isn't submitted.
    pub disabled: bool,
    /// Keeps it focusable and submitted, but stops changes.
    pub read_only: bool,
    /// Blocks form submission until it's on, with the browser's own message.
    pub required: bool,
    /// The name it submits under. Without one, it submits nothing.
    pub name: Option<SharedString>,
    /// What it submits while on, `"on"` by default.
    pub value: Option<SharedString>,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
    /// Its id, which a label's `for_id` names, the element's `id` in the
    /// original.
    pub id: Option<SharedString>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl SwitchOptions {
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    pub fn default_checked(mut self, checked: bool) -> Self {
        self.default_checked = Some(checked);
        self
    }

    pub fn on_checked_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_checked_change = Some(Rc::new(callback));
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

    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn form(mut self, form: impl Into<SharedString>) -> Self {
        self.form = Some(form.into());
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root part's `with` and the app read: whether it's on, disabled,
/// read-only and required.
pub type SwitchState = ui_sdk_forms::SwitchState;

/// What the thumb part's `with` reads: the switch's state.
pub type SwitchThumbState = SwitchState;

// The control's options: a switch's booleans as checked states.
fn toggle_options(options: &SwitchOptions) -> ToggleOptions {
    let on_checked_change = options.on_checked_change.clone().map(|callback| {
        Rc::new(
            move |checked: &Checked, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
                callback(&checked.is_checked(), details, window, cx)
            },
        ) as ui_sdk_kernel::ChangeCallback<Checked>
    });
    ToggleOptions {
        checked: options.checked.map(Checked::from),
        default_checked: options.default_checked.map(Checked::from),
        on_checked_change,
        disabled: options.disabled,
        read_only: options.read_only,
        required: options.required,
        name: options.name.clone(),
        value: options.value.clone(),
        form: options.form.clone(),
    }
}

/// A switch. Its entity is its controller.
pub struct Switch {
    control: ToggleControl,
    options: SwitchOptions,
}

impl ToggleHost for Switch {
    fn toggle_control(&self) -> &ToggleControl {
        &self.control
    }

    fn toggle_control_mut(&mut self) -> &mut ToggleControl {
        &mut self.control
    }
}

impl Switch {
    pub fn new(options: SwitchOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("switch").with_agent(cx);
        instance.debug = options.debug;
        let control = ToggleControl::new(instance, toggle_options(&options), cx);
        control.internals().set_id(options.id.clone(), cx);
        Self { control, options }
    }

    /// Hands it new options, as a framework re-renders with new props (`sync`).
    pub fn set_options(&mut self, options: SwitchOptions, cx: &mut Context<Self>) {
        self.control.instance_mut().debug = options.debug;
        self.control.internals().set_id(options.id.clone(), cx);
        let changed = self.control.set_options(toggle_options(&options));
        self.options = options;
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled state, or `None` to let the switch keep its own.
    pub fn set_controlled_checked(&mut self, checked: Option<bool>, cx: &mut Context<Self>) {
        self.options.checked = checked;
        if self.control.set_controlled_checked(checked.map(Checked::from)) {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        self.control.instance()
    }

    /// Its part in forms.
    pub fn internals(&self) -> &ElementInternals {
        self.control.internals()
    }

    pub fn state(&self) -> SwitchState {
        switch_state(&self.control)
    }

    /// Whether it's on (`getChecked`).
    pub fn is_checked(&self) -> bool {
        self.control.get_checked().is_checked()
    }

    /// Whether it's disabled, by itself, its field or a fieldset.
    pub fn is_disabled(&self) -> bool {
        self.control.is_disabled()
    }

    /// Turns it on or off, as a user would. A disabled or read-only switch
    /// ignores it. Returns false when nothing changed or the change was
    /// canceled.
    pub fn set_checked(
        &mut self,
        checked: bool,
        options: CheckboxChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.control
            .set_checked(Checked::from(checked), options, window, cx)
    }

    /// Turns it on, or off.
    pub fn toggle(&mut self, options: CheckboxChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.control.toggle(options, window, cx)
    }

    /// Moves focus to it.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.control.focus(window, cx);
    }

    /// The form it belongs to.
    pub fn form(&self, cx: &App) -> Option<Form> {
        self.control.form(cx)
    }

    /// The `root` part: the switch itself, which a press, Space or a label
    /// press turns on or off, and which Enter submits its form from, never
    /// turning it.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<SwitchState> {
        ToggleControl::root(this, ToggleRole::Switch, switch_state, cx)
    }

    /// The `thumb` part: the part that slides, with the switch's state.
    pub fn thumb(this: &Entity<Self>, cx: &App) -> Part<SwitchThumbState> {
        let switch = this.read(cx);
        Part::new(switch.control.instance(), "thumb", None, switch.state())
    }
}

impl Focusable for Switch {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.control.focus_handle().clone()
    }
}

/// `agent: { actions: ["setChecked"] }` and `controlled: ["checked"]`, with
/// `toggle`, which `setChecked` covers, unlisted.
impl Agent for Switch {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setChecked", |switch, args, window, cx| {
            let checked = args.boolean(0)?;
            Ok(switch
                .set_checked(checked, ChangeOptions::default(), window, cx)
                .into())
        }),
        AgentAction::unlisted("toggle", |switch, _args, window, cx| {
            Ok(switch.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["checked"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        let state = self.state();
        AgentState::new()
            .with("checked", state.checked)
            .with("disabled", state.disabled)
            .with("readOnly", state.read_only)
            .with("required", state.required)
    }
}
