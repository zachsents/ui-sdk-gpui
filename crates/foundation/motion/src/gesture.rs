//! Port of `foundation/motion/gesture.ts`: the gesture layer's public entry,
//! the physics under Drawer and Carousel, and which way a stroke points.
//! Your component still handles input, state and animation frames.

pub use crate::projection::project_distance;
pub use crate::rubber_band::{rubber_band, rubber_band_with};
pub use crate::sector::{SectorOptions, sector_of};
pub use crate::spring::{SpringSolver, solve_spring};
pub use crate::velocity::{
    VelocityTracker, VelocityTrackerOptions, create_velocity_tracker, timestamp,
};
