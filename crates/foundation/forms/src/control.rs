//! Port of `foundation/forms/control.ts`: the element a label names and
//! activates.
//!
//! The original matches a selector of this package's controls and native
//! ones. Here a control is a node whose internals say it is one, which every
//! control that takes part in forms does by default. A radio is the control
//! of a label for it alone. A label for its whole group finds the group
//! first, since it comes first in the page. A slider with several thumbs is a
//! group the label names, ahead of its thumbs; with one thumb, the slider
//! clears `set_control` on its root, so the thumb is the control. The
//! selector leaves out plain buttons, and so does [`find_control`]; a `Label`
//! still names one, as a native `<label>` names a `<button>`.

use gpui::{App, Window};
use ui_sdk_kernel::InputEvent;

use crate::document::{Node, is_control};
use crate::form::focus_control;
use crate::internals::ElementInternals;

/// The control at or inside `root` that a label names: `root` itself when it's
/// one, else the first control inside it, in page order. `accepts` skips
/// controls that belong to something else, like a nested field. A plain
/// button is never one.
pub fn find_control(root: &Node, accepts: impl Fn(&Node) -> bool) -> Option<Node> {
    if is_found(root) {
        return Some(root.clone());
    }
    root.descendants()
        .into_iter()
        .find(|node| is_found(node) && accepts(node))
}

// What the original's selector takes: any control but a plain button, one
// that submits, resets or does nothing to its form. A file input's trigger,
// which the selector takes, isn't one.
fn is_found(node: &Node) -> bool {
    is_control(node) && ElementInternals::of(node).is_none_or(|internals| internals.button().is_none())
}

/// Activates a control the way a native label does: a checkbox, switch,
/// radio, radio group or plain button is pressed, as its internals'
/// activation says, and anything else is focused. A group, like a range
/// slider, focuses its tab stop.
pub fn activate_control(control: &Node, event: Option<InputEvent>, window: &mut Window, cx: &mut App) {
    match ElementInternals::of(control).and_then(|internals| internals.activation()) {
        Some(activate) => activate(event, window, cx),
        None => focus_control(control, window, cx),
    }
}
