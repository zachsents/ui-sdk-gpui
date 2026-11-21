//! Port of `foundation/popups/create-popup.ts`: popups a component puts
//! together: [`create_popup`] in its constructor, its parts from the builders
//! in [`popup_slots`](crate::popup_slots), and its state from
//! [`Popup::state`]. The preset is a value, like
//! [`hover_card_preset`](crate::hover_card_preset), that carries the behavior
//! its WAI-ARIA pattern needs: placement, hover and focus opening, the sheet.
//!
//! A component built on a popup implements [`PopupHost`], which is how the
//! popup reaches the component's instance and itself from inside the
//! component's update. Its options carry a [`PopupOptions`], handed to
//! [`Popup::set_options`] whenever they change. Its controller is a kernel
//! [`Controller`] with [`PopupControllerExt`]'s methods.

use std::cell::{Cell, RefCell};
use std::fmt::Debug;
use std::rc::{Rc, Weak};
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{AnyWindowHandle, App, Context, ElementId, FocusHandle, SharedString, Subscription, Task, Window};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    ChangeCallback, ChangeDetails, ChangeOptions, ChangeSource, Controllable, Controller, InputEvent,
    Instance, InstanceHandle, Measured, flag_while_bound, with_optional_source,
};
use ui_sdk_platform::{Align, Anchor, ElementBounds, Layer, Region, Target};

use crate::dismissal::{DismissAttemptCallback, DismissOptions, DismissRules, Dismissal, RefusalFn, RefusedFn, create_dismissal};
use crate::popup::{
    AttachConfig, CloseDetails, CloseFn, ContainsFn, EscapeFn, HookFn, InitialFocusFn, Modal,
    OpenChangeFn, PopupClose, PopupCloseOptions, PopupLayer, PopupLayerOptions, PopupModal,
    PopupOpenOptions, PopupPending, RegionFn, ShowFn, modal_triple,
};
use crate::popup_behavior::PopupBehaviorOverrides;
use crate::popup_elements::{PopupElement, register_popup_element};
use crate::popup_features::{
    FocusOpening, HoverOpening, PlacementDefaults, PressOpening, TriggerName, WarmGroup,
    content_anchor, focus_opening, hover_opening, placement_options, resolve_initial_focus, warm_group,
};
use crate::popup_payload::create_popup_payload;
use crate::popup_presets::PopupPreset;
use crate::popup_slots::{PopupContentProps, PopupTriggerProps};
use crate::popup_steps::{
    CloseRule, HideRequest, OpenChange, OpenRule, OpenRules, OpenSteps, OpenStepsOptions,
    create_open_steps,
};
use crate::presentation::{
    Presentation, PresentationFeature, PresentationSetting, SheetConfig, create_presentation,
};

/// A component built on a popup: how the popup reaches the component's
/// instance and itself from inside the component's update.
pub trait PopupHost: Sized + 'static {
    /// The data a trigger or a controller hands the content, `()` for none.
    type Payload: Clone + PartialEq + Debug + 'static;

    /// The component's instance, which its changes report with.
    fn instance(&self) -> &Instance;

    /// The popup it's built on.
    fn popup(&self) -> &Popup<Self>;
}

/// Hears the popup asking to open or close: the requested `open`, the
/// details, and what closed it, like a Close part's `value`. It runs inside
/// the component's update, so it may update any entity but the component.
pub type PopupOpenChangeCallback = Rc<dyn Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App)>;

/// Hears an open or a close finish, after any animation.
pub type OpenChangeCompleteCallback = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// The options [`create_popup`] reads from the component's, when it has
/// them (`PopupOptions`).
pub struct PopupOptions<P> {
    /// Whether it's open. Pass it to control the popup.
    pub open: Option<bool>,
    /// Whether it starts open, when uncontrolled.
    pub default_open: bool,
    /// Runs when it wants to open or close. `details.reason` says why, and
    /// `details.cancel()` stops it.
    pub on_open_change: Option<PopupOpenChangeCallback>,
    /// Runs once it has finished opening or closing, after any animation.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Runs when the payload the content shows changes: a trigger with
    /// another payload opens it, or takes it over while open, and so does a
    /// controller's `open(payload)` on the open popup. `details.cancel()`
    /// keeps the previous payload.
    pub on_payload_change: Option<ChangeCallback<Option<P>>>,
    /// Overrides the preset's `modal`.
    pub modal: Option<Modal>,
    /// How it presents. Decided each time it opens.
    pub presentation: Option<PresentationSetting>,
    /// Whether resting the pointer on the trigger opens a popup a press
    /// opens.
    pub open_on_hover: bool,
    /// How long the pointer rests on the trigger before it opens.
    pub delay: Option<Duration>,
    /// How long before it closes once the pointer has left it and its
    /// trigger.
    pub close_delay: Option<Duration>,
    /// Stops it from opening.
    pub disabled: bool,
    /// Whether Escape, an outside press, focus or the window leaving, the
    /// back gesture and a swipe can close it. Defaults to true.
    pub dismissible: bool,
    /// Runs when `dismissible` refused a dismissal made on purpose.
    pub on_dismiss_attempt: Option<DismissAttemptCallback>,
}

impl<P> Default for PopupOptions<P> {
    fn default() -> Self {
        Self {
            open: None,
            default_open: false,
            on_open_change: None,
            on_open_change_complete: None,
            on_payload_change: None,
            modal: None,
            presentation: None,
            open_on_hover: false,
            delay: None,
            close_delay: None,
            disabled: false,
            dismissible: true,
            on_dismiss_attempt: None,
        }
    }
}

impl<P> Clone for PopupOptions<P> {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            modal: self.modal,
            presentation: self.presentation,
            open_on_hover: self.open_on_hover,
            delay: self.delay,
            close_delay: self.close_delay,
            disabled: self.disabled,
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
        }
    }
}

impl<P> PopupOptions<P> {
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
        self.on_open_change = Some(Rc::new(move |open, details, _, window, cx| callback(open, details, window, cx)));
        self
    }

    /// `on_open_change` with what closed it, like a Close part's `value`.
    pub fn on_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
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

    pub fn modal(mut self, modal: impl Into<Modal>) -> Self {
        self.modal = Some(modal.into());
        self
    }

    pub fn presentation(mut self, presentation: PresentationSetting) -> Self {
        self.presentation = Some(presentation);
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

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(
        mut self,
        callback: impl Fn(&ui_sdk_kernel::DismissAttemptDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
        self
    }
}

/// The state a popup keeps (`PopupState`), as [`Popup::state`] reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupState<P> {
    /// Whether it's open. False as soon as it starts closing, while `mounted`
    /// stays true through the exit animation.
    pub open: bool,
    /// True while it's in the page, including while it animates out.
    pub mounted: bool,
    /// The trigger that opened it last, by key. `None` until a trigger
    /// renders.
    pub trigger: Option<SharedString>,
    /// The payload of the last open.
    pub payload: Option<P>,
    /// Whether a title renders, which names it. `None` until the parts have
    /// rendered, while a title may still render.
    pub titled: Option<bool>,
    /// Whether a description renders, which describes it.
    pub described: bool,
    /// The `modal` in effect, from the option, the preset and the
    /// presentation.
    pub modal: Modal,
    /// How it presents while open, decided as it opens.
    pub presentation: Presentation,
}

/// The state a popup starts with, from its first options (`popupState`).
pub fn popup_state<P>(open: Option<bool>, default_open: bool) -> PopupState<P> {
    let open = open.unwrap_or(default_open);
    PopupState {
        open,
        mounted: open,
        trigger: None,
        payload: None,
        titled: None,
        described: false,
        modal: Modal::False,
        presentation: Presentation::Popover,
    }
}

/// What only one popup does, beside its preset's behavior, for a component
/// whose rules no preset has, like a dialog that waits for an action
/// (`PopupRules`). Each is optional and replaces or extends what
/// [`create_popup`] does at that step.
pub struct PopupRules<T: PopupHost> {
    /// How it refuses a close beyond `dismissible`.
    pub refusal: Option<RefusalFn<T>>,
    /// Runs after `on_dismiss_attempt` heard a refused attempt.
    pub refused: Option<RefusedFn<T>>,
    /// Asks to close for a dismissal the popup layer saw, like Escape or an
    /// outside press. Closes by default.
    pub dismiss: Option<CloseFn<T>>,
    /// The element it opens from, for placement, nesting and focus return,
    /// when that isn't its trigger slot's, like a combobox's input. The
    /// window's layers and placement read it outside the component's update,
    /// and from inside other components' updates, so it reads what it holds,
    /// like regions and an instance handle, never the component.
    pub trigger: Option<RegionFn>,
    /// Whether a close returns focus when its request doesn't say. True by
    /// default; false for a list whose focus stays in an input.
    pub return_focus: Option<bool>,
    /// Whether a press on something outside the popup, or focus there,
    /// counts as inside. Its triggers do by default. A point is a press and
    /// comes with its event, `None` for focus, so a context menu's area can
    /// count only for the secondary button. Like `trigger`, it reads what it
    /// holds, never the component.
    pub contains: Option<ContainsFn>,
    /// Regions that stay usable while the popup blocks the window. Presses
    /// on them never dismiss it.
    pub persistent: Option<Rc<dyn Fn(&T) -> Vec<Region>>>,
    /// Escape while it's the topmost layer. Closes by default.
    pub on_escape: Option<EscapeFn<T>>,
    /// Runs as it shows, with its layer.
    pub on_show: Option<ShowFn<T>>,
    /// Runs as it starts opening or closing, with why it closed.
    pub on_open_change: Option<OpenChangeFn<T>>,
    /// Runs once opening or closing has finished, after any animation.
    pub on_open_change_complete: Option<OpenChangeFn<T>>,
    /// Decides an open: call the step's `open` to go on, with how it opens
    /// when the rules decide that, or return without calling it to refuse.
    pub open: Option<OpenRule<T, T::Payload>>,
    /// Decides a close: call the step's `close` to go on, or return without
    /// calling it to stay open.
    pub close: Option<CloseRule<T, T::Payload>>,
    /// What the content's `initial_focus` of `Auto` prefers to its first tab
    /// stop, like an autofocused element.
    pub preferred_focus: Option<InitialFocusFn>,
    /// More of what its content binds with, like a sheet's snap points, read
    /// from the component as the content binds.
    pub bind: Option<Rc<dyn Fn(&T) -> SheetConfig<T>>>,
    /// Runs each time the content binds, as the popup opens and as its
    /// content renders open from the start, before the layer shows it: the
    /// content's own `bind`, like a sheet attaching to the element it moves.
    pub on_bind: Option<HookFn<T>>,
    /// Runs once the content has left the page and the layer let go of it:
    /// the cleanup of the content's `bind`, like a sheet letting go of it.
    pub on_unbind: Option<HookFn<T>>,
    /// The layer it nests under, like a submenu's under its parent menu. By
    /// default, the open popup around its trigger.
    pub parent: Option<Rc<dyn Fn(&T) -> Option<Layer>>>,
    /// What being modal does while it shows, when the `modal` in its state
    /// doesn't say it all, like a submenu that never blocks the window.
    pub modal: Option<Rc<dyn Fn(&T) -> PopupModal>>,
    /// How it asks to present, when that isn't its `presentation` option,
    /// like a context menu that's always a popover.
    pub presentation: Option<Rc<dyn Fn(&T) -> Option<PresentationSetting>>>,
    /// An open popup asked to open again. Another trigger takes it over by
    /// default.
    pub reopen: Option<OpenRule<T, T::Payload>>,
}

impl<T: PopupHost> Default for PopupRules<T> {
    fn default() -> Self {
        Self {
            refusal: None,
            refused: None,
            dismiss: None,
            trigger: None,
            return_focus: None,
            contains: None,
            persistent: None,
            on_escape: None,
            on_show: None,
            on_open_change: None,
            on_open_change_complete: None,
            open: None,
            close: None,
            preferred_focus: None,
            bind: None,
            on_bind: None,
            on_unbind: None,
            parent: None,
            modal: None,
            presentation: None,
            reopen: None,
        }
    }
}

impl<T: PopupHost> Clone for PopupRules<T> {
    fn clone(&self) -> Self {
        Self {
            refusal: self.refusal.clone(),
            refused: self.refused.clone(),
            dismiss: self.dismiss.clone(),
            trigger: self.trigger.clone(),
            return_focus: self.return_focus,
            contains: self.contains.clone(),
            persistent: self.persistent.clone(),
            on_escape: self.on_escape.clone(),
            on_show: self.on_show.clone(),
            on_open_change: self.on_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            open: self.open.clone(),
            close: self.close.clone(),
            preferred_focus: self.preferred_focus.clone(),
            bind: self.bind.clone(),
            on_bind: self.on_bind.clone(),
            on_unbind: self.on_unbind.clone(),
            parent: self.parent.clone(),
            modal: self.modal.clone(),
            presentation: self.presentation.clone(),
            reopen: self.reopen.clone(),
        }
    }
}

/// What [`create_popup`] takes beside the component's instance.
pub struct CreatePopupOptions<T: PopupHost> {
    /// The behavior, like [`popover_preset`](crate::popover_preset).
    pub preset: PopupPreset,
    /// The slot that is the popup. Defaults to `"content"`.
    pub slot: &'static str,
    /// The slot that opens it. Defaults to `"trigger"`.
    pub trigger: &'static str,
    /// The content's role, like `AlertDialog`. Defaults to the preset's.
    pub role: Option<Role>,
    /// What only this popup does, beside the preset's behavior.
    pub rules: PopupRules<T>,
    /// Its behavior where it differs from its preset's, like a submenu that
    /// stays open when the window loses focus.
    pub behavior: PopupBehaviorOverrides,
    /// The options it reads from the component's.
    pub options: PopupOptions<T::Payload>,
}

impl<T: PopupHost> CreatePopupOptions<T> {
    pub fn new(preset: PopupPreset) -> Self {
        Self {
            preset,
            slot: "content",
            trigger: "trigger",
            role: None,
            rules: PopupRules::default(),
            behavior: PopupBehaviorOverrides::default(),
            options: PopupOptions::default(),
        }
    }

    pub fn slot(mut self, slot: &'static str) -> Self {
        self.slot = slot;
        self
    }

    pub fn trigger(mut self, trigger: &'static str) -> Self {
        self.trigger = trigger;
        self
    }

    pub fn role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    pub fn rules(mut self, rules: PopupRules<T>) -> Self {
        self.rules = rules;
        self
    }

    pub fn behavior(mut self, behavior: PopupBehaviorOverrides) -> Self {
        self.behavior = behavior;
        self
    }

    pub fn options(mut self, options: PopupOptions<T::Payload>) -> Self {
        self.options = options;
        self
    }
}

#[derive(Clone)]
struct Settings {
    modal: Option<Modal>,
    presentation: Option<PresentationSetting>,
    open_on_hover: bool,
    delay: Option<Duration>,
    close_delay: Option<Duration>,
    disabled: bool,
    on_open_change_complete: Option<OpenChangeCompleteCallback>,
}

pub(crate) struct TriggerEntry<P> {
    pub(crate) key: SharedString,
    pub(crate) handle: FocusHandle,
    pub(crate) region: Region,
    pub(crate) props: PopupTriggerProps<P>,
    pub(crate) subscriptions: Option<Vec<Subscription>>,
}

pub(crate) struct ContentParts {
    pub(crate) region: Region,
    pub(crate) props: RefCell<PopupContentProps>,
    pub(crate) live: Rc<Cell<usize>>,
    // Whether the content is in the page, from its first frame there until
    // it's heard leaving.
    pub(crate) present: Cell<bool>,
    pub(crate) departures: UnboundedSender<()>,
    _registered: PopupElement,
}

pub(crate) struct PopupInner<T: PopupHost> {
    pub(crate) label: SharedString,
    pub(crate) role: Option<Role>,
    pub(crate) preset: PopupPreset,
    pub(crate) content_slot: &'static str,
    pub(crate) trigger_slot: &'static str,
    pub(crate) instance: InstanceHandle,
    pub(crate) window: AnyWindowHandle,
    pub(crate) open: Rc<RefCell<Controllable<bool>>>,
    pub(crate) payload: Rc<RefCell<Controllable<Option<T::Payload>>>>,
    pub(crate) trigger_key: Rc<RefCell<Option<SharedString>>>,
    pub(crate) modal: Cell<Modal>,
    pub(crate) presentation_value: Rc<Cell<Presentation>>,
    settings: RefCell<Settings>,
    open_callback: Rc<RefCell<Option<PopupOpenChangeCallback>>>,
    return_value: Rc<RefCell<Option<SharedString>>>,
    gone_at_open: RefCell<Option<SharedString>>,
    only_trigger: Cell<Option<bool>>,
    was_open: Cell<bool>,
    pub(crate) triggers: RefCell<Vec<TriggerEntry<T::Payload>>>,
    pub(crate) content: ContentParts,
    pub(crate) arrow: Measured,
    pub(crate) backdrop: Region,
    pub(crate) viewport: Region,
    pub(crate) closes: RefCell<Vec<(Option<SharedString>, FocusHandle)>>,
    pub(crate) layer: PopupLayer<T>,
    pub(crate) control: OpenSteps<T, T::Payload>,
    pub(crate) rules: PopupRules<T>,
    pub(crate) dismissal: Option<Dismissal<T>>,
    pub(crate) presentation: Option<PresentationFeature<T>>,
    pub(crate) press: Option<PressOpening>,
    pub(crate) group: Option<WarmGroup<T>>,
    pub(crate) hover: Option<HoverOpening<T>>,
    pub(crate) focus: Option<FocusOpening<T>>,
    pub(crate) trigger_name: Option<TriggerName>,
    _departures: Task<()>,
}

/// A component's popup (`createPopup`'s return): the api a popup controller
/// drives, [`is_open`](Self::is_open), [`set_open`](Self::set_open),
/// [`open`](Self::open), [`close`](Self::close), [`toggle`](Self::toggle),
/// [`payload`](Self::payload) and [`focus`](Self::focus), and the features
/// its parts and the component reach, the original's `internal`. Clones
/// share it.
pub struct Popup<T: PopupHost> {
    pub(crate) inner: Rc<PopupInner<T>>,
}

impl<T: PopupHost> Clone for Popup<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// A popup held weakly, for closures that outlive what they read.
pub struct WeakPopup<T: PopupHost>(Weak<PopupInner<T>>);

impl<T: PopupHost> Clone for WeakPopup<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: PopupHost> WeakPopup<T> {
    pub fn upgrade(&self) -> Option<Popup<T>> {
        self.0.upgrade().map(|inner| Popup { inner })
    }
}

/// Makes a component a popup (`createPopup`). Call it in the component's
/// constructor with its instance, keep the popup in a field, return it from
/// [`PopupHost::popup`], and build the parts with the builders in
/// [`popup_slots`](crate::popup_slots). It brings the top layer, presence,
/// Escape, outside presses, focus moving in and back, modality, nesting under
/// other popups, and several triggers that each hand the content a payload.
///
/// `preset` is the behavior: [`popover_preset`](crate::popover_preset),
/// [`dialog_preset`](crate::dialog_preset),
/// [`tooltip_preset`](crate::tooltip_preset) or
/// [`hover_card_preset`](crate::hover_card_preset), among others. Each follows
/// its WAI-ARIA pattern: how Tab, Escape, outside presses and the window
/// losing focus behave, whether it's placed beside its trigger, whether a
/// press, resting the pointer or keyboard focus opens it, the content's role
/// and the `modal` it has unless the component's option says.
///
/// It reads `open`, `modal`, `presentation`, `open_on_hover`, `delay`,
/// `close_delay`, `disabled`, `dismissible` and `on_dismiss_attempt` from
/// [`PopupOptions`], and changes `open` and `payload` through
/// `Controllable::change`, so `on_open_change` and `on_payload_change` hear
/// every reason: `TriggerPress`, `TriggerHover`, `TriggerFocus`, `EscapeKey`,
/// `OutsidePress`, `FocusOut`, `WindowBlur`, `ClosePress` and `Imperative`.
pub fn create_popup<T: PopupHost>(
    instance: &Instance,
    config: CreatePopupOptions<T>,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Popup<T> {
    let CreatePopupOptions {
        preset,
        slot: content_slot,
        trigger: trigger_slot,
        role,
        rules,
        behavior,
        options,
    } = config;
    let initial = options.open.unwrap_or(options.default_open);

    let open_callback: Rc<RefCell<Option<PopupOpenChangeCallback>>> = Rc::new(RefCell::new(options.on_open_change.clone()));
    let return_value: Rc<RefCell<Option<SharedString>>> = Rc::new(RefCell::new(None));
    let mut open = Controllable::new("open", initial);
    open.set_controlled(options.open);
    {
        let callback = open_callback.clone();
        let value = return_value.clone();
        open.set_on_change(Some(Rc::new(move |open: &bool, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
            let callback = callback.borrow().clone();
            let value = value.borrow().clone();
            if let Some(callback) = callback {
                callback(open, details, value.as_ref(), window, cx);
            }
        })));
    }
    let open = Rc::new(RefCell::new(open));
    let mut payload = Controllable::new("payload", None);
    payload.set_on_change(options.on_payload_change.clone());
    let payload = Rc::new(RefCell::new(payload));
    let trigger_key: Rc<RefCell<Option<SharedString>>> = Rc::new(RefCell::new(None));
    let presentation_value = Rc::new(Cell::new(Presentation::Popover));

    let content_region = Region::with_focus_handle(cx.focus_handle());
    let registered = register_popup_element(&content_region, instance.slot_name(content_slot));
    let (departures, mut left) = unbounded::<()>();
    let departures_task = cx.spawn_in(window, async move |this, cx| {
        while left.next().await.is_some() {
            this.update_in(cx, |this, window, cx| {
                let popup = this.popup().clone();
                popup.content_left(this, window, cx);
            })
            .ok();
        }
    });

    Popup {
        inner: Rc::new_cyclic(|me: &Weak<PopupInner<T>>| {
            let weak = WeakPopup(me.clone());
            let trigger: RegionFn = {
                let weak = weak.clone();
                Rc::new(move || weak.upgrade().and_then(|popup| popup.trigger_element()))
            };
            let return_trigger: RegionFn = {
                let weak = weak.clone();
                Rc::new(move || weak.upgrade().and_then(|popup| popup.opened_from()))
            };
            let contains: ContainsFn = match rules.contains.clone() {
                Some(contains) => contains,
                None => {
                    let weak = weak.clone();
                    Rc::new(move |target: &Target, event: Option<&InputEvent>, window: &Window, _cx: &App| {
                        let Some(popup) = weak.upgrade() else {
                            return false;
                        };
                        popup
                            .trigger_element()
                            .is_some_and(|trigger| trigger.contains(target, window))
                            || popup
                                .press()
                                .is_some_and(|press| press.contains(&popup, target, event, window))
                    })
                }
            };
            let final_focus = {
                let weak = weak.clone();
                Rc::new(move || weak.upgrade().and_then(|popup| popup.content_props().final_focus))
            };
            let mut layer_options = PopupLayerOptions::new(
                preset.behavior,
                move || trigger(),
                |this: &mut T, reason, details, window, cx| {
                    let popup = this.popup().clone();
                    popup.dismiss(this, reason, details, window, cx);
                },
                |this: &T| this.popup().modal_effects(this),
            );
            layer_options.behavior = behavior;
            layer_options.owner = Some((instance.label().clone(), instance.id().clone()));
            layer_options.return_trigger = Some(return_trigger);
            layer_options.parent = rules.parent.clone();
            layer_options.contains = Some(contains);
            layer_options.final_focus = Some(final_focus);
            layer_options.persistent = rules.persistent.clone();
            layer_options.position = preset.placement;
            layer_options.on_escape = Some(Rc::new(|this: &mut T, event, window, cx| {
                let popup = this.popup().clone();
                match popup.inner.rules.on_escape.clone() {
                    Some(on_escape) => on_escape(this, event, window, cx),
                    None => popup.dismiss(
                        this,
                        ui_sdk_kernel::Reason::EscapeKey,
                        CloseDetails {
                            event: Some(ui_sdk_kernel::InputEvent::KeyDown(event.clone())),
                            focus: crate::popup::ReturnMode::Return,
                            return_value: None,
                        },
                        window,
                        cx,
                    ),
                }
            }));
            layer_options.on_show = rules.on_show.clone();
            layer_options.on_open_change = Some(Rc::new(|this: &mut T, open, closed, window, cx| {
                let popup = this.popup().clone();
                popup.layer_open_change(this, open, closed, window, cx);
            }));
            layer_options.on_open_change_complete = Some(Rc::new(|this: &mut T, open, closed, window, cx| {
                let popup = this.popup().clone();
                if let Some(rule) = popup.inner.rules.on_open_change_complete.clone() {
                    rule(this, open, closed, window, cx);
                }
                let complete = popup.inner.settings.borrow().on_open_change_complete.clone();
                if let Some(complete) = complete {
                    complete(open, window, cx);
                }
            }));
            let layer = PopupLayer::new(layer_options, initial, window, cx);

            let dismissal = preset.dismissal.then(|| {
                create_dismissal(
                    DismissOptions {
                        dismissible: options.dismissible,
                        on_dismiss_attempt: options.on_dismiss_attempt.clone(),
                    },
                    DismissRules {
                        refusal: rules.refusal.clone(),
                        refused: rules.refused.clone(),
                    },
                )
            });
            let presentation = preset.presentation.then(|| {
                let dialog = preset.behavior == crate::popup_behavior::PopupBehaviorName::Dialog;
                if dialog {
                    presentation_value.set(Presentation::Dialog);
                }
                let rule = rules.presentation.clone();
                create_presentation(
                    presentation_value.clone(),
                    move |this: &T| match &rule {
                        Some(rule) => rule(this),
                        None => this.popup().inner.settings.borrow().presentation,
                    },
                    options.presentation,
                    if dialog {
                        Presentation::Dialog
                    } else {
                        Presentation::Popover
                    },
                    dismissal.as_ref().map(Dismissal::sheet),
                )
            });
            let payload_feature = preset
                .payload
                .then(|| create_popup_payload::<T, T::Payload>(trigger_key.clone(), payload.clone(), T::instance));
            let return_focus = rules.return_focus;
            let control = create_open_steps(OpenStepsOptions {
                layer: layer.clone(),
                is_open: Rc::new(|this: &T| this.popup().is_open()),
                change: Rc::new(|this: &mut T, open, change, window, cx| {
                    let popup = this.popup().clone();
                    popup.change_open(this, open, change, window, cx)
                }),
                payload: payload_feature,
                dismissal: dismissal.clone(),
                return_focus: return_focus.map(|value| Rc::new(move || Some(value)) as Rc<dyn Fn() -> Option<bool>>),
                rules: OpenRules {
                    // A pending hover open or close never fires once something
                    // else opens it.
                    open: Some(Rc::new(|this: &mut T, request, step, window, cx| {
                        let popup = this.popup().clone();
                        if let Some(hover) = popup.hover() {
                            hover.clear();
                        }
                        if let Some(group) = popup.group() {
                            group.opening(request.request.event.as_ref(), window, cx);
                        }
                        match popup.inner.rules.open.clone() {
                            Some(rule) => rule(this, request, step, window, cx),
                            None => {
                                step.open(this, None, window, cx);
                            }
                        }
                    })),
                    // Another trigger that shares the open popup takes it
                    // over, with its payload.
                    reopen: Some(Rc::new(|this: &mut T, request, step, window, cx| {
                        let popup = this.popup().clone();
                        if let Some(hover) = popup.hover() {
                            hover.clear();
                        }
                        if let Some(rule) = popup.inner.rules.reopen.clone() {
                            return rule(this, request, step, window, cx);
                        }
                        if let Some(trigger) = &request.request.trigger
                            && Some(trigger) != popup.trigger_key().as_ref()
                        {
                            popup.control().clone().take_over(
                                this,
                                trigger.clone(),
                                request.request.payload.clone().flatten(),
                                None,
                                request.payload_reason,
                                request.request.event.clone(),
                                window,
                                cx,
                            );
                        }
                    })),
                    close: rules.close.clone(),
                },
            });
            let press = preset.press.then_some(PressOpening);
            let group = preset.group.then(|| warm_group(window, cx));
            let hover = preset.hover.then(|| hover_opening(cx));
            let focus = preset.focus.then(focus_opening);
            let trigger_name = preset.trigger_name.then_some(TriggerName);
            let label = instance.label().clone();

            PopupInner {
                label,
                role: role.or(preset.role),
                preset,
                content_slot,
                trigger_slot,
                instance: instance.handle(),
                window: window.window_handle(),
                open,
                payload,
                trigger_key,
                modal: Cell::new(Modal::False),
                presentation_value,
                settings: RefCell::new(Settings {
                    modal: options.modal,
                    presentation: options.presentation,
                    open_on_hover: options.open_on_hover,
                    delay: options.delay,
                    close_delay: options.close_delay,
                    disabled: options.disabled,
                    on_open_change_complete: options.on_open_change_complete.clone(),
                }),
                open_callback,
                return_value,
                gone_at_open: RefCell::new(None),
                only_trigger: Cell::new(None),
                was_open: Cell::new(initial),
                triggers: RefCell::new(Vec::new()),
                content: ContentParts {
                    region: content_region,
                    props: RefCell::new(PopupContentProps::default()),
                    live: Rc::new(Cell::new(0)),
                    present: Cell::new(false),
                    departures,
                    _registered: registered,
                },
                arrow: Measured::default(),
                backdrop: Region::new(),
                viewport: Region::new(),
                closes: RefCell::new(Vec::new()),
                layer,
                control,
                rules,
                dismissal,
                presentation,
                press,
                group,
                hover,
                focus,
                trigger_name,
                _departures: departures_task,
            }
        }),
    }
    .initialized()
}

impl<T: PopupHost> Popup<T> {
    fn initialized(self) -> Self {
        self.update_modal_value();
        self
    }

    /// This popup held weakly.
    pub fn weak(&self) -> WeakPopup<T> {
        WeakPopup(Rc::downgrade(&self.inner))
    }

    /// The component's label, for warnings.
    pub fn label(&self) -> &SharedString {
        &self.inner.label
    }

    /// The content's role: the component's, else the preset's.
    pub fn role(&self) -> Option<Role> {
        self.inner.role
    }

    pub fn preset(&self) -> PopupPreset {
        self.inner.preset
    }

    /// The content's slot name, like `"content"`.
    pub fn content_slot(&self) -> &'static str {
        self.inner.content_slot
    }

    /// The trigger's slot name, like `"trigger"`.
    pub fn trigger_slot(&self) -> &'static str {
        self.inner.trigger_slot
    }

    /// The popup layer it shows through.
    pub fn layer(&self) -> &PopupLayer<T> {
        &self.inner.layer
    }

    /// Its open and close steps, the original's `control`.
    pub fn control(&self) -> &OpenSteps<T, T::Payload> {
        &self.inner.control
    }

    pub fn rules(&self) -> &PopupRules<T> {
        &self.inner.rules
    }

    pub fn dismissal(&self) -> Option<&Dismissal<T>> {
        self.inner.dismissal.as_ref()
    }

    pub fn presentation(&self) -> Option<&PresentationFeature<T>> {
        self.inner.presentation.as_ref()
    }

    pub fn press(&self) -> Option<&PressOpening> {
        self.inner.press.as_ref()
    }

    pub fn group(&self) -> Option<&WarmGroup<T>> {
        self.inner.group.as_ref()
    }

    pub fn hover(&self) -> Option<&HoverOpening<T>> {
        self.inner.hover.as_ref()
    }

    pub fn focus_opening(&self) -> Option<&FocusOpening<T>> {
        self.inner.focus.as_ref()
    }

    pub fn trigger_name(&self) -> Option<&TriggerName> {
        self.inner.trigger_name.as_ref()
    }

    /// The content's region: its bounds, hitbox and focus handle.
    pub fn content_region(&self) -> Region {
        self.inner.content.region.clone()
    }

    /// The content's focus handle, which it tracks without being a tab stop.
    pub fn content_focus_handle(&self) -> FocusHandle {
        self.inner
            .content
            .region
            .focus_handle()
            .expect("the content tracks a focus handle")
    }

    /// The backdrop's region.
    pub fn backdrop_region(&self) -> Region {
        self.inner.backdrop.clone()
    }

    /// The viewport's region: the window-sized host a sheet sits in.
    pub fn viewport_region(&self) -> Region {
        self.inner.viewport.clone()
    }

    /// The sheet it presents as, while it does.
    pub fn sheet(&self) -> Option<crate::sheet::Sheet<T>> {
        self.presentation().and_then(|presentation| presentation.sheet())
    }

    /// Where the arrow laid out, which placement reads.
    pub fn arrow_bounds(&self) -> Measured {
        self.inner.arrow.clone()
    }

    /// The content's props from its last render.
    pub fn content_props(&self) -> PopupContentProps {
        self.inner.content.props.borrow().clone()
    }

    /// Whether it's open. False as soon as it starts closing, while it still
    /// animates out (`isOpen`).
    pub fn is_open(&self) -> bool {
        *self.inner.open.borrow().get()
    }

    /// Whether it's in the page, including while it animates out.
    pub fn is_mounted(&self) -> bool {
        self.inner.layer.is_mounted()
    }

    /// The payload of the last open, from a controller or the trigger that
    /// opened it (`getPayload`).
    pub fn payload(&self) -> Option<T::Payload> {
        self.inner.payload.borrow().get().clone()
    }

    /// The trigger that opened it last, by key.
    pub fn trigger_key(&self) -> Option<SharedString> {
        self.inner.trigger_key.borrow().clone()
    }

    /// Sets the trigger that opened it last, by key, for a component whose
    /// own parts stand for triggers, like a menu's context areas: the first
    /// to render holds it until one opens the popup (`store.set({ trigger
    /// })`). Its parts read it as they render.
    pub fn set_trigger_key(&self, key: Option<SharedString>) {
        *self.inner.trigger_key.borrow_mut() = key;
    }

    /// The `modal` in effect.
    pub fn modal(&self) -> Modal {
        self.inner.modal.get()
    }

    /// How it presents now.
    pub fn presentation_value(&self) -> Presentation {
        self.inner.presentation_value.get()
    }

    pub(crate) fn open_on_hover_option(&self) -> bool {
        self.inner.settings.borrow().open_on_hover
    }

    pub(crate) fn delay_option(&self) -> Option<Duration> {
        self.inner.settings.borrow().delay
    }

    pub(crate) fn close_delay_option(&self) -> Option<Duration> {
        self.inner.settings.borrow().close_delay
    }

    pub(crate) fn disabled_option(&self) -> bool {
        self.inner.settings.borrow().disabled
    }

    /// The popup's state, as its parts and the component read it.
    pub fn state(&self, this: &T) -> PopupState<T::Payload> {
        let instance = this.instance();
        PopupState {
            open: self.is_open(),
            mounted: self.is_mounted(),
            trigger: self.trigger_key(),
            payload: self.payload(),
            titled: flag_while_bound(instance, "title"),
            described: flag_while_bound(instance, "description").unwrap_or(false),
            modal: self.modal(),
            presentation: self.presentation_value(),
        }
    }

    fn bound_keys(&self, slot: &str) -> Vec<SharedString> {
        self.inner
            .instance
            .bound_parts()
            .into_iter()
            .filter(|part| part.slot == slot)
            .filter_map(|part| part.key)
            .fold(Vec::new(), |mut keys, key| {
                if !keys.contains(&key) {
                    keys.push(key);
                }
                keys
            })
    }

    /// The keys of the triggers in the page, in the order they rendered.
    pub fn bound_triggers(&self) -> Vec<SharedString> {
        self.bound_keys(self.inner.trigger_slot)
    }

    pub(crate) fn slot_bound(&self, slot: &str) -> bool {
        self.inner.instance.bound_parts().iter().any(|part| part.slot == slot)
    }

    /// The region of the trigger with `key`, once it has rendered.
    pub fn trigger_region(&self, key: &SharedString) -> Option<Region> {
        self.inner
            .triggers
            .borrow()
            .iter()
            .find(|entry| entry.key == *key)
            .map(|entry| entry.region.clone())
    }

    /// The focus handle of the trigger with `key`, once it has rendered.
    pub fn trigger_focus_handle(&self, key: &SharedString) -> Option<FocusHandle> {
        self.inner
            .triggers
            .borrow()
            .iter()
            .find(|entry| entry.key == *key)
            .map(|entry| entry.handle.clone())
    }

    /// Whether the trigger with `key` rendered disabled (`triggerDisabled`).
    pub fn trigger_disabled(&self, key: &SharedString) -> bool {
        self.inner
            .triggers
            .borrow()
            .iter()
            .any(|entry| entry.key == *key && entry.props.disabled)
    }

    pub(crate) fn trigger_contains(&self, target: &Target, window: &Window) -> bool {
        let bound = self.bound_triggers();
        self.inner
            .triggers
            .borrow()
            .iter()
            .any(|entry| bound.contains(&entry.key) && !entry.props.disabled && entry.region.contains(target, window))
    }

    /// The trigger that opened the popup last, or the first one in the page,
    /// or the element the rules name (`triggerElement`).
    pub fn trigger_element(&self) -> Option<Region> {
        if let Some(trigger) = &self.inner.rules.trigger {
            return trigger();
        }
        let key = self.trigger_key();
        let bound = self.bound_triggers();
        let chosen = key.filter(|key| bound.contains(key)).or_else(|| bound.first().cloned());
        chosen.and_then(|key| self.trigger_region(&key))
    }

    // Only the trigger that opened it, so focus never returns to another one
    // once that one is gone. An open after that trigger was already gone
    // falls back like `trigger_element`, and so does a popup's only trigger,
    // like the same button rendered again (`openedFrom`).
    fn opened_from(&self) -> Option<Region> {
        let key = self.trigger_key();
        if self.inner.rules.trigger.is_some() || key.is_none() || key == *self.inner.gone_at_open.borrow() {
            return self.trigger_element();
        }
        let key = key?;
        let bound = self.bound_triggers();
        let only = self.inner.only_trigger.get().unwrap_or(bound.len() <= 1);
        if bound.contains(&key) {
            self.trigger_region(&key)
        } else if only {
            bound.first().and_then(|first| self.trigger_region(first))
        } else {
            None
        }
    }

    /// The element id of the trigger that names the content: the one that
    /// opened it last, or the first in the page.
    pub fn active_trigger_id(&self, instance: &Instance, state: &PopupState<T::Payload>) -> Option<ElementId> {
        let bound = self.bound_triggers();
        let key = state
            .trigger
            .clone()
            .filter(|key| bound.contains(key))
            .or_else(|| bound.first().cloned())?;
        Some(instance.element_id(self.inner.trigger_slot, Some(&key)))
    }

    /// The modal effects it shows with: the rules', else its `modal`'s.
    pub fn modal_effects(&self, this: &T) -> PopupModal {
        match &self.inner.rules.modal {
            Some(modal) => modal(this),
            None => modal_triple(self.modal()),
        }
    }

    // The app's `modal`, else modal while it presents as a sheet, like iOS's
    // sheets, else the preset's. A preset without one leaves `modal` to the
    // component, like Dialog's, or is never modal, like hover content, which
    // WCAG 1.4.13 needs dismissible without trapping anyone.
    fn modal_of(&self) -> Option<Modal> {
        let preset = self.inner.preset.modal?;
        let arriving = self.inner.settings.borrow().modal;
        Some(arriving.unwrap_or(if self.presentation_value() == Presentation::Sheet {
            Modal::True
        } else {
            preset
        }))
    }

    fn update_modal_value(&self) -> bool {
        let Some(modal) = self.modal_of() else {
            return false;
        };
        let changed = self.inner.modal.get() != modal;
        self.inner.modal.set(modal);
        changed
    }

    /// Sets the `modal` in effect, for a component whose preset names none,
    /// like Dialog's. While it shows, the window hears the change at once.
    pub fn set_modal(&self, this: &mut T, modal: Modal, window: &mut Window, cx: &mut Context<T>) {
        if self.inner.modal.replace(modal) == modal {
            return;
        }
        self.inner.layer.update_modal(this, window, cx);
    }

    /// Hands it new options, as the component's arrive (the original's
    /// `sync`). A new controlled `open` mounts or unmounts it once the
    /// current update is over. Returns whether what renders changed.
    pub fn set_options(&self, options: PopupOptions<T::Payload>, cx: &mut Context<T>) -> bool {
        let inner = &self.inner;
        let (changed_open, source) = inner.open.borrow_mut().set_controlled_with_source(options.open);
        *inner.open_callback.borrow_mut() = options.on_open_change.clone();
        inner.payload.borrow_mut().set_on_change(options.on_payload_change.clone());
        if let Some(dismissal) = &inner.dismissal {
            dismissal.set_options(DismissOptions {
                dismissible: options.dismissible,
                on_dismiss_attempt: options.on_dismiss_attempt.clone(),
            });
        }
        let changed_settings = {
            let mut settings = inner.settings.borrow_mut();
            let changed = settings.modal != options.modal
                || settings.presentation != options.presentation
                || settings.disabled != options.disabled;
            *settings = Settings {
                modal: options.modal,
                presentation: options.presentation,
                open_on_hover: options.open_on_hover,
                delay: options.delay,
                close_delay: options.close_delay,
                disabled: options.disabled,
                on_open_change_complete: options.on_open_change_complete.clone(),
            };
            changed
        };
        let changed_modal = self.update_modal_value();
        if changed_open || changed_modal {
            self.schedule_sync(source, cx);
        }
        let changed = changed_open || changed_settings || changed_modal;
        if changed {
            cx.notify();
        }
        changed
    }

    /// Passes the controlled `open`, or `None` to let the popup keep its
    /// own. It mounts or unmounts once the current update is over.
    pub fn set_controlled_open(&self, open: Option<bool>, cx: &mut Context<T>) -> bool {
        let (changed, source) = self.inner.open.borrow_mut().set_controlled_with_source(open);
        if changed {
            self.schedule_sync(source, cx);
            cx.notify();
        }
        changed
    }

    // The layer hears what arrived with the options once the update that
    // handed them over is done, with the source of the change that asked
    // for them.
    fn schedule_sync(&self, source: Option<ChangeSource>, cx: &mut Context<T>) {
        let window = self.inner.window;
        let entity = cx.weak_entity();
        let app: &mut App = cx;
        app.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    entity
                        .update(cx, |this, cx| {
                            let popup = this.popup().clone();
                            with_optional_source(source, || popup.sync(this, window, cx));
                        })
                        .ok();
                })
                .ok();
        });
    }

    fn sync(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        self.open_changed(this, window, cx);
        if self.inner.layer.is_shown() && self.inner.layer.modal() != Some(self.modal_effects(this)) {
            self.inner.layer.update_modal(this, window, cx);
        }
    }

    /// `open` changed, from any source: the layer shows or animates out.
    /// Changes the popup makes call it themselves; a component that writes
    /// `open` another way calls it after.
    pub fn open_changed(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let open = self.is_open();
        if open != self.inner.was_open.replace(open) && open {
            let key = self.trigger_key();
            let bound = self.bound_triggers();
            *self.inner.gone_at_open.borrow_mut() = key.filter(|key| !bound.contains(key));
            self.inner.only_trigger.set(Some(bound.len() <= 1));
        }
        self.inner.layer.open_changed(this, open, window, cx);
    }

    fn change_open(&self, this: &mut T, open: bool, change: OpenChange, window: &mut Window, cx: &mut Context<T>) -> bool {
        *self.inner.return_value.borrow_mut() = change.return_value.clone();
        let before = self.is_open();
        let changed = {
            let instance = this.instance();
            self.inner
                .open
                .borrow_mut()
                .change(instance, open, change.options(), window, cx)
        };
        *self.inner.return_value.borrow_mut() = None;
        if self.is_open() != before {
            self.open_changed(this, window, cx);
        }
        changed
    }

    fn layer_open_change(
        &self,
        this: &mut T,
        open: bool,
        closed: Option<&PopupClose>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if let Some(rule) = self.inner.rules.on_open_change.clone() {
            rule(this, open, closed, window, cx);
        }
        if let Some(group) = self.group() {
            group.change(open, window, cx);
        }
        if open {
            if let Some(presentation) = self.presentation() {
                presentation.open(this, window);
            }
            self.update_modal_value();
            self.bind_content(this, window, cx);
            return;
        }
        if let Some(hover) = self.hover() {
            hover.clear();
        }
        if let Some(focus) = self.focus_opening() {
            focus.clear();
        }
    }

    // A dismissal the popup layer saw, like Escape or an outside press.
    fn dismiss(&self, this: &mut T, reason: ui_sdk_kernel::Reason, details: CloseDetails, window: &mut Window, cx: &mut Context<T>) {
        if let Some(dismiss) = self.inner.rules.dismiss.clone() {
            return dismiss(this, reason, details, window, cx);
        }
        self.control().clone().hide(
            this,
            HideRequest {
                reason: Some(reason),
                event: details.event,
                source: None,
                focus: Some(details.focus),
                return_value: details.return_value,
            },
            window,
            cx,
        );
    }

    /// How the content binds: placed against a controller's anchor, the
    /// content's, or the trigger, with focus moving in when a press opens
    /// it, and as a sheet when it presents as one (`bindPopupContent`).
    pub fn content_config(&self) -> SheetConfig<T> {
        let weak = self.weak();
        let props = self.content_props();
        let inline = props.inline;
        let preset = self.inner.preset;
        let anchor = {
            let weak = weak.clone();
            Rc::new(move || {
                let popup = weak.upgrade()?;
                popup.control().anchor().or_else(|| {
                    content_anchor(popup.content_props().position.anchor.clone(), || {
                        popup.trigger_element().map(Anchor::element)
                    })
                })
            }) as Rc<dyn Fn() -> Option<Anchor>>
        };
        let position = (preset.placement && !inline).then(|| {
            let weak = weak.clone();
            Rc::new(move || {
                let Some(popup) = weak.upgrade() else {
                    return ui_sdk_platform::PositionOptions::default();
                };
                let arrow = {
                    let popup = popup.weak();
                    ElementBounds::new(move || {
                        let popup = popup.upgrade()?;
                        if popup.slot_bound("arrow") {
                            popup.inner.arrow.get()
                        } else {
                            None
                        }
                    })
                };
                placement_options(
                    &popup.content_props().position,
                    PlacementDefaults::new(Align::Center).side(preset.side),
                    Some(arrow),
                )
            }) as Rc<dyn Fn() -> ui_sdk_platform::PositionOptions>
        });
        // Where focus goes once the popup and its children have rendered.
        // Hover content takes none.
        let initial_focus = self.inner.press.map(|_| {
            let weak = weak.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let popup = weak.upgrade()?;
                let setting = popup.content_props().initial_focus;
                let preferred = popup.inner.rules.preferred_focus.clone();
                resolve_initial_focus(&popup.content_region(), &setting, preferred.as_ref(), window, cx)
            }) as InitialFocusFn
        });
        let backdrop = {
            let weak = weak.clone();
            Rc::new(move || {
                let popup = weak.upgrade()?;
                popup.slot_bound("backdrop").then(|| popup.backdrop_region())
            }) as RegionFn
        };
        let viewport = self.viewport_region();
        SheetConfig {
            attach: AttachConfig {
                position,
                anchor: Some(anchor),
                initial_focus,
                backdrop: Some(backdrop),
                host: Some(Rc::new(move || Some(viewport.clone()))),
                inline,
                ..AttachConfig::default()
            },
            ..SheetConfig::default()
        }
    }

    // Binds the content as the popup opens, with how it presents this time.
    fn bind_content(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        if let Some(on_bind) = self.inner.rules.on_bind.clone() {
            on_bind(this, window, cx);
        }
        let mut config = self.content_config();
        if let Some(bind) = self.inner.rules.bind.clone() {
            config = config.merge(bind(this));
        }
        let region = self.content_region();
        match self.presentation().cloned() {
            Some(presentation) => presentation.bind(this, &self.inner.layer, region, config, window, cx),
            None => self.inner.layer.bind(this, region, config.attach, window, cx),
        }
    }

    // The content rendered again after it left, or for the first time open
    // from the start: it binds, and shows while mounted.
    pub(crate) fn content_bound(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        if self.inner.layer.is_shown() || !self.inner.layer.is_mounted() || self.inner.content.live.get() == 0 {
            return;
        }
        self.bind_content(this, window, cx);
    }

    // The content left the page: unbound, and focus inside it goes back.
    // Then the component lets go of what it bound to it.
    fn content_left(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        if self.inner.content.live.get() > 0 || !self.inner.content.present.replace(false) {
            return;
        }
        let region = self.content_region();
        self.inner.layer.unbind(this, &region, window, cx);
        if let Some(on_unbind) = self.inner.rules.on_unbind.clone() {
            on_unbind(this, window, cx);
        }
    }

    /// Opens or closes it. Returns false when the change was canceled or it
    /// was already open or closed (`setOpen`).
    pub fn set_open(&self, this: &mut T, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<T>) -> bool {
        self.control().clone().set_open(this, open, options, window, cx)
    }

    /// Opens it, and resolves once it closes, with why, how and any return
    /// value (`open`). Resolves exactly once and never fails: on the next
    /// close from any source, when the signal aborts, or with `Unmount` when
    /// the component leaves.
    pub fn open(&self, this: &mut T, options: PopupOpenOptions<T::Payload>, window: &mut Window, cx: &mut Context<T>) -> PopupPending {
        self.control().clone().open(this, options, window, cx)
    }

    /// Closes it. `return_value` is passed to the future `open()` returned.
    /// Returns false when the close was canceled or it was already closed
    /// (`close`).
    pub fn close(&self, this: &mut T, options: PopupCloseOptions, window: &mut Window, cx: &mut Context<T>) -> bool {
        self.control().clone().close(this, options, window, cx)
    }

    /// Opens it when it's closed, and closes it when it's open. Returns false
    /// when the change was canceled (`toggle`).
    pub fn toggle(&self, this: &mut T, options: ChangeOptions, window: &mut Window, cx: &mut Context<T>) -> bool {
        self.control().clone().toggle(this, options, window, cx)
    }

    /// Moves focus to the trigger that opened it last, or the first trigger
    /// (`focus`).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(handle) = self.trigger_element().and_then(|trigger| trigger.focus_handle()) {
            window.focus(&handle, cx);
        }
    }
}

/// A popup's methods on its component's controller (`PopupController`):
/// calls before the component attaches, and once it's gone, do nothing and
/// return `None`. Call them outside the component's own update.
pub trait PopupControllerExt<T: PopupHost> {
    /// Whether it's open.
    fn is_open(&self, cx: &App) -> bool;
    /// Opens or closes it.
    fn set_open(&self, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut App) -> Option<bool>;
    /// Opens it, and resolves once it closes.
    fn open(&self, options: PopupOpenOptions<T::Payload>, window: &mut Window, cx: &mut App) -> Option<PopupPending>;
    /// Closes it.
    fn close(&self, options: PopupCloseOptions, window: &mut Window, cx: &mut App) -> Option<bool>;
    /// Opens it when it's closed, and closes it when it's open.
    fn toggle(&self, options: ChangeOptions, window: &mut Window, cx: &mut App) -> Option<bool>;
    /// The payload of the last open.
    fn payload(&self, cx: &App) -> Option<T::Payload>;
    /// Moves focus to its trigger.
    fn focus(&self, window: &mut Window, cx: &mut App);
}

impl<T: PopupHost> PopupControllerExt<T> for Controller<T> {
    fn is_open(&self, cx: &App) -> bool {
        self.read(cx, |this, _| this.popup().is_open()).unwrap_or(false)
    }

    fn set_open(&self, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut App) -> Option<bool> {
        self.update_in(window, cx, |this, window, cx| {
            let popup = this.popup().clone();
            popup.set_open(this, open, options, window, cx)
        })
    }

    fn open(&self, options: PopupOpenOptions<T::Payload>, window: &mut Window, cx: &mut App) -> Option<PopupPending> {
        self.update_in(window, cx, |this, window, cx| {
            let popup = this.popup().clone();
            popup.open(this, options, window, cx)
        })
    }

    fn close(&self, options: PopupCloseOptions, window: &mut Window, cx: &mut App) -> Option<bool> {
        self.update_in(window, cx, |this, window, cx| {
            let popup = this.popup().clone();
            popup.close(this, options, window, cx)
        })
    }

    fn toggle(&self, options: ChangeOptions, window: &mut Window, cx: &mut App) -> Option<bool> {
        self.update_in(window, cx, |this, window, cx| {
            let popup = this.popup().clone();
            popup.toggle(this, options, window, cx)
        })
    }

    fn payload(&self, cx: &App) -> Option<T::Payload> {
        self.read(cx, |this, _| this.popup().payload()).flatten()
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self
            .read(cx, |this, _| this.popup().trigger_element())
            .flatten()
            .and_then(|trigger| trigger.focus_handle());
        if let Some(handle) = handle {
            window.focus(&handle, cx);
        }
    }
}
