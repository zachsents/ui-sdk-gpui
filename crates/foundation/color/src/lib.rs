//! Port of `foundation/color`: CSS colors as plain data. Read any CSS color,
//! convert it between spaces, write it back as CSS, fit it to a screen's
//! gamut, mix and compare two colors, for any color component of your own.
//!
//! `color.ts` and `index.ts` both re-export the files' API; this crate root
//! does for both. The original marks the whole API experimental.
//!
//! # gpui
//!
//! The original hands colors to the page as CSS text. Here a [`Color`] turns
//! into what gpui paints with: [`Color::to_rgba`], [`Color::to_hsla`], and
//! `From<Color>` for `Rgba`, `Hsla`, `Background` and `Fill`, so
//! `.bg(color)` and `.text_color(color)` take one. gpui paints in sRGB, so a
//! wider color is clipped, as `rgb()` writes one: [`to_gamut`] first keeps its
//! lightness and hue. `From<Rgba>` and `From<Hsla>` read gpui's colors back.
//!
//! # Deviations
//!
//! - Default arguments become explicit ones: [`is_in_gamut`] and [`to_gamut`]
//!   take a [`ColorGamut`] (sRGB is [`ColorGamut::default`]), [`mix_color`]
//!   takes its amount (the original's default is 0.5), and [`format_color`]
//!   takes `None` for the color's own format and [`ColorFormatOptions`],
//!   whose `Default` is the original's precision of 5.
//! - Spaces, formats, gamuts and hue methods are enums, with `as_str` for the
//!   original's names; spaces and formats also parse from them.
//! - The math runs in `f64` with the original's constants and order of
//!   operations, and JavaScript's `Math.hypot`, `Math.max`, `Math.min`,
//!   `Math.round` and `toFixed` where Rust's own differ, so the text
//!   [`format_color`] writes matches the original's.

pub mod compare;
pub mod convert;
pub mod format;
pub mod gamut;
pub mod mix;
mod names;
pub mod parse;

pub use compare::{color_distance, contrast_ratio};
pub use convert::{Color, ColorSpace, UnknownColorSpace, convert_color};
pub use format::{ColorFormat, ColorFormatOptions, UnknownColorFormat, format_color};
pub use gamut::{ColorGamut, is_in_gamut, to_gamut};
pub use mix::{ColorHueInterpolation, ColorMixOptions, mix_color};
pub use parse::parse_color;
