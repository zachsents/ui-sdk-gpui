//! Port of `foundation/motion/content-scroll.ts`.
//!
//! The hand-off between moving the sheet and scrolling a scroll area inside
//! it, in one gesture: content scrolled down scrolls back to its top before a
//! pull moves the sheet, and a push past the largest snap point scrolls the
//! content on. Sheets with scroll areas pass this to the machine as
//! `config.content_scroll`; a carousel, whose finger never lands on a scroll
//! area the machine knows about, doesn't.

use crate::handoff::{HandoffInput, MoveOwner, handoff};
use crate::machine::{
    SheetConfig, SheetEffect, SheetEvent, SheetPhase, SheetSnapReason, SheetState, initial_state,
    largest_extent, start_dragging,
};

/// The hand-off rules the machine runs for a sheet with scrollable content.
/// Each takes the state in the phase it names and returns nothing for any
/// other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContentScroll;

impl ContentScroll {
    /// Past the slop, whether the scroll area under the finger takes the
    /// gesture. Returns the scrolling state when it does. `state` is pending
    /// and `event` a move.
    pub fn claim(
        &self,
        state: &SheetState,
        event: &SheetEvent,
        toward_more: bool,
        config: &SheetConfig,
    ) -> Option<SheetState> {
        let SheetEvent::Move {
            y,
            scroll: Some(scroll),
            ..
        } = *event
        else {
            return None;
        };
        if !matches!(state.phase, SheetPhase::Pending { .. }) {
            return None;
        }
        let owner = handoff(HandoffInput {
            toward_more,
            scroll_at_start: scroll.at_start,
            at_largest: state.extent >= largest_extent(config) - 0.5,
            expand_on_scroll: config.expand_on_scroll,
        });
        if owner != MoveOwner::Scroll {
            return None;
        }
        Some(SheetState {
            extent: state.extent,
            resting_index: state.resting_index,
            phase: SheetPhase::Scrolling { last_y: y },
        })
    }

    /// A move while the content scrolls. The content reached its top while
    /// the finger keeps pulling down: the rest of the same gesture moves the
    /// sheet. `state` is scrolling.
    pub fn r#move(&self, state: &SheetState, event: &SheetEvent) -> (SheetState, Vec<SheetEffect>) {
        let (SheetPhase::Scrolling { last_y }, &SheetEvent::Move { y, scroll, .. }) =
            (state.phase, event)
        else {
            return (*state, Vec::new());
        };
        let is_moving_down = y > last_y;
        if is_moving_down && scroll.is_some_and(|scroll| scroll.at_start) {
            return (start_dragging(state, y), Vec::new());
        }
        let scrolling = SheetState {
            phase: SheetPhase::Scrolling { last_y: y },
            ..*state
        };
        (scrolling, Vec::new())
    }

    /// A drag move the content takes, or `None` when the sheet moves.
    /// `delta` is positive when the finger moves up, and `finger_extent` is
    /// where the finger would put the sheet. `state` is dragging.
    pub fn drag(
        &self,
        state: &SheetState,
        event: &SheetEvent,
        delta: f64,
        finger_extent: f64,
        config: &SheetConfig,
    ) -> Option<(SheetState, Vec<SheetEffect>)> {
        let SheetEvent::Move {
            y,
            scroll: Some(scroll),
            ..
        } = *event
        else {
            return None;
        };
        let SheetPhase::Dragging {
            anchor_y,
            anchor_extent,
            is_scrolling_content,
            ..
        } = state.phase
        else {
            return None;
        };
        if is_scrolling_content
            && (if delta > 0.0 {
                !scroll.at_end
            } else {
                !scroll.at_start
            })
        {
            let scrolling = SheetState {
                phase: SheetPhase::Dragging {
                    anchor_y,
                    anchor_extent,
                    last_y: y,
                    is_scrolling_content,
                },
                ..*state
            };
            return Some((scrolling, vec![SheetEffect::Scroll { delta }]));
        }
        let largest = largest_extent(config);
        if !(delta > 0.0 && finger_extent > largest && state.extent <= largest && !scroll.at_end) {
            return None;
        }
        // The sheet reached its largest snap point while the finger keeps going
        // up: the rest of the travel scrolls the content, in the same gesture.
        let mut effects = Vec::with_capacity(2);
        if state.extent != largest {
            effects.push(SheetEffect::Move { extent: largest });
        }
        effects.push(SheetEffect::Scroll {
            delta: finger_extent - largest,
        });
        let scrolling = SheetState {
            extent: largest,
            resting_index: state.resting_index,
            phase: SheetPhase::Dragging {
                anchor_y: y,
                anchor_extent: largest,
                last_y: y,
                is_scrolling_content: true,
            },
        };
        Some((scrolling, effects))
    }

    /// A release while the content scrolls: the sheet reached its largest
    /// snap point on the way, rests there, and the content coasts. `state`
    /// is dragging and `event` a release.
    pub fn release(
        &self,
        state: &SheetState,
        event: &SheetEvent,
        config: &SheetConfig,
    ) -> Option<(SheetState, Vec<SheetEffect>)> {
        let SheetPhase::Dragging {
            is_scrolling_content: true,
            ..
        } = state.phase
        else {
            return None;
        };
        let SheetEvent::Up { velocity, .. } = *event else {
            return None;
        };
        let largest_index = config.snap_points.len().checked_sub(1)?;
        let mut effects = vec![SheetEffect::FlingScroll {
            velocity: -velocity,
        }];
        if state.resting_index != Some(largest_index) {
            effects.push(SheetEffect::Snap {
                id: config.snap_points[largest_index].id.clone(),
                reason: SheetSnapReason::Swipe,
            });
        }
        Some((initial_state(state.extent, Some(largest_index)), effects))
    }
}
