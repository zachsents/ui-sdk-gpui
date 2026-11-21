//! Port of `foundation/platform/position.ts`: the positioning engine behind
//! every popup.
//!
//! [`compute_placement`] and the functions beside it are the original's
//! geometry, over gpui's bounds in window coordinates and the window's visual
//! viewport. [`position`] makes a [`Positioner`], which keeps a popup beside
//! its anchor: the popup's content renders inside [`positioned`], usually in
//! `deferred(..)`, which lays it out where it's declared, like gpui's
//! `anchored`, and moves it to its placement as it prepaints. That happens on
//! every frame the window draws, and gpui draws a frame for anything that
//! moves, so the popup follows its anchor and its own size in the frame they
//! change, without the original's watchers.
//!
//! Parts read [`Positioner::placement`] for what the original writes on the
//! popup: the side it flipped to and its alignment (`data-side`,
//! `data-align`), the room it has (`--available-width`,
//! `--available-height`), the anchor's size, its transform origin, where the
//! anchor's center falls along its edge (`--arrow-offset`), and its arrow's
//! offset. Those style the next render, so a placement that changes them asks
//! for one, as the original's writes apply at the next style recalc. The
//! content itself is placed in the frame it lays out in; on its very first
//! frame its parts have no placement yet, which [`Positioner::is_placed`]
//! tells, for a popup that would rather render hidden for that frame.

use std::cell::RefCell;
use std::panic;
use std::rc::{Rc, Weak};

use gpui::{
    AnyElement, App, Bounds, Display, Element, ElementId, EntityId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, ParentElement, Pixels, Point, Size, Style, Styled,
    Window, point, px,
};
use ui_sdk_kernel::Measured;

use crate::env::is_rtl;
use crate::layout::probe;
use crate::platform::Region;
use crate::sides::{PhysicalSide, Side, opposite, physical_side};

/// How a popup lines up along the side it opens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Start,
    Center,
    End,
}

impl Align {
    /// The original's name for it, its `data-align`.
    pub fn as_str(self) -> &'static str {
        match self {
            Align::Start => "start",
            Align::Center => "center",
            Align::End => "end",
        }
    }
}

/// Where an element last laid out, read on every check: a [`Region`], the
/// kernel's `Measured`, or a lookup like a part's bounds in `Parts`. It
/// changes only when the window draws.
#[derive(Clone)]
pub struct ElementBounds(Rc<dyn Fn() -> Option<Bounds<Pixels>>>);

impl ElementBounds {
    pub fn new(bounds: impl Fn() -> Option<Bounds<Pixels>> + 'static) -> Self {
        Self(Rc::new(bounds))
    }

    pub fn get(&self) -> Option<Bounds<Pixels>> {
        (self.0)()
    }
}

impl From<Region> for ElementBounds {
    fn from(region: Region) -> Self {
        Self::new(move || region.bounds())
    }
}

impl From<&Region> for ElementBounds {
    fn from(region: &Region) -> Self {
        region.clone().into()
    }
}

impl From<Measured> for ElementBounds {
    fn from(measured: Measured) -> Self {
        Self::new(move || measured.get())
    }
}

impl From<&Measured> for ElementBounds {
    fn from(measured: &Measured) -> Self {
        measured.clone().into()
    }
}

impl From<Bounds<Pixels>> for ElementBounds {
    fn from(bounds: Bounds<Pixels>) -> Self {
        Self::new(move || Some(bounds))
    }
}

/// Something to position against that isn't an element: a point, like the
/// pointer for a context menu, or anything with bounds, like a text
/// selection's range.
#[derive(Clone)]
pub enum VirtualAnchor {
    /// Anything with bounds, read on every frame while shown, so it can move
    /// without anything else redrawing.
    Rect(Rc<dyn Fn(&Window, &App) -> Option<Bounds<Pixels>>>),
    /// A point, like the pointer for a context menu.
    Point(Point<Pixels>),
}

/// An element, a point or a rectangle a popup is positioned against.
#[derive(Clone)]
pub enum Anchor {
    Element(ElementBounds),
    Virtual(VirtualAnchor),
}

impl Anchor {
    /// An element, by where it last laid out.
    pub fn element(bounds: impl Into<ElementBounds>) -> Self {
        Anchor::Element(bounds.into())
    }

    /// A point, like the pointer for a context menu.
    pub fn point(point: Point<Pixels>) -> Self {
        Anchor::Virtual(VirtualAnchor::Point(point))
    }

    /// Anything with bounds, read on every frame while shown.
    pub fn rect(bounds: impl Fn(&Window, &App) -> Option<Bounds<Pixels>> + 'static) -> Self {
        Anchor::Virtual(VirtualAnchor::Rect(Rc::new(bounds)))
    }

    // Nothing reports a virtual rectangle moving, so it's read every frame.
    fn polls(&self) -> bool {
        matches!(self, Anchor::Virtual(VirtualAnchor::Rect(_)))
    }
}

impl From<ElementBounds> for Anchor {
    fn from(bounds: ElementBounds) -> Self {
        Anchor::Element(bounds)
    }
}

impl From<Region> for Anchor {
    fn from(region: Region) -> Self {
        Anchor::element(region)
    }
}

impl From<&Region> for Anchor {
    fn from(region: &Region) -> Self {
        Anchor::element(region)
    }
}

impl From<Measured> for Anchor {
    fn from(measured: Measured) -> Self {
        Anchor::element(measured)
    }
}

impl From<Bounds<Pixels>> for Anchor {
    fn from(bounds: Bounds<Pixels>) -> Self {
        Anchor::element(bounds)
    }
}

impl From<Point<Pixels>> for Anchor {
    fn from(point: Point<Pixels>) -> Self {
        Anchor::point(point)
    }
}

impl From<VirtualAnchor> for Anchor {
    fn from(anchor: VirtualAnchor) -> Self {
        Anchor::Virtual(anchor)
    }
}

/// Placement options every positioned popup takes, as a component's props.
/// Components document their own defaults.
#[derive(Clone, Default)]
pub struct PositionProps {
    /// Which side of the anchor the popup opens on. Flips to the other side
    /// when there isn't room.
    pub side: Option<Side>,
    /// How the popup lines up with the anchor along that side. `Start` and
    /// `End` follow the reading direction.
    pub align: Option<Align>,
    /// Gap between the anchor and the popup.
    pub side_offset: Option<Pixels>,
    /// Shift along the anchor.
    pub align_offset: Option<Pixels>,
    /// Space to keep from the viewport edges. Defaults to 8.
    pub collision_padding: Option<Pixels>,
    /// Space to keep between the arrow and the popup's corners. Defaults to 6.
    pub arrow_padding: Option<Pixels>,
    /// Positions the popup against this instead of its trigger: an element, a
    /// point, or a rectangle, read on every check, so it can move. A
    /// controller's `open` with an anchor wins for that open.
    pub anchor: Option<Anchor>,
}

impl PositionProps {
    pub fn side(mut self, side: Side) -> Self {
        self.side = Some(side);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = Some(align);
        self
    }

    pub fn side_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.side_offset = Some(offset.into());
        self
    }

    pub fn align_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.align_offset = Some(offset.into());
        self
    }

    pub fn collision_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.collision_padding = Some(padding.into());
        self
    }

    pub fn arrow_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.arrow_padding = Some(padding.into());
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.anchor = Some(anchor.into());
        self
    }
}

/// How [`position`] places a popup against its anchor.
#[derive(Clone)]
pub struct PositionOptions {
    /// Which side of the anchor it opens on. Defaults to `Bottom`.
    pub side: Side,
    /// How it lines up with the anchor along that side. Defaults to `Start`.
    pub align: Align,
    /// Gap between the anchor and the popup. Defaults to 4.
    pub side_offset: Pixels,
    /// Shift along the anchor. Defaults to 0.
    pub align_offset: Pixels,
    /// Space to keep from the viewport edges. Defaults to 8.
    pub collision_padding: Pixels,
    /// An element inside that points at the anchor, like an arrow, placed
    /// along the edge facing it. Read on every check, so an arrow that mounts
    /// later is still found.
    pub arrow: Option<ElementBounds>,
    /// Space to keep between the arrow and the corners. Defaults to 6.
    pub arrow_padding: Pixels,
}

impl Default for PositionOptions {
    fn default() -> Self {
        Self {
            side: Side::Bottom,
            align: Align::Start,
            side_offset: px(4.),
            align_offset: px(0.),
            collision_padding: px(8.),
            arrow: None,
            arrow_padding: px(6.),
        }
    }
}

impl PositionOptions {
    pub fn side(mut self, side: Side) -> Self {
        self.side = side;
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    pub fn side_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.side_offset = offset.into();
        self
    }

    pub fn align_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.align_offset = offset.into();
        self
    }

    pub fn collision_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.collision_padding = padding.into();
        self
    }

    pub fn arrow(mut self, arrow: impl Into<ElementBounds>) -> Self {
        self.arrow = Some(arrow.into());
        self
    }

    pub fn arrow_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.arrow_padding = padding.into();
        self
    }
}

/// An anchor's rectangle now. A point has no size. `None` for an element
/// that hasn't laid out yet.
pub fn anchor_rect(anchor: &Anchor, window: &Window, cx: &App) -> Option<Bounds<Pixels>> {
    match anchor {
        Anchor::Element(bounds) => bounds.get(),
        Anchor::Virtual(VirtualAnchor::Rect(bounds)) => bounds(window, cx),
        Anchor::Virtual(VirtualAnchor::Point(point)) => Some(Bounds::new(*point, Size::default())),
    }
}

/// Where [`compute_placement`] puts a popup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComputedPlacement {
    /// The popup's top left corner, in window coordinates.
    pub origin: Point<Pixels>,
    /// The side it opens on, after flipping.
    pub side: PhysicalSide,
    /// How it lines up, as asked: `Start` and `End` stay logical.
    pub align: Align,
    /// The room it has on that side.
    pub available: Size<Pixels>,
}

fn place(
    anchor: Bounds<Pixels>,
    size: Size<Pixels>,
    side: PhysicalSide,
    align: Align,
    options: &PositionOptions,
) -> Point<Pixels> {
    let (side_offset, align_offset) = (options.side_offset, options.align_offset);
    let mut x = px(0.);
    let mut y = px(0.);
    match side {
        PhysicalSide::Bottom => y = anchor.origin.y + anchor.size.height + side_offset,
        PhysicalSide::Top => y = anchor.origin.y - size.height - side_offset,
        PhysicalSide::Right => x = anchor.origin.x + anchor.size.width + side_offset,
        PhysicalSide::Left => x = anchor.origin.x - size.width - side_offset,
    }
    if side.is_vertical() {
        x = match align {
            Align::Start => anchor.origin.x + align_offset,
            Align::Center => {
                anchor.origin.x + anchor.size.width / 2. - size.width / 2. + align_offset
            }
            Align::End => anchor.origin.x + anchor.size.width - size.width - align_offset,
        };
    } else {
        y = match align {
            Align::Start => anchor.origin.y + align_offset,
            Align::Center => {
                anchor.origin.y + anchor.size.height / 2. - size.height / 2. + align_offset
            }
            Align::End => anchor.origin.y + anchor.size.height - size.height - align_offset,
        };
    }
    point(x, y)
}

fn overflow(
    side: PhysicalSide,
    at: Point<Pixels>,
    size: Size<Pixels>,
    viewport: Bounds<Pixels>,
    padding: Pixels,
) -> Pixels {
    match side {
        PhysicalSide::Bottom => {
            at.y + size.height - (viewport.origin.y + viewport.size.height - padding)
        }
        PhysicalSide::Top => viewport.origin.y + padding - at.y,
        PhysicalSide::Right => at.x + size.width - (viewport.origin.x + viewport.size.width - padding),
        PhysicalSide::Left => viewport.origin.x + padding - at.x,
    }
}

fn available(
    side: PhysicalSide,
    anchor: Bounds<Pixels>,
    viewport: Bounds<Pixels>,
    options: &PositionOptions,
) -> Size<Pixels> {
    let (side_offset, padding) = (options.side_offset, options.collision_padding);
    let height = match side {
        PhysicalSide::Bottom => {
            viewport.origin.y + viewport.size.height
                - padding
                - (anchor.origin.y + anchor.size.height + side_offset)
        }
        PhysicalSide::Top => anchor.origin.y - side_offset - padding - viewport.origin.y,
        _ => viewport.size.height - padding * 2.,
    };
    let width = match side {
        PhysicalSide::Right => {
            viewport.origin.x + viewport.size.width
                - padding
                - (anchor.origin.x + anchor.size.width + side_offset)
        }
        PhysicalSide::Left => anchor.origin.x - side_offset - padding - viewport.origin.x,
        _ => viewport.size.width - padding * 2.,
    };
    Size {
        width: width.max(px(0.)),
        height: height.max(px(0.)),
    }
}

// Start and end along the top or bottom follow the reading direction.
fn physical_align(side: PhysicalSide, align: Align, rtl: bool) -> Align {
    match align {
        Align::Start if rtl && side.is_vertical() => Align::End,
        Align::End if rtl && side.is_vertical() => Align::Start,
        align => align,
    }
}

fn clamp(value: Pixels, min: Pixels, max: Pixels) -> Pixels {
    value.max(min).min(min.max(max))
}

// `Math.round`: halves round up.
fn round(value: Pixels) -> Pixels {
    px((value.as_f32() + 0.5).floor())
}

/// Places a popup of `size` beside `anchor`, inside `viewport`: on `side`,
/// flipped to the other side when that overflows less, then shifted along
/// the anchor to stay `collision_padding` inside the viewport, but never past
/// the anchor's far edge. `rtl` swaps logical sides and alignments.
pub fn compute_placement(
    anchor: Bounds<Pixels>,
    size: Size<Pixels>,
    viewport: Bounds<Pixels>,
    options: &PositionOptions,
    rtl: bool,
) -> ComputedPlacement {
    let padding = options.collision_padding;
    let asked = physical_side(options.side, rtl);
    let align = physical_align(asked, options.align, rtl);

    let mut side = asked;
    let mut at = place(anchor, size, side, align, options);
    let over = overflow(side, at, size, viewport, padding);
    if over > px(0.) {
        let other = opposite(side);
        let other_at = place(anchor, size, other, align, options);
        if overflow(other, other_at, size, viewport, padding) < over {
            side = other;
            at = other_at;
        }
    }

    let mut room = available(side, anchor, viewport, options);
    // Beside an anchor at least its size, like a context area opened from the
    // keyboard, with room on neither side, it goes on screen inside the anchor.
    // A popup not measured yet has no size to fit.
    if overflow(side, at, size, viewport, padding) > px(0.) {
        if side.is_vertical() && size.height != px(0.) && anchor.size.height >= size.height {
            at.y = clamp(
                clamp(
                    at.y,
                    viewport.origin.y + padding,
                    viewport.origin.y + viewport.size.height - size.height - padding,
                ),
                anchor.origin.y,
                anchor.origin.y + anchor.size.height - size.height,
            );
            room.height = (viewport.size.height - padding * 2.).max(px(0.));
        } else if !side.is_vertical() && size.width != px(0.) && anchor.size.width >= size.width {
            at.x = clamp(
                clamp(
                    at.x,
                    viewport.origin.x + padding,
                    viewport.origin.x + viewport.size.width - size.width - padding,
                ),
                anchor.origin.x,
                anchor.origin.x + anchor.size.width - size.width,
            );
            room.width = (viewport.size.width - padding * 2.).max(px(0.));
        }
    }

    // Shifts along the anchor to stay on screen, but never past the anchor's
    // far edge, so a popup whose anchor scrolls away leaves with it. An
    // `align_offset` that alone takes it past the anchor is kept.
    if side.is_vertical() {
        let x = clamp(
            at.x,
            viewport.origin.x + padding,
            viewport.origin.x + viewport.size.width - size.width - padding,
        );
        at.x = if x == at.x {
            x
        } else {
            clamp(
                x,
                anchor.origin.x - size.width,
                anchor.origin.x + anchor.size.width,
            )
        };
    } else {
        let y = clamp(
            at.y,
            viewport.origin.y + padding,
            viewport.origin.y + viewport.size.height - size.height - padding,
        );
        at.y = if y == at.y {
            y
        } else {
            clamp(
                y,
                anchor.origin.y - size.height,
                anchor.origin.y + anchor.size.height,
            )
        };
    }

    ComputedPlacement {
        origin: at,
        side,
        align: options.align,
        available: room,
    }
}

/// Where an arrow sits along its popup's edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrowPlacement {
    /// Its offset from the start of the popup's padding box along the edge
    /// facing the anchor: its `left` on the top and bottom sides, its `top` on
    /// the left and right ones.
    pub offset: Pixels,
    /// Whether the anchor's center is past a corner and the arrow had to stop
    /// short (`data-arrow-clamped`).
    pub clamped: bool,
}

/// Where an arrow sits along the popup's edge so it points at the anchor's
/// center, kept clear of the corners. Clamped when the anchor's center is
/// past a corner and the arrow had to stop short.
pub fn compute_arrow(
    anchor: Bounds<Pixels>,
    popup: Bounds<Pixels>,
    arrow: Size<Pixels>,
    side: PhysicalSide,
    padding: Pixels,
) -> ArrowPlacement {
    let vertical = side.is_vertical();
    let length = if vertical { arrow.width } else { arrow.height };
    let center = if vertical {
        anchor.origin.x + anchor.size.width / 2. - popup.origin.x
    } else {
        anchor.origin.y + anchor.size.height / 2. - popup.origin.y
    };
    let ideal = center - length / 2.;
    let max = (if vertical {
        popup.size.width
    } else {
        popup.size.height
    }) - length
        - padding;
    let offset = padding.max(ideal.min(max));
    ArrowPlacement {
        offset: round(offset),
        clamped: (offset - ideal).abs() > px(0.5),
    }
}

/// How far along the popup's edge facing the anchor its center falls, from
/// the start of the popup's padding box, kept `padding` away from both
/// corners.
pub fn arrow_offset(
    anchor: Bounds<Pixels>,
    padding_box: Bounds<Pixels>,
    side: PhysicalSide,
    padding: Pixels,
) -> Pixels {
    let vertical = side.is_vertical();
    let center = if vertical {
        anchor.origin.x + anchor.size.width / 2. - padding_box.origin.x
    } else {
        anchor.origin.y + anchor.size.height / 2. - padding_box.origin.y
    };
    let length = if vertical {
        padding_box.size.width
    } else {
        padding_box.size.height
    };
    round(center.max(padding).min(padding.max(length - padding)))
}

/// The point a popup grows from, from its top left corner: the anchor's
/// edge it lines up with, on the side facing the anchor.
pub fn transform_origin(
    side: PhysicalSide,
    align: Align,
    rtl: bool,
    anchor: Bounds<Pixels>,
    size: Size<Pixels>,
    at: Point<Pixels>,
) -> Point<Pixels> {
    if side.is_vertical() {
        let (left, right) = (anchor.origin.x, anchor.origin.x + anchor.size.width);
        let start = if rtl { right - at.x } else { left - at.x };
        let end = if rtl { left - at.x } else { right - at.x };
        let x = match align {
            Align::Center => left + anchor.size.width / 2. - at.x,
            Align::Start => start,
            Align::End => end,
        };
        let y = if side == PhysicalSide::Bottom {
            px(0.)
        } else {
            size.height
        };
        return point(x, y);
    }
    let (top, bottom) = (anchor.origin.y, anchor.origin.y + anchor.size.height);
    let y = match align {
        Align::Center => top + anchor.size.height / 2. - at.y,
        Align::Start => top - at.y,
        Align::End => bottom - at.y,
    };
    let x = if side == PhysicalSide::Right {
        px(0.)
    } else {
        size.width
    };
    point(x, y)
}

/// Where a popup is, as its parts read it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Its top left corner in window coordinates, on the device's pixel grid
    /// so text inside renders the same each time.
    pub origin: Point<Pixels>,
    /// The side it opened on, after flipping (`data-side`).
    pub side: PhysicalSide,
    /// How it lines up, as asked (`data-align`).
    pub align: Align,
    /// The room it has on that side (`--available-width`,
    /// `--available-height`), like a list's most height.
    pub available: Size<Pixels>,
    /// The anchor's size (`--anchor-width`, `--anchor-height`).
    pub anchor_size: Size<Pixels>,
    /// The point it grows from, from its top left corner, for an entrance
    /// animation (`--transform-origin`).
    pub transform_origin: Point<Pixels>,
    /// Where the anchor's center falls along the edge facing it, from the
    /// start of its padding box, so a part can draw the arrow without an
    /// arrow element (`--arrow-offset`).
    pub arrow_offset: Pixels,
    /// Where the arrow goes, with an [`PositionOptions::arrow`] that laid out.
    pub arrow: Option<ArrowPlacement>,
}

impl Placement {
    // What parts render from, which a new placement re-renders them for.
    fn styles_differ(&self, other: &Placement) -> bool {
        Placement {
            origin: other.origin,
            ..*self
        } != *other
    }
}

struct Current {
    computed: ComputedPlacement,
    anchor: Bounds<Pixels>,
    size: Size<Pixels>,
    origin: Point<Pixels>,
    rtl: bool,
}

struct PositionState {
    anchor: Option<Anchor>,
    options: PositionOptions,
    placement: Option<Placement>,
    placed: bool,
    stopped: bool,
    // The content's padding box this frame, from its probe.
    padding_box: Option<Bounds<Pixels>>,
    // The placement this frame, until the content has laid out its arrow.
    current: Option<Current>,
    // What the last placement read, which the check after its frame compares.
    read: Option<(Option<Bounds<Pixels>>, Bounds<Pixels>)>,
    checking: bool,
}

/// Keeps a popup beside its anchor, flipping to the other side when there
/// isn't room. Clones share it. Made by [`position`]; its content renders in
/// [`positioned`].
#[derive(Clone)]
pub struct Positioner(Rc<RefCell<PositionState>>);

/// Places a popup beside its anchor and keeps it there while anything moves,
/// flipping to the other side when there isn't room. The anchor can be an
/// element, a point for a context menu at the pointer, or a rectangle, like a
/// text selection's. Render the popup's content in [`positioned`] with the
/// returned [`Positioner`]; [`Positioner::stop`] ends it, as the original's
/// returned cleanup does.
pub fn position(anchor: Option<Anchor>, options: PositionOptions) -> Positioner {
    Positioner(Rc::new(RefCell::new(PositionState {
        anchor,
        options,
        placement: None,
        placed: false,
        stopped: false,
        padding_box: None,
        current: None,
        read: None,
        checking: false,
    })))
}

impl Positioner {
    /// What to position against from the next frame, like a new trigger
    /// taking the popup over.
    pub fn set_anchor(&self, anchor: Option<Anchor>) {
        self.0.borrow_mut().anchor = anchor;
    }

    pub fn anchor(&self) -> Option<Anchor> {
        self.0.borrow().anchor.clone()
    }

    /// New options from the next frame, like a side rendered while open: the
    /// original's `follow`. Set them as the popup renders, or notify its view.
    pub fn set_options(&self, options: PositionOptions) {
        self.0.borrow_mut().options = options;
    }

    pub fn options(&self) -> PositionOptions {
        self.0.borrow().options.clone()
    }

    /// Places the popup again, as its first placement, without animating: for
    /// a popup whose size only just became known.
    pub fn place(&self) {
        self.0.borrow_mut().placed = false;
    }

    /// Stops positioning: the popup stays where it was, and a virtual
    /// rectangle anchor is no longer read every frame. Call it as the popup
    /// hides; a new open takes a new [`position`]. Dropping every clone stops
    /// it too.
    pub fn stop(&self) {
        self.0.borrow_mut().stopped = true;
    }

    /// Where the popup is, as of the last frame. `None` until its first
    /// placement, or while its anchor hasn't laid out.
    pub fn placement(&self) -> Option<Placement> {
        self.0.borrow().placement
    }

    /// Whether it has been placed since it started, or since [`place`]. A
    /// popup not placed yet renders where it's declared for that frame.
    ///
    /// [`place`]: Positioner::place
    pub fn is_placed(&self) -> bool {
        self.0.borrow().placed
    }

    // Places the popup for this frame, before its content prepaints. An
    // anchor that left or never laid out has no place to follow, so a placed
    // popup stays where it was rather than jumping to a corner.
    fn begin(&self, size: Size<Pixels>, window: &mut Window, cx: &mut App) -> Option<Point<Pixels>> {
        let (anchor, options, last) = {
            let mut state = self.0.borrow_mut();
            state.padding_box = None;
            state.current = None;
            let last = state.placement.map(|placement| placement.origin);
            if state.stopped {
                return last;
            }
            (state.anchor.clone(), state.options.clone(), last)
        };
        let Some(anchor) = anchor else {
            return last;
        };
        let rect = anchor_rect(&anchor, window, cx);
        let viewport = window.visual_viewport_bounds();
        self.0.borrow_mut().read = Some((rect, viewport));
        let Some(rect) = rect else {
            return last;
        };
        let rtl = is_rtl(cx);
        let computed = compute_placement(rect, size, viewport, &options, rtl);
        let origin = window.pixel_snap_point(computed.origin);
        self.0.borrow_mut().current = Some(Current {
            computed,
            anchor: rect,
            size,
            origin,
            rtl,
        });
        Some(origin)
    }

    // Finishes the placement once the content has laid out at it: its padding
    // box, and its arrow's size, place the arrow.
    fn finish(&self, window: &mut Window) {
        let view = window.current_view();
        let (current, options, padding) = {
            let mut state = self.0.borrow_mut();
            let Some(current) = state.current.take() else {
                // An anchor that lays out later in this frame is seen after it.
                let check = !state.stopped
                    && state.read.is_some()
                    && !std::mem::replace(&mut state.checking, true);
                drop(state);
                if check {
                    check_after_frame(Rc::downgrade(&self.0), view, window);
                }
                return;
            };
            (current, state.options.clone(), state.padding_box)
        };
        let Current {
            computed,
            anchor,
            size,
            origin,
            rtl,
        } = current;
        // The padding box, which arrows are placed in, where the placement
        // put the popup.
        let padding_box = padding.map_or(Bounds::new(computed.origin, size), |probe| {
            Bounds::new(computed.origin + (probe.origin - origin), probe.size)
        });
        let side = computed.side;
        let arrow = options.arrow.as_ref().and_then(ElementBounds::get).map(|arrow| {
            compute_arrow(
                anchor,
                Bounds::new(padding_box.origin, size),
                arrow.size,
                side,
                options.arrow_padding,
            )
        });
        let grow = transform_origin(side, computed.align, rtl, anchor, size, computed.origin);
        let placement = Placement {
            origin,
            side,
            align: computed.align,
            available: computed.available,
            anchor_size: anchor.size,
            transform_origin: point(round(grow.x), round(grow.y)),
            arrow_offset: arrow_offset(anchor, padding_box, side, options.arrow_padding),
            arrow,
        };
        let (restyle, check) = {
            let mut state = self.0.borrow_mut();
            let restyle = !state.placed
                || state
                    .placement
                    .is_none_or(|last| last.styles_differ(&placement));
            state.placement = Some(placement);
            state.placed = true;
            let check = !std::mem::replace(&mut state.checking, true);
            (restyle, check)
        };
        if restyle {
            window.on_next_frame(move |_, cx| cx.notify(view));
        }
        if check {
            check_after_frame(Rc::downgrade(&self.0), view, window);
        }
    }
}

// Once the frame has drawn, an anchor measured after the popup in it, or a
// virtual rectangle nothing reports moving, may have moved: the popup's view
// draws again then. A rectangle is checked on every frame while shown, as the
// original's loop keeps following one.
fn check_after_frame(state: Weak<RefCell<PositionState>>, view: EntityId, window: &Window) {
    window.on_next_frame(move |window, cx| {
        let Some(state) = state.upgrade() else {
            return;
        };
        let (anchor, read, stopped) = {
            let state = state.borrow();
            (state.anchor.clone(), state.read, state.stopped)
        };
        let now = anchor
            .as_ref()
            .map(|anchor| (anchor_rect(anchor, window, cx), window.visual_viewport_bounds()));
        if !stopped && now.is_some() && now != read {
            state.borrow_mut().checking = false;
            cx.notify(view);
            return;
        }
        if !stopped && anchor.as_ref().is_some_and(Anchor::polls) {
            check_after_frame(Rc::downgrade(&state), view, window);
            return;
        }
        state.borrow_mut().checking = false;
    });
}

/// Renders a popup's content where its [`Positioner`] places it. Lay it out
/// as gpui's `anchored` is: where it's declared, usually inside
/// `deferred(..)`, so it paints above the window; its width is bounded by
/// what it's declared in, so declare it where nothing narrower than the
/// window holds it, or size it. Unlike `anchored`, it never snaps into the
/// window on its own, since the placement already keeps it on screen and lets
/// it leave with an anchor that scrolls away. Content that should stop
/// presses from reaching what's under it occludes, as any gpui overlay does.
pub fn positioned<E>(positioner: &Positioner, content: E) -> Positioned
where
    E: IntoElement + ParentElement + Styled + 'static,
{
    let state = positioner.0.clone();
    let content = probe(content, move |bounds, _, _| {
        state.borrow_mut().padding_box = Some(bounds);
    });
    Positioned {
        positioner: positioner.clone(),
        child: content.into_any_element(),
    }
}

/// The element [`positioned`] returns.
pub struct Positioned {
    positioner: Positioner,
    child: AnyElement,
}

impl Element for Positioned {
    type RequestLayoutState = LayoutId;
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
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child = self.child.request_layout(window, cx);
        let style = Style {
            position: gpui::Position::Absolute,
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
        let offset = self
            .positioner
            .begin(laid_out.size, window, cx)
            .map_or_else(Point::default, |origin| origin - laid_out.origin);
        window.with_element_offset(offset, |window| {
            self.child.prepaint(window, cx);
        });
        self.positioner.finish(window);
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
