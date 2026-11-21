//! Port of `foundation/popups/modal-attribute.ts`: a `modal` given as text,
//! like a web component's attribute, as a popup's [`Modal`]. Here that's text
//! from anywhere a setting arrives as a string, like an agent's step or a
//! config file.

use crate::popup::Modal;

/// A `modal` setting given as text, as a [`Modal`] (`modalAttribute`). An
/// empty one means true, like any boolean attribute, `"false"` is false, and
/// `"pointer"` or `"trap-focus"` keep one effect. A missing one is `None`, so
/// the component's default applies.
pub fn modal_attribute(attribute: Option<&str>) -> Option<Modal> {
    let attribute = attribute?;
    Some(match attribute {
        "false" => Modal::False,
        "pointer" => Modal::Pointer,
        "trap-focus" => Modal::TrapFocus,
        _ => Modal::True,
    })
}

/// The same for popups that take no `"trap-focus"`, like menus and the lists
/// of Select, Combobox and Autocomplete: it reads as a bare `modal`
/// (`pointerModalAttribute`).
pub fn pointer_modal_attribute(attribute: Option<&str>) -> Option<Modal> {
    let attribute = attribute?;
    Some(match attribute {
        "false" => Modal::False,
        "pointer" => Modal::Pointer,
        _ => Modal::True,
    })
}
