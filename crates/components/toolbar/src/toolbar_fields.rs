//! Port of `components/toolbar/toolbar-fields.ts`: text fields in a toolbar,
//! which keep the arrows that move their caret.
//!
//! A field is any `ui_sdk_text::TextField`, like `ui-sdk-native`'s, which is
//! what an `<input>` or a `<textarea>` is to the original.

use gpui::{App, KeyDownEvent, Window};
use ui_sdk_focus::{FieldCaret, field_keeps_key};
use ui_sdk_kernel::Key;
use ui_sdk_platform::is_rtl;
use ui_sdk_text::{TextField, TextFieldKind, TextRange};

// The field's selection, or `None` for one without a caret: a number field
// has no selection, as a native one doesn't.
fn selection(field: &dyn TextField, cx: &App) -> Option<TextRange> {
    (field.kind(cx) != TextFieldKind::Number).then(|| field.selection_range(cx))
}

/// Puts the caret of a text field that focus moved into at the end the move
/// pointed to, so pressing the same arrow again moves on.
pub fn place_caret(field: &dyn TextField, forward: bool, window: &mut Window, cx: &mut App) {
    if selection(field, cx).is_none() {
        return;
    }
    let at = if forward { field.value(cx).len() } else { 0 };
    field.set_selection_range(TextRange::caret(at), window, cx);
}

/// Whether a focused text field keeps a key for its caret. See
/// `field_keeps_key`.
pub fn field_keeps(event: &KeyDownEvent, field: &dyn TextField, cx: &App) -> bool {
    // Fields without a caret, like number fields, have no selection.
    let selection = selection(field, cx);
    let kind = field.kind(cx);
    field_keeps_key(
        &Key::of(&event.keystroke),
        &FieldCaret {
            start: selection.map(|range| range.start),
            end: selection.map(|range| range.end),
            length: field.value(cx).len(),
            rtl: is_rtl(cx),
            uses_up_down: matches!(kind, TextFieldKind::TextArea | TextFieldKind::Number),
        },
    )
}
