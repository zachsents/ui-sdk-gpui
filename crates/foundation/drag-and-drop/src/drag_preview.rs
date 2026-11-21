//! Port of `foundation/drag-and-drop/drag-preview.ts`: a copy of a dragged
//! element that follows the pointer above everything else. The element
//! itself stays in the page, where it would land, so it keeps rendering, and
//! the copy is never clipped by a scrolling list or adds to its scroll area,
//! as a translated element in a scrolling container does.
//!
//! The original clones the element's DOM, copies its computed styles and
//! shows the clone in the top layer. gpui can't copy an element, so the
//! component renders the copy itself: [`DragPreview`] holds where the
//! element was when the drag started and how far the pointer has gone since,
//! and [`DragPreview::render`] puts what the component renders for it at
//! that place, at the element's size, above every popup. The copy is hidden
//! from assistive technology. Render plain content for it, not the item's
//! parts: a part's element id, focus handle or `occlude` would make it a
//! second item, and the original's copy has no ids and lets the pointer
//! through (`pointer-events: none`).

use gpui::{
    AnyElement, App, Bounds, Display, Element, ElementId, GlobalElementId, InspectorElementId,
    InteractiveElement, IntoElement, LayoutId, ParentElement, Pixels, Point, Position, Role,
    Style, Styled, Window, deferred, div,
};
use ui_sdk_kernel::part::with_role;
use ui_sdk_kernel::{Aria, with_aria};

// Above every popup and persistent layer, below the drag's own listeners.
const PREVIEW_PRIORITY: usize = usize::MAX - 1;

/// A copy of a dragged element following the pointer (`DragPreview`, what
/// `bindDragPreview` returns): where the element was when the drag started,
/// where the drag started, and where the pointer is now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragPreview {
    /// The element's bounds when the drag started.
    pub bounds: Bounds<Pixels>,
    /// Where the pointer was when the drag started.
    pub from: Point<Pixels>,
    /// Where the pointer is now.
    pub at: Point<Pixels>,
}

/// Shows a copy of an element over the page at the element's place, and
/// moves it with the pointer from the point the drag started at
/// (`bindDragPreview`). `bounds` are the element's, as measured, and `from`
/// where the press was. The component keeps the preview while the drag
/// lasts, renders the copy with [`DragPreview::render`], and drops it to
/// take the copy away (`remove`).
///
/// ```ignore
/// self.preview = Some(bind_drag_preview(tag_bounds, event.position));
/// // on_move: if let Some(preview) = &mut self.preview { preview.r#move(sample.x, sample.y) }
/// // on_end: self.preview = None;
/// // render: .children(self.preview.map(|preview| preview.render(tag_copy(&self.dragging))))
/// ```
pub fn bind_drag_preview(bounds: Bounds<Pixels>, from: Point<Pixels>) -> DragPreview {
    DragPreview {
        bounds,
        from,
        at: from,
    }
}

impl DragPreview {
    /// Moves the copy as far as the pointer moved from where the drag
    /// started (`move`).
    pub fn r#move(&mut self, x: Pixels, y: Pixels) {
        self.at = Point::new(x, y);
    }

    /// Takes the copy away (`remove`): the component stops rendering it.
    pub fn remove(self) {}

    /// How far the copy has moved from the element's place, the original's
    /// `translate`.
    pub fn offset(&self) -> Point<Pixels> {
        self.at - self.from
    }

    /// Where the copy's top left corner is now, in window coordinates.
    pub fn origin(&self) -> Point<Pixels> {
        self.bounds.origin + self.offset()
    }

    /// The copy: `content`, at the element's size, where the preview is now,
    /// drawn above everything else and hidden from assistive technology
    /// (`data-drag-preview`: style the content as the copy).
    pub fn render<E: IntoElement>(&self, content: E) -> impl IntoElement + use<E> {
        // A group, since gpui leaves generic containers out of the tree, so
        // hiding it hides what it holds.
        let copy = with_role(div().id("ui-sdk-drag-preview"), Role::Group)
            .w(self.bounds.size.width)
            .h(self.bounds.size.height)
            .child(content);
        deferred(AtWindowPoint {
            origin: self.origin(),
            child: with_aria(copy, Aria::new().hidden(true)).into_any_element(),
        })
        .with_priority(PREVIEW_PRIORITY)
    }
}

// Lays its child out at a point of the window, whatever holds it, as a
// `position: fixed` element sits.
struct AtWindowPoint {
    origin: Point<Pixels>,
    child: AnyElement,
}

impl Element for AtWindowPoint {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child = self.child.request_layout(window, cx);
        let style = Style {
            position: Position::Absolute,
            display: Display::Flex,
            ..Style::default()
        };
        (window.request_layout(style, [child], cx), child)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let laid_out = window.layout_bounds(*child);
        let offset = self.origin - laid_out.origin;
        window.with_element_offset(offset, |window| {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
