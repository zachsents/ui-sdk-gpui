//! Port of `foundation/forms`: how controls take part in forms, and what the
//! browser gives them to do it, which gpui has none of.
//!
//! The original ties controls to HTML forms: hidden inputs or
//! `ElementInternals` carry a control's name and value, constraint
//! validation blocks a submit, labels name and activate controls, fieldsets
//! disable them, and a form reset puts every default back. Here the forms
//! crate is that browser too:
//!
//! - [`document`] stands in for the page: a [`Node`] per element the forms
//!   code reads, which its part puts in its element with `track`, so each
//!   frame says what holds it and where it sits in page order. What follows
//!   from the tree, like the form that owns a control or the fieldsets that
//!   disable it, is worked out once a frame has drawn, and the entities that
//!   read it are notified. Events (`Input`, `Change`, `Invalid`, `Submit`,
//!   `Reset`) travel through it as in the DOM.
//! - [`elements`] stands in for `<form>`, `<fieldset>`, `<legend>` and
//!   `<label>`, and the [`FormData`] a form submits.
//! - [`internals`] stands in for `ElementInternals`, which every control
//!   uses: the form it belongs to, what it submits, its validity, the labels
//!   that name it.
//!
//! A component joins a form in three steps: [`ElementInternals::new`] in its
//! constructor, with [`associate`] for its form's resets; `track` on its
//! control's element and `aria` on its part; and a binder
//! ([`bind_internals`], [`bind_hidden_input`] or [`bind_value_input`]) each
//! time it renders and after it changes. [`ToggleControl`] and
//! [`TextFieldApi`] do all three for checkboxes, switches and text fields.
//!
//! # TypeScript file → Rust module
//!
//! `control` → [`control`], `files` → [`files`], `form` → [`form`],
//! `form-internals` → [`form_internals`](mod@form_internals), `input-hints` → [`input_hints`](mod@input_hints),
//! `text-field` → [`text_field`] (`textFieldApi` is [`TextFieldApi`]),
//! `toggle-control` → [`toggle_control`] (`toggleControl` is
//! [`ToggleControl`], `toggleState` its `new`, `syncToggle` its
//! `set_options`, `toggleSlots`' `root` its `root`, and `CheckedState` the
//! kernel's `Checked`), `value` → [`value`], `index` → these re-exports.
//! [`document`], [`elements`] and [`internals`] are what the browser gives
//! those files.
//!
//! # Dropped, and why
//!
//! - Hidden inputs, `hiddenInputStyle`, `attachFormInternals`, and the split
//!   between a framework's hidden input and a custom element's internals:
//!   every control reports through its internals, so `bindHiddenInput` and
//!   `bindValueInput` report what the hidden input would have, and the
//!   toggles' `input` slot is gone.
//! - `takeTypedText` and `checkedAttribute`: text typed before the page's
//!   script ran, and attributes of server markup. There is always a window.
//! - Restoring a form after a reload or a trip through history
//!   (`formStateRestoreCallback`, a control's saved `state`, `restore`):
//!   gpui has neither.
//! - Autofill (`bindValueInput`'s `autofill`): no browser writes the value.
//! - `toggleDataAttrs`: CSS hooks. Parts expose state instead.
//! - `value.ts`'s wrapper around a native input's `value`: the text control
//!   is the native crate's, and whoever writes it tells the watchers.
//! - A label's mousedown that stops a double click from selecting its text:
//!   gpui text isn't selected by clicks.
//! - The browser's validation bubble: headless, a form hands what it would
//!   show to `on_report`.

pub mod control;
pub mod document;
pub mod elements;
pub mod files;
pub mod form;
pub mod form_internals;
pub mod input_hints;
pub mod internals;
pub mod text_field;
pub mod toggle_control;
pub mod value;

pub use control::{activate_control, find_control};
pub use document::{
    FormEvent, FormEventType, FormListener, Node, NodeKind, WeakNode, document_order,
    listen_document,
};
pub use elements::{
    Fieldset, Form, FormData, FormDataEntryValue, FormOptions, Label, LabelOptions, Legend,
    SubmitDetails, ValidityReport,
};
pub use files::{
    Accept, DragStatus, FileLike, FileRejection, FileRejectionReason, FileRules, FormFile,
    ValidatedFiles, accepts_file, accepts_type, drag_status, format_file_size, parse_accept,
    relative_path, set_relative_path, validate_files,
};
pub use form::{
    Association, FormValue, HiddenInputOptions, INPUT_AND_CHANGE, ValidityFlag, ValueInputOptions,
    ValueMissingKind, activate_button, associate, bind_hidden_input, bind_internals,
    bind_segment_submit, bind_value_input, claim_invalid_focus, component_flag_of,
    dispatch_change, element_moved, field_control_id, fieldset_disabled, focus_control,
    focus_with, form_internals, invalid_focus_claimed, is_disabled, is_disabled_element,
    is_labelable, labels_of, name_from_labels, on_validity_change, set_component_validity,
    set_control_attrs, set_value_missing_message, submit_implicitly, validity_changed,
    value_missing_message, watch_fieldsets, watch_form_disabled,
};
pub use form_internals::{FormCallback, InternalsOptions, Problem, bind_element_internals, form_callback};
pub use input_hints::{
    AutoCapitalize, EnterKeyHint, InputHintKind, InputHintOptions, InputHints, InputMode,
    input_hints,
};
pub use internals::{ActivateCallback, ButtonType, ControlAttrs, ElementInternals, ValidityState};
pub use text_field::{
    ClearTarget, FocusTarget, InputValueChangeReason, TextFieldApi, TextFieldHost,
    TextFieldOptions, TextFormOptions, TextState, focus_from_surroundings, initial_text, sync_text,
    text_fields,
};
pub use toggle_control::{
    CheckboxChangeOptions, CheckboxCheckedChangeReason, CheckboxState, CheckedState, SwitchState,
    ToggleControl, ToggleGroup, ToggleHost, ToggleOptions, ToggleRole, checkbox_state,
    switch_state,
};
pub use value::{
    TextControl, ValueSource, after_reset, keeps_value_on_reset, notify_value, watch_value,
    write_from_script, write_value, writing_from_script,
};
