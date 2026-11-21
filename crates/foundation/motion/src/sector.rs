//! Port of `foundation/motion/sector.ts`.
//!
//! Which way a stroke points, as one of a few equal wedges around a center:
//! the choice in a radial or marking menu, a flick to pick a reaction, a
//! swipe that means "archive" one way and "snooze" another.

use crate::js;

/// Where `sector_of` puts its wedges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectorOptions {
    /// How far from the center, in px, a stroke starts pointing somewhere. Defaults to 18.
    pub dead_zone: f64,
    /// Where the first wedge starts, in turns clockwise from the top. Defaults to 0.
    pub start: f64,
}

impl Default for SectorOptions {
    fn default() -> Self {
        Self {
            dead_zone: 18.0,
            start: 0.0,
        }
    }
}

impl SectorOptions {
    pub fn dead_zone(mut self, dead_zone: f64) -> Self {
        self.dead_zone = dead_zone;
        self
    }

    pub fn start(mut self, start: f64) -> Self {
        self.start = start;
        self
    }
}

/// The wedge a stroke points into, from 0 to `count - 1`, clockwise from the
/// top (or from `start` turns), or `None` while it's within `dead_zone`
/// pixels of the center. Each wedge is centered on its direction: with four,
/// 0 is up, 1 right, 2 down, 3 left. Pure.
///
/// ```text
/// sector_of(30.0, -2.0, 4, SectorOptions::default()) // Some(1), right
/// sector_of(3.0, 4.0, 8, SectorOptions::default())   // None, too short to point anywhere
/// ```
pub fn sector_of(dx: f64, dy: f64, count: usize, options: SectorOptions) -> Option<usize> {
    if count < 1 || dx.hypot(dy) < options.dead_zone {
        return None;
    }
    let turn = (dx.atan2(-dy) / (2.0 * std::f64::consts::PI) - options.start + 2.0) % 1.0;
    let sector = js::round(turn * count as f64) % count as f64;
    // A `start` outside -1.5..1.5 turns can land below zero, which the
    // original returns as is and points nowhere.
    (sector >= 0.0).then_some(sector as usize)
}
