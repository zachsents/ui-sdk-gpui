//! Port of `components/switch`: turns a setting on or off.
//!
//! # TypeScript file → Rust module
//!
//! `switch.ts` → [`switch`] (`switchPrimitive` is [`Switch`]).
//!
//! # In gpui
//!
//! - The forms crate's `ToggleControl` is the original's `toggleControl`,
//!   `toggleState` and `toggleSlots`' `root`; a switch's booleans go in and
//!   out of it as checked states.
//! - Inside a field, the field writes its control attributes on the switch's
//!   internals, and the control takes the field's `disabled` from them, as
//!   the original's adapters pass a field's `disabled` into the switch.
//!
//! # Dropped, and why
//!
//! - `createSwitchController` and the `controller` option: the entity is the
//!   controller (`toggle`, `is_checked`, `set_checked`, `focus`).
//! - The `input` slot, the hidden native checkbox: every control reports to
//!   the form through its internals (see the forms crate).
//! - `toggleDataAttrs` on the thumb: CSS hooks; the thumb's state is the
//!   switch's.
//! - The `restore` reason, for a form the browser restores after a reload or
//!   a trip through history: gpui has neither, so the forms crate drops it.

pub mod switch;

pub use switch::{
    Switch, SwitchCheckedChange, SwitchCheckedChangeReason, SwitchOptions, SwitchState,
    SwitchThumbState,
};
