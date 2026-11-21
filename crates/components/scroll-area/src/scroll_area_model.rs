//! Port of `components/scroll-area/scroll-area-model.ts`: the decisions a
//! scroll area makes, as pure functions of plain numbers. `scroll_area_view`
//! measures the viewport and calls these.

use ui_sdk_kernel::Key;

/// How a scroll area shows its scrollbars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScrollAreaType {
    /// Whenever the content overflows.
    Auto,
    /// Even when the content doesn't overflow.
    Always,
    /// While scrolling.
    Scroll,
    /// While scrolling or while the pointer is over the scroll area.
    #[default]
    Hover,
}

impl ScrollAreaType {
    /// The original's name for it, like `"hover"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ScrollAreaType::Auto => "auto",
            ScrollAreaType::Always => "always",
            ScrollAreaType::Scroll => "scroll",
            ScrollAreaType::Hover => "hover",
        }
    }
}

/// An axis of the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
}

impl Axis {
    /// The original's name for it, `"x"` or `"y"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Y => "y",
        }
    }
}

/// One axis of the viewport: where it is and how far it can go.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisMetrics {
    /// Distance scrolled from the start edge. Right to left, it's already positive.
    pub position: f64,
    /// The visible length.
    pub client: f64,
    /// The whole scrollable length.
    pub scroll: f64,
}

/// Scroll positions are fractional on high density screens, while scroll
/// sizes are rounded, so an edge within a pixel counts as reached.
pub const EDGE_TOLERANCE: f64 = 1.0;

/// Thumbs never shrink below this, so they stay easy to grab on long content.
pub const MIN_THUMB_SIZE: f64 = 16.0;

// Chrome pages by this much of the viewport, keeping a little of the last page in view.
const PAGE_FRACTION: f64 = 0.875;

/// The axis a scroll area's start and end refer to: the vertical one, or the
/// horizontal one when only that overflows, like a carousel.
pub fn main_axis(overflow_x: bool, overflow_y: bool) -> Axis {
    if overflow_x && !overflow_y { Axis::X } else { Axis::Y }
}

/// How far the axis can scroll.
pub fn max_scroll(metrics: &AxisMetrics) -> f64 {
    (metrics.scroll - metrics.client).max(0.0)
}

/// Whether the content overflows the axis.
pub fn overflows(metrics: &AxisMetrics) -> bool {
    max_scroll(metrics) >= EDGE_TOLERANCE
}

// The position clamped to the scrollable range. Safari's rubber band goes past both ends.
fn clamped_position(metrics: &AxisMetrics) -> f64 {
    metrics.position.max(0.0).min(max_scroll(metrics))
}

/// How far the content reaches past each edge of an axis, and how far along
/// the scroll is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisEdges {
    pub before: f64,
    pub after: f64,
    pub at_start: bool,
    pub at_end: bool,
    /// From 0 to 1.
    pub progress: f64,
}

/// How far the content reaches past each edge, and how far along the scroll is, from 0 to 1.
pub fn edges_of(metrics: &AxisMetrics) -> AxisEdges {
    let max = max_scroll(metrics);
    let position = clamped_position(metrics);
    let before = if max < EDGE_TOLERANCE { 0.0 } else { position };
    let after = if max < EDGE_TOLERANCE { 0.0 } else { max - position };
    AxisEdges {
        before,
        after,
        at_start: before < EDGE_TOLERANCE,
        at_end: after < EDGE_TOLERANCE,
        progress: if max < EDGE_TOLERANCE { 0.0 } else { position / max },
    }
}

/// A thumb's length and its offset from the start of the track, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ThumbGeometry {
    pub size: f64,
    pub offset: f64,
}

/// The thumb's length and its offset from the start of the track, in pixels.
pub fn thumb_geometry(metrics: &AxisMetrics, track: f64) -> ThumbGeometry {
    if track <= 0.0 || metrics.scroll <= 0.0 {
        return ThumbGeometry::default();
    }
    let size = track.min(MIN_THUMB_SIZE.max((track * metrics.client) / metrics.scroll));
    let max = max_scroll(metrics);
    let offset = if max <= 0.0 {
        0.0
    } else {
        ((track - size) * clamped_position(metrics)) / max
    };
    ThumbGeometry { size, offset }
}

/// The scroll position for a thumb dragged `delta` pixels along a track, from where the drag started.
pub fn dragged_position(start: f64, delta: f64, metrics: &AxisMetrics, track: f64) -> f64 {
    let size = thumb_geometry(metrics, track).size;
    let travel = track - size;
    // A thumb as long as its track can't move, and dividing by zero would jump.
    if travel <= 0.0 {
        return start;
    }
    let max = max_scroll(metrics);
    (start + (delta / travel) * max).max(0.0).min(max)
}

/// The scroll position that centers the thumb on a point of the track.
pub fn position_at(point: f64, metrics: &AxisMetrics, track: f64) -> f64 {
    let size = thumb_geometry(metrics, track).size;
    let travel = track - size;
    if travel <= 0.0 {
        return 0.0;
    }
    let ratio = ((point - size / 2.0) / travel).max(0.0).min(1.0);
    ratio * max_scroll(metrics)
}

/// Which way a press on the track pages: toward the start (-1), toward the
/// end (1), or not at all once the thumb is under the pointer (0). `point`
/// is along the track, from its start.
pub fn page_direction(point: f64, thumb: ThumbGeometry) -> i32 {
    if point < thumb.offset {
        return -1;
    }
    if point > thumb.offset + thumb.size {
        return 1;
    }
    0
}

/// One page, for a viewport this long.
pub fn page_length(client: f64) -> f64 {
    (client * PAGE_FRACTION).round().max(1.0)
}

/// What a wheel delta counts in (`WheelEvent.deltaMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WheelMode {
    #[default]
    Pixel,
    Line,
    Page,
}

/// A wheel delta in pixels. Some mouse wheels report lines, and some devices
/// pages, so a wheel forwarded from a scrollbar scrolls as far as it would
/// over the content.
pub fn wheel_pixels(delta: f64, mode: WheelMode, line_height: f64, page: f64) -> f64 {
    match mode {
        WheelMode::Line => delta * line_height,
        WheelMode::Page => delta * page,
        WheelMode::Pixel => delta,
    }
}

/// Why a scrollbar wants to show: the inputs [`scrollbar_shown`] weighs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ScrollbarSignals {
    pub overflow: bool,
    pub hovering: bool,
    pub scrolling: bool,
    pub dragging: bool,
}

/// Whether a scrollbar should show now, before the hide delay. `Always`
/// shows even without overflow, `Auto` whenever there's overflow, `Scroll`
/// while scrolling, and `Hover` while the pointer is over the scroll area too.
pub fn scrollbar_shown(r#type: ScrollAreaType, signals: ScrollbarSignals) -> bool {
    if r#type == ScrollAreaType::Always {
        return true;
    }
    if !signals.overflow {
        return false;
    }
    if r#type == ScrollAreaType::Auto || signals.dragging || signals.scrolling {
        return true;
    }
    r#type == ScrollAreaType::Hover && signals.hovering
}

/// Whether a scroll area sticking to the end still follows it after the
/// viewport scrolled by `delta` (positive toward the end). Moving away from
/// the end lets go, whatever moved it: the user or the app. Reaching the end
/// while moving toward it follows again. Content growing or shrinking scrolls
/// nothing toward the start and changes nothing.
pub fn follows_end(following: bool, delta: f64, after: f64) -> bool {
    let at_end = after < EDGE_TOLERANCE;
    if following {
        return at_end || delta >= 0.0;
    }
    at_end && delta > 0.0
}

/// Whether a key pressed in the viewport asks to scroll toward the start,
/// which lets go of the end. Not in a text field, where it moves the caret
/// instead.
pub fn scrolls_toward_start(key: &Key, shift: bool, editing: bool) -> bool {
    if editing {
        return false;
    }
    matches!(key, Key::ArrowUp | Key::PageUp | Key::Home) || (*key == Key::Space && shift)
}
