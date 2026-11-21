//! Port of `foundation/motion/spring.ts`.
//!
//! An analytic damped spring, parameterized like SwiftUI: `response` (the
//! period of the undamped oscillation, in seconds) and `damping_ratio` (1 is
//! critical). Positions are in px, times in ms and velocities in px/ms, so one
//! solver serves every driver.
//!
//! gpui's own `SpringConfig` is parameterized by stiffness, damping and mass
//! in `f32`, with its own critical tolerance and settle-time bound, so this
//! keeps the original's math in `f64` and drivers sample it themselves.

use std::fmt;

use crate::js;

// Solves x'' = −ω²x − 2ζωx' for a start displacement (position − target) and
// velocity, and keeps the coefficients, so sampling it every frame allocates
// nothing.
/// A damped spring from `displacement` px away from its target with
/// `velocity` px/ms: `position(t)` and `velocity(t)` at `t` ms, so a frame
/// loop samples it with no allocation. `response` is the period in seconds,
/// `damping_ratio` 1 for no bounce, below 1 to bounce.
pub fn solve_spring(
    response: f64,
    damping_ratio: f64,
    displacement: f64,
    velocity: f64,
) -> SpringSolver {
    let omega = (2.0 * std::f64::consts::PI) / (response * 1000.0);
    let zeta = damping_ratio;
    if (zeta - 1.0).abs() < 1e-6 {
        return critically_damped(omega, displacement, velocity);
    }
    if zeta < 1.0 {
        return underdamped(omega, zeta, displacement, velocity);
    }
    overdamped(omega, zeta, displacement, velocity)
}

/// What `solve_spring` returns: the spring's displacement from its target
/// and its velocity at any time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringSolver {
    kind: SpringKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SpringKind {
    Underdamped {
        decay: f64,
        damped: f64,
        x0: f64,
        b: f64,
    },
    CriticallyDamped {
        omega: f64,
        x0: f64,
        b: f64,
    },
    Overdamped {
        slow: f64,
        fast: f64,
        slow_weight: f64,
        fast_weight: f64,
    },
}

impl SpringSolver {
    /// The displacement from the target, in px, `t` ms after the start.
    pub fn position(&self, t: f64) -> f64 {
        match self.kind {
            SpringKind::Underdamped { decay, damped, x0, b } => {
                (-decay * t).exp() * (x0 * (damped * t).cos() + b * (damped * t).sin())
            }
            SpringKind::CriticallyDamped { omega, x0, b } => (-omega * t).exp() * (x0 + b * t),
            SpringKind::Overdamped {
                slow,
                fast,
                slow_weight,
                fast_weight,
            } => slow_weight * (slow * t).exp() + fast_weight * (fast * t).exp(),
        }
    }

    /// The velocity, in px/ms, `t` ms after the start.
    pub fn velocity(&self, t: f64) -> f64 {
        match self.kind {
            SpringKind::Underdamped { decay, damped, x0, b } => {
                let cos = (damped * t).cos();
                let sin = (damped * t).sin();
                (-decay * t).exp() * ((b * damped - decay * x0) * cos - (x0 * damped + decay * b) * sin)
            }
            SpringKind::CriticallyDamped { omega, x0, b } => {
                (-omega * t).exp() * (b - omega * (x0 + b * t))
            }
            SpringKind::Overdamped {
                slow,
                fast,
                slow_weight,
                fast_weight,
            } => slow_weight * slow * (slow * t).exp() + fast_weight * fast * (fast * t).exp(),
        }
    }
}

fn underdamped(omega: f64, zeta: f64, x0: f64, v0: f64) -> SpringSolver {
    let decay = zeta * omega;
    let damped = omega * (1.0 - zeta * zeta).sqrt();
    let b = (v0 + decay * x0) / damped;
    SpringSolver {
        kind: SpringKind::Underdamped { decay, damped, x0, b },
    }
}

fn critically_damped(omega: f64, x0: f64, v0: f64) -> SpringSolver {
    let b = v0 + omega * x0;
    SpringSolver {
        kind: SpringKind::CriticallyDamped { omega, x0, b },
    }
}

fn overdamped(omega: f64, zeta: f64, x0: f64, v0: f64) -> SpringSolver {
    let root = (zeta * zeta - 1.0).sqrt();
    let fast = -omega * (zeta + root);
    let slow = -omega * (zeta - root);
    let slow_weight = (v0 - fast * x0) / (slow - fast);
    let fast_weight = x0 - slow_weight;
    SpringSolver {
        kind: SpringKind::Overdamped {
            slow,
            fast,
            slow_weight,
            fast_weight,
        },
    }
}

/// What `settle_time` checks the spring against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettleTimeOptions {
    /// How near the target counts as there, in px. Defaults to 0.5.
    pub position_tolerance: f64,
    /// How slow counts as stopped, in px/ms. Defaults to 0.02.
    pub velocity_tolerance: f64,
    /// How often it samples, in ms. Defaults to 1.
    pub step: f64,
    /// The longest it looks, in ms. Defaults to 10 000.
    pub limit: f64,
}

impl Default for SettleTimeOptions {
    fn default() -> Self {
        Self {
            position_tolerance: 0.5,
            velocity_tolerance: 0.02,
            step: 1.0,
            limit: 10_000.0,
        }
    }
}

impl SettleTimeOptions {
    pub fn position_tolerance(mut self, tolerance: f64) -> Self {
        self.position_tolerance = tolerance;
        self
    }

    pub fn velocity_tolerance(mut self, tolerance: f64) -> Self {
        self.velocity_tolerance = tolerance;
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    pub fn limit(mut self, limit: f64) -> Self {
        self.limit = limit;
        self
    }
}

/// The first time, in ms, when the spring is within `position_tolerance` px
/// of rest and slower than `velocity_tolerance` px/ms, or `limit` when it
/// never is. Checking both skips the zero crossings of an underdamped spring,
/// where it passes the target at speed.
pub fn settle_time(solver: &SpringSolver, options: SettleTimeOptions) -> f64 {
    let SettleTimeOptions {
        position_tolerance,
        velocity_tolerance,
        step,
        limit,
    } = options;
    let mut t = 0.0;
    while t <= limit {
        if solver.position(t).abs() <= position_tolerance
            && solver.velocity(t).abs() <= velocity_tolerance
        {
            return t;
        }
        // A step that doesn't move forward would sample t = 0 forever.
        if !(step > 0.0) {
            break;
        }
        t += step;
    }
    limit
}

/// How often `spring_linear_easing` samples by default, in ms: 120 times a
/// second.
pub const SPRING_EASING_STEP: f64 = 1000.0 / 120.0;

/// A CSS `linear()` easing that replays the spring from `from` to `to`,
/// sampled every [`SPRING_EASING_STEP`] ms, for an animation of `duration`
/// ms. Progress overshoots past 1 when the spring bounces, which `linear()`
/// allows.
pub fn spring_linear_easing(
    solver: &SpringSolver,
    from: f64,
    to: f64,
    duration: f64,
) -> LinearEasing {
    spring_linear_easing_with_step(solver, from, to, duration, SPRING_EASING_STEP)
}

/// `spring_linear_easing`, sampled every `step` ms.
pub fn spring_linear_easing_with_step(
    solver: &SpringSolver,
    from: f64,
    to: f64,
    duration: f64,
    step: f64,
) -> LinearEasing {
    let distance = to - from;
    // No distance, no progress to ease. Callers sample positions instead.
    if distance.abs() < 1e-6 {
        return LinearEasing::Linear;
    }
    let mut stops = Vec::new();
    let mut t = 0.0;
    while t < duration {
        stops.push(round(1.0 + solver.position(t) / distance));
        if !(step > 0.0) {
            break;
        }
        t += step;
    }
    stops.push(1.0);
    LinearEasing::Stops(stops)
}

fn round(value: f64) -> f64 {
    js::round(value * 10_000.0) / 10_000.0
}

/// An easing CSS writes as `linear(…)`: output progress at stops spread
/// evenly over the input, straight lines between them. `Display` writes it
/// as CSS does; `sample` reads it, as an easing for gpui's animations.
#[derive(Clone, Debug, PartialEq)]
pub enum LinearEasing {
    /// `linear`: output is input.
    Linear,
    /// `linear(a, b, …)`, rounded to four decimals.
    Stops(Vec<f64>),
}

impl LinearEasing {
    /// The output progress at input `progress`, from 0 to 1.
    pub fn sample(&self, progress: f64) -> f64 {
        let stops = match self {
            LinearEasing::Linear => return progress,
            LinearEasing::Stops(stops) => stops,
        };
        match stops.len() {
            0 => progress,
            1 => stops[0],
            count => {
                let position = progress.clamp(0.0, 1.0) * (count - 1) as f64;
                let index = (position.floor() as usize).min(count - 2);
                let fraction = position - index as f64;
                stops[index] + (stops[index + 1] - stops[index]) * fraction
            }
        }
    }
}

impl fmt::Display for LinearEasing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinearEasing::Linear => f.write_str("linear"),
            LinearEasing::Stops(stops) => {
                f.write_str("linear(")?;
                for (index, stop) in stops.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    f.write_str(&js::number_string(*stop))?;
                }
                f.write_str(")")
            }
        }
    }
}
