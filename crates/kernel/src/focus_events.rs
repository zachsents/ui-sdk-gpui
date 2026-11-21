//! Focus listeners whose changes repaint.
//!
//! gpui calls focus listeners (`cx.on_focus`, `on_focus_in`, `on_blur`,
//! `on_focus_out`, `on_focus_lost`) at the end of its draw, where
//! `cx.notify()` marks a view dirty without asking for a frame. What such a
//! listener changes shows only once something else redraws, like a caret's
//! next blink half a second later. These subscribe the same way but run the
//! listener right after the draw, where its changes repaint at once. Every
//! component subscribes to focus through them.
//!
//! gpui keeps the focus a frame moved from to itself, so the kernel notes
//! what holds focus in each window once a frame that drew parts is done.
//! That's where focus was ([`FocusChange::previous`], the DOM's
//! `relatedTarget`), and a change in it is what [`on_focus_change`], the
//! document's `focusin` and `focusout`, hears.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, Context, FocusHandle, FocusOutEvent, Global, Subscription, WeakFocusHandle, Window, WindowId,
};

/// Wraps `listener` so each call runs after the draw that reported it.
fn after_draw<T: 'static>(
    listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
) -> impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static {
    let listener = Rc::new(RefCell::new(listener));
    move |_, window, cx| {
        let listener = listener.clone();
        cx.defer_in(window, move |this, window, cx| {
            (listener.borrow_mut())(this, window, cx)
        });
    }
}

/// `handle` took focus itself (`cx.on_focus`).
pub fn on_focus<T: 'static>(
    handle: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<T>,
    listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
) -> Subscription {
    cx.on_focus(handle, window, after_draw(listener))
}

/// Focus moved into `handle` or something inside it (`cx.on_focus_in`).
pub fn on_focus_in<T: 'static>(
    handle: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<T>,
    listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
) -> Subscription {
    cx.on_focus_in(handle, window, after_draw(listener))
}

/// `on_focus_in` with where focus came from (`focusin` and its
/// `relatedTarget`): `previous` held it in the frame before, `current` holds
/// it now.
pub fn on_focus_in_from<T: 'static>(
    handle: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<T>,
    listener: impl FnMut(&mut T, &FocusChange, &mut Window, &mut Context<T>) + 'static,
) -> Subscription {
    let listener = Rc::new(RefCell::new(listener));
    cx.on_focus_in(handle, window, move |_, window, cx| {
        // gpui reports it before the kernel notes what holds focus now.
        let change = FocusChange {
            previous: noted(window, cx),
            current: holder(window, cx),
        };
        let listener = listener.clone();
        cx.defer_in(window, move |this, window, cx| {
            (listener.borrow_mut())(this, &change, window, cx)
        });
    })
}

/// `handle` itself lost focus (`cx.on_blur`).
pub fn on_blur<T: 'static>(
    handle: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<T>,
    listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
) -> Subscription {
    cx.on_blur(handle, window, after_draw(listener))
}

/// Focus left `handle` and everything inside it (`cx.on_focus_out`).
pub fn on_focus_out<T: 'static>(
    handle: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<T>,
    listener: impl FnMut(&mut T, FocusOutEvent, &mut Window, &mut Context<T>) + 'static,
) -> Subscription {
    let listener = Rc::new(RefCell::new(listener));
    cx.on_focus_out(handle, window, move |_, event, window, cx| {
        let listener = listener.clone();
        cx.defer_in(window, move |this, window, cx| {
            (listener.borrow_mut())(this, event, window, cx)
        });
    })
}

/// Nothing in the window holds focus any more (`cx.on_focus_lost`).
pub fn on_focus_lost<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    listener: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
) -> Subscription {
    cx.on_focus_lost(window, after_draw(listener))
}

/// Where focus moved: what held it in the frame before, and what holds it
/// now. `None` is nothing, as in a window that isn't active, or an element
/// that's gone.
#[derive(Clone, Debug, Default)]
pub struct FocusChange {
    pub previous: Option<FocusHandle>,
    pub current: Option<FocusHandle>,
}

/// Focus moved anywhere in the window, from one element to another, or
/// from or to nothing (`focusin` and `focusout` on the document). The
/// window going inactive moves it to nothing and coming back moves it back,
/// as gpui's focus events have it. It runs once the frame that showed the
/// move is done.
pub fn on_focus_change<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    mut listener: impl FnMut(&mut T, &FocusChange, &mut Window, &mut Context<T>) + 'static,
) -> Subscription {
    let entity = cx.weak_entity();
    let listener: Listener = Rc::new(RefCell::new(
        move |change: &FocusChange, window: &mut Window, cx: &mut App| {
            entity
                .update(cx, |this, cx| listener(this, change, window, cx))
                .is_ok()
        },
    ));
    let track = track(window, cx);
    let id = {
        let mut track = track.borrow_mut();
        track.next += 1;
        let id = track.next;
        track.listeners.push((id, listener));
        id
    };
    let track = Rc::downgrade(&track);
    Subscription::new(move || {
        if let Some(track) = track.upgrade() {
            track.borrow_mut().listeners.retain(|(other, _)| *other != id);
        }
    })
}

type Listener = Rc<RefCell<dyn FnMut(&FocusChange, &mut Window, &mut App) -> bool>>;

// What held focus in a window as of its last frame the kernel saw, and who
// hears it move.
#[derive(Default)]
struct Track {
    focused: Option<WeakFocusHandle>,
    next: u64,
    listeners: Vec<(u64, Listener)>,
}

#[derive(Default)]
struct Tracks(HashMap<WindowId, Rc<RefCell<Track>>>);

impl Global for Tracks {}

fn track(window: &Window, cx: &mut App) -> Rc<RefCell<Track>> {
    let id = window.window_handle().window_id();
    if let Some(track) = cx.try_global::<Tracks>().and_then(|tracks| tracks.0.get(&id)) {
        return track.clone();
    }
    let track = Rc::new(RefCell::new(Track {
        focused: holder(window, cx).map(|focused| focused.downgrade()),
        ..Track::default()
    }));
    cx.default_global::<Tracks>().0.insert(id, track.clone());
    track
}

// What holds focus in `window` as its last frame drew it: nothing while the
// window isn't active, or while the focused element isn't in the frame, as
// gpui's focus events see it.
fn holder(window: &Window, cx: &App) -> Option<FocusHandle> {
    if !window.is_window_active() {
        return None;
    }
    window.focused(cx).filter(|focused| focused.is_rendered(window))
}

// What the kernel last noted holding focus in `window`.
fn noted(window: &Window, cx: &App) -> Option<FocusHandle> {
    let tracks = cx.try_global::<Tracks>()?;
    let track = tracks.0.get(&window.window_handle().window_id())?.borrow();
    track.focused.as_ref().and_then(WeakFocusHandle::upgrade)
}

/// Notes what holds focus once a frame that drew parts is done, and tells
/// the window's focus-change listeners when it moved.
pub(crate) fn frame_drawn(window: &mut Window, cx: &mut App) {
    let current = holder(window, cx);
    let track = track(window, cx);
    let (previous, listeners) = {
        let mut track = track.borrow_mut();
        let moved = match (&track.focused, &current) {
            (Some(was), Some(now)) => was != now,
            (None, None) => false,
            _ => true,
        };
        if !moved {
            return;
        }
        let previous = std::mem::replace(&mut track.focused, current.as_ref().map(FocusHandle::downgrade));
        let listeners: Vec<(u64, Listener)> = track.listeners.clone();
        (previous, listeners)
    };
    let change = FocusChange {
        previous: previous.as_ref().and_then(WeakFocusHandle::upgrade),
        current,
    };
    // Focus on an element that's gone, then on nothing, moved nowhere.
    if change.previous.is_none() && change.current.is_none() {
        return;
    }
    let gone: Vec<u64> = listeners
        .into_iter()
        .filter_map(|(id, listener)| (!(listener.borrow_mut())(&change, window, cx)).then_some(id))
        .collect();
    if !gone.is_empty() {
        track.borrow_mut().listeners.retain(|(id, _)| !gone.contains(id));
    }
}
