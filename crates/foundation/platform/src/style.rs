//! Port of `foundation/platform/style.ts`'s timing helpers: work that waits
//! for the frame an interaction paints, or for the app to be idle.
//!
//! The rest of the original writes CSS: style properties put back on
//! release, stylesheets, the rule that keeps hidden parts hidden, and
//! properties that don't inherit. gpui styles come from render, so a part
//! that changes another element's look exposes state its render reads, and a
//! hidden part isn't rendered.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{App, Window};

/// Cancels work [`after_paint`] scheduled. Dropping it lets the work run.
#[derive(Clone, Debug, Default)]
pub struct AfterPaint {
    canceled: Rc<Cell<bool>>,
}

impl AfterPaint {
    pub fn cancel(&self) {
        self.canceled.set(true);
    }

    pub fn is_canceled(&self) -> bool {
        self.canceled.get()
    }
}

/// Runs `work` once the window's next frame has painted, so what the
/// interaction shows comes first. Returns a cancel.
///
/// The next frame's callback runs before that frame draws, like
/// `requestAnimationFrame`, and the work then waits for the task after it, as
/// the original's `setTimeout` does.
pub fn after_paint(
    window: &Window,
    work: impl FnOnce(&mut Window, &mut App) + 'static,
) -> AfterPaint {
    let handle = AfterPaint::default();
    let canceled = handle.canceled.clone();
    let window_handle = window.window_handle();
    window.on_next_frame(move |_, cx| {
        if canceled.get() {
            return;
        }
        cx.spawn(async move |cx| {
            if canceled.get() {
                return;
            }
            window_handle
                .update(cx, |_, window, cx| work(window, cx))
                .ok();
        })
        .detach();
    });
    handle
}

type Job = Box<dyn FnOnce(&mut App)>;

thread_local! {
    static QUEUED: RefCell<Option<Vec<Job>>> = const { RefCell::new(None) };
}

/// Runs work ahead of the interaction that needs it, once the app is idle,
/// with the other work queued by then. The original runs it at once before
/// the page's first paint, when nothing is styled yet; gpui restyles nothing
/// either way, so it always waits for idle.
pub fn ahead(work: impl FnOnce(&mut App) + 'static, cx: &mut App) {
    let first = QUEUED.with(|queued| {
        let mut queued = queued.borrow_mut();
        match queued.as_mut() {
            Some(jobs) => {
                jobs.push(Box::new(work));
                false
            }
            None => {
                *queued = Some(vec![Box::new(work)]);
                true
            }
        }
    });
    if !first {
        return;
    }
    cx.spawn(async move |cx| {
        let jobs = QUEUED.with(|queued| queued.borrow_mut().take()).unwrap_or_default();
        cx.update(|cx| {
            for job in jobs {
                job(cx);
            }
        });
    })
    .detach();
}
