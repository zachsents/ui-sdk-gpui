//! Port of `components/checkbox`: a checkbox, checked, unchecked or
//! indeterminate, and a group of them with one list of checked values and a
//! parent checkbox.
//!
//! `checkbox.ts` → [`checkbox`](mod@checkbox), `checkbox-group.ts` →
//! [`checkbox_group`], `index.ts` → these re-exports, with what
//! `checkbox.ts` re-exports from `ui-sdk-forms`.
//!
//! An app makes a [`Checkbox`] with `cx.new(|cx| Checkbox::new(options,
//! window, cx))` and renders [`Checkbox::root`] with
//! [`Checkbox::indicator`] inside it, which is `None` while the indicator is
//! out of the page, and names it with a forms crate `Label` around it or
//! `aria_label`. A [`CheckboxGroup`] renders [`CheckboxGroup::group`] around
//! its checkboxes, each made with the group in its `group` option; one with
//! `parent` checks or unchecks the rest. The checkbox is the forms crate's
//! `ToggleControl`, which takes part in forms, labels and fieldsets.
//!
//! # Dropped, and why
//!
//! - `createCheckboxController`, `createCheckboxGroupController`, their
//!   types and the `controller` options: the app holds the entities, whose
//!   methods are the controllers'.
//! - The `input` and `group-input` slots, native inputs for React and Vue:
//!   the checkbox and the group report to forms through their own
//!   internals, as the original's form-associated elements do.
//! - `CheckboxGroupGeneric`: TypeScript's inference of the value type. Values
//!   are `SharedString`.
//! - The `restore` reason, `state` and `restore` for a form restored after a
//!   reload: gpui has neither.
//! - `live` for a parent's `aria-controls`, and `data-*` attributes: parts
//!   render from state.
//!
//! # Deviations
//!
//! - The group submits every checked value under its name, as the web
//!   adapter's form-associated group does. A checkbox in a group submits
//!   only under a `name` of its own; React's hidden inputs take the group's.
//! - A group knows its checkboxes' page order once a frame has drawn them,
//!   so a parent without `all_values` shows its state from the frame after.
//! - `set_options` takes the window: a controlled `checked` starts the
//!   indicator's exit in the same pass, which runs on animation frames.
//!   `indicator_timing` is the transition the original's CSS declares.

pub mod checkbox;
pub mod checkbox_group;

pub use checkbox::{Checkbox, CheckboxIndicatorProps, CheckboxIndicatorState, CheckboxOptions};
pub use checkbox_group::{
    CheckboxGroup, CheckboxGroupChangeOptions, CheckboxGroupOptions, CheckboxGroupState,
    CheckboxGroupValueChangeReason, group_checked,
};
pub use ui_sdk_forms::{CheckboxChangeOptions, CheckboxCheckedChangeReason, CheckboxState, CheckedState};
