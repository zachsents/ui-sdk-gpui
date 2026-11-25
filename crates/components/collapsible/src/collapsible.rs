//! Port of `components/collapsible/collapsible.ts`.
//!
//! A trigger that shows and hides one panel. The panel is the disclosure
//! crate's [`Panel`]: it stays in the page while it animates closed, and its
//! part exposes the phases and the size it animates through.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, StatefulInteractiveElement, Styled, Window,
};
use ui_sdk_disclosure::{
    Opening, Panel, PanelBinding, PanelState, PendingReason, create_pending_reason,
    focus_pressed_trigger, hidden_attrs,
};
use ui_sdk_forms::{Node, is_disabled, watch_form_disabled};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, Aria, ChangeCallback, ChangeDetails, ChangeOptions,
    Controllable, Instance, Part, Reason, after_update, held,
};
use ui_sdk_motion::PresenceTiming;

/// Why a collapsible opened or closed: its trigger was pressed
/// (`TriggerPress`), find in page matched text in its closed panel
/// (`FindInPage`), or a controller or api call changed it (`Imperative`).
pub type CollapsibleOpenChangeReason = Reason;

/// Options for every write method.
pub type CollapsibleChangeOptions = ChangeOptions;

/// `on_open_change_complete`: the panel finished opening or closing, with
/// whether it's open.
pub type OpenChangeCompleteCallback = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// The options `Collapsible` takes.
#[derive(Clone, Default)]
pub struct CollapsibleOptions {
    /// Whether the panel is open. Pass it to control the collapsible.
    pub open: Option<bool>,
    /// Whether the panel is open at first, when uncontrolled.
    pub default_open: bool,
    /// Runs when the panel opens or closes. `details.reason` is
    /// `TriggerPress`, `FindInPage` or `Imperative`. It runs once the
    /// collapsible's update is over, so it may update any entity, the
    /// collapsible too.
    pub on_open_change: Option<ChangeCallback<bool>>,
    /// Runs as the panel is asked to open or close, before it does, inside
    /// the collapsible's update: `details.cancel()` stops it. It may update
    /// any entity but the collapsible.
    pub on_before_open_change: Option<ChangeCallback<bool>>,
    /// Runs once the panel has finished opening or closing, after any
    /// animation, once the collapsible's update is over.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Stops the trigger from opening or closing the panel. The trigger stays
    /// focusable.
    pub disabled: bool,
    /// How the panel animates open and closed, as its CSS transition would
    /// declare. The default has none, so it shows and hides at once.
    pub panel_timing: PresenceTiming,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl CollapsibleOptions {
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    pub fn on_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn panel_timing(mut self, timing: PresenceTiming) -> Self {
        self.panel_timing = timing;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `panel` part takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CollapsiblePanelProps {
    /// Whether the panel stays in the page, hidden, while closed.
    pub keep_mounted: bool,
    /// Keeps the closed panel in the page, hidden, where the original keeps
    /// it as `hidden="until-found"` for the browser's find in page. gpui has
    /// none, so it keeps the panel as `keep_mounted` does.
    pub hidden_until_found: bool,
}

impl CollapsiblePanelProps {
    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn hidden_until_found(mut self, hidden_until_found: bool) -> Self {
        self.hidden_until_found = hidden_until_found;
        self
    }
}

/// What every part's `with` and the app read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollapsibleState {
    /// Whether the panel is open.
    pub open: bool,
    /// Whether the trigger ignores presses, by its own `disabled` or a
    /// disabled fieldset around it. The `open` option and the api can still
    /// open and close it.
    pub disabled: bool,
}

/// The state the `trigger` part hands `with`.
pub type CollapsibleTriggerState = CollapsibleState;

/// The state the `panel` part hands `with`: the collapsible's, with the
/// panel's presence and the size it animates through, which the original's
/// `data-starting-style`, `data-ending-style` and
/// `--collapsible-panel-height` carry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollapsiblePanelState {
    /// Whether the panel is open.
    pub open: bool,
    /// Whether the trigger ignores presses.
    pub disabled: bool,
    /// The panel's presence, and the height and width to give it now.
    pub panel: PanelState,
}

/// A collapsible. Its entity is its controller.
pub struct Collapsible {
    instance: Instance,
    open: Controllable<bool>,
    on_open_change_complete: Option<OpenChangeCompleteCallback>,
    disabled: bool,
    // The root's place in the page, which says whether a fieldset disables it.
    form: Node,
    // Shown while it animates out.
    exiting: bool,
    // Why the panel opened after mount, until it settles. None while it's open
    // from the first paint, so it doesn't animate.
    opened_by: Option<Reason>,
    reason: PendingReason<Reason>,
    // The open state the last change saw.
    previous: bool,
    panel: Panel,
    // The props the panel rendered with last, which decide whether it stays
    // in the page.
    panel_props: Cell<CollapsiblePanelProps>,
    trigger: FocusHandle,
}

impl Collapsible {
    pub fn new(options: CollapsibleOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut open = Controllable::new("open", options.default_open);
        open.set_controlled(options.open);
        open.set_on_change(options.on_open_change.clone());
        open.set_on_before_change(options.on_before_open_change.clone());
        let mut instance = Instance::new("collapsible").with_agent(cx);
        instance.debug = options.debug;
        let previous = *open.get();
        let mut this = Self {
            instance,
            open,
            on_open_change_complete: options.on_open_change_complete,
            disabled: options.disabled,
            form: watch_form_disabled(cx),
            exiting: false,
            opened_by: None,
            reason: create_pending_reason(Reason::Imperative),
            previous,
            panel: Panel::new(cx).with_timing(options.panel_timing),
            panel_props: Cell::new(CollapsiblePanelProps::default()),
            trigger: cx.focus_handle(),
        };
        this.sync_panel(window, cx);
        this
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A closing `open` starts the panel's exit in the same pass.
    pub fn set_options(&mut self, options: CollapsibleOptions, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = self.open.set_controlled(options.open);
        self.open.set_on_change(options.on_open_change);
        self.open.set_on_before_change(options.on_before_open_change);
        self.on_open_change_complete = options.on_open_change_complete;
        changed |= self.disabled != options.disabled;
        self.disabled = options.disabled;
        self.panel.set_timing(options.panel_timing);
        self.instance.debug = options.debug;
        self.after_change(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled `open`, or `None` to let the collapsible keep
    /// its own.
    pub fn set_controlled_open(&mut self, open: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.set_controlled(open) {
            cx.notify();
        }
        self.after_change(window, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// What every part's state starts from.
    pub fn state(&self) -> CollapsibleState {
        CollapsibleState {
            open: self.is_open(),
            disabled: self.is_disabled(),
        }
    }

    /// Whether the trigger ignores presses: by its own `disabled`, or a
    /// disabled fieldset around it.
    pub fn is_disabled(&self) -> bool {
        is_disabled(self.disabled, self.form.form_disabled())
    }

    /// Whether the panel is open.
    pub fn is_open(&self) -> bool {
        *self.open.get()
    }

    /// Opens or closes the panel. Returns false when the change was canceled
    /// or not possible.
    pub fn set_open(&mut self, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.is_open() == open {
            return false;
        }
        self.reason.set(options.reason.unwrap_or(Reason::Imperative));
        let changed = self.open.change(&self.instance, open, options, window, cx);
        if !changed {
            self.reason.take();
        }
        self.after_change(window, cx);
        changed
    }

    /// Opens the panel.
    pub fn open(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.set_open(true, options, window, cx)
    }

    /// Closes the panel.
    pub fn close(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.set_open(false, options, window, cx)
    }

    /// Opens the panel when it's closed, and closes it when it's open.
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let open = !self.is_open();
        self.set_open(open, options, window, cx)
    }

    /// Moves focus to the trigger.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.trigger, cx);
    }

    // Find in page shows its match at once, without an animation.
    fn opening(&self) -> Opening {
        match self.opened_by {
            None => Opening::Mount,
            Some(Reason::FindInPage) => Opening::Instant,
            Some(_) => Opening::Animate,
        }
    }

    // What the original's store subscription does: hears `open` change from
    // any source, a press, the api or a controlled value, before anything
    // renders the change.
    fn after_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.is_open();
        if open != self.previous {
            self.previous = open;
            // A later value from options, like a parent's own toggle, isn't
            // this change.
            let why = self.reason.take();
            self.opened_by = open.then_some(why);
            // A panel in the page animates out before it goes.
            self.exiting = !open && held(&self.instance, "panel", None);
            cx.notify();
        }
        self.sync_panel(window, cx);
    }

    // Binds, checks or unbinds the panel for the state now (`bindPanel`).
    fn sync_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let props = self.panel_props.get();
        let open = self.is_open();
        let present = open || self.exiting || props.keep_mounted || props.hidden_until_found;
        let binding = PanelBinding::default()
            .open(open)
            .exiting(self.exiting)
            .opening(self.opening())
            .trigger(Some(self.trigger.clone()))
            .settled(|this: &mut Self, open, window, cx| this.settled(open, window, cx))
            .exited(|this: &mut Self, window, cx| this.exited(window, cx));
        let panel = self.panel.clone();
        panel.sync(self, present, &binding, window, cx);
    }

    fn settled(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.opened_by = None;
        if let Some(callback) = self.on_open_change_complete.clone() {
            after_update(window, cx, move |window, cx| callback(open, window, cx));
        }
    }

    fn exited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.exiting = false;
        cx.notify();
        self.sync_panel(window, cx);
    }

    /// The `root` part, around the trigger and the panel.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<CollapsibleState> {
        let collapsible = this.read(cx);
        let form = collapsible.form.clone();
        Part::new(&collapsible.instance, "root", None, collapsible.state()).map_el(|el| form.track(el))
    }

    /// The `trigger` part: the button that opens and closes the panel. It
    /// says whether the panel is open, and points at it while it's in the
    /// page. Disabled, it stays in the tab order.
    pub fn trigger(this: &Entity<Self>, cx: &App) -> Part<CollapsibleTriggerState> {
        let collapsible = this.read(cx);
        let state = collapsible.state();
        let entity = this.downgrade();
        Part::new(&collapsible.instance, "trigger", None, state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::TabStop, Some(&collapsible.trigger), false)
            .aria_expanded(state.open)
            .aria(
                Aria::new()
                    .disabled(state.disabled)
                    .controls(collapsible.instance.element_id("panel", None)),
            )
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |collapsible, cx| {
                        if collapsible.is_disabled() {
                            return;
                        }
                        let trigger = collapsible.trigger.clone();
                        focus_pressed_trigger(Some(&trigger), window, cx);
                        collapsible.toggle(ChangeOptions::event_of(Reason::TriggerPress, event), window, cx);
                    })
                    .ok();
            })
    }

    /// The `panel` part: what the trigger shows and hides, or `None` while
    /// it's out of the page. It stays in the page while it animates closed,
    /// and while closed with `keep_mounted`, hidden. Style it from its state:
    /// give it `panel.height` and clip its overflow while it animates.
    pub fn panel(this: &Entity<Self>, props: CollapsiblePanelProps, cx: &App) -> Option<Part<CollapsiblePanelState>> {
        let collapsible = this.read(cx);
        // A render with new props, like `keep_mounted`, checks the panel
        // again (`stateAndRenders`).
        let stale = collapsible.panel_props.replace(props) != props;
        let open = collapsible.is_open();
        let present = open || collapsible.exiting || props.keep_mounted || props.hidden_until_found;
        if !present {
            return None;
        }
        let hidden = hidden_attrs(open || collapsible.exiting, props.hidden_until_found);
        let state = CollapsiblePanelState {
            open,
            disabled: collapsible.is_disabled(),
            panel: collapsible.panel.state(),
        };
        let mut part = Part::new(&collapsible.instance, "panel", None, state).aria(Aria::new().hidden(hidden));
        if hidden {
            part = part.hidden();
        }
        part = collapsible.panel.track(part);
        if stale {
            let entity = this.downgrade();
            part = part.on_props(move |window, cx| {
                entity.update(cx, |collapsible, cx| collapsible.sync_panel(window, cx)).ok();
            });
        }
        Some(part)
    }
}

impl Focusable for Collapsible {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.trigger.clone()
    }
}

/// `agent: { actions: ["setOpen"] }`, with `open`, `close` and `toggle`,
/// which `setOpen` covers, unlisted.
impl Agent for Collapsible {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", |collapsible, args, window, cx| {
            let open = args.boolean(0)?;
            Ok(collapsible.set_open(open, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("open", |collapsible, _args, window, cx| {
            Ok(collapsible.open(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("close", |collapsible, _args, window, cx| {
            Ok(collapsible.close(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |collapsible, _args, window, cx| {
            Ok(collapsible.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("open", self.is_open())
            .with("disabled", self.is_disabled())
    }
}
