//! Port of `foundation/motion/physics.ts`.
//!
//! How a sheet moves, as one struct of numbers, so tuning it against a device
//! never touches code. `ios` is fitted to iOS 26.5 system sheets recorded
//! frame by frame in the simulator.

/// The numbers behind a sheet's motion. Every driver reads the same preset.
/// Start from [`SheetPhysics::IOS`] and change what differs:
/// `SheetPhysics::IOS.rubber_band(0.55)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetPhysics {
    /// Spring period in seconds, for releases without momentum and for programmatic moves.
    pub response: f64,
    /// Damping ratio for releases without momentum and programmatic moves. 1 never overshoots.
    pub damping_ratio: f64,
    /// Damping ratio once a release carries momentum, which lets it overshoot a little.
    pub momentum_damping_ratio: f64,
    /// Per-millisecond deceleration used to project where a fling would stop.
    pub projection_rate: f64,
    /// Below this speed, in px per ms, a release settles on the nearest snap point.
    pub min_fling_velocity: f64,
    /// Whether a fast fling can pass over snap points.
    pub skip_snap_points: bool,
    /// The rubber band's coefficient, past the largest snap point or a refused dismissal.
    pub rubber_band: f64,
    /// The most the rubber band can stretch, in px, or `None` for the container's size.
    pub rubber_band_dimension: Option<f64>,
    /// Travel, in px, before a touch or pen drag moves the sheet.
    pub touch_slop: f64,
    /// Travel, in px, before a mouse drag moves the sheet.
    pub mouse_slop: f64,
    /// How dark `themeColor` makes the browser's theme color, as the opacity of black.
    pub dim: f64,
    /// How far back, in ms, the release velocity looks at input samples.
    pub velocity_horizon: f64,
}

impl SheetPhysics {
    /// The iOS preset, the default everywhere.
    pub const IOS: SheetPhysics = SheetPhysics {
        response: 0.34,
        damping_ratio: 1.0,
        momentum_damping_ratio: 0.88,
        projection_rate: 0.99,
        min_fling_velocity: 0.5,
        skip_snap_points: true,
        // UIKit's sheet band stops near 23pt: d 24, c 1.9 fit the recording.
        rubber_band: 1.9,
        rubber_band_dimension: Some(24.0),
        touch_slop: 8.0,
        mouse_slop: 4.0,
        dim: 0.2,
        velocity_horizon: 70.0,
    };

    pub fn response(mut self, response: f64) -> Self {
        self.response = response;
        self
    }

    pub fn damping_ratio(mut self, ratio: f64) -> Self {
        self.damping_ratio = ratio;
        self
    }

    pub fn momentum_damping_ratio(mut self, ratio: f64) -> Self {
        self.momentum_damping_ratio = ratio;
        self
    }

    pub fn projection_rate(mut self, rate: f64) -> Self {
        self.projection_rate = rate;
        self
    }

    pub fn min_fling_velocity(mut self, velocity: f64) -> Self {
        self.min_fling_velocity = velocity;
        self
    }

    pub fn skip_snap_points(mut self, skip: bool) -> Self {
        self.skip_snap_points = skip;
        self
    }

    pub fn rubber_band(mut self, coefficient: f64) -> Self {
        self.rubber_band = coefficient;
        self
    }

    pub fn rubber_band_dimension(mut self, dimension: Option<f64>) -> Self {
        self.rubber_band_dimension = dimension;
        self
    }

    pub fn touch_slop(mut self, slop: f64) -> Self {
        self.touch_slop = slop;
        self
    }

    pub fn mouse_slop(mut self, slop: f64) -> Self {
        self.mouse_slop = slop;
        self
    }

    pub fn dim(mut self, dim: f64) -> Self {
        self.dim = dim;
        self
    }

    pub fn velocity_horizon(mut self, horizon: f64) -> Self {
        self.velocity_horizon = horizon;
        self
    }
}

impl Default for SheetPhysics {
    fn default() -> Self {
        Self::IOS
    }
}

/// The physics presets, by name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetPhysicsPresets {
    pub ios: SheetPhysics,
}

/// Physics presets. `ios` is the default everywhere.
pub const SHEET_PHYSICS: SheetPhysicsPresets = SheetPhysicsPresets {
    ios: SheetPhysics::IOS,
};
