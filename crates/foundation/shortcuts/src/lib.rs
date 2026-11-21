//! Port of `foundation/shortcuts`: keyboard shortcuts registered for the
//! whole app or bound to a part, scoped to the open layers, and the Shortcut
//! part that shows their keys.
//!
//! The shared set is [`shortcuts()`]; [`register_shortcut`] adds to it and
//! returns a [`RegisteredShortcut`] guard that unregisters on release or
//! drop. A shortcut belongs to a layer by its element ([`ShortcutLayer`], a
//! platform `Region` or a focus handle): one in a popup runs while that popup
//! is open, a modal pauses the window's, and one in a persistent layer runs
//! over every modal. A part that registers what its props name, like a
//! trigger's `shortcut`, adds [`bind_part_shortcut`]'s element, which
//! registers while the part renders. [`shortcut_slot()`] builds the Shortcut
//! part, [`shortcut_of`] finds the part an element sits in, and
//! [`adopt_platform`] follows the platform the keys are named for. Keys match
//! and format with the platform crate's `matches_shortcut`,
//! `format_shortcut` and `aria_keyshortcuts`.
//!
//! # TypeScript file → Rust module
//!
//! `create-shortcuts` → [`create_shortcuts`](mod@create_shortcuts),
//! `shortcut-order` → [`shortcut_order`](mod@shortcut_order),
//! `shortcut-slot` → [`shortcut_slot`](mod@shortcut_slot), `shortcuts` →
//! [`shortcuts`](mod@shortcuts), `index` → these re-exports.
//!
//! # Deviations and drops
//!
//! - The document's `keydown` listener is a keystroke observer
//!   (`cx.observe_keystrokes`), which hears a key once the focused element
//!   and the keymap had their turn: a key an element prevented or stopped,
//!   or one an action took, runs no shortcut. It hears every window and
//!   measures a key against the layers of the window it was pressed in. A
//!   native text field edits once every observer had the key, as a default
//!   action follows `keydown`, so a shortcut that takes a key, like `mod+z`,
//!   keeps it from the field.
//! - `repeat`: gpui doesn't tell keystroke observers whether a key is held,
//!   so the kernel's `key_held` says, as the focusable parts a press passed
//!   saw it. A press that met none, like one with nothing focused, counts
//!   as a new one.
//! - A `layer` is a `Region` or a focus handle, not a DOM element: it's in
//!   the window while gpui rendered its focus handle last frame
//!   (`isConnected`), and containment asks about that handle.
//! - `isTypingIn`'s element types: gpui's text entry is the native crate's
//!   field, so typing is a focused field (`ui_sdk_native::is_typing`). There
//!   are no content-editable elements or selects.
//! - `bindPartShortcut`'s `onProps` subscription and cleanup: the
//!   registration lives in an element the part renders, which gpui drops
//!   with the part.
//! - `shortcutOf`'s DOM containment: elements are compared by the paths of
//!   their element ids.
//! - `shortcutSlot`'s `translate="no"`: gpui translates nothing.
//! - `exposeShortcuts`, the devtools' `__UI_SDK__.shortcuts()`, and the
//!   `globalThis` symbol that shares one set between copies of the package:
//!   the kernel dropped the devtools hook, and a Rust app links one copy.
//! - `isBrowser`: there is always a window.
//!
//! Ported.

pub mod create_shortcuts;
pub mod shortcut_order;
pub mod shortcut_slot;
pub mod shortcuts;

pub use create_shortcuts::{
    RegisteredShortcut, ShortcutHandler, ShortcutInfo, ShortcutKeys, ShortcutLayer, ShortcutOptions,
    ShortcutWhen, Shortcuts, create_shortcuts,
};
pub use shortcut_order::{ABOVE_EVERY_LAYER, is_shortcut_allowed, is_typing_in, shortcut_order};
pub use shortcut_slot::{
    ShortcutPart, ShortcutPartElement, ShortcutProps, ShortcutState, adopt_platform, bind_part_shortcut,
    shortcut_of, shortcut_slot, shortcut_text,
};
pub use shortcuts::{register_shortcut, shortcuts};
