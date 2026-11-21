//! Port of `components/carousel/carousel-dom.ts`: what a carousel reads from
//! and writes to its slides: their layout, their "1 of 5" labels, and the
//! order that rotates a looping track. The decisions are in
//! `carousel_layout`.
//!
//! The original reads the DOM. Here each part records its bounds as it lays
//! out ([`CarouselProbes`]), less the shifts the carousel applied that frame,
//! so what's read is where the part sits in the content's own layout, as a
//! DOM offset is. gpui has no CSS `order`, so a looping track's rotation is
//! a shift on each slide, from where the content lays it out to where the
//! rotation puts it, which leaves the slides in their own order in the tree.

use std::collections::HashMap;

use gpui::{Bounds, Hitbox, Pixels, SharedString};

use crate::carousel_layout::{CarouselSlideBox, measure_track, place_slides};
use crate::carousel_motion::CarouselAxis;

/// What the carousel's parts report as they draw: the stand-in for reading
/// the DOM. Shared between the parts' probes and the carousel.
#[derive(Default)]
pub struct CarouselProbes {
    /// The viewport's bounds.
    pub(crate) viewport: Option<Bounds<Pixels>>,
    /// The content's bounds where it lays out, without the scroll's shift.
    pub(crate) content: Option<Bounds<Pixels>>,
    /// Each slide's bounds where the content lays it out, without the
    /// scroll's and the rotation's shifts.
    pub(crate) items: HashMap<SharedString, Bounds<Pixels>>,
    /// The slides and indicators rendered this frame, in page order.
    pub(crate) item_order: Vec<SharedString>,
    pub(crate) indicator_order: Vec<SharedString>,
    /// The root's hitbox, which tells whether the mouse is over it.
    pub(crate) hitbox: Option<Hitbox>,
    /// Whether the root showed in the window, inside every clip around it.
    pub(crate) on_screen: Option<bool>,
}

impl CarouselProbes {
    /// Records a part rendered this frame, once, in page order.
    pub(crate) fn rendered(order: &mut Vec<SharedString>, key: &SharedString) {
        if !order.contains(key) {
            order.push(key.clone());
        }
    }
}

/// What a frame laid out: the viewport, the content and each slide, in the
/// content's own layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaidOut {
    pub viewport: Bounds<Pixels>,
    pub content: Option<Bounds<Pixels>>,
    pub slides: Vec<(SharedString, Bounds<Pixels>)>,
}

impl LaidOut {
    /// Whether `other` lays out the same, within a pixel, so the rounding of
    /// a shift that changes every frame never reads as a resize.
    pub fn same_as(&self, other: &LaidOut) -> bool {
        fn near(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
            let close = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() < 1.0;
            close(a.origin.x, b.origin.x)
                && close(a.origin.y, b.origin.y)
                && close(a.size.width, b.size.width)
                && close(a.size.height, b.size.height)
        }
        near(self.viewport, other.viewport)
            && match (self.content, other.content) {
                (Some(a), Some(b)) => near(a, b),
                (None, None) => true,
                _ => false,
            }
            && self.slides.len() == other.slides.len()
            && self
                .slides
                .iter()
                .zip(&other.slides)
                .all(|((a_key, a), (b_key, b))| a_key == b_key && near(*a, *b))
    }
}

/// A measured layout along the axis: each slide's key and place, the
/// viewport's size, the largest scroll position and the scroll padding.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CarouselLayoutRead {
    pub slides: Vec<(SharedString, CarouselSlideBox)>,
    pub viewport_size: f64,
    pub max_scroll: f64,
    pub padding_start: f64,
    pub padding_end: f64,
}

fn f(pixels: Pixels) -> f64 {
    f64::from(f32::from(pixels))
}

/// Reads each slide's place along the axis, and the viewport's size and
/// range, from where the parts laid out. Right to left, places count from
/// the right. gpui has no scroll padding, so it's 0.
pub fn read_carousel_layout(laid_out: &LaidOut, axis: CarouselAxis) -> CarouselLayoutRead {
    let CarouselAxis { vertical, rtl } = axis;
    let viewport = laid_out.viewport;
    let size = if vertical {
        f(viewport.size.height)
    } else {
        f(viewport.size.width)
    };
    let slides: Vec<(SharedString, CarouselSlideBox)> = laid_out
        .slides
        .iter()
        .map(|(key, bounds)| {
            let left = f(bounds.left() - viewport.left());
            let top = f(bounds.top() - viewport.top());
            let width = f(bounds.size.width);
            let slide = if vertical {
                CarouselSlideBox {
                    start: top,
                    size: f(bounds.size.height),
                }
            } else {
                CarouselSlideBox {
                    start: if rtl { size - left - width } else { left },
                    size: width,
                }
            };
            (key.clone(), slide)
        })
        .collect();
    // The scrollable length: to the last slide's end, and the viewport's
    // padding after it.
    let padding_after = laid_out.content.map_or(0.0, |content| {
        f(if vertical {
            viewport.bottom() - content.bottom()
        } else if rtl {
            content.left() - viewport.left()
        } else {
            viewport.right() - content.right()
        })
        .max(0.0)
    });
    let end = slides
        .iter()
        .map(|(_, slide)| slide.start + slide.size)
        .fold(0.0, f64::max);
    CarouselLayoutRead {
        max_scroll: (end + padding_after - size).max(0.0),
        viewport_size: size,
        slides,
        padding_start: 0.0,
        padding_end: 0.0,
    }
}

/// Where the slides sit with slide `first` shown first and the rest after it
/// in their own order, as the original's CSS `order` lays them out, when the
/// content lays them out as an even row; as they are otherwise, as content
/// that doesn't follow `order` does.
pub fn write_slide_order(slides: &[CarouselSlideBox], first: usize) -> Vec<CarouselSlideBox> {
    match measure_track(slides, 0) {
        Some(track) if first > 0 => place_slides(&track, first),
        _ => slides.to_vec(),
    }
}

/// How far each slide shifts along the axis from where the content lays it
/// out to where a rotation puts it, by key.
pub fn slide_shifts(
    keys: &[SharedString],
    laid: &[CarouselSlideBox],
    ordered: &[CarouselSlideBox],
) -> HashMap<SharedString, f64> {
    keys.iter()
        .zip(laid.iter().zip(ordered))
        .filter_map(|(key, (laid, ordered))| {
            let shift = ordered.start - laid.start;
            (shift.abs() >= 0.01).then(|| (key.clone(), shift))
        })
        .collect()
}

/// A slide's label, "1 of 5", unless the app names it. Its own `aria_label`
/// comes after this one and wins, which is also how it's translated.
pub fn label_slide(index: usize, count: usize) -> SharedString {
    format!("{} of {}", index + 1, count).into()
}
