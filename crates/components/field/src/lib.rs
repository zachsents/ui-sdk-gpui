//! Port of `components/field`: connects a form control to its label, its
//! description and the errors it shows.
//!
//! # TypeScript file → Rust module
//!
//! `field.ts` → [`field`] (`field` is [`Field`], `FieldApi` its methods),
//! `field-attrs.ts` → `field_attrs` (the store, the parts' state and the
//! attributes the field writes on its control), `field-control.ts` →
//! [`field_control`], `field-outside.ts` → [`field_outside`],
//! `field-set.ts` → [`field_set`] (`fieldSet` is [`FieldSet`], `fieldGroup`
//! is [`FieldGroup`]).
//!
//! # In gpui
//!
//! - The control is one of ours, which takes part in forms through its
//!   `ElementInternals`, or a control of the app's own put in the field's
//!   `control` part. Once a frame has drawn, the field finds it in its root,
//!   reads the validity of the controls inside, and writes what names and
//!   describes the control with `set_control_attrs`: the label, the
//!   descriptions and errors in the page, whether it shows as invalid, the
//!   field's `disabled` and `name`. Every control renders that through its
//!   internals' ARIA, a frame after the field read it, as controls hear
//!   their form and labels in the forms crate, and takes the `disabled` and
//!   `name` as its own, as the original's adapters pass them in. A control of the app's own
//!   reports its value and validity through [`Field::control_internals`],
//!   as a native input would by itself.
//! - The label part is the forms crate's `Label`, whose `for` names the
//!   control when the control can take a label that way, so the forms code
//!   counts it among the control's labels. A press on it activates the
//!   control as a native label does.
//! - Parts are named for assistive technology with `aria_label`, since text
//!   children don't name a div in gpui, and descriptions and errors take a
//!   role of their own so the control's relations find them. An error's
//!   [`FieldErrorProps::text`] names it, and is what the field announces;
//!   without it, the browser's message.
//! - An error that doesn't show isn't rendered: [`Field::error`] returns
//!   `None` (the slot's `present`). The field remembers every error its last
//!   render asked for, shown or not, which is what an invalid submit reads
//!   (`held`).
//! - `focused` comes from a focus handle the root tracks, which never takes
//!   focus itself: a press on the root leaves focus where it is.
//! - The original reads validity on `input` and `change`, on `invalid` and
//!   `reset`, when `invalid` is passed or let go, on a component's own
//!   check, and when a `MutationObserver` sees a control become required,
//!   disabled or enabled, or leave. Here the field listens for the same
//!   events, and reads its controls again each time a frame has drawn it,
//!   which is when the mutations would show.
//! - `debug` sets the instance's flag: a field has no change events, and
//!   the original logs none of its writes either.
//! - The agent state's `errors` lists what the errors in the page say,
//!   where the original lists their element ids.
//!
//! # Dropped, and why
//!
//! - `data-*` attributes on the parts and the control: part state.
//! - The ids made before the field binds, so server markup and the first
//!   render agree: the control pointing at `${id}-label`,
//!   `${id}-description` and `${id}-error` until its parts register
//!   (`references`, `bound`), and the first description and the first
//!   error that shows taking those bare ids (`descriptionId`, `errorId`,
//!   `holdError`). There's no server render, and relations follow parts by
//!   the ids their slots make.
//! - `registerById`: a part that takes an author's id later keeps its made
//!   one in the kernel's registry, so what points at it still does.
//! - `afterBind` and the microtasks: the field settles once a frame has
//!   drawn.
//! - `matchAttribute`: an error's `match` as a custom element's attribute.
//! - The document listener that waits for React's controlled inputs: forms
//!   events go out once the control has taken its value.
//! - Hidden inputs among a field's controls, and other libraries'
//!   form-associated elements (`isFormAssociated`): neither exists here.
//! - `submitterOf`: every control reports through its own internals, so the
//!   control a field finds is the one that submits.

pub mod field;
mod field_attrs;
pub mod field_control;
pub mod field_outside;
pub mod field_set;

pub use field::{
    Field, FieldDescriptionState, FieldErrorMatch, FieldErrorProps, FieldErrorState, FieldOptions,
    FieldState,
};
pub use field_control::{VALIDITY_KEYS, Validity, ValidityKey};
pub use field_outside::{
    FieldOutsideSlot, field_outside_error_shown, field_outside_state,
};
pub use field_set::{FieldGroup, FieldGroupOptions, FieldSet, FieldSetOptions, FieldSetState};
