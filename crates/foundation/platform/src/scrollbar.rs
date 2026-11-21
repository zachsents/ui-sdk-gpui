//! Port of `foundation/platform/scrollbar.ts`: whether a press landed on an
//! element's scrollbar rather than its content.
//!
//! The original asks the browser for an element's border and client boxes to
//! find its native scrollbar. gpui draws no scrollbars of its own: the ones an
//! app draws are elements, and a press on them is a press like any other. So
//! only the geometry ports, for a component that reserves a scrollbar gutter
//! inside a box it measured.

use gpui::{Edges, Pixels, Point};

/// An element's border box and the client box inside its borders and
/// scrollbars, relative to its border box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollbarBox {
    pub width: Pixels,
    pub height: Pixels,
    /// Where the client box starts. In right-to-left layouts it includes a
    /// scrollbar on the left.
    pub client_left: Pixels,
    pub client_top: Pixels,
    pub client_width: Pixels,
    pub client_height: Pixels,
    pub borders: Edges<Pixels>,
}

/// Whether a press landed on an element's scrollbar rather than its content.
/// `point` is relative to the element's border box.
pub fn on_scrollbar(point: Point<Pixels>, scroll_box: &ScrollbarBox) -> bool {
    let Point { x, y } = point;
    let borders = scroll_box.borders;
    let in_borders = x >= borders.left
        && x < scroll_box.width - borders.right
        && y >= borders.top
        && y < scroll_box.height - borders.bottom;
    // The padding box. In right-to-left layouts, `client_left` includes a scrollbar on the left.
    let in_padding = x >= scroll_box.client_left
        && x < scroll_box.client_left + scroll_box.client_width
        && y >= scroll_box.client_top
        && y < scroll_box.client_top + scroll_box.client_height;
    in_borders && !in_padding
}
