//! Port of `components/collapsible`: a trigger that shows and hides one
//! panel, which stays in the page while it animates closed.
//!
//! `collapsible.ts` → [`collapsible`](mod@collapsible), `index.ts` → these
//! re-exports.
//!
//! An app makes a [`Collapsible`] with `cx.new(|cx| Collapsible::new(options,
//! window, cx))` and renders its parts: [`Collapsible::root`] around
//! [`Collapsible::trigger`] and [`Collapsible::panel`], which is `None` while
//! the panel is out of the page. The panel is `ui-sdk-disclosure`'s `Panel`:
//! its part's state carries its presence phases and the height it animates
//! through, which the app gives it, clipping its overflow while it moves.
//! `panel_timing` is the transition the original's CSS declares.
//!
//! # Dropped, and why
//!
//! - `createCollapsibleController`, `CollapsibleController` and the
//!   `controller` option: the app holds the entity, whose methods are the
//!   controller's.
//! - `CollapsibleTriggerProps`, which is empty: the trigger takes no props.
//! - `triggerKeydown`, which clicks a trigger that isn't a button on Enter and
//!   Space: gpui clicks a focused part on both. `bindTriggerElement`'s role
//!   and tab stop are the trigger part's `aria_role` and `focus_mode`.
//! - Find in page and `hidden="until-found"`: gpui has neither. A panel with
//!   `hidden_until_found` stays in the page, hidden, as `keep_mounted` keeps
//!   it, and only a call that passes `Reason::FindInPage` opens it at once.
//! - `settleFlag` and `direct` for the trigger's `aria-controls`: it always
//!   names the panel, and the kernel leaves out a relation to a part that
//!   isn't in the page.
//! - `data-*` attributes and `--collapsible-panel-height` and `-width`: part
//!   state.
//!
//! # Deviations
//!
//! - `set_options` and `set_controlled_open` take the window: a closing `open`
//!   starts the panel's exit in the same pass, which runs on animation frames.
//! - A hidden panel is `display: none`, as the `hidden` attribute makes it,
//!   so what's inside it isn't drawn: its controls leave the form's page
//!   order until it shows, where a hidden native input still submits.

pub mod collapsible;

pub use collapsible::{
    Collapsible, CollapsibleChangeOptions, CollapsibleOpenChangeReason, CollapsibleOptions,
    CollapsiblePanelProps, CollapsiblePanelState, CollapsibleState, CollapsibleTriggerState,
    OpenChangeCompleteCallback,
};
