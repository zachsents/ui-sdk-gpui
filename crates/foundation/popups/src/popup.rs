//! Port of `foundation/popups/popup.ts`: the popup layer every overlay
//! shares: presence, the top layer, positioning, dismissal, modal side
//! effects, Tab handling, and where focus goes on open and close. A component
//! makes a [`PopupLayer`] once and binds its content to it, then adds only
//! what's specific to it. [`create_popup`](fn@crate::create_popup) does that for
//! most popups.
//!
//! The original reads a DOM element for each of these. Here the content is a
//! [`Region`], which knows where its element laid out and the focus handle it
//! tracks, and its element renders through the layer: inside `deferred(..)`
//! at [`LAYER_PRIORITY`](ui_sdk_platform::LAYER_PRIORITY), placed by a
//! [`Positioner`] the layer makes as it shows, with the layer's
//! [`listener`](ui_sdk_platform::listener) painted every frame. The layer's
//! state is the component's, so its methods take the component, `this`, and
//! its context, like the drag and drop behaviors.
//!
//! A modal popup can't make the rest of the window inert, as gpui has no such
//! state, so the layer paints a blocker beneath the popup: a hitbox over the
//! whole window, with holes for what stays usable, that takes every press,
//! hover and scroll from what's under it. That is the original's inert page
//! and its scroll lock at once.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::task::Poll;
use std::time::Duration;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::Shared;
use gpui::{
    AnyWindowHandle, App, BackGestureEvent, BackGesturePhase, Context, DispatchPhase,
    ElementId, FocusHandle, IntoElement, KeyDownEvent, MouseDownEvent, Pixels, Point,
    SharedString, Styled, Subscription, Task, WeakEntity, WeakFocusHandle, Window, canvas,
};
use ui_sdk_focus::{next_tabbable, tab_within, tabbables};
use ui_sdk_kernel::focus_events::on_focus_change;
use ui_sdk_kernel::modality::{move_focus, restore_focus, track_input, window_source};
use ui_sdk_kernel::{ChangeOptions, ChangeSource, Controller, HasPopup, InputEvent, Key, Reason};
use ui_sdk_motion::Presence;
use ui_sdk_platform::{
    AfterPaint, Anchor, Layer, LayerOptions, OutsidePressEvent, PositionOptions, Positioner,
    PressKind, PushedLayer, Region, ReturnTarget, Target, active_layers, after_paint,
    escape_layer, find_layer, in_persistent_layer, last_closed, last_escape, layer_contains,
    layer_count, mark_closed, opened_since, position, push_layer,
};

use crate::popup_behavior::{
    PopupBehavior, PopupBehaviorName, PopupBehaviorOverrides, PopupBlur, PopupTab, resolve_behavior,
};

/// How a popup was last opened. Decides where focus goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PopupOpener {
    Pointer,
    Touch,
    Keyboard,
    Hover,
    #[default]
    Imperative,
    Initial,
}

impl PopupOpener {
    /// The original's name for it, like `"pointer"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PopupOpener::Pointer => "pointer",
            PopupOpener::Touch => "touch",
            PopupOpener::Keyboard => "keyboard",
            PopupOpener::Hover => "hover",
            PopupOpener::Imperative => "imperative",
            PopupOpener::Initial => "initial",
        }
    }
}

/// Where focus goes. `Trigger` follows the fallback chain: the trigger, the
/// element focused before opening, the trigger of the popup it was opened
/// from, then the nearest element that survived. `None` leaves focus alone. A
/// focus handle, or a function that returns one, picks a specific element.
#[derive(Clone, Default)]
pub enum PopupFocusTarget {
    #[default]
    Trigger,
    None,
    Handle(FocusHandle),
    Fn(Rc<dyn Fn(&Window, &App) -> Option<FocusHandle>>),
}

impl std::fmt::Debug for PopupFocusTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PopupFocusTarget::Trigger => write!(f, "Trigger"),
            PopupFocusTarget::None => write!(f, "None"),
            PopupFocusTarget::Handle(handle) => f.debug_tuple("Handle").field(handle).finish(),
            PopupFocusTarget::Fn(_) => write!(f, "Fn(..)"),
        }
    }
}

impl PopupFocusTarget {
    /// A function that picks the element to focus.
    pub fn function(pick: impl Fn(&Window, &App) -> Option<FocusHandle> + 'static) -> Self {
        PopupFocusTarget::Fn(Rc::new(pick))
    }
}

impl From<FocusHandle> for PopupFocusTarget {
    fn from(handle: FocusHandle) -> Self {
        PopupFocusTarget::Handle(handle)
    }
}

/// What being modal means. `True` blocks the rest of the window, so it can't
/// be pressed, hovered or scrolled, traps focus and lets an outside press only
/// close the popup. `False` does none of these. The others keep exactly one
/// effect: `Pointer` keeps outside presses from reaching the window, and
/// `TrapFocus` keeps the focus trap. Each component accepts a documented
/// subset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Modal {
    #[default]
    False,
    True,
    Pointer,
    TrapFocus,
}

impl Modal {
    /// The original's value, like `"trap-focus"`, or `"true"` and `"false"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Modal::False => "false",
            Modal::True => "true",
            Modal::Pointer => "pointer",
            Modal::TrapFocus => "trap-focus",
        }
    }

    /// Whether it's `True`, every effect at once.
    pub fn is_true(self) -> bool {
        self == Modal::True
    }
}

impl From<bool> for Modal {
    fn from(modal: bool) -> Self {
        if modal { Modal::True } else { Modal::False }
    }
}

/// What being modal means, as three separate effects. Components map their
/// `modal` option onto it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PopupModal {
    /// An outside press only closes the popup and reaches nothing else.
    pub consume_outside_press: bool,
    /// Tab wraps inside the popup.
    pub trap_focus: bool,
    /// The rest of the window is blocked and doesn't scroll.
    pub inert_and_lock: bool,
}

/// Every `modal` value as a point in the triple (`modalTriple`).
pub fn modal_triple(modal: Modal) -> PopupModal {
    PopupModal {
        consume_outside_press: matches!(modal, Modal::True | Modal::Pointer),
        trap_focus: matches!(modal, Modal::True | Modal::TrapFocus),
        inert_and_lock: modal == Modal::True,
    }
}

/// How each input source opens a popup. A screen reader's virtual click
/// moves focus in like a key, and a call from code opens it like code.
pub fn opener_by_source(source: ChangeSource) -> PopupOpener {
    match source {
        ChangeSource::Mouse | ChangeSource::Pen => PopupOpener::Pointer,
        ChangeSource::Touch => PopupOpener::Touch,
        ChangeSource::Keyboard | ChangeSource::Virtual => PopupOpener::Keyboard,
        ChangeSource::Programmatic | ChangeSource::Agent => PopupOpener::Imperative,
    }
}

/// How the event that opened a popup opens it, `Imperative` without one
/// (`openerOf`).
pub fn opener_of(event: Option<&InputEvent>) -> PopupOpener {
    event.map_or(PopupOpener::Imperative, |event| opener_by_source(event.source()))
}

/// Where focus goes on open, by opener.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InitialFocusPlan {
    /// Focus stays where it is.
    None,
    /// The popup itself takes focus.
    Popup,
    /// The component decides.
    Auto,
}

/// Where focus goes on open, by opener (`initialFocusPlan`). Hover and an
/// open from the start move no focus. Touch focuses the popup itself, so no
/// on-screen keyboard appears. Anything else asks the component.
pub fn initial_focus_plan(opener: PopupOpener) -> InitialFocusPlan {
    match opener {
        PopupOpener::Initial | PopupOpener::Hover => InitialFocusPlan::None,
        PopupOpener::Touch => InitialFocusPlan::Popup,
        _ => InitialFocusPlan::Auto,
    }
}

/// Where focus goes on the next close. `Return` returns it, `Leave` leaves
/// it, `Later` waits for the press to end, and `UnlessMoved` waits for the
/// update to end and skips the return when another layer opened or took
/// focus meanwhile, like a dialog an item opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ReturnMode {
    Leave,
    #[default]
    Return,
    Later,
    UnlessMoved,
}

impl From<bool> for ReturnMode {
    fn from(returns: bool) -> Self {
        if returns { ReturnMode::Return } else { ReturnMode::Leave }
    }
}

/// Whether focus returns after an outside press (`outsidePressFocus`). Focus
/// stays on a focusable element the press reached. A mouse press waits until
/// it's over, because its own focus change would undo the return. A tap
/// dismisses on its release, after that change, so it returns at once.
pub fn outside_press_focus(kind: PressKind, focusable: bool, consumed: bool) -> ReturnMode {
    if focusable && !consumed {
        return ReturnMode::Leave;
    }
    if kind == PressKind::PointerDown {
        ReturnMode::Later
    } else {
        ReturnMode::Return
    }
}

/// The first of `candidates` that gives a value `usable` accepts
/// (`firstUsable`).
pub fn first_usable<V>(
    candidates: impl IntoIterator<Item = Option<V>>,
    mut usable: impl FnMut(&V) -> bool,
) -> Option<V> {
    candidates
        .into_iter()
        .flatten()
        .find(|value| usable(value))
}

/// How a popup closed, which a controller's `open()` resolves with
/// (`PopupResult`).
#[derive(Clone, Debug, PartialEq)]
pub struct PopupResult {
    /// Why it closed, like `EscapeKey` or `ClosePress`. `Unmount` when the
    /// component left while open, and `Imperative` when the signal aborted.
    pub reason: Reason,
    /// How the user closed it: `Mouse`, `Touch`, `Pen`, `Keyboard`,
    /// `Virtual`, `Agent` or `Programmatic`.
    pub source: ChangeSource,
    /// What closed it reported: a Close part's `value`, or `close`'s own.
    pub return_value: Option<SharedString>,
}

/// Why a popup closed: the close its component requested, or `Imperative`
/// for one nobody requested, like a controlled `open` (`PopupClose`).
pub type PopupClose = PopupResult;

/// Why a popup asks its component to close: a dismissal its behavior allows,
/// an aborted signal, or a sheet swiped away (`PopupCloseReason`): one of
/// `OutsidePress`, `EscapeKey`, `WindowBlur`, `FocusOut`, `Back`,
/// `ClosePress`, `Imperative` and `Swipe`.
pub type PopupCloseReason = Reason;

/// What `open()` resolves with once the popup closes: a future that
/// resolves exactly once and never fails. Clones resolve together.
#[derive(Clone)]
pub struct PopupPending(Shared<Pin<Box<dyn Future<Output = PopupResult>>>>);

impl PopupPending {
    pub(crate) fn channel() -> (oneshot::Sender<PopupResult>, PopupPending) {
        let (sender, receiver) = oneshot::channel::<PopupResult>();
        let future: Pin<Box<dyn Future<Output = PopupResult>>> = Box::pin(receiver.map(|result| {
            // Gone without a close: the component left.
            result.unwrap_or(PopupResult {
                reason: Reason::Unmount,
                source: ChangeSource::Programmatic,
                return_value: None,
            })
        }));
        (sender, PopupPending(future.shared()))
    }

    /// A result there already, like for an open that was refused.
    pub fn ready(result: PopupResult) -> Self {
        let future: Pin<Box<dyn Future<Output = PopupResult>>> = Box::pin(futures::future::ready(result));
        PopupPending(future.shared())
    }

    /// The result, once it's there.
    pub fn result(&self) -> Option<PopupResult> {
        self.0.peek().cloned()
    }
}

impl Future for PopupPending {
    type Output = PopupResult;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<PopupResult> {
        self.0.poll_unpin(cx)
    }
}

impl std::fmt::Debug for PopupPending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PopupPending").field(&self.result()).finish()
    }
}

type AbortListener = Box<dyn FnOnce(&mut App)>;

#[derive(Default)]
struct SignalInner {
    aborted: Cell<bool>,
    next: Cell<u64>,
    listeners: RefCell<Vec<(u64, AbortListener)>>,
    timer: RefCell<Option<Task<()>>>,
}

/// Closes the popup an open was passed it to, with reason `Imperative`,
/// when it aborts, like `AbortSignal`. An already aborted signal never opens
/// one. Clones are the same signal.
#[derive(Clone, Default)]
pub struct AbortSignal(Rc<SignalInner>);

impl std::fmt::Debug for AbortSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbortSignal")
            .field("aborted", &self.is_aborted())
            .finish()
    }
}

impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// A signal that aborts after `duration`, like `AbortSignal.timeout`.
    pub fn timeout(duration: Duration, cx: &mut App) -> Self {
        let signal = Self::new();
        let weak = Rc::downgrade(&signal.0);
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(duration).await;
            cx.update(|cx| {
                if let Some(inner) = weak.upgrade() {
                    AbortSignal(inner).abort(cx);
                }
            });
        });
        *signal.0.timer.borrow_mut() = Some(task);
        signal
    }

    /// Aborts it. What listens hears it once the current update is over.
    pub fn abort(&self, cx: &mut App) {
        if self.0.aborted.replace(true) {
            return;
        }
        let listeners = std::mem::take(&mut *self.0.listeners.borrow_mut());
        cx.defer(move |cx| {
            for (_, listener) in listeners {
                listener(cx);
            }
        });
    }

    pub fn is_aborted(&self) -> bool {
        self.0.aborted.get()
    }

    pub(crate) fn on_abort(&self, listener: impl FnOnce(&mut App) + 'static) -> u64 {
        let id = self.0.next.get() + 1;
        self.0.next.set(id);
        self.0.listeners.borrow_mut().push((id, Box::new(listener)));
        id
    }

    pub(crate) fn off(&self, id: u64) {
        self.0.listeners.borrow_mut().retain(|(listener, _)| *listener != id);
    }
}

/// What every popup's `open` takes from a controller (`PopupOpenOptions`).
pub struct PopupOpenOptions<P> {
    pub reason: Option<Reason>,
    pub event: Option<InputEvent>,
    pub source: Option<ChangeSource>,
    /// Data for the content to render, like the row a menu acts on. Kept until
    /// the next open, so it stays put while the popup animates out. On an open
    /// popup it replaces the current payload, like another trigger taking it
    /// over, and an `anchor` passed with it moves the popup. `Some(None)`
    /// clears it.
    pub payload: Option<Option<P>>,
    /// Positions the popup against this instead of the trigger: an element,
    /// a point, or a rectangle.
    pub anchor: Option<Anchor>,
    /// Closes the popup, with reason `Imperative`, when it aborts. An already
    /// aborted signal never opens it.
    pub signal: Option<AbortSignal>,
}

impl<P> Default for PopupOpenOptions<P> {
    fn default() -> Self {
        Self {
            reason: None,
            event: None,
            source: None,
            payload: None,
            anchor: None,
            signal: None,
        }
    }
}

impl<P> Clone for PopupOpenOptions<P>
where
    P: Clone,
{
    fn clone(&self) -> Self {
        Self {
            reason: self.reason,
            event: self.event.clone(),
            source: self.source,
            payload: self.payload.clone(),
            anchor: self.anchor.clone(),
            signal: self.signal.clone(),
        }
    }
}

impl<P> PopupOpenOptions<P> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reason(mut self, reason: Reason) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn event(mut self, event: impl Into<InputEvent>) -> Self {
        self.event = Some(event.into());
        self
    }

    pub fn source(mut self, source: ChangeSource) -> Self {
        self.source = Some(source);
        self
    }

    pub fn payload(mut self, payload: impl Into<Option<P>>) -> Self {
        self.payload = Some(payload.into());
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.anchor = Some(anchor.into());
        self
    }

    pub fn signal(mut self, signal: AbortSignal) -> Self {
        self.signal = Some(signal);
        self
    }

    /// The change options it carries.
    pub fn change(&self) -> ChangeOptions {
        ChangeOptions {
            reason: self.reason,
            source: self.source,
            event: self.event.clone(),
        }
    }
}

/// What a popup's `close` takes: why, and what `open()` resolves with.
#[derive(Clone, Debug, Default)]
pub struct PopupCloseOptions {
    pub reason: Option<Reason>,
    pub event: Option<InputEvent>,
    pub source: Option<ChangeSource>,
    /// Passed to the future `open()` returned.
    pub return_value: Option<SharedString>,
}

impl PopupCloseOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reason(mut self, reason: Reason) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn event(mut self, event: impl Into<InputEvent>) -> Self {
        self.event = Some(event.into());
        self
    }

    pub fn source(mut self, source: ChangeSource) -> Self {
        self.source = Some(source);
        self
    }

    pub fn return_value(mut self, value: impl Into<SharedString>) -> Self {
        self.return_value = Some(value.into());
        self
    }
}

impl From<ChangeOptions> for PopupCloseOptions {
    fn from(options: ChangeOptions) -> Self {
        Self {
            reason: options.reason,
            event: options.event,
            source: options.source,
            return_value: None,
        }
    }
}

/// Opens, closes and inspects a popup from outside it (`PopupController`).
/// Components wrap it in their own `create_<name>_controller()`; its popup
/// methods are [`PopupControllerExt`](crate::PopupControllerExt)'s.
pub type PopupController<T> = Controller<T>;

/// A popup controller, attached by the component it's passed to
/// (`createPopupController`).
pub fn create_popup_controller<T: 'static>() -> Controller<T> {
    Controller::new()
}

/// The state that changes with a controlled `open`, in the same pass as it,
/// like the highlighted item a closing menu clears (`deriveOpen`): `derive`
/// gets the arriving `open` when it differs from the one before.
pub fn derive_open<R>(arriving: Option<bool>, before: bool, derive: impl FnOnce(bool) -> R) -> Option<R> {
    ui_sdk_kernel::derive_from_options(arriving.as_ref(), &before, |open, _| derive(*open))
}

/// The ARIA every popup trigger carries (`popupTriggerAttrs`). Only the kind
/// of popup differs. With several triggers, only the one that opened the
/// popup is expanded, controls it and names it.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupTriggerAria {
    /// Whether this trigger opened the popup last, or no trigger has.
    pub active: bool,
    /// `aria-expanded`, and its `data-open`.
    pub open: bool,
    pub has_popup: Option<HasPopup>,
    /// `aria-controls`: the content, while it's in the page.
    pub controls: Option<ElementId>,
}

/// The trigger ARIA for `key`, from the popup's state: whether it's open and
/// mounted and which trigger opened it, and the content's element id.
pub fn popup_trigger_attrs(
    open: bool,
    mounted: bool,
    trigger: Option<&str>,
    content: ElementId,
    has_popup: Option<HasPopup>,
    key: &str,
) -> PopupTriggerAria {
    let active = trigger.is_none_or(|trigger| trigger == key);
    PopupTriggerAria {
        active,
        open: open && active,
        has_popup,
        controls: (active && mounted).then_some(content),
    }
}

/// How a disabled trigger shows it (`disabledTriggerAttrs`): one that stays
/// focusable says so with `aria-disabled`, and one that doesn't leaves the
/// tab order, as a disabled native button does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisabledTriggerAttrs {
    /// Out of the tab order and unfocusable, as the native `disabled`.
    pub unfocusable: bool,
    /// `aria-disabled`, or the native `disabled`'s state.
    pub disabled: bool,
    /// `data-disabled`.
    pub data_disabled: bool,
}

pub fn disabled_trigger_attrs(disabled: bool, focusable_when_disabled: bool) -> DisabledTriggerAttrs {
    DisabledTriggerAttrs {
        unfocusable: disabled && !focusable_when_disabled,
        disabled,
        data_disabled: disabled,
    }
}

/// What every popup's content carries (`popupContentAttrs`): focusable but
/// no tab stop, the original's `tabindex="-1"`, hidden while not mounted, and
/// open or closed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PopupContentAttrs {
    /// The original's `hidden`: kept in the page while closed, it renders
    /// hidden.
    pub hidden: bool,
    /// `data-open`, or `data-closed`.
    pub open: bool,
}

pub fn popup_content_attrs(open: bool, mounted: bool) -> PopupContentAttrs {
    PopupContentAttrs {
        hidden: !mounted,
        open,
    }
}

/// Runs after the popup shows, with its layer. Returns what undoes it once
/// it hides.
pub type ShowCleanup<T> = Box<dyn FnOnce(&mut T, &mut Window, &mut Context<T>)>;

/// Runs as the popup shows, with its layer, inside the component's update.
pub type ShowFn<T> = Rc<dyn Fn(&mut T, &Layer, &mut Window, &mut Context<T>) -> Option<ShowCleanup<T>>>;

/// Runs inside the component's update.
pub type HookFn<T> = Rc<dyn Fn(&mut T, &mut Window, &mut Context<T>)>;

/// Picks the element focus goes to as the popup opens, once its content has
/// rendered.
pub type InitialFocusFn = Rc<dyn Fn(&mut Window, &mut App) -> Option<FocusHandle>>;

/// Reads a region that may change, looked up live.
pub type RegionFn = Rc<dyn Fn() -> Option<Region>>;

/// Whether a target counts as inside: a point is a press, a focus handle is
/// focus. A press comes with its event, the mouse down, its release, or a
/// tap's touch click, and focus with none (`contains(target, event)`).
pub type ContainsFn = Rc<dyn Fn(&Target, Option<&InputEvent>, &Window, &App) -> bool>;

/// How a popup's content attaches as it shows (`AttachConfig`).
pub struct AttachConfig<T: 'static> {
    /// Where the popup goes beside its anchor, read on every frame. Needs the
    /// layer's `position`.
    pub position: Option<Rc<dyn Fn() -> PositionOptions>>,
    /// What to position against, read on every frame. Defaults to the
    /// trigger.
    pub anchor: Option<Rc<dyn Fn() -> Option<Anchor>>>,
    /// Where focus goes on open, once the content has rendered.
    pub initial_focus: Option<InitialFocusFn>,
    /// A part shown just below the popup, like a dialog's backdrop, while it
    /// renders. It animates in and out with the popup, and pressing it is an
    /// outside press.
    pub backdrop: Option<RegionFn>,
    /// The element the popup sits in, when the popup isn't one of its own: a
    /// full-window element, like a sheet's viewport.
    pub host: Option<RegionFn>,
    /// Stays where it renders instead of going above the window, with its
    /// backdrop, like a submenu shown in place of its parent's items inside a
    /// sheet.
    pub inline: bool,
    /// Runs when the popup shows, with its layer, and returns what undoes it
    /// once it hides. A sheet rises here.
    pub on_show: Option<ShowFn<T>>,
    /// Runs as a close starts, before the exit is awaited, so an animation it
    /// starts counts toward it. A sheet settles closed here.
    pub on_close: Option<HookFn<T>>,
}

impl<T: 'static> Default for AttachConfig<T> {
    fn default() -> Self {
        Self {
            position: None,
            anchor: None,
            initial_focus: None,
            backdrop: None,
            host: None,
            inline: false,
            on_show: None,
            on_close: None,
        }
    }
}

impl<T: 'static> Clone for AttachConfig<T> {
    fn clone(&self) -> Self {
        Self {
            position: self.position.clone(),
            anchor: self.anchor.clone(),
            initial_focus: self.initial_focus.clone(),
            backdrop: self.backdrop.clone(),
            host: self.host.clone(),
            inline: self.inline,
            on_show: self.on_show.clone(),
            on_close: self.on_close.clone(),
        }
    }
}

/// What a layer passes when it asks its component to close.
#[derive(Clone, Debug, Default)]
pub struct CloseDetails {
    pub event: Option<InputEvent>,
    /// Whether focus should return.
    pub focus: ReturnMode,
    pub return_value: Option<SharedString>,
}

/// Asks the component to close, inside its update.
pub type CloseFn<T> = Rc<dyn Fn(&mut T, Reason, CloseDetails, &mut Window, &mut Context<T>)>;

/// Hears an open or a close, with why it closed when the popup's control
/// says, inside the component's update.
pub type OpenChangeFn<T> = Rc<dyn Fn(&mut T, bool, Option<&PopupClose>, &mut Window, &mut Context<T>)>;

/// Escape on the topmost layer, inside the component's update.
pub type EscapeFn<T> = Rc<dyn Fn(&mut T, &KeyDownEvent, &mut Window, &mut Context<T>)>;

/// Hears every open and close first and returns why a close happened.
pub type ObserveFn = Rc<dyn Fn(bool) -> Option<PopupClose>>;

/// What [`PopupLayer::new`] takes (`createPopupLayer`'s options).
pub struct PopupLayerOptions<T: 'static> {
    /// The behavior to start from.
    pub preset: PopupBehaviorName,
    /// Where this popup differs from its preset.
    pub behavior: PopupBehaviorOverrides,
    /// The component and instance, for the layer stack the devtools show.
    pub owner: Option<(SharedString, SharedString)>,
    /// The trigger, looked up live, so one that rendered anew still works.
    pub trigger: RegionFn,
    /// Where focus returns on close, when that isn't always `trigger`.
    pub return_trigger: Option<RegionFn>,
    /// Asks the component to close. `focus` says whether focus should return.
    pub close: CloseFn<T>,
    /// Read when the popup shows.
    pub modal: Rc<dyn Fn(&T) -> PopupModal>,
    /// The layer this one nests under. Without one, the open popup that
    /// contains the trigger.
    pub parent: Option<Rc<dyn Fn(&T) -> Option<Layer>>>,
    /// Whether a press on something outside the popup, or focus there,
    /// counts as inside, so it doesn't dismiss, with the press being decided.
    /// By default, the trigger does.
    pub contains: Option<ContainsFn>,
    /// Where focus goes on close.
    pub final_focus: Option<Rc<dyn Fn() -> Option<PopupFocusTarget>>>,
    /// Runs on open before the popup shows, and on close before it animates
    /// out, with why it closed when the popup's control says.
    pub on_open_change: Option<OpenChangeFn<T>>,
    /// Runs once opening or closing has finished, after any animation. Never
    /// for a popup open from the start.
    pub on_open_change_complete: Option<OpenChangeFn<T>>,
    /// Escape when this is the topmost layer. Closes by default.
    pub on_escape: Option<EscapeFn<T>>,
    /// Runs when the popup shows, with its layer.
    pub on_show: Option<ShowFn<T>>,
    /// Regions that stay usable while the popup blocks the window, like a
    /// chat launcher. Presses on them never dismiss it. Read when it shows.
    pub persistent: Option<Rc<dyn Fn(&T) -> Vec<Region>>>,
    /// Whether the popup is placed beside an anchor, with a [`Positioner`].
    pub position: bool,
}

impl<T: 'static> PopupLayerOptions<T> {
    /// Options for a layer of `preset`, with its trigger and how it closes.
    pub fn new(
        preset: PopupBehaviorName,
        trigger: impl Fn() -> Option<Region> + 'static,
        close: impl Fn(&mut T, Reason, CloseDetails, &mut Window, &mut Context<T>) + 'static,
        modal: impl Fn(&T) -> PopupModal + 'static,
    ) -> Self {
        Self {
            preset,
            behavior: PopupBehaviorOverrides::default(),
            owner: None,
            trigger: Rc::new(trigger),
            return_trigger: None,
            close: Rc::new(close),
            modal: Rc::new(modal),
            parent: None,
            contains: None,
            final_focus: None,
            on_open_change: None,
            on_open_change_complete: None,
            on_escape: None,
            on_show: None,
            persistent: None,
            position: false,
        }
    }
}

// What stays usable while a modal popup blocks the window.
#[derive(Default)]
pub(crate) struct Blocker {
    pub(crate) frozen: bool,
    pub(crate) keep: Vec<Region>,
    pub(crate) extra: Vec<(u64, Region)>,
    next: u64,
}

impl Blocker {
    pub(crate) fn kept(&self) -> Vec<Region> {
        self.keep
            .iter()
            .cloned()
            .chain(self.extra.iter().map(|(_, region)| region.clone()))
            .collect()
    }
}

// The tab stops around where the trigger was, so focus can go to the nearest
// one that survived when the trigger leaves (`path`).
#[derive(Clone, Default)]
struct Neighbours {
    trigger: Option<FocusHandle>,
    next: Option<FocusHandle>,
    previous: Option<FocusHandle>,
}

struct Shown<T: 'static> {
    el: Region,
    config: AttachConfig<T>,
    backdrop: Option<Region>,
    layer: Layer,
    _pushed: PushedLayer,
    positioner: Option<Positioner>,
    modal: PopupModal,
    persistent: Vec<Region>,
    trigger: Option<Region>,
    pending_freeze: Option<AfterPaint>,
    cleanups: Vec<ShowCleanup<T>>,
    releases: Vec<Box<dyn FnOnce()>>,
    _subscriptions: Vec<Subscription>,
    watcher: Option<u64>,
}

struct LayerState<T: 'static> {
    was_open: bool,
    open: bool,
    mounted: bool,
    bound: Option<(Region, AttachConfig<T>)>,
    shown: Option<Shown<T>>,
    exiting: Option<u64>,
    exit_run: u64,
    opener: PopupOpener,
    focus_pending: bool,
    focus_scheduled: bool,
    enter_pending: bool,
    opened_on_press: bool,
    opened_by_key: bool,
    return_mode: ReturnMode,
    before: Option<FocusHandle>,
    opened_without_trigger: bool,
    upstream: ReturnTarget,
    neighbours: Neighbours,
    observer: Option<ObserveFn>,
    last_press: Option<(MouseDownEvent, std::time::Instant)>,
}

struct LayerInner<T: 'static> {
    options: PopupLayerOptions<T>,
    behavior: PopupBehavior,
    entity: WeakEntity<T>,
    window: AnyWindowHandle,
    presence: Presence,
    backdrop_presence: Presence,
    blocker: Rc<RefCell<Blocker>>,
    state: RefCell<LayerState<T>>,
    resolving: Cell<bool>,
    me: Weak<LayerInner<T>>,
}

/// The popup layer a popup component shows its content through
/// (`createPopupLayer`'s return). Clones share it.
pub struct PopupLayer<T: 'static> {
    inner: Rc<LayerInner<T>>,
}

impl<T: 'static> Clone for PopupLayer<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// Layers' own parts, which a modal that shows later keeps out of its block,
// the modal layers that block the window, the close watchers, from the
// oldest, and popups that show again under a newer one around them.
type ShowsAgain = (Region, Option<Region>, Rc<dyn Fn(&mut Window, &mut App)>);

thread_local! {
    static LAYER_PARTS: RefCell<Vec<(Layer, Vec<Region>)>> = const { RefCell::new(Vec::new()) };
    static FREEZING: RefCell<Vec<Layer>> = const { RefCell::new(Vec::new()) };
    static WATCHERS: RefCell<Vec<(u64, Layer)>> = const { RefCell::new(Vec::new()) };
    static NEXT_WATCHER: Cell<u64> = const { Cell::new(1) };
    static SHOWS_AGAIN: RefCell<Vec<(Layer, ShowsAgain)>> = const { RefCell::new(Vec::new()) };
    static INERT: RefCell<Vec<Region>> = const { RefCell::new(Vec::new()) };
    static UNFOCUSABLE: RefCell<Vec<WeakFocusHandle>> = const { RefCell::new(Vec::new()) };
}

fn parts_of(layer: &Layer) -> Vec<Region> {
    LAYER_PARTS.with(|parts| {
        parts
            .borrow()
            .iter()
            .find(|(own, _)| own == layer)
            .map(|(_, regions)| regions.clone())
            .unwrap_or_default()
    })
}

fn is_freezing(layer: &Layer) -> bool {
    FREEZING.with(|freezing| freezing.borrow().contains(layer))
}

fn set_freezing(layer: &Layer, freezing_now: bool) {
    FREEZING.with(|freezing| {
        let mut freezing = freezing.borrow_mut();
        freezing.retain(|other| other != layer);
        if freezing_now {
            freezing.push(layer.clone());
        }
    });
}

/// Whether `layer` shows above the modal `modal`, which then spares it: one
/// opened from it, a newer layer that blocks the window like a confirm, and
/// anything opened from that one.
fn above(layer: &Layer, modal: &Layer) -> bool {
    let mut up = Some(layer.clone());
    while let Some(current) = up {
        if current == *modal || (is_freezing(&current) && current.order() > modal.order()) {
            return true;
        }
        up = current.parent();
    }
    false
}

/// Marks a focus handle as unable to take focus, like a disabled trigger's,
/// or clears it, so focus a popup hands back skips it, as it would a disabled
/// native control.
pub fn set_unfocusable(handle: &FocusHandle, unfocusable: bool) {
    let weak = handle.downgrade();
    UNFOCUSABLE.with(|list| {
        let mut list = list.borrow_mut();
        list.retain(|other| *other != weak && other.upgrade().is_some());
        if unfocusable {
            list.push(weak);
        }
    });
}

fn is_unfocusable(handle: &FocusHandle) -> bool {
    let weak = handle.downgrade();
    UNFOCUSABLE.with(|list| list.borrow().iter().any(|other| *other == weak))
}

/// Whether focus can rest on the element `handle` tracks: it rendered, isn't
/// marked unfocusable, and isn't inside a popup that's animating out, which
/// is inert (`usable`).
pub fn usable(handle: &FocusHandle, window: &Window) -> bool {
    handle.is_rendered(window)
        && !is_unfocusable(handle)
        && !INERT.with(|inert| inert.borrow().iter().any(|region| region.contains_focus(handle, window)))
}

fn set_inert(region: &Region, inert_now: bool) {
    INERT.with(|inert| {
        let mut inert = inert.borrow_mut();
        inert.retain(|other| other != region);
        if inert_now {
            inert.push(region.clone());
        }
    });
}

// Whether focus rests on nothing a person can use: nothing, or an element
// that's gone or inert (`focusIsLost`).
fn focus_is_lost(window: &Window, cx: &App) -> bool {
    window
        .focused(cx)
        .is_none_or(|active| !usable(&active, window))
}

// Where Tab goes from `handle`, with focus put back where it was.
fn step_from(handle: &FocusHandle, backward: bool, window: &mut Window, cx: &mut App) -> Option<FocusHandle> {
    let original = window.focused(cx);
    window.focus(handle, cx);
    if backward {
        window.focus_prev(cx);
    } else {
        window.focus_next(cx);
    }
    let next = window.focused(cx).filter(|next| next != handle);
    match original {
        Some(original) => window.focus(&original, cx),
        None => window.blur(cx),
    }
    next
}

fn record_neighbours(trigger: Option<FocusHandle>, el: &Region, window: &mut Window, cx: &mut App) -> Neighbours {
    let Some(trigger) = trigger else {
        return Neighbours::default();
    };
    if !trigger.is_rendered(window) {
        return Neighbours {
            trigger: Some(trigger),
            next: None,
            previous: None,
        };
    }
    let skip = el.focus_handle();
    let next = next_tabbable(&trigger, skip.as_ref(), window, cx);
    let previous = step_from(&trigger, true, window, cx).filter(|previous| !el.contains_focus(previous, window));
    Neighbours {
        trigger: Some(trigger),
        next,
        previous,
    }
}

type Take<'a> = &'a mut dyn FnMut(&FocusHandle, &mut Window, &mut App) -> bool;

#[derive(Clone, Copy)]
enum Candidate {
    ReturnTrigger,
    Before,
    Inherited,
    Nearby,
    Trigger,
}

impl<T: 'static> PopupLayer<T> {
    /// A layer for the component being made, closed or open from the start
    /// (`createPopupLayer`).
    pub fn new(options: PopupLayerOptions<T>, open: bool, window: &mut Window, cx: &mut Context<T>) -> Self {
        let behavior = resolve_behavior(options.preset, &options.behavior);
        let entity = cx.weak_entity();
        let window_handle = window.window_handle();
        let inner = Rc::new_cyclic(|me| LayerInner {
            options,
            behavior,
            entity,
            window: window_handle,
            presence: Presence::new(false),
            backdrop_presence: Presence::new(false),
            blocker: Rc::new(RefCell::new(Blocker::default())),
            state: RefCell::new(LayerState {
                was_open: open,
                open,
                mounted: open,
                bound: None,
                shown: None,
                exiting: None,
                exit_run: 0,
                opener: if open {
                    PopupOpener::Initial
                } else {
                    PopupOpener::Imperative
                },
                focus_pending: behavior.focus_on_mount && open,
                focus_scheduled: false,
                enter_pending: false,
                opened_on_press: false,
                opened_by_key: false,
                return_mode: ReturnMode::Return,
                before: None,
                opened_without_trigger: open,
                upstream: Rc::new(|_, _, _, _| None),
                neighbours: Neighbours::default(),
                observer: None,
                last_press: None,
            }),
            resolving: Cell::new(false),
            me: me.clone(),
        });
        PopupLayer { inner }
    }

    fn from_weak(weak: &Weak<LayerInner<T>>) -> Option<Self> {
        weak.upgrade().map(|inner| PopupLayer { inner })
    }

    /// The behavior it runs with: its preset's, with its overrides.
    pub fn behavior(&self) -> PopupBehavior {
        self.inner.behavior
    }

    /// The window it shows in.
    pub fn window_handle(&self) -> AnyWindowHandle {
        self.inner.window
    }

    /// The component it belongs to.
    pub fn entity(&self) -> WeakEntity<T> {
        self.inner.entity.clone()
    }

    /// The content's presence: its enter and exit, which its part styles from.
    pub fn presence(&self) -> &Presence {
        &self.inner.presence
    }

    /// The backdrop's presence, which runs with the content's.
    pub fn backdrop_presence(&self) -> &Presence {
        &self.inner.backdrop_presence
    }

    pub(crate) fn blocker(&self) -> Rc<RefCell<Blocker>> {
        self.inner.blocker.clone()
    }

    /// Whether it's open, as the layer last heard.
    pub fn is_open(&self) -> bool {
        self.inner.state.borrow().open
    }

    /// Whether it's in the page, including while it animates out.
    pub fn is_mounted(&self) -> bool {
        self.inner.state.borrow().mounted
    }

    /// Whether its content shows.
    pub fn is_shown(&self) -> bool {
        self.inner.state.borrow().shown.is_some()
    }

    /// Whether it stays in the page as `open` changes to this: an open mounts
    /// it, and a close leaves it while it animates out, unless nothing of it
    /// shows (`mountedAs`).
    pub fn mounted_as(&self, open: bool) -> bool {
        open || self.is_shown()
    }

    /// The layer it shows in, while it shows.
    pub fn layer(&self) -> Option<Layer> {
        self.inner
            .state
            .borrow()
            .shown
            .as_ref()
            .map(|shown| shown.layer.clone())
    }

    /// Its content's region, while it shows.
    pub fn element(&self) -> Option<Region> {
        self.inner
            .state
            .borrow()
            .shown
            .as_ref()
            .map(|shown| shown.el.clone())
    }

    /// The backdrop that shows with it, while it does.
    pub fn backdrop_element(&self) -> Option<Region> {
        self.inner
            .state
            .borrow()
            .shown
            .as_ref()
            .and_then(|shown| shown.backdrop.clone())
    }

    /// The positioner placing its content, while it shows beside an anchor.
    pub fn positioner(&self) -> Option<Positioner> {
        self.inner
            .state
            .borrow()
            .shown
            .as_ref()
            .and_then(|shown| shown.positioner.clone())
    }

    /// Whether it shows in place, its content's `inline`.
    pub fn is_inline(&self) -> bool {
        let state = self.inner.state.borrow();
        state
            .shown
            .as_ref()
            .map(|shown| shown.config.inline)
            .or_else(|| state.bound.as_ref().map(|(_, config)| config.inline))
            .unwrap_or(false)
    }

    /// What its content is placed against now: the config's anchor, else
    /// the trigger.
    pub fn anchor(&self) -> Option<Anchor> {
        let config = {
            let state = self.inner.state.borrow();
            state
                .shown
                .as_ref()
                .map(|shown| shown.config.clone())
                .or_else(|| state.bound.as_ref().map(|(_, config)| config.clone()))
        };
        config
            .and_then(|config| config.anchor.and_then(|anchor| anchor()))
            .or_else(|| (self.inner.options.trigger)().map(Anchor::element))
    }

    /// The placement options its content is placed with now.
    pub fn position_options(&self) -> Option<PositionOptions> {
        let config = self
            .inner
            .state
            .borrow()
            .shown
            .as_ref()
            .map(|shown| shown.config.clone())?;
        config.position.map(|position| position())
    }

    /// How it was last opened.
    pub fn opener(&self) -> PopupOpener {
        self.inner.state.borrow().opener
    }

    /// The modal effects it shows with now.
    pub fn modal(&self) -> Option<PopupModal> {
        self.inner
            .state
            .borrow()
            .shown
            .as_ref()
            .map(|shown| shown.modal)
    }

    /// How Tab moves in it now.
    pub fn tab_mode(&self, this: &T) -> PopupTab {
        self.inner.behavior.tab.resolve(&(self.inner.options.modal)(this))
    }

    /// Call before opening, with how and from what event.
    pub fn opening(&self, by: PopupOpener, event: Option<&InputEvent>) {
        let mut state = self.inner.state.borrow_mut();
        state.opener = by;
        state.opened_on_press = matches!(event, Some(InputEvent::MouseDown(_) | InputEvent::TapDown(_)));
        state.opened_by_key = matches!(event, Some(InputEvent::KeyDown(_)));
    }

    /// For the popup's control only: hears every open and close first, from
    /// any source, and returns why a close happened. A second call replaces
    /// the first.
    pub fn observe(&self, listener: impl Fn(bool) -> Option<PopupClose> + 'static) {
        self.inner.state.borrow_mut().observer = Some(Rc::new(listener));
    }

    /// Where focus goes on the next close. Reset after every close, and on
    /// open.
    pub fn return_focus(&self, mode: ReturnMode) {
        self.inner.state.borrow_mut().return_mode = mode;
    }

    /// Asks the component to close, returning focus, like a sheet swiped
    /// away (`requestClose`).
    pub fn request_close(
        &self,
        this: &mut T,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let close = self.inner.options.close.clone();
        close(
            this,
            reason,
            CloseDetails {
                event,
                focus: ReturnMode::Return,
                return_value: None,
            },
            window,
            cx,
        );
    }

    /// Places the shown popup again now, without animating, like once its
    /// size is known.
    pub fn place(&self) {
        if let Some(positioner) = self.positioner() {
            positioner.place();
        }
    }

    /// Checks the shown popup's placement without starting it over, like when
    /// its arrow renders late. Placement follows every frame, so this asks
    /// for one.
    pub fn check_placement(&self, window: &mut Window) {
        if self.is_shown() {
            window.refresh();
        }
    }

    fn complete(&self, this: &mut T, open: bool, closed: Option<&PopupClose>, window: &mut Window, cx: &mut Context<T>) {
        if let Some(complete) = self.inner.options.on_open_change_complete.clone() {
            complete(this, open, closed, window, cx);
        }
    }

    fn opened_from(&self, focused: Option<FocusHandle>, window: &Window, cx: &App) {
        let opener_layer = focused
            .as_ref()
            .and_then(|focused| find_layer(&Target::Focus(focused.clone()), window, cx));
        let opener_target = if opener_layer.is_some() {
            None
        } else {
            last_closed(window)
        };
        let upstream: ReturnTarget = opener_layer
            .and_then(|layer| layer.return_target())
            .unwrap_or_else(|| Rc::new(move |_, _, _, _| opener_target.clone()));
        let mut state = self.inner.state.borrow_mut();
        state.before = focused;
        state.upstream = upstream;
    }

    fn return_trigger(&self) -> Option<Region> {
        match &self.inner.options.return_trigger {
            Some(return_trigger) => return_trigger(),
            None => (self.inner.options.trigger)(),
        }
    }

    /// Binds the content, from where its element renders, with how it
    /// attaches. Content kept in the page binds while closed and shows when
    /// the popup opens (`bind`).
    pub fn bind(&self, this: &mut T, el: Region, config: AttachConfig<T>, window: &mut Window, cx: &mut Context<T>) {
        let replace = {
            let mut state = self.inner.state.borrow_mut();
            state.bound = Some((el.clone(), config));
            state.mounted && state.shown.as_ref().is_some_and(|shown| shown.el != el)
        };
        if replace {
            self.hide(this, window, cx);
        }
        if self.inner.state.borrow().mounted {
            self.show(this, window, cx);
        }
    }

    /// Unbinds the content. Removed while open, like a dialog rendered only
    /// while it's open, focus goes back to what opened it; new content in its
    /// place keeps it (`bind`'s cleanup).
    pub fn unbind(&self, this: &mut T, el: &Region, window: &mut Window, cx: &mut Context<T>) {
        let setting = self
            .inner
            .options
            .final_focus
            .as_ref()
            .and_then(|final_focus| final_focus())
            .unwrap_or_default();
        let (shown_here, held, from) = {
            let mut state = self.inner.state.borrow_mut();
            if state.bound.as_ref().is_none_or(|(bound, _)| bound != el) {
                return;
            }
            state.bound = None;
            let shown_here = state.shown.as_ref().is_some_and(|shown| shown.el == *el);
            let active = window.focused(cx);
            let held = state.open
                && self.inner.behavior.moves_focus
                && active
                    .as_ref()
                    .is_none_or(|active| !active.is_rendered(window) || el.contains_focus(active, window));
            (shown_here, held, state.upstream.clone())
        };
        if !shown_here {
            return;
        }
        self.hide(this, window, cx);
        if held {
            let me = self.inner.me.clone();
            let el = el.clone();
            cx.defer_in(window, move |_, window, cx| {
                let Some(layer) = Self::from_weak(&me) else {
                    return;
                };
                let rebound = layer.inner.state.borrow().bound.is_some();
                if !rebound && focus_is_lost(window, cx) {
                    layer.return_focus_to(&el, &from, true, Some(setting), window, cx);
                }
            });
        }
    }

    /// The store's `open` changed, from any source: the layer shows or
    /// animates out, and focus moves in or back. The owner calls it after
    /// every change `open` makes, controlled or not.
    pub fn open_changed(&self, this: &mut T, open: bool, window: &mut Window, cx: &mut Context<T>) {
        {
            let mut state = self.inner.state.borrow_mut();
            state.open = open;
            if open == state.was_open {
                return;
            }
            state.was_open = open;
        }
        if open {
            self.opened(this, window, cx);
        } else {
            self.closed(this, window, cx);
        }
        cx.notify();
    }

    fn opened(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let observer = self.inner.state.borrow().observer.clone();
        if let Some(observer) = observer {
            observer(true);
        }
        if let Some(on_open_change) = self.inner.options.on_open_change.clone() {
            on_open_change(this, true, None, window, cx);
        }
        // Reopened while animating out: its exit stops.
        let exiting = self.inner.state.borrow_mut().exiting.take();
        if exiting.is_some() {
            self.inner.presence.stop(cx);
            self.inner.backdrop_presence.stop(cx);
            if let Some(el) = self.element() {
                set_inert(&el, false);
            }
        }
        {
            let mut state = self.inner.state.borrow_mut();
            state.focus_pending = true;
            state.focus_scheduled = false;
            state.enter_pending = true;
            state.return_mode = ReturnMode::Return;
        }
        let active = window.focused(cx).filter(|active| active.is_rendered(window));
        self.opened_from(active, window, cx);
        // A key pressed away from the trigger, like its shortcut in a field,
        // opened it from where focus was, unless that was a popup that just
        // closed, like the last menu of a menubar.
        let trigger = self.return_trigger();
        let (opener, by_key, before) = {
            let state = self.inner.state.borrow();
            (state.opener, state.opened_by_key, state.before.clone())
        };
        let without_trigger = opener == PopupOpener::Imperative
            || (by_key
                && before.as_ref().is_some_and(|before| {
                    find_layer(&Target::Focus(before.clone()), window, cx).is_none()
                        && last_closed(window).is_none()
                        && !trigger
                            .as_ref()
                            .is_some_and(|trigger| trigger.contains_focus(before, window))
                }));
        let (exiting_shown, bound) = {
            let mut state = self.inner.state.borrow_mut();
            state.opened_without_trigger = without_trigger;
            state.mounted = true;
            (state.shown.is_some(), state.bound.is_some())
        };
        // Reopened while animating out, it attaches again: the close already
        // let go of the blocked window and the close watcher, and focus has to
        // move in.
        if bound {
            if exiting_shown {
                self.hide(this, window, cx);
            }
            self.show(this, window, cx);
        }
    }

    fn closed(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let (mode, observer) = {
            let mut state = self.inner.state.borrow_mut();
            let mode = if self.inner.behavior.moves_focus {
                state.return_mode
            } else {
                ReturnMode::Leave
            };
            state.return_mode = ReturnMode::Return;
            state.focus_pending = false;
            state.focus_scheduled = false;
            state.enter_pending = false;
            state.opener = PopupOpener::Imperative;
            state.opened_by_key = false;
            (mode, state.observer.clone())
        };
        let closed = observer.and_then(|observer| observer(false));
        let (el, layer, from, on_close) = {
            let state = self.inner.state.borrow();
            let shown = state.shown.as_ref();
            (
                shown.map(|shown| shown.el.clone()),
                shown.map(|shown| shown.layer.clone()),
                state.upstream.clone(),
                shown.and_then(|shown| shown.config.on_close.clone()),
            )
        };
        if layer.is_some() {
            let target = self.return_target(None, false, window, cx);
            mark_closed(target, window);
        }
        let on_open_change = self.inner.options.on_open_change.clone();
        let Some(el) = el else {
            if let Some(on_open_change) = on_open_change {
                on_open_change(this, false, closed.as_ref(), window, cx);
            }
            self.inner.state.borrow_mut().mounted = false;
            self.complete(this, false, closed.as_ref(), window, cx);
            return;
        };
        // Focus on nothing belongs to no one. Focus in a popup nested under
        // this one counts as inside.
        let active = window.focused(cx);
        let trigger = (self.inner.options.trigger)();
        let had_focus = active.as_ref().is_none_or(|active| {
            !active.is_rendered(window)
                || el.contains_focus(active, window)
                || (layer
                    .as_ref()
                    .is_some_and(|layer| layer_contains(layer, &Target::Focus(active.clone()), window, cx))
                    && !trigger
                        .as_ref()
                        .is_some_and(|trigger| trigger.contains_focus(active, window)))
        });
        if let Some(on_open_change) = on_open_change {
            on_open_change(this, false, closed.as_ref(), window, cx);
        }
        if let Some(on_close) = on_close {
            on_close(this, window, cx);
        }
        let order = layer_count();
        // It stops hearing dismissals, and animates out, inert.
        if let Some(layer) = &layer {
            layer.set_active(false);
            layer.set_exiting(true);
        }
        set_inert(&el, true);
        let backdrop = self.backdrop_element();
        let run = {
            let mut state = self.inner.state.borrow_mut();
            state.exit_run += 1;
            state.exiting = Some(state.exit_run);
            state.exit_run
        };
        let left = Rc::new(Cell::new(if backdrop.is_some() { 2 } else { 1 }));
        let closed = Rc::new(closed);
        for presence in [Some(&self.inner.presence), backdrop.as_ref().map(|_| &self.inner.backdrop_presence)]
            .into_iter()
            .flatten()
        {
            let me = self.inner.me.clone();
            let left = left.clone();
            let closed = closed.clone();
            presence.clone().animate_out(
                this,
                move |this, window, cx| {
                    if let Some(layer) = Self::from_weak(&me) {
                        layer.exit_step(this, run, &left, closed.as_ref().as_ref(), window, cx);
                    }
                },
                window,
                cx,
            );
        }
        // The block on the window and the close watcher go at once.
        self.thaw(cx);
        self.release_watcher();
        if mode == ReturnMode::Leave || !had_focus {
            return;
        }
        match mode {
            ReturnMode::Return => {
                self.return_focus_to(&el, &from, false, None, window, cx);
                // What focus went back to can leave as the close renders, like
                // the row a confirm dialog's action removes. The chain goes on
                // then.
                let target = window.focused(cx);
                let me = self.inner.me.clone();
                let el = el.clone();
                window.on_next_frame(move |window, _cx| {
                    window.on_next_frame(move |window, cx| {
                        let Some(layer) = Self::from_weak(&me) else {
                            return;
                        };
                        if target.as_ref().is_some_and(|target| !target.is_rendered(window))
                            && focus_is_lost(window, cx)
                            && !opened_since(order, window)
                        {
                            layer.return_focus_to(&el, &from, false, None, window, cx);
                        }
                    });
                });
            }
            ReturnMode::UnlessMoved => {
                let me = self.inner.me.clone();
                let el = el.clone();
                window.defer(cx, move |window, cx| {
                    let Some(layer) = Self::from_weak(&me) else {
                        return;
                    };
                    let moved = window
                        .focused(cx)
                        .is_some_and(|now| now.is_rendered(window) && !el.contains_focus(&now, window));
                    if !moved && !opened_since(order, window) {
                        layer.return_focus_to(&el, &from, false, None, window, cx);
                    }
                });
            }
            ReturnMode::Later => {
                // After the press's own focus change: focus returns unless the
                // press focused something.
                let me = self.inner.me.clone();
                let el = el.clone();
                window.defer(cx, move |window, cx| {
                    let Some(layer) = Self::from_weak(&me) else {
                        return;
                    };
                    let lost = window
                        .focused(cx)
                        .is_none_or(|active| !usable(&active, window) || el.contains_focus(&active, window));
                    if lost {
                        layer.return_focus_to(&el, &from, false, None, window, cx);
                    }
                });
            }
            ReturnMode::Leave => {}
        }
    }

    fn exit_step(
        &self,
        this: &mut T,
        run: u64,
        left: &Cell<usize>,
        closed: Option<&PopupClose>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if self.inner.state.borrow().exiting != Some(run) {
            return;
        }
        left.set(left.get().saturating_sub(1));
        if left.get() > 0 {
            return;
        }
        {
            let mut state = self.inner.state.borrow_mut();
            state.exiting = None;
            state.mounted = false;
        }
        self.hide(this, window, cx);
        self.complete(this, false, closed, window, cx);
        cx.notify();
    }

    fn resolve_candidate(
        &self,
        candidate: Candidate,
        el: Option<&Region>,
        take: Option<Take<'_>>,
        from: &ReturnTarget,
        strict: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<FocusHandle> {
        match candidate {
            Candidate::ReturnTrigger => self.return_trigger().and_then(|trigger| trigger.focus_handle()),
            Candidate::Before => self.inner.state.borrow().before.clone(),
            Candidate::Inherited => from(take, strict, window, cx),
            Candidate::Nearby => self.neighbour(el, strict, window, cx),
            Candidate::Trigger => (self.inner.options.trigger)().and_then(|trigger| trigger.focus_handle()),
        }
    }

    // Where focus goes back to, in order. Opened without its trigger, what had
    // focus comes first, since the trigger was never used.
    fn chain(&self) -> [Candidate; 5] {
        if self.inner.state.borrow().opened_without_trigger {
            [
                Candidate::Before,
                Candidate::Inherited,
                Candidate::ReturnTrigger,
                Candidate::Nearby,
                Candidate::Trigger,
            ]
        } else {
            [
                Candidate::ReturnTrigger,
                Candidate::Before,
                Candidate::Inherited,
                Candidate::Nearby,
                Candidate::Trigger,
            ]
        }
    }

    /// Where focus would go back to from this popup: the first target `take`
    /// accepts, `usable` by default. A popup opened from this one returns here
    /// after this one closed, trying each target in turn with its own `take`.
    /// With `strict`, what took the place of a trigger that left isn't a
    /// neighbour (`returnTarget`).
    pub fn return_target(
        &self,
        mut take: Option<Take<'_>>,
        strict: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<FocusHandle> {
        // Reopened from that popup, each leads back to the other, so the
        // lookup stops here.
        if self.inner.resolving.replace(true) {
            return None;
        }
        let el = self.element();
        let from = self.inner.state.borrow().upstream.clone();
        let mut found = None;
        for candidate in self.chain() {
            let value = match &mut take {
                Some(take) => self.resolve_candidate(candidate, el.as_ref(), Some(&mut **take), &from, strict, window, cx),
                None => self.resolve_candidate(candidate, el.as_ref(), None, &from, strict, window, cx),
            };
            let Some(value) = value else {
                continue;
            };
            let accepted = match &mut take {
                Some(take) => take(&value, window, cx),
                None => usable(&value, window),
            };
            if accepted {
                found = Some(value);
                break;
            }
        }
        self.inner.resolving.set(false);
        found
    }

    // The nearest tab stop that survived where the trigger was. With
    // `strict`, what took the place of a trigger that left isn't one.
    fn neighbour(&self, el: Option<&Region>, strict: bool, window: &mut Window, cx: &mut App) -> Option<FocusHandle> {
        let neighbours = self.inner.state.borrow().neighbours.clone();
        let trigger = neighbours.trigger?;
        let inside = |handle: &FocusHandle, window: &Window| el.is_some_and(|el| el.contains_focus(handle, window));
        // A trigger still in the page that can't take focus, like one disabled
        // while it was open, hands focus to what follows it, as one that left
        // does. One inside the popup isn't one.
        if trigger.is_rendered(window) && !inside(&trigger, window) {
            let skip = el.and_then(Region::focus_handle);
            return next_tabbable(&trigger, skip.as_ref(), window, cx)
                .or_else(|| step_from(&trigger, true, window, cx).filter(|found| !inside(found, window)));
        }
        let next = neighbours
            .next
            .filter(|next| next.is_rendered(window) && !inside(next, window));
        let previous = neighbours
            .previous
            .filter(|previous| previous.is_rendered(window) && !inside(previous, window));
        if strict {
            // The gap the trigger left closed up: nothing took its place.
            let closed = match (&previous, &next) {
                (Some(previous), Some(next)) => step_from(previous, false, window, cx).as_ref() == Some(next),
                _ => true,
            };
            if !closed {
                return None;
            }
        }
        next.or(previous)
    }

    fn return_focus_to(
        &self,
        el: &Region,
        from: &ReturnTarget,
        strict: bool,
        setting: Option<PopupFocusTarget>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let setting = setting
            .or_else(|| self.inner.options.final_focus.as_ref().and_then(|final_focus| final_focus()))
            .unwrap_or_default();
        if matches!(setting, PopupFocusTarget::None) {
            return;
        }
        // Focusing can fail, on an element that's gone or inert. Then the next
        // one gets a turn, in the popups this one inherits its target from too.
        let el_for_take = el.clone();
        let mut take = move |target: &FocusHandle, window: &mut Window, cx: &mut App| {
            if !usable(target, window) || el_for_take.contains_focus(target, window) {
                return false;
            }
            restore_focus(target, window, cx);
            window.focused(cx).as_ref() == Some(target)
        };
        let first = match &setting {
            PopupFocusTarget::Handle(handle) => Some(handle.clone()),
            PopupFocusTarget::Fn(pick) => pick(window, cx),
            _ => None,
        };
        if let Some(first) = first
            && take(&first, window, cx)
        {
            return;
        }
        for candidate in self.chain() {
            let value = self.resolve_candidate(candidate, Some(el), Some(&mut take), from, strict, window, cx);
            if let Some(value) = value
                && take(&value, window, cx)
            {
                return;
            }
        }
    }

    /// Runs the component's initial focus now, once the content has
    /// rendered, like when a controller asks an open popup. `resolve` picks
    /// the element, the content's `initial_focus` (`initialFocus`).
    pub fn initial_focus(&self, resolve: Option<InitialFocusFn>, window: &mut Window, cx: &mut App) {
        let (plan, el, opened_on_press) = {
            let mut state = self.inner.state.borrow_mut();
            if !state.focus_pending || state.shown.is_none() || !state.open || !self.inner.behavior.moves_focus {
                return;
            }
            state.focus_pending = false;
            let plan = if state.opener == PopupOpener::Initial && self.inner.behavior.focus_on_mount {
                InitialFocusPlan::Auto
            } else {
                initial_focus_plan(state.opener)
            };
            let el = state.shown.as_ref().map(|shown| shown.el.clone());
            (plan, el, state.opened_on_press)
        };
        if plan == InitialFocusPlan::None {
            return;
        }
        let Some(el) = el else {
            return;
        };
        // The pointer can reach an item before the content rendered, which
        // focused it.
        if opened_on_press && window.focused(cx).is_some_and(|active| el.contains_focus(&active, window)) {
            return;
        }
        let target = match plan {
            InitialFocusPlan::Popup => el.focus_handle(),
            _ => resolve.and_then(|resolve| resolve(window, cx)),
        };
        let Some(target) = target else {
            return;
        };
        move_focus(&target, window, cx);
        // A target that can't take focus would leave focus behind the popup,
        // so the content takes it.
        let landed = target.is_rendered(window)
            && window
                .focused(cx)
                .is_some_and(|active| active == target || target.contains(&active, window) || el.contains_focus(&active, window));
        if !landed && let Some(content) = el.focus_handle() {
            move_focus(&content, window, cx);
        }
    }

    /// The content rendered this frame: focus moves in once the frame is
    /// drawn, since tab order is what the frame drew, to where the attach
    /// config's `initial_focus` picks. A component that renders its content
    /// itself calls it on every frame the content renders, while it lays out;
    /// [`popup_content`](crate::popup_content) does.
    pub fn content_rendered(&self, window: &mut Window, cx: &mut App) {
        let resolve = {
            let mut state = self.inner.state.borrow_mut();
            if !state.focus_pending || state.focus_scheduled || state.shown.is_none() {
                return;
            }
            state.focus_scheduled = true;
            state
                .shown
                .as_ref()
                .and_then(|shown| shown.config.initial_focus.clone())
        };
        let me = self.inner.me.clone();
        window.defer(cx, move |window, cx| {
            if let Some(layer) = Self::from_weak(&me) {
                layer.inner.state.borrow_mut().focus_scheduled = false;
                layer.initial_focus(resolve, window, cx);
            }
        });
    }

    fn show(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let bound = {
            let state = self.inner.state.borrow();
            if state.shown.is_some() {
                return;
            }
            state.bound.clone()
        };
        let Some((el, config)) = bound else {
            return;
        };
        self.attach(this, el, config, window, cx);
    }

    fn hide(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let Some(shown) = self.inner.state.borrow_mut().shown.take() else {
            return;
        };
        let Shown {
            el,
            layer,
            _pushed,
            positioner,
            pending_freeze,
            cleanups,
            releases,
            watcher,
            ..
        } = shown;
        if let Some(pending) = pending_freeze {
            pending.cancel();
        }
        for cleanup in cleanups.into_iter().rev() {
            cleanup(this, window, cx);
        }
        for release in releases {
            release();
        }
        if let Some(positioner) = positioner {
            positioner.stop();
        }
        {
            let mut blocker = self.inner.blocker.borrow_mut();
            blocker.frozen = false;
            blocker.keep.clear();
            blocker.extra.clear();
        }
        layer.set_keep(None);
        layer.set_active(false);
        set_freezing(&layer, false);
        LAYER_PARTS.with(|parts| parts.borrow_mut().retain(|(own, _)| *own != layer));
        SHOWS_AGAIN.with(|shows| shows.borrow_mut().retain(|(own, _)| *own != layer));
        if let Some(watcher) = watcher {
            WATCHERS.with(|watchers| watchers.borrow_mut().retain(|(id, _)| *id != watcher));
        }
        set_inert(&el, false);
        drop(_pushed);
        cx.notify();
    }

    fn attach(&self, this: &mut T, el: Region, config: AttachConfig<T>, window: &mut Window, cx: &mut Context<T>) {
        let options = &self.inner.options;
        let behavior = self.inner.behavior;
        let trigger = (options.trigger)();
        // Mounted open, like a dialog a menu item opens, focus returns to what
        // had it as the popup mounted, since no open recorded it.
        let (opener, before) = {
            let state = self.inner.state.borrow();
            (state.opener, state.before.clone())
        };
        if opener == PopupOpener::Initial && before.is_none() {
            let active = window
                .focused(cx)
                .filter(|active| active.is_rendered(window) && !el.contains_focus(active, window));
            self.opened_from(active, window, cx);
        }
        let backdrop = config.backdrop.as_ref().and_then(|backdrop| backdrop());
        let positioner = (config.position.is_some() && options.position && !config.inline).then(|| {
            let anchor = config
                .anchor
                .as_ref()
                .and_then(|anchor| anchor())
                .or_else(|| trigger.clone().map(Anchor::element));
            let placement = config.position.as_ref().map(|position| position()).unwrap_or_default();
            position(anchor, placement)
        });
        // The enter: the content, and its backdrop below it.
        let enter_pending = self.inner.state.borrow().enter_pending;
        if enter_pending {
            let me = self.inner.me.clone();
            self.inner.presence.animate_in_then(
                move |this, window, cx| {
                    if let Some(layer) = Self::from_weak(&me) {
                        let pending = std::mem::replace(&mut layer.inner.state.borrow_mut().enter_pending, false);
                        if pending {
                            layer.complete(this, true, None, window, cx);
                        }
                    }
                },
                window,
                cx,
            );
        } else {
            self.inner.presence.animate_in(window, cx);
        }
        if backdrop.is_some() {
            self.inner.backdrop_presence.animate_in(window, cx);
        }
        track_input(window, cx);

        let modal = (options.modal)(this);
        let persistent = options
            .persistent
            .as_ref()
            .map(|persistent| persistent(this))
            .unwrap_or_default();
        // Without a trigger, what had focus stands in for the way back.
        let returns_to = self
            .return_trigger()
            .and_then(|trigger| trigger.focus_handle())
            .or_else(|| {
                if trigger.is_none() {
                    self.inner
                        .state
                        .borrow()
                        .before
                        .clone()
                        .filter(|before| before.is_rendered(window))
                } else {
                    None
                }
            });
        if returns_to.is_some() || trigger.is_none() {
            let neighbours = record_neighbours(returns_to, &el, window, cx);
            self.inner.state.borrow_mut().neighbours = neighbours;
        }

        let parent = options
            .parent
            .as_ref()
            .and_then(|parent| parent(this))
            .or_else(|| {
                trigger
                    .as_ref()
                    .and_then(|trigger| trigger.focus_handle())
                    .and_then(|handle| find_layer(&Target::Focus(handle), window, cx))
            });
        let layer = self.make_layer(&el, backdrop.clone(), &persistent, parent, modal);
        LAYER_PARTS.with(|parts| {
            parts
                .borrow_mut()
                .push((layer.clone(), [Some(el.clone()), backdrop.clone()].into_iter().flatten().collect()))
        });
        let pushed = push_layer(layer.clone(), window, cx);
        // Shows again from the start when it sits inside a newer popup: its
        // content does, or its trigger, when the content renders elsewhere.
        {
            let me = self.inner.me.clone();
            let again: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |window, cx| {
                let Some(layer) = Self::from_weak(&me) else {
                    return;
                };
                let focused = window.focused(cx);
                layer
                    .inner
                    .entity
                    .update(cx, |this, cx| {
                        layer.hide(this, window, cx);
                        layer.show(this, window, cx);
                    })
                    .ok();
                if let Some(focused) = focused
                    && window.focused(cx).as_ref() != Some(&focused)
                {
                    window.focus(&focused, cx);
                }
            });
            SHOWS_AGAIN.with(|shows| {
                shows
                    .borrow_mut()
                    .push((layer.clone(), (el.clone(), trigger.clone(), again)))
            });
        }
        if modal.inert_and_lock {
            set_freezing(&layer, true);
        }
        self.inner.state.borrow_mut().shown = Some(Shown {
            el: el.clone(),
            config: config.clone(),
            backdrop: backdrop.clone(),
            layer: layer.clone(),
            _pushed: pushed,
            positioner,
            modal,
            persistent: persistent.clone(),
            trigger: trigger.clone(),
            pending_freeze: None,
            cleanups: Vec::new(),
            releases: Vec::new(),
            _subscriptions: Vec::new(),
            watcher: None,
        });

        for on_show in [options.on_show.clone(), config.on_show.clone()].into_iter().flatten() {
            if let Some(cleanup) = on_show(this, &layer, window, cx)
                && let Some(shown) = self.inner.state.borrow_mut().shown.as_mut()
            {
                shown.cleanups.push(cleanup);
            }
        }

        // The window freezes a paint after opening, so a confirm opened in
        // between is already above this one.
        if modal.inert_and_lock {
            let me = self.inner.me.clone();
            let initial_focus = config.initial_focus.clone();
            let pending = after_paint(window, move |window, cx| {
                let Some(layer) = Self::from_weak(&me) else {
                    return;
                };
                if let Some(shown) = layer.inner.state.borrow_mut().shown.as_mut() {
                    shown.pending_freeze = None;
                }
                if !layer.is_open() {
                    return;
                }
                let focused = window.focused(cx);
                layer.freeze(window);
                // Focus that rested on the window, blocked now, moves in.
                // Persistent layers, like a toast region, stay usable, as the
                // original's inert page leaves them live.
                let trapping = layer
                    .inner
                    .entity
                    .read_with(cx, |this, _| layer.tab_mode(this) == PopupTab::Trap)
                    .unwrap_or(false);
                let kept = layer.inner.blocker.borrow().kept();
                let blocked = focused.as_ref().is_some_and(|focused| {
                    let target = Target::Focus(focused.clone());
                    !kept.iter().any(|region| region.contains_focus(focused, window))
                        && !in_persistent_layer(&target, window)
                        && !layer
                            .layer()
                            .is_some_and(|own| layer_contains(&own, &target, window, cx))
                });
                if trapping && blocked {
                    layer.inner.state.borrow_mut().focus_pending = true;
                    layer.initial_focus(initial_focus.clone(), window, cx);
                }
            });
            if let Some(shown) = self.inner.state.borrow_mut().shown.as_mut() {
                shown.pending_freeze = Some(pending);
            }
        }

        // A modal beneath spares this layer when it shows above it, like a
        // confirm opened from code over a dialog, not only when it opened from
        // that modal.
        let mut beneath: Vec<Layer> = active_layers(window);
        let mut up = layer.parent();
        while let Some(current) = up {
            if !beneath.contains(&current) {
                beneath.push(current.clone());
            }
            up = current.parent();
        }
        beneath.retain(|other| *other != layer);
        let mut releases: Vec<Box<dyn FnOnce()>> = Vec::new();
        for other in beneath {
            if let Some(keep) = other.keep()
                && above(&layer, &other)
            {
                for part in [Some(el.clone()), backdrop.clone()].into_iter().flatten() {
                    releases.push(keep(&part));
                }
            }
        }

        let mut subscriptions = Vec::new();
        let tab = behavior.tab.resolve(&modal);
        if tab == PopupTab::Flow && behavior.dismiss_on_focus_loss {
            // Focus landing anywhere (`focusin` on the document).
            let me = self.inner.me.clone();
            subscriptions.push(on_focus_change(window, cx, move |this, change, window, cx| {
                if let (Some(target), Some(layer)) = (change.current.as_ref(), Self::from_weak(&me)) {
                    layer.focus_moved(this, target, window, cx);
                }
            }));
        }

        let watcher = behavior.close_watcher.then(|| {
            let id = NEXT_WATCHER.with(|next| {
                let id = next.get();
                next.set(id + 1);
                id
            });
            WATCHERS.with(|watchers| watchers.borrow_mut().push((id, layer.clone())));
            id
        });

        // A trapping popup that shows again while open has to hold focus
        // again: focus left, or rests on what the popup blocks.
        let (open, focus_pending) = {
            let state = self.inner.state.borrow();
            (state.open, state.focus_pending)
        };
        if open && !focus_pending && tab == PopupTab::Trap && focus_is_lost(window, cx) {
            self.inner.state.borrow_mut().focus_pending = true;
        }
        if let Some(shown) = self.inner.state.borrow_mut().shown.as_mut() {
            shown.releases.extend(releases);
            shown._subscriptions = subscriptions;
            shown.watcher = watcher;
        }

        // A popup inside this one that showed first, like nested dialogs open
        // from the start, shows again, so it lands above this one and nests
        // under it.
        let others: Vec<Rc<dyn Fn(&mut Window, &mut App)>> = SHOWS_AGAIN.with(|shows| {
            shows
                .borrow()
                .iter()
                .filter(|(other, (other_el, other_trigger, _))| {
                    *other != layer
                        && other.is_active()
                        && (el_contains(&el, other_el, window)
                            || other_trigger
                                .as_ref()
                                .is_some_and(|other_trigger| el_contains(&el, other_trigger, window)))
                })
                .map(|(_, (_, _, again))| again.clone())
                .collect()
        });
        if !others.is_empty() {
            window.defer(cx, move |window, cx| {
                for again in others {
                    again(window, cx);
                }
            });
        }
        // Children render with the content. Focus once they have.
        self.inner.state.borrow_mut().focus_scheduled = false;
        cx.notify();
    }

    fn make_layer(
        &self,
        el: &Region,
        backdrop: Option<Region>,
        persistent: &[Region],
        parent: Option<Layer>,
        modal: PopupModal,
    ) -> Layer {
        let options = &self.inner.options;
        let behavior = self.inner.behavior;
        let contains = {
            let el = el.clone();
            let persistent = persistent.to_vec();
            let rule = options.contains.clone();
            let trigger = options.trigger.clone();
            move |target: &Target, event: Option<&InputEvent>, window: &Window, cx: &App| {
                el.contains(target, window)
                    || persistent.iter().any(|region| region.contains(target, window))
                    || match &rule {
                        Some(rule) => rule(target, event, window, cx),
                        None => trigger().is_some_and(|trigger| trigger.contains(target, window)),
                    }
            }
        };
        let over_backdrop = {
            let el = el.clone();
            move |point: Point<Pixels>, _window: &Window, _cx: &App| {
                let Some(backdrop) = backdrop.as_ref().and_then(Region::bounds) else {
                    return false;
                };
                let outside_el = el.bounds().is_none_or(|bounds| !bounds.contains(&point));
                backdrop.contains(&point) && outside_el
            }
        };
        let me = self.inner.me.clone();
        let return_target: ReturnTarget = Rc::new(move |take, strict, window, cx| {
            Self::from_weak(&me).and_then(|layer| layer.return_target(take, strict, window, cx))
        });
        let escape_me = self.inner.me.clone();
        let outside_me = self.inner.me.clone();
        let blur_me = self.inner.me.clone();
        let mut layer_options = LayerOptions::default()
            .contains(contains)
            .parent(parent)
            .over_backdrop(over_backdrop)
            .modal(modal.consume_outside_press)
            .outside_press(behavior.outside_press)
            .shield(behavior.shield)
            .return_target(return_target)
            .blocking(behavior.blocking.resolve(&modal))
            .on_escape(move |event, window, cx| {
                window.prevent_default();
                if let Some(layer) = Self::from_weak(&escape_me) {
                    layer.on_escape(event, window, cx);
                }
            })
            .on_outside_press(move |press, window, cx| {
                if let Some(layer) = Self::from_weak(&outside_me) {
                    layer.on_outside_press(press, window, cx);
                }
            });
        if behavior.blur == PopupBlur::Always {
            layer_options = layer_options.on_blur(move |window, cx| {
                if let Some(layer) = Self::from_weak(&blur_me) {
                    let event = InputEvent::Focus(window_source(window));
                    layer.close_from_layer(Reason::WindowBlur, Some(event), ReturnMode::Leave, window, cx);
                }
            });
        }
        if let Some((component, id)) = &options.owner {
            layer_options = layer_options.component(component.clone()).id(id.clone());
        }
        Layer::new(layer_options)
    }

    fn close_from_layer(
        &self,
        reason: Reason,
        event: Option<InputEvent>,
        focus: ReturnMode,
        window: &mut Window,
        cx: &mut App,
    ) {
        let close = self.inner.options.close.clone();
        self.inner
            .entity
            .update(cx, |this, cx| {
                close(
                    this,
                    reason,
                    CloseDetails {
                        event,
                        focus,
                        return_value: None,
                    },
                    window,
                    cx,
                )
            })
            .ok();
    }

    fn on_escape(&self, event: &KeyDownEvent, window: &mut Window, cx: &mut App) {
        let on_escape = self.inner.options.on_escape.clone();
        match on_escape {
            Some(on_escape) => {
                self.inner
                    .entity
                    .update(cx, |this, cx| on_escape(this, event, window, cx))
                    .ok();
            }
            None => self.close_from_layer(
                Reason::EscapeKey,
                Some(InputEvent::KeyDown(event.clone())),
                ReturnMode::Return,
                window,
                cx,
            ),
        }
    }

    fn on_outside_press(&self, press: &OutsidePressEvent, window: &mut Window, cx: &mut App) {
        let consumed = self.modal().is_some_and(|modal| modal.consume_outside_press);
        // gpui can't tell what a point holds, so whether the press focused
        // something is read once it's over.
        let focus = match outside_press_focus(press.kind, false, consumed) {
            ReturnMode::Return if press.kind == PressKind::PointerUp && !consumed => ReturnMode::Later,
            focus => focus,
        };
        self.close_from_layer(Reason::OutsidePress, Some(press.event.clone()), focus, window, cx);
    }

    // Focus moved somewhere: out of the layer, it closes a popup in Tab's
    // flow. A press on something it counts as inside, like another trigger,
    // keeps it.
    fn focus_moved(&self, this: &mut T, target: &FocusHandle, window: &mut Window, cx: &mut Context<T>) {
        if !self.is_open() {
            return;
        }
        let Some(layer) = self.layer() else {
            return;
        };
        if layer_contains(&layer, &Target::Focus(target.clone()), window, cx) {
            return;
        }
        let press = self.inner.state.borrow_mut().last_press.take();
        if let Some((press, at)) = press
            && at.elapsed() < Duration::from_secs(1)
            && layer.contains(
                &Target::Point(press.position),
                Some(&InputEvent::MouseDown(press)),
                window,
                cx,
            )
        {
            return;
        }
        let close = self.inner.options.close.clone();
        close(
            this,
            Reason::FocusOut,
            CloseDetails {
                event: Some(InputEvent::Focus(window_source(window))),
                focus: ReturnMode::Leave,
                return_value: None,
            },
            window,
            cx,
        );
    }

    fn freeze(&self, window: &Window) {
        let (el, backdrop, persistent, trigger, own) = {
            let state = self.inner.state.borrow();
            let Some(shown) = state.shown.as_ref() else {
                return;
            };
            (
                shown.el.clone(),
                shown.backdrop.clone(),
                shown.persistent.clone(),
                shown.trigger.clone(),
                shown.layer.clone(),
            )
        };
        let mut keep: Vec<Region> = [Some(el), backdrop].into_iter().flatten().collect();
        keep.extend(persistent);
        if self.inner.behavior.keep_trigger
            && let Some(trigger) = trigger
        {
            keep.push(trigger);
        }
        for other in active_layers(window) {
            if other != own && above(&other, &own) {
                keep.extend(parts_of(&other));
            }
        }
        {
            let mut blocker = self.inner.blocker.borrow_mut();
            blocker.frozen = true;
            blocker.keep = keep;
        }
        let blocker = Rc::downgrade(&self.inner.blocker);
        own.set_keep(Some(Rc::new(move |region: &Region| -> Box<dyn FnOnce()> {
            let Some(strong) = blocker.upgrade() else {
                return Box::new(|| {});
            };
            let id = {
                let mut blocker = strong.borrow_mut();
                blocker.next += 1;
                let id = blocker.next;
                blocker.extra.push((id, region.clone()));
                id
            };
            let blocker = Rc::downgrade(&strong);
            Box::new(move || {
                if let Some(blocker) = blocker.upgrade() {
                    blocker.borrow_mut().extra.retain(|(other, _)| *other != id);
                }
            })
        })));
    }

    fn thaw(&self, cx: &mut App) {
        let pending = self
            .inner
            .state
            .borrow_mut()
            .shown
            .as_mut()
            .and_then(|shown| shown.pending_freeze.take());
        if let Some(pending) = pending {
            pending.cancel();
        }
        let was_frozen = {
            let mut blocker = self.inner.blocker.borrow_mut();
            let was_frozen = blocker.frozen;
            blocker.frozen = false;
            blocker.keep.clear();
            blocker.extra.clear();
            was_frozen
        };
        if let Some(layer) = self.layer() {
            layer.set_keep(None);
        }
        if was_frozen && let Some(entity) = self.inner.entity.upgrade() {
            cx.notify(entity.entity_id());
        }
    }

    fn release_watcher(&self) {
        let watcher = self
            .inner
            .state
            .borrow_mut()
            .shown
            .as_mut()
            .and_then(|shown| shown.watcher.take());
        if let Some(watcher) = watcher {
            WATCHERS.with(|watchers| watchers.borrow_mut().retain(|(id, _)| *id != watcher));
        }
    }

    /// Applies `modal` again while shown, after it changed, like a sheet
    /// crossing the snap point above which it's modal. Focus doesn't move
    /// (`updateModal`).
    pub fn update_modal(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let next = (self.inner.options.modal)(this);
        let (layer, current) = {
            let state = self.inner.state.borrow();
            let Some(shown) = state.shown.as_ref() else {
                return;
            };
            (shown.layer.clone(), shown.modal)
        };
        layer.set_modal(next.consume_outside_press);
        layer.set_blocking(self.inner.behavior.blocking.resolve(&next));
        set_freezing(&layer, next.inert_and_lock);
        if next.inert_and_lock && !current.inert_and_lock && self.is_open() {
            self.freeze(window);
        } else if !next.inert_and_lock && current.inert_and_lock {
            self.thaw(cx);
        }
        if let Some(shown) = self.inner.state.borrow_mut().shown.as_mut() {
            shown.modal = next;
        }
        cx.notify();
    }

    /// A backdrop that renders after its popup has shown joins it below the
    /// popup, and a modal beneath keeps it usable, until the popup hides
    /// (`backdrop`).
    pub fn backdrop(&self, backdrop: Region, window: &mut Window, cx: &mut Context<T>) {
        let joined = {
            let mut state = self.inner.state.borrow_mut();
            match state.shown.as_mut() {
                Some(shown) if shown.backdrop.as_ref() != Some(&backdrop) => {
                    shown.backdrop = Some(backdrop.clone());
                    Some(shown.layer.clone())
                }
                _ => None,
            }
        };
        let Some(layer) = joined else {
            return;
        };
        self.inner.backdrop_presence.animate_in(window, cx);
        LAYER_PARTS.with(|parts| {
            if let Some((_, regions)) = parts.borrow_mut().iter_mut().find(|(own, _)| *own == layer) {
                regions.push(backdrop.clone());
            }
        });
        // What a modal beneath keeps for it, released as this popup hides.
        let mut releases: Vec<Box<dyn FnOnce()>> = Vec::new();
        for other in active_layers(window) {
            if other != layer
                && let Some(keep) = other.keep()
                && above(&layer, &other)
            {
                releases.push(keep(&backdrop));
            }
        }
        if let Some(shown) = self.inner.state.borrow_mut().shown.as_mut() {
            shown.releases.extend(releases);
        }
        cx.notify();
    }

    /// Escape closes the popup before its ancestors hear the key. Call it
    /// from the content's key handler for its own keys, outside the
    /// component's update, before [`PopupLayer::keydown`]. Returns whether it
    /// took the key.
    pub fn escape(&self, event: &KeyDownEvent, window: &mut Window, cx: &mut App) -> bool {
        if Key::of(&event.keystroke) != Key::Escape {
            return false;
        }
        let Some(layer) = self.layer() else {
            return false;
        };
        escape_layer(&layer, event, window, cx);
        true
    }

    /// Tab stays inside a trapping popup, goes back to the window after the
    /// trigger from a flowing one, wherever it renders, and closes a
    /// menu-like one. Call it from the content's key handler, for keys from
    /// this popup only, inside the component's update. Returns whether it
    /// took the key (`keydown`).
    pub fn keydown(&self, this: &mut T, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<T>) -> bool {
        if Key::of(&event.keystroke) != Key::Tab {
            return false;
        }
        let Some(popup) = self.element() else {
            return false;
        };
        let Some(root) = popup.focus_handle() else {
            return false;
        };
        let backward = event.keystroke.modifiers.shift;
        match self.tab_mode(this) {
            PopupTab::Native => false,
            PopupTab::Close => {
                // Focus moves on from the trigger: the content's tab stops come
                // last in the window's order, so Tab goes on from there.
                let inside = window
                    .focused(cx)
                    .is_some_and(|focused| popup.contains_focus(&focused, window));
                if inside
                    && let Some(trigger) = (self.inner.options.trigger)().and_then(|trigger| trigger.focus_handle())
                    && trigger.is_rendered(window)
                {
                    restore_focus(&trigger, window, cx);
                }
                let close = self.inner.options.close.clone();
                close(
                    this,
                    Reason::FocusOut,
                    CloseDetails {
                        event: Some(InputEvent::KeyDown(event.clone())),
                        focus: ReturnMode::Leave,
                        return_value: None,
                    },
                    window,
                    cx,
                );
                false
            }
            PopupTab::Trap => {
                window.prevent_default();
                cx.stop_propagation();
                if tab_within(&root, backward, true, window, cx).is_none() {
                    window.focus(&root, cx);
                }
                true
            }
            PopupTab::Flow => {
                let stops = tabbables(&root, window, cx);
                let active = window.focused(cx);
                let at_start = stops.first().is_none_or(|first| active.as_ref() == Some(first))
                    || active.as_ref() == Some(&root);
                let at_end = stops.last().is_none_or(|last| active.as_ref() == Some(last));
                let Some(trigger) = (self.inner.options.trigger)().and_then(|trigger| trigger.focus_handle()) else {
                    return false;
                };
                if backward && at_start {
                    window.prevent_default();
                    cx.stop_propagation();
                    window.focus(&trigger, cx);
                    return true;
                }
                if !backward && at_end {
                    let Some(next) = next_tabbable(&trigger, Some(&root), window, cx) else {
                        return false;
                    };
                    window.prevent_default();
                    cx.stop_propagation();
                    window.focus(&next, cx);
                    return true;
                }
                false
            }
        }
    }

    /// Tab from the trigger enters an open flowing popup, even when it
    /// renders somewhere else in the window. Call it from the trigger's key
    /// handler. Returns whether it took the key (`triggerKeydown`).
    pub fn trigger_keydown(&self, this: &T, event: &KeyDownEvent, window: &mut Window, cx: &mut App) -> bool {
        if Key::of(&event.keystroke) != Key::Tab || event.keystroke.modifiers.shift || !self.is_open() {
            return false;
        }
        let Some(popup) = self.element().and_then(|popup| popup.focus_handle()) else {
            return false;
        };
        if self.tab_mode(this) != PopupTab::Flow {
            return false;
        }
        window.prevent_default();
        cx.stop_propagation();
        let first = tabbables(&popup, window, cx).into_iter().next().unwrap_or(popup);
        window.focus(&first, cx);
        true
    }

    /// An element that takes no space and, on every frame its content
    /// renders it, calls [`content_rendered`](Self::content_rendered) and
    /// [`listen`](Self::listen). A component that renders its content itself
    /// adds it to the content, beside the window's layer
    /// [`listener`](ui_sdk_platform::listener).
    pub fn listener(&self) -> impl IntoElement + use<T> {
        let rendered = self.clone();
        let listening = self.clone();
        canvas(
            move |_, window, cx| rendered.content_rendered(window, cx),
            move |_, _, window, _| listening.listen(window),
        )
        .absolute()
    }

    /// What the content's element hears while it paints: presses, which tell
    /// a press's focus change from another, and the back gesture for a popup
    /// that closes on it. A component that renders its content itself calls
    /// it on every frame the content paints, beside the window's layer
    /// [`listener`](ui_sdk_platform::listener);
    /// [`popup_content`](crate::popup_content) does.
    pub fn listen(&self, window: &mut Window) {
        let me = self.inner.me.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, _cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            if let Some(layer) = Self::from_weak(&me) {
                layer.inner.state.borrow_mut().last_press = Some((event.clone(), std::time::Instant::now()));
            }
        });
        if !self.inner.behavior.close_watcher {
            return;
        }
        let me = self.inner.me.clone();
        window.on_mouse_event(move |event: &BackGestureEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let Some(layer) = Self::from_weak(&me) else {
                return;
            };
            // A sheet that keeps the gesture from starting on it closes
            // nothing.
            if crate::sheet::prevents_edge_swipe(event.start_position, window) {
                return;
            }
            // Only the topmost watcher hears it.
            let Some(own) = layer.layer() else {
                return;
            };
            let top = WATCHERS.with(|watchers| {
                watchers
                    .borrow()
                    .iter()
                    .rev()
                    .find(|(_, watcher)| watcher.is_active())
                    .map(|(_, watcher)| watcher.clone())
            });
            if top.as_ref() != Some(&own) {
                return;
            }
            window.prevent_default();
            cx.stop_propagation();
            if event.phase != BackGesturePhase::Committed {
                return;
            }
            // A close request for an Escape the layers just handled is
            // ignored, so one key press never closes two popups.
            let now = cx.background_executor().now();
            if last_escape(window).is_some_and(|at| now.saturating_duration_since(at) < Duration::from_millis(100)) {
                return;
            }
            layer.close_from_layer(
                Reason::Back,
                Some(InputEvent::BackGesture(event.clone())),
                ReturnMode::Return,
                window,
                cx,
            );
        });
    }
}

fn el_contains(outer: &Region, inner: &Region, window: &Window) -> bool {
    match (outer.focus_handle(), inner.focus_handle()) {
        (Some(outer), Some(inner)) => outer != inner && outer.contains(&inner, window),
        _ => false,
    }
}
