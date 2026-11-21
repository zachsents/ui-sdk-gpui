//! Port of `components/field/field-attrs.ts`: what a field's parts render:
//! their public state, and the attributes the field puts on its control.
//!
//! The original also writes `data-*` attributes on every part and on the
//! control, which part state replaces, and points the control at the ids its
//! parts take before the field binds (`references`), so server markup and
//! the first render agree. Here the field writes the control's attributes
//! once its parts are in the page, and relations follow parts by the ids
//! their slots make, so there's no pre-bind guess and no first description
//! or error taking a bare id (`descriptionId`). The form-associated element
//! around a control (`submitterOf`) is the control itself, since every
//! control reports through its own internals.

use gpui::{App, ElementId, SharedString};
use ui_sdk_forms::{ControlAttrs, ElementInternals, Node, set_control_attrs};

use crate::field::{FieldErrorMatch, FieldErrorProps, FieldErrorState, FieldState};
use crate::field_control::Validity;

/// A field's state (`FieldStore`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FieldStore {
    pub(crate) disabled: bool,
    // The `name` you pass, which a control without its own submits under.
    pub(crate) name: Option<SharedString>,
    // The `invalid` you pass. `None` leaves it to the browser's validation.
    pub(crate) external: Option<bool>,
    pub(crate) focused: bool,
    // A submit found the field invalid, so the browser's validation shows.
    pub(crate) revealed: bool,
    pub(crate) form_disabled: bool,
    pub(crate) required: bool,
    pub(crate) validity: Validity,
    // Parts in the page, by element id. The control points at these.
    pub(crate) labels: Vec<ElementId>,
    pub(crate) descriptions: Vec<ElementId>,
    pub(crate) errors: Vec<ElementId>,
    // The control's id, which labels point at.
    pub(crate) control_id: Option<SharedString>,
    // Whether a label's `for` can point at the control. A radio group can't.
    pub(crate) labelable: bool,
    // A plain button, like a file input's trigger, keeps its own text in its
    // name after the label's.
    pub(crate) named_by_content: bool,
}

impl FieldStore {
    // Whether errors show: what you pass, or the browser's once a submit
    // found the field invalid.
    pub(crate) fn shown(&self) -> bool {
        self.external.unwrap_or(self.revealed)
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled || self.form_disabled
    }

    pub(crate) fn invalid(&self) -> bool {
        self.external.unwrap_or(!self.validity.valid)
    }
}

/// What every part of a field renders (`fieldState`).
pub(crate) fn field_state(store: &FieldStore) -> FieldState {
    let show = store.shown();
    let bad = store.invalid();
    FieldState {
        disabled: store.is_disabled(),
        required: store.required,
        focused: store.focused,
        invalid: show && bad,
        // Only the browser's own validation says a value is valid.
        valid: store.external.is_none() && show && !bad,
    }
}

/// What an error renders (`errorState`).
pub(crate) fn error_state(store: &FieldStore) -> FieldErrorState {
    let FieldState {
        disabled,
        required,
        focused,
        invalid,
        valid,
    } = field_state(store);
    FieldErrorState {
        disabled,
        required,
        focused,
        invalid,
        valid,
        message: store.validity.message.clone(),
    }
}

/// Whether an error shows (`errorShown`). `match` always or never decides on
/// its own. Otherwise an error shows while the field shows as invalid, and a
/// validity flag only while the browser's own validation reports that
/// problem.
pub(crate) fn error_shown(store: &FieldStore, props: &FieldErrorProps) -> bool {
    match props.r#match {
        Some(FieldErrorMatch::Always) => return true,
        Some(FieldErrorMatch::Never) => return false,
        _ => {}
    }
    if store.is_disabled() || !store.shown() || !store.invalid() {
        return false;
    }
    if let Some(FieldErrorMatch::Flag(flag)) = props.r#match {
        return store.external.is_none() && store.validity.flags.has(flag);
    }
    true
}

/// The attributes the field writes on its control (`controlAttrs`). The
/// control's own description, like a date picker's format hint, comes first,
/// then the field's descriptions and errors: the control merges its own
/// ahead of these. `content` is the element whose own text names a plain
/// button after the label, like a file input's trigger, and `fallback` the
/// id the field gives a control with none of its own.
pub(crate) fn control_attrs(
    store: &FieldStore,
    content: Option<ElementId>,
    fallback: SharedString,
) -> ControlAttrs {
    let FieldState { invalid, .. } = field_state(store);
    let described_by = store
        .descriptions
        .iter()
        .chain(&store.errors)
        .cloned()
        .collect();
    // A button's own text says what it does, like "Browse files", so it
    // follows the label. Also written beside a label's `for`, so labels a
    // control can't take that way, like a radio group's, name it too.
    let mut labelled_by = store.labels.clone();
    if store.named_by_content
        && !labelled_by.is_empty()
        && let Some(content) = content
    {
        labelled_by.push(content);
    }
    ControlAttrs {
        id: Some(fallback),
        name: store.name.clone(),
        labelled_by,
        described_by,
        invalid,
        disabled: store.disabled,
    }
}

/// The control a field wires, and what it last wrote on it
/// (`bindFoundControl`'s own state). The field finds its control in its
/// root once a frame has drawn, adopts it and writes its attributes. Only
/// attributes the field wrote are its to take off again.
#[derive(Default)]
pub(crate) struct ControlBinding {
    current: Option<ElementInternals>,
    written: Option<ControlAttrs>,
}

impl ControlBinding {
    /// The control it wires now.
    pub(crate) fn current(&self) -> Option<&ElementInternals> {
        self.current.as_ref()
    }

    /// Wires `next` from now on. The control before it, if the field still
    /// holds what it wrote there, gets its own attributes back. Returns
    /// whether the control changed.
    pub(crate) fn find(&mut self, next: Option<ElementInternals>, cx: &mut App) -> bool {
        if next == self.current {
            return false;
        }
        self.release(cx);
        self.current = next;
        true
    }

    /// Writes the field's attributes on its control. A control found in
    /// the root that moved out, maybe into another field, is no longer this
    /// one's to write; `own` is the field's own control part, which is.
    pub(crate) fn write(
        &mut self,
        store: &FieldStore,
        root: &Node,
        own: bool,
        fallback: SharedString,
        cx: &mut App,
    ) {
        let Some(control) = self.current.clone() else {
            return;
        };
        if !own && !root.contains(control.node()) {
            return;
        }
        let attrs = control_attrs(store, control.named_by_content(), fallback);
        set_control_attrs(control.node(), Some(attrs.clone()), cx);
        self.written = Some(attrs);
    }

    /// Takes what the field wrote off its control, when it's still there.
    pub(crate) fn release(&mut self, cx: &mut App) {
        let (Some(control), Some(written)) = (self.current.take(), self.written.take()) else {
            return;
        };
        if control.control_attrs() == written {
            set_control_attrs(control.node(), None, cx);
        }
    }
}
