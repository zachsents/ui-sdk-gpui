//! Port of `foundation/preferences`: a value people choose from a list and
//! keep between runs, the app's color scheme, and the device's pointer.
//!
//! A preference and a color scheme are entities that views read in
//! `render`, observe, or `follow` into their own state, where the original
//! shows the value on a `data-*` attribute for CSS. A change to either goes
//! through the kernel's `Controllable`, so it reports a reason and a source,
//! can be canceled, and can be controlled, like any changeable state.
//!
//! What gpui reports in place of the media queries the original reads:
//! - `prefers-color-scheme`: the window's appearance, `window.appearance()`,
//!   heard through `cx.observe_window_appearance`.
//! - `pointer`, `hover`, `any-pointer` and `any-hover`: nothing. gpui doesn't
//!   report the device's pointers, so a `PointerKind` is what the app tells
//!   it.
//!
//! Reduced motion isn't one of these in the original: the platform module
//! reads it (`prefersReducedMotion`).
//!
//! Dropped from the original:
//! - `attribute`, `global` and their check: where `Preferences` shows a value
//!   on a `data-*` attribute, on `<html>` or on its own element, for CSS. gpui
//!   styles from state.
//! - `Preferences`, `preferencesScript` and `PreferenceLike`: the framework
//!   component and inline script that show preferences on their attributes
//!   before hydration, and the shape they take.
//! - `initialState`, a color scheme's `initial`, the `"unknown"` system
//!   scheme, `colorSchemeOf` and `pointerKindOf`: what the server and the
//!   first client render assume, read from cookies or request headers, so
//!   hydration matches. Nothing here renders on a server or hydrates, and
//!   gpui always knows a window's appearance.
//! - Reading local and session storage back on the first subscription, with
//!   the reason `Restore`, and hearing it only while subscribed: a preference
//!   reads its storage as it's created, as the original does for a cookie,
//!   and hears it while it lives (`preference.rs`).
//! - `view`: a color scheme reads the window `ColorScheme::new` is given, and
//!   a pointer kind reads no window.
//! - `PreferenceStore`, `ColorSchemeStore` and `PointerKindStore`: `follow`
//!   takes the field of the entity's state to keep in step, not a store and
//!   a key.
//! - A pointer kind's `initialState`, `subscribe` and `follow`: nothing
//!   changes it.
//! - States that stay the same object until something changes, so frameworks
//!   can compare snapshots: states compare by value.

pub mod color_scheme;
pub mod pointer_kind;
pub mod preference;

pub use color_scheme::{
    ColorScheme, ColorSchemeOptions, ColorSchemeState, ColorSchemeValue, SystemColorScheme,
    SystemValueChangeCallback,
};
pub use pointer_kind::{
    PointerHover, PointerKind, PointerKindOptions, PointerKindState, PointerPrecision,
};
pub use preference::{Preference, PreferenceOptions, PreferenceState, PreferenceValue};
