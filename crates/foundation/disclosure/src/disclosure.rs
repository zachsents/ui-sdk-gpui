//! Port of `foundation/disclosure/disclosure.ts`.
//!
//! What Collapsible and Accordion share: a trigger that shows and hides a
//! panel, and the panel's presence, size and focus handling. An accordion is
//! a group of these, so both primitives build on this module.

use std::time::{Duration, Instant};

use gpui::{App, Div, FocusHandle, Role, Stateful, StatefulInteractiveElement, Window};
use ui_sdk_kernel::part::{self, FocusMode};

pub use ui_sdk_motion::panel::{
    Opening, Panel, PanelBinding, PanelExitedCallback, PanelSettledCallback, PanelState,
    hidden_attrs, supports_hidden_until_found,
};

// How long a reason waits for the value it asked for.
const LAPSE: Duration = Duration::from_millis(100);

/// The reason of the change in flight, for the value change it causes. A
/// controlled value arrives later, through options, and a parent that
/// ignores the request never sends it back, so the reason lapses 100 ms after
/// it was asked for: a later value of the parent's own isn't this change.
#[derive(Clone, Debug)]
pub struct PendingReason<R> {
    none: R,
    reason: R,
    asked_at: Option<Instant>,
}

impl<R: Clone> PendingReason<R> {
    /// A pending reason that is `none` until one is set.
    pub fn new(none: R) -> Self {
        Self {
            reason: none.clone(),
            none,
            asked_at: None,
        }
    }

    /// The reason a change was just asked for with.
    pub fn set(&mut self, next: R) {
        self.reason = next;
        self.asked_at = Some(Instant::now());
    }

    /// The reason for the value that just arrived, once.
    pub fn take(&mut self) -> R {
        let fresh = self.asked_at.is_some_and(|asked_at| asked_at.elapsed() < LAPSE);
        let taken = if fresh {
            self.reason.clone()
        } else {
            self.none.clone()
        };
        self.reason = self.none.clone();
        taken
    }
}

/// A [`PendingReason`] that is `none` until one is set
/// (`createPendingReason`).
pub fn create_pending_reason<R: Clone>(none: R) -> PendingReason<R> {
    PendingReason::new(none)
}

/// Makes a trigger a button: the role, and a stop in the tab order. Every
/// part is a div here, which the original's triggers rendered as something
/// other than a button need to be one too. A role the app sets after wins.
/// Focus stays on a disabled trigger, as on the original's.
pub fn bind_trigger_element(el: Stateful<Div>, focus: &FocusHandle) -> Stateful<Div> {
    part::apply_focus(el, Some(focus), FocusMode::TabStop, false).role(Role::Button)
}

/// Focuses a pressed trigger that doesn't have focus, so the next Tab starts
/// from it: a press that moved no focus, like one from assistive technology,
/// would leave it where it was. The original does this for Safari, which
/// doesn't focus buttons on click.
pub fn focus_pressed_trigger(trigger: Option<&FocusHandle>, window: &mut Window, cx: &mut App) {
    if let Some(trigger) = trigger
        && !trigger.is_focused(window)
    {
        window.focus(trigger, cx);
    }
}
