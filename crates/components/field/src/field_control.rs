//! Port of `components/field/field-control.ts`: reading a field's controls
//! from the page: which ones take part in validation, which one the field
//! wires to its parts, and their validity.
//!
//! The original reads native inputs, selects and textareas, and elements
//! with form internals. Every control here reports through its
//! [`ElementInternals`], so those are the controls a field reads.

use gpui::SharedString;
use ui_sdk_forms::{ElementInternals, Node, ValidityFlag, ValidityState};

/// The browser's validity flags, as `ValidityState` names them.
pub type ValidityKey = ValidityFlag;

/// Every validity flag, in the order `ValidityState` lists them.
pub const VALIDITY_KEYS: [ValidityKey; 10] = ValidityFlag::ALL;

/// A field's validity as it last read it: whether its controls pass, the
/// flags of the first one that doesn't, and that one's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validity {
    pub valid: bool,
    pub flags: ValidityState,
    pub message: SharedString,
}

impl Default for Validity {
    fn default() -> Self {
        valid()
    }
}

/// The validity of a field whose controls all pass.
pub fn valid() -> Validity {
    Validity {
        valid: true,
        flags: ValidityState::default(),
        message: SharedString::default(),
    }
}

// Elements that take part in constraint validation: every control that
// takes part in forms.
fn validates(node: &Node) -> bool {
    ElementInternals::of(node).is_some()
}

/// The form controls inside a field, not counting nested fields
/// (`controlsIn`). `root` is the field's root node.
pub fn controls_in(root: &Node) -> Vec<ElementInternals> {
    root.descendants()
        .into_iter()
        .filter(|node| validates(node) && node.closest_tagged("field").as_ref() == Some(root))
        .filter_map(|node| ElementInternals::of(&node))
        .collect()
}

/// What a control reports about its validity (`validityOf`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlValidity {
    /// Whether it takes part in constraint validation now.
    pub will_validate: bool,
    pub validity: ValidityState,
    pub message: SharedString,
}

/// A control's validity, as its internals report it.
pub fn validity_of(control: &ElementInternals) -> ControlValidity {
    ControlValidity {
        will_validate: control.will_validate(),
        validity: control.validity(),
        message: control.validation_message(),
    }
}

/// Whether a control says it's required. Our own controls report it, and
/// groups too, as the original's say `aria-required`.
pub fn is_required(control: &ElementInternals) -> bool {
    control.required()
}
