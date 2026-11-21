//! Port of `foundation/disclosure`: what Collapsible and Accordion share, a
//! trigger that shows and hides a panel, and the panel's presence, size and
//! focus handling, from the motion crate's [`Panel`].
//!
//! A disclosure's owner keeps a [`Panel`] per panel, calls `Panel::sync` with
//! a [`PanelBinding`] after each change to its state or the panel's options,
//! puts `Panel::track` on the panel part and styles it from `Panel::state`.
//! Its trigger takes [`bind_trigger_element`], and its press
//! [`focus_pressed_trigger`]. [`PendingReason`] carries why a change was asked
//! for to the value it causes, which arrives later when controlled.
//!
//! Dropped from the original:
//! - `triggerKeydown`, which clicks a trigger that isn't a button on Enter and
//!   Space: gpui clicks a focused element on Enter and Space itself (a
//!   keyboard click), so every trigger activates as a button does.
//! - `openAttrs`, the kernel's `data-open` and `data-closed`, which this
//!   module passes on: parts carry `open` in their state.
//! - `bindFindable` and `hidden="until-found"`: find in page has no gpui
//!   meaning. `supports_hidden_until_found` stays, always false.

pub mod disclosure;

pub use disclosure::{
    Opening, Panel, PanelBinding, PanelExitedCallback, PanelSettledCallback, PanelState,
    PendingReason, bind_trigger_element, create_pending_reason, focus_pressed_trigger,
    hidden_attrs, supports_hidden_until_found,
};
