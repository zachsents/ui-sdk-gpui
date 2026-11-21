//! Port of `foundation/lists/field-name.ts`: how a field's list is named
//! when no label part names it. A list labelled by the input itself would be
//! named by the text typed in it.

use gpui::{ElementId, SharedString};
use ui_sdk_kernel::{Aria, Instance};

/// How the field is named when no label part names it (`FieldName`): by the
/// parts that label its input, or by a label of its own. `None` names it by
/// its input.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldName {
    LabelledBy(Vec<ElementId>),
    Label(SharedString),
}

/// What names a field's input, from what the app gave it (`fieldNameOf`): its
/// labelled-by, unless that's the component's own label, else the labels
/// pointing at it, like a Field's (`labels`, by their element ids), else its
/// own label. The original reads these off the input's attributes and its
/// `labels`; here the component passes them: the label its input rendered
/// with (`name_of`), and what its form internals say.
pub fn field_name_of(
    labelled_by: &[ElementId],
    own_label: &ElementId,
    labels: &[ElementId],
    aria_label: Option<&SharedString>,
) -> Option<FieldName> {
    if !labelled_by.is_empty() && !(labelled_by.len() == 1 && &labelled_by[0] == own_label) {
        return Some(FieldName::LabelledBy(labelled_by.to_vec()));
    }
    if !labels.is_empty() {
        return Some(FieldName::LabelledBy(labels.to_vec()));
    }
    aria_label
        .filter(|label| !label.is_empty())
        .map(|label| FieldName::Label(label.clone()))
}

/// Keeps a field's name current (`bindFieldName`): writes `next` into
/// `current` when it changed, and says whether it did, so the component
/// renders again. The original writes it a microtask after the input binds;
/// call it once the input has rendered, like from its part's `on_props`.
pub fn bind_field_name(current: &mut Option<FieldName>, next: Option<FieldName>) -> bool {
    if *current == next {
        return false;
    }
    *current = next;
    true
}

/// The list's name (`listName`): the label part names the list while it's in
/// the page (`labeled`), or else what names the field, or else its input.
pub fn list_name(labeled: bool, field_name: Option<&FieldName>, instance: &Instance) -> Aria {
    if labeled {
        return Aria::new().labelled_by(instance.element_id("label", None));
    }
    match field_name {
        Some(FieldName::Label(label)) => Aria::new().fallback_label(label.clone()),
        Some(FieldName::LabelledBy(ids)) => ids
            .iter()
            .fold(Aria::new(), |aria, id| aria.labelled_by(id.clone())),
        None => Aria::new().labelled_by(instance.element_id("input", None)),
    }
}
