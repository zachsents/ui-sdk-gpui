//! Port of `components/dialog/alert-dialog.ts`: Alert Dialog: Dialog's parts
//! and behavior, always modal, never closed by an outside press and never
//! presented as a sheet, and `confirm()` on top of it.

use std::cell::{Cell, RefCell};
use std::fmt::Debug;
use std::rc::Rc;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{self, LocalBoxFuture, Shared};

use gpui::{App, Context, Entity, SharedString, Window};
use ui_sdk_kernel::{Agent, AgentAction, AgentState, ChangeOptions, ConnectedController, Instance, Part};
use ui_sdk_motion::PresenceTiming;
use ui_sdk_shortcuts::ShortcutPart;
use ui_sdk_page::overlays::{Overlays, overlays as shared_overlays};
use ui_sdk_popups::{
    Modal, Popup, PopupCloseOptions, PopupContent, PopupControllerExt, PopupHost,
    PopupOpenOptions, PopupOptions, PopupPending, PopupResult, create_popup_controller,
};

use crate::dialog::{
    AlertDialogBackdropState, ConfirmEntry, ConfirmOpen, ConfirmOptions, AlertDialogCloseProps, AlertDialogCloseState, AlertDialogContentProps,
    AlertDialogContentState, AlertDialogController, AlertDialogOptions, AlertDialogShortcutProps,
    AlertDialogTriggerProps, AlertDialogTriggerState,
};
use crate::dialog_api::{DialogApi, DialogHost, DialogRules, DialogSettings, DialogState};
use crate::dialog_parts::{
    DialogBackdrop, dialog_backdrop, dialog_close, dialog_content, dialog_description,
    dialog_shortcut, dialog_title, dialog_trigger,
};
use crate::dialog_primitive::{
    DefinedDialog, close_action, define_dialog, dialog_agent_state, open_action, set_open_action,
    synced_modal, toggle_action,
};

/// Opens, closes and inspects an alert dialog from outside it, before it
/// exists too. Pass it as the `controller` option; `confirm()` asks whether
/// one is attached (`createAlertDialogController`). Its methods are
/// [`PopupControllerExt`]'s; `open()` resolves once it closes, with why and
/// the `value` of the Close part that closed it.
pub fn create_alert_dialog_controller<P: Clone + PartialEq + Debug + 'static>() -> AlertDialogController<P> {
    create_popup_controller()
}

/// What a question resolves with: whether someone agreed. Clones resolve
/// together.
pub type ConfirmAnswer = Shared<LocalBoxFuture<'static, bool>>;

struct ConfirmInner<P: Clone + PartialEq + Debug + 'static> {
    controller: AlertDialogController<P>,
    overlays: Overlays,
    // Each question waits for the one before it, so an answer never goes to
    // the wrong question.
    previous: RefCell<ConfirmAnswer>,
    unanswered: Cell<usize>,
    hosted: Cell<bool>,
}

/// Asks questions with an alert dialog and resolves with the answers, from
/// [`create_confirm`]. Clones are the same.
pub struct Confirm<P: Clone + PartialEq + Debug + 'static = ConfirmOptions> {
    inner: Rc<ConfirmInner<P>>,
}

impl<P: Clone + PartialEq + Debug + 'static> Clone for Confirm<P> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Asks a question with an alert dialog and resolves with the answer, for
/// `if confirm.ask(Some(question), window, cx).await { remove() }`
/// (`createConfirm`). With your own alert dialog, pass it
/// [`Confirm::controller`] and give the Close part that agrees the value
/// `"confirm"`. Without one, the overlay host renders an alert dialog from
/// the payload, one question at a time: `overlays` is its registry, the
/// shared one by default. Any other close, like Cancel or Escape, answers
/// no.
pub fn create_confirm<P: Clone + PartialEq + Debug + 'static>(overlays: Option<Overlays>) -> Confirm<P> {
    Confirm {
        inner: Rc::new(ConfirmInner {
            controller: create_alert_dialog_controller(),
            overlays: overlays.unwrap_or_else(shared_overlays),
            previous: RefCell::new(future::ready(false).boxed_local().shared()),
            unanswered: Cell::new(0),
            hosted: Cell::new(false),
        }),
    }
}

thread_local! {
    static CONFIRM: Confirm<ConfirmOptions> = create_confirm(None);
}

/// The shared confirm, which asks through the overlay host's alert dialog:
/// `confirm().ask(Some(ConfirmOptions::new("Delete this project?")), window, cx)`.
pub fn confirm() -> Confirm<ConfirmOptions> {
    CONFIRM.with(Confirm::clone)
}

impl<P: Clone + PartialEq + Debug + 'static> Confirm<P> {
    /// The alert dialog's controller. Pass it as the `controller` option of
    /// your own alert dialog.
    pub fn controller(&self) -> &AlertDialogController<P> {
        &self.inner.controller
    }

    // Your own alert dialog answers, unless the host's is showing.
    fn own(&self, cx: &App) -> bool {
        self.inner.controller.is_attached(cx) && !self.inner.hosted.get()
    }

    /// Asks `payload` and resolves with whether someone agreed. Call it
    /// outside the alert dialog's update. Your idle alert dialog opens at
    /// once, in the press that asked; any other question waits for the ones
    /// before it.
    pub fn ask(&self, payload: Option<P>, window: &mut Window, cx: &mut App) -> ConfirmAnswer {
        let asked = if self.inner.unanswered.get() == 0 && self.own(cx) {
            self.answer(payload, window, cx).shared()
        } else {
            let previous = self.inner.previous.borrow().clone();
            let confirm = self.clone();
            let handle = window.window_handle();
            let task = cx.spawn(async move |cx| {
                previous.await;
                let next = handle.update(cx, |_, window, cx| confirm.answer(payload, window, cx));
                match next {
                    Ok(next) => next.await,
                    Err(_) => false,
                }
            });
            task.boxed_local().shared()
        };
        self.inner.unanswered.set(self.inner.unanswered.get() + 1);
        {
            let confirm = self.clone();
            let asked = asked.clone();
            cx.spawn(async move |_| {
                asked.await;
                let left = confirm.inner.unanswered.get().saturating_sub(1);
                confirm.inner.unanswered.set(left);
            })
            .detach();
        }
        *self.inner.previous.borrow_mut() = asked.clone();
        asked
    }

    fn answer(&self, payload: Option<P>, window: &mut Window, cx: &mut App) -> LocalBoxFuture<'static, bool> {
        if !self.own(cx) {
            return self.host(payload, cx);
        }
        match self.inner.controller.open(PopupOpenOptions::new().payload(payload), window, cx) {
            Some(pending) => async move { agreed(&pending.await) }.boxed_local(),
            None => future::ready(false).boxed_local(),
        }
    }

    // The overlay host renders the question as an alert dialog.
    fn host(&self, payload: Option<P>, cx: &mut App) -> LocalBoxFuture<'static, bool> {
        let (sender, receiver) = oneshot::channel::<bool>();
        let sender = Rc::new(RefCell::new(Some(sender)));
        let opened = Rc::new(Cell::new(false));
        let id: Rc<RefCell<Option<SharedString>>> = Rc::new(RefCell::new(None));
        let open = {
            let controller = self.inner.controller.clone();
            let payload = payload.clone();
            let opened = opened.clone();
            let sender = sender.clone();
            // Once, even when a host asks twice.
            Rc::new(move |window: &mut Window, cx: &mut App| {
                if opened.replace(true) {
                    return;
                }
                let options = PopupOpenOptions::new().payload(payload.clone());
                let Some(pending) = controller.open(options, window, cx) else {
                    if let Some(sender) = sender.borrow_mut().take() {
                        sender.send(false).ok();
                    }
                    return;
                };
                let sender = sender.clone();
                cx.spawn(async move |_| {
                    let result = pending.await;
                    if let Some(sender) = sender.borrow_mut().take() {
                        sender.send(agreed(&result)).ok();
                    }
                })
                .detach();
            }) as ConfirmOpen
        };
        let done = {
            let confirm = Rc::downgrade(&self.inner);
            let id = id.clone();
            Rc::new(move |cx: &mut App| {
                let Some(confirm) = confirm.upgrade() else {
                    return;
                };
                confirm.hosted.set(false);
                let mounted = id.borrow_mut().take();
                if let Some(mounted) = mounted {
                    confirm.overlays.unmount(&mounted, cx);
                }
                // The open dialog answers itself, even when it settles after
                // this. A host that went away before opening it counts as a
                // no.
                if !opened.get()
                    && let Some(sender) = sender.borrow_mut().take()
                {
                    sender.send(false).ok();
                }
            }) as Rc<dyn Fn(&mut App)>
        };
        let entry = ConfirmEntry {
            controller: self.inner.controller.clone(),
            payload,
            open,
            done,
        };
        let mounted = self.inner.overlays.mount("confirm", Rc::new(entry), None, cx);
        *id.borrow_mut() = Some(mounted);
        self.inner.hosted.set(true);
        async move { receiver.await.unwrap_or(false) }.boxed_local()
    }
}

// Whether a close agreed: the Close part with the value `"confirm"` closed it.
fn agreed(result: &PopupResult) -> bool {
    result.return_value.as_deref() == Some("confirm")
}

/// An alert dialog: a question someone answers before an action goes on.
/// Its entity is its controller. `P` is the type of the data triggers and
/// `open()` pass to the content.
pub struct AlertDialog<P: Clone + PartialEq + Debug + 'static = ()> {
    instance: Instance,
    popup: Popup<Self>,
    dialog: DialogApi<Self>,
    controller: ConnectedController<Self>,
}

impl<P: Clone + PartialEq + Debug + 'static> AlertDialog<P> {
    pub fn new(options: AlertDialogOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let debug = options.debug;
        let controller = ConnectedController::new(options.controller.clone(), cx);
        let (popup, settings) = split(options);
        let DefinedDialog { instance, popup, dialog } =
            define_dialog("alert-dialog", true, popup, settings, debug, DialogRules::default(), window, cx);
        let mut this = Self {
            instance,
            popup,
            dialog,
            controller,
        };
        // Its preset names no `modal`: it keeps itself modal.
        let popup = this.popup.clone();
        popup.set_modal(&mut this, synced_modal(true, Modal::True), window, cx);
        this
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: AlertDialogOptions<P>, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        self.controller.set(options.controller.clone());
        let (popup, settings) = split(options);
        self.dialog.set_settings(settings);
        self.popup.set_options(popup, cx);
    }

    /// Passes the controlled `open`, or `None` to let it keep its own.
    pub fn set_controlled_open(&mut self, open: Option<bool>, cx: &mut Context<Self>) {
        self.popup.set_controlled_open(open, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// Its behavior, which its parts call.
    pub fn api(&self) -> &DialogApi<Self> {
        &self.dialog
    }

    /// The controller it's attached to, from its options.
    pub fn controller(&self) -> Option<&AlertDialogController<P>> {
        self.controller.controller()
    }

    /// Its state, as its parts and the app read it.
    pub fn state(&self) -> DialogState<P> {
        self.dialog.state(self)
    }

    /// Whether it's open. False as soon as it starts closing.
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// Opens or closes it. Returns false when the change was canceled or it
    /// was already open or closed. Closing this way always works, even while
    /// an action is running.
    pub fn set_open(&mut self, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.set_open(self, open, options, window, cx)
    }

    /// Opens it, and resolves once it closes, with why and the `value` of
    /// the Close part that closed it. A payload replaces the open one's.
    pub fn open(&mut self, options: PopupOpenOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        let popup = self.popup.clone();
        popup.open(self, options, window, cx)
    }

    /// Closes it, with a return value for the future `open()` returned.
    pub fn close(&mut self, options: PopupCloseOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.close(self, options, window, cx)
    }

    /// Opens it when it's closed, and closes it when it's open.
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.toggle(self, options, window, cx)
    }

    /// The payload of the last open.
    pub fn payload(&self) -> Option<P> {
        self.popup.payload()
    }

    /// Moves focus to the trigger that opened it last, or the first trigger.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.popup.focus(window, cx);
    }

    /// The `trigger` part: a button that opens it, and presses itself with
    /// its `shortcut`.
    pub fn trigger(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: AlertDialogTriggerProps<P>,
        cx: &App,
    ) -> Part<AlertDialogTriggerState> {
        dialog_trigger(this, key, props, cx)
    }

    /// The `backdrop` part, which dims the window behind it. Declare it
    /// before the content.
    pub fn backdrop(
        this: &Entity<Self>,
        timing: Option<PresenceTiming>,
        cx: &App,
    ) -> Option<DialogBackdrop<Self, AlertDialogBackdropState>> {
        dialog_backdrop(this, timing, cx)
    }

    /// The `content` part, with the `alertdialog` role, named by its title
    /// and described by its description. Opening it focuses its first Close
    /// part, so put the least destructive answer first. Render it every
    /// frame, so it hears its props while closed.
    pub fn content(
        this: &Entity<Self>,
        props: AlertDialogContentProps,
        cx: &App,
    ) -> Option<PopupContent<Self, AlertDialogContentState<P>>> {
        dialog_content(this, props, cx)
    }

    /// The `title` part: the question, which names it. Give it its text as an
    /// `aria_label`.
    pub fn title(this: &Entity<Self>, cx: &App) -> Part<()> {
        dialog_title(this, cx)
    }

    /// The `description` part: what happens if you agree. Give it its text
    /// as an `aria_label`.
    pub fn description(this: &Entity<Self>, cx: &App) -> Part<()> {
        dialog_description(this, cx)
    }

    /// The `close` part, one per answer, each with a key and the `value` it
    /// reports. `"confirm"` is the agreeing answer `confirm()` recognizes.
    pub fn close_part(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: AlertDialogCloseProps,
        cx: &App,
    ) -> Part<AlertDialogCloseState> {
        dialog_close(this, key, props, cx)
    }

    /// The `shortcut` part. Without keys of its own, it shows the shortcut
    /// of the trigger it sits in or beside.
    pub fn shortcut(
        this: &Entity<Self>,
        key: Option<SharedString>,
        props: AlertDialogShortcutProps,
        cx: &App,
    ) -> ShortcutPart {
        dialog_shortcut(this, key, props, cx)
    }
}

// What the popup reads, and what Dialog's behavior reads. An alert dialog is
// always modal and never closes on an outside press.
fn split<P: Clone + PartialEq + Debug + 'static>(options: AlertDialogOptions<P>) -> (PopupOptions<P>, DialogSettings) {
    let settings = DialogSettings {
        dismissible: options.dismissible,
        persistent_elements: options.persistent_elements,
        pending_announcement: options.pending_announcement,
        close_on_outside_press: Some(false),
    };
    let popup = PopupOptions {
        open: options.open,
        default_open: options.default_open,
        on_open_change: options.on_open_change,
        on_before_open_change: options.on_before_open_change,
        on_open_change_complete: options.on_open_change_complete,
        on_payload_change: options.on_payload_change,
        on_before_payload_change: options.on_before_payload_change,
        dismissible: options.dismissible,
        on_dismiss_attempt: options.on_dismiss_attempt,
        ..PopupOptions::default()
    };
    (popup, settings)
}

impl<P: Clone + PartialEq + Debug + 'static> PopupHost for AlertDialog<P> {
    type Payload = P;

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

impl<P: Clone + PartialEq + Debug + 'static> DialogHost for AlertDialog<P> {
    fn dialog(&self) -> &DialogApi<Self> {
        &self.dialog
    }
}

/// `agent: { actions: ["setOpen"] }`, with `open`, `close` and `toggle`,
/// which `setOpen` covers, unlisted.
impl<P: Clone + PartialEq + Debug + 'static> Agent for AlertDialog<P> {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", set_open_action::<Self>),
        AgentAction::unlisted("open", open_action::<Self>),
        AgentAction::unlisted("close", close_action::<Self>),
        AgentAction::unlisted("toggle", toggle_action::<Self>),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        dialog_agent_state(self)
    }
}
