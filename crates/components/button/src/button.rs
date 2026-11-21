//! Port of `components/button/button.ts`: a button that runs an action,
//! shows that it's working, and stays reachable when disabled.
//!
//! It stays pending while an action's work goes on, so a form can't submit
//! twice, says so to screen readers once a wait is worth mentioning, can stay
//! focusable while disabled, and acts the same when it renders as a link.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, KeyboardButton, SharedString, Window,
};
use ui_sdk_forms::{ButtonType, ElementInternals, InternalsOptions, activate_button, bind_internals};
use ui_sdk_kernel::action::ActionHandlers;
use ui_sdk_kernel::change::log_with;
use ui_sdk_kernel::{
    ActionDetails, ActionError, ActionOutcome, Agent, AgentState, AnnouncePending, ChangeSource,
    FocusMode, InputEvent, Instance, Part, Reason, change_source, track_input,
};
use ui_sdk_platform::{
    ButtonElement, ButtonElementOptions, announce, button_element_attrs, button_role,
};

/// What a Button's action receives after the event: how the button was
/// pressed.
pub type ButtonActionDetails = ActionDetails;

/// What a press runs: the event behind it, when there is one, and how the
/// button was pressed. It runs inside the button's update, so it may update
/// any entity but the button. Return [`ActionOutcome::wait`] with work that
/// goes on to keep the button pending until it settles, and
/// [`ActionOutcome::Canceled`] to stop what the press would do next, like
/// submitting the form.
pub type ButtonAction =
    Rc<dyn Fn(Option<&InputEvent>, &ButtonActionDetails, &mut Window, &mut App) -> ActionOutcome>;

/// The options `Button` takes.
#[derive(Clone)]
pub struct ButtonOptions {
    /// Stops presses. The button leaves the tab order and its form doesn't
    /// submit through it, unless `focusable_when_disabled` is set.
    pub disabled: bool,
    /// Keeps a disabled button in the tab order, reporting it disabled, so it
    /// can hold focus while it becomes disabled and host a tooltip that says
    /// why. Presses still do nothing.
    pub focusable_when_disabled: bool,
    /// What a native button does in a form: `Submit` submits it and `Reset`
    /// resets it. `Button`, the default, does neither. Ignored when the button
    /// renders as something else.
    pub r#type: ButtonType,
    /// What screen readers hear once the button has been pending for a
    /// second, so a slow action doesn't pass in silence. Set it in the app's
    /// language. An empty one says nothing. `None` says "Loading".
    pub pending_announcement: Option<SharedString>,
    /// What a press runs (`onClick`).
    pub action: Option<ButtonAction>,
    /// What the part renders as: a native button, the default, a link, or
    /// another element that acts as a button, which `button_element` picks
    /// from whether it has somewhere to go. The original reads it off the
    /// element it renders.
    pub element: ButtonElement,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for ButtonOptions {
    fn default() -> Self {
        Self {
            disabled: false,
            focusable_when_disabled: false,
            r#type: ButtonType::Button,
            pending_announcement: None,
            action: None,
            element: ButtonElement::Button,
            debug: false,
        }
    }
}

impl ButtonOptions {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn focusable_when_disabled(mut self, focusable: bool) -> Self {
        self.focusable_when_disabled = focusable;
        self
    }

    pub fn r#type(mut self, button_type: ButtonType) -> Self {
        self.r#type = button_type;
        self
    }

    pub fn pending_announcement(mut self, announcement: impl Into<SharedString>) -> Self {
        self.pending_announcement = Some(announcement.into());
        self
    }

    pub fn action(
        mut self,
        action: impl Fn(Option<&InputEvent>, &ButtonActionDetails, &mut Window, &mut App) -> ActionOutcome
        + 'static,
    ) -> Self {
        self.action = Some(Rc::new(action));
        self
    }

    pub fn element(mut self, element: ButtonElement) -> Self {
        self.element = element;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ButtonState {
    /// Whether it's disabled, by the `disabled` option or a disabled
    /// fieldset around it. A running press shows in `pending` instead.
    pub disabled: bool,
    /// Whether the work the last press started is still going on. Presses do
    /// nothing meanwhile.
    pub pending: bool,
    /// Why the last press failed: what its action failed with, at once or
    /// once its work settled. Cleared by the next press.
    pub error: Option<ActionError>,
}

/// A button. Its entity is its controller.
pub struct Button {
    instance: Instance,
    disabled: bool,
    focusable_when_disabled: bool,
    button_type: ButtonType,
    pending_announcement: Option<SharedString>,
    action: Option<ButtonAction>,
    // What the part renders as.
    element: ButtonElement,
    pending: bool,
    error: Option<ActionError>,
    internals: ElementInternals,
    focus_handle: FocusHandle,
    announce_pending: AnnouncePending,
}

impl Button {
    pub fn new(options: ButtonOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Tracks input from the start, so the first press already knows what
        // made it.
        track_input(window, cx);
        let mut instance = Instance::new("button").with_agent(cx);
        instance.debug = options.debug;
        let internals = ElementInternals::new(cx);
        let focus_handle = cx.focus_handle();
        internals.set_focus_handle(Some(focus_handle.clone()));
        // A label presses a plain button it names, as a native label clicks it.
        internals.on_activate(cx, |button: &mut Self, event, window, cx| {
            button.press(event, window, cx)
        });
        internals.set_control(options.element == ButtonElement::Button, cx);
        let button = Self {
            instance,
            disabled: options.disabled,
            focusable_when_disabled: options.focusable_when_disabled,
            button_type: options.r#type,
            pending_announcement: options.pending_announcement,
            action: options.action,
            element: options.element,
            pending: false,
            error: None,
            internals,
            focus_handle,
            announce_pending: AnnouncePending::new(),
        };
        button.sync_form();
        button
    }

    /// Hands it new options, as a framework re-renders with new props (`sync`).
    pub fn set_options(&mut self, options: ButtonOptions, cx: &mut Context<Self>) {
        let changed = self.disabled != options.disabled
            || self.focusable_when_disabled != options.focusable_when_disabled
            || self.button_type != options.r#type
            || self.element != options.element;
        if self.element != options.element {
            self.internals
                .set_control(options.element == ButtonElement::Button, cx);
        }
        self.disabled = options.disabled;
        self.focusable_when_disabled = options.focusable_when_disabled;
        self.button_type = options.r#type;
        self.element = options.element;
        self.pending_announcement = options.pending_announcement;
        self.action = options.action;
        self.instance.debug = options.debug;
        self.sync_form();
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// Its part in forms, which a submit or reset button acts on.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    pub fn state(&self) -> ButtonState {
        ButtonState {
            disabled: self.is_disabled(),
            pending: self.pending,
            error: self.error.clone(),
        }
    }

    /// Whether it's disabled: by its `disabled` option or a disabled
    /// fieldset around it.
    pub fn is_disabled(&self) -> bool {
        self.disabled || self.internals.form_disabled()
    }

    /// Whether the work its last press started is still going on.
    pub fn is_pending(&self) -> bool {
        self.pending
    }

    /// Why its last press failed, until the next press.
    pub fn error(&self) -> Option<&ActionError> {
        self.error.as_ref()
    }

    /// Moves focus to it.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus_handle, cx);
    }

    /// A press: runs the action, unless the button is disabled or still
    /// pending, when the press is refused, which keeps a second submit from
    /// happening and a disabled link from going anywhere. An action whose
    /// work goes on makes the button pending until it settles. Then, unless
    /// the action canceled the press, a submit button submits its form and a
    /// reset button resets it, as a native button does after a click nothing
    /// prevented.
    pub fn press(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_disabled() || self.pending {
            return;
        }
        let source = change_source(Reason::Action, event.as_ref());
        self.write_error(None, source);
        let action = self.action.clone();
        let canceled = Rc::new(Cell::new(false));
        let heard = canceled.clone();
        let pressed = event.clone();
        let run = move |details: &ActionDetails, window: &mut Window, cx: &mut App| {
            let outcome = match &action {
                Some(action) => action(pressed.as_ref(), details, window, cx),
                None => ActionOutcome::Done,
            };
            if matches!(outcome, ActionOutcome::Canceled) {
                heard.set(true);
            }
            outcome
        };
        run_action(self, &run, source, window, cx);
        if !canceled.get() && self.element == ButtonElement::Button {
            activate_button(&self.internals, event, window, cx);
        }
        cx.notify();
    }

    // Through `log`, so `debug` and the watchers see the action's state.
    fn write_pending(&mut self, pending: bool, source: ChangeSource, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending == pending {
            return;
        }
        self.pending = pending;
        log_with(&self.instance, "pending", &pending, Reason::Action, Some(source));
        self.announce_pending
            .update(pending, self.pending_announcement.clone(), announce, window, cx);
        cx.notify();
    }

    fn write_error(&mut self, error: Option<ActionError>, source: ChangeSource) {
        if self.error == error {
            return;
        }
        log_with(&self.instance, "error", &error, Reason::Action, Some(source));
        self.error = error;
    }

    // Reports it to its form, as a native button takes part in one: a
    // submit or reset button, which submits or resets it, or a plain one,
    // which does neither. A link or another element takes no part.
    fn sync_form(&self) {
        let native = self.element == ButtonElement::Button;
        self.internals.set_button(native.then_some(self.button_type));
        self.internals.set_labelable(native);
        bind_internals(
            &self.internals,
            InternalsOptions {
                disabled: self.disabled,
                ..Default::default()
            },
        );
    }

    /// The `root` part: the button itself, or the link or element it renders
    /// as. A press, Enter or Space runs it; a link answers Enter alone, as a
    /// native one does.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<ButtonState> {
        let button = this.read(cx);
        button.sync_form();
        let state = button.state();
        let attrs = button_element_attrs(
            button.element,
            ButtonElementOptions {
                button_type: None,
                disabled: state.disabled,
                focusable: button.focusable_when_disabled,
                tab_index: None,
            },
        );
        let unfocusable = attrs.disabled || attrs.tab_index == Some(-1);
        // Presses do nothing while it's pending, so screen readers hear it's
        // unavailable, while it keeps focus.
        let aria = button
            .internals
            .aria()
            .disabled(attrs.disabled || attrs.aria_disabled || (state.pending && !state.disabled))
            .busy(state.pending);
        let link = button.element == ButtonElement::Link;
        let entity = this.downgrade();
        Part::new(&button.instance, "root", None, state)
            .aria_role(button_role(button.element))
            .focus_mode(FocusMode::Native, Some(&button.focus_handle), unfocusable)
            .map_el(|el| button.internals.track(el))
            .aria(aria)
            .on_press(move |event, window, cx| {
                if link
                    && let InputEvent::Click(ClickEvent::Keyboard(click)) = &event
                    && click.button == KeyboardButton::Space
                {
                    return;
                }
                entity
                    .update(cx, |button, cx| button.press(Some(event), window, cx))
                    .ok();
            })
    }
}

// Runs the action with the handlers that write the button's state.
fn run_action(
    button: &mut Button,
    action: &dyn Fn(&ActionDetails, &mut Window, &mut App) -> ActionOutcome,
    source: ChangeSource,
    window: &mut Window,
    cx: &mut Context<Button>,
) {
    ui_sdk_kernel::run_action(
        button,
        action,
        source,
        ActionHandlers {
            pending: Box::new(move |button, pending, window, cx| {
                button.write_pending(pending, source, window, cx)
            }),
            done: Box::new(|_, _, _| {}),
            failed: Box::new(move |button, error, _window, cx| {
                button.write_error(Some(error), source);
                cx.notify();
            }),
        },
        window,
        cx,
    );
}

impl Focusable for Button {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// No `agent` key and no api method named like a change: the bridge reads
/// whether it's disabled, its running press and why the last one failed.
impl Agent for Button {
    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("disabled", self.is_disabled())
            .with("pending", self.pending)
            .with("error", self.error.as_ref().map(|error| error.to_string()))
    }
}
