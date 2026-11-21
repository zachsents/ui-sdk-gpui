//! Port of `components/dialog/dialog-api.ts`: Dialog's behavior, shared with
//! Drawer: opening and closing through the popup layer, nesting, focus,
//! dismissal policy, Close actions and trigger shortcuts. `dialog.rs`,
//! `alert_dialog.rs` and the drawer define their components on it, and
//! `dialog_parts.rs` holds the parts they share.
//!
//! A component built on Dialog holds the [`Popup`] and the [`DialogApi`]
//! [`dialog_api`] makes, and implements [`DialogHost`], which is how both
//! reach it from inside its update.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gpui::{App, Context, EntityId, FocusHandle, SharedString, Window};
use ui_sdk_focus::tabbables;
use ui_sdk_kernel::action::{ActionDetails, ActionError, ActionHandlers, AnnouncePending, run_action};
use ui_sdk_kernel::change::log_with;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{DismissAttemptReason, InputEvent, Instance, Reason, change_source, is_dismissal};
use ui_sdk_platform::{Layer, Region, TrackKeyboard, announce, is_apple, track_keyboard};
use ui_sdk_shortcuts::adopt_platform;
use ui_sdk_popups::{
    CloseDetails, CreatePopupOptions, EscapeFn, HideRequest, HookFn, InitialFocusFn, Modal,
    OpenChangeFn, OpenRule, Popup, PopupHost, PopupOpener, PopupOptions, PopupPending, PopupPreset,
    PopupRules, Presentation, Refusal, SheetConfig, ShowCleanup, ShowFn, ShowRequest, WeakPopup,
    alert_dialog_preset, create_popup, resolve_initial_focus,
};

use crate::dialog::DialogCloseProps;

/// Dialog's state: the popup's, and its own (`DialogState`).
#[derive(Clone, Debug, PartialEq)]
pub struct DialogState<P> {
    /// Whether it's open. False as soon as it starts closing.
    pub open: bool,
    /// True while it's in the page, including while it animates out.
    pub mounted: bool,
    /// Whether a title renders, which names it. `None` until the parts have
    /// rendered.
    pub titled: Option<bool>,
    /// Whether a description renders, which describes it.
    pub described: bool,
    /// The trigger that opened it last, by key.
    pub trigger: Option<SharedString>,
    /// The payload of the last open.
    pub payload: Option<P>,
    /// The `modal` in effect.
    pub modal: Modal,
    /// Opened from inside another dialog.
    pub nested: bool,
    /// How many dialogs are open above it.
    pub stack: usize,
    /// The Close part whose action is running, by key.
    pub pending: Option<SharedString>,
    /// Why actions failed, by Close part.
    pub errors: HashMap<SharedString, ActionError>,
    /// Apple keyboard conventions for shortcut hints.
    pub apple: bool,
    /// How it presents while open, decided as it opens. A drawer is always
    /// a sheet.
    pub presentation: Presentation,
}

/// Every content part built on Dialog, by slot name. Keys from one nested
/// inside another bubble through both, and only the innermost handles them
/// (`ownContent`).
pub const OWN_CONTENT: &[&str] = &["dialog-content", "alert-dialog-content", "drawer-content"];

// What a running action lets close the dialog: its api, the action itself,
// and the dialog it's nested in.
const CLOSES_WHILE_PENDING: [Reason; 3] = [Reason::Imperative, Reason::Action, Reason::ParentClose];

/// What [`dialog_refusal`] weighs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefusalPolicy {
    /// `dismissible`.
    pub dismissible: bool,
    /// Whether an outside press closes it.
    pub outside_press: bool,
    /// Whether a Close part's action is running.
    pub pending: bool,
    /// Whether it's beside the window rather than blocking it.
    pub beside: bool,
}

/// How Dialog refuses a close, for its dismissal (`dialogRefusal`). A
/// running action refuses every close but the api's, the action's own and
/// the parent dialog's. A dismissal needs `dismissible`, and an outside press
/// needs `close_on_outside_press` too. Refused dismissals are attempts the
/// app hears about, except a press beside a non-modal dialog, which is the
/// window's.
pub fn dialog_refusal(reason: Reason, policy: RefusalPolicy) -> Option<Refusal> {
    let dismissal = is_dismissal(reason);
    let refused = (policy.pending && !CLOSES_WHILE_PENDING.contains(&reason))
        || (dismissal
            && (!policy.dismissible || (reason == Reason::OutsidePress && !policy.outside_press)));
    if !refused {
        return None;
    }
    Some(if dismissal && !(reason == Reason::OutsidePress && policy.beside) {
        Refusal::Attempt
    } else {
        Refusal::Silent
    })
}

/// Regions that stay usable while a modal dialog blocks the window, like a
/// chat launcher or a password manager's button (`persistentElements`):
/// regions, or a function that returns them, read when the dialog opens.
#[derive(Clone)]
pub enum PersistentElements {
    List(Vec<Region>),
    Fn(Rc<dyn Fn() -> Vec<Region>>),
}

impl From<Vec<Region>> for PersistentElements {
    fn from(regions: Vec<Region>) -> Self {
        PersistentElements::List(regions)
    }
}

// Regions from `persistent_elements` (`resolveElements`).
fn resolve_elements(list: Option<&PersistentElements>) -> Vec<Region> {
    match list {
        None => Vec::new(),
        Some(PersistentElements::List(regions)) => regions.clone(),
        Some(PersistentElements::Fn(regions)) => regions(),
    }
}

/// What Dialog's behavior reads from a component's options, beside the
/// popup's: the shared options it acts on itself.
#[derive(Clone)]
pub struct DialogSettings {
    /// Whether Escape, an outside press, the back gesture and a swipe can
    /// close it.
    pub dismissible: bool,
    /// What stays usable while it blocks the window.
    pub persistent_elements: Option<PersistentElements>,
    /// What screen readers hear once a Close part's action has been running
    /// for a second.
    pub pending_announcement: Option<SharedString>,
    /// Whether an outside press closes it. True by default, or false when it
    /// isn't modal. An alert dialog never closes on one.
    pub close_on_outside_press: Option<bool>,
}

impl Default for DialogSettings {
    fn default() -> Self {
        Self {
            dismissible: true,
            persistent_elements: None,
            pending_announcement: None,
            close_on_outside_press: None,
        }
    }
}

/// A component built on Dialog: a popup host whose Dialog behavior it holds.
pub trait DialogHost: PopupHost {
    /// Its Dialog behavior, from [`dialog_api`].
    fn dialog(&self) -> &DialogApi<Self>;
}

/// Runs inside the component's update.
pub type DialogHook<T, A> = Rc<dyn Fn(&mut T, A, &mut Window, &mut Context<T>)>;

/// Hears a dismissal before it's decided, and returns true to handle it
/// instead.
pub type InterceptFn<T> = Rc<dyn Fn(&mut T, Reason, Option<&InputEvent>, &mut Window, &mut Context<T>) -> bool>;

/// What a component built on Dialog adds to its behavior, like a sheet
/// (`DialogRules`).
pub struct DialogRules<T: DialogHost> {
    /// Its preset: `dialog_preset`, which presents as a sheet, or Drawer's.
    /// Alert Dialog's is the default, so it doesn't present as one.
    pub preset: Option<PopupPreset>,
    /// More of what its content binds with, like the sheet it presents as,
    /// read from the component as the content binds.
    pub bind: Option<Rc<dyn Fn(&T) -> SheetConfig<T>>>,
    /// Runs each time the content binds, before the popup shows it, like a
    /// drawer's sheet attaching to it.
    pub on_bind: Option<HookFn<T>>,
    /// Runs once the content has left the page, like a drawer's sheet
    /// letting go of it.
    pub on_unbind: Option<HookFn<T>>,
    /// Runs when the popup shows, with its layer. Returns a cleanup.
    pub on_show: Option<ShowFn<T>>,
    /// Runs on open before it mounts, and on close before it animates out.
    pub on_open_change: Option<DialogHook<T, bool>>,
    /// Runs once an open or a close has finished, after any animation, a
    /// close once it's out of the page.
    pub on_open_change_complete: Option<DialogHook<T, bool>>,
    /// Runs before a dismissal is decided. Return true to handle it instead,
    /// like a sheet stepping down on the back gesture.
    pub intercept: Option<InterceptFn<T>>,
    /// Runs after a dismissal was refused, after `on_dismiss_attempt`.
    pub on_refuse: Option<DialogHook<T, DismissAttemptReason>>,
    /// How it presents when its preset doesn't decide: a dialog, or a
    /// drawer's sheet.
    pub presentation: Presentation,
}

impl<T: DialogHost> Default for DialogRules<T> {
    fn default() -> Self {
        Self {
            preset: None,
            bind: None,
            on_bind: None,
            on_unbind: None,
            on_show: None,
            on_open_change: None,
            on_open_change_complete: None,
            intercept: None,
            on_refuse: None,
            presentation: Presentation::Dialog,
        }
    }
}

impl<T: DialogHost> Clone for DialogRules<T> {
    fn clone(&self) -> Self {
        Self {
            preset: self.preset,
            bind: self.bind.clone(),
            on_bind: self.on_bind.clone(),
            on_unbind: self.on_unbind.clone(),
            on_show: self.on_show.clone(),
            on_open_change: self.on_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            intercept: self.intercept.clone(),
            on_refuse: self.on_refuse.clone(),
            presentation: self.presentation,
        }
    }
}

// What a dialog shows other dialogs nested inside it or around it.
#[derive(Clone)]
struct Nesting(Rc<NestingInner>);

struct NestingInner {
    stack: Cell<usize>,
    children: RefCell<Vec<Nesting>>,
    // The dialog this one is nested in.
    parent: RefCell<Option<Nesting>>,
    owner: Cell<Option<EntityId>>,
    close: RefCell<Option<NestedClose>>,
}

// Closes a nested dialog, with why.
type NestedClose = Rc<dyn Fn(Reason, &mut Window, &mut App)>;

impl PartialEq for Nesting {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Nesting {
    fn new() -> Self {
        Self(Rc::new(NestingInner {
            stack: Cell::new(0),
            children: RefCell::new(Vec::new()),
            parent: RefCell::new(None),
            owner: Cell::new(None),
            close: RefCell::new(None),
        }))
    }

    fn close(&self, reason: Reason, window: &mut Window, cx: &mut App) {
        let close = self.0.close.borrow().clone();
        if let Some(close) = close {
            close(reason, window, cx);
        }
    }

    fn stack(&self) -> usize {
        self.0.stack.get()
    }

    fn add(&self, child: &Nesting, cx: &mut App) {
        {
            let mut children = self.0.children.borrow_mut();
            if !children.contains(child) {
                children.push(child.clone());
            }
        }
        self.restack(Some(cx));
    }

    fn remove(&self, child: &Nesting, cx: Option<&mut App>) {
        self.0.children.borrow_mut().retain(|other| other != child);
        self.restack(cx);
    }

    // How many dialogs are open above it, which the one it's nested in
    // hears in turn.
    fn restack(&self, cx: Option<&mut App>) {
        let stack = self
            .0
            .children
            .borrow()
            .iter()
            .map(|child| child.stack() + 1)
            .max()
            .unwrap_or(0);
        if stack == self.0.stack.get() {
            return;
        }
        self.0.stack.set(stack);
        let parent = self.0.parent.borrow().clone();
        match cx {
            Some(cx) => {
                if let Some(owner) = self.0.owner.get() {
                    cx.notify(owner);
                }
                if let Some(parent) = parent {
                    parent.add(self, cx);
                }
            }
            None => {
                if let Some(parent) = parent {
                    parent.restack(None);
                }
            }
        }
    }
}

thread_local! {
    // Open dialogs by their layer, so a dialog opened from inside another
    // finds it.
    static DIALOG_LAYERS: RefCell<Vec<(Layer, Nesting)>> = const { RefCell::new(Vec::new()) };
}

fn nesting_of(layer: &Layer) -> Option<Nesting> {
    DIALOG_LAYERS.with(|layers| {
        layers
            .borrow()
            .iter()
            .find(|(own, _)| own == layer)
            .map(|(_, nesting)| nesting.clone())
    })
}

// A Close part as it rendered: its focus handle and region.
struct CloseEntry {
    key: SharedString,
    handle: FocusHandle,
    region: Region,
}

struct DialogInner<T: DialogHost> {
    alert: bool,
    settings: RefCell<DialogSettings>,
    nested: Cell<bool>,
    pending: RefCell<Option<SharedString>>,
    errors: RefCell<HashMap<SharedString, ActionError>>,
    apple: Cell<bool>,
    // Counts closes, so a Close action that outlives the open it started in
    // changes nothing.
    session: Cell<u64>,
    nesting: Nesting,
    announce: RefCell<AnnouncePending>,
    keyboard: TrackKeyboard,
    closes: RefCell<Vec<CloseEntry>>,
    shortcuts: Rc<RefCell<HashMap<SharedString, SharedString>>>,
    rules: DialogRules<T>,
    popup: RefCell<Option<WeakPopup<T>>>,
}

impl<T: DialogHost> Drop for DialogInner<T> {
    fn drop(&mut self) {
        let nesting = self.nesting.clone();
        DIALOG_LAYERS.with(|layers| {
            if let Ok(mut layers) = layers.try_borrow_mut() {
                layers.retain(|(_, own)| *own != nesting);
            }
        });
        let parent = nesting.0.parent.borrow_mut().take();
        if let Some(parent) = parent {
            parent.remove(&nesting, None);
        }
    }
}

/// Dialog's behavior, which its parts and its component call (`DialogApi`,
/// the original's `internal`). Clones share it.
pub struct DialogApi<T: DialogHost> {
    inner: Rc<DialogInner<T>>,
}

impl<T: DialogHost> Clone for DialogApi<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Builds a component's popup with Dialog's behavior (`dialogApi`): call it
/// in the constructor, with the component's instance, whether it's an alert
/// dialog, the options the popup reads and those Dialog reads, and what the
/// component adds. Keep both, and return them from [`PopupHost::popup`] and
/// [`DialogHost::dialog`].
///
/// A modal dialog wraps Tab, pauses the window's shortcuts and blocks the
/// window, its trigger included. A non-modal one is part of the window, and
/// focus leaving doesn't close it. Either stays open whatever has focus.
pub fn dialog_api<T: DialogHost>(
    instance: &Instance,
    alert: bool,
    options: PopupOptions<T::Payload>,
    settings: DialogSettings,
    rules: DialogRules<T>,
    window: &mut Window,
    cx: &mut Context<T>,
) -> (Popup<T>, DialogApi<T>) {
    let preset = rules.preset.unwrap_or_else(alert_dialog_preset);
    let bind = rules.bind.clone();
    let inner = Rc::new(DialogInner {
        alert,
        settings: RefCell::new(settings),
        nested: Cell::new(false),
        pending: RefCell::new(None),
        errors: RefCell::new(HashMap::new()),
        apple: Cell::new(is_apple(cx)),
        session: Cell::new(0),
        nesting: Nesting::new(),
        announce: RefCell::new(AnnouncePending::new()),
        keyboard: track_keyboard(None),
        closes: RefCell::new(Vec::new()),
        shortcuts: Rc::new(RefCell::new(HashMap::new())),
        rules,
        popup: RefCell::new(None),
    });
    let api = DialogApi { inner };
    let popup_rules = popup_rules(&api, bind);
    let role = if alert { Role::AlertDialog } else { Role::Dialog };
    let popup = create_popup(
        instance,
        CreatePopupOptions::new(preset)
            .role(role)
            .rules(popup_rules)
            .options(options),
        window,
        cx,
    );
    *api.inner.popup.borrow_mut() = Some(popup.weak());
    let entity = cx.weak_entity();
    *api.inner.nesting.0.close.borrow_mut() = Some(Rc::new(move |reason, window, cx| {
        // Once the update that closes the dialog around it is over: this one
        // may be what's closing it, like its Close part's action, and gpui
        // can't update it from inside its own update.
        let entity = entity.clone();
        window.defer(cx, move |window, cx| {
            entity
                .update(cx, |this, cx| {
                    let control = this.popup().control().clone();
                    control.hide(this, HideRequest::new().reason(reason), window, cx);
                })
                .ok();
        });
    }));
    api.inner.nesting.0.owner.set(Some(cx.entity_id()));
    (popup, api)
}

// How Dialog's behavior plugs into the popup: the popup's rules, each a
// step of `createPopup` that Dialog replaces or extends.
fn popup_rules<T: DialogHost>(api: &DialogApi<T>, bind: Option<Rc<dyn Fn(&T) -> SheetConfig<T>>>) -> PopupRules<T> {
    let on_escape: EscapeFn<T> = Rc::new(|this, event, window, cx| {
        let popup = this.popup().clone();
        // A non-modal dialog closes on Escape only while focus is inside it.
        if popup.modal() == Modal::False {
            let inside = match (popup.layer().element(), window.focused(cx)) {
                (Some(content), Some(focused)) => content.contains_focus(&focused, window),
                _ => false,
            };
            if !inside {
                return;
            }
        }
        popup.control().clone().hide(
            this,
            HideRequest::new()
                .reason(Reason::EscapeKey)
                .event(Some(InputEvent::KeyDown(event.clone()))),
            window,
            cx,
        );
    });
    let on_show: ShowFn<T> = Rc::new(|this, layer, window, cx| {
        let dialog = this.dialog().clone();
        dialog.shown(this, layer, window, cx)
    });
    let on_open_change: OpenChangeFn<T> = Rc::new(|this, open, _closed, window, cx| {
        let dialog = this.dialog().clone();
        dialog.open_change(this, open, window, cx);
    });
    let on_open_change_complete: Option<OpenChangeFn<T>> = api.inner.rules.on_open_change_complete.clone().map(|complete| {
        Rc::new(move |this: &mut T, open: bool, _closed: Option<&ui_sdk_popups::PopupClose>, window: &mut Window, cx: &mut Context<T>| {
            complete(this, open, window, cx)
        }) as OpenChangeFn<T>
    });
    // Close parts' errors are from the last time it was open.
    let open: OpenRule<T, T::Payload> = Rc::new(|this, _request, step, window, cx| {
        this.dialog().inner.errors.borrow_mut().clear();
        step.open(this, None, window, cx);
    });
    let preferred_focus = api.inner.alert.then(|| {
        let weak = Rc::downgrade(&api.inner);
        Rc::new(move |window: &mut Window, cx: &mut App| preferred_close(&weak, window, cx)) as InitialFocusFn
    });
    PopupRules {
        persistent: Some(Rc::new(|this: &T| this.dialog().persistent_elements())),
        on_escape: Some(on_escape),
        on_show: Some(on_show),
        on_open_change: Some(on_open_change),
        on_open_change_complete,
        open: Some(open),
        // How Dialog refuses a close, before any rule hears it.
        refusal: Some(Rc::new(|this: &T, reason| this.dialog().refusal(this, reason))),
        refused: Some(Rc::new(|this: &mut T, reason, window, cx| {
            let on_refuse = this.dialog().inner.rules.on_refuse.clone();
            if let Some(on_refuse) = on_refuse {
                on_refuse(this, reason, window, cx);
            }
        })),
        dismiss: Some(Rc::new(|this: &mut T, reason, details, window, cx| {
            let dialog = this.dialog().clone();
            dialog.dismiss(this, reason, details, window, cx);
        })),
        // `Auto` focuses an alert dialog's least destructive action, its
        // first Close part, before the first tab stop.
        preferred_focus,
        bind,
        on_bind: api.inner.rules.on_bind.clone(),
        on_unbind: api.inner.rules.on_unbind.clone(),
        ..PopupRules::default()
    }
}

// An alert dialog's first Close part that Tab reaches, in the order Tab
// visits the content.
fn preferred_close<T: DialogHost>(weak: &Weak<DialogInner<T>>, window: &mut Window, cx: &mut App) -> Option<FocusHandle> {
    let inner = weak.upgrade()?;
    let popup = inner.popup.borrow().as_ref().and_then(WeakPopup::upgrade)?;
    let root = popup.content_region().focus_handle()?;
    let closes: Vec<FocusHandle> = inner.closes.borrow().iter().map(|close| close.handle.clone()).collect();
    tabbables(&root, window, cx)
        .into_iter()
        .find(|handle| closes.contains(handle))
}

impl<T: DialogHost> DialogApi<T> {
    /// Whether it's an alert dialog, always modal and never closed by an
    /// outside press.
    pub fn alert(&self) -> bool {
        self.inner.alert
    }

    /// What it reads from the component's options.
    pub fn settings(&self) -> DialogSettings {
        self.inner.settings.borrow().clone()
    }

    /// New settings, as the component's options arrive.
    pub fn set_settings(&self, settings: DialogSettings) {
        *self.inner.settings.borrow_mut() = settings;
    }

    /// Its state: the popup's and its own.
    pub fn state(&self, this: &T) -> DialogState<T::Payload> {
        let popup = this.popup().state(this);
        let presentation = if this.popup().presentation().is_some() {
            popup.presentation
        } else {
            self.inner.rules.presentation
        };
        DialogState {
            open: popup.open,
            mounted: popup.mounted,
            titled: popup.titled,
            described: popup.described,
            trigger: popup.trigger,
            payload: popup.payload,
            modal: popup.modal,
            nested: self.nested(),
            stack: self.stack(),
            pending: self.pending(),
            errors: self.inner.errors.borrow().clone(),
            apple: self.inner.apple.get(),
            presentation,
        }
    }

    /// Whether it was opened from inside another dialog.
    pub fn nested(&self) -> bool {
        self.inner.nested.get()
    }

    /// How many dialogs are open above it, nested inside it.
    pub fn stack(&self) -> usize {
        self.inner.nesting.stack()
    }

    /// The Close part whose action is running, by key.
    pub fn pending(&self) -> Option<SharedString> {
        self.inner.pending.borrow().clone()
    }

    /// Why the action of the Close part with `key` failed, until it's
    /// pressed again or the dialog opens again.
    pub fn error(&self, key: &SharedString) -> Option<ActionError> {
        self.inner.errors.borrow().get(key).cloned()
    }

    /// Whether shortcut hints follow Apple's conventions.
    pub fn apple(&self) -> bool {
        self.inner.apple.get()
    }

    /// Follows the platform shortcuts are named for, as a part renders
    /// (`adoptPlatform`).
    pub fn adopt_platform(&self, cx: &App) {
        let mut apple = self.inner.apple.get();
        if adopt_platform(&mut apple, cx) {
            self.inner.apple.set(apple);
        }
    }

    /// What follows the on-screen keyboard for the content.
    pub fn keyboard(&self) -> &TrackKeyboard {
        &self.inner.keyboard
    }

    /// Whether an outside press closes it: never an alert dialog's, and by
    /// default only a modal one's.
    pub fn closes_on_outside_press(&self, this: &T) -> bool {
        !self.inner.alert
            && self
                .inner
                .settings
                .borrow()
                .close_on_outside_press
                .unwrap_or(this.popup().modal() != Modal::False)
    }

    /// The regions that stay usable while it blocks the window.
    pub fn persistent_elements(&self) -> Vec<Region> {
        resolve_elements(self.inner.settings.borrow().persistent_elements.as_ref())
    }

    /// How it refuses a close (`refusal`).
    pub fn refusal(&self, this: &T, reason: Reason) -> Option<Refusal> {
        dialog_refusal(
            reason,
            RefusalPolicy {
                dismissible: self.inner.settings.borrow().dismissible,
                outside_press: self.closes_on_outside_press(this),
                pending: self.pending().is_some(),
                beside: this.popup().modal() != Modal::True && !self.inner.alert,
            },
        )
    }

    // It shows: it joins the dialog it opened from, if any.
    fn shown(&self, this: &mut T, layer: &Layer, window: &mut Window, cx: &mut Context<T>) -> Option<ShowCleanup<T>> {
        let nesting = self.inner.nesting.clone();
        DIALOG_LAYERS.with(|layers| layers.borrow_mut().push((layer.clone(), nesting.clone())));
        let mut parent = None;
        let mut up = layer.parent();
        while parent.is_none()
            && let Some(current) = up
        {
            parent = nesting_of(&current);
            up = current.parent();
        }
        *nesting.0.parent.borrow_mut() = parent.clone();
        if let Some(parent) = &parent {
            parent.add(&nesting, cx);
        }
        self.inner.nested.set(parent.is_some());
        let cleanup = self
            .inner
            .rules
            .on_show
            .clone()
            .and_then(|on_show| on_show(this, layer, window, cx));
        let layer = layer.clone();
        Some(Box::new(move |this: &mut T, window: &mut Window, cx: &mut Context<T>| {
            if let Some(cleanup) = cleanup {
                cleanup(this, window, cx);
            }
            DIALOG_LAYERS.with(|layers| layers.borrow_mut().retain(|(own, _)| *own != layer));
        }))
    }

    // It starts opening or closing. A close settles what it was doing.
    fn open_change(&self, this: &mut T, open: bool, window: &mut Window, cx: &mut Context<T>) {
        let rule = self.inner.rules.on_open_change.clone();
        if let Some(rule) = rule {
            rule(this, open, window, cx);
        }
        if open {
            return;
        }
        self.inner.session.set(self.inner.session.get() + 1);
        self.set_pending(None, window, cx);
        // Nested dialogs close too, once this update is over. Focus inside
        // them counts as inside this one, so it goes back to where this one
        // opened from.
        let children = self.inner.nesting.0.children.borrow().clone();
        for child in children {
            child.close(Reason::ParentClose, window, cx);
        }
        let parent = self.inner.nesting.0.parent.borrow_mut().take();
        if let Some(parent) = parent {
            parent.remove(&self.inner.nesting, Some(cx));
        }
    }

    // A dismissal the popup layer saw, unless the component handles it
    // itself.
    fn dismiss(&self, this: &mut T, reason: Reason, details: CloseDetails, window: &mut Window, cx: &mut Context<T>) {
        let intercept = self.inner.rules.intercept.clone();
        if let Some(intercept) = intercept
            && intercept(this, reason, details.event.as_ref(), window, cx)
        {
            return;
        }
        this.popup().control().clone().hide(
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

    fn set_pending(&self, pending: Option<SharedString>, window: &mut Window, cx: &mut Context<T>) {
        if *self.inner.pending.borrow() == pending {
            return;
        }
        let waiting = pending.is_some();
        *self.inner.pending.borrow_mut() = pending;
        let text = self.inner.settings.borrow().pending_announcement.clone();
        self.inner
            .announce
            .borrow_mut()
            .update(waiting, text, announce, window, cx);
        cx.notify();
    }

    /// Asks to open (`show`), like a trigger or its shortcut.
    pub fn show(&self, this: &mut T, request: ShowRequest<T::Payload>, window: &mut Window, cx: &mut Context<T>) -> PopupPending {
        this.popup().control().clone().show(this, request, None, window, cx)
    }

    /// Asks to close (`hide`). Returns whether it closed.
    pub fn hide(&self, this: &mut T, request: HideRequest, window: &mut Window, cx: &mut Context<T>) -> bool {
        this.popup().control().clone().hide(this, request, window, cx)
    }

    /// Another trigger takes the open popup over, with its payload
    /// (`takeOver`).
    #[allow(clippy::too_many_arguments)]
    pub fn take_over(
        &self,
        this: &mut T,
        trigger: SharedString,
        payload: Option<T::Payload>,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        this.popup()
            .control()
            .clone()
            .take_over(this, trigger, payload, None, reason, event, window, cx);
    }

    /// Where focus goes on open, from the content's `initial_focus`.
    pub fn initial_focus(&self, this: &T, window: &mut Window, cx: &mut App) -> Option<FocusHandle> {
        let popup = this.popup();
        let content = popup.layer().element()?;
        let setting = popup.content_props().initial_focus;
        let preferred = popup.rules().preferred_focus.clone();
        resolve_initial_focus(&content, &setting, preferred.as_ref(), window, cx)
    }

    /// A Close part was pressed. Its action runs first, and work that goes
    /// on keeps the dialog open until it settles (`closePress`).
    pub fn close_press(
        &self,
        this: &mut T,
        key: SharedString,
        props: DialogCloseProps,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let popup = this.popup().clone();
        if !popup.is_open() || self.pending().is_some() {
            return;
        }
        let Some(action) = props.action.clone() else {
            popup.control().clone().hide(
                this,
                HideRequest::new()
                    .reason(Reason::ClosePress)
                    .event(event)
                    .return_value(props.value.clone()),
                window,
                cx,
            );
            return;
        };
        if self.inner.errors.borrow_mut().remove(&key).is_some() {
            cx.notify();
        }
        let source = change_source(Reason::Action, event.as_ref());
        // A close after waiting has no event: the press is long over.
        let waited = Rc::new(Cell::new(false));
        let started = self.inner.session.get();
        let pressed = event.clone();
        let run = move |details: &ActionDetails, window: &mut Window, cx: &mut App| {
            action(pressed.as_ref(), details, window, cx)
        };
        let pending_key = key.clone();
        let pending_waited = waited.clone();
        let failed_key = key;
        let value = props.value;
        run_action(
            this,
            &run,
            source,
            ActionHandlers {
                pending: Box::new(move |this: &mut T, pending, window, cx| {
                    let dialog = this.dialog().clone();
                    if started != dialog.inner.session.get() {
                        return;
                    }
                    pending_waited.set(true);
                    dialog.set_pending(pending.then(|| pending_key.clone()), window, cx);
                    log_with(this.instance(), "pending", &pending, Reason::Action, Some(source));
                }),
                done: Box::new(move |this: &mut T, window, cx| {
                    if started != this.dialog().inner.session.get() {
                        return;
                    }
                    let event = if waited.get() { None } else { event.clone() };
                    this.popup().control().clone().hide(
                        this,
                        HideRequest::new()
                            .reason(Reason::Action)
                            .event(event)
                            .return_value(value.clone()),
                        window,
                        cx,
                    );
                }),
                failed: Box::new(move |this: &mut T, error, _window, cx| {
                    let dialog = this.dialog().clone();
                    if started != dialog.inner.session.get() {
                        return;
                    }
                    log_with(this.instance(), "error", &error, Reason::Action, Some(source));
                    dialog.inner.errors.borrow_mut().insert(failed_key.clone(), error);
                    cx.notify();
                }),
            },
            window,
            cx,
        );
    }

    /// A trigger's shortcut presses it: it opens the dialog, or closes one
    /// that isn't modal, whose window shortcuts still run (`shortcutPress`).
    pub fn shortcut_press(
        &self,
        this: &mut T,
        key: &SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let popup = this.popup().clone();
        let props = popup.trigger_props(key).unwrap_or_default();
        if props.disabled {
            return;
        }
        let control = popup.control().clone();
        if popup.is_open() {
            control.hide(this, HideRequest::new().reason(Reason::Shortcut).event(event), window, cx);
            return;
        }
        control.show(
            this,
            ShowRequest {
                reason: Some(Reason::Shortcut),
                event,
                payload: Some(props.payload),
                by: Some(PopupOpener::Keyboard),
                trigger: Some(key.clone()),
                ..ShowRequest::default()
            },
            None,
            window,
            cx,
        );
    }

    /// The shortcut the trigger with `key` rendered with, which a Shortcut
    /// part in it or beside it shows (`shortcutOf`).
    pub fn shortcut_of(&self, key: &SharedString) -> Option<SharedString> {
        self.inner.shortcuts.borrow().get(key).cloned()
    }

    /// The shortcuts the triggers rendered with, by key, for a Shortcut part
    /// to look up where it renders.
    pub fn trigger_shortcuts(&self) -> Rc<RefCell<HashMap<SharedString, SharedString>>> {
        self.inner.shortcuts.clone()
    }

    /// Records the shortcut the trigger with `key` renders with.
    pub fn set_trigger_shortcut(&self, key: &SharedString, shortcut: Option<SharedString>) {
        let mut shortcuts = self.inner.shortcuts.borrow_mut();
        match shortcut {
            Some(shortcut) => {
                shortcuts.insert(key.clone(), shortcut);
            }
            None => {
                shortcuts.remove(key);
            }
        }
    }

    /// The backdrop's region while a backdrop part is in the page
    /// (`backdropElement`).
    pub fn backdrop_element(&self, this: &T) -> Option<Region> {
        let popup = this.popup();
        popup
            .layer()
            .backdrop_element()
            .or_else(|| ui_sdk_kernel::held(this.instance(), "backdrop", None).then(|| popup.backdrop_region()))
    }

    /// The focus handle and region of the Close part with `key`, made the
    /// first time it renders.
    pub fn close_handle(&self, key: &SharedString, cx: &App) -> (FocusHandle, Region) {
        let mut closes = self.inner.closes.borrow_mut();
        if let Some(close) = closes.iter().find(|close| close.key == *key) {
            return (close.handle.clone(), close.region.clone());
        }
        let handle = cx.focus_handle();
        let region = Region::with_focus_handle(handle.clone());
        closes.push(CloseEntry {
            key: key.clone(),
            handle: handle.clone(),
            region: region.clone(),
        });
        (handle, region)
    }

    /// The regions of the Close parts, which a sheet doesn't drag from.
    pub fn close_regions(&self) -> Vec<Region> {
        self.inner.closes.borrow().iter().map(|close| close.region.clone()).collect()
    }
}
