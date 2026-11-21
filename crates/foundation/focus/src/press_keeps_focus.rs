//! Port of `foundation/focus/press-keeps-focus.ts`: presses that leave focus
//! where it is, like on a list while typing goes on in its field, or on a
//! token's remove button while the caret stays in the editor.
//!
//! gpui focuses an element that tracks a focus handle when it's pressed,
//! unless a mouse down handler that ran before called
//! `window.prevent_default()`. Handlers run from the innermost element out,
//! so a field inside the part, like a search field in a list, has focused
//! itself before the part's handler runs, as the original lets it; a field
//! around the part, like the editor a token's remove button sits in, would
//! run after, and doesn't get to.

use gpui::{App, InteractiveElement, MouseDownEvent, Window};

/// Cancels a press so it takes no focus. Hand it to a part's
/// `on_any_mouse_down`.
pub fn hold_focus(_event: &MouseDownEvent, window: &mut Window, _cx: &mut App) {
    window.prevent_default();
}

/// Makes presses on `el`, with any button, leave focus where it is
/// (`pressKeepsFocus`).
pub fn press_keeps_focus<E: InteractiveElement>(el: E) -> E {
    el.on_any_mouse_down(hold_focus)
}
