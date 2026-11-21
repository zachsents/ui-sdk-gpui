//! Port of `foundation/focus/collection.ts`: keyboard and layout helpers
//! shared by collections like tabs, radio groups and toolbars. Pure, so
//! nothing here needs a window.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::{Add, Sub};

use ui_sdk_kernel::Key;

/// Which way a collection runs, which picks the arrows that move through it:
/// Left and Right, or Up and Down. It's gpui's accessibility orientation, so
/// a part hands it to `aria_orientation` as it is.
pub use gpui::Orientation;

/// An orientation as the option gives it: `fallback` when it's unset. The
/// original also turns values that aren't one of the two into the default,
/// which a typed option can't hold.
pub fn orientation_of(value: Option<Orientation>, fallback: Orientation) -> Orientation {
    value.unwrap_or(fallback)
}

/// Which way a selection moved, on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    #[default]
    None,
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    /// The original's name for it, like `"left"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::None => "none",
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        }
    }
}

/// Which arrows move through a list: Up and Down, Left and Right, or all
/// four, like radios and a rating (`"both"` in the original).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Arrows {
    #[default]
    Vertical,
    Horizontal,
    Both,
}

impl From<Orientation> for Arrows {
    fn from(orientation: Orientation) -> Self {
        match orientation {
            Orientation::Horizontal => Arrows::Horizontal,
            Orientation::Vertical => Arrows::Vertical,
        }
    }
}

/// How far a key moves through a list: by some items, or to the first or
/// the last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    /// Forward by a count, or back when it's negative.
    By(isize),
    First,
    Last,
}

impl From<isize> for Step {
    fn from(by: isize) -> Self {
        Step::By(by)
    }
}

/// How many items a key moves focus by, or the first and last. `None` for
/// keys the list doesn't handle, including arrows along the other axis. With
/// [`Arrows::Both`], every arrow works, like radios: Down and the inline end
/// go forward.
pub fn step_for(key: &Key, orientation: impl Into<Arrows>, rtl: bool) -> Option<Step> {
    let orientation = orientation.into();
    if orientation == Arrows::Both {
        return radio_step(key, rtl).map(Step::By).or(match key {
            Key::Home => Some(Step::First),
            Key::End => Some(Step::Last),
            _ => None,
        });
    }
    let (forward, backward) = match (orientation, rtl) {
        (Arrows::Vertical, _) => (Key::ArrowDown, Key::ArrowUp),
        (_, true) => (Key::ArrowLeft, Key::ArrowRight),
        (_, false) => (Key::ArrowRight, Key::ArrowLeft),
    };
    if *key == forward {
        return Some(Step::By(1));
    }
    if *key == backward {
        return Some(Step::By(-1));
    }
    match key {
        Key::Home => Some(Step::First),
        Key::End => Some(Step::Last),
        _ => None,
    }
}

/// The item a radio key moves to: Down and Right go forward, Up and Left
/// back, with Left and Right swapped right to left. Like native radios, both
/// axes work.
pub fn radio_step(key: &Key, rtl: bool) -> Option<isize> {
    let (end, start) = if rtl {
        (Key::ArrowLeft, Key::ArrowRight)
    } else {
        (Key::ArrowRight, Key::ArrowLeft)
    };
    if *key == Key::ArrowDown || *key == end {
        return Some(1);
    }
    if *key == Key::ArrowUp || *key == start {
        return Some(-1);
    }
    None
}

/// An item of a roving group, as the tab stop decision sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RovingCandidate<K> {
    pub key: K,
    /// Whether it can't be activated.
    pub disabled: bool,
    /// Whether it can take focus at all.
    pub focusable: bool,
    /// Whether it's selected, checked or pressed, so Tab lands on it first.
    pub preferred: bool,
}

/// The item that holds a group's one tab stop: the item focused last, when
/// the group remembers one, else the first preferred item that's enabled,
/// else the first enabled one, else the first that can take focus.
pub fn roving_stop<K: Clone + PartialEq>(
    items: &[RovingCandidate<K>],
    remembered: Option<&K>,
) -> Option<K> {
    let focusable: Vec<&RovingCandidate<K>> = items.iter().filter(|item| item.focusable).collect();
    if let Some(remembered) = remembered
        && focusable.iter().any(|item| &item.key == remembered)
    {
        return Some(remembered.clone());
    }
    let enabled: Vec<&&RovingCandidate<K>> =
        focusable.iter().filter(|item| !item.disabled).collect();
    enabled
        .iter()
        .find(|item| item.preferred)
        .or(enabled.first())
        .map(|item| item.key.clone())
        .or_else(|| focusable.first().map(|item| item.key.clone()))
}

/// A text field's caret, as [`field_keeps_key`] reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldCaret {
    /// Where the selection starts, or `None` for a field without a caret.
    pub start: Option<usize>,
    /// Where it ends.
    pub end: Option<usize>,
    /// How long the field's text is.
    pub length: usize,
    /// Whether the field runs right to left.
    pub rtl: bool,
    /// Whether the field uses Up and Down itself, like multi-line text and
    /// number fields.
    pub uses_up_down: bool,
}

/// Whether a text field keeps a key for its caret instead of letting a
/// roving group move focus. Left and Right leave the field only from the end
/// they point at, with nothing selected, so its text can still be edited.
/// Home and End always move the caret. Up and Down belong to fields that use
/// them: multi-line text and number fields.
pub fn field_keeps_key(key: &Key, field: &FieldCaret) -> bool {
    match key {
        Key::Home | Key::End => return true,
        Key::ArrowUp | Key::ArrowDown => return field.uses_up_down,
        Key::ArrowLeft | Key::ArrowRight => {}
        _ => return false,
    }
    // Fields without a caret, like checkboxes, never keep arrows.
    let (Some(start), Some(end)) = (field.start, field.end) else {
        return false;
    };
    if start != end {
        return true;
    }
    let backward = *key == if field.rtl { Key::ArrowRight } else { Key::ArrowLeft };
    if backward { start > 0 } else { end < field.length }
}

/// The index a step lands on, wrapping or stopping at the ends. `index` may
/// be `-1`, for a step from before the first item. A list with no items has
/// nowhere to land, so it gives 0.
pub fn move_index(index: isize, step: impl Into<Step>, count: usize, r#loop: bool) -> usize {
    match step.into() {
        Step::First => 0,
        Step::Last => count.saturating_sub(1),
        Step::By(by) => {
            if count == 0 {
                return 0;
            }
            let count = count as isize;
            let next = index + by;
            if r#loop {
                next.rem_euclid(count) as usize
            } else {
                next.clamp(0, count - 1) as usize
            }
        }
    }
}

/// Values sorted by where they appear in `order`, like a group's items in
/// page order, so a value reads the same however it was built. Values
/// missing from `order` keep their place at the end.
pub fn in_order<T: Clone + Eq + Hash>(values: &[T], order: &[T]) -> Vec<T> {
    let rank: HashMap<&T, usize> = order
        .iter()
        .enumerate()
        .map(|(index, value)| (value, index))
        .collect();
    let mut sorted = values.to_vec();
    sorted.sort_by_key(|value| rank.get(value).copied().unwrap_or(usize::MAX));
    sorted
}

/// The scroll position that brings a span into a viewport along one axis,
/// or the current one when it's already in view. The span stays `before` and
/// `after` clear of the edges, like scroll-padding. Works in `f32` or gpui's
/// `Pixels`.
pub fn scroll_to_show<T>(scroll: T, viewport: T, start: T, size: T, before: T, after: T) -> T
where
    T: Copy + PartialOrd + Default + Add<Output = T> + Sub<Output = T>,
{
    let zero = T::default();
    let max = |a: T, b: T| if a > b { a } else { b };
    if start - before < scroll {
        return max(zero, start - before);
    }
    if start + size + after > scroll + viewport {
        return max(zero, start + size + after - viewport);
    }
    scroll
}
