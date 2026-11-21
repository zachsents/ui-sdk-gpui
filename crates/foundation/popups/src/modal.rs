//! Port of `foundation/popups/modal.ts`: the window-wide side effects of
//! modal layers, the scroll lock and making the rest of the window inert.
//!
//! The original makes everything outside the modal `inert` and stops its
//! scroller, one style property at a time, so several modals and other
//! libraries can hold them together. gpui has neither state, so a modal
//! popup paints a blocker beneath itself instead: hitboxes over the whole
//! window that take every press, hover and scroll from what's under them,
//! with holes for what stays usable: the popup, its backdrop, persistent
//! layers, a trigger that stays usable, layers above it, and regions another
//! popup asked to keep. Each modal paints its own, so several hold at once,
//! and each lets go as it closes. Keyboard focus is the focus trap's.

use std::panic;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, HitboxBehavior, InspectorElementId,
    LayoutId, Pixels, Style, Window, point, px, size,
};
use ui_sdk_platform::Region;

/// The parts of `window` outside every hole: what a modal blocks.
pub fn blocked_rects(window: Bounds<Pixels>, holes: &[Bounds<Pixels>]) -> Vec<Bounds<Pixels>> {
    let holes: Vec<Bounds<Pixels>> = holes
        .iter()
        .map(|hole| hole.intersect(&window))
        .filter(|hole| hole.size.width > px(0.) && hole.size.height > px(0.))
        .collect();
    if holes.is_empty() {
        return vec![window];
    }
    let mut edges: Vec<Pixels> = vec![window.top(), window.bottom()];
    for hole in &holes {
        edges.push(hole.top());
        edges.push(hole.bottom());
    }
    edges.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    edges.dedup();
    let mut rects = Vec::new();
    for band in edges.windows(2) {
        let (top, bottom) = (band[0], band[1]);
        if bottom <= top {
            continue;
        }
        let mut covered: Vec<(Pixels, Pixels)> = holes
            .iter()
            .filter(|hole| hole.top() < bottom && hole.bottom() > top)
            .map(|hole| (hole.left(), hole.right()))
            .collect();
        covered.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut x = window.left();
        for (left, right) in covered {
            if left > x {
                rects.push(Bounds::from_corners(point(x, top), point(left, bottom)));
            }
            x = x.max(right);
        }
        if x < window.right() {
            rects.push(Bounds::from_corners(point(x, top), point(window.right(), bottom)));
        }
    }
    rects
}

/// Makes everything outside `keep` unusable with the pointer while it
/// paints: no press, hover or scroll reaches it (`inertOutside` and
/// `lockScroll`). Render it in `deferred(..)` at
/// [`LAYER_PRIORITY`](ui_sdk_platform::LAYER_PRIORITY), declared before the
/// modal's content, so the content paints above it.
pub fn block_outside(keep: Vec<Region>) -> Blocker {
    Blocker { keep }
}

/// The element [`block_outside`] returns. It takes no room.
pub struct Blocker {
    keep: Vec<Region>,
}

impl Element for Blocker {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            position: gpui::Position::Absolute,
            size: size(px(0.).into(), px(0.).into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) {
        let viewport = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let holes: Vec<Bounds<Pixels>> = self.keep.iter().filter_map(Region::bounds).collect();
        for rect in blocked_rects(viewport, &holes) {
            window.insert_hitbox(rect, HitboxBehavior::BlockMouse);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}
