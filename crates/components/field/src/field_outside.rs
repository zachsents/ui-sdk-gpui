//! Port of `components/field/field-outside.ts`: what a field's parts show
//! outside a field.
//!
//! The original's adapters render a FieldLabel, FieldDescription,
//! FieldError or FieldControl outside any Field as a plain element with no
//! field wiring, so a FieldLabel can wrap a whole Field, like a choice card,
//! and label the control inside it the way any native `<label>` does. Here
//! every part comes from its field, so an element outside one is the app's
//! own: the forms crate's `Label`, or the label component, around a field
//! labels the control inside it. These say what such an element shows, for
//! an app that renders the same piece inside a field and outside one.
//!
//! Dropped: `checkFieldOutside`, the development warning for a label outside
//! a field that labels nothing or a control nothing names, with its wait
//! and its record of the elements it checked. No part renders outside its
//! field here, and a label outside one is the label component's.

use crate::field::{FieldErrorMatch, FieldErrorProps, FieldErrorState};

/// The parts that may render outside a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldOutsideSlot {
    Label,
    Description,
    Error,
    Control,
}

impl FieldOutsideSlot {
    /// The part's slot name, so styles that pick a part by its slot still
    /// match it (`fieldOutsideAttrs`).
    pub fn slot_name(self) -> &'static str {
        match self {
            FieldOutsideSlot::Label => "field-label",
            FieldOutsideSlot::Description => "field-description",
            FieldOutsideSlot::Error => "field-error",
            FieldOutsideSlot::Control => "field-control",
        }
    }
}

/// What a part outside a field shows: no field disables, requires or
/// invalidates it.
pub fn field_outside_state() -> FieldErrorState {
    FieldErrorState::default()
}

/// Whether an error outside a field shows. No validation decides it, so only
/// a `match` of `Always` does.
pub fn field_outside_error_shown(props: &FieldErrorProps) -> bool {
    props.r#match == Some(FieldErrorMatch::Always)
}
