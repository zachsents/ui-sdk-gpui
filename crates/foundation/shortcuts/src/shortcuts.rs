//! Port of `foundation/shortcuts/shortcuts.ts`: keyboard shortcuts for a
//! whole app, scoped to the layers that are open. A modal dialog pauses the
//! window's shortcuts and keeps its own. Components with a `shortcut` prop
//! register here, and apps register their own the same way.

use gpui::{App, KeyDownEvent, Window};

use crate::create_shortcuts::create_shortcuts;
pub use crate::create_shortcuts::{
    RegisteredShortcut, ShortcutHandler, ShortcutInfo, ShortcutKeys, ShortcutOptions, Shortcuts,
};

thread_local! {
    static SHARED: Shortcuts = create_shortcuts();
}

/// Registers a shortcut in the shared [`shortcuts()`], like
/// `register_shortcut("mod+k", open, ShortcutOptions::default(), cx)`.
/// Returns a guard that unregisters it.
pub fn register_shortcut(
    keys: impl Into<ShortcutKeys>,
    handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    options: ShortcutOptions,
    cx: &mut App,
) -> RegisteredShortcut {
    shortcuts().register(keys, handler, options, cx)
}

/// The shared shortcuts every component registers into. One per app. It
/// listens only once something registers.
pub fn shortcuts() -> Shortcuts {
    SHARED.with(Shortcuts::clone)
}
