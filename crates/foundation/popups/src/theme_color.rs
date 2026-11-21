//! Port of `foundation/popups/theme-color.ts`: dims the window's chrome
//! color with a sheet's backdrop, so a status bar or title bar darkens with
//! the window, as it does around a native sheet.
//!
//! The original rewrites `<meta name="theme-color">`, which Safari tints its
//! toolbars from. A gpui window has no such tag: the app hands the colors it
//! paints its chrome with, and applies what [`ThemeColor::write`] returns.
//! Tinting is costly on some platforms, so callers write only where motion
//! starts and ends, never per frame.

use std::cell::Cell;

use gpui::Rgba;

/// The chrome colors as a sheet's dim darkens them.
#[derive(Clone, Debug)]
pub struct ThemeColor {
    originals: Vec<Rgba>,
    alpha: f64,
    written: Cell<f64>,
}

/// Dims `colors` toward black by `dim × alpha`, where `alpha` is how dark
/// the backdrop gets (`dimThemeColor`). An app with no chrome colors passes
/// none, and nothing changes.
pub fn dim_theme_color(colors: Vec<Rgba>, alpha: f64) -> ThemeColor {
    ThemeColor {
        originals: colors,
        alpha,
        written: Cell::new(-1.0),
    }
}

impl ThemeColor {
    /// The colors for a dim amount, from 0 to 1, or `None` when they're what
    /// was written last.
    pub fn write(&self, dim: f64) -> Option<Vec<Rgba>> {
        let shade = (dim * self.alpha * 100.0).round() / 100.0;
        if shade == self.written.get() {
            return None;
        }
        self.written.set(shade);
        if shade == 0.0 {
            return Some(self.originals.clone());
        }
        let channel = |value: f32| ((f64::from(value) * 255.0 * (1.0 - shade)).round() / 255.0) as f32;
        Some(
            self.originals
                .iter()
                .map(|color| Rgba {
                    r: channel(color.r),
                    g: channel(color.g),
                    b: channel(color.b),
                    a: 1.0,
                })
                .collect(),
        )
    }

    /// The colors as they were.
    pub fn restore(&self) -> Vec<Rgba> {
        self.written.set(-1.0);
        self.originals.clone()
    }
}
