//! Port of `foundation/popups/sheet-axis.ts`. A sheet moves along one axis,
//! toward or away from the edge it's attached to. The machine only knows
//! extents and a finger position that grows toward that edge; these helpers
//! translate between it and the window for each side.
//!
//! The original finds the scroll area under a finger by walking up from the
//! press's target. gpui can't list what's under a point, so the component
//! names its sheet's scroll areas, each a [`ScrollArea`]: the
//! [`ScrollHandle`] its element tracks and the [`Region`] it measures.

use gpui::{Bounds, Pixels, Point, ScrollHandle, Window, point, px};
use ui_sdk_motion::ScrollContext;
use ui_sdk_platform::{PhysicalSide, Region, Side, physical_side};

/// The edge a sheet comes from (`SheetSide`). `Start` and `End` follow the
/// writing direction, like a popup's `side`.
pub type SheetSide = Side;

/// The edge on screen: `Start` and `End` follow the app's direction.
pub fn resolve_side(side: SheetSide, rtl: bool) -> PhysicalSide {
    physical_side(side, matches!(side, Side::Start | Side::End) && rtl)
}

pub fn is_vertical(side: PhysicalSide) -> bool {
    matches!(side, PhysicalSide::Bottom | PhysicalSide::Top)
}

/// 1 when the edge is where positions grow, the bottom or the right, else -1.
pub fn toward_edge(side: PhysicalSide) -> f64 {
    if matches!(side, PhysicalSide::Bottom | PhysicalSide::Right) {
        1.0
    } else {
        -1.0
    }
}

/// A pointer position as the machine reads it: `y` along the axis, growing
/// toward the edge, and `x` across it (`axisPoint`).
pub fn axis_point(side: PhysicalSide, position: Point<Pixels>) -> (f64, f64) {
    let (x, y) = (f64::from(f32::from(position.x)), f64::from(f32::from(position.y)));
    if is_vertical(side) {
        (x, y * toward_edge(side))
    } else {
        (y, x * toward_edge(side))
    }
}

/// The offset that moves a sheet `offset` px toward its edge from where it's
/// laid out (`translateValue`).
pub fn translate_value(side: PhysicalSide, offset: f64) -> Point<Pixels> {
    let value = px(((offset * toward_edge(side) * 100.0).round() / 100.0) as f32);
    if is_vertical(side) {
        point(px(0.), value)
    } else {
        point(value, px(0.))
    }
}

/// The sheet's own size along its axis, as laid out (`sizeAlong`).
pub fn size_along(bounds: Bounds<Pixels>, side: PhysicalSide) -> f64 {
    let size = if is_vertical(side) {
        bounds.size.height
    } else {
        bounds.size.width
    };
    f64::from(f32::from(size))
}

/// A scroll area inside a sheet: the scroll handle its element tracks and
/// the region it measures.
#[derive(Clone, Debug)]
pub struct ScrollArea {
    pub handle: ScrollHandle,
    pub region: Region,
}

impl PartialEq for ScrollArea {
    fn eq(&self, other: &Self) -> bool {
        self.region == other.region
    }
}

fn scrollable(area: &ScrollArea, side: PhysicalSide) -> bool {
    let max = area.handle.max_offset();
    if is_vertical(side) {
        max.y > px(1.)
    } else {
        max.x > px(1.)
    }
}

/// The innermost of `areas` under `position` that scrolls along the axis
/// (`scrollAreaAt`).
pub fn scroll_area_at(position: Point<Pixels>, areas: &[ScrollArea], side: PhysicalSide, window: &Window) -> Option<ScrollArea> {
    let mut found: Option<ScrollArea> = None;
    for area in areas {
        if !scrollable(area, side) || !area.region.contains_point(position, window) {
            continue;
        }
        let inner = match (&found, area.region.bounds()) {
            (Some(current), Some(bounds)) => current.region.bounds().is_none_or(|outer| {
                let area = |bounds: Bounds<Pixels>| f32::from(bounds.size.width) * f32::from(bounds.size.height);
                area(bounds) <= area(outer)
            }),
            _ => true,
        };
        if inner {
            found = Some(area.clone());
        }
    }
    found
}

/// What a scroll area under the finger can do, in the machine's terms:
/// `at_start` means it can't scroll further the way a finger moving toward
/// the edge scrolls it, and `at_end` the other way. `None` when no scroll
/// area under it scrolls along the axis (`scrollContext`).
pub fn scroll_context(
    position: Point<Pixels>,
    areas: &[ScrollArea],
    side: PhysicalSide,
    window: &Window,
) -> Option<(ScrollContext, ScrollArea)> {
    let area = scroll_area_at(position, areas, side, window)?;
    let (at_start, at_end) = scroll_edges(&area, side);
    Some((ScrollContext { at_start, at_end }, area))
}

// How far the area is scrolled forward, away from the edge the sheet closes
// toward, and how far it can go.
fn forward_scroll(area: &ScrollArea, side: PhysicalSide) -> (f64, f64) {
    let offset = area.handle.offset();
    let max = area.handle.max_offset();
    if is_vertical(side) {
        let max = f64::from(f32::from(max.y));
        let top = f64::from(f32::from(-offset.y));
        let position = if side == PhysicalSide::Bottom { top } else { max - top };
        return (position, max);
    }
    let max = f64::from(f32::from(max.x));
    let from_left = f64::from(f32::from(-offset.x));
    let position = if side == PhysicalSide::Right {
        from_left
    } else {
        max - from_left
    };
    (position, max)
}

/// Whether the area is at its start and at its end, along the axis.
pub fn scroll_edges(area: &ScrollArea, side: PhysicalSide) -> (bool, bool) {
    let (position, max) = forward_scroll(area, side);
    (position <= 0.5, position >= max - 1.0)
}

/// Scrolls the area forward, away from the sheet's edge, by `delta` px, or
/// back when negative (`scrollForward`).
pub fn scroll_forward(area: &ScrollArea, side: PhysicalSide, delta: f64) {
    let offset = area.handle.offset();
    let max = area.handle.max_offset();
    if is_vertical(side) {
        let top = f64::from(f32::from(-offset.y));
        let next = if side == PhysicalSide::Bottom { top + delta } else { top - delta };
        let next = next.clamp(0.0, f64::from(f32::from(max.y)));
        area.handle.set_offset(point(offset.x, px(-(next as f32))));
    } else {
        let left = f64::from(f32::from(-offset.x));
        let next = if side == PhysicalSide::Right { left + delta } else { left - delta };
        let next = next.clamp(0.0, f64::from(f32::from(max.x)));
        area.handle.set_offset(point(px(-(next as f32)), offset.y));
    }
}
