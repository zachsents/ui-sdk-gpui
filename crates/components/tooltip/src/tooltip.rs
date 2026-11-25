//! Port of `components/tooltip/tooltip.ts`: names a control or adds a short
//! hint when the pointer rests on it or keyboard focus reaches it.
//!
//! A tooltip is `create_popup` with `tooltip_preset`, which brings the warm
//! group. What's its own: naming or describing its trigger, and a press that
//! closes it or, when toggleable, opens it and announces its text.

use std::cell::Cell;
use std::fmt::Debug;
use std::panic::Location;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    A11ySubtreeBuilder, AnyElement, App, Bounds, Context, Div, Element, ElementId, Entity, GlobalElementId,
    InspectorElementId, InteractiveElement, Interactivity, IntoElement, LayoutId, ParentElement, Pixels,
    SharedString, Stateful, StatefulInteractiveElement, StyleRefinement, Styled, WeakEntity, Window,
};
use ui_sdk_kernel::accesskit::{Node, NodeId};
use ui_sdk_kernel::part::node_id_of;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, Aria, ChangeCallback, ChangeDetails, ChangeOptions, ConnectedController,
    FocusMode, InputEvent, Instance, PENDING, Part, PartElement, Reason, track_input,
};
use ui_sdk_motion::{PresenceState, PresenceTiming};
use ui_sdk_platform::{
    Align, Anchor, Placement, PositionProps, Side, announce, aria_keyshortcuts, initial_apple, is_apple,
};
use ui_sdk_popups::{
    CreatePopupOptions, HideRequest, HoverEvent, OpenChangeCompleteCallback, Popup, PopupArrowState,
    PopupCloseOptions, PopupContent, PopupContentProps, PopupController, PopupHost, PopupOpenChangeCallback,
    PopupOpenOptions, PopupOpener, PopupOptions, PopupPending, PopupRules, PopupTriggerProps, ShowRequest,
    bind_popup_content, create_popup, create_popup_controller, opener_of, popup_arrow, popup_content_attrs,
    set_unfocusable, tooltip_preset,
};
use ui_sdk_shortcuts::{ShortcutPart, ShortcutProps, ShortcutState, adopt_platform, shortcut_slot};

/// Why a tooltip opened or closed: `TriggerHover`, `TriggerFocus`,
/// `TriggerPress`, `FocusOut`, `EscapeKey`, `ClosePress`, `OutsidePress`,
/// `WindowBlur`, `Back` or `Imperative`.
pub type TooltipOpenChangeReason = Reason;

/// Why the tooltip's payload changed: a trigger opened it (`TriggerHover`,
/// `TriggerFocus`, `TriggerPress`), another trigger took it over while open,
/// or a controller opened it or gave the open one another payload
/// (`Imperative`).
pub type TooltipPayloadChangeReason = Reason;

/// Opens, closes and inspects a tooltip from outside it, before it's made or
/// without its entity at hand, like a step in a guided tour. Pass it as the
/// `controller` option; its methods are `PopupControllerExt`'s. `P` is the
/// type of the data triggers and `open()` pass to the content.
pub type TooltipController<P = ()> = PopupController<Tooltip<P>>;

/// A controller for a tooltip (`createTooltipController`).
pub fn create_tooltip_controller<P: Clone + PartialEq + Debug + 'static>() -> TooltipController<P> {
    create_popup_controller()
}

/// Whether a tooltip names its trigger or adds a description to it (`type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TooltipType {
    Label,
    Description,
}

/// The options `Tooltip` takes.
pub struct TooltipOptions<P: Clone + PartialEq + Debug + 'static = ()> {
    /// Whether the tooltip is open. Pass it to control the tooltip.
    pub open: Option<bool>,
    /// Whether the tooltip starts open, when uncontrolled. Defaults to false.
    pub default_open: bool,
    /// Runs when the tooltip opens or closes. `details.reason` says why (see
    /// [`TooltipOpenChangeReason`]). It runs once the tooltip's update is
    /// over, so it may update any entity, the tooltip too.
    pub on_open_change: Option<PopupOpenChangeCallback>,
    /// Runs as the tooltip is asked to open or close, before it does, inside
    /// the tooltip's update: `details.cancel()` stops it. It may update any
    /// entity but the tooltip.
    pub on_before_open_change: Option<PopupOpenChangeCallback>,
    /// Runs once the tooltip has finished opening or closing, after any
    /// animation, once the tooltip's update is over.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Runs when the payload the content shows changes: a trigger with
    /// another payload opens the tooltip, or takes the open tooltip over,
    /// which fires no `on_open_change`, and so does a controller's
    /// `open(payload)` on the open tooltip. It runs once the tooltip's update
    /// is over.
    pub on_payload_change: Option<ChangeCallback<Option<P>>>,
    /// Runs as the payload is asked to change, before it does, inside the
    /// tooltip's update: `details.cancel()` keeps the previous payload. It
    /// may update any entity but the tooltip.
    pub on_before_payload_change: Option<ChangeCallback<Option<P>>>,
    /// How long the pointer rests on the trigger before the tooltip opens.
    /// While another tooltip is open, or was a moment ago, the next one
    /// opens at once. Defaults to 600 milliseconds.
    pub delay: Option<Duration>,
    /// How long before the tooltip closes once the pointer leaves the trigger
    /// and the tooltip. Defaults to none.
    pub close_delay: Option<Duration>,
    /// Stops the tooltip from opening. The trigger keeps working. Defaults to
    /// false.
    pub disabled: bool,
    /// Whether pressing the trigger opens and closes the tooltip, so touch
    /// screens can show it too. For info buttons whose only job is the tip.
    /// The text is announced when a press opens it. Defaults to false.
    pub toggleable: bool,
    /// Whether pressing the trigger closes the tooltip. Set it to false to
    /// keep it open, like for a copy button whose tooltip turns into
    /// "Copied". Defaults to true.
    pub close_on_press: bool,
    /// Whether the tooltip names its trigger (`Label`) or adds a description
    /// to it (`Description`) (`type`). By default it names a trigger that has
    /// no name of its own, like an icon button, and describes any other.
    pub kind: Option<TooltipType>,
    /// The keyboard shortcut of what the trigger does, like `"mod+s"`, as a
    /// hint. A Shortcut part shows it, and the trigger gets
    /// `aria-keyshortcuts`. It doesn't bind the keys: the trigger's own
    /// component, or `register_shortcut`, does.
    pub shortcut: Option<SharedString>,
    /// The tooltip's text as a screen reader hears it, without the parts
    /// hidden from it, like a Shortcut part's keys. gpui can't read text back
    /// from what the content renders, so this is what names or describes the
    /// trigger, even while the tooltip is closed, what the content is named,
    /// and what a press that opens a toggleable tooltip announces.
    pub text: Option<SharedString>,
    /// Opens, closes and inspects the tooltip from anywhere. Create one with
    /// [`create_tooltip_controller`].
    pub controller: Option<TooltipController<P>>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl<P: Clone + PartialEq + Debug + 'static> Default for TooltipOptions<P> {
    fn default() -> Self {
        Self {
            open: None,
            default_open: false,
            on_open_change: None,
            on_before_open_change: None,
            on_open_change_complete: None,
            on_payload_change: None,
            on_before_payload_change: None,
            delay: None,
            close_delay: None,
            disabled: false,
            toggleable: false,
            close_on_press: true,
            kind: None,
            shortcut: None,
            text: None,
            controller: None,
            debug: false,
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Clone for TooltipOptions<P> {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_before_open_change: self.on_before_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            on_before_payload_change: self.on_before_payload_change.clone(),
            delay: self.delay,
            close_delay: self.close_delay,
            disabled: self.disabled,
            toggleable: self.toggleable,
            close_on_press: self.close_on_press,
            kind: self.kind,
            shortcut: self.shortcut.clone(),
            text: self.text.clone(),
            controller: self.controller.clone(),
            debug: self.debug,
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> TooltipOptions<P> {
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

    pub fn on_before_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change = Some(Rc::new(move |open, details, _, window, cx| {
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

    pub fn on_before_payload_change(
        mut self,
        callback: impl Fn(&Option<P>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_payload_change = Some(Rc::new(callback));
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

    pub fn toggleable(mut self, toggleable: bool) -> Self {
        self.toggleable = toggleable;
        self
    }

    pub fn close_on_press(mut self, close_on_press: bool) -> Self {
        self.close_on_press = close_on_press;
        self
    }

    pub fn kind(mut self, kind: TooltipType) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn controller(mut self, controller: TooltipController<P>) -> Self {
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
            on_before_open_change: self.on_before_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            on_before_payload_change: self.on_before_payload_change.clone(),
            delay: self.delay,
            close_delay: self.close_delay,
            disabled: self.disabled,
            ..PopupOptions::default()
        }
    }
}

/// The props the `trigger` slot of `Tooltip` takes.
#[derive(Clone, Debug, PartialEq)]
pub struct TooltipTriggerProps<P = ()> {
    /// Data for the content to render when this trigger opens the tooltip,
    /// like the row it belongs to.
    pub payload: Option<P>,
}

impl<P> Default for TooltipTriggerProps<P> {
    fn default() -> Self {
        Self { payload: None }
    }
}

impl<P> TooltipTriggerProps<P> {
    pub fn payload(mut self, payload: P) -> Self {
        self.payload = Some(payload);
        self
    }
}

/// The props the `content` slot of `Tooltip` takes: where it goes beside its
/// trigger, and how it animates.
#[derive(Clone, Default)]
pub struct TooltipContentProps {
    /// The side defaults to the top, the alignment to the center, the side
    /// offset to 4 and the align offset to 0.
    pub position: PositionProps,
    /// How it animates in and out, the transitions its CSS would declare.
    pub timing: Option<PresenceTiming>,
}

impl TooltipContentProps {
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

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = Some(timing);
        self
    }
}

impl From<TooltipContentProps> for PopupContentProps {
    fn from(props: TooltipContentProps) -> Self {
        PopupContentProps {
            position: props.position,
            timing: props.timing,
            ..PopupContentProps::default()
        }
    }
}

/// The props the `shortcut` slot of `Tooltip` takes. By default it shows the
/// tooltip's `shortcut`.
pub type TooltipShortcutProps = ShortcutProps;

/// What the `trigger` part styles from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooltipTriggerState {
    /// Whether the tooltip is open from this trigger.
    pub open: bool,
}

/// What the `content` part styles from.
#[derive(Clone, Debug, PartialEq)]
pub struct TooltipContentState<P = ()> {
    /// Whether the tooltip is open. False while it animates out.
    pub open: bool,
    /// Whether it opened at once, because another tooltip was open a moment
    /// ago, so animations can skip (`data-instant`).
    pub instant: bool,
    /// The payload of the last open, from the trigger that opened it or a
    /// controller. The content renders before the first open, so it may be
    /// `None`.
    pub payload: Option<P>,
    /// Its enter and exit, the original's `data-starting-style` and
    /// `data-ending-style`.
    pub presence: PresenceState,
    /// Where it's placed, as of the last frame: its side, alignment, room and
    /// transform origin (`data-side`, `--transform-origin`, ..).
    pub placement: Option<Placement>,
    /// Closed, it stays in the window, hidden.
    pub hidden: bool,
}

/// What the `arrow` part styles from: whether the tooltip is open (false as
/// soon as it starts closing), the side it opened on, and where the arrow
/// goes.
pub type TooltipArrowState = PopupArrowState;

/// What the `shortcut` part styles from: the shortcut formatted for this
/// platform, `⌘S` on Apple platforms and `Ctrl+S` elsewhere. Empty without
/// one.
pub type TooltipShortcutState = ShortcutState;

/// A tooltip. Its entity is its controller.
pub struct Tooltip<P: Clone + PartialEq + Debug + 'static = ()> {
    instance: Instance,
    popup: Popup<Self>,
    disabled: bool,
    toggleable: bool,
    close_on_press: bool,
    kind: Option<TooltipType>,
    shortcut: Option<SharedString>,
    text: Option<SharedString>,
    // Apple keyboard conventions for shortcut names.
    apple: bool,
    // Set by a press that closed the tooltip, until the pointer leaves the
    // trigger.
    pressed: bool,
    // A press opened it, so its text is announced once it shows.
    announcing: Rc<Cell<bool>>,
    controller: ConnectedController<Self>,
}

impl<P: Clone + PartialEq + Debug + 'static> PopupHost for Tooltip<P> {
    type Payload = P;

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Tooltip<P> {
    pub fn new(options: TooltipOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("tooltip").with_agent(cx);
        instance.debug = options.debug;
        // Focus opens it only from the keyboard, which this tells from the
        // start.
        track_input(window, cx);
        let announcing = Rc::new(Cell::new(false));
        let rules = PopupRules {
            // Its text once a press opened it.
            on_open_change: Some(Rc::new({
                let announcing = announcing.clone();
                move |this: &mut Self,
                      open: bool,
                      _closed: Option<&ui_sdk_popups::PopupClose>,
                      window: &mut Window,
                      cx: &mut Context<Self>| {
                    if open && announcing.get() {
                        let text = this.text.clone();
                        window.defer(cx, move |window, cx| {
                            if let Some(text) = text {
                                announce(text.trim(), window, cx);
                            }
                        });
                    }
                    announcing.set(false);
                }
            })),
            ..PopupRules::default()
        };
        let popup = create_popup(
            &instance,
            CreatePopupOptions::new(tooltip_preset())
                .rules(rules)
                .options(options.popup()),
            window,
            cx,
        );
        Self {
            instance,
            popup,
            disabled: options.disabled,
            toggleable: options.toggleable,
            close_on_press: options.close_on_press,
            kind: options.kind,
            shortcut: options.shortcut,
            text: options.text,
            apple: initial_apple(cx),
            pressed: false,
            announcing,
            controller: ConnectedController::new(options.controller, cx),
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: TooltipOptions<P>, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        self.controller.set(options.controller.clone());
        let changed = self.kind != options.kind
            || self.shortcut != options.shortcut
            || self.text != options.text
            || self.toggleable != options.toggleable
            || self.close_on_press != options.close_on_press
            || self.disabled != options.disabled;
        self.disabled = options.disabled;
        self.toggleable = options.toggleable;
        self.close_on_press = options.close_on_press;
        self.kind = options.kind;
        self.shortcut = options.shortcut.clone();
        self.text = options.text.clone();
        let popup = self.popup.clone();
        let rendered = popup.set_options(options.popup(), cx);
        if changed && !rendered {
            cx.notify();
        }
    }

    /// Passes the controlled `open`, or `None` to let the tooltip keep its
    /// own.
    pub fn set_controlled_open(&mut self, open: Option<bool>, cx: &mut Context<Self>) {
        let popup = self.popup.clone();
        popup.set_controlled_open(open, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The popup it's built on, the original's `internal`.
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

    /// Whether it opened at once, because another tooltip was warm.
    pub fn is_instant(&self) -> bool {
        self.popup.group().is_some_and(|group| group.instant())
    }

    /// Whether it names its trigger rather than describes it, for a trigger
    /// that has a name of its own or not (`labels`).
    pub fn labels(&self, named: bool) -> bool {
        self.kind.unwrap_or(if named {
            TooltipType::Description
        } else {
            TooltipType::Label
        }) == TooltipType::Label
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

    /// Opens it, and resolves once it closes (`open`). On an open tooltip, a
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

    // A press toggles a toggleable tooltip. Any other closes it, unless
    // `close_on_press` is false, and it stays closed until the pointer
    // leaves (`tooltipPress`).
    fn press(
        &mut self,
        key: &SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let popup = self.popup.clone();
        let by = opener_of(event.as_ref());
        let control = popup.control().clone();
        if self.toggleable {
            if popup.is_open() && popup.layer().opener() != PopupOpener::Hover {
                control.hide(
                    self,
                    HideRequest::new().reason(Reason::TriggerPress).event(event),
                    window,
                    cx,
                );
                return;
            }
            if self.disabled {
                return;
            }
            self.announcing.set(!popup.is_open());
            let request = ShowRequest {
                reason: Some(Reason::TriggerPress),
                event,
                payload: Some(popup.trigger_payload(key)),
                by: Some(by),
                trigger: Some(key.clone()),
                ..ShowRequest::default()
            };
            control.show(self, request, None, window, cx);
            return;
        }
        if !self.close_on_press {
            return;
        }
        if let Some(hover) = popup.hover() {
            hover.clear();
        }
        self.pressed = event.is_some() && by != PopupOpener::Keyboard;
        control.hide(
            self,
            HideRequest::new().reason(Reason::TriggerPress).event(event),
            window,
            cx,
        );
    }

    // The pointer entered a trigger: it opens it, unless a press closed it.
    fn enter(&mut self, key: &SharedString, event: &HoverEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.pressed {
            return;
        }
        let popup = self.popup.clone();
        popup.trigger_hover(self, key, true, event, window, cx);
    }

    // The pointer left a trigger.
    fn leave(&mut self, key: &SharedString, event: &HoverEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.pressed = false;
        let popup = self.popup.clone();
        popup.trigger_hover(self, key, false, event, window, cx);
    }

    // Switches the shortcut names to the platform once a part renders
    // (`adoptPlatform`).
    fn adopt(entity: WeakEntity<Self>) -> impl FnOnce(&mut Window, &mut App) + 'static {
        move |_window, cx| {
            let Some(entity) = entity.upgrade() else {
                return;
            };
            if entity.read(cx).apple == is_apple(cx) {
                return;
            }
            entity.update(cx, |this, cx| {
                if adopt_platform(&mut this.apple, cx) {
                    cx.notify();
                }
            });
        }
    }

    /// The `trigger` part: a button the tooltip names or describes, with its
    /// own presses. Resting the pointer on it opens the tooltip, and keyboard
    /// focus does at once. It carries no open state of its own. Give its text
    /// with gpui's `text!(..)`; one without text or an `aria_label`, like an
    /// icon button, is named by the tooltip.
    pub fn trigger(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: TooltipTriggerProps<P>,
        cx: &App,
    ) -> TooltipTrigger {
        let key = key.into();
        let tooltip = this.read(cx);
        let popup = tooltip.popup.clone();
        let instance = &tooltip.instance;
        let trigger_props = PopupTriggerProps {
            payload: props.payload,
            ..PopupTriggerProps::default()
        };
        let (handle, region, subscribe) = popup.bind_trigger(&key, &trigger_props, cx);
        let state = popup.state(tooltip);
        let active = state.trigger.as_ref().is_none_or(|trigger| *trigger == key);
        let trigger_state = TooltipTriggerState {
            open: state.open && active,
        };
        set_unfocusable(&handle, false);
        let mut part = Part::new(instance, popup.trigger_slot(), Some(key.as_ref()), trigger_state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&handle), false);
        if let Some(shortcut) = tooltip.shortcut.as_ref().filter(|shortcut| !shortcut.is_empty()) {
            part = part.aria_keyshortcuts(aria_keyshortcuts(shortcut, tooltip.apple));
        }
        let entity = this.downgrade();
        {
            let entity = entity.clone();
            let key = key.clone();
            part = part.on_press(move |event, window, cx| {
                entity
                    .update(cx, |this, cx| this.press(&key, Some(event), window, cx))
                    .ok();
            });
        }
        {
            let entity = entity.clone();
            let key = key.clone();
            part = part.map_el(|el| {
                el.on_hover(move |hovered, window, cx| {
                    let event = HoverEvent::from_window(window);
                    let hovered = *hovered;
                    entity
                        .update(cx, |this, cx| {
                            if hovered {
                                this.enter(&key, &event, window, cx);
                            } else {
                                this.leave(&key, &event, window, cx);
                            }
                        })
                        .ok();
                })
            });
        }
        {
            let entity = entity.clone();
            part = part.map_el(|el| {
                el.on_key_down(move |event, window, cx| {
                    let Some(target) = entity.upgrade() else {
                        return;
                    };
                    let popup = target.read(cx).popup().clone();
                    let took = target.update(cx, |this, cx| popup.trigger_keydown(this, event, window, cx));
                    if took {
                        cx.stop_propagation();
                    }
                })
            });
        }
        if subscribe && popup.focus_opening().is_some() {
            let entity = entity.clone();
            let key = key.clone();
            part = part.on_props(move |window, cx| {
                entity
                    .update(cx, |this, cx| {
                        let popup = this.popup().clone();
                        popup.subscribe_trigger(key, window, cx);
                    })
                    .ok();
            });
        }
        let part = region.measure(part.on_props(Self::adopt(entity)));
        TooltipTrigger {
            part,
            naming: Naming {
                kind: tooltip.kind,
                text: tooltip.text.clone().filter(|text| !text.trim().is_empty()),
                active,
                content: instance.element_id(popup.content_slot(), None),
            },
        }
    }

    /// The `content` part: the tooltip itself, always in the window, hidden
    /// while closed, so its trigger is named or described before focus
    /// reaches it. It never takes focus. It renders above the window, placed
    /// beside its trigger, while open. Give it plain text: nothing in it can
    /// be reached.
    pub fn content(
        this: &Entity<Self>,
        props: TooltipContentProps,
        cx: &App,
    ) -> PopupContent<Self, TooltipContentState<P>> {
        let tooltip = this.read(cx);
        let popup = tooltip.popup.clone();
        let state = popup.state(tooltip);
        let layer = popup.layer();
        let attrs = popup_content_attrs(state.open, state.mounted);
        let content_state = TooltipContentState {
            open: attrs.open,
            instant: tooltip.is_instant(),
            payload: state.payload.clone(),
            presence: layer.presence().state(),
            placement: layer.positioner().and_then(|positioner| positioner.placement()),
            hidden: attrs.hidden,
        };
        let mut part = Part::new(&tooltip.instance, popup.content_slot(), None, content_state)
            .aria_role(Role::Tooltip)
            .aria(Aria::new().hidden(attrs.hidden));
        // Hidden content names or describes its trigger in full, so the
        // content carries its spoken text as its label, which the trigger's
        // name and description read. An `aria_label` the app gives wins.
        if let Some(text) = tooltip.text.as_ref() {
            part = part.aria_label(text.trim().to_string());
        }
        let entity = this.downgrade();
        {
            let entity = entity.clone();
            part = part.on_keys(&["Tab", "Escape"], move |event, window, cx| {
                Popup::content_keydown(&entity, event, window, cx);
            });
        }
        {
            let entity = entity.clone();
            part = part.map_el(|el| {
                el.on_hover(move |hovered, window, cx| {
                    let event = HoverEvent::from_window(window);
                    let hovered = *hovered;
                    entity
                        .update(cx, |this, cx| {
                            let popup = this.popup().clone();
                            let Some(hover) = popup.hover().cloned() else {
                                return;
                            };
                            if hovered {
                                hover.enter(this, &event, None, None, window, cx);
                            } else {
                                hover.leave_content(this, &event, window, cx);
                            }
                        })
                        .ok();
                })
            });
        }
        // One open from the start is the open one of the warm group, once
        // its content is in the window.
        part = part.on_props(move |_window, cx| {
            if let Some(tooltip) = entity.upgrade() {
                let popup = tooltip.read(cx).popup().clone();
                if let Some(group) = popup.group() {
                    group.join(popup.is_open());
                }
            }
        });
        bind_popup_content(this, part, props.into(), cx)
    }

    /// The `arrow` part, which points at the trigger from the side the
    /// tooltip opened on.
    pub fn arrow(this: &Entity<Self>, cx: &App) -> Part<TooltipArrowState> {
        popup_arrow(this, cx)
    }

    /// A `shortcut` part: the tooltip's `shortcut`, or its own, for people to
    /// see, with `key` for a tooltip that renders several. It's hidden from
    /// assistive technology, which hears the trigger's `aria-keyshortcuts`.
    pub fn shortcut(
        this: &Entity<Self>,
        key: Option<SharedString>,
        props: TooltipShortcutProps,
        cx: &App,
    ) -> ShortcutPart {
        let tooltip = this.read(cx);
        shortcut_slot(
            &tooltip.instance,
            key.as_deref(),
            props,
            tooltip.shortcut.clone(),
            tooltip.apple,
        )
        .map_part(|part| part.on_props(Self::adopt(this.downgrade())))
    }
}

/// `agent: { actions: ["setOpen"] }`, with `open`, `close` and `toggle`,
/// which `setOpen` covers, unlisted.
impl<P: Clone + PartialEq + Debug + 'static> Agent for Tooltip<P> {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", |tooltip, args, window, cx| {
            let open = args.boolean(0)?;
            Ok(tooltip
                .set_open(open, ChangeOptions::default(), window, cx)
                .into())
        }),
        AgentAction::unlisted("open", |tooltip, _args, window, cx| {
            let _pending = tooltip.open(PopupOpenOptions::default(), window, cx);
            Ok(PENDING.into())
        }),
        AgentAction::unlisted("close", |tooltip, _args, window, cx| {
            Ok(tooltip.close(PopupCloseOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |tooltip, _args, window, cx| {
            Ok(tooltip.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("open", self.is_open())
            .with("disabled", self.disabled)
    }
}

// How a trigger is named or described, decided as gpui builds its
// accessibility node, once its children are in: the original's `unnamed`,
// which it reads from the trigger's element.
#[derive(Clone)]
struct Naming {
    kind: Option<TooltipType>,
    text: Option<SharedString>,
    active: bool,
    content: ElementId,
}

impl Naming {
    // Whether a trigger has a name without the tooltip: a label, a
    // labelled-by relation, or content of its own in the accessibility tree,
    // like text (`hasOwnName`).
    fn has_own_name(node: &Node) -> bool {
        node.label().is_some_and(|label| !label.trim().is_empty())
            || !node.labelled_by().is_empty()
            || !node.children().is_empty()
    }

    fn write(&self, node: &mut Node, content: Option<NodeId>) {
        let label = self.kind.unwrap_or(if Self::has_own_name(node) {
            TooltipType::Description
        } else {
            TooltipType::Label
        }) == TooltipType::Label;
        if label {
            if let Some(text) = &self.text {
                node.set_label(text.trim().to_string());
            }
            if let Some(content) = content {
                node.set_labelled_by(vec![content]);
            }
            return;
        }
        // Only the trigger that opened it is described by it.
        if !self.active {
            return;
        }
        if let Some(text) = &self.text {
            node.set_description(text.trim().to_string());
        }
        if let Some(content) = content {
            node.push_described_by(content);
        }
    }
}

/// The `trigger` part, as the app styles it: a [`Part`] the tooltip names or
/// describes as gpui builds its accessibility node, since only then are the
/// trigger's own name and text known.
pub struct TooltipTrigger {
    part: Part<TooltipTriggerState>,
    naming: Naming,
}

impl TooltipTrigger {
    pub fn state(&self) -> &TooltipTriggerState {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &TooltipTriggerState) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters, as on any part.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.part = self.part.aria(aria);
        self
    }

    /// Changes the part with any of [`Part`]'s methods, like another role.
    pub fn map_part(
        mut self,
        map: impl FnOnce(Part<TooltipTriggerState>) -> Part<TooltipTriggerState>,
    ) -> Self {
        self.part = map(self.part);
        self
    }
}

impl Styled for TooltipTrigger {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl InteractiveElement for TooltipTrigger {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl StatefulInteractiveElement for TooltipTrigger {}

impl ParentElement for TooltipTrigger {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl IntoElement for TooltipTrigger {
    type Element = TooltipTriggerElement;

    fn into_element(self) -> Self::Element {
        let (mut id, mut source_location) = (None, None);
        let part = self.part.map_el(|el| {
            id = Element::id(&el);
            source_location = Element::source_location(&el);
            el
        });
        TooltipTriggerElement {
            element: part.into_element(),
            id,
            source_location,
            naming: self.naming,
            content: None,
        }
    }
}

/// What a [`TooltipTrigger`] renders as.
pub struct TooltipTriggerElement {
    element: PartElement,
    id: Option<ElementId>,
    source_location: Option<&'static Location<'static>>,
    naming: Naming,
    // The content's accessibility node, as of its last layout.
    content: Option<NodeId>,
}

impl Element for TooltipTriggerElement {
    type RequestLayoutState = <PartElement as Element>::RequestLayoutState;
    type PrepaintState = <PartElement as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.id.clone()
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        self.source_location
    }

    fn a11y_role(&self) -> Option<gpui::Role> {
        Element::a11y_role(&self.element)
    }

    fn write_a11y_info(&self, node: &mut Node) {
        Element::write_a11y_info(&self.element, node);
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut A11ySubtreeBuilder,
    ) {
        Element::a11y_synthetic_children(&mut self.element, prepaint, builder);
        self.naming.write(builder.parent_node(), self.content);
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        Element::request_layout(&mut self.element, id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let prepaint = Element::prepaint(
            &mut self.element,
            id,
            inspector_id,
            bounds,
            request_layout,
            window,
            cx,
        );
        // The content's node, while this frame's tree has it or will.
        self.content = node_id_of(&self.naming.content, window, cx);
        prepaint
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        Element::paint(
            &mut self.element,
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        )
    }
}
