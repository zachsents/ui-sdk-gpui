//! Port of `foundation/popups/popup-steps.ts`: the open and close steps every
//! popup component shares, beside [`PopupLayer`]: a request to open or
//! close, the change it asks for, what a refused change puts back, why a
//! close happened, and the future a controller's `open()` returns. A
//! component keeps only its own rules, in three that each get the next step,
//! so the steps can't run out of order.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::fmt::Debug;
use std::rc::{Rc, Weak};

use futures::channel::oneshot;
use gpui::{App, Context, SharedString, Window};
use ui_sdk_kernel::{ChangeOptions, ChangeSource, InputEvent, Reason, change_source};
use ui_sdk_platform::Anchor;

use crate::dismissal::Dismissal;
use crate::popup::{
    AbortSignal, PopupClose, PopupCloseOptions, PopupLayer, PopupLayerOptions, PopupOpenOptions,
    PopupOpener, PopupPending, PopupResult, ReturnMode,
};
use crate::popup_payload::PopupPayload;

/// A request to open, from a trigger, a hover, a key or a controller
/// (`ShowRequest`). `payload` passed, even as `Some(None)`, replaces the
/// payload; left out, it stays on a popup that's open.
pub struct ShowRequest<P> {
    /// Why the payload changes, and the open's reason unless the caller
    /// names another.
    pub reason: Option<Reason>,
    pub event: Option<InputEvent>,
    pub source: Option<ChangeSource>,
    pub payload: Option<Option<P>>,
    pub anchor: Option<Anchor>,
    pub signal: Option<AbortSignal>,
    /// How it opens, which decides where focus goes. The `open` rule may
    /// decide instead.
    pub by: Option<PopupOpener>,
    /// The trigger asking, by key. A controller passes none.
    pub trigger: Option<SharedString>,
    /// What a component passes its own rules beside, like where a menu puts
    /// focus.
    pub extra: Option<Rc<dyn Any>>,
}

impl<P> Default for ShowRequest<P> {
    fn default() -> Self {
        Self {
            reason: None,
            event: None,
            source: None,
            payload: None,
            anchor: None,
            signal: None,
            by: None,
            trigger: None,
            extra: None,
        }
    }
}

impl<P: Clone> Clone for ShowRequest<P> {
    fn clone(&self) -> Self {
        Self {
            reason: self.reason,
            event: self.event.clone(),
            source: self.source,
            payload: self.payload.clone(),
            anchor: self.anchor.clone(),
            signal: self.signal.clone(),
            by: self.by,
            trigger: self.trigger.clone(),
            extra: self.extra.clone(),
        }
    }
}

impl<P> ShowRequest<P> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reason(mut self, reason: Reason) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn event(mut self, event: Option<InputEvent>) -> Self {
        self.event = event;
        self
    }

    pub fn payload(mut self, payload: Option<P>) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.anchor = Some(anchor.into());
        self
    }

    pub fn by(mut self, by: PopupOpener) -> Self {
        self.by = Some(by);
        self
    }

    pub fn trigger(mut self, trigger: impl Into<SharedString>) -> Self {
        self.trigger = Some(trigger.into());
        self
    }

    pub fn extra(mut self, extra: impl Any) -> Self {
        self.extra = Some(Rc::new(extra));
        self
    }
}

impl<P> From<PopupOpenOptions<P>> for ShowRequest<P> {
    fn from(options: PopupOpenOptions<P>) -> Self {
        Self {
            reason: options.reason,
            event: options.event,
            source: options.source,
            payload: options.payload,
            anchor: options.anchor,
            signal: options.signal,
            by: None,
            trigger: None,
            extra: None,
        }
    }
}

/// A request to open as the rules hear it: with the open's reason and the
/// payload's.
pub struct SettledShow<P> {
    pub request: ShowRequest<P>,
    /// Why it opens, which `on_open_change` hears.
    pub reason: Reason,
    /// Why the payload changes, which `on_payload_change` hears.
    pub payload_reason: Reason,
}

/// A request to close (`HideRequest`).
#[derive(Clone, Debug, Default)]
pub struct HideRequest {
    pub reason: Option<Reason>,
    pub event: Option<InputEvent>,
    pub source: Option<ChangeSource>,
    /// Where focus goes on this close. Defaults to the steps' `return_focus`.
    pub focus: Option<ReturnMode>,
    /// What the future `open()` returned resolves with, and `on_open_change`
    /// hears.
    pub return_value: Option<SharedString>,
}

impl HideRequest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reason(mut self, reason: Reason) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn event(mut self, event: Option<InputEvent>) -> Self {
        self.event = event;
        self
    }

    pub fn focus(mut self, focus: impl Into<ReturnMode>) -> Self {
        self.focus = Some(focus.into());
        self
    }

    pub fn return_value(mut self, value: Option<SharedString>) -> Self {
        self.return_value = value;
        self
    }
}

impl From<PopupCloseOptions> for HideRequest {
    fn from(options: PopupCloseOptions) -> Self {
        Self {
            reason: options.reason,
            event: options.event,
            source: options.source,
            focus: None,
            return_value: options.return_value,
        }
    }
}

impl From<ChangeOptions> for HideRequest {
    fn from(options: ChangeOptions) -> Self {
        Self {
            reason: options.reason,
            event: options.event,
            source: options.source,
            focus: None,
            return_value: None,
        }
    }
}

/// A change `open` asks for: why, the event, and what closed it.
#[derive(Clone, Debug)]
pub struct OpenChange {
    pub reason: Reason,
    pub event: Option<InputEvent>,
    pub source: Option<ChangeSource>,
    pub return_value: Option<SharedString>,
}

impl OpenChange {
    /// The change options for `Controllable::change`.
    pub fn options(&self) -> ChangeOptions {
        ChangeOptions {
            reason: Some(self.reason),
            event: self.event.clone(),
            source: self.source,
        }
    }
}

/// Fires `on_open_change` and writes `open` unless controlled. False when
/// refused. It runs inside the component's update.
pub type ChangeOpenFn<T> = Rc<dyn Fn(&mut T, bool, OpenChange, &mut Window, &mut Context<T>) -> bool>;

/// The next step of an open, which an `open` or `reopen` rule calls to go
/// on.
pub struct OpenStep<'a, T: 'static, P: Clone + PartialEq + Debug + 'static> {
    steps: &'a OpenSteps<T, P>,
    request: &'a ShowRequest<P>,
    reason: Reason,
    payload_reason: Reason,
    opened: &'a Cell<bool>,
}

impl<T: 'static, P: Clone + PartialEq + Debug + 'static> OpenStep<'_, T, P> {
    /// Asks for the change, with how it opens when the rule decides that.
    /// Returns whether the change went through, with the payload already put
    /// back when it didn't.
    pub fn open(&self, this: &mut T, by: Option<PopupOpener>, window: &mut Window, cx: &mut Context<T>) -> bool {
        let inner = &self.steps.inner;
        *inner.open_anchor.borrow_mut() = self.request.anchor.clone();
        let event = self.request.event.clone();
        let restore = inner.payload.as_ref().map(|payload| {
            payload.opening(
                this,
                self.request.trigger.clone(),
                self.request.payload.clone().flatten(),
                ChangeOptions {
                    reason: Some(self.payload_reason),
                    event: event.clone(),
                    source: self.request.source,
                },
                window,
                cx,
            )
        });
        let by = by.or(self.request.by).unwrap_or(PopupOpener::Imperative);
        inner.layer.opening(by, event.as_ref());
        let opened = (inner.change)(
            this,
            true,
            OpenChange {
                reason: self.reason,
                event,
                source: self.request.source,
                return_value: None,
            },
            window,
            cx,
        );
        if !opened && let Some(restore) = restore {
            restore.restore();
        }
        self.opened.set(opened);
        opened
    }
}

/// The next step of a close, which a `close` rule calls to go on.
pub struct CloseStep<'a, T: 'static, P: Clone + PartialEq + Debug + 'static> {
    steps: &'a OpenSteps<T, P>,
    request: &'a HideRequest,
    reason: Reason,
    closed: &'a Cell<bool>,
}

impl<T: 'static, P: Clone + PartialEq + Debug + 'static> CloseStep<'_, T, P> {
    /// Asks for the change. Returns whether it went through, with focus
    /// return already reset when it didn't.
    pub fn close(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) -> bool {
        let inner = &self.steps.inner;
        let event = self.request.event.clone();
        let source = self
            .request
            .source
            .unwrap_or_else(|| change_source(self.reason, event.as_ref()));
        *inner.requested.borrow_mut() = Some(PopupClose {
            reason: self.reason,
            source,
            return_value: self.request.return_value.clone(),
        });
        let focus = self
            .request
            .focus
            .or_else(|| inner.return_focus.as_ref().and_then(|return_focus| return_focus()).map(ReturnMode::from))
            .unwrap_or(ReturnMode::Return);
        inner.layer.return_focus(focus);
        let changed = (inner.change)(
            this,
            false,
            OpenChange {
                reason: self.reason,
                event,
                source: self.request.source,
                return_value: self.request.return_value.clone(),
            },
            window,
            cx,
        );
        if !changed {
            inner.layer.return_focus(ReturnMode::Return);
            *inner.requested.borrow_mut() = None;
        }
        self.closed.set(changed);
        changed
    }
}

/// A closed popup is asked to open. Call the step's `open` to ask for the
/// change; returning without calling it refuses, like a disabled select.
pub type OpenRule<T, P> = Rc<dyn Fn(&mut T, &SettledShow<P>, &OpenStep<'_, T, P>, &mut Window, &mut Context<T>)>;

/// An open popup is asked to close. Call the step's `close` to ask for the
/// change; returning without calling it keeps the popup open.
pub type CloseRule<T, P> = Rc<dyn Fn(&mut T, &HideRequest, Reason, &CloseStep<'_, T, P>, &mut Window, &mut Context<T>)>;

/// A component's own rules for its steps.
pub struct OpenRules<T: 'static, P: Clone + PartialEq + Debug + 'static> {
    /// A closed popup is asked to open.
    pub open: Option<OpenRule<T, P>>,
    /// An open popup is asked to open again, after a controller's payload and
    /// anchor applied. Call the step's `open` to open it anew, like a panel
    /// that stays open while another item takes it over.
    pub reopen: Option<OpenRule<T, P>>,
    /// An open popup is asked to close.
    pub close: Option<CloseRule<T, P>>,
}

impl<T: 'static, P: Clone + PartialEq + Debug + 'static> Default for OpenRules<T, P> {
    fn default() -> Self {
        Self {
            open: None,
            reopen: None,
            close: None,
        }
    }
}

impl<T: 'static, P: Clone + PartialEq + Debug + 'static> Clone for OpenRules<T, P> {
    fn clone(&self) -> Self {
        Self {
            open: self.open.clone(),
            reopen: self.reopen.clone(),
            close: self.close.clone(),
        }
    }
}

/// What [`create_open_steps`] takes.
pub struct OpenStepsOptions<T: 'static, P: Clone + PartialEq + Debug + 'static> {
    pub layer: PopupLayer<T>,
    /// Whether it's open now.
    pub is_open: Rc<dyn Fn(&T) -> bool>,
    /// Fires `on_open_change` and writes `open` unless controlled, then has
    /// the layer hear it. False when refused.
    pub change: ChangeOpenFn<T>,
    /// For popups whose triggers hand their content a payload.
    pub payload: Option<PopupPayload<T, P>>,
    /// `dismissible` and `on_dismiss_attempt`, which may refuse a dismissal
    /// before any rule hears it.
    pub dismissal: Option<Dismissal<T>>,
    /// Whether a close returns focus when its request doesn't say. Lists
    /// whose focus stays in an input pass false. Read on every close.
    pub return_focus: Option<Rc<dyn Fn() -> Option<bool>>>,
    pub rules: OpenRules<T, P>,
}

struct StepsInner<T: 'static, P: Clone + PartialEq + Debug + 'static> {
    layer: PopupLayer<T>,
    is_open: Rc<dyn Fn(&T) -> bool>,
    change: ChangeOpenFn<T>,
    payload: Option<PopupPayload<T, P>>,
    dismissal: Option<Dismissal<T>>,
    return_focus: Option<Rc<dyn Fn() -> Option<bool>>>,
    rules: RefCell<OpenRules<T, P>>,
    pending: RefCell<Option<(oneshot::Sender<PopupResult>, PopupPending)>>,
    signals: RefCell<Vec<(AbortSignal, u64)>>,
    requested: RefCell<Option<PopupClose>>,
    open_anchor: RefCell<Option<Anchor>>,
}

/// The open and close steps over a layer (`createOpenSteps`'s return).
/// Clones share them. Dropping every clone settles a pending `open()` with
/// `Unmount`.
pub struct OpenSteps<T: 'static, P: Clone + PartialEq + Debug + 'static = ()> {
    inner: Rc<StepsInner<T, P>>,
}

impl<T: 'static, P: Clone + PartialEq + Debug + 'static> Clone for OpenSteps<T, P> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// What `open()` resolves with when nothing opened: refused, or given an
/// aborted signal (`unopened`).
pub fn unopened(reason: Reason, event: Option<&InputEvent>) -> PopupPending {
    PopupPending::ready(PopupResult {
        reason,
        source: change_source(reason, event),
        return_value: None,
    })
}

fn resolve(inner: &StepsInner<impl Sized + 'static, impl Clone + PartialEq + Debug + 'static>, result: PopupResult) {
    if let Some((sender, _)) = inner.pending.borrow_mut().take() {
        sender.send(result).ok();
    }
    for (signal, id) in inner.signals.borrow_mut().drain(..) {
        signal.off(id);
    }
}

/// The steps for a layer (`createOpenSteps`).
pub fn create_open_steps<T: 'static, P: Clone + PartialEq + Debug + 'static>(
    options: OpenStepsOptions<T, P>,
) -> OpenSteps<T, P> {
    let inner = Rc::new(StepsInner {
        layer: options.layer,
        is_open: options.is_open,
        change: options.change,
        payload: options.payload,
        dismissal: options.dismissal,
        return_focus: options.return_focus,
        rules: RefCell::new(options.rules),
        pending: RefCell::new(None),
        signals: RefCell::new(Vec::new()),
        requested: RefCell::new(None),
        open_anchor: RefCell::new(None),
    });
    let weak: Weak<StepsInner<T, P>> = Rc::downgrade(&inner);
    inner.layer.observe(move |open| {
        let inner = weak.upgrade()?;
        let requested = inner.requested.borrow_mut().take();
        if open {
            return None;
        }
        let closed = requested.unwrap_or(PopupClose {
            reason: Reason::Imperative,
            source: ChangeSource::Programmatic,
            return_value: None,
        });
        resolve(&inner, closed.clone());
        Some(closed)
    });
    OpenSteps { inner }
}

impl<T: 'static, P: Clone + PartialEq + Debug + 'static> OpenSteps<T, P> {
    /// The layer the steps open and close.
    pub fn layer(&self) -> &PopupLayer<T> {
        &self.inner.layer
    }

    /// The payload, for popups whose triggers hand one over.
    pub fn payload(&self) -> Option<&PopupPayload<T, P>> {
        self.inner.payload.as_ref()
    }

    /// Replaces the component's rules.
    pub fn set_rules(&self, rules: OpenRules<T, P>) {
        *self.inner.rules.borrow_mut() = rules;
    }

    /// A controller's or a take-over's anchor. The component falls back to
    /// its own.
    pub fn anchor(&self) -> Option<Anchor> {
        self.inner.open_anchor.borrow().clone()
    }

    /// The next close, for an `open()` that opened the popup its own way
    /// (`result`).
    pub fn result(&self, signal: Option<AbortSignal>) -> PopupPending {
        let pending = {
            let mut pending = self.inner.pending.borrow_mut();
            pending
                .get_or_insert_with(PopupPending::channel)
                .1
                .clone()
        };
        if let Some(signal) = signal
            && !signal.is_aborted()
        {
            let layer = self.inner.layer.clone();
            let id = signal.on_abort(move |cx: &mut App| {
                let window = layer.window_handle();
                let entity = layer.entity();
                window
                    .update(cx, |_, window, cx| {
                        entity
                            .update(cx, |this, cx| layer.request_close(this, Reason::Imperative, None, window, cx))
                            .ok();
                    })
                    .ok();
            });
            self.inner.signals.borrow_mut().push((signal, id));
        }
        pending
    }

    /// Asks to open, and returns what the next close resolves with (`show`).
    /// `open_reason` is the reason `set_open` and `toggle` pass, beside the
    /// payload's.
    pub fn show(
        &self,
        this: &mut T,
        request: ShowRequest<P>,
        open_reason: Option<Reason>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> PopupPending {
        self.attempt(this, request, open_reason, window, cx).1
    }

    /// Asks to open, and returns whether it opened, with the future (`attempt`).
    pub fn attempt(
        &self,
        this: &mut T,
        request: ShowRequest<P>,
        open_reason: Option<Reason>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> (bool, PopupPending) {
        if request.signal.as_ref().is_some_and(AbortSignal::is_aborted) {
            return (false, unopened(Reason::Imperative, None));
        }
        let payload_reason = request.reason.unwrap_or(if request.event.is_some() {
            Reason::TriggerPress
        } else {
            Reason::Imperative
        });
        let reason = open_reason.unwrap_or(payload_reason);
        let signal = request.signal.clone();
        let opened = Cell::new(false);
        let rules = self.inner.rules.borrow().clone();
        let settled = SettledShow {
            request,
            reason,
            payload_reason,
        };
        let step = OpenStep {
            steps: self,
            request: &settled.request,
            reason,
            payload_reason,
            opened: &opened,
        };
        if (self.inner.is_open)(this) {
            let layer = &self.inner.layer;
            if layer.opener() == PopupOpener::Hover && settled.request.by != Some(PopupOpener::Hover) {
                layer.opening(
                    settled.request.by.unwrap_or(PopupOpener::Imperative),
                    settled.request.event.as_ref(),
                );
            }
            if settled.request.trigger.is_none()
                && let Some(payload) = &self.inner.payload
                && payload.reopening(
                    this,
                    settled.request.payload.clone(),
                    ChangeOptions {
                        reason: Some(payload_reason),
                        event: settled.request.event.clone(),
                        source: settled.request.source,
                    },
                    window,
                    cx,
                )
                && let Some(anchor) = &settled.request.anchor
            {
                *self.inner.open_anchor.borrow_mut() = Some(anchor.clone());
            }
            if let Some(reopen) = &rules.reopen {
                reopen(this, &settled, &step, window, cx);
            }
            return (opened.get(), self.result(signal));
        }
        match &rules.open {
            Some(rule) => rule(this, &settled, &step, window, cx),
            None => {
                step.open(this, None, window, cx);
            }
        }
        let event = settled.request.event.clone();
        if opened.get() {
            (true, self.result(signal))
        } else {
            (false, unopened(reason, event.as_ref()))
        }
    }

    /// Asks to close. Returns whether it closed (`hide`).
    pub fn hide(&self, this: &mut T, request: HideRequest, window: &mut Window, cx: &mut Context<T>) -> bool {
        if !(self.inner.is_open)(this) {
            return false;
        }
        let reason = request.reason.unwrap_or(Reason::Imperative);
        if let Some(dismissal) = &self.inner.dismissal
            && !dismissal.allows(this, reason, request.event.as_ref(), window, cx)
        {
            return false;
        }
        let closed = Cell::new(false);
        let rule = self.inner.rules.borrow().close.clone();
        let step = CloseStep {
            steps: self,
            request: &request,
            reason,
            closed: &closed,
        };
        match rule {
            Some(rule) => rule(this, &request, reason, &step, window, cx),
            None => {
                step.close(this, window, cx);
            }
        }
        closed.get()
    }

    /// Another trigger takes the open popup over, with its payload. It
    /// anchors at `anchor`, like a context menu moving to a point, else at
    /// that trigger (`takeOver`).
    #[allow(clippy::too_many_arguments)]
    pub fn take_over(
        &self,
        this: &mut T,
        trigger: SharedString,
        payload: Option<P>,
        anchor: Option<Anchor>,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        *self.inner.open_anchor.borrow_mut() = anchor;
        if let Some(payload_feature) = &self.inner.payload {
            payload_feature.hand_over(
                this,
                trigger,
                payload,
                ChangeOptions {
                    reason: Some(reason),
                    event,
                    source: None,
                },
                window,
                cx,
            );
        }
        cx.notify();
    }

    // A controller's open that names only an open reason. It carries no
    // payload, so the payload hears `Imperative`, event or not.
    fn show_for(&self, this: &mut T, options: ChangeOptions, window: &mut Window, cx: &mut Context<T>) -> bool {
        let request = ShowRequest {
            reason: Some(Reason::Imperative),
            event: options.event,
            source: options.source,
            ..ShowRequest::default()
        };
        self.attempt(this, request, options.reason, window, cx).0
    }

    /// Whether it's open. False as soon as it starts closing, while it still
    /// animates out (`isOpen`).
    pub fn is_open(&self, this: &T) -> bool {
        (self.inner.is_open)(this)
    }

    /// Opens or closes it. Returns false when the change was canceled or it
    /// was already open or closed (`setOpen`).
    pub fn set_open(&self, this: &mut T, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<T>) -> bool {
        if open {
            self.show_for(this, options, window, cx)
        } else {
            self.hide(this, options.into(), window, cx)
        }
    }

    /// Opens it, and resolves once it closes, with why, how and any return
    /// value. Resolves exactly once: on the next close from any source, when
    /// `signal` aborts, or with `Unmount` when the component leaves (`open`).
    /// An app's event is no trigger's, so the reason stays `Imperative`
    /// unless it names one.
    pub fn open(&self, this: &mut T, options: PopupOpenOptions<P>, window: &mut Window, cx: &mut Context<T>) -> PopupPending {
        let mut request = ShowRequest::from(options);
        request.reason = Some(request.reason.unwrap_or(Reason::Imperative));
        self.show(this, request, None, window, cx)
    }

    /// Closes it. `return_value` is passed to the future `open()` returned.
    /// Returns false when the close was canceled or it was already closed
    /// (`close`).
    pub fn close(&self, this: &mut T, options: PopupCloseOptions, window: &mut Window, cx: &mut Context<T>) -> bool {
        self.hide(this, options.into(), window, cx)
    }

    /// Opens it when it's closed, and closes it when it's open. Returns false
    /// when the change was canceled (`toggle`).
    pub fn toggle(&self, this: &mut T, options: ChangeOptions, window: &mut Window, cx: &mut Context<T>) -> bool {
        if self.is_open(this) {
            self.hide(this, options.into(), window, cx)
        } else {
            self.show_for(this, options, window, cx)
        }
    }
}

/// A popup layer and the steps that open and close it, for a component
/// that shows several popups over one value, like Navigation Menu's panels
/// (`createSteppedPopup`). Other popups get theirs from
/// [`create_popup`](fn@crate::create_popup).
pub fn create_stepped_popup<T: 'static>(
    layer: PopupLayerOptions<T>,
    open: bool,
    steps: impl FnOnce(PopupLayer<T>) -> OpenStepsOptions<T, ()>,
    window: &mut Window,
    cx: &mut Context<T>,
) -> (PopupLayer<T>, OpenSteps<T, ()>) {
    let popup = PopupLayer::new(layer, open, window, cx);
    let steps = create_open_steps(steps(popup.clone()));
    (popup, steps)
}
