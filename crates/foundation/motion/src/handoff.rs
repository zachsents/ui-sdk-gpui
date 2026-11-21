//! Port of `foundation/motion/handoff.ts`.
//!
//! Who owns a vertical move over a scroll area inside the sheet: the sheet or
//! the scroll area. It is iOS's `prefersScrollingExpandsWhenScrolledToEdge`
//! and Material's pre-scroll and post-scroll rules in one function. The
//! machine evaluates it when the slop is passed and again whenever the scroll
//! area reaches its start. "Start" is the scroll area's edge in the direction
//! that shrinks the sheet.

/// What `handoff` decides from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandoffInput {
    /// The finger moves toward a larger extent.
    pub toward_more: bool,
    /// The scroll area is at its start (scrolled to the top for a bottom sheet).
    pub scroll_at_start: bool,
    /// The sheet rests at its largest snap point.
    pub at_largest: bool,
    /// Dragging up on scrollable content first expands the sheet.
    pub expand_on_scroll: bool,
}

/// Who a move belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoveOwner {
    Sheet,
    Scroll,
}

impl MoveOwner {
    /// The original's name for it, `"sheet"` or `"scroll"`.
    pub fn as_str(self) -> &'static str {
        match self {
            MoveOwner::Sheet => "sheet",
            MoveOwner::Scroll => "scroll",
        }
    }
}

/// Who owns a vertical move over a scroll area inside the sheet.
pub fn handoff(input: HandoffInput) -> MoveOwner {
    if !input.toward_more {
        return if input.scroll_at_start {
            MoveOwner::Sheet
        } else {
            MoveOwner::Scroll
        };
    }
    if !input.at_largest && input.expand_on_scroll {
        MoveOwner::Sheet
    } else {
        MoveOwner::Scroll
    }
}
