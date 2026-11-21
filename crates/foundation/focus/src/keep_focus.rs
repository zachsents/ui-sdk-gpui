//! Port of `foundation/focus/keep-focus.ts`: focus that follows an element
//! a component moves or renders anew.
//!
//! In gpui focus is a handle, and stays with an element that renders
//! somewhere else with the same handle. An element that renders with a new
//! one, like an item that moved to another list, which keeps a handle per
//! item, leaves focus on a handle nothing renders any more.

use std::rc::Rc;

use gpui::{App, FocusHandle, Window};

// Focus on an element that's gone fell to the window. Focus on an ancestor
// of the element is where a press on it left focus, not where it belongs.
fn lost(active: &FocusHandle, el: &FocusHandle, window: &Window) -> bool {
    !active.is_rendered(window) || active.contains(el, window)
}

/// Focuses the element `element` returns, by its focus handle, if focus
/// fell to the window, or sits on an element that's gone or on an ancestor
/// of it, once the current update is over and again once the next frame is
/// drawn, when a component that renders it anew has. Call it after a
/// keyboard move. If the user moves focus elsewhere meanwhile, it leaves
/// that focus alone.
pub fn keep_focus(
    element: impl Fn(&Window, &App) -> Option<FocusHandle> + 'static,
    window: &Window,
    cx: &mut App,
) {
    let restore = Rc::new(move |window: &mut Window, cx: &mut App| {
        let Some(el) = element(window, cx) else {
            return;
        };
        if !el.is_rendered(window) {
            return;
        }
        let active = window.focused(cx);
        if active.as_ref() != Some(&el) && active.as_ref().is_none_or(|active| lost(active, &el, window)) {
            window.focus(&el, cx);
        }
    });
    let soon = restore.clone();
    window.defer(cx, move |window, cx| soon(window, cx));
    // A callback for the next frame runs before it's drawn, so the one it
    // asks for runs after.
    window.on_next_frame(move |window, _cx| {
        window.on_next_frame(move |window, cx| restore(window, cx));
    });
}
