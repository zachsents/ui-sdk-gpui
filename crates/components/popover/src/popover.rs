//! Port of `components/popover/popover.ts`: rich content opened from a
//! button, beside it, a non-modal dialog by default.
//!
//! A popover is `create_popup` with `popover_preset`. What's its own: trigger
//! shortcuts, and the Shortcut part that shows them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, Context, Entity, KeyDownEvent, SharedString, StatefulInteractiveElement, WeakEntity, Window,
};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, ChangeCallback, ChangeDetails, ChangeOptions, ConnectedController,
    DismissAttemptDetails, InputEvent, Instance, PENDING, Part, Reason,
};
use ui_sdk_platform::{aria_keyshortcuts, initial_apple, is_apple};
use ui_sdk_popups::{
    CreatePopupOptions, DismissAttemptCallback, HideRequest, Modal, OpenChangeCompleteCallback, Popup,
    PopupArrowState, PopupCloseOptions, PopupContent, PopupContentProps, PopupContentState, PopupController,
    PopupHost, PopupOpenChangeCallback, PopupOpenOptions, PopupOpener, PopupOptions, PopupPending,
    PopupTriggerProps, PopupTriggerState, PopupViewport, PresentationSetting, ShowRequest, create_popup,
    create_popup_controller, popover_preset, popup_arrow, popup_close, popup_content, popup_description,
    popup_title, popup_trigger, popup_viewport,
};
use ui_sdk_shortcuts::{
    ShortcutOptions, ShortcutPart, ShortcutProps, ShortcutState, adopt_platform, bind_part_shortcut,
    shortcut_of, shortcut_slot,
};

/// Why a popover opened or closed: `TriggerPress`, `TriggerHover`,
/// `OutsidePress`, `EscapeKey`, `FocusOut`, `WindowBlur`, `ClosePress`,
/// `Back`, `Shortcut`, `Swipe` or `Imperative`.
pub type PopoverOpenChangeReason = Reason;

/// The dismissals `dismissible` refuses that `on_dismiss_attempt` hears
/// about: `OutsidePress`, `EscapeKey`, `Back` and `Swipe`.
pub type PopoverDismissReason = Reason;

/// Why the popover's payload changed: a trigger opened it (`TriggerPress`,
/// `TriggerHover`, `Shortcut`), another trigger took it over while open, or
/// a controller opened it or gave the open one another payload
/// (`Imperative`).
pub type PopoverPayloadChangeReason = Reason;

/// Opens, closes and inspects a popover from outside it, before it's made or
/// without its entity at hand: a toolbar, a hotkey. Pass it as the
/// `controller` option; its methods are `PopupControllerExt`'s. `P` is the
/// type of the data triggers and `open()` pass to the content.
pub type PopoverController<P = ()> = PopupController<Popover<P>>;

/// A controller for a popover (`createPopoverController`).
pub fn create_popover_controller<P: Clone + PartialEq + Debug + 'static>() -> PopoverController<P> {
    create_popup_controller()
}

/// Where focus goes when the popover opens. `Auto` picks the first tab stop,
/// or the popover itself when it has none. `Content` focuses the popover
/// itself and `None` leaves focus where it is. A focus handle or a function
/// picks a specific element.
pub type PopoverInitialFocus = ui_sdk_popups::PopupInitialFocus;

/// Where focus goes when the popover closes while focus was inside it.
/// `Trigger` returns it to the trigger and `None` leaves it. A focus handle
/// or a function picks a specific element.
pub type PopoverFinalFocus = ui_sdk_popups::PopupFocusTarget;

/// The `modal` a popover takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PopoverModal {
    /// The rest of the window stays usable, and focus leaving the popover
    /// closes it.
    False,
    /// Also traps focus, blocks the rest of the window and its scrolling, and
    /// an outside press only closes the popover.
    True,
    /// Traps focus but leaves the window scrollable and pressable.
    TrapFocus,
}

impl From<bool> for PopoverModal {
    fn from(modal: bool) -> Self {
        if modal {
            PopoverModal::True
        } else {
            PopoverModal::False
        }
    }
}

impl From<PopoverModal> for Modal {
    fn from(modal: PopoverModal) -> Self {
        match modal {
            PopoverModal::False => Modal::False,
            PopoverModal::True => Modal::True,
            PopoverModal::TrapFocus => Modal::TrapFocus,
        }
    }
}

/// The options `Popover` takes.
pub struct PopoverOptions<P: Clone + PartialEq + Debug + 'static = ()> {
    /// Whether the popover is open. Pass it to control the popover.
    pub open: Option<bool>,
    /// Whether the popover starts open, when uncontrolled. Defaults to false.
    pub default_open: bool,
    /// Runs when the popover opens or closes, with what closed it, like a
    /// Close part's value. `details.reason` says why (see
    /// [`PopoverOpenChangeReason`]). It runs once the popover's update is
    /// over, so it may update any entity, the popover too.
    pub on_open_change: Option<PopupOpenChangeCallback>,
    /// Runs as the popover is asked to open or close, before it does, inside
    /// the popover's update: `details.cancel()` stops it. It may update any
    /// entity but the popover.
    pub on_before_open_change: Option<PopupOpenChangeCallback>,
    /// Runs once the popover has finished opening or closing, after any
    /// animation, once the popover's update is over.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Runs when the payload the content shows changes: a trigger with
    /// another payload opens the popover, or takes the open popover over,
    /// which fires no `on_open_change`, and so does a controller's
    /// `open(payload)` on the open popover. It runs once the popover's update
    /// is over.
    pub on_payload_change: Option<ChangeCallback<Option<P>>>,
    /// Runs as the payload is asked to change, before it does, inside the
    /// popover's update: `details.cancel()` keeps the previous payload. It
    /// may update any entity but the popover.
    pub on_before_payload_change: Option<ChangeCallback<Option<P>>>,
    /// `False`: the rest of the window stays usable, and focus leaving the
    /// popover closes it. `True`: also traps focus, blocks the rest of the
    /// window and its scrolling, and an outside press only closes the
    /// popover. `TrapFocus`: traps focus but leaves the window scrollable and
    /// pressable. Defaults to `False`, or `True` while it presents as a
    /// sheet.
    pub modal: Option<PopoverModal>,
    /// Whether resting the pointer on the trigger opens the popover. Pressing
    /// the trigger still opens it, so keyboard and touch users can too.
    /// Defaults to false.
    pub open_on_hover: bool,
    /// How long the pointer rests on the trigger before the popover opens,
    /// with `open_on_hover`. Defaults to 300 milliseconds.
    pub delay: Option<Duration>,
    /// How long before the popover closes once the pointer leaves it and its
    /// trigger, with `open_on_hover`. Defaults to none.
    pub close_delay: Option<Duration>,
    /// How the popover presents. `Sheet` slides it up from the bottom of the
    /// window, where it can be swiped away, and `Auto` does that on small
    /// touch screens. Decided each time it opens. Defaults to `Default`, a
    /// popover.
    pub presentation: PresentationSetting,
    /// Opens, closes and inspects the popover from anywhere. Create one with
    /// [`create_popover_controller`].
    pub controller: Option<PopoverController<P>>,
    /// Whether Escape, an outside press, focus or the window leaving, the
    /// back gesture and a swipe can close it. Triggers, Close parts and the
    /// controller always can. Defaults to true.
    pub dismissible: bool,
    /// Runs when `dismissible` refused a dismissal made on purpose (see
    /// [`PopoverDismissReason`]), once the popover's update is over.
    pub on_dismiss_attempt: Option<DismissAttemptCallback>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl<P: Clone + PartialEq + Debug + 'static> Default for PopoverOptions<P> {
    fn default() -> Self {
        Self {
            open: None,
            default_open: false,
            on_open_change: None,
            on_before_open_change: None,
            on_open_change_complete: None,
            on_payload_change: None,
            on_before_payload_change: None,
            modal: None,
            open_on_hover: false,
            delay: None,
            close_delay: None,
            presentation: PresentationSetting::Default,
            controller: None,
            dismissible: true,
            on_dismiss_attempt: None,
            debug: false,
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Clone for PopoverOptions<P> {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_before_open_change: self.on_before_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            on_before_payload_change: self.on_before_payload_change.clone(),
            modal: self.modal,
            open_on_hover: self.open_on_hover,
            delay: self.delay,
            close_delay: self.close_delay,
            presentation: self.presentation,
            controller: self.controller.clone(),
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            debug: self.debug,
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> PopoverOptions<P> {
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// `on_open_change`, for a callback that doesn't read what closed it.
    pub fn on_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(move |open, details, _, window, cx| {
            callback(open, details, window, cx)
        }));
        self
    }

    /// `on_open_change` with what closed it, like a Close part's value.
    pub fn on_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    /// `on_before_open_change`, for a callback that doesn't read what
    /// closes it.
    pub fn on_before_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change = Some(Rc::new(move |open, details, _, window, cx| {
            callback(open, details, window, cx)
        }));
        self
    }

    /// `on_before_open_change` with what closes it, like a Close part's
    /// value.
    pub fn on_before_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change = Some(Rc::new(callback));
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

    pub fn modal(mut self, modal: impl Into<PopoverModal>) -> Self {
        self.modal = Some(modal.into());
        self
    }

    pub fn open_on_hover(mut self, open_on_hover: bool) -> Self {
        self.open_on_hover = open_on_hover;
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

    pub fn presentation(mut self, presentation: PresentationSetting) -> Self {
        self.presentation = presentation;
        self
    }

    pub fn controller(mut self, controller: PopoverController<P>) -> Self {
        self.controller = Some(controller);
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(
        mut self,
        callback: impl Fn(&DismissAttemptDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
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
            modal: self.modal.map(Modal::from),
            presentation: Some(self.presentation),
            open_on_hover: self.open_on_hover,
            delay: self.delay,
            close_delay: self.close_delay,
            disabled: false,
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
        }
    }
}

/// The props the `trigger` slot of `Popover` takes.
#[derive(Clone, Debug, PartialEq)]
pub struct PopoverTriggerProps<P = ()> {
    /// Stops the trigger from opening the popover. It leaves the tab order,
    /// unless `focusable_when_disabled` is set.
    pub disabled: bool,
    /// Keeps a disabled trigger in the tab order with `aria-disabled`, so it
    /// can host a tooltip that says why. Presses still do nothing.
    pub focusable_when_disabled: bool,
    /// Data for the content to render when this trigger opens the popover,
    /// like the row it belongs to.
    pub payload: Option<P>,
    /// A keyboard shortcut that presses the trigger, like `"mod+i"`. `mod` is
    /// Command on Apple platforms and Control elsewhere. Sets
    /// `aria-keyshortcuts`.
    pub shortcut: Option<SharedString>,
}

impl<P> Default for PopoverTriggerProps<P> {
    fn default() -> Self {
        Self {
            disabled: false,
            focusable_when_disabled: false,
            payload: None,
            shortcut: None,
        }
    }
}

impl<P> PopoverTriggerProps<P> {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn focusable_when_disabled(mut self, focusable: bool) -> Self {
        self.focusable_when_disabled = focusable;
        self
    }

    pub fn payload(mut self, payload: P) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
}

/// The props the `content` slot of `Popover` takes: where it goes beside its
/// trigger (the side defaults to the bottom, the alignment to the center,
/// the side offset to 4 and the align offset to 0), `initial_focus` (`Auto`
/// by default; opening on hover never moves focus, and opening with touch
/// focuses the popover itself so no on-screen keyboard appears),
/// `final_focus` (`Trigger` by default), `keep_mounted`, `inline`, which
/// renders it in place inside its parent's layout and leaves placing it to
/// the app, and how it animates.
pub type PopoverContentProps = PopupContentProps;

/// The props the `shortcut` slot of `Popover` takes. Inside a trigger, or
/// beside one, it shows that trigger's `shortcut` by default.
pub type PopoverShortcutProps = ShortcutProps;

/// What the `trigger` part styles from: whether the popover is open from
/// this trigger, and whether its `disabled` prop is set.
pub type PopoverTriggerState = PopupTriggerState;

/// What the `content` part styles from: whether it's open (false while it
/// animates out), the `modal` setting (never `Pointer`), how it presents
/// this time (`Popover` or `Sheet`), the payload of the last open, and its
/// presence, placement and sheet.
pub type PopoverContentState<P = ()> = PopupContentState<P>;

/// What the `arrow` part styles from: whether the popover is open (false as
/// soon as it starts closing), the side it opened on, and where the arrow
/// goes.
pub type PopoverArrowState = PopupArrowState;

/// What the `shortcut` part styles from: the shortcut formatted for this
/// platform, `⌘I` on Apple platforms and `Ctrl+I` elsewhere. Empty without
/// one.
pub type PopoverShortcutState = ShortcutState;

/// A popover. Its entity is its controller.
pub struct Popover<P: Clone + PartialEq + Debug + 'static = ()> {
    instance: Instance,
    popup: Popup<Self>,
    // Apple keyboard conventions for shortcut names.
    apple: bool,
    controller: ConnectedController<Self>,
    // The shortcut each trigger rendered with, by key, which a Shortcut part
    // that sits in or beside it shows.
    shortcuts: Rc<RefCell<HashMap<SharedString, SharedString>>>,
}

impl<P: Clone + PartialEq + Debug + 'static> PopupHost for Popover<P> {
    type Payload = P;

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Popover<P> {
    pub fn new(options: PopoverOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("popover").with_agent(cx);
        instance.debug = options.debug;
        let popup = create_popup(
            &instance,
            CreatePopupOptions::new(popover_preset()).options(options.popup()),
            window,
            cx,
        );
        Self {
            instance,
            popup,
            apple: initial_apple(cx),
            controller: ConnectedController::new(options.controller, cx),
            shortcuts: Rc::default(),
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: PopoverOptions<P>, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        self.controller.set(options.controller.clone());
        let popup = self.popup.clone();
        popup.set_options(options.popup(), cx);
    }

    /// Passes the controlled `open`, or `None` to let the popover keep its
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

    /// Whether it's open. False as soon as it starts closing, while it still
    /// animates out (`isOpen`).
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// The payload of the last open, from a controller or the trigger that
    /// opened it (`getPayload`).
    pub fn payload(&self) -> Option<P> {
        self.popup.payload()
    }

    /// Opens or closes it. Returns false when the change was canceled or it
    /// was already open or closed (`setOpen`).
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

    /// Opens it, and resolves once it closes, with why, how and any return
    /// value (`open`). On an open popover, a payload replaces the current one
    /// and an anchor moves it.
    pub fn open(
        &mut self,
        options: PopupOpenOptions<P>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupPending {
        let popup = self.popup.clone();
        popup.open(self, options, window, cx)
    }

    /// Closes it. `return_value` is passed to the future `open()` returned
    /// (`close`).
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

    /// The shortcut of the trigger an element sits in or beside, by the
    /// element's place (`shortcutOf`).
    pub fn shortcut_of(&self, el: &gpui::GlobalElementId) -> Option<SharedString> {
        let shortcuts = self.shortcuts.clone();
        shortcut_of(
            &self.instance.handle(),
            self.popup.trigger_slot(),
            el,
            true,
            |key| key.and_then(|key| shortcuts.borrow().get(key).cloned()),
        )
    }

    // The trigger's shortcut: closes the open popover, else opens it from
    // that trigger, as the keyboard does.
    fn shortcut_press(
        &mut self,
        key: &SharedString,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let popup = self.popup.clone();
        if popup.trigger_disabled(key) {
            return;
        }
        let event = InputEvent::KeyDown(event.clone());
        let control = popup.control().clone();
        if popup.is_open() {
            control.hide(
                self,
                HideRequest::new().reason(Reason::Shortcut).event(Some(event)),
                window,
                cx,
            );
            return;
        }
        let request = ShowRequest {
            reason: Some(Reason::Shortcut),
            event: Some(event),
            payload: Some(popup.trigger_payload(key)),
            by: Some(PopupOpener::Keyboard),
            trigger: Some(key.clone()),
            ..ShowRequest::default()
        };
        control.show(self, request, None, window, cx);
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

    /// The `trigger` part: a popup trigger whose `shortcut` presses it. A
    /// press opens the popover and closes it; several triggers can share one
    /// popover, each handing the content its `payload`. Give its text with
    /// gpui's `text!(..)`, or name it with `aria_label`.
    pub fn trigger(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: PopoverTriggerProps<P>,
        cx: &App,
    ) -> Part<PopoverTriggerState> {
        let key = key.into();
        let popover = this.read(cx);
        let apple = popover.apple;
        let shortcut = props.shortcut.clone().filter(|shortcut| !shortcut.is_empty());
        {
            let mut shortcuts = popover.shortcuts.borrow_mut();
            match &shortcut {
                Some(shortcut) => {
                    shortcuts.insert(key.clone(), shortcut.clone());
                }
                None => {
                    shortcuts.remove(&key);
                }
            }
        }
        let trigger_props = PopupTriggerProps {
            disabled: props.disabled,
            focusable_when_disabled: props.focusable_when_disabled,
            payload: props.payload,
        };
        let mut part = popup_trigger(this, key.clone(), trigger_props, cx);
        if let Some(shortcut) = &shortcut {
            part = part.aria_keyshortcuts(aria_keyshortcuts(shortcut, apple));
        }
        let region = popover.popup.trigger_region(&key);
        let entity = this.downgrade();
        let press_key = key.clone();
        let mut options = ShortcutOptions::default();
        if let Some(region) = region {
            options = options.layer(region);
        }
        part = bind_part_shortcut(
            part,
            shortcut,
            move |event, window, cx| {
                entity
                    .update(cx, |this, cx| this.shortcut_press(&press_key, event, window, cx))
                    .ok();
            },
            options,
        );
        part.on_props(Self::adopt(this.downgrade()))
    }

    /// The `content` part: the popover itself, a dialog named by its title,
    /// else its trigger, and described by its description, placed beside its
    /// trigger. `None` while it's out of the window. It renders above the
    /// window, unless `inline`.
    pub fn content(
        this: &Entity<Self>,
        props: PopoverContentProps,
        cx: &App,
    ) -> Option<PopupContent<Self, PopoverContentState<P>>> {
        popup_content(this, props, cx)
    }

    /// The `viewport` part: the window-sized element the popover sits in
    /// while it presents as a sheet. Put the content inside it. `None`
    /// otherwise.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> Option<PopupViewport<Self>> {
        popup_viewport(this, cx)
    }

    /// The `title` part, which names the popover while it renders. Give it
    /// its text as an `aria_label`, the popover's name, and draw the same
    /// text inside it.
    pub fn title(this: &Entity<Self>, cx: &App) -> Part<()> {
        popup_title(this, cx)
    }

    /// The `description` part, which describes the popover while it renders.
    /// Give it its text as an `aria_label`.
    pub fn description(this: &Entity<Self>, cx: &App) -> Part<()> {
        popup_description(this, cx)
    }

    /// A `close` part, which closes the popover with reason `ClosePress`. A
    /// popover with several gives each a key.
    pub fn close_part(this: &Entity<Self>, key: Option<SharedString>, cx: &App) -> Part<()> {
        popup_close(this, key, cx)
    }

    /// The `arrow` part, which points at the trigger from the side the
    /// popover opened on.
    pub fn arrow(this: &Entity<Self>, cx: &App) -> Part<PopoverArrowState> {
        popup_arrow(this, cx)
    }

    /// A `shortcut` part: a shortcut's keys, for people to see, with `key`
    /// for a popover that renders several. Without keys of its own, it shows
    /// the shortcut of the trigger it sits in or beside. It's hidden from
    /// assistive technology, which hears the trigger's `aria-keyshortcuts`.
    pub fn shortcut(
        this: &Entity<Self>,
        key: Option<SharedString>,
        props: PopoverShortcutProps,
        cx: &App,
    ) -> ShortcutPart {
        let popover = this.read(cx);
        let instance = popover.instance.handle();
        let slot = popover.popup.trigger_slot();
        let shortcuts = popover.shortcuts.clone();
        shortcut_slot(&popover.instance, key.as_deref(), props, None, popover.apple)
            .inherit_from(move |el, _cx| {
                shortcut_of(&instance, slot, el, true, |key| {
                    key.and_then(|key| shortcuts.borrow().get(key).cloned())
                })
            })
            .map_part(|part| part.on_props(Self::adopt(this.downgrade())))
    }
}

/// `agent: { actions: ["setOpen"] }`, with `open`, `close` and `toggle`,
/// which `setOpen` covers, unlisted.
impl<P: Clone + PartialEq + Debug + 'static> Agent for Popover<P> {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", |popover, args, window, cx| {
            let open = args.boolean(0)?;
            Ok(popover
                .set_open(open, ChangeOptions::default(), window, cx)
                .into())
        }),
        AgentAction::unlisted("open", |popover, _args, window, cx| {
            let _pending = popover.open(PopupOpenOptions::default(), window, cx);
            Ok(PENDING.into())
        }),
        AgentAction::unlisted("close", |popover, _args, window, cx| {
            Ok(popover.close(Default::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |popover, _args, window, cx| {
            Ok(popover.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new().with("open", self.is_open())
    }
}
