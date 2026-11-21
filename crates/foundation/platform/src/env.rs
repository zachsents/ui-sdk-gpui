//! Port of `foundation/platform/env.ts`.
//!
//! The original reads an element's computed `direction`. gpui has no `dir`
//! attribute, so the direction is the app's: [`set_direction`] sets it, and
//! every behavior that mirrors arrow keys or sides reads [`is_rtl`].

use gpui::{App, Global};

/// Which way text and layout run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
}

struct AppDirection(Direction);

impl Global for AppDirection {}

/// Sets the direction everything lays out in, like `dir` on the page's root.
pub fn set_direction(direction: Direction, cx: &mut App) {
    cx.set_global(AppDirection(direction));
}

/// The direction everything lays out in, left to right unless set.
pub fn direction(cx: &App) -> Direction {
    cx.try_global::<AppDirection>()
        .map_or(Direction::Ltr, |direction| direction.0)
}

/// Whether layout runs right to left (`isRTL`).
pub fn is_rtl(cx: &App) -> bool {
    direction(cx) == Direction::Rtl
}

/// iPhone and iPad.
pub fn is_ios() -> bool {
    cfg!(target_os = "ios")
}
