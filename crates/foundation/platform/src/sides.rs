//! Port of `foundation/platform/sides.ts`: the sides a popup opens on.

/// A side of the anchor. `Start` and `End` are logical: left and right,
/// swapped in right-to-left layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
    Start,
    End,
}

impl Side {
    /// The original's name for it, like `"bottom"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Top => "top",
            Side::Bottom => "bottom",
            Side::Left => "left",
            Side::Right => "right",
            Side::Start => "start",
            Side::End => "end",
        }
    }

    /// Whether it's the top or the bottom.
    pub fn is_vertical(self) -> bool {
        matches!(self, Side::Top | Side::Bottom)
    }
}

/// A side on screen, after logical sides are resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PhysicalSide {
    Top,
    Bottom,
    Left,
    Right,
}

impl PhysicalSide {
    /// The original's name for it, its `data-side`, like `"bottom"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PhysicalSide::Top => "top",
            PhysicalSide::Bottom => "bottom",
            PhysicalSide::Left => "left",
            PhysicalSide::Right => "right",
        }
    }

    /// Whether it's the top or the bottom.
    pub fn is_vertical(self) -> bool {
        matches!(self, PhysicalSide::Top | PhysicalSide::Bottom)
    }
}

impl From<PhysicalSide> for Side {
    fn from(side: PhysicalSide) -> Self {
        match side {
            PhysicalSide::Top => Side::Top,
            PhysicalSide::Bottom => Side::Bottom,
            PhysicalSide::Left => Side::Left,
            PhysicalSide::Right => Side::Right,
        }
    }
}

/// `start` and `end` on screen.
pub fn physical_side(side: Side, rtl: bool) -> PhysicalSide {
    match side {
        Side::Start => {
            if rtl {
                PhysicalSide::Right
            } else {
                PhysicalSide::Left
            }
        }
        Side::End => {
            if rtl {
                PhysicalSide::Left
            } else {
                PhysicalSide::Right
            }
        }
        Side::Top => PhysicalSide::Top,
        Side::Bottom => PhysicalSide::Bottom,
        Side::Left => PhysicalSide::Left,
        Side::Right => PhysicalSide::Right,
    }
}

/// The side across from `side`.
pub fn opposite(side: PhysicalSide) -> PhysicalSide {
    match side {
        PhysicalSide::Top => PhysicalSide::Bottom,
        PhysicalSide::Bottom => PhysicalSide::Top,
        PhysicalSide::Left => PhysicalSide::Right,
        PhysicalSide::Right => PhysicalSide::Left,
    }
}
