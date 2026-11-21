//! Port of `components/segmented-control`: one of a few options picked from
//! a row of buttons, with an indicator that slides to the choice.
//!
//! `segmented-control.ts` → [`segmented_control`], `index.ts` → these
//! re-exports.
//!
//! A segmented control is a radio group, built on `ui-sdk-radio`'s
//! [`RadioGroupApi`](ui_sdk_radio::RadioGroupApi) and parts, as the original
//! is on `radioGroupApi` and `radioSlots`: one value and one tab stop, arrow
//! keys on both axes that move to the next enabled item and check it
//! (unless it's read-only), `loop` to wrap, Space to check and Enter to
//! submit its form. An app makes a [`SegmentedControl`] with
//! `cx.new(|cx| SegmentedControl::new(options, window, cx))` and renders
//! [`SegmentedControl::root`], then inside it the
//! [`SegmentedControl::indicator`] and an [`SegmentedControl::item`] per
//! value. Name the control with `aria_label`, a label or a field. A field it's
//! the control of disables it with its own `disabled`, as the original's
//! adapters pass the field's `disabled` in.
//!
//! The indicator measures the checked item against the root with
//! `ui-sdk-selection`'s indicator, as they lay out, and says which way the
//! checked item last moved (`direction_between`), set before the change
//! renders.
//!
//! # Deviations
//!
//! - The indicator's `--segmented-control-indicator-*` variables are its
//!   state's `item_box`, and `data-activation-direction` its `direction`.
//!   The app places it from them, as the original's CSS does, and skips its
//!   animation while the direction is `None`.
//! - Its direction follows every change to the value, from the value before
//!   it, where the original's follows changes from when the indicator binds.
//!
//! # Dropped, and why
//!
//! - `segmented-control-generic.ts` (`SegmentedControlGeneric`):
//!   TypeScript's inference of the value type for renderers. Values are
//!   `SharedString`.
//! - `createSegmentedControlController`, `SegmentedControlController` and
//!   the `controller` option: the app holds the entity, whose methods are
//!   the controller's.
//! - The `input` slot, a native radio per item for React and Vue: the
//!   control reports to forms through its root's internals, and each item is
//!   a control of its own that labels name and press, as in Radio.
//! - The `Restore` reason: gpui restores no forms after a reload.
//! - `data-*` attributes: part state.

pub mod segmented_control;

pub use segmented_control::{
    SegmentedControl, SegmentedControlChangeOptions, SegmentedControlDirection,
    SegmentedControlIndicatorState, SegmentedControlItemProps, SegmentedControlItemState,
    SegmentedControlOptions, SegmentedControlOrientation, SegmentedControlState,
    SegmentedControlValueChangeReason,
};
