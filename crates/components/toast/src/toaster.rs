//! Port of `components/toast/toaster.ts`: the toast queue. `toast(cx).show("Saved", cx)`
//! works from anywhere there's an app, and a region, rendered by the overlay
//! host or placed by the app, shows it. The queue holds data and decides
//! nothing about time or focus; the region runs timers only for toasts it
//! shows, so a toast never expires unseen.
//!
//! A toaster is an entity the regions that show it observe: every change
//! notifies it. Its handle, [`Toaster`], is cheap to clone, and its calls run
//! `on_dismiss` outside the queue's own update, so a callback may add or
//! update toasts.

use std::any::Any;
use std::cell::Cell;
use std::fmt;
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, Global, SharedString, Subscription, Window};
use ui_sdk_kernel::{
    ActionDetails, ActionOutcome, ChangeDetails, InputEvent, Reason, change_source, create_details,
    with_source,
};
use ui_sdk_page::overlays::{Overlays, RenderedFn, overlays};
use ui_sdk_platform::AnnouncePriority;

/// What a toast is about. `Error` is announced assertively, and `Loading`
/// stays until updated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ToastType {
    #[default]
    Default,
    Success,
    Error,
    Info,
    Warning,
    Loading,
}

impl ToastType {
    /// The original's name for it, its `data-type`, like `"success"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ToastType::Default => "default",
            ToastType::Success => "success",
            ToastType::Error => "error",
            ToastType::Info => "info",
            ToastType::Warning => "warning",
            ToastType::Loading => "loading",
        }
    }
}

/// Why a toast closed: `Timeout`, `ClosePress`, `Swipe`, `EscapeKey`,
/// `Action` or `Imperative`, from the shared vocabulary.
pub type ToastDismissReason = Reason;

/// What `on_dismiss` receives. `cancel()` keeps the toast, like a swipe that
/// snaps back.
pub type ToastDismissDetails = ChangeDetails;

/// What a toast's action and close buttons receive after the event: how the
/// button was pressed.
pub type ToastActionDetails = ActionDetails;

/// What an action runs when it's pressed: the event behind it, when there is
/// one, and how it was pressed. It runs inside the region's update, so it may
/// update any entity but the region. Return [`ActionOutcome::wait`] with work
/// that goes on to mark the action pending, which dismisses the toast once
/// it succeeds; a failure keeps the toast.
pub type ToastActionHandler =
    Rc<dyn Fn(Option<&InputEvent>, &ToastActionDetails, &mut Window, &mut App) -> ActionOutcome>;

/// What a cancel button runs when it's pressed, before the toast closes.
pub type ToastCancelHandler =
    Rc<dyn Fn(Option<&InputEvent>, &ToastActionDetails, &mut Window, &mut App)>;

/// What `on_dismiss` runs as a toast closes, with why. It runs inside the
/// update of what closed it, like the region a swipe or a timer closed it
/// in, so it may update any entity but that one. Adding, updating and
/// dismissing toasts is fine: the queue isn't in an update then.
pub type ToastDismissCallback = Rc<dyn Fn(&ToastDismissDetails, &mut App)>;

/// A button in the toast that does something, like Undo.
#[derive(Clone)]
pub struct ToastAction {
    /// The button's text.
    pub label: SharedString,
    /// Runs when it's pressed. Work it returns that goes on marks the action
    /// pending, then dismisses the toast once it succeeds. A failure keeps the
    /// toast. `details.source` says how it was pressed, for changes made after
    /// the wait.
    pub on_action: Option<ToastActionHandler>,
    /// Makes the action a link. Where it goes is the app's to follow, as
    /// gpui has no browser to navigate.
    pub href: Option<SharedString>,
    /// Whether pressing it dismisses the toast. `None` dismisses.
    pub dismiss: Option<bool>,
}

impl ToastAction {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            on_action: None,
            href: None,
            dismiss: None,
        }
    }

    pub fn on_action(
        mut self,
        on_action: impl Fn(
            Option<&InputEvent>,
            &ToastActionDetails,
            &mut Window,
            &mut App,
        ) -> ActionOutcome
        + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(on_action));
        self
    }

    pub fn href(mut self, href: impl Into<SharedString>) -> Self {
        self.href = Some(href.into());
        self
    }

    pub fn dismiss(mut self, dismiss: bool) -> Self {
        self.dismiss = Some(dismiss);
        self
    }
}

impl fmt::Debug for ToastAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToastAction")
            .field("label", &self.label)
            .field("on_action", &self.on_action.is_some())
            .field("href", &self.href)
            .field("dismiss", &self.dismiss)
            .finish()
    }
}

/// A button that dismisses the toast, like Cancel.
#[derive(Clone)]
pub struct ToastCancel {
    /// The button's text.
    pub label: SharedString,
    /// Runs when it's pressed, before the toast closes. `details.source` says
    /// how it was pressed.
    pub on_cancel: Option<ToastCancelHandler>,
}

impl ToastCancel {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            on_cancel: None,
        }
    }

    pub fn on_cancel(
        mut self,
        on_cancel: impl Fn(Option<&InputEvent>, &ToastActionDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_cancel = Some(Rc::new(on_cancel));
        self
    }
}

impl fmt::Debug for ToastCancel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToastCancel")
            .field("label", &self.label)
            .field("on_cancel", &self.on_cancel.is_some())
            .finish()
    }
}

/// Everything a toast can say and do. Every field is optional, and one left
/// out of an update keeps the toast's. Text converts to options with that
/// title, so `"Saved"` is a toast, and `ToastOptions::from("Deleted").action(..)`
/// is the original's `toast("Deleted", { action })`.
#[derive(Clone, Default)]
pub struct ToastOptions {
    /// Reuses a toast: a toast with this id that's still showing is updated
    /// instead of added again.
    pub id: Option<SharedString>,
    /// The main message.
    pub title: Option<SharedString>,
    /// More detail, under the title.
    pub description: Option<SharedString>,
    /// What it's about, for styling and for how it's announced. `None` is
    /// `Default`.
    pub r#type: Option<ToastType>,
    /// How long it shows while nothing pauses it. `Duration::MAX` keeps it
    /// until it's dismissed, the original's `Infinity`. `None` takes the
    /// toaster's, 5 seconds unless it says otherwise, and `Duration::MAX`
    /// for `Loading`.
    pub duration: Option<Duration>,
    /// How screen readers announce it: `Polite` waits for a pause, `Assertive`
    /// interrupts. `None` is `Assertive` for errors, `Polite` otherwise.
    pub priority: Option<AnnouncePriority>,
    /// A button that does something, like Undo.
    pub action: Option<ToastAction>,
    /// A button that dismisses it, like Cancel.
    pub cancel: Option<ToastCancel>,
    /// Whether the user can dismiss it: swiping, Escape and Close parts.
    /// Timers and `dismiss()` still close it. `None` is true.
    pub dismissible: Option<bool>,
    /// Anything else, for your own rendering, like an avatar.
    pub data: Option<Rc<dyn Any>>,
    /// Runs when it closes, with why. Calling `details.cancel()` keeps it. It
    /// runs inside the update of what closed it, like the region, so it may
    /// update any entity but that one.
    pub on_dismiss: Option<ToastDismissCallback>,
}

impl ToastOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn r#type(mut self, r#type: ToastType) -> Self {
        self.r#type = Some(r#type);
        self
    }

    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    pub fn priority(mut self, priority: AnnouncePriority) -> Self {
        self.priority = Some(priority);
        self
    }

    pub fn action(mut self, action: ToastAction) -> Self {
        self.action = Some(action);
        self
    }

    pub fn cancel(mut self, cancel: ToastCancel) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = Some(dismissible);
        self
    }

    pub fn data(mut self, data: impl Any) -> Self {
        self.data = Some(Rc::new(data));
        self
    }

    pub fn on_dismiss(
        mut self,
        on_dismiss: impl Fn(&ToastDismissDetails, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// These options with `extra`'s on top, where it sets them (the
    /// original's `{ ...options, ...extra }`).
    pub fn merge(self, extra: ToastOptions) -> Self {
        Self {
            id: extra.id.or(self.id),
            title: extra.title.or(self.title),
            description: extra.description.or(self.description),
            r#type: extra.r#type.or(self.r#type),
            duration: extra.duration.or(self.duration),
            priority: extra.priority.or(self.priority),
            action: extra.action.or(self.action),
            cancel: extra.cancel.or(self.cancel),
            dismissible: extra.dismissible.or(self.dismissible),
            data: extra.data.or(self.data),
            on_dismiss: extra.on_dismiss.or(self.on_dismiss),
        }
    }
}

impl From<&str> for ToastOptions {
    fn from(title: &str) -> Self {
        Self::new().title(SharedString::from(title.to_string()))
    }
}

impl From<String> for ToastOptions {
    fn from(title: String) -> Self {
        Self::new().title(title)
    }
}

impl From<SharedString> for ToastOptions {
    fn from(title: SharedString) -> Self {
        Self::new().title(title)
    }
}

impl From<&SharedString> for ToastOptions {
    fn from(title: &SharedString) -> Self {
        Self::new().title(title.clone())
    }
}

impl fmt::Debug for ToastOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToastOptions")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("description", &self.description)
            .field("type", &self.r#type)
            .field("duration", &self.duration)
            .field("priority", &self.priority)
            .field("action", &self.action)
            .field("cancel", &self.cancel)
            .field("dismissible", &self.dismissible)
            .field("data", &self.data.is_some())
            .field("on_dismiss", &self.on_dismiss.is_some())
            .finish()
    }
}

/// A toast in the queue, as regions render it.
#[derive(Clone)]
pub struct Toast {
    /// Unique within its toaster.
    pub id: SharedString,
    pub title: Option<SharedString>,
    pub description: Option<SharedString>,
    /// What it's about.
    pub r#type: ToastType,
    /// How long it shows. `Duration::MAX` stays until it's dismissed.
    pub duration: Duration,
    /// How screen readers announce it.
    pub priority: AnnouncePriority,
    pub action: Option<ToastAction>,
    pub cancel: Option<ToastCancel>,
    /// Whether the user can dismiss it.
    pub dismissible: bool,
    pub data: Option<Rc<dyn Any>>,
    pub on_dismiss: Option<ToastDismissCallback>,
    /// Increases with every update, so a region announces it again and
    /// restarts its timer.
    pub version: u64,
    /// When it was added, relative to the others. Newer toasts have a higher
    /// order.
    pub order: u64,
}

impl Toast {
    /// Its data, as the type it was given as.
    pub fn data<D: 'static>(&self) -> Option<&D> {
        self.data.as_ref()?.downcast_ref()
    }
}

/// Two snapshots of a toast are the same while neither was updated.
impl PartialEq for Toast {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.version == other.version && self.order == other.order
    }
}

impl fmt::Debug for Toast {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Toast")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("description", &self.description)
            .field("type", &self.r#type)
            .field("duration", &self.duration)
            .field("priority", &self.priority)
            .field("action", &self.action)
            .field("cancel", &self.cancel)
            .field("dismissible", &self.dismissible)
            .field("data", &self.data.is_some())
            .field("version", &self.version)
            .field("order", &self.order)
            .finish()
    }
}

/// A message, or the options for a toast, or a function of the settled value
/// that returns either.
pub enum PromiseMessage<V> {
    Options(ToastOptions),
    With(Rc<dyn Fn(&V) -> ToastOptions>),
}

impl<V> PromiseMessage<V> {
    /// A message made from the settled value, like an error's text.
    pub fn with(message: impl Fn(&V) -> ToastOptions + 'static) -> Self {
        PromiseMessage::With(Rc::new(message))
    }

    fn resolve(&self, value: &V) -> ToastOptions {
        match self {
            PromiseMessage::Options(options) => options.clone(),
            PromiseMessage::With(message) => message(value),
        }
    }
}

impl<V> Clone for PromiseMessage<V> {
    fn clone(&self) -> Self {
        match self {
            PromiseMessage::Options(options) => PromiseMessage::Options(options.clone()),
            PromiseMessage::With(message) => PromiseMessage::With(message.clone()),
        }
    }
}

impl<V> From<ToastOptions> for PromiseMessage<V> {
    fn from(options: ToastOptions) -> Self {
        PromiseMessage::Options(options)
    }
}

impl<V> From<&str> for PromiseMessage<V> {
    fn from(title: &str) -> Self {
        PromiseMessage::Options(title.into())
    }
}

impl<V> From<String> for PromiseMessage<V> {
    fn from(title: String) -> Self {
        PromiseMessage::Options(title.into())
    }
}

impl<V> From<SharedString> for PromiseMessage<V> {
    fn from(title: SharedString) -> Self {
        PromiseMessage::Options(title.into())
    }
}

/// What [`Toaster::promise`] shows: a loading toast while the work runs, then
/// the same toast as a success or an error.
pub struct ToastPromise<V, E> {
    /// While it's pending.
    pub loading: ToastOptions,
    /// Once it succeeds. Without it, the toast closes.
    pub success: Option<PromiseMessage<V>>,
    /// Once it fails. Without it, the toast closes.
    pub error: Option<PromiseMessage<E>>,
}

impl<V, E> ToastPromise<V, E> {
    pub fn new(loading: impl Into<ToastOptions>) -> Self {
        Self {
            loading: loading.into(),
            success: None,
            error: None,
        }
    }

    pub fn success(mut self, success: impl Into<PromiseMessage<V>>) -> Self {
        self.success = Some(success.into());
        self
    }

    pub fn error(mut self, error: impl Into<PromiseMessage<E>>) -> Self {
        self.error = Some(error.into());
        self
    }
}

thread_local! {
    static IDS: Cell<u64> = const { Cell::new(0) };
    static ORDERS: Cell<u64> = const { Cell::new(0) };
}

fn next(counter: &'static std::thread::LocalKey<Cell<u64>>) -> u64 {
    counter.with(|counter| {
        counter.set(counter.get() + 1);
        counter.get()
    })
}

/// The kind of overlay a toaster opens in its registry (`"toaster"`). The
/// entry's `data` is the [`Toaster`] whose region the host renders, with
/// `placed(false)`.
pub const TOASTER_OVERLAY: &str = "toaster";

/// What [`create_toaster`] takes.
#[derive(Clone)]
pub struct ToasterOptions {
    /// How long toasts show, unless a toast says otherwise. Defaults to 5
    /// seconds.
    pub duration: Duration,
    /// The registry whose host renders this toaster's region when the app
    /// hasn't placed one. `None` is the shared `overlays()`.
    pub overlays: Option<Overlays>,
}

impl Default for ToasterOptions {
    fn default() -> Self {
        Self {
            duration: Duration::from_millis(5000),
            overlays: None,
        }
    }
}

impl ToasterOptions {
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    pub fn overlays(mut self, overlays: Overlays) -> Self {
        self.overlays = Some(overlays);
        self
    }
}

// The queue a `Toaster` is a handle to, which regions observe.
pub(crate) struct ToasterQueue {
    // Newest first.
    toasts: Vec<Toast>,
    duration: Duration,
    overlays: Overlays,
    placed_regions: usize,
    mounted: bool,
}

impl ToasterQueue {
    fn find(&self, id: &str) -> Option<&Toast> {
        self.toasts.iter().find(|toast| toast.id.as_ref() == id)
    }

    // Fills in what a toast leaves out, from its type.
    fn complete(&self, options: ToastOptions, previous: Option<&Toast>) -> Toast {
        let r#type = options
            .r#type
            .or(previous.map(|previous| previous.r#type))
            .unwrap_or_default();
        // A loading toast that becomes a result starts a fresh duration of
        // its own.
        let type_changed = previous.is_some_and(|previous| previous.r#type != r#type);
        let kept = previous.filter(|_| !type_changed);
        let duration = options
            .duration
            .or(kept.map(|previous| previous.duration))
            .unwrap_or(if r#type == ToastType::Loading {
                Duration::MAX
            } else {
                self.duration
            });
        let priority = options
            .priority
            .or(kept.map(|previous| previous.priority))
            .unwrap_or(if r#type == ToastType::Error {
                AnnouncePriority::Assertive
            } else {
                AnnouncePriority::Polite
            });
        let id = options
            .id
            .or(previous.map(|previous| previous.id.clone()))
            .unwrap_or_else(|| format!("toast-{}", next(&IDS)).into());
        Toast {
            id,
            title: options
                .title
                .or(previous.and_then(|previous| previous.title.clone())),
            description: options
                .description
                .or(previous.and_then(|previous| previous.description.clone())),
            r#type,
            duration,
            priority,
            action: options
                .action
                .or(previous.and_then(|previous| previous.action.clone())),
            cancel: options
                .cancel
                .or(previous.and_then(|previous| previous.cancel.clone())),
            dismissible: options
                .dismissible
                .unwrap_or(previous.is_none_or(|previous| previous.dismissible)),
            data: options
                .data
                .or(previous.and_then(|previous| previous.data.clone())),
            on_dismiss: options
                .on_dismiss
                .or(previous.and_then(|previous| previous.on_dismiss.clone())),
            version: previous.map_or(0, |previous| previous.version) + 1,
            order: previous.map_or_else(|| next(&ORDERS), |previous| previous.order),
        }
    }
}

/// A queue of toasts and what adds to it (`createToaster`). Most apps use the
/// shared one, [`toast`]. Create another for a second region with its own
/// place and toasts, and pass it to that region as `toaster`. Clones are the
/// same toaster.
#[derive(Clone)]
pub struct Toaster {
    queue: Entity<ToasterQueue>,
}

impl PartialEq for Toaster {
    fn eq(&self, other: &Self) -> bool {
        self.queue == other.queue
    }
}

impl fmt::Debug for Toaster {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Toaster")
            .field(&self.queue.entity_id())
            .finish()
    }
}

/// Creates a queue of toasts (`createToaster`).
pub fn create_toaster(options: ToasterOptions, cx: &mut App) -> Toaster {
    let queue = cx.new(|_| ToasterQueue {
        toasts: Vec::new(),
        duration: options.duration,
        overlays: options.overlays.unwrap_or_else(overlays),
        placed_regions: 0,
        mounted: false,
    });
    Toaster { queue }
}

struct SharedToaster(Toaster);

impl Global for SharedToaster {}

/// The shared toaster: `toast(cx).show("Saved", cx)`,
/// `toast(cx).success(..)`, `toast(cx).promise(save, ..)`. The overlay host,
/// or a region you place, shows it. One per app.
pub fn toast(cx: &mut App) -> Toaster {
    if let Some(shared) = cx.try_global::<SharedToaster>() {
        return shared.0.clone();
    }
    let toaster = create_toaster(ToasterOptions::default(), cx);
    cx.set_global(SharedToaster(toaster.clone()));
    toaster
}

/// A region the app placed, counted by its toaster until released, so the
/// overlay host doesn't render a second one (`attachRegion`'s release).
#[must_use = "release it when the region leaves the page"]
pub struct RegionAttachment {
    queue: Option<Entity<ToasterQueue>>,
}

impl RegionAttachment {
    /// Stops counting the region.
    pub fn release(mut self, cx: &mut App) {
        if let Some(queue) = self.queue.take() {
            queue.update(cx, |queue, cx| {
                queue.placed_regions = queue.placed_regions.saturating_sub(1);
                cx.notify();
            });
        }
    }
}

impl Toaster {
    /// Shows a toast and returns its id: `show("Saved", cx)` or
    /// `show(ToastOptions::from("Deleted").action(undo), cx)`. A toast with
    /// the id of one still showing updates it instead (the toaster's call,
    /// `toast(message, options)`).
    pub fn show(&self, message: impl Into<ToastOptions>, cx: &mut App) -> SharedString {
        self.add(message.into(), cx)
    }

    /// Shows a success toast. Returns its id.
    pub fn success(&self, message: impl Into<ToastOptions>, cx: &mut App) -> SharedString {
        self.typed(ToastType::Success, message.into(), cx)
    }

    /// Shows an error toast, announced assertively. Returns its id.
    pub fn error(&self, message: impl Into<ToastOptions>, cx: &mut App) -> SharedString {
        self.typed(ToastType::Error, message.into(), cx)
    }

    /// Shows an informational toast. Returns its id.
    pub fn info(&self, message: impl Into<ToastOptions>, cx: &mut App) -> SharedString {
        self.typed(ToastType::Info, message.into(), cx)
    }

    /// Shows a warning toast. Returns its id.
    pub fn warning(&self, message: impl Into<ToastOptions>, cx: &mut App) -> SharedString {
        self.typed(ToastType::Warning, message.into(), cx)
    }

    /// Shows a loading toast, which stays until you update or dismiss it.
    /// Returns its id.
    pub fn loading(&self, message: impl Into<ToastOptions>, cx: &mut App) -> SharedString {
        self.typed(ToastType::Loading, message.into(), cx)
    }

    /// Shows a loading toast while `work` runs, then turns it into a success
    /// or an error toast when it settles. A toast dismissed while loading
    /// stays closed. Returns its id. The work runs as a task on the app's
    /// main thread, to its end even if no region shows the toast, as the
    /// original's promise does; options for the toast go in `loading`, where
    /// the original takes a third argument.
    pub fn promise<V: 'static, E: 'static>(
        &self,
        work: impl Future<Output = Result<V, E>> + 'static,
        messages: ToastPromise<V, E>,
        cx: &mut App,
    ) -> SharedString {
        let ToastPromise {
            loading,
            success,
            error,
        } = messages;
        let id = self.add(loading.r#type(ToastType::Loading), cx);
        let toaster = self.clone();
        let settling = id.clone();
        cx.spawn(async move |cx| {
            let result = work.await;
            cx.update(|cx| match &result {
                Ok(value) => {
                    toaster.settle(&settling, ToastType::Success, success.as_ref(), value, cx)
                }
                Err(failure) => {
                    toaster.settle(&settling, ToastType::Error, error.as_ref(), failure, cx)
                }
            });
        })
        .detach();
        id
    }

    fn settle<V>(
        &self,
        id: &SharedString,
        r#type: ToastType,
        message: Option<&PromiseMessage<V>>,
        value: &V,
        cx: &mut App,
    ) {
        let Some(message) = message else {
            self.close(id, Reason::Imperative, None, cx);
            return;
        };
        self.update(id, message.resolve(value).r#type(r#type), cx);
    }

    fn typed(&self, r#type: ToastType, options: ToastOptions, cx: &mut App) -> SharedString {
        self.add(options.r#type(r#type), cx)
    }

    fn add(&self, options: ToastOptions, cx: &mut App) -> SharedString {
        let existing = options
            .id
            .as_ref()
            .and_then(|id| self.queue.read(cx).find(id).map(|toast| toast.id.clone()));
        if let Some(existing) = existing {
            self.update(&existing, options, cx);
            return existing;
        }
        let (id, mount) = self.queue.update(cx, |queue, cx| {
            let toast = queue.complete(options, None);
            let id = toast.id.clone();
            queue.toasts.insert(0, toast);
            // The host renders this toaster's region from its first toast on,
            // unless the app placed one.
            let mount = !queue.mounted && queue.placed_regions == 0;
            queue.mounted |= mount;
            cx.notify();
            (id, mount.then(|| queue.overlays.clone()))
        });
        // Outside the queue's update, since the hosts hear it at once.
        if let Some(overlays) = mount {
            let placed = self.queue.downgrade();
            let rendered: RenderedFn = Rc::new(move |cx: &App| {
                placed
                    .upgrade()
                    .is_some_and(|queue| queue.read(cx).placed_regions > 0)
            });
            overlays.mount(TOASTER_OVERLAY, Rc::new(self.clone()), Some(rendered), cx);
        }
        id
    }

    /// Changes a toast that's still showing, and restarts its timer. Fields
    /// the update leaves out keep the toast's. Returns false when it already
    /// closed, so a late update never brings it back.
    pub fn update(&self, id: &str, options: impl Into<ToastOptions>, cx: &mut App) -> bool {
        let options = options.into();
        self.queue.update(cx, |queue, cx| {
            let Some(index) = queue
                .toasts
                .iter()
                .position(|toast| toast.id.as_ref() == id)
            else {
                return false;
            };
            let options = options.id(SharedString::from(id.to_string()));
            let next = queue.complete(options, Some(&queue.toasts[index]));
            queue.toasts[index] = next;
            cx.notify();
            true
        })
    }

    /// Closes one toast by id, or every toast with `None`.
    pub fn dismiss(&self, id: Option<&str>, cx: &mut App) {
        let ids: Vec<SharedString> = self
            .queue
            .read(cx)
            .toasts
            .iter()
            .filter(|toast| id.is_none_or(|id| toast.id.as_ref() == id))
            .map(|toast| toast.id.clone())
            .collect();
        for id in ids {
            self.close(&id, Reason::Imperative, None, cx);
        }
    }

    /// The toasts in the queue, newest first.
    pub fn toasts<'a>(&self, cx: &'a App) -> &'a [Toast] {
        &self.queue.read(cx).toasts
    }

    /// Calls `listener` whenever a toast is added, updated or closed, once
    /// the change's update is over. Dropping the subscription stops it.
    pub fn subscribe(
        &self,
        mut listener: impl FnMut(&mut App) + 'static,
        cx: &mut App,
    ) -> Subscription {
        cx.observe(&self.queue, move |_, cx| listener(cx))
    }

    /// Closes a toast for a reason, unless `on_dismiss` cancels. Returns
    /// whether it closed. `on_dismiss` runs before the queue changes, outside
    /// its update.
    pub fn close(
        &self,
        id: &str,
        reason: ToastDismissReason,
        event: Option<InputEvent>,
        cx: &mut App,
    ) -> bool {
        let Some((version, on_dismiss)) = self
            .queue
            .read(cx)
            .find(id)
            .map(|toast| (toast.version, toast.on_dismiss.clone()))
        else {
            return false;
        };
        let source = change_source(reason, event.as_ref());
        let details = create_details(reason, event, source);
        // What it changes in turn reports the same source.
        if let Some(on_dismiss) = on_dismiss {
            with_source(source, || on_dismiss(&details, cx));
        }
        if details.canceled() {
            return false;
        }
        self.queue.update(cx, |queue, cx| {
            queue
                .toasts
                .retain(|toast| !(toast.id.as_ref() == id && toast.version == version));
            cx.notify();
        });
        true
    }

    /// Counts a region the app placed, so the overlay host doesn't render a
    /// second one (`attachRegion`). A region the host renders isn't counted.
    pub fn attach_region(&self, placed: bool, cx: &mut App) -> RegionAttachment {
        if !placed {
            return RegionAttachment { queue: None };
        }
        self.queue.update(cx, |queue, cx| {
            queue.placed_regions += 1;
            cx.notify();
        });
        RegionAttachment {
            queue: Some(self.queue.clone()),
        }
    }

    /// Whether a region the app placed shows this toaster.
    pub fn has_placed_region(&self, cx: &App) -> bool {
        self.queue.read(cx).placed_regions > 0
    }

    /// Hears every change to the queue in a component, with its window, like
    /// a region following its toaster.
    pub fn observe_in<T: 'static>(
        &self,
        window: &mut Window,
        cx: &mut Context<T>,
        mut on_change: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        cx.observe_in(&self.queue, window, move |this, _, window, cx| {
            on_change(this, window, cx)
        })
    }
}
