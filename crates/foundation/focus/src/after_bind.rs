//! What `foundation/kernel/after-bind.ts` gives the behaviors here, which the
//! kernel crate doesn't have: work that waits until a slot's parts have
//! rendered.
//!
//! The original runs it a microtask after the parts bind. Here parts join as
//! they render, which happens while gpui draws a frame; the first to join
//! starts a task, which gpui runs once the frame is drawn, with the window
//! the component lives in.

use gpui::{AnyWindowHandle, App, Window};

/// Runs `run` once the frame being drawn is done, with `window`.
pub(crate) fn after_bind(
    window: AnyWindowHandle,
    cx: &App,
    run: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    cx.spawn(async move |cx| {
        window.update(cx, |_, window, cx| run(window, cx)).ok();
    })
    .detach();
}
