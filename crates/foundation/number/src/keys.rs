//! Port of `foundation/number/keys.ts`.
//!
//! One keyboard map for numeric controls, as the APG's slider and spinbutton
//! patterns describe it, read into a move that [`step_by`] applies.
//!
//! [`step_by`]: crate::step_by

use gpui::{App, Keystroke, Orientation, Window};
use ui_sdk_kernel::{Key, is_composing};

use crate::steps::{NumberBound, NumberMove, StepDirection, StepSize};

/// The direction a control lays out in, like the page's `dir`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dir {
    #[default]
    Ltr,
    Rtl,
}

/// Which arrows step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Arrows {
    /// All four.
    #[default]
    All,
    /// Left and Right, leaving Up and Down to the page, like a timeline in a
    /// list of tracks.
    Horizontal,
    /// Up and Down, leaving Left and Right to a text caret, as a number field does.
    Vertical,
}

/// How a control lays out and which arrows it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NumberKeyOptions {
    /// Which way the control runs. Right to left flips Left and Right only on
    /// a horizontal one. Horizontal by default.
    pub orientation: Orientation,
    /// The control's direction, like `ui_sdk_platform::is_rtl`. Left to right
    /// by default.
    pub dir: Dir,
    /// Which arrows step. All of them by default.
    pub arrows: Arrows,
}

impl Default for NumberKeyOptions {
    fn default() -> Self {
        Self {
            orientation: Orientation::Horizontal,
            dir: Dir::Ltr,
            arrows: Arrows::All,
        }
    }
}

impl NumberKeyOptions {
    pub fn orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    pub fn dir(mut self, dir: Dir) -> Self {
        self.dir = dir;
        self
    }

    /// Lays the control out right to left, or left to right.
    pub fn rtl(mut self, rtl: bool) -> Self {
        self.dir = if rtl { Dir::Rtl } else { Dir::Ltr };
        self
    }

    pub fn arrows(mut self, arrows: Arrows) -> Self {
        self.arrows = arrows;
        self
    }
}

/// The move a key asks of a numeric control, or `None` for keys it leaves
/// alone. Up and Right step up, Down and Left step down, Shift takes the large
/// step and Alt the small one; Page Up and Down take the large step; Home and
/// End go to the bounds. Ctrl and Cmd are the caller's to check. A key an
/// input method is composing with in `window`, like an arrow choosing a
/// candidate, is `None` too.
pub fn step_from_key(
    keystroke: &Keystroke,
    options: NumberKeyOptions,
    window: &Window,
    cx: &App,
) -> Option<NumberMove> {
    if is_composing(window, cx) {
        return None;
    }
    let key = Key::of(keystroke);
    match key {
        Key::Home => return Some(NumberMove::Bound(NumberBound::Min)),
        Key::End => return Some(NumberMove::Bound(NumberBound::Max)),
        Key::PageUp | Key::PageDown => {
            return Some(NumberMove::Step {
                direction: if key == Key::PageUp {
                    StepDirection::Up
                } else {
                    StepDirection::Down
                },
                size: StepSize::Large,
            });
        }
        _ => {}
    }
    let flip = options.dir == Dir::Rtl && options.orientation == Orientation::Horizontal;
    let vertical = match (options.arrows, &key) {
        (Arrows::Horizontal, _) => None,
        (_, Key::ArrowUp) => Some(StepDirection::Up),
        (_, Key::ArrowDown) => Some(StepDirection::Down),
        _ => None,
    };
    let horizontal = match (options.arrows, &key) {
        (Arrows::Vertical, _) => None,
        (_, Key::ArrowRight) => Some(if flip { StepDirection::Down } else { StepDirection::Up }),
        (_, Key::ArrowLeft) => Some(if flip { StepDirection::Up } else { StepDirection::Down }),
        _ => None,
    };
    let direction = vertical.or(horizontal)?;
    let modifiers = keystroke.modifiers;
    Some(NumberMove::Step {
        direction,
        size: if modifiers.shift {
            StepSize::Large
        } else if modifiers.alt {
            StepSize::Small
        } else {
            StepSize::Step
        },
    })
}
