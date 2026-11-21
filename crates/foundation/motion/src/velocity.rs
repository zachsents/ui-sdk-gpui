//! Port of `foundation/motion/velocity.ts`.
//!
//! Release velocity from input samples: least squares over recent event
//! timestamps, degree 2 (Android's `VelocityTracker` and Flutter default to
//! it), degree 1 with fewer samples. Times are in ms and velocities in px/ms.
//!
//! Degree 2 needs four samples here, not three: three samples fit exactly, and
//! a hard deceleration between them (a coarse mouse drag, a dropped frame)
//! can flip the end slope's sign and turn a fling up into a dismissal.
//!
//! The horizon is 70ms, not 100ms: a slow drag that ends in a 50ms flick to
//! 2px/ms reads 1.43px/ms over 100ms at 120Hz, because a quadratic can't
//! follow the kink, and 1.9px/ms over 70ms.
//!
//! gpui's input events carry no time, so drivers stamp samples with
//! [`timestamp`] as they handle them, where the original reads an event's
//! `timeStamp`.

use std::sync::OnceLock;
use std::time::Instant;

use crate::js;

/// Milliseconds on the input clock that samples, sheet events and settles
/// share, like an event's `timeStamp`: the time since the clock was first
/// read.
pub fn timestamp() -> f64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// How a [`VelocityTracker`] reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VelocityTrackerOptions {
    /// How far back samples count, in ms before the newest. Defaults to 70.
    pub horizon: f64,
    /// How old the newest sample can be, in ms, before the pointer counts as stopped and reads 0. Defaults to 40.
    pub stale_after: f64,
    /// The fastest the tracker reads, in px/ms either way. Defaults to 8.
    pub max_speed: f64,
    /// How many samples it keeps. Defaults to 20, enough for the horizon at 240 Hz.
    pub capacity: usize,
}

impl Default for VelocityTrackerOptions {
    fn default() -> Self {
        Self {
            horizon: 70.0,
            stale_after: 40.0,
            max_speed: 8.0,
            capacity: 20,
        }
    }
}

impl VelocityTrackerOptions {
    pub fn horizon(mut self, horizon: f64) -> Self {
        self.horizon = horizon;
        self
    }

    pub fn stale_after(mut self, stale_after: f64) -> Self {
        self.stale_after = stale_after;
        self
    }

    pub fn max_speed(mut self, max_speed: f64) -> Self {
        self.max_speed = max_speed;
        self
    }

    pub fn capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }
}

/// Fits the samples and returns the slope at the newest one. Times are taken
/// relative to the newest sample, so the fitted derivative at 0 is the linear
/// coefficient. `times` must be ascending.
pub fn least_squares_velocity(times: &[f64], positions: &[f64]) -> f64 {
    let count = times.len().min(positions.len());
    let (times, positions) = (&times[..count], &positions[..count]);
    if count < 2 {
        return 0.0;
    }
    let newest = times[count - 1];
    if count < 4 {
        return linear_slope(times, positions);
    }
    // Normal equations for x = a + b·t + c·t², with sums of powers of t.
    let (mut s0, mut s1, mut s2, mut s3, mut s4) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut x0, mut x1, mut x2) = (0.0, 0.0, 0.0);
    for index in 0..count {
        let t = times[index] - newest;
        let x = positions[index];
        let t2 = t * t;
        s0 += 1.0;
        s1 += t;
        s2 += t2;
        s3 += t2 * t;
        s4 += t2 * t2;
        x0 += x;
        x1 += x * t;
        x2 += x * t2;
    }
    // Cramer's rule for the b coefficient of the 3×3 system.
    let determinant =
        s0 * (s2 * s4 - s3 * s3) - s1 * (s1 * s4 - s3 * s2) + s2 * (s1 * s3 - s2 * s2);
    if determinant.abs() < 1e-9 {
        return linear_slope(times, positions);
    }
    let numerator = s0 * (x1 * s4 - s3 * x2) - x0 * (s1 * s4 - s3 * s2) + s2 * (s1 * x2 - x1 * s2);
    numerator / determinant
}

fn linear_slope(times: &[f64], positions: &[f64]) -> f64 {
    let count = times.len() as f64;
    let (mut mean_time, mut mean_position) = (0.0, 0.0);
    for (time, position) in times.iter().zip(positions) {
        mean_time += time / count;
        mean_position += position / count;
    }
    let (mut covariance, mut variance) = (0.0, 0.0);
    for (time, position) in times.iter().zip(positions) {
        let dt = time - mean_time;
        covariance += dt * (position - mean_position);
        variance += dt * dt;
    }
    if variance > 0.0 {
        covariance / variance
    } else {
        0.0
    }
}

/// Release velocity along one axis, the way Drawer and Carousel measure a
/// flick: `add(time, position)` on each move, then `velocity(now)` on
/// release, in px/ms. A pointer that stopped before letting go reads 0. Track
/// x and y with one tracker each.
///
/// A fixed ring of samples, so pointer moves allocate nothing. `velocity`
/// copies the in-horizon window into scratch buffers in time order and fits
/// it.
#[derive(Clone, Debug)]
pub struct VelocityTracker {
    horizon: f64,
    stale_after: f64,
    max_speed: f64,
    capacity: usize,
    times: Vec<f64>,
    positions: Vec<f64>,
    window_times: Vec<f64>,
    window_positions: Vec<f64>,
    count: usize,
    // The newest sample's slot. One before slot 0 while the ring is empty, so
    // the first sample lands there, as the original's -1 does.
    newest_index: usize,
}

impl VelocityTracker {
    pub fn new(options: VelocityTrackerOptions) -> Self {
        let capacity = options.capacity;
        Self {
            horizon: options.horizon,
            stale_after: options.stale_after,
            max_speed: options.max_speed,
            capacity,
            times: vec![0.0; capacity],
            positions: vec![0.0; capacity],
            window_times: vec![0.0; capacity],
            window_positions: vec![0.0; capacity],
            count: 0,
            newest_index: capacity.saturating_sub(1),
        }
    }

    /// Adds a sample: a pointer's position along the axis at `time`, like
    /// [`timestamp`] as a move is handled. A sample no newer than the last is
    /// ignored.
    pub fn add(&mut self, time: f64, position: f64) {
        if self.capacity == 0 {
            return;
        }
        if self.count > 0 && time <= self.times[self.newest_index] {
            return;
        }
        self.newest_index = (self.newest_index + 1) % self.capacity;
        self.times[self.newest_index] = time;
        self.positions[self.newest_index] = position;
        self.count = (self.count + 1).min(self.capacity);
    }

    /// The speed at `now` from the recent samples, in px/ms, or 0 when the
    /// newest is older than `stale_after`.
    pub fn velocity(&mut self, now: f64) -> f64 {
        if self.count < 2 {
            return 0.0;
        }
        let newest = self.times[self.newest_index];
        if now - newest > self.stale_after {
            return 0.0;
        }
        let mut window_count = 0;
        for age in (0..self.count).rev() {
            let index = (self.newest_index + self.capacity - age) % self.capacity;
            if newest - self.times[index] > self.horizon {
                continue;
            }
            self.window_times[window_count] = self.times[index];
            self.window_positions[window_count] = self.positions[index];
            window_count += 1;
        }
        let fitted = least_squares_velocity(
            &self.window_times[..window_count],
            &self.window_positions[..window_count],
        );
        js::max(-self.max_speed, js::min(self.max_speed, fitted))
    }

    /// Forgets every sample, so the next gesture starts fresh.
    pub fn reset(&mut self) {
        self.count = 0;
        self.newest_index = self.capacity.saturating_sub(1);
    }
}

impl Default for VelocityTracker {
    fn default() -> Self {
        Self::new(VelocityTrackerOptions::default())
    }
}

/// A [`VelocityTracker`] with these options (`createVelocityTracker`).
pub fn create_velocity_tracker(options: VelocityTrackerOptions) -> VelocityTracker {
    VelocityTracker::new(options)
}
