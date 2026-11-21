//! Port of `foundation/kernel/action.ts`. Action handlers may return work
//! that goes on, which means the part that ran them is pending until it
//! settles. A Button, a dialog's Close part and a toast's buttons share this.
//!
//! The original reads a handler's return value: a Promise makes the part
//! pending (`isPromise`), a popup controller's `open()` result doesn't
//! (`popupResult`), and a canceled web component event returns a marker
//! (`canceledAction`). A Rust handler says which with its
//! [`ActionOutcome`]: an action that opens a popup returns `Done`.

use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use gpui::{App, Context, SharedString, Task, Window};

use super::details::{ChangeSource, with_source};
use super::dev;

/// What a pending part says by default, in English, once it has been pending
/// for a second. A quick action that settles sooner says nothing, so screen
/// readers hear only waits worth mentioning.
pub const DEFAULT_PENDING_ANNOUNCEMENT: &str = "Loading";

/// How long a part is pending before it says so.
pub const PENDING_ANNOUNCE_DELAY: Duration = Duration::from_millis(1000);

/// What an action receives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionDetails {
    /// How it was pressed, like `Mouse`, `Keyboard` or `Agent`. When an agent
    /// pressed it, a change the action makes after it waits comes once the
    /// press is over and reports `Programmatic`; wrap it in
    /// `with_source(details.source, ..)` and it reports `Agent`.
    pub source: ChangeSource,
}

/// Why an action failed, its message, which the part keeps in its `error`
/// state.
pub type ActionError = SharedString;

/// What an action handler returns.
pub enum ActionOutcome {
    /// It's done, or it opened something, which isn't a wait.
    Done,
    /// It goes on: the part is pending until the work settles.
    Pending(Pin<Box<dyn Future<Output = Result<(), ActionError>>>>),
    /// A listener canceled it: nothing more happens, so the dialog a Close
    /// part closes stays open.
    Canceled,
    /// It failed at once.
    Failed(ActionError),
}

impl ActionOutcome {
    /// Work that goes on, like a save, from any future whose error displays.
    pub fn wait<E: Display + 'static>(work: impl Future<Output = Result<(), E>> + 'static) -> Self {
        ActionOutcome::Pending(Box::pin(async move {
            work.await.map_err(|error| SharedString::from(error.to_string()))
        }))
    }

    /// A failure, from anything that displays.
    pub fn failed(error: impl Display) -> Self {
        ActionOutcome::Failed(error.to_string().into())
    }
}

/// An action handler, like a Button's `action` option. It runs inside the
/// component's update, so it may update any entity but the component.
pub type ActionHandler = std::rc::Rc<dyn Fn(&ActionDetails, &mut Window, &mut App) -> ActionOutcome>;

/// The part going pending, or settling.
pub type PendingHandler<T> = Box<dyn Fn(&mut T, bool, &mut Window, &mut Context<T>)>;

/// The action succeeding.
pub type DoneHandler<T> = Box<dyn Fn(&mut T, &mut Window, &mut Context<T>)>;

/// The action failing, with why.
pub type FailedHandler<T> = Box<dyn Fn(&mut T, ActionError, &mut Window, &mut Context<T>)>;

/// What the part does as its action runs.
pub struct ActionHandlers<T> {
    /// The action went pending, or settled.
    pub pending: PendingHandler<T>,
    /// It succeeded. Runs with the press's source, as part of the press, so
    /// what it changes after the wait, like closing a dialog, reports it too.
    pub done: DoneHandler<T>,
    /// It failed or its work did, with the reason.
    pub failed: FailedHandler<T>,
}

/// Runs an action handler with the source of the press that ran it, read
/// during the press (`runAction`). Work that goes on calls `pending(true)`
/// and, once it settles, `pending(false)`. Then `done` runs on success, or
/// `failed` with the reason. The failure is handled here: it belongs in the
/// part's `error` state. Development builds still log it, so an app that
/// doesn't show that state doesn't lose the error. Work that goes on runs to
/// its end even if the component leaves, as the original's promise does.
pub fn run_action<T: 'static>(
    this: &mut T,
    action: &dyn Fn(&ActionDetails, &mut Window, &mut App) -> ActionOutcome,
    source: ChangeSource,
    handlers: ActionHandlers<T>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let failed = |this: &mut T, error: ActionError, window: &mut Window, cx: &mut Context<T>, handlers: &ActionHandlers<T>| {
        dev::note(
            "an action failed. The error is in its part's `error` state.",
            None,
            Some(&error),
        );
        (handlers.failed)(this, error, window, cx);
    };
    match action(&ActionDetails { source }, window, cx) {
        ActionOutcome::Done => with_source(source, || (handlers.done)(this, window, cx)),
        ActionOutcome::Canceled => {}
        ActionOutcome::Failed(error) => failed(this, error, window, cx, &handlers),
        ActionOutcome::Pending(work) => {
            (handlers.pending)(this, true, window, cx);
            cx.spawn_in(window, async move |entity, cx| {
                let result = work.await;
                entity
                    .update_in(cx, |this, window, cx| {
                        (handlers.pending)(this, false, window, cx);
                        match result {
                            Ok(()) => with_source(source, || (handlers.done)(this, window, cx)),
                            Err(error) => failed(this, error, window, cx, &handlers),
                        }
                    })
                    .ok();
            })
            .detach();
        }
    }
}

/// Says the part's pending announcement politely, through the shared
/// announcer, once it has been pending for a second, since screen readers
/// don't read a change to `busy` (`announcePending`). Settling first, or the
/// part leaving, cancels it. Nothing is said on settle: what the action did
/// says that. The announcer lives in `ui-sdk-platform`, which a component
/// passes in.
#[derive(Default)]
pub struct AnnouncePending {
    was_pending: bool,
    timer: Option<Task<()>>,
}

impl AnnouncePending {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hears whether the part is pending now. `text` is its
    /// `pending_announcement`, `None` for the default.
    pub fn update<T: 'static>(
        &mut self,
        pending: bool,
        text: Option<SharedString>,
        announce: impl FnOnce(&str, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if pending == self.was_pending {
            return;
        }
        self.was_pending = pending;
        self.timer = None;
        if !pending {
            return;
        }
        let text = text.unwrap_or_else(|| DEFAULT_PENDING_ANNOUNCEMENT.into());
        self.timer = Some(cx.spawn_in(window, async move |_, cx| {
            cx.background_executor().timer(PENDING_ANNOUNCE_DELAY).await;
            cx.update(|window, cx| announce(&text, window, cx)).ok();
        }));
    }

    /// Whether it's waiting to announce.
    pub fn is_waiting(&self) -> bool {
        self.timer.is_some()
    }
}
