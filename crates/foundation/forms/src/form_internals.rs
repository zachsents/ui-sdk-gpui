//! Port of `foundation/forms/form-internals.ts`: form participation through a
//! control's `ElementInternals`.
//!
//! The original hands a form-associated custom element's internals to the
//! core and forwards its form callbacks here. Every control has internals
//! here, so this is where any control reports its value and validity.

use gpui::{App, SharedString, Window};

use crate::document::Node;
use crate::form::{FormValue, Report, ValidityFlag, report};
use crate::internals::{ElementInternals, ValidityState};

/// The callbacks a browser calls a form-associated element with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormCallback {
    /// Its form reset (`formResetCallback`).
    Reset,
    /// The element's own `setCustomValidity` changed its message. Like a
    /// native input's, it stays through form resets until the app clears it.
    CustomValidity(SharedString),
}

/// Forwards a browser callback to what the control bound. Returns whether it
/// bound one.
pub fn form_callback(
    internals: &ElementInternals,
    callback: FormCallback,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    match callback {
        FormCallback::Reset => {
            let resets = internals.reset_callbacks();
            for reset in &resets {
                reset(window, cx);
            }
            !resets.is_empty()
        }
        FormCallback::CustomValidity(message) => {
            internals.set_custom_validity(message);
            true
        }
    }
}

/// A problem the component finds itself, like a date picker's text that isn't
/// a date: the constraints it fails, its message, and where the message
/// points.
#[derive(Clone, Debug, Default)]
pub struct Problem {
    pub flags: ValidityState,
    pub message: SharedString,
    pub anchor: Option<Node>,
}

impl Problem {
    /// A problem that fails `flag`.
    pub fn new(flag: ValidityFlag, message: impl Into<SharedString>) -> Self {
        Self {
            flags: ValidityState::of(flag),
            message: message.into(),
            anchor: None,
        }
    }

    pub fn anchor(mut self, anchor: Node) -> Self {
        self.anchor = Some(anchor);
        self
    }
}

/// What a control reports through its internals.
#[derive(Clone, Debug, Default)]
pub struct InternalsOptions {
    /// What it submits under `name`, several values under `name` for a
    /// group, or entries of its own, like files. `FormValue::None` submits
    /// nothing.
    pub value: FormValue,
    /// The name it submits a value or values under.
    pub name: Option<SharedString>,
    /// Whether a required value is missing.
    pub value_missing: bool,
    /// The message for a missing value, like `value_missing_message`'s.
    pub message: SharedString,
    /// A problem the component finds itself.
    pub invalid: Option<Problem>,
    /// Where its validation message points, when not at the control.
    pub anchor: Option<Node>,
    /// Its own `disabled`: it's neither submitted nor validated.
    pub disabled: bool,
    /// Whether it's read-only, which it reports to assistive technology.
    pub read_only: bool,
    /// Whether it's required, which fields and assistive technology read.
    pub required: bool,
    /// Keeps it out of constraint validation, like a read-only text field.
    pub barred: bool,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
}

/// Reports a control's value and validity through its internals: its own
/// problem first, then a missing value. The element's own custom validity
/// goes ahead of both. Fields around it hear when what it reports changes.
/// Call it each time the control renders and after its state changes, the
/// way the original syncs on every store change.
pub fn bind_element_internals(internals: &ElementInternals, options: InternalsOptions) {
    report(
        internals,
        Report {
            value: options.value,
            name: options.name,
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            barred: options.barred,
            form: options.form,
        },
    );
    let anchor = options.anchor;
    if let Some(invalid) = options.invalid {
        let anchor = invalid.anchor.or(anchor);
        internals.set_validity(invalid.flags, invalid.message, anchor.as_ref());
    } else if options.value_missing {
        internals.set_validity(
            ValidityState::of(ValidityFlag::ValueMissing),
            options.message,
            anchor.as_ref(),
        );
    } else {
        internals.set_validity(ValidityState::default(), "", anchor.as_ref());
    }
}
