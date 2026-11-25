//! Port of `components/toast/toast-timers.ts`: when toasts close on their
//! own. One countdown per shown toast, and one set of pause reasons per
//! region: each reason is cleared on its own, so the window regaining focus
//! never resumes a toast the pointer still rests on. Paused time is kept,
//! never reset.
//!
//! Pure apart from its timers, which are gpui tasks the countdowns hold:
//! dropping one cancels it, the original's `clearTimeout`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{Context, SharedString, Task};

/// Why a region's timers are paused: the pointer on it, focus in it, the
/// window or the page going away, a swipe, or a reason of your own.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ToastPauseReason {
    ItemHover,
    FocusIn,
    WindowBlur,
    PageHide,
    Swipe,
    /// A reason of your own, by name.
    Custom(SharedString),
}

impl ToastPauseReason {
    /// The original's name for it, like `"item-hover"`.
    pub fn as_str(&self) -> &str {
        match self {
            ToastPauseReason::ItemHover => "item-hover",
            ToastPauseReason::FocusIn => "focus-in",
            ToastPauseReason::WindowBlur => "window-blur",
            ToastPauseReason::PageHide => "page-hide",
            ToastPauseReason::Swipe => "swipe",
            ToastPauseReason::Custom(name) => name,
        }
    }

    /// The reason a name names: one of the original's, or one of your own.
    pub fn from_name(name: &str) -> Self {
        match name {
            "item-hover" => ToastPauseReason::ItemHover,
            "focus-in" => ToastPauseReason::FocusIn,
            "window-blur" => ToastPauseReason::WindowBlur,
            "page-hide" => ToastPauseReason::PageHide,
            "swipe" => ToastPauseReason::Swipe,
            name => ToastPauseReason::Custom(SharedString::from(name.to_string())),
        }
    }
}

impl From<&str> for ToastPauseReason {
    fn from(name: &str) -> Self {
        Self::from_name(name)
    }
}

/// Whether a duration counts down: `Duration::MAX` stands for the original's
/// `Infinity`, which keeps a toast until it's dismissed.
pub(crate) fn is_finite(duration: Duration) -> bool {
    duration != Duration::MAX
}

struct Countdown {
    // Which update of the toast it counts for. An update restarts it.
    version: u64,
    duration: Duration,
    remaining: Duration,
    // When it last started running, or `None` while it waits.
    started_at: Option<Instant>,
    timer: Option<Task<()>>,
    // Tells this countdown from one that replaced it.
    serial: u64,
}

/// What a countdown needs to know about a toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimedToast {
    pub id: SharedString,
    pub version: u64,
    pub duration: Duration,
    /// Whether something holds it open, like a pending action. It waits until
    /// that ends.
    pub held: bool,
}

#[derive(Default)]
struct TimersState {
    countdowns: HashMap<SharedString, Countdown>,
    reasons: HashSet<ToastPauseReason>,
    shown: Vec<TimedToast>,
    serials: u64,
}

/// What a countdown that ran out calls, inside the owner's update, with the
/// toast's id. Returning false, like an `on_before_dismiss` that canceled the
/// close, starts a new countdown, so the toast tries again later.
pub type ExpireCallback<T> = Rc<dyn Fn(&mut T, &SharedString, &mut Context<T>) -> bool>;

/// A region's countdowns (`createToastTimers`). Cheap to clone: clones share
/// them. Calls take the context of the entity that owns them, whose update
/// `expire` runs in.
pub struct ToastTimers<T: 'static> {
    state: Rc<RefCell<TimersState>>,
    expire: ExpireCallback<T>,
}

impl<T: 'static> Clone for ToastTimers<T> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            expire: self.expire.clone(),
        }
    }
}

/// Countdowns that call `expire` for a toast whose time ran out
/// (`createToastTimers`).
pub fn create_toast_timers<T: 'static>(
    expire: impl Fn(&mut T, &SharedString, &mut Context<T>) -> bool + 'static,
) -> ToastTimers<T> {
    ToastTimers {
        state: Rc::new(RefCell::new(TimersState::default())),
        expire: Rc::new(expire),
    }
}

fn stop(countdown: &mut Countdown) {
    let Some(started_at) = countdown.started_at.take() else {
        return;
    };
    countdown.timer = None;
    countdown.remaining = countdown.remaining.saturating_sub(started_at.elapsed());
}

impl<T: 'static> ToastTimers<T> {
    /// The toasts on screen now, and the ids still in the queue. Countdowns
    /// for toasts that left the queue are dropped.
    pub fn update(
        &self,
        next: Vec<TimedToast>,
        queued: &HashSet<SharedString>,
        cx: &mut Context<T>,
    ) {
        {
            let mut state = self.state.borrow_mut();
            state.shown = next;
            state.countdowns.retain(|id, countdown| {
                if queued.contains(id) {
                    return true;
                }
                stop(countdown);
                false
            });
        }
        self.apply(cx);
    }

    /// Pauses every countdown for a reason, until that reason is cleared.
    pub fn pause(&self, reason: ToastPauseReason, cx: &mut Context<T>) {
        if !self.state.borrow_mut().reasons.insert(reason) {
            return;
        }
        self.apply(cx);
    }

    /// Clears one reason. Countdowns resume once no reason is left.
    pub fn resume(&self, reason: &ToastPauseReason, cx: &mut Context<T>) {
        if !self.state.borrow_mut().reasons.remove(reason) {
            return;
        }
        self.apply(cx);
    }

    /// Whether anything pauses the countdowns now.
    pub fn is_paused(&self) -> bool {
        !self.state.borrow().reasons.is_empty()
    }

    /// The time a toast has left, for devtools. `Duration::MAX` for one that
    /// stays until it's dismissed.
    pub fn remaining(&self, id: &str) -> Option<Duration> {
        let state = self.state.borrow();
        let countdown = state.countdowns.get(id)?;
        Some(match countdown.started_at {
            None => countdown.remaining,
            Some(started_at) => countdown.remaining.saturating_sub(started_at.elapsed()),
        })
    }

    /// Stops every countdown, for when the region leaves the page.
    pub fn destroy(&self) {
        let mut state = self.state.borrow_mut();
        for countdown in state.countdowns.values_mut() {
            stop(countdown);
        }
        state.countdowns.clear();
    }

    fn apply(&self, cx: &mut Context<T>) {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let ids: HashSet<&SharedString> = state.shown.iter().map(|toast| &toast.id).collect();
        // Toasts that are no longer shown, like ones pushed past
        // `max_visible`, wait with the time they have left.
        for (id, countdown) in state.countdowns.iter_mut() {
            if !ids.contains(id) {
                stop(countdown);
            }
        }
        let paused = !state.reasons.is_empty();
        for toast in &state.shown {
            let current = state
                .countdowns
                .get(&toast.id)
                .is_some_and(|countdown| countdown.version == toast.version);
            if !current {
                if let Some(old) = state.countdowns.get_mut(&toast.id) {
                    stop(old);
                }
                state.serials += 1;
                state.countdowns.insert(
                    toast.id.clone(),
                    Countdown {
                        version: toast.version,
                        duration: toast.duration,
                        remaining: toast.duration,
                        started_at: None,
                        timer: None,
                        serial: state.serials,
                    },
                );
            }
            let Some(countdown) = state.countdowns.get_mut(&toast.id) else {
                continue;
            };
            if paused || toast.held {
                stop(countdown);
            } else {
                self.start(&toast.id, countdown, cx);
            }
        }
    }

    fn start(&self, id: &SharedString, countdown: &mut Countdown, cx: &mut Context<T>) {
        if countdown.started_at.is_some() || !is_finite(countdown.remaining) {
            return;
        }
        countdown.started_at = Some(Instant::now());
        let delay = countdown.remaining;
        let serial = countdown.serial;
        let state = Rc::downgrade(&self.state);
        let expire = self.expire.clone();
        let id = id.clone();
        countdown.timer = Some(cx.spawn(async move |owner, cx| {
            cx.background_executor().timer(delay).await;
            owner
                .update(cx, |owner, cx| fire(state, expire, owner, &id, serial, cx))
                .ok();
        }));
    }
}

// A countdown ran out. An `on_before_dismiss` that cancels the close starts a
// new countdown, so the toast tries again later.
fn fire<T: 'static>(
    state: Weak<RefCell<TimersState>>,
    expire: ExpireCallback<T>,
    owner: &mut T,
    id: &SharedString,
    serial: u64,
    cx: &mut Context<T>,
) {
    let Some(state) = state.upgrade() else {
        return;
    };
    {
        let mut state = state.borrow_mut();
        let Some(countdown) = state
            .countdowns
            .get_mut(id)
            .filter(|countdown| countdown.serial == serial)
        else {
            return;
        };
        countdown.started_at = None;
        // This runs inside the countdown's own task, which ends with it.
        if let Some(task) = countdown.timer.take() {
            task.detach();
        }
        countdown.remaining = Duration::ZERO;
    }
    if expire(owner, id, cx) {
        return;
    }
    let timers = ToastTimers {
        state: state.clone(),
        expire,
    };
    {
        let mut state = state.borrow_mut();
        let Some(countdown) = state
            .countdowns
            .get_mut(id)
            .filter(|countdown| countdown.serial == serial)
        else {
            return;
        };
        countdown.remaining = countdown.duration;
    }
    timers.apply(cx);
}
