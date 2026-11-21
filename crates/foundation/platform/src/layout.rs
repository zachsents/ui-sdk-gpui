//! Port of `foundation/platform/layout.ts`: where an element sits in the
//! layout, unmoved by scrolling.
//!
//! The original reads offsets from the DOM, which come from layout, so
//! transforms and scrolling never move them. gpui has no offsets: an element's
//! bounds come from measuring it (the kernel's `Measured`, `Parts::measure`, or
//! a [`Region`](crate::Region)), and those already include the scroll of the
//! container around it, so these take the container's [`ScrollHandle`] and
//! take its scroll back out. gpui has no transforms or `zoom`, and its bounds
//! keep their fractions, so the original's rounding fixes have nothing to fix.

use std::panic::Location;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId, ParentElement,
    Pixels, Point, Refineable as _, ScrollHandle, Size, Style, StyleRefinement, Styled, Window,
    canvas, px,
};

/// An element's box inside a container's scrollable area: the offset from
/// each edge of that area, and the size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutBox {
    pub left: Pixels,
    pub top: Pixels,
    pub width: Pixels,
    pub height: Pixels,
    pub right: Pixels,
    pub bottom: Pixels,
}

/// An element's place in the window's layout: its measured origin, less the
/// scroll of the container it scrolls in, so scrolling never moves it.
pub fn layout_origin(el: Bounds<Pixels>, scroll: Option<&ScrollHandle>) -> Point<Pixels> {
    el.origin - scroll.map(ScrollHandle::offset).unwrap_or_default()
}

/// An element's size as laid out. gpui keeps the fraction the DOM's offsets
/// round away.
pub fn layout_size(el: Bounds<Pixels>) -> Size<Pixels> {
    el.size
}

/// An element's box inside a container's scrollable area: the offset from
/// each edge of that area, and the size. What an indicator positioned inside
/// the container needs to cover the element.
///
/// `el` and `container` are measured bounds, like a tab's and its list's;
/// `scroll` is the container's scroll handle when it scrolls. A container
/// measured with a probe child, as `Measured` does, reports its padding box,
/// where the scrollable area starts.
pub fn layout_box(
    el: Bounds<Pixels>,
    container: Bounds<Pixels>,
    scroll: Option<&ScrollHandle>,
) -> LayoutBox {
    let at = layout_origin(el, scroll);
    let left = at.x - container.origin.x;
    let top = at.y - container.origin.y;
    let Size { width, height } = layout_size(el);
    let extra = scroll.map(ScrollHandle::max_offset).unwrap_or_default();
    let area = Size {
        width: container.size.width + extra.x.max(px(0.)),
        height: container.size.height + extra.y.max(px(0.)),
    };
    LayoutBox {
        left,
        top,
        width,
        height,
        right: area.width - (left + width),
        bottom: area.height - (top + height),
    }
}

/// Adds a child to `el` that hears its bounds each time it lays out, the way
/// the kernel's `Measured` does: a layer over its padding box, measured during
/// prepaint.
pub(crate) fn probe<E: ParentElement + Styled>(
    el: E,
    measure: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static,
) -> E {
    ui_sdk_kernel::ensure_positioned(el).child(
        canvas(measure, |_, _, _, _| {})
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
    )
}

type PlacedPrepaint<T> = Box<dyn FnOnce(Bounds<Pixels>, &[ElementId], &mut Window, &mut App) -> T>;
type PlacedPaint<T> = Box<dyn FnOnce(Bounds<Pixels>, T, &[ElementId], &mut Window, &mut App)>;

/// A canvas with an id, so it hears where it is as it draws: the path of
/// element ids to the element it's in, as the kernel records parts' places.
/// An element without an id is where its nearest ancestor with one is.
pub(crate) struct PlacedCanvas<T> {
    id: ElementId,
    prepaint: Option<PlacedPrepaint<T>>,
    paint: Option<PlacedPaint<T>>,
    style: StyleRefinement,
}

pub(crate) fn placed_canvas<T>(
    id: impl Into<ElementId>,
    prepaint: impl FnOnce(Bounds<Pixels>, &[ElementId], &mut Window, &mut App) -> T + 'static,
    paint: impl FnOnce(Bounds<Pixels>, T, &[ElementId], &mut Window, &mut App) + 'static,
) -> PlacedCanvas<T> {
    PlacedCanvas {
        id: id.into(),
        prepaint: Some(Box::new(prepaint)),
        paint: Some(Box::new(paint)),
        style: StyleRefinement::default(),
    }
}

// The place of the element a canvas with `id` is in.
fn around(id: Option<&GlobalElementId>) -> &[ElementId] {
    id.map_or(&[], |id| &id[..id.len().saturating_sub(1)])
}

impl<T: 'static> Element for PlacedCanvas<T> {
    type RequestLayoutState = ();
    type PrepaintState = Option<T>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.refine(&self.style);
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<T> {
        let prepaint = self.prepaint.take()?;
        Some(prepaint(bounds, around(id), window, cx))
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut Option<T>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let (Some(paint), Some(prepaint)) = (self.paint.take(), prepaint.take()) {
            paint(bounds, prepaint, around(id), window, cx);
        }
    }
}

impl<T> Styled for PlacedCanvas<T> {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
