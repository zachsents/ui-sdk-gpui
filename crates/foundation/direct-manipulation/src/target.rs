//! What the original reads from the page around an event: the elements a
//! behavior binds as they render, each with where it last laid out, its
//! hitbox and the focus handle it tracks (the `Map`s of elements its `bind`
//! callbacks fill and their cleanups empty), and whether a part inside the
//! one hearing an event saw the event first, like a press on a resize handle
//! bubbling to its box (`event.target.closest(..)`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::rc::{Rc, Weak};

use gpui::{App, FocusHandle, KeyDownEvent, Keystroke, ParentElement, Styled, Window, WindowId};
use ui_sdk_drag_and_drop::PointerEvent;
use ui_sdk_platform::Region;

struct BoundInner<K> {
    // Whether a render of the elements is under way.
    open: bool,
    order: Vec<K>,
    regions: HashMap<K, Region>,
}

/// The elements a behavior binds as they render, by key, in render order,
/// which is page order. The first to bind in a frame starts a new render of
/// them, which ends once the frame is drawn, so an element that stops
/// rendering leaves, as the original's cleanup removes it.
pub(crate) struct Bound<K> {
    inner: Rc<RefCell<BoundInner<K>>>,
}

impl<K> Clone for Bound<K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<K: Clone + Eq + Hash + 'static> Bound<K> {
    pub(crate) fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(BoundInner {
                open: false,
                order: Vec::new(),
                regions: HashMap::new(),
            })),
        }
    }

    /// Starts a new render of the elements, unless one is under way.
    pub(crate) fn begin(&self, cx: &App) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.open {
                return;
            }
            inner.open = true;
            let BoundInner { order, regions, .. } = &mut *inner;
            let kept: HashSet<&K> = order.iter().collect();
            regions.retain(|key, _| kept.contains(key));
            order.clear();
        }
        let inner: Weak<RefCell<BoundInner<K>>> = Rc::downgrade(&self.inner);
        cx.spawn(async move |_cx| {
            if let Some(inner) = inner.upgrade() {
                inner.borrow_mut().open = false;
            }
        })
        .detach();
    }

    /// Joins an element as it renders, with the focus handle it tracks, and
    /// measures it. One bound twice in a render keeps its first place.
    pub(crate) fn bind<E: ParentElement + Styled>(
        &self,
        key: K,
        focus: Option<&FocusHandle>,
        el: E,
        cx: &App,
    ) -> E {
        self.begin(cx);
        let region = {
            let mut inner = self.inner.borrow_mut();
            if !inner.order.contains(&key) {
                inner.order.push(key.clone());
            }
            inner.regions.entry(key).or_default().clone()
        };
        region.set_focus_handle(focus.cloned());
        region.measure(el)
    }

    /// The element bound for `key` in the last render.
    pub(crate) fn region(&self, key: &K) -> Option<Region> {
        let inner = self.inner.borrow();
        if !inner.order.contains(key) {
            return None;
        }
        inner.regions.get(key).cloned()
    }

    /// The focus handle the element for `key` tracked when it last bound,
    /// so it keeps the same one from frame to frame.
    pub(crate) fn focus_of(&self, key: &K) -> Option<FocusHandle> {
        self.inner
            .borrow()
            .regions
            .get(key)
            .and_then(|region| region.focus_handle())
    }

    /// The keys bound in the last render, in render order.
    pub(crate) fn keys(&self) -> Vec<K> {
        self.inner.borrow().order.clone()
    }

    pub(crate) fn contains(&self, key: &K) -> bool {
        self.inner.borrow().order.contains(key)
    }
}

/// What a part inside the one hearing an event was, when it saw the event
/// first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    /// A resize handle, which its box leaves its events to.
    ResizeHandle,
    /// A span, which its track leaves its presses to.
    Span,
}

thread_local! {
    // The presses and keys parts saw during the event being dispatched now.
    static PRESSES: RefCell<Vec<(Mark, PointerEvent)>> = const { RefCell::new(Vec::new()) };
    static KEYS: RefCell<Vec<(Mark, WindowId, Keystroke, bool)>> = const { RefCell::new(Vec::new()) };
}

// Forgets the marks once the event being dispatched is over.
fn forget_after(window: &Window, cx: &mut App) {
    window.defer(cx, |_window, _cx| {
        PRESSES.with(|presses| presses.borrow_mut().clear());
        KEYS.with(|keys| keys.borrow_mut().clear());
    });
}

/// Notes that a part of `mark`'s kind saw the press `event`.
pub(crate) fn mark_press(mark: Mark, event: &PointerEvent, window: &Window, cx: &mut App) {
    PRESSES.with(|presses| presses.borrow_mut().push((mark, event.clone())));
    forget_after(window, cx);
}

/// Whether a part of `mark`'s kind saw the press `event` first.
pub(crate) fn marked_press(mark: Mark, event: &PointerEvent) -> bool {
    PRESSES.with(|presses| {
        presses
            .borrow()
            .iter()
            .any(|(kind, press)| *kind == mark && press.is_same(event))
    })
}

/// Notes that a part of `mark`'s kind saw the key `event`.
pub(crate) fn mark_key(mark: Mark, event: &KeyDownEvent, window: &Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    KEYS.with(|keys| {
        keys.borrow_mut()
            .push((mark, id, event.keystroke.clone(), event.is_held))
    });
    forget_after(window, cx);
}

/// Whether a part of `mark`'s kind saw the key `event` first.
pub(crate) fn marked_key(mark: Mark, event: &KeyDownEvent, window: &Window) -> bool {
    let id = window.window_handle().window_id();
    KEYS.with(|keys| {
        keys.borrow().iter().any(|(kind, window, keystroke, held)| {
            *kind == mark
                && *window == id
                && *keystroke == event.keystroke
                && *held == event.is_held
        })
    })
}
