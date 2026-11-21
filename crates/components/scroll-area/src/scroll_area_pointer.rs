//! Port of `components/scroll-area/scroll-area-pointer.ts`: pointer input on
//! a scroll area's scrollbars: dragging the thumb, pressing the track, and
//! wheels over them. The engine in `scroll_area_view` owns the measurements
//! and hands them over; the scroll area is the original's `ScrollbarHost`.

use gpui::{Context, Pixels, Point, ScrollEvent, Task, Window};
use ui_sdk_drag_and_drop::{
    BindDragOptions, DragSession, PointerEvent, PointerPhase, RepeatOptions, repeat_while_held,
};
use ui_sdk_kernel::held;

use crate::scroll_area::ScrollArea;
use crate::scroll_area_model::{
    Axis, AxisMetrics, EDGE_TOLERANCE, WheelMode, dragged_position, max_scroll, page_direction,
    page_length, position_at, thumb_geometry, wheel_pixels,
};

/// The part whose drag or press is held, so a part leaving the page
/// mid-gesture ends its own instead of following the pointer until it lifts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Holder {
    Thumb(Axis),
    Scrollbar(Axis),
}

impl Holder {
    pub(crate) fn slot(self) -> &'static str {
        match self {
            Holder::Thumb(_) => "thumb",
            Holder::Scrollbar(_) => "scrollbar",
        }
    }

    pub(crate) fn axis(self) -> Axis {
        match self {
            Holder::Thumb(axis) | Holder::Scrollbar(axis) => axis,
        }
    }
}

// A press on the track paging toward the pointer while it's held.
#[derive(Clone, Copy, Debug)]
struct Paging {
    axis: Axis,
    point: f64,
    direction: i32,
    page: f64,
    max: f64,
    target: f64,
}

/// The scrollbars' pointer input: one drag at a time, the part it holds,
/// and a held press's paging (`createScrollbarInput`'s closure).
pub struct ScrollbarInput {
    pub(crate) drags: DragSession<ScrollArea>,
    pub(crate) holder: Option<Holder>,
    paging: Option<Paging>,
    repeat: Option<Task<()>>,
}

impl ScrollbarInput {
    pub(crate) fn new(cx: &Context<ScrollArea>) -> Self {
        Self {
            drags: DragSession::new(cx),
            holder: None,
            paging: None,
            repeat: None,
        }
    }
}

impl ScrollArea {
    /// Ended where it is: only Escape puts the content back.
    pub(crate) fn end_press(&mut self, holder: Holder, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.holder != Some(holder) {
            return;
        }
        self.input.holder = None;
        let drags = self.input.drags.clone();
        drags.cancel(self, window, cx);
    }

    /// A wheel over a scrollbar outside the viewport scrolls the viewport, as
    /// far as it would over the content, and lets what's around it scroll
    /// once the viewport can't. gpui already scrolls the viewport for a wheel
    /// over a scrollbar that sits over it, as scrollbars do.
    pub(crate) fn forward_wheel(
        &mut self,
        axis: Axis,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A hidden scrollbar leaves the wheel to what's under it.
        if !self.viewport_bound() || !self.model.visible(axis) || event.modifiers.control {
            return;
        }
        if self.view.handle.bounds().contains(&event.position) {
            return;
        }
        let delta = event.delta.pixel_delta(window.line_height());
        if delta.x != Pixels::ZERO {
            self.read_direction(cx);
        }
        self.measure(cx);
        let metrics = self.view.metrics;
        // gpui's deltas point the way the content moves, a wheel's the way
        // the view does.
        let mode = if event.delta.precise() {
            WheelMode::Pixel
        } else {
            WheelMode::Line
        };
        let (lines_x, lines_y) = match event.delta {
            gpui::ScrollDelta::Pixels(pixels) => (-f64::from(f32::from(pixels.x)), -f64::from(f32::from(pixels.y))),
            gpui::ScrollDelta::Lines(lines) => (-f64::from(lines.x), -f64::from(lines.y)),
        };
        let line_height = f64::from(f32::from(window.line_height()));
        let dx = wheel_pixels(lines_x, mode, line_height, metrics.x.client);
        let dy = wheel_pixels(lines_y, mode, line_height, metrics.y.client);
        let rtl = self.view.rtl;
        let can_scroll = |delta: f64, axis: Axis| {
            let metrics = metrics.get(axis);
            // Right to left, a positive horizontal delta moves toward the start.
            let toward = if axis == Axis::X && rtl { -delta } else { delta };
            if toward < 0.0 {
                return metrics.position > 0.0;
            }
            toward > 0.0 && max_scroll(&metrics) - metrics.position >= EDGE_TOLERANCE
        };
        if !can_scroll(dx, Axis::X) && !can_scroll(dy, Axis::Y) {
            return;
        }
        cx.stop_propagation();
        if dy < 0.0 {
            self.release(cx);
        } else {
            self.stop_easing();
        }
        let x = metrics.x.position + if rtl { -dx } else { dx };
        let y = metrics.y.position + dy;
        self.scroll_to(Axis::X, x, cx);
        self.scroll_to(Axis::Y, y, cx);
    }

    /// A press on a thumb drags it.
    pub(crate) fn press_thumb(
        &mut self,
        axis: Axis,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A hidden scrollbar's thumb leaves the press to what's under it.
        if !self.model.visible(axis) {
            return;
        }
        let drags = self.input.drags.clone();
        if !self.viewport_bound() || !drags.accepts(self, event, window, cx) {
            return;
        }
        // The track's own press handler would page.
        cx.stop_propagation();
        window.prevent_default();
        if axis == Axis::X {
            self.read_direction(cx);
        }
        self.measure(cx);
        let start = self.view.metrics.get(axis).position;
        self.drag(Holder::Thumb(axis), event, axis, start, window, cx);
    }

    // The thumb moves as far as the pointer does. Escape puts the content
    // back where the drag started, so a drag can be called off (WCAG 2.5.2).
    fn drag(
        &mut self,
        holder: Holder,
        down: &PointerEvent,
        axis: Axis,
        start: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let along = move |point: Point<Pixels>| {
            f64::from(f32::from(match axis {
                Axis::X => point.x,
                Axis::Y => point.y,
            }))
        };
        let from = along(down.position);
        self.stop_easing();
        self.set_dragging(Some(axis), window, cx);
        self.input.holder = Some(holder);
        let drags = self.input.drags.clone();
        drags.start(
            down,
            BindDragOptions::new()
                .cancel_on_escape(true)
                .on_move(move |this: &mut ScrollArea, sample, _window, cx| {
                    let distance = along(sample.position()) - from;
                    let delta = if axis == Axis::X && this.view.rtl {
                        -distance
                    } else {
                        distance
                    };
                    this.measure(cx);
                    let metrics = this.view.metrics.get(axis);
                    let position = dragged_position(start, delta, &metrics, this.track(axis));
                    this.scroll_to(axis, position, cx);
                })
                // Only Escape puts the content back. It ends with the last
                // move; a lost pointer or a new press ends with its own event.
                .on_end(move |this, end, canceled, window, cx| {
                    if canceled && this.input.holder == Some(holder) && end.phase == PointerPhase::Move {
                        this.measure(cx);
                        this.scroll_to(axis, start, cx);
                    }
                    this.input.holder = None;
                    this.set_dragging(None, window, cx);
                }),
            window,
            cx,
        );
    }

    /// A press on the track pages toward the pointer, and keeps paging while
    /// it's held, until the thumb reaches the pointer. With Alt (Option on a
    /// Mac) it jumps there and drags instead, like macOS.
    pub(crate) fn press_track(
        &mut self,
        axis: Axis,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let drags = self.input.drags.clone();
        if !self.viewport_bound() || !drags.accepts(self, event, window, cx) {
            return;
        }
        if axis == Axis::X {
            self.read_direction(cx);
        }
        self.measure(cx);
        let point = self.track_point(axis, event.position);
        let track = self.track(axis);
        let metrics = self.view.metrics.get(axis);
        if event.modifiers.alt {
            let position = position_at(point, &metrics, track);
            self.scroll_to(axis, position, cx);
            if held(&self.instance, "thumb", Some(axis.as_str())) {
                self.drag(Holder::Thumb(axis), event, axis, position, window, cx);
            }
            return;
        }
        let direction = page_direction(point, thumb_geometry(&metrics, track));
        if direction == 0 {
            return;
        }
        if axis == Axis::Y && direction < 0 {
            self.release(cx);
        }
        self.input.paging = Some(Paging {
            axis,
            point,
            direction,
            page: page_length(metrics.client),
            max: max_scroll(&metrics),
            target: metrics.position,
        });
        let repeat = repeat_while_held(
            self,
            |this: &mut ScrollArea, _window, cx| this.page_step(cx),
            RepeatOptions::default(),
            window,
            cx,
        );
        self.input.repeat = Some(repeat);
        self.input.holder = Some(Holder::Scrollbar(axis));
        drags.start(
            event,
            BindDragOptions::new()
                .on_move(move |this: &mut ScrollArea, sample, _window, _cx| {
                    let point = this.track_point(axis, sample.position());
                    if let Some(paging) = &mut this.input.paging {
                        paging.point = point;
                    }
                })
                .on_end(|this, _end, _canceled, _window, _cx| {
                    this.input.holder = None;
                    this.input.paging = None;
                    this.input.repeat = None;
                }),
            window,
            cx,
        );
    }

    // One page toward the pointer, while the thumb hasn't reached it. Pages
    // land at once: gpui has no `scroll-behavior` for them to follow.
    fn page_step(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(paging) = self.input.paging else {
            return false;
        };
        let metrics = AxisMetrics {
            position: paging.target,
            ..self.view.metrics.get(paging.axis)
        };
        let thumb = thumb_geometry(&metrics, self.track(paging.axis));
        if page_direction(paging.point, thumb) != paging.direction {
            return false;
        }
        let target = (paging.target + f64::from(paging.direction) * paging.page)
            .max(0.0)
            .min(paging.max);
        if let Some(paging) = &mut self.input.paging {
            paging.target = target;
        }
        self.scroll_to(paging.axis, target, cx);
        target > 0.0 && target < paging.max
    }

    // Where a pointer is along a scrollbar's track: from the start of its
    // content box, in the axis's direction.
    fn track_point(&self, axis: Axis, position: Point<Pixels>) -> f64 {
        let Some(track) = self.view.tracks.borrow().get(axis) else {
            return 0.0;
        };
        let content = track.content;
        let point = match axis {
            Axis::Y => position.y - content.top(),
            Axis::X if self.view.rtl => content.right() - position.x,
            Axis::X => position.x - content.left(),
        };
        f64::from(f32::from(point))
    }
}
