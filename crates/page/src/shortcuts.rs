//! Port of `page/shortcuts/shortcuts.ts`: the public shortcuts entry. The
//! manager lives in `ui-sdk-shortcuts`, so components can register their
//! triggers' shortcuts without the page crate.

pub use ui_sdk_platform::{aria_keyshortcuts, format_shortcut, matches_shortcut};
pub use ui_sdk_shortcuts::{
    ShortcutHandler, ShortcutInfo, ShortcutKeys, ShortcutOptions, Shortcuts, register_shortcut, shortcuts,
};
