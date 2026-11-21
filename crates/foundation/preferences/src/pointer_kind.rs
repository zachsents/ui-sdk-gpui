//! Port of `foundation/preferences/pointer-kind.ts`.
//!
//! The device's pointer, which the original reads from the `pointer`,
//! `hover`, `any-pointer` and `any-hover` media features. gpui doesn't report
//! the device's pointers, so a pointer kind is what `initial` says, `Unknown`
//! for the rest, and doesn't change. An app that knows its device, like a
//! desktop app driven by a mouse, says so in `initial`.

/// How precise a pointer is: `Fine` for a mouse, trackpad or stylus,
/// `Coarse` for a finger, `None` without one, or `Unknown` when it isn't
/// known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PointerPrecision {
    Fine,
    Coarse,
    None,
    #[default]
    Unknown,
}

impl PointerPrecision {
    /// The original's name for it, like `"fine"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PointerPrecision::Fine => "fine",
            PointerPrecision::Coarse => "coarse",
            PointerPrecision::None => "none",
            PointerPrecision::Unknown => "unknown",
        }
    }
}

/// Whether a pointer can hover: `Hover`, `None`, or `Unknown` when it isn't
/// known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PointerHover {
    Hover,
    None,
    #[default]
    Unknown,
}

impl PointerHover {
    /// The original's name for it, like `"hover"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PointerHover::Hover => "hover",
            PointerHover::None => "none",
            PointerHover::Unknown => "unknown",
        }
    }
}

/// Everything a pointer kind knows, named after the media features the
/// original reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PointerKindState {
    /// The main pointer's precision: `Coarse` on a phone.
    pub pointer: PointerPrecision,
    /// Whether the main pointer can hover.
    pub hover: PointerHover,
    /// The most precise of all the pointers, like `Fine` for a tablet with a
    /// trackpad.
    pub any_pointer: PointerPrecision,
    /// Whether any of the pointers can hover.
    pub any_hover: PointerHover,
}

/// Options for [`PointerKind`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointerKindOptions {
    /// What the app knows about the device's pointers. Fields left `Unknown`
    /// stay unknown.
    pub initial: PointerKindState,
}

impl PointerKindOptions {
    pub fn initial(mut self, initial: PointerKindState) -> Self {
        self.initial = initial;
        self
    }
}

/// The device's pointer: whether it's a finger or a mouse, and whether it can
/// hover, for behavior like opening on tap where nothing hovers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerKind {
    state: PointerKindState,
}

impl PointerKind {
    pub fn new(options: PointerKindOptions) -> Self {
        Self {
            state: options.initial,
        }
    }

    /// What the pointer kind knows.
    pub fn get(&self) -> PointerKindState {
        self.state
    }
}
