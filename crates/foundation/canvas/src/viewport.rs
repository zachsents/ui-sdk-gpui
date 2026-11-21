//! Port of `foundation/canvas/viewport.ts`: a viewport onto a canvas larger
//! than the screen: a node editor, a whiteboard, a map of anything. It pans
//! and zooms one world layer, and turns screen points into canvas points and
//! back, so items keep plain positions however far you scroll. Wheel and
//! trackpad pan, Ctrl or ⌘ with the wheel and a trackpad pinch zoom at the
//! pointer, a drag on empty space pans, and the keys zoom too.
//!
//! The original moves the world with one CSS transform. gpui has no
//! transforms, so [`CanvasViewport::world`] places the world at the pan
//! and scales the rem size inside it by the zoom: what's in the world, sized
//! and placed in canvas units with [`CanvasViewport::units`], scales with it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    AbsoluteLength, AnyElement, App, Bounds, Context, DefiniteLength, Display, Edges, Element,
    ElementId, GlobalElementId, InspectorElementId, IntoElement, KeyDownEvent, LayoutId, Length,
    MouseButton, ParentElement, PinchEvent, Pixels, Point, Position, Rems, ScrollEvent, Style,
    Styled, Window, point, px, rems,
};
use ui_sdk_drag_and_drop::{BindDragOptions, DragSession, PointerEvent, PointerKind};
use ui_sdk_kernel::{ChangeOptions, InputEvent, Key, Measured, Reason};
use ui_sdk_platform::announce;

/// Where the canvas sits on screen: its origin's offset from the surface's
/// top left, in pixels, and its scale (`Viewport`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            x: 0.,
            y: 0.,
            zoom: 1.,
        }
    }
}

impl Viewport {
    pub fn new(x: f32, y: f32, zoom: f32) -> Self {
        Self { x, y, zoom }
    }
}

/// A box on the canvas, in canvas units (`CanvasRect`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CanvasRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl CanvasRect {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

/// Why the viewport moved (`ViewportReason`). A change reports it as the
/// kernel's reason of the same name ([`reason`](Self::reason)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewportReason {
    Wheel,
    Drag,
    Pinch,
    ZoomKey,
    FocusIn,
    /// Kept for the vocabulary; gpui has no find in page.
    FindInPage,
    Imperative,
}

impl ViewportReason {
    /// The original's name for it, like `"zoom-key"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ViewportReason::Wheel => "wheel",
            ViewportReason::Drag => "drag",
            ViewportReason::Pinch => "pinch",
            ViewportReason::ZoomKey => "zoom-key",
            ViewportReason::FocusIn => "focus-in",
            ViewportReason::FindInPage => "find-in-page",
            ViewportReason::Imperative => "imperative",
        }
    }

    /// The kernel's reason for it.
    pub fn reason(self) -> Reason {
        match self {
            ViewportReason::Wheel => Reason::Wheel,
            ViewportReason::Drag => Reason::Drag,
            ViewportReason::Pinch => Reason::Pinch,
            ViewportReason::ZoomKey => Reason::ZoomKey,
            ViewportReason::FocusIn => Reason::FocusIn,
            ViewportReason::FindInPage => Reason::FindInPage,
            ViewportReason::Imperative => Reason::Imperative,
        }
    }
}

/// Why and how the viewport moved, what `set` receives.
#[derive(Clone, Debug)]
pub struct ViewportDetails {
    pub reason: ViewportReason,
    pub event: Option<InputEvent>,
}

impl ViewportDetails {
    pub fn new(reason: ViewportReason) -> Self {
        Self {
            reason,
            event: None,
        }
    }

    pub fn event(reason: ViewportReason, event: impl Into<InputEvent>) -> Self {
        Self {
            reason,
            event: Some(event.into()),
        }
    }

    /// The details as a kernel change reports them, for
    /// `Controllable::change`.
    pub fn change_options(&self) -> ChangeOptions {
        ChangeOptions {
            reason: Some(self.reason.reason()),
            source: None,
            event: self.event.clone(),
        }
    }
}

impl Default for ViewportDetails {
    fn default() -> Self {
        Self::new(ViewportReason::Imperative)
    }
}

/// Reads the component's viewport.
pub type ViewportFn<T> = Rc<dyn Fn(&T) -> Viewport>;

/// Asks the component for a new viewport and returns whether it went
/// through.
pub type SetViewportFn<T> =
    Rc<dyn Fn(&mut T, Viewport, ViewportDetails, &mut Window, &mut Context<T>) -> bool>;

/// Reads a zoom bound from the component.
pub type ZoomFn<T> = Rc<dyn Fn(&T) -> f32>;

/// What screen readers hear after a zoom key, by the zoom.
pub type ZoomMessageFn = Rc<dyn Fn(f32) -> String>;

/// How [`create_viewport`] keeps the viewport (`ViewportOptions`).
pub struct ViewportOptions<T: 'static> {
    /// The viewport when the component keeps it (`key`). Without it, the
    /// viewport keeps it.
    pub value: Option<ViewportFn<T>>,
    /// Asks for a new viewport. Defaults to keeping it; pass the component's
    /// change to make it controllable.
    pub set: Option<SetViewportFn<T>>,
    /// Defaults to 0.1.
    pub min_zoom: Option<ZoomFn<T>>,
    /// Defaults to 4.
    pub max_zoom: Option<ZoomFn<T>>,
    /// What screen readers hear after a zoom key. Defaults to English, like
    /// "Zoom 150%".
    pub message: Option<ZoomMessageFn>,
}

impl<T: 'static> Default for ViewportOptions<T> {
    fn default() -> Self {
        Self {
            value: None,
            set: None,
            min_zoom: None,
            max_zoom: None,
            message: None,
        }
    }
}

impl<T: 'static> Clone for ViewportOptions<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            set: self.set.clone(),
            min_zoom: self.min_zoom.clone(),
            max_zoom: self.max_zoom.clone(),
            message: self.message.clone(),
        }
    }
}

impl<T: 'static> ViewportOptions<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lets the component keep the viewport: `value` reads it, `set` asks
    /// for a new one.
    pub fn controlled(
        mut self,
        value: impl Fn(&T) -> Viewport + 'static,
        set: impl Fn(&mut T, Viewport, ViewportDetails, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        self.value = Some(Rc::new(value));
        self.set = Some(Rc::new(set));
        self
    }

    pub fn set(
        mut self,
        set: impl Fn(&mut T, Viewport, ViewportDetails, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        self.set = Some(Rc::new(set));
        self
    }

    pub fn min_zoom(mut self, min_zoom: impl Fn(&T) -> f32 + 'static) -> Self {
        self.min_zoom = Some(Rc::new(min_zoom));
        self
    }

    pub fn max_zoom(mut self, max_zoom: impl Fn(&T) -> f32 + 'static) -> Self {
        self.max_zoom = Some(Rc::new(max_zoom));
        self
    }

    pub fn message(mut self, message: impl Fn(f32) -> String + 'static) -> Self {
        self.message = Some(Rc::new(message));
        self
    }
}

/// How [`CanvasViewport::fit`] fits a box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitOptions {
    /// Screen pixels around the box. Defaults to 32.
    pub padding: f32,
    /// The furthest in it zooms. Defaults to 1.
    pub max_zoom: f32,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            padding: 32.,
            max_zoom: 1.,
        }
    }
}

// The rem size the world scales from: a canvas unit is a pixel at zoom 1.
const BASE_REM: f32 = 16.;

// A drag on the surface, from where it started.
struct Gesture {
    center: Point<Pixels>,
    view: Viewport,
}

struct Inner<T: 'static> {
    options: RefCell<ViewportOptions<T>>,
    own: Cell<Viewport>,
    surface: Measured,
    gesture: RefCell<Option<Gesture>>,
    drags: DragSession<T>,
}

/// Pans and zooms a canvas (`createViewport`'s return). Cloning it gives
/// another handle to the same viewport.
pub struct CanvasViewport<T: 'static> {
    inner: Rc<Inner<T>>,
}

impl<T: 'static> Clone for CanvasViewport<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Pans and zooms a canvas (`createViewport`). Make it in the component's
/// constructor and keep it in a field. Measure the surface, the element that
/// clips the canvas and takes the gestures, with
/// [`surface`](CanvasViewport::surface); put every item in one world element
/// rendered through [`world`](CanvasViewport::world), positioned inside it
/// in canvas units; render [`listener`](CanvasViewport::listener) on every
/// frame; and hand the surface's wheel, pinch, presses on empty space and
/// keys to [`wheel`](CanvasViewport::wheel), [`pinch`](CanvasViewport::pinch),
/// [`pointer_down`](CanvasViewport::pointer_down) and
/// [`keydown`](CanvasViewport::keydown).
///
/// `to_canvas` and `to_screen` convert points, `fit` shows a box, and
/// `reveal` scrolls the least to bring one into view, which a focused item
/// calls.
pub fn create_viewport<T: 'static>(options: ViewportOptions<T>, cx: &Context<T>) -> CanvasViewport<T> {
    CanvasViewport {
        inner: Rc::new(Inner {
            options: RefCell::new(options),
            own: Cell::new(Viewport::default()),
            surface: Measured::default(),
            gesture: RefCell::new(None),
            drags: DragSession::new(cx),
        }),
    }
}

// The surface's box on screen.
struct Frame {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

impl<T: 'static> CanvasViewport<T> {
    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: ViewportOptions<T>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn options(&self) -> ViewportOptions<T> {
        self.inner.options.borrow().clone()
    }

    /// Measures the surface, the element that clips the canvas and takes
    /// the gestures (`surface`).
    pub fn surface<E: ParentElement + Styled>(&self, el: E) -> E {
        self.inner.surface.measure(el)
    }

    /// The element that follows the pointer while a drag pans. Render it on
    /// every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T> {
        self.inner.drags.listener()
    }

    fn frame(&self) -> Frame {
        let bounds = self.inner.surface.get().unwrap_or_default();
        Frame {
            left: f32::from(bounds.left()),
            top: f32::from(bounds.top()),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
        }
    }

    fn clamp_zoom(&self, this: &T, zoom: f32) -> f32 {
        let options = self.options();
        let min = options.min_zoom.map_or(0.1, |min| min(this));
        let max = options.max_zoom.map_or(4., |max| max(this));
        zoom.max(min).min(max)
    }

    /// The viewport now.
    pub fn get(&self, this: &T) -> Viewport {
        let value = self.inner.options.borrow().value.clone();
        match value {
            Some(value) => value(this),
            None => self.inner.own.get(),
        }
    }

    /// A point on screen, like a pointer's, in canvas units.
    pub fn to_canvas(&self, this: &T, client: Point<Pixels>) -> Point<f32> {
        let Viewport { x, y, zoom } = self.get(this);
        let frame = self.frame();
        point(
            (f32::from(client.x) - frame.left - x) / zoom,
            (f32::from(client.y) - frame.top - y) / zoom,
        )
    }

    /// A canvas point on screen.
    pub fn to_screen(&self, this: &T, at: Point<f32>) -> Point<Pixels> {
        let Viewport { x, y, zoom } = self.get(this);
        let frame = self.frame();
        point(
            px(at.x * zoom + x + frame.left),
            px(at.y * zoom + y + frame.top),
        )
    }

    /// Where an element is on the canvas, like a port inside a node, from
    /// its bounds on screen (`rectOf`).
    pub fn rect_of(&self, this: &T, bounds: Bounds<Pixels>) -> CanvasRect {
        let a = self.to_canvas(this, bounds.origin);
        let b = self.to_canvas(this, bounds.bottom_right());
        CanvasRect::new(a.x, a.y, b.x, b.y)
    }

    /// The part of the canvas on screen now.
    pub fn visible(&self, this: &T) -> CanvasRect {
        let Viewport { x, y, zoom } = self.get(this);
        let frame = self.frame();
        CanvasRect::new(
            -x / zoom,
            -y / zoom,
            (frame.width - x) / zoom,
            (frame.height - y) / zoom,
        )
    }

    /// Moves to a viewport, keeping the zoom within bounds (`setViewport`).
    /// Returns whether it moved.
    pub fn set_viewport(
        &self,
        this: &mut T,
        value: Viewport,
        details: ViewportDetails,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let next = Viewport {
            x: value.x,
            y: value.y,
            zoom: self.clamp_zoom(this, value.zoom),
        };
        if next == self.get(this) {
            return false;
        }
        let options = self.options();
        match options.set {
            Some(set) => {
                let done = set(this, next, details, window, cx);
                if done && options.value.is_none() {
                    self.inner.own.set(next);
                    cx.notify();
                }
                done
            }
            None => {
                self.inner.own.set(next);
                cx.notify();
                true
            }
        }
    }

    /// Pans by screen pixels.
    pub fn pan_by(
        &self,
        this: &mut T,
        dx: f32,
        dy: f32,
        details: ViewportDetails,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let Viewport { x, y, zoom } = self.get(this);
        self.set_viewport(this, Viewport::new(x + dx, y + dy, zoom), details, window, cx)
    }

    /// Zooms to a scale, keeping the canvas point under a screen point
    /// still. Defaults to the surface's center.
    pub fn zoom_to(
        &self,
        this: &mut T,
        zoom: f32,
        at: Option<Point<Pixels>>,
        details: ViewportDetails,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let view = self.get(this);
        let frame = self.frame();
        let px_ = at.map_or(frame.left + frame.width / 2., |at| f32::from(at.x)) - frame.left;
        let py_ = at.map_or(frame.top + frame.height / 2., |at| f32::from(at.y)) - frame.top;
        let next = self.clamp_zoom(this, zoom);
        let k = next / view.zoom;
        self.set_viewport(
            this,
            Viewport::new(px_ - (px_ - view.x) * k, py_ - (py_ - view.y) * k, next),
            details,
            window,
            cx,
        )
    }

    /// Zooms by a factor, like `1.2` in and `1. / 1.2` out.
    pub fn zoom_by(
        &self,
        this: &mut T,
        factor: f32,
        at: Option<Point<Pixels>>,
        details: ViewportDetails,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let zoom = self.get(this).zoom * factor;
        self.zoom_to(this, zoom, at, details, window, cx)
    }

    /// Shows a canvas box whole and centered, zooming no further in than
    /// `max_zoom`.
    pub fn fit(
        &self,
        this: &mut T,
        rect: CanvasRect,
        options: FitOptions,
        details: ViewportDetails,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let frame = self.frame();
        let width = (rect.right - rect.left).max(1.);
        let height = (rect.bottom - rect.top).max(1.);
        let zoom = self.clamp_zoom(
            this,
            ((frame.width - options.padding * 2.) / width)
                .min((frame.height - options.padding * 2.) / height)
                .min(options.max_zoom),
        );
        self.set_viewport(
            this,
            Viewport::new(
                frame.width / 2. - (rect.left + width / 2.) * zoom,
                frame.height / 2. - (rect.top + height / 2.) * zoom,
                zoom,
            ),
            details,
            window,
            cx,
        )
    }

    /// Pans the least distance that brings a canvas box into view, with
    /// `padding` screen pixels around it, 24 by default. A focused item
    /// calls it, which reports `FocusIn`.
    pub fn reveal(
        &self,
        this: &mut T,
        rect: CanvasRect,
        padding: Option<f32>,
        details: ViewportDetails,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let zoom = self.get(this).zoom;
        let seen = self.visible(this);
        let pad = padding.unwrap_or(24.) / zoom;
        let shift = |low: f32, high: f32, from: f32, to: f32| {
            if high - low > to - from - pad * 2. {
                (from + to) / 2. - (low + high) / 2.
            } else if low < from + pad {
                from + pad - low
            } else if high > to - pad {
                to - pad - high
            } else {
                0.
            }
        };
        let dx = shift(rect.left, rect.right, seen.left, seen.right);
        let dy = shift(rect.top, rect.bottom, seen.top, seen.bottom);
        if dx == 0. && dy == 0. {
            return false;
        }
        self.pan_by(this, dx * zoom, dy * zoom, details, window, cx)
    }

    /// Pans with the wheel or a two finger scroll; zooms at the pointer with
    /// Ctrl or ⌘. Stops the scroll from reaching a scroller around it.
    pub fn wheel(&self, this: &mut T, event: &ScrollEvent, window: &mut Window, cx: &mut Context<T>) {
        if self.inner.surface.get().is_none() {
            return;
        }
        cx.stop_propagation();
        // A line is 16 pixels, the browser's line `deltaMode`. gpui's
        // deltas move the content, the browser's the scroll position.
        let delta = event.delta.pixel_delta(px(16.));
        let details = ViewportDetails::event(ViewportReason::Wheel, event);
        if event.modifiers.control || event.modifiers.platform {
            let dom = (-f32::from(delta.y)).clamp(-50., 50.);
            self.zoom_by(this, (-dom * 0.01).exp(), Some(event.position), details, window, cx);
        } else {
            self.pan_by(this, f32::from(delta.x), f32::from(delta.y), details, window, cx);
        }
    }

    /// Zooms at a trackpad pinch's center, reporting `pinch`. The original
    /// hears a trackpad pinch as a Ctrl wheel, and two fingers on the
    /// surface as pointers; gpui doesn't recognize a touch pinch.
    pub fn pinch(&self, this: &mut T, event: &PinchEvent, window: &mut Window, cx: &mut Context<T>) {
        if self.inner.surface.get().is_none() {
            return;
        }
        cx.stop_propagation();
        let details = ViewportDetails::event(ViewportReason::Pinch, event);
        self.zoom_by(this, 1. + event.delta, Some(event.position), details, window, cx);
    }

    /// Pans while a pointer drags the surface: the main button, the middle
    /// one, or a finger. Call it for presses on empty space.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if self.inner.surface.get().is_none()
            || (event.kind == PointerKind::Mouse
                && event.button != MouseButton::Left
                && event.button != MouseButton::Middle)
        {
            return;
        }
        // The original prevents the press's default, text selection and
        // native dragging, which gpui doesn't do; the surface still takes
        // focus, for the zoom keys.
        *self.inner.gesture.borrow_mut() = Some(Gesture {
            center: event.position,
            view: self.get(this),
        });
        let moved = Rc::downgrade(&self.inner);
        let ended = moved.clone();
        self.inner.drags.start(
            event,
            BindDragOptions::new()
                .on_move(move |this: &mut T, sample, window, cx| {
                    let Some(inner) = moved.upgrade() else {
                        return;
                    };
                    let Some((center, view)) = inner
                        .gesture
                        .borrow()
                        .as_ref()
                        .map(|gesture| (gesture.center, gesture.view))
                    else {
                        return;
                    };
                    let viewport = CanvasViewport { inner };
                    let next = Viewport::new(
                        view.x + f32::from(sample.x - center.x),
                        view.y + f32::from(sample.y - center.y),
                        view.zoom,
                    );
                    let details = ViewportDetails {
                        reason: ViewportReason::Drag,
                        event: Some(sample.event.event.clone()),
                    };
                    viewport.set_viewport(this, next, details, window, cx);
                })
                .on_end(move |_this: &mut T, _end, _canceled, _window, _cx| {
                    if let Some(inner) = ended.upgrade() {
                        inner.gesture.borrow_mut().take();
                    }
                }),
            window,
            cx,
        );
    }

    /// `+` and `-` zoom at `at`, or the center, and `0` goes back to 100%.
    /// Returns whether it handled the key.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let modifiers = event.keystroke.modifiers;
        if modifiers.alt || modifiers.control || modifiers.platform {
            return false;
        }
        let factor = match Key::of(&event.keystroke) {
            Key::Character(typed) => match typed.as_str() {
                "+" | "=" => 1.25,
                "-" | "_" => 0.8,
                "0" => 0.,
                _ => return false,
            },
            _ => return false,
        };
        window.prevent_default();
        let details = ViewportDetails::event(ViewportReason::ZoomKey, event);
        let done = if factor != 0. {
            self.zoom_by(this, factor, at, details, window, cx)
        } else {
            self.zoom_to(this, 1., at, details, window, cx)
        };
        if done {
            let zoom = self.get(this).zoom;
            let message = self.inner.options.borrow().message.clone();
            let text = match message {
                Some(message) => message(zoom),
                None => format!("Zoom {}%", (zoom * 100.).round()),
            };
            announce(&text, window, cx);
        }
        true
    }

    /// The world: `content` placed at the pan, with the rem size inside it
    /// scaled by the zoom (`style`, the original's one transform for every
    /// item). Size and place what's inside in [`units`](Self::units): lengths
    /// in rems scale with the zoom; lengths in pixels don't.
    pub fn world<E: IntoElement>(&self, this: &T, content: E) -> impl IntoElement + use<T, E> {
        let Viewport { x, y, zoom } = self.get(this);
        World {
            offset: point(px(x), px(y)),
            zoom,
            child: content.into_any_element(),
        }
    }

    /// A length in canvas units, for what's inside the world, which scales
    /// with the zoom (the original's `--canvas-zoom` is `get(this).zoom`).
    pub fn units(value: f32) -> Rems {
        rems(value / BASE_REM)
    }
}

// The world layer: laid out at the surface's top left, drawn at the pan,
// with rems scaled by the zoom.
struct World {
    offset: Point<Pixels>,
    zoom: f32,
    child: AnyElement,
}

impl World {
    fn rem(&self) -> Pixels {
        px(BASE_REM * self.zoom)
    }
}

impl Element for World {
    type RequestLayoutState = ();
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
        let rem = self.rem();
        let child = window.with_rem_size(Some(rem), |window| self.child.request_layout(window, cx));
        let zero = Length::Definite(DefiniteLength::Absolute(AbsoluteLength::Pixels(px(0.))));
        let style = Style {
            position: Position::Absolute,
            display: Display::Flex,
            inset: Edges {
                top: zero,
                left: zero,
                right: Length::Auto,
                bottom: Length::Auto,
            },
            ..Style::default()
        };
        (window.request_layout(style, [child], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let rem = self.rem();
        let offset = self.offset;
        window.with_rem_size(Some(rem), |window| {
            window.with_element_offset(offset, |window| {
                self.child.prepaint(window, cx);
            })
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
        let rem = self.rem();
        window.with_rem_size(Some(rem), |window| self.child.paint(window, cx));
    }
}
