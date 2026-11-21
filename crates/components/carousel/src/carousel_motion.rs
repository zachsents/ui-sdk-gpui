//! Port of `components/carousel/carousel-motion.ts`: how a carousel's
//! viewport moves. Everything the carousel does itself goes through the
//! sheet's gesture machine, with the scroll position as its extent and the
//! pages as its snap points: mouse drags, which follow the pointer and
//! spring from the release velocity to the page the projection picks, and
//! programmatic moves, which spring from wherever the slides are, at
//! whatever speed they're going. Springs write the scroll position once a
//! frame. Past either end, a drag stretches the content with a rubber band.
//!
//! The original's viewport is a native scroller with CSS scroll snap doing
//! the paging, so fingers, wheels and trackpads scroll it themselves. gpui
//! has no scroll snapping, so the carousel scrolls the slides itself: the
//! viewport clips them, the content shifts by the scroll position, wheels,
//! trackpads and touch pans move it as a browser would, and when they come
//! to rest the slides settle where mandatory scroll snap would put them: the
//! next page the way a wheel's lines head, or the page nearest where a
//! precise scroll rests, one page on at most unless snaps may be skipped.

use std::time::Duration;

use gpui::{
    Context, MouseButton, Pixels, Point, ScrollEvent, ScrollSource, Task, TouchEvent, TouchPhase,
    Window,
};
use ui_sdk_drag_and_drop::{BindDragOptions, PointerEvent, PointerKind, swallow_next_click};
use ui_sdk_kernel::InputEvent;
use ui_sdk_motion::{
    PointerType, SettleTimeOptions, SheetConfig, SheetEffect, SheetEvent, SheetPhase, SheetSnapPoint,
    SheetSnapReason, SheetState, SpringSolver, VelocityTracker, initial_state, next,
    project_distance, settle_time, solve_spring, timestamp,
};
use ui_sdk_number::clamp;
use ui_sdk_platform::prefers_reduced_motion;

use crate::carousel::Carousel;
use crate::carousel_autoplay::CarouselPause;
use crate::carousel_layout::nearest_page;

/// Which way the carousel scrolls, as the viewport lays it out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CarouselAxis {
    pub vertical: bool,
    pub rtl: bool,
}

// No scroll for this long, with no finger down, counts as rest.
const REST_TIMEOUT: Duration = Duration::from_millis(120);

// UIScrollView's normal deceleration, for where a free drag coasts to.
const FREE_DECELERATION_RATE: f64 = 0.998;

// A spring moving the slides, which a looping track's rotation can retarget.
struct Spring {
    serial: u64,
    to: f64,
    solver: SpringSolver,
    duration: f64,
    start_time: f64,
}

// The user's own scroll since it last rested: where it began, and whether a
// wheel's lines drove it, which snaps the way it heads.
#[derive(Clone, Copy, Debug)]
struct Gesture {
    from: f64,
    lines: bool,
}

/// What moves the slides while the viewport is on the page
/// (`createCarouselMotion`'s closure).
pub struct CarouselMotion {
    state: SheetState,
    spring: Option<Spring>,
    springs: u64,
    /// The scroll position, which the content shifts by.
    pub(crate) position: f64,
    /// How far past either end a drag stretches the content.
    pub(crate) overshoot: f64,
    touching: bool,
    touched: bool,
    // A trackpad's fingers are on it, so its scroll hasn't ended.
    scroll_held: bool,
    user_scrolled: bool,
    rest_missed: bool,
    rest_timer: Option<Task<()>>,
    pointer: bool,
    dragging: bool,
    mouse_dragged: bool,
    last_scroll_event: Option<InputEvent>,
    destroyed: bool,
    tracker: VelocityTracker,
    gesture: Option<Gesture>,
}

impl CarouselMotion {
    pub(crate) fn new(position: f64) -> Self {
        Self {
            state: initial_state(0.0, Some(0)),
            spring: None,
            springs: 0,
            position,
            overshoot: 0.0,
            touching: false,
            touched: false,
            scroll_held: false,
            user_scrolled: false,
            rest_missed: false,
            rest_timer: None,
            pointer: false,
            dragging: false,
            mouse_dragged: false,
            last_scroll_event: None,
            destroyed: false,
            tracker: VelocityTracker::default(),
            gesture: None,
        }
    }

    /// Whether the user holds or scrolls it: a finger, a drag, or a scroll
    /// that hasn't come to rest.
    pub fn is_held(&self) -> bool {
        self.touching || self.dragging || self.user_scrolled || self.pointer || self.scroll_held
    }

    /// Whether it's held, or a spring is moving it, so new layout shouldn't jump it.
    pub fn is_moving(&self) -> bool {
        self.is_held() || self.spring.is_some()
    }
}

fn f(pixels: Pixels) -> f64 {
    f64::from(f32::from(pixels))
}

// When an event happened, on the motion crate's input clock.
fn time_of(event: &PointerEvent) -> f64 {
    timestamp() - event.time.elapsed().as_secs_f64() * 1000.0
}

impl Carousel {
    /// The scroll position.
    pub(crate) fn motion_position(&self) -> f64 {
        self.motion.as_ref().map_or(0.0, |motion| motion.position)
    }

    /// Shows `target`: the scroll position within range, and the rest as a stretch.
    pub(crate) fn motion_write(&mut self, target: f64, cx: &mut Context<Self>) {
        let max = self.max_scroll;
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if motion.destroyed {
            return;
        }
        let scroll = clamp(target, 0.0, max);
        motion.position = scroll;
        motion.overshoot = target - scroll;
        self.on_scroll(cx);
        cx.notify();
    }

    fn motion_config(&self, positions: &[f64]) -> SheetConfig {
        SheetConfig {
            snap_points: positions
                .iter()
                .enumerate()
                .map(|(index, extent)| SheetSnapPoint {
                    id: index.to_string().into(),
                    extent: *extent,
                })
                .collect(),
            dimension: self.viewport_size,
            dismissible: false,
            expand_on_scroll: false,
            physics: self.motion_physics(),
            content_scroll: None,
        }
    }

    // The machine's idea of where it rests, refreshed after the user scrolled it.
    fn rest_here(&mut self) {
        let positions = self.motion_positions();
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        let at = motion.position;
        let index = nearest_page(positions.iter().copied(), at).unwrap_or(0);
        motion.state = initial_state(at, Some(index));
    }

    fn motion_dispatch(
        &mut self,
        event: SheetEvent,
        source: Option<&PointerEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let positions = self.motion_positions();
        let Some(motion) = self.motion.as_ref() else {
            return;
        };
        if motion.destroyed || positions.is_empty() {
            return;
        }
        let config = self.motion_config(&positions);
        let (state, effects) = next(&motion.state, &event, &config);
        if let Some(motion) = self.motion.as_mut() {
            motion.state = state;
        }
        // Before the effects, so a release that reports its page finds the drag over.
        self.motion_set_dragging(matches!(state.phase, SheetPhase::Dragging { .. }), window, cx);
        let time = match event {
            SheetEvent::Down { time, .. }
            | SheetEvent::Move { time, .. }
            | SheetEvent::Up { time, .. }
            | SheetEvent::SnapTo { time, .. }
            | SheetEvent::Close { time }
            | SheetEvent::Nudge { time } => time,
            SheetEvent::Settled | SheetEvent::Rest { .. } => timestamp(),
        };
        for effect in effects {
            self.motion_run(effect, time, source, window, cx);
        }
    }

    fn motion_set_dragging(&mut self, value: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if motion.dragging == value {
            return;
        }
        motion.dragging = value;
        self.on_drag_change(value, window, cx);
    }

    fn motion_run(
        &mut self,
        effect: SheetEffect,
        time: f64,
        source: Option<&PointerEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match effect {
            SheetEffect::Move { extent } => {
                // Catching a spring replaces it at once, so its target never flashes.
                if self.motion.as_ref().is_some_and(|motion| motion.spring.is_some()) {
                    self.stop_spring();
                    return self.motion_write(extent, cx);
                }
                // The drag session reports moves once a frame already, so
                // each lands at once.
                self.set_snapping(false, cx);
                self.motion_write(extent, cx);
            }
            SheetEffect::Settle {
                from,
                to,
                velocity,
                response,
                damping_ratio,
            } => self.start_spring(from, to, velocity, response, damping_ratio, time, window, cx),
            // Only a drag's release picks a page here. Programmatic moves already changed it.
            SheetEffect::Snap {
                id,
                reason: SheetSnapReason::Swipe,
            } => {
                if let (Some(source), Ok(index)) = (source, id.parse::<usize>()) {
                    self.on_release(index, source, window, cx);
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_spring(
        &mut self,
        from: f64,
        to: f64,
        velocity: f64,
        response: f64,
        damping_ratio: f64,
        start_time: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_spring();
        let still = (to - from).abs() < 0.5 && velocity.abs() < 0.01;
        if still || prefers_reduced_motion(cx) {
            self.motion_write(to, cx);
            return self.finish_spring(window, cx);
        }
        self.set_snapping(false, cx);
        let solver = solve_spring(response, damping_ratio, from - to, velocity);
        let duration = settle_time(&solver, SettleTimeOptions::default());
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        motion.springs += 1;
        let serial = motion.springs;
        // Its target moves when a looping track rotates under it.
        motion.spring = Some(Spring {
            serial,
            to,
            solver,
            duration,
            start_time,
        });
        self.next_spring_step(serial, window, cx);
    }

    fn next_spring_step(&mut self, serial: u64, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            entity
                .update(cx, |this, cx| this.spring_step(serial, window, cx))
                .ok();
        });
    }

    fn spring_step(&mut self, serial: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spring) = self
            .motion
            .as_ref()
            .and_then(|motion| motion.spring.as_ref())
            .filter(|spring| spring.serial == serial)
        else {
            return;
        };
        let elapsed = timestamp() - spring.start_time;
        if elapsed >= spring.duration {
            let to = spring.to;
            self.motion_write(to, cx);
            return self.finish_spring(window, cx);
        }
        let at = spring.to + spring.solver.position(elapsed.max(0.0));
        self.motion_write(at, cx);
        self.next_spring_step(serial, window, cx);
    }

    fn stop_spring(&mut self) {
        if let Some(motion) = self.motion.as_mut() {
            motion.spring = None;
        }
    }

    fn finish_spring(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_spring();
        self.motion_dispatch(SheetEvent::Settled, None, window, cx);
        if !self
            .motion
            .as_ref()
            .is_some_and(|motion| motion.state.phase == SheetPhase::Idle)
        {
            return;
        }
        // Resting exactly on a page, so snapping comes back on without a jump.
        if !self.drag_free || self.is_on_page() {
            self.set_snapping(true, cx);
        }
        self.on_settle(window, cx);
    }

    fn is_on_page(&self) -> bool {
        let at = self.motion_position();
        self.motion_positions()
            .iter()
            .any(|position| (position - at).abs() < 1.0)
    }

    /// A wheel, a trackpad or a finger's pan over the viewport: the user's
    /// own scroll, which moves the slides as a browser scrolls a scroller.
    pub(crate) fn viewport_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.drag || self.motion.is_none() {
            return;
        }
        let touch = event.source == ScrollSource::Touch;
        let precise = event.delta.precise();
        let phase = event.touch_phase;
        if phase == TouchPhase::Started {
            if touch {
                // A finger down pauses rotation; the swipe it makes stops it.
                self.set_paused(CarouselPause::Touch, true, window, cx);
                self.motion_touch_start();
            } else if precise && let Some(motion) = self.motion.as_mut() {
                motion.scroll_held = true;
            }
        }
        let vertical = self.orientation == gpui::Orientation::Vertical;
        let delta = event.delta.pixel_delta(window.line_height());
        let along = if vertical {
            delta.y
        } else if delta.x == Pixels::ZERO && event.modifiers.shift {
            delta.y
        } else {
            delta.x
        };
        // gpui's deltas point the way the content moves; the scroll position
        // runs the other way, and right to left, the same way.
        let scrolled = if !vertical && self.is_rtl { f(along) } else { -f(along) };
        if scrolled != 0.0 {
            // The viewport contains its scroll: what's around it doesn't
            // scroll along its axis (`overscroll-behavior: contain`).
            cx.stop_propagation();
            // A finger's pan is the touch behind it, which a swipe reports.
            let input = if touch {
                InputEvent::Touch(TouchEvent {
                    phase,
                    position: event.position,
                    ..TouchEvent::default()
                })
            } else {
                InputEvent::Scroll(event.clone())
            };
            self.user_scroll(scrolled, input, !precise && !touch, window, cx);
        }
        if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            if touch {
                self.set_paused(CarouselPause::Touch, false, window, cx);
                self.motion_touch_end(window, cx);
            } else if precise && let Some(motion) = self.motion.as_mut() {
                motion.scroll_held = false;
                if motion.rest_missed {
                    motion.rest_missed = false;
                    self.motion_rest(None, window, cx);
                }
            }
        }
    }

    // The browser's scroll, then its scroll event. A spring or a drag
    // overrides it on its next frame, so it's left out.
    fn user_scroll(
        &mut self,
        delta: f64,
        event: InputEvent,
        lines: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let max = self.max_scroll;
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if motion.spring.is_some() || motion.dragging {
            return;
        }
        let at = clamp(motion.position + delta, 0.0, max);
        if at == motion.position {
            return;
        }
        match &mut motion.gesture {
            Some(gesture) => gesture.lines |= lines,
            None => {
                motion.gesture = Some(Gesture {
                    from: motion.position,
                    lines,
                })
            }
        }
        motion.position = at;
        motion.overshoot = 0.0;
        motion.user_scrolled = true;
        motion.last_scroll_event = Some(event);
        let rest = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(REST_TIMEOUT).await;
            this.update_in(cx, |this, window, cx| this.motion_rest(None, window, cx))
                .ok();
        });
        motion.rest_timer = Some(rest);
        self.on_scroll(cx);
        cx.notify();
    }

    fn motion_rest(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if motion.spring.is_some() || motion.dragging || !motion.user_scrolled {
            return;
        }
        // The scroll can end before the finger lifts.
        if motion.touching || motion.scroll_held {
            motion.rest_missed = true;
            return;
        }
        motion.user_scrolled = false;
        let touch = std::mem::take(&mut motion.touched);
        let gesture = motion.gesture.take();
        let event = event.or_else(|| motion.last_scroll_event.clone());
        self.rest_here();
        let at = self.motion_position();
        let position = if self.drag_free {
            at
        } else {
            self.snapped(at, gesture)
        };
        self.on_scroll_rest(position, touch, event, window, cx);
    }

    // Where mandatory scroll snap would settle a scroll that rests at `at`:
    // the next page the way a wheel's lines head, or the page nearest a
    // precise scroll's rest, stopping at the first page it passes unless
    // snaps may be skipped (`scroll-snap-stop: always`).
    fn snapped(&self, at: f64, gesture: Option<Gesture>) -> f64 {
        let positions = self.motion_positions();
        let Some(nearest) = nearest_page(positions.iter().copied(), at).map(|index| positions[index]) else {
            return at;
        };
        let Some(gesture) = gesture else {
            return nearest;
        };
        let moved = at - gesture.from;
        if moved.abs() < 0.5 {
            return nearest;
        }
        let ahead = positions.iter().copied().filter(|position| {
            if moved > 0.0 {
                *position > gesture.from + 0.5
            } else {
                *position < gesture.from - 0.5
            }
        });
        let next = if moved > 0.0 {
            ahead.fold(None, |best: Option<f64>, position| Some(best.map_or(position, |best| best.min(position))))
        } else {
            ahead.fold(None, |best: Option<f64>, position| Some(best.map_or(position, |best| best.max(position))))
        };
        if gesture.lines {
            return next.unwrap_or(nearest);
        }
        match next {
            Some(next) if !self.skip_snaps && (nearest - next) * moved > 0.0 => next,
            _ => nearest,
        }
    }

    fn motion_touch_start(&mut self) {
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        motion.touching = true;
        motion.touched = true;
        motion.rest_missed = false;
        // A finger catches a spring where it is. The gesture rests, then the
        // carousel settles it on a page.
        if motion.spring.is_some() {
            motion.spring = None;
            motion.user_scrolled = true;
            self.rest_here();
        }
    }

    fn motion_touch_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        motion.touching = false;
        if motion.rest_missed {
            motion.rest_missed = false;
            self.motion_rest(None, window, cx);
        }
    }

    // The pointer as the machine reads it: `y` along the axis, growing the
    // way scrolling shrinks.
    fn axis_point(&self, position: Point<Pixels>) -> (f64, f64) {
        let axis = self.axis();
        let (client_x, client_y) = (f(position.x), f(position.y));
        let y = if axis.vertical {
            client_y
        } else if axis.rtl {
            -client_x
        } else {
            client_x
        };
        let x = if axis.vertical { client_x } else { client_y };
        (x, y)
    }

    /// A press on the viewport: a mouse that may drag the slides. Fingers
    /// scroll them instead, and a text field inside keeps its press, which
    /// it stops before it gets here.
    pub(crate) fn motion_pointer_down(&mut self, event: &PointerEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.kind == PointerKind::Touch || event.button != MouseButton::Left {
            return;
        }
        if self.motion.as_ref().is_none_or(|motion| motion.pointer) || !self.drag {
            return;
        }
        let drags = self.drags.clone();
        if !drags.accepts(self, event, window, cx) {
            return;
        }
        if self
            .motion
            .as_ref()
            .is_some_and(|motion| motion.state.phase == SheetPhase::Idle)
        {
            self.rest_here();
        }
        let time = time_of(event);
        let (x, y) = self.axis_point(event.position);
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        motion.pointer = true;
        motion.mouse_dragged = false;
        motion.tracker.reset();
        motion.tracker.add(time, y);
        // The pointer is followed until it lifts, anywhere, and the press
        // stays a click on what it started on until it drags.
        drags.start(
            event,
            BindDragOptions::new()
                .on_move(|this: &mut Carousel, sample, window, cx| {
                    this.motion_pointer_move(&sample.event, window, cx)
                })
                .on_end(|this, end, canceled, window, cx| this.motion_pointer_up(end, canceled, window, cx)),
            window,
            cx,
        );
        self.motion_dispatch(
            SheetEvent::Down {
                time,
                x,
                y,
                pointer: PointerType::Mouse,
            },
            Some(event),
            window,
            cx,
        );
    }

    fn motion_pointer_move(&mut self, event: &PointerEvent, window: &mut Window, cx: &mut Context<Self>) {
        let time = time_of(event);
        let (x, y) = self.axis_point(event.position);
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        motion.tracker.add(time, y);
        self.motion_dispatch(
            SheetEvent::Move {
                time,
                x,
                y,
                scroll: None,
            },
            Some(event),
            window,
            cx,
        );
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if !matches!(motion.state.phase, SheetPhase::Dragging { .. }) || motion.mouse_dragged {
            return;
        }
        // The drag wins over anything the press would do, and follows the
        // pointer outside the viewport and the window.
        motion.mouse_dragged = true;
    }

    fn motion_pointer_up(&mut self, event: &PointerEvent, canceled: bool, window: &mut Window, cx: &mut Context<Self>) {
        let time = time_of(event);
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        motion.pointer = false;
        let velocity = if canceled { 0.0 } else { motion.tracker.velocity(time) };
        let dragging = matches!(motion.state.phase, SheetPhase::Dragging { .. });
        let mouse_dragged = motion.mouse_dragged;
        if dragging && self.drag_free {
            self.release_free(event, velocity, time, window, cx);
        } else {
            self.motion_dispatch(SheetEvent::Up { time, velocity }, Some(event), window, cx);
        }
        // The press that dragged the slides isn't a click on what it started on.
        if mouse_dragged {
            swallow_next_click(false, window, cx);
        }
    }

    // A free drag coasts to where the release velocity would carry it, like a
    // scroll view, and rests there. The nearest page becomes the current one.
    fn release_free(
        &mut self,
        event: &PointerEvent,
        finger_velocity: f64,
        time: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let positions = self.motion_positions();
        let max = self.max_scroll;
        let physics = self.motion_physics();
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if !matches!(motion.state.phase, SheetPhase::Dragging { .. }) {
            return;
        }
        let velocity = -finger_velocity;
        let extent = motion.state.extent;
        let target = clamp(extent + project_distance(velocity, FREE_DECELERATION_RATE), 0.0, max);
        let index = nearest_page(positions.iter().copied(), target).unwrap_or(0);
        motion.state = SheetState {
            extent,
            resting_index: motion.state.resting_index,
            phase: SheetPhase::Settling {
                target_index: Some(index),
                from: extent,
                to: target,
                velocity,
                response: physics.response,
                damping_ratio: 1.0,
                start_time: time,
            },
        };
        self.motion_set_dragging(false, window, cx);
        self.start_spring(extent, target, velocity, physics.response, 1.0, time, window, cx);
        self.on_release(index, event, window, cx);
    }

    /// Moves to a page index: springs there from wherever it is, or jumps
    /// without `animate`. A finger or a drag holds it: its release settles
    /// it, and the carousel follows from there.
    pub(crate) fn motion_move_to(&mut self, index: usize, animate: bool, window: &mut Window, cx: &mut Context<Self>) {
        let positions = self.motion_positions();
        let Some(motion) = self.motion.as_ref() else {
            return;
        };
        if motion.destroyed || index >= positions.len() {
            return;
        }
        if motion.touching || motion.dragging || motion.pointer {
            return;
        }
        if !animate {
            self.stop_spring();
            self.motion_write(positions[index], cx);
            if let Some(motion) = self.motion.as_mut() {
                motion.state = initial_state(positions[index], Some(index));
            }
            self.set_snapping(true, cx);
            return;
        }
        if motion.state.phase == SheetPhase::Idle {
            self.rest_here();
        }
        self.motion_dispatch(
            SheetEvent::SnapTo {
                time: timestamp(),
                id: index.to_string().into(),
                reason: SheetSnapReason::Imperative,
            },
            None,
            window,
            cx,
        );
    }

    /// Moves the scroll position from `from` by `delta` after a looping
    /// track rotated by as much, so the same slides stay in view, along with
    /// a running spring. `remap` turns the old page indexes into the new ones.
    pub(crate) fn motion_shift(
        &mut self,
        from: f64,
        delta: f64,
        remap: impl Fn(Option<usize>) -> Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if motion.destroyed {
            return;
        }
        if let Some(spring) = &mut motion.spring {
            spring.to += delta;
        }
        if let SheetPhase::Settling {
            target_index,
            from: settle_from,
            to,
            ..
        } = &mut motion.state.phase
        {
            motion.state.extent += delta;
            *settle_from += delta;
            *to += delta;
            *target_index = remap(*target_index);
            motion.state.resting_index = remap(motion.state.resting_index);
        }
        let idle = motion.state.phase == SheetPhase::Idle;
        self.motion_write(from + delta, cx);
        if idle {
            self.rest_here();
        }
    }

    /// Stops moving the slides, when the viewport leaves the page.
    pub(crate) fn motion_destroy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        motion.destroyed = true;
        motion.spring = None;
        motion.rest_timer = None;
        motion.overshoot = 0.0;
        let dragging = std::mem::take(&mut motion.dragging);
        if dragging {
            self.on_drag_change(false, window, cx);
        }
    }
}
