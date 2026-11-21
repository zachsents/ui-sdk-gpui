//! Port of `components/toggle`: a button that stays pressed, and a group of
//! them with a shared value and arrow-key navigation.
//!
//! `toggle.ts` → [`toggle`](mod@toggle), `toggle-group.ts` →
//! [`toggle_group`], `index.ts` → these re-exports.
//!
//! An app makes a [`Toggle`] with `cx.new(|cx| Toggle::new(options, window,
//! cx))` and renders [`Toggle::root`], named with `aria_label`, with an
//! optional [`Toggle::icon`] inside, which screen readers skip. A `text!`
//! child names it for screen readers too, but not for the agent bridge or
//! the development check for a toggle with only an icon. A [`ToggleGroup`] renders [`ToggleGroup::group`] around its
//! toggles, each made with the group in its `group` option and a `value`;
//! the group's value lists the pressed ones, in page order. The group is one
//! tab stop: Tab lands on its first enabled pressed toggle, or its first
//! enabled one, and then on the one focused last, and the arrow keys along
//! its orientation, Home and End move between toggles, wrapping unless
//! `loop` is off. Disabled toggles in a group stay focusable and report
//! themselves disabled.
//!
//! Inside a toolbar, a toggle takes the toolbar's scope in its `toolbar`
//! option, and a group in its own: the group's toggles then join the
//! toolbar's tab stop and arrow keys instead of the group's, and a disabled
//! toolbar, or toolbar group, disables them.
//!
//! # Dropped, and why
//!
//! - `createToggleController`, `createToggleGroupController`, their types
//!   and the `controller` options: the app holds the entities, whose
//!   methods are the controllers'.
//! - `ToggleGroupGeneric`: TypeScript's inference of the value type for
//!   renderers. Values are `SharedString`.
//! - The `element` state and `buttonElementAttrs`' `type="button"`: every
//!   part is a div that acts as a button, which gpui presses on Enter and
//!   Space itself (`buttonKeydown`, `buttonKeyup`).
//! - `join`'s follow subscriptions, `queueCheck` on a toggle's `disabled`
//!   and the group's `stop` and `scopeDisabled` state: toggles render from
//!   their group, which they read as they render, and the roving group
//!   checks its stop once every frame that renders them has drawn.
//! - `elementOf`, which lets the agent bridge read a toggle's marks through
//!   its group: each toggle is an instance the bridge reads itself.
//! - `data-*` attributes: part state.
//!
//! # Deviations
//!
//! - The development check for a toggle with only an icon reads the name
//!   the toggle rendered with, its `aria_label` or a labelled-by: gpui
//!   can't read the text it shows.
//! - The group knows its toggles' page order from the frame that rendered
//!   them, so a value set before they first render keeps the order it was
//!   given in.
//! - A toggle reads its group as it renders and as its api runs, so a
//!   group's `on_value_change` runs while the toggle that was pressed and
//!   the group are both being updated, and may update neither.

pub mod toggle;
pub mod toggle_group;

pub use toggle::{
    Toggle, ToggleChangeOptions, ToggleIconProps, ToggleIconState, ToggleOptions, TogglePressedChangeReason,
    ToggleState,
};
pub use toggle_group::{
    ToggleGroup, ToggleGroupChangeOptions, ToggleGroupOptions, ToggleGroupOrientation, ToggleGroupState,
    ToggleGroupValueChangeReason, next_value,
};
