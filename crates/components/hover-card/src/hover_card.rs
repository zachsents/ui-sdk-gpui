//! Port of `components/hover-card/hover-card.ts`: a preview of what a link
//! points to, opened when the pointer or keyboard focus rests on the link.
//!
//! A hover card is `create_popup` with `hover_card_preset`, and nothing of
//! its own.

use std::fmt::Debug;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, Context, Entity, Pixels, SharedString, Window};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, ChangeCallback, ChangeDetails, ChangeOptions, ConnectedController,
    Instance, PENDING, Part, Reason,
};
use ui_sdk_motion::PresenceTiming;
use ui_sdk_platform::{Align, Anchor, PositionProps, Side};
use ui_sdk_popups::{
    CreatePopupOptions, OpenChangeCompleteCallback, Popup, PopupArrowState, PopupCloseOptions, PopupContent,
    PopupContentProps, PopupContentState, PopupController, PopupHost, PopupOpenChangeCallback,
    PopupOpenOptions, PopupOptions, PopupPending, PopupTriggerProps, PopupTriggerState, create_popup,
    create_popup_controller, hover_card_preset, popup_arrow, popup_content, popup_trigger,
};

/// Why a hover card opened or closed: `TriggerHover`, `TriggerFocus`,
/// `TriggerPress`, `FocusOut`, `EscapeKey`, `ClosePress`, `OutsidePress`,
/// `WindowBlur`, `Back` or `Imperative`.
pub type HoverCardOpenChangeReason = Reason;

/// Why the card's payload changed: a trigger opened it (`TriggerHover`,
/// `TriggerFocus`, `TriggerPress`), another trigger took it over while open,
/// or a controller opened it or gave the open one another payload
/// (`Imperative`).
pub type HoverCardPayloadChangeReason = Reason;

/// Opens, closes and inspects a hover card from outside it, before it's made
/// or without its entity at hand. Pass it as the `controller` option; its
/// methods are `PopupControllerExt`'s. `P` is the type of the data triggers
/// and `open()` pass to the content.
pub type HoverCardController<P = ()> = PopupController<HoverCard<P>>;

/// A controller for a hover card (`createHoverCardController`).
pub fn create_hover_card_controller<P: Clone + PartialEq + Debug + 'static>() -> HoverCardController<P> {
    create_popup_controller()
}

/// The options `HoverCard` takes.
pub struct HoverCardOptions<P: Clone + PartialEq + Debug + 'static = ()> {
    /// Whether the card is open. Pass it to control the card.
    pub open: Option<bool>,
    /// Whether the card starts open, when uncontrolled. Defaults to false.
    pub default_open: bool,
    /// Runs when the card wants to open or close. `details.reason` says why
    /// (see [`HoverCardOpenChangeReason`]), and `details.cancel()` stops it.
    /// It runs inside the card's update, so it may update any entity but the
    /// card.
    pub on_open_change: Option<PopupOpenChangeCallback>,
    /// Runs once the card has finished opening or closing, after any
    /// animation.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Runs when the payload the content shows changes: a trigger with
    /// another payload opens the card, or takes the open card over, which
    /// fires no `on_open_change`, and so does a controller's `open(payload)`
    /// on the open card. `details.cancel()` keeps the previous payload.
    pub on_payload_change: Option<ChangeCallback<Option<P>>>,
    /// How long the pointer rests on the trigger, or keyboard focus stays on
    /// it, before the card opens. Defaults to 600 milliseconds.
    pub delay: Option<Duration>,
    /// How long before the card closes once the pointer leaves the trigger
    /// and the card. Defaults to 300 milliseconds.
    pub close_delay: Option<Duration>,
    /// Stops the card from opening. The trigger keeps working as a link.
    /// Defaults to false.
    pub disabled: bool,
    /// Opens, closes and inspects the card from anywhere. Create one with
    /// [`create_hover_card_controller`].
    pub controller: Option<HoverCardController<P>>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl<P: Clone + PartialEq + Debug + 'static> Default for HoverCardOptions<P> {
    fn default() -> Self {
        Self {
            open: None,
            default_open: false,
            on_open_change: None,
            on_open_change_complete: None,
            on_payload_change: None,
            delay: None,
            close_delay: None,
            disabled: false,
            controller: None,
            debug: false,
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Clone for HoverCardOptions<P> {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            delay: self.delay,
            close_delay: self.close_delay,
            disabled: self.disabled,
            controller: self.controller.clone(),
            debug: self.debug,
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> HoverCardOptions<P> {
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
        self.on_open_change = Some(Rc::new(move |open, details, _, window, cx| {
            callback(open, details, window, cx)
        }));
        self
    }

    pub fn on_open_change_complete(
        mut self,
        callback: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn on_payload_change(
        mut self,
        callback: impl Fn(&Option<P>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_payload_change = Some(Rc::new(callback));
        self
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    pub fn close_delay(mut self, delay: Duration) -> Self {
        self.close_delay = Some(delay);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn controller(mut self, controller: HoverCardController<P>) -> Self {
        self.controller = Some(controller);
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    // What the popup reads of them.
    fn popup(&self) -> PopupOptions<P> {
        PopupOptions {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            delay: self.delay,
            close_delay: self.close_delay,
            disabled: self.disabled,
            ..PopupOptions::default()
        }
    }
}

/// The props the `trigger` slot of `HoverCard` takes.
#[derive(Clone, Debug, PartialEq)]
pub struct HoverCardTriggerProps<P = ()> {
    /// Data for the content to render when this trigger opens the card, like
    /// the profile it links to.
    pub payload: Option<P>,
}

impl<P> Default for HoverCardTriggerProps<P> {
    fn default() -> Self {
        Self { payload: None }
    }
}

impl<P> HoverCardTriggerProps<P> {
    pub fn payload(mut self, payload: P) -> Self {
        self.payload = Some(payload);
        self
    }
}

/// The props the `content` slot of `HoverCard` takes: where it goes beside
/// its trigger, whether it stays in the window while closed, and how it
/// animates.
#[derive(Clone, Default)]
pub struct HoverCardContentProps {
    /// The side defaults to the bottom, the alignment to the center, the
    /// side offset to 4 and the align offset to 0.
    pub position: PositionProps,
    /// Whether the card stays in the window, hidden, while closed. Its
    /// content keeps its state between opens. Defaults to false.
    pub keep_mounted: bool,
    /// How it animates in and out, the transitions its CSS would declare.
    pub timing: Option<PresenceTiming>,
}

impl HoverCardContentProps {
    pub fn side(mut self, side: Side) -> Self {
        self.position.side = Some(side);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.position.align = Some(align);
        self
    }

    pub fn side_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.position.side_offset = Some(offset.into());
        self
    }

    pub fn align_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.position.align_offset = Some(offset.into());
        self
    }

    pub fn collision_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.position.collision_padding = Some(padding.into());
        self
    }

    pub fn arrow_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.position.arrow_padding = Some(padding.into());
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.position.anchor = Some(anchor.into());
        self
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = Some(timing);
        self
    }
}

impl From<HoverCardContentProps> for PopupContentProps {
    fn from(props: HoverCardContentProps) -> Self {
        PopupContentProps {
            position: props.position,
            keep_mounted: props.keep_mounted,
            timing: props.timing,
            ..PopupContentProps::default()
        }
    }
}

/// What the `trigger` part styles from: whether the card is open from this
/// trigger. A hover card's trigger is never disabled.
pub type HoverCardTriggerState = PopupTriggerState;

/// What the `content` part styles from: whether the card is open (false
/// while it animates out), the payload of the last open, and its presence
/// and placement. A hover card is never modal and always presents as a
/// popover.
pub type HoverCardContentState<P = ()> = PopupContentState<P>;

/// What the `arrow` part styles from: whether the card is open (false as
/// soon as it starts closing), the side it opened on, and where the arrow
/// goes.
pub type HoverCardArrowState = PopupArrowState;

/// A hover card. Its entity is its controller.
pub struct HoverCard<P: Clone + PartialEq + Debug + 'static = ()> {
    instance: Instance,
    popup: Popup<Self>,
    disabled: bool,
    controller: ConnectedController<Self>,
}

impl<P: Clone + PartialEq + Debug + 'static> PopupHost for HoverCard<P> {
    type Payload = P;

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

impl<P: Clone + PartialEq + Debug + 'static> HoverCard<P> {
    pub fn new(options: HoverCardOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("hover-card").with_agent(cx);
        instance.debug = options.debug;
        let popup = create_popup(
            &instance,
            CreatePopupOptions::new(hover_card_preset()).options(options.popup()),
            window,
            cx,
        );
        Self {
            instance,
            popup,
            disabled: options.disabled,
            controller: ConnectedController::new(options.controller, cx),
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: HoverCardOptions<P>, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        self.disabled = options.disabled;
        self.controller.set(options.controller.clone());
        let popup = self.popup.clone();
        popup.set_options(options.popup(), cx);
    }

    /// Passes the controlled `open`, or `None` to let the card keep its own.
    pub fn set_controlled_open(&mut self, open: Option<bool>, cx: &mut Context<Self>) {
        let popup = self.popup.clone();
        popup.set_controlled_open(open, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The popup it's built on.
    pub fn popup(&self) -> &Popup<Self> {
        &self.popup
    }

    /// Whether it's open. False as soon as it starts closing (`isOpen`).
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// The payload of the last open (`getPayload`).
    pub fn payload(&self) -> Option<P> {
        self.popup.payload()
    }

    /// Opens or closes it (`setOpen`).
    pub fn set_open(
        &mut self,
        open: bool,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let popup = self.popup.clone();
        popup.set_open(self, open, options, window, cx)
    }

    /// Opens it, and resolves once it closes (`open`). On an open card, a
    /// payload replaces the current one and an anchor moves it.
    pub fn open(
        &mut self,
        options: PopupOpenOptions<P>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupPending {
        let popup = self.popup.clone();
        popup.open(self, options, window, cx)
    }

    /// Closes it (`close`).
    pub fn close(&mut self, options: PopupCloseOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.close(self, options, window, cx)
    }

    /// Opens it when it's closed, and closes it when it's open (`toggle`).
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.toggle(self, options, window, cx)
    }

    /// Moves focus to the trigger that opened it last, or the first trigger
    /// (`focus`).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.popup.focus(window, cx);
    }

    /// The `trigger` part: a link, usually. Pressing it follows the link, so
    /// only resting the pointer and keyboard focus open the card. It keeps
    /// the role the app gives it, like `Role::Link` with the press that
    /// follows it.
    pub fn trigger(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: HoverCardTriggerProps<P>,
        cx: &App,
    ) -> Part<HoverCardTriggerState> {
        let props = PopupTriggerProps {
            payload: props.payload,
            ..PopupTriggerProps::default()
        };
        popup_trigger(this, key, props, cx)
    }

    /// The `content` part: the card, placed beside its trigger. Tab moves
    /// into it from the trigger, and focus leaving closes it. `None` while
    /// it's out of the window. It renders above the window.
    pub fn content(
        this: &Entity<Self>,
        props: HoverCardContentProps,
        cx: &App,
    ) -> Option<PopupContent<Self, HoverCardContentState<P>>> {
        popup_content(this, props.into(), cx)
    }

    /// The `arrow` part, which points at the trigger from the side the card
    /// opened on.
    pub fn arrow(this: &Entity<Self>, cx: &App) -> Part<HoverCardArrowState> {
        popup_arrow(this, cx)
    }
}

/// `agent: { actions: ["setOpen"] }`, with `open`, `close` and `toggle`,
/// which `setOpen` covers, unlisted.
impl<P: Clone + PartialEq + Debug + 'static> Agent for HoverCard<P> {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", |card, args, window, cx| {
            let open = args.boolean(0)?;
            Ok(card.set_open(open, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("open", |card, _args, window, cx| {
            let _pending = card.open(PopupOpenOptions::default(), window, cx);
            Ok(PENDING.into())
        }),
        AgentAction::unlisted("close", |card, _args, window, cx| {
            Ok(card.close(PopupCloseOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |card, _args, window, cx| {
            Ok(card.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("open", self.is_open())
            .with("disabled", self.disabled)
    }
}
