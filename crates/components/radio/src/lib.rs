//! Port of `components/radio`: radios that share one value, with one tab
//! stop and arrow keys between them, which check as they move.
//!
//! `radio.ts` → [`radio`](mod@radio), `radio-api.ts` → [`radio_api`],
//! `radio-parts.ts` → [`radio_parts`], `index.ts` → these re-exports.
//!
//! An app makes a [`RadioGroup`] with `cx.new(|cx| RadioGroup::new(options,
//! window, cx))` and renders [`RadioGroup::group`], then inside it a
//! [`RadioGroup::root`] per radio, keyed by its value, with
//! [`RadioGroup::indicator`] inside each, which is `None` while it's out of
//! the page. A forms crate `Label` around a radio names it and checks it when
//! pressed; a label or field pointing at the group names the group, and
//! pressing the label focuses the group's tab stop. The group reports its
//! value to its form under `name`. A field the group is the control of
//! disables it with its own `disabled`, and a field around one radio
//! disables that radio, as the original's adapters pass the field's
//! `disabled` in.
//!
//! Tab lands on the checked radio, or with nothing checked the first enabled
//! one, and moves with the value as it changes, before the radios render it.
//! Arrow keys on both axes move to the next or previous enabled radio and
//! check it, unless the group is read-only; `loop` wraps them. Space checks
//! the focused radio, and Enter submits its form.
//!
//! Segmented Control and Rating are radio groups too. Each keeps a
//! [`RadioGroupApi`] in its entity, implements [`RadioHost`], and builds its
//! group and radios with [`radio_group`] (or [`bind_radio_group`] and
//! [`radio_group_attrs`] on a part of its own) and [`radio_item`], under the
//! slot names it gives in [`RadioSlotNames`]. The api keeps what only they
//! read: a segmented control's orientation and direction, a rating's
//! highlighted radio, and each radio's bounds.
//!
//! # Dropped, and why
//!
//! - `radio-generic.ts` (`RadioGroupGeneric`): TypeScript's inference of the
//!   value type for renderers. Values are `SharedString`.
//! - `createRadioGroupController`, `RadioGroupController` and the
//!   `controller` option: the app holds the entity, whose methods are the
//!   controller's.
//! - The `input` slot, a native radio per item for React and Vue: the group
//!   reports to forms through its own internals, as the web adapter's
//!   form-associated group does, and each radio is a control of its own that
//!   labels name and press, as the web adapter's form-associated radios are.
//! - `restore`, the `Restore` reason and the state a group saves with its
//!   value: gpui restores no forms after a reload.
//! - `isNativeButton`, Space's `keyup` check and its `keydown`
//!   `preventDefault`: every part is a div, which gpui clicks on Space's
//!   release, as a browser clicks a native button.
//! - The press's `preventDefault`, which keeps a label around a radio from
//!   clicking it again: the forms crate's labels leave a press inside their
//!   control to the control.
//! - `bindRadioGroup`'s label click and its `pointerdown` and `focusin`
//!   listeners: a label pointing at the group focuses the tab stop through
//!   the group's internals, and a label press focuses a radio only when it's
//!   enabled, which is what the listeners kept a disabled one from.
//! - `selects` and `data-*` attributes: a change renders the group's parts
//!   again, and parts render from state.
//!
//! # Deviations
//!
//! - The indicator takes its radio's value, which the original fills in
//!   from the radio around it.
//! - `indicator_timing` is the transition the original's CSS declares, and
//!   `set_options` and `set_controlled_value` take the window: a controlled
//!   value starts the indicators' animations in the same pass, which run on
//!   animation frames.
//! - Changes the group reports to its host's `value_changed` (`RadioHost`)
//!   stand in for the store subscriptions other components add.

pub mod radio;
pub mod radio_api;
pub mod radio_parts;

pub use radio::{
    RadioGroup, RadioGroupChangeOptions, RadioGroupOptions, RadioGroupState,
    RadioGroupValueChangeReason, RadioIndicatorProps, RadioIndicatorState, RadioProps, RadioState,
};
pub use radio_api::{RadioGroupApi, RadioHost, RadioSlotNames};
pub use radio_parts::{bind_radio_group, radio_group, radio_group_attrs, radio_item};
