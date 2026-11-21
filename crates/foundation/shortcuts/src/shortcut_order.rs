//! Port of `foundation/shortcuts/shortcut-order.ts`: how the shortcut manager
//! decides whether a key press runs a shortcut: where it stands among the
//! open layers, and whether the user is typing. Internal to
//! [`create_shortcuts`](fn@crate::create_shortcuts), kept apart so it reads
//! alone.

use gpui::{App, Window};
use ui_sdk_platform::{Layer, Region, Target};

use crate::create_shortcuts::ShortcutLayer;

/// The order of a shortcut that stands above every layer, the original's
/// `Infinity`.
pub const ABOVE_EVERY_LAYER: u64 = u64::MAX;

// Whether a layer's element is in the window: its focus handle rendered last
// frame, or, without one, it has laid out.
fn is_connected(el: &Region, window: &Window) -> bool {
    match el.focus_handle() {
        Some(handle) => handle.is_rendered(window),
        None => el.bounds().is_some(),
    }
}

// What containment asks about for a layer's element: its focus handle, or
// the middle of where it laid out.
fn target_of(el: &Region) -> Option<Target> {
    el.focus_handle()
        .map(Target::Focus)
        .or_else(|| el.bounds().map(|bounds| Target::Point(bounds.center())))
}

/// Where a shortcut stands among the open layers: the order of the layer it
/// belongs to, 0 for the window, or `None` when it belongs to a popup that's
/// closed, whether it left the window or stays in it hidden. One in a
/// persistent layer, like a toast region, stands above every layer
/// ([`ABOVE_EVERY_LAYER`]), so no modal pauses it.
pub fn shortcut_order(
    layer: Option<&ShortcutLayer>,
    find: impl FnOnce(&Target) -> Option<Layer>,
    persistent: impl FnOnce(&Target) -> bool,
    in_popup: impl FnOnce(&Target) -> bool,
    window: &Window,
    cx: &App,
) -> Option<u64> {
    let Some(layer) = layer else {
        return Some(0);
    };
    let el = layer.element(cx)?;
    if !is_connected(&el, window) {
        return None;
    }
    let target = target_of(&el)?;
    if persistent(&target) {
        return Some(ABOVE_EVERY_LAYER);
    }
    if let Some(open) = find(&target) {
        return Some(open.order());
    }
    if in_popup(&target) { None } else { Some(0) }
}

/// Whether a shortcut at `order` may run while the topmost blocking layer,
/// like a modal dialog, opened at `blocking`. Shortcuts of that layer and of
/// layers opened after it run. The window's and those of layers beneath
/// pause.
pub fn is_shortcut_allowed(order: Option<u64>, blocking: Option<u64>) -> bool {
    let Some(order) = order else {
        return false;
    };
    blocking.is_none_or(|blocking| order >= blocking)
}

/// Whether a key press happened while typing text, where letters are text:
/// a text field has focus. gpui's text entry is the native crate's field,
/// which stands in for `<input>` and `<textarea>`.
pub fn is_typing_in(window: &Window) -> bool {
    ui_sdk_native::is_typing(window)
}
