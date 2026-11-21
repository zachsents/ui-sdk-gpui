//! Port of `foundation/focus/tabbable.ts`: what Tab can reach, for focus
//! traps in dialogs and popups and for focus a popup hands back.
//!
//! The original queries the DOM for the elements Tab can stop on. gpui has
//! no DOM to query. Its tab order is the tab stops the last frame painted,
//! each a focus handle an element tracks with `tab_stop(true)`, ordered by
//! `tab_index` within each `tab_group`, and only `Window::focus_next` and
//! `focus_prev` read it. So this reads it by stepping focus through the
//! window's tab stops and putting it back where it was. gpui tells where
//! focus is when it draws, so nothing renders or hears the steps; like any
//! focus change, they restart a key binding of several keys in progress. A
//! container is the focus handle its element tracks, and what's inside it is
//! what was inside that element in the last frame.
//!
//! Browsers move focus with Tab themselves. A gpui app binds it once, with
//! [`bind_tab_keys`].

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gpui::{App, FocusHandle, Global, InteractiveElement, Window};
use ui_sdk_kernel::Key;

// More than any window holds. The walk stops at the first stop it meets
// again.
const MOST: usize = 100_000;

/// Whether Tab can stop on the element `handle` tracks: it rendered in the
/// last frame, as a tab stop.
pub fn is_tabbable(handle: &FocusHandle, window: &Window) -> bool {
    // A copy of the handle is as old as the copy; a new one reads its last
    // `tab_stop`.
    handle.is_rendered(window) && handle.clone().tab_stop
}

// The window's tab stops, in the order Tab visits them, as of the last
// frame, with focus put back where it was.
fn walk(window: &mut Window, cx: &mut App) -> Vec<FocusHandle> {
    let original = window.focused(cx);
    window.blur(cx);
    let mut order: Vec<FocusHandle> = Vec::new();
    while order.len() < MOST {
        window.focus_next(cx);
        let Some(handle) = window.focused(cx) else {
            break;
        };
        if order.first() == Some(&handle) || order.last() == Some(&handle) {
            break;
        }
        order.push(handle);
    }
    restore(original, window, cx);
    order
}

fn restore(original: Option<FocusHandle>, window: &mut Window, cx: &mut App) {
    match original {
        Some(handle) => window.focus(&handle, cx),
        None => window.blur(cx),
    }
}

// Where Tab goes from `handle`, with focus put back.
fn step_from(handle: &FocusHandle, backward: bool, window: &mut Window, cx: &mut App) -> Option<FocusHandle> {
    let original = window.focused(cx);
    window.focus(handle, cx);
    if backward {
        window.focus_prev(cx);
    } else {
        window.focus_next(cx);
    }
    let next = window.focused(cx).filter(|next| next != handle);
    restore(original, window, cx);
    next
}

// A bound tab stop holding stops of its own isn't one: Tab passes over it.
fn holds_stops(handle: &FocusHandle, order: &[FocusHandle], window: &Window) -> bool {
    is_bound(handle)
        && order
            .iter()
            .any(|other| other != handle && handle.contains(other, window))
}

fn tab_order(window: &mut Window, cx: &mut App) -> Vec<FocusHandle> {
    let order = walk(window, cx);
    order
        .iter()
        .filter(|handle| !holds_stops(handle, &order, window))
        .cloned()
        .collect()
}

fn inside(root: &FocusHandle, handle: &FocusHandle, window: &Window) -> bool {
    handle != root && root.contains(handle, window)
}

/// The tab stops inside the element `root` tracks, in the order Tab visits
/// them. The original lists them in page order; gpui keeps no order but
/// Tab's, which only differs from it where `tab_index` does.
pub fn tabbables(root: &FocusHandle, window: &mut Window, cx: &mut App) -> Vec<FocusHandle> {
    tab_order(window, cx)
        .into_iter()
        .filter(|handle| inside(root, handle, window))
        .collect()
}

/// The tab stops inside `root` in the order Tab visits them. The original
/// follows open shadow roots and slots into, which gpui has none of, so it's
/// [`tabbables`].
pub fn composed_tabbables(root: &FocusHandle, window: &mut Window, cx: &mut App) -> Vec<FocusHandle> {
    tabbables(root, window, cx)
}

/// The first tab stop after the element `node` tracks, in the order Tab
/// visits them, skipping anything inside `skip`, like the popup that is
/// handing focus back, and anything around `node`. After an element that
/// isn't a tab stop itself is after where Tab goes from it. `None` when
/// nothing is, or `node` didn't render.
pub fn next_tabbable(
    node: &FocusHandle,
    skip: Option<&FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) -> Option<FocusHandle> {
    if !node.is_rendered(window) {
        return None;
    }
    let order = walk(window, cx);
    let start = match order.iter().position(|handle| handle == node) {
        Some(index) => index + 1,
        None => {
            let next = step_from(node, false, window, cx)?;
            order.iter().position(|handle| *handle == next)?
        }
    };
    order[start..]
        .iter()
        .find(|handle| {
            !holds_stops(handle, &order, window)
                && !handle.contains(node, window)
                && !skip.is_some_and(|skip| skip.contains(handle, window))
        })
        .cloned()
}

/// Moves focus to the next tab stop inside `root`, or the previous one, in
/// the order Tab visits them, wrapping at the ends when `wrap`, as a focus
/// trap does: a trap that handles Tab with it never depends on where the
/// window's order goes after its last stop. From `root` itself, or from
/// outside it, focus goes to the first stop, or the last going back. Returns
/// the stop it focused, or `None` when there's none inside, or at an end
/// without `wrap`.
pub fn tab_within(
    root: &FocusHandle,
    backward: bool,
    wrap: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<FocusHandle> {
    let order = walk(window, cx);
    let stops: Vec<FocusHandle> = order
        .iter()
        .filter(|handle| inside(root, handle, window) && !holds_stops(handle, &order, window))
        .cloned()
        .collect();
    let last = stops.len().checked_sub(1)?;
    let active = window.focused(cx).filter(|active| inside(root, active, window));
    let next = match active {
        None => Some(if backward { last } else { 0 }),
        Some(active) => match stops.iter().position(|handle| *handle == active) {
            Some(index) if backward => index.checked_sub(1).or(wrap.then_some(last)),
            Some(index) if index < last => Some(index + 1),
            Some(_) => wrap.then_some(0),
            // An element inside that isn't a stop sits before the stop Tab
            // goes to from it.
            None => {
                let after = step_from(&active, false, window, cx)
                    .and_then(|next| stops.iter().position(|handle| *handle == next))
                    .unwrap_or(stops.len());
                if backward {
                    after.checked_sub(1).or(wrap.then_some(last))
                } else if after <= last {
                    Some(after)
                } else {
                    wrap.then_some(0)
                }
            }
        },
    }?;
    let handle = stops[next].clone();
    window.focus(&handle, cx);
    Some(handle)
}

struct Bound {
    handle: FocusHandle,
}

thread_local! {
    static BOUND: RefCell<Vec<Weak<Bound>>> = const { RefCell::new(Vec::new()) };
}

fn is_bound(handle: &FocusHandle) -> bool {
    BOUND.with(|bound| {
        let mut bound = bound.borrow_mut();
        bound.retain(|stop| stop.strong_count() > 0);
        bound
            .iter()
            .any(|stop| stop.upgrade().is_some_and(|stop| &stop.handle == handle))
    })
}

/// A container that is a tab stop while it's wanted and holds nothing Tab
/// could stop on, so keyboard users can still reach and scroll content
/// without controls, as axe's scrollable-region-focusable asks. What's
/// inside only shows in the tab order, which gpui reads as Tab moves, so the
/// container is a tab stop whenever it's wanted, and Tab, bound with
/// [`bind_tab_keys`], passes over it while it holds one; [`tabbables`] and
/// the rest leave it out then too. Dropping every copy unbinds it.
#[derive(Clone)]
pub struct TabStop {
    inner: Rc<Bound>,
}

/// Makes the element that tracks `handle` a tab stop while `wanted` holds
/// and nothing inside it can take focus (`bindTabStop`). Track it on the
/// element with [`TabStop::bind`].
pub fn bind_tab_stop(handle: FocusHandle) -> TabStop {
    let inner = Rc::new(Bound { handle });
    BOUND.with(|bound| bound.borrow_mut().push(Rc::downgrade(&inner)));
    TabStop { inner }
}

impl TabStop {
    /// The container's focus handle.
    pub fn handle(&self) -> &FocusHandle {
        &self.inner.handle
    }

    /// Tracks the container's focus handle on `el`, a tab stop while
    /// `wanted`, like a scroll area while it can scroll. The element is
    /// focusable either way, as gpui needs to see what's inside it.
    pub fn bind<E: InteractiveElement>(&self, el: E, wanted: bool) -> E {
        el.track_focus(&self.inner.handle.clone().tab_stop(wanted))
    }
}

struct TabKeys;

impl Global for TabKeys {}

/// Binds Tab and Shift-Tab to move focus to the next and previous tab stop,
/// what a browser does natively. Call it once as the app starts; calling it
/// again does nothing. Handlers hear the key first: one that uses it, like
/// a focus trap, calls `cx.stop_propagation()` or `window.prevent_default()`
/// and focus stays, as `preventDefault` keeps it in a browser, and a key
/// binding whose action handles Tab wins too. Tab with Control, Alt or ⌘ is
/// left alone. It passes over a [`TabStop`] that holds stops of its own.
pub fn bind_tab_keys(cx: &mut App) {
    if cx.has_global::<TabKeys>() {
        return;
    }
    cx.set_global(TabKeys);
    cx.observe_keystrokes(|event, window, cx| {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if Key::of(keystroke) != Key::Tab
            || event.action.is_some()
            || window.default_prevented()
            || modifiers.control
            || modifiers.alt
            || modifiers.platform
            || modifiers.function
        {
            return;
        }
        tab(modifiers.shift, window, cx);
    })
    .detach();
}

fn tab(backward: bool, window: &mut Window, cx: &mut App) {
    let start = window.focused(cx);
    let mut order: Option<Vec<FocusHandle>> = None;
    for _ in 0..MOST {
        if backward {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        let Some(now) = window.focused(cx) else {
            return;
        };
        if start.as_ref() == Some(&now) || !is_bound(&now) {
            return;
        }
        let order = order.get_or_insert_with(|| walk(window, cx));
        if !holds_stops(&now, order, window) {
            return;
        }
    }
}
