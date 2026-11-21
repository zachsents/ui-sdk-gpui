//! Port of `components/scroll-area/scroll-area-view.ts`: the scroll area's
//! engine. It measures the viewport once per frame, keeps the view sticking
//! to the end, decides when scrollbars show, and hands pointer input on the
//! scrollbars to `scroll_area_pointer`.
//!
//! The viewport is a gpui scroll container (`overflow_scroll`,
//! `track_scroll`), so the wheel, trackpads and touch pans scroll it
//! themselves. The engine reads its [`ScrollHandle`] after each frame, the
//! way the original's frame callback reads the viewport after a scroll event,
//! and parts read the per-frame values, the original's CSS variables, as
//! they render. A browser also scrolls a focused scroller with the keyboard,
//! and gpui doesn't, so the viewport does that here too.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, FocusHandle, Hitbox, KeyDownEvent, Pixels, Point, ScrollDelta,
    ScrollEvent, ScrollHandle, ScrollSource, Size, Task, TouchPhase, Window, px,
};
use ui_sdk_focus::TabStop;
use ui_sdk_kernel::{Key, held, key_while_bound};
use ui_sdk_platform::{is_rtl, prefers_reduced_motion};

use crate::scroll_area::ScrollArea;
use crate::scroll_area_model::{
    Axis, AxisMetrics, EDGE_TOLERANCE, ScrollAreaType, ScrollbarSignals, edges_of, follows_end,
    main_axis, max_scroll, overflows, page_length, scrollbar_shown, scrolls_toward_start,
};

/// What the scroll area holds, as its parts and callbacks read it (the
/// original's store).
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollAreaModel {
    pub r#type: ScrollAreaType,
    pub scroll_hide_delay: Duration,
    pub stick_to_end: bool,
    /// How many scrollbar parts of each axis are on the page.
    pub scrollbars_x: usize,
    pub scrollbars_y: usize,
    pub overflow_x: bool,
    pub overflow_y: bool,
    pub at_x_start: bool,
    pub at_x_end: bool,
    pub at_y_start: bool,
    pub at_y_end: bool,
    pub scrolling_x: bool,
    pub scrolling_y: bool,
    pub hovering: bool,
    pub visible_x: bool,
    pub visible_y: bool,
    pub dragging: Option<Axis>,
    pub sticking: bool,
}

impl ScrollAreaModel {
    pub(crate) fn overflow(&self, axis: Axis) -> bool {
        match axis {
            Axis::X => self.overflow_x,
            Axis::Y => self.overflow_y,
        }
    }

    pub(crate) fn scrolling(&self, axis: Axis) -> bool {
        match axis {
            Axis::X => self.scrolling_x,
            Axis::Y => self.scrolling_y,
        }
    }

    pub(crate) fn visible(&self, axis: Axis) -> bool {
        match axis {
            Axis::X => self.visible_x,
            Axis::Y => self.visible_y,
        }
    }

    fn set_overflow(&mut self, axis: Axis, value: bool) {
        match axis {
            Axis::X => self.overflow_x = value,
            Axis::Y => self.overflow_y = value,
        }
    }

    fn set_at_start(&mut self, axis: Axis, value: bool) {
        match axis {
            Axis::X => self.at_x_start = value,
            Axis::Y => self.at_y_start = value,
        }
    }

    fn set_at_end(&mut self, axis: Axis, value: bool) {
        match axis {
            Axis::X => self.at_x_end = value,
            Axis::Y => self.at_y_end = value,
        }
    }

    fn set_scrolling(&mut self, axis: Axis, value: bool) {
        match axis {
            Axis::X => self.scrolling_x = value,
            Axis::Y => self.scrolling_y = value,
        }
    }

    fn set_visible(&mut self, axis: Axis, value: bool) {
        match axis {
            Axis::X => self.visible_x = value,
            Axis::Y => self.visible_y = value,
        }
    }

    /// Whether the view is at the start: the top, or the inline start when
    /// only the horizontal axis overflows.
    pub fn at_start(&self) -> bool {
        match main_axis(self.overflow_x, self.overflow_y) {
            Axis::X => self.at_x_start,
            Axis::Y => self.at_y_start,
        }
    }

    /// Whether the view is at the end: the bottom, or the inline end when
    /// only the horizontal axis overflows.
    pub fn at_end(&self) -> bool {
        match main_axis(self.overflow_x, self.overflow_y) {
            Axis::X => self.at_x_end,
            Axis::Y => self.at_y_end,
        }
    }

    /// Whether either scrollbar part is on the page.
    pub fn has_scrollbars(&self) -> bool {
        self.scrollbars_x + self.scrollbars_y > 0
    }

    /// The scroll-to-end button hides at the end, and as soon as the view
    /// heads there.
    pub fn end_button_shown(&self) -> bool {
        !self.at_end() && !(self.stick_to_end && self.sticking)
    }
}

/// How a programmatic scroll moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ScrollAreaScrollOptions {
    /// `Smooth` eases there, following content that keeps growing. It's
    /// instant when the user prefers reduced motion. `Smooth` by default.
    pub behavior: ScrollBehavior,
}

impl ScrollAreaScrollOptions {
    pub fn behavior(mut self, behavior: ScrollBehavior) -> Self {
        self.behavior = behavior;
        self
    }
}

/// How a scroll moves: eased, or at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScrollBehavior {
    #[default]
    Smooth,
    Instant,
}

// Scroll events further apart than this end a scroll.
const SCROLL_END_DELAY: Duration = Duration::from_millis(150);

// How far an arrow key scrolls, as Chrome does.
const LINE_STEP: f64 = 40.0;

const AXES: [Axis; 2] = [Axis::X, Axis::Y];

/// The viewport's two axes as last measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    pub x: AxisMetrics,
    pub y: AxisMetrics,
}

impl Metrics {
    pub fn get(&self, axis: Axis) -> AxisMetrics {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
        }
    }
}

/// The viewport's metrics from its scroll handle: the offset now, the range
/// and size as of its last layout. gpui scrolls by a negative offset from the
/// top left; right to left, the horizontal position counts from the right
/// end of the range, the inline start.
pub fn metrics_of(handle: &ScrollHandle, rtl: bool) -> Metrics {
    let offset = handle.offset();
    let max = handle.max_offset();
    let size = handle.bounds().size;
    let (offset_x, max_x) = (f64::from(f32::from(offset.x)), f64::from(f32::from(max.x)));
    let (offset_y, max_y) = (f64::from(f32::from(offset.y)), f64::from(f32::from(max.y)));
    let (width, height) = (
        f64::from(f32::from(size.width)),
        f64::from(f32::from(size.height)),
    );
    Metrics {
        x: AxisMetrics {
            position: if rtl { max_x + offset_x } else { -offset_x },
            client: width,
            scroll: width + max_x,
        },
        y: AxisMetrics {
            position: -offset_y,
            client: height,
            scroll: height + max_y,
        },
    }
}

/// A scrollbar's box as it last laid out: its content box, where the thumb
/// travels, and its border box, whose thickness the corner takes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackBox {
    pub content: Bounds<Pixels>,
    pub border: Bounds<Pixels>,
}

/// Each axis's track, from the scrollbars' layout (their resize observers in
/// the original).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tracks {
    pub x: Option<TrackBox>,
    pub y: Option<TrackBox>,
}

impl Tracks {
    pub fn get(&self, axis: Axis) -> Option<TrackBox> {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
        }
    }

    pub fn set(&mut self, axis: Axis, track: TrackBox) {
        match axis {
            Axis::X => self.x = Some(track),
            Axis::Y => self.y = Some(track),
        }
    }

    /// The track's length.
    pub fn length(&self, axis: Axis) -> f64 {
        let Some(track) = self.get(axis) else {
            return 0.0;
        };
        let size = track.content.size;
        f64::from(f32::from(match axis {
            Axis::X => size.width,
            Axis::Y => size.height,
        }))
        .max(0.0)
    }

    /// The scrollbar's thickness: a vertical one's width, a horizontal one's height.
    pub fn thickness(&self, axis: Axis) -> f64 {
        let Some(track) = self.get(axis) else {
            return 0.0;
        };
        let size = track.border.size;
        f64::from(f32::from(match axis {
            Axis::X => size.height,
            Axis::Y => size.width,
        }))
    }
}

// What a frame showed, to tell what changed by the next.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seen {
    offset: Point<Pixels>,
    max: Point<Pixels>,
    size: Size<Pixels>,
    tracks: [f64; 4],
    rtl: bool,
}

// An ease the scroll area runs itself, on animation frames.
#[derive(Clone, Copy, Debug, PartialEq)]
enum EaseTarget {
    // The end, read every frame, since content may still be growing.
    End,
    Position(f64),
}

#[derive(Clone, Copy, Debug)]
struct Ease {
    serial: u64,
    axis: Axis,
    target: EaseTarget,
}

/// The engine's own state, what the original keeps in `createScrollView`'s
/// closure.
pub struct ScrollView {
    /// The viewport's scroll state, which the viewport part tracks.
    pub(crate) handle: ScrollHandle,
    /// The viewport's tab stop, which it holds while it scrolls and nothing
    /// inside takes focus.
    pub(crate) tab_stop: TabStop,
    /// Track lengths and thicknesses, written by the scrollbars as they lay out.
    pub(crate) tracks: Rc<RefCell<Tracks>>,
    /// The root's hitbox, which tells whether the pointer is over it.
    pub(crate) hitbox: Rc<RefCell<Option<Hitbox>>>,
    /// Set when a render asked gpui to keep the end in view, so the scroll
    /// that follows isn't taken for the user's.
    pub(crate) pinned: Rc<Cell<bool>>,
    pub(crate) rtl: bool,
    /// The last measurement, reused between frames.
    pub(crate) metrics: Metrics,
    previous: [f64; 2],
    seen: Option<Seen>,
    // Where sticking to the end last scrolled the viewport, to tell its own scroll apart.
    stuck_to: f64,
    // Whether a finger or a button is down in the viewport, when growth must not move the content.
    pub(crate) touching: bool,
    pub(crate) pressed: bool,
    easing: Option<Ease>,
    eases: u64,
    scroll_ends: [Option<Task<()>>; 2],
    last_scroll: [Option<Instant>; 2],
    hide_timers: [Option<Task<()>>; 2],
    // Right to left, the view has been put at the inline start.
    placed: bool,
}

fn index(axis: Axis) -> usize {
    match axis {
        Axis::X => 0,
        Axis::Y => 1,
    }
}

impl ScrollView {
    pub(crate) fn new(focus: FocusHandle) -> Self {
        Self {
            handle: ScrollHandle::new(),
            tab_stop: ui_sdk_focus::bind_tab_stop(focus),
            tracks: Rc::new(RefCell::new(Tracks::default())),
            hitbox: Rc::new(RefCell::new(None)),
            pinned: Rc::new(Cell::new(false)),
            rtl: false,
            metrics: Metrics::default(),
            previous: [f64::NAN; 2],
            seen: None,
            stuck_to: f64::NAN,
            touching: false,
            pressed: false,
            easing: None,
            eases: 0,
            scroll_ends: [None, None],
            last_scroll: [None, None],
            hide_timers: [None, None],
            placed: false,
        }
    }

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        self.tab_stop.handle()
    }
}

impl ScrollArea {
    pub(crate) fn viewport_bound(&self) -> bool {
        held(&self.instance, "viewport", None)
    }

    pub(crate) fn track(&self, axis: Axis) -> f64 {
        self.view.tracks.borrow().length(axis)
    }

    /// Reads the viewport. Right to left, the horizontal position counts
    /// from the inline start.
    pub(crate) fn measure(&mut self, cx: &App) {
        self.view.rtl = is_rtl(cx);
        self.view.metrics = metrics_of(&self.view.handle, self.view.rtl);
    }

    /// Reads the direction again, for horizontal input as it starts.
    pub(crate) fn read_direction(&mut self, cx: &App) {
        self.view.rtl = is_rtl(cx);
    }

    /// Measures, then patches what changed: overflow, edges, scrolling and
    /// sticking.
    pub(crate) fn update(&mut self, resized: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.viewport_bound() {
            return;
        }
        self.measure(cx);
        if self.view.pinned.replace(false) {
            self.view.stuck_to = max_scroll(&self.view.metrics.y);
        }
        if resized {
            self.follow(cx);
        }
        let state = self.model.clone();
        for axis in AXES {
            let metrics = self.view.metrics.get(axis);
            let previous = self.view.previous[index(axis)];
            let delta = if previous.is_nan() { 0.0 } else { metrics.position - previous };
            self.view.previous[index(axis)] = metrics.position;
            let edges = edges_of(&metrics);
            self.model.set_overflow(axis, overflows(&metrics));
            self.model.set_at_start(axis, edges.at_start);
            self.model.set_at_end(axis, edges.at_end);
            if delta == 0.0 {
                continue;
            }
            // Sticking to the end isn't scrolling: streaming shouldn't flash the scrollbars.
            let own_scroll =
                axis == Axis::Y && (metrics.position - self.view.stuck_to).abs() < EDGE_TOLERANCE;
            if !own_scroll {
                self.view.last_scroll[index(axis)] = Some(Instant::now());
                self.model.set_scrolling(axis, true);
                if self.view.scroll_ends[index(axis)].is_none() {
                    self.arm_scroll_end(axis, SCROLL_END_DELAY, window, cx);
                }
            }
            if axis == Axis::Y && state.stick_to_end {
                self.model.sticking = follows_end(state.sticking, delta, edges.after);
            }
        }
        let patched = self.model != state;
        if patched {
            cx.notify();
        }
        // Scrolling only matters to visibility when a flag flipped.
        if patched || resized {
            self.refresh_visibility(window, cx);
        }
    }

    fn arm_scroll_end(&mut self, axis: Axis, delay: Duration, window: &mut Window, cx: &mut Context<Self>) {
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| this.end_scroll(axis, window, cx))
                .ok();
        });
        self.view.scroll_ends[index(axis)] = Some(task);
    }

    // Scrolling ends once scroll events stop for a moment. One timer per
    // axis, armed again only when it fires early, so scrolling doesn't churn
    // timers.
    fn end_scroll(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let elapsed = self.view.last_scroll[index(axis)].map_or(SCROLL_END_DELAY, |at| at.elapsed());
        if elapsed < SCROLL_END_DELAY {
            self.arm_scroll_end(axis, SCROLL_END_DELAY - elapsed, window, cx);
            return;
        }
        self.view.scroll_ends[index(axis)] = None;
        if self.model.scrolling(axis) {
            self.model.set_scrolling(axis, false);
            cx.notify();
        }
        self.refresh_visibility(window, cx);
    }

    /// Shows a scrollbar at once, and hides it after the delay once nothing
    /// asks for it.
    pub(crate) fn refresh_visibility(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for axis in AXES {
            let shown = scrollbar_shown(
                self.model.r#type,
                ScrollbarSignals {
                    overflow: self.model.overflow(axis),
                    hovering: self.model.hovering,
                    scrolling: self.model.scrolling(axis),
                    dragging: self.model.dragging == Some(axis),
                },
            );
            if shown {
                self.view.hide_timers[index(axis)] = None;
                if !self.model.visible(axis) {
                    self.model.set_visible(axis, true);
                    cx.notify();
                }
                continue;
            }
            if !self.model.visible(axis) || self.view.hide_timers[index(axis)].is_some() {
                continue;
            }
            let delay = match self.model.r#type {
                ScrollAreaType::Scroll | ScrollAreaType::Hover => self.model.scroll_hide_delay,
                ScrollAreaType::Auto | ScrollAreaType::Always => Duration::ZERO,
            };
            if delay.is_zero() {
                self.model.set_visible(axis, false);
                cx.notify();
            } else {
                let task = cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    this.update_in(cx, |this, window, cx| {
                        this.view.hide_timers[index(axis)] = None;
                        if this.model.visible(axis) {
                            this.model.set_visible(axis, false);
                            cx.notify();
                        }
                        this.refresh_visibility(window, cx);
                    })
                    .ok();
                });
                self.view.hide_timers[index(axis)] = Some(task);
            }
        }
    }

    /// Whether a render should keep the end in view: sticking to it, and not
    /// while a finger or a button is down or the thumb is held, so selecting
    /// text or a touch scroll isn't pulled away, and not while easing there
    /// already.
    pub(crate) fn following(&self) -> bool {
        self.model.stick_to_end
            && self.model.sticking
            && self.model.dragging.is_none()
            && !self.view.pressed
            && !self.view.touching
            && self.view.easing.is_none()
    }

    /// Whether nothing moved the view since the last frame measured it, so a
    /// render can keep the end in view without undoing a scroll the frame
    /// hasn't seen.
    pub(crate) fn unmoved(&self) -> bool {
        self.view
            .seen
            .is_none_or(|seen| seen.offset == self.view.handle.offset())
    }

    /// Sticking to the end, growth keeps the end in view.
    pub(crate) fn follow(&mut self, cx: &mut Context<Self>) {
        if !self.following() {
            return;
        }
        let max = max_scroll(&self.view.metrics.y);
        if max - self.view.metrics.y.position < EDGE_TOLERANCE {
            return;
        }
        self.view.stuck_to = max;
        let mut offset = self.view.handle.offset();
        offset.y = px(-max as f32);
        self.view.handle.set_offset(offset);
        self.view.metrics.y.position = max;
        cx.notify();
    }

    /// Scrolls the viewport itself, never its ancestors, so the page stays put.
    pub(crate) fn scroll_to(&mut self, axis: Axis, position: f64, cx: &mut Context<Self>) {
        let metrics = self.view.metrics.get(axis);
        let position = position.max(0.0).min(max_scroll(&metrics));
        let max = self.view.handle.max_offset();
        let mut offset = self.view.handle.offset();
        match axis {
            Axis::Y => offset.y = px(-position as f32),
            Axis::X if self.view.rtl => offset.x = px(position as f32) - max.x,
            Axis::X => offset.x = px(-position as f32),
        }
        if offset == self.view.handle.offset() {
            return;
        }
        self.view.handle.set_offset(offset);
        match axis {
            Axis::X => self.view.metrics.x.position = position,
            Axis::Y => self.view.metrics.y.position = position,
        }
        cx.notify();
    }

    /// Input that scrolls toward the start lets go of the end before the
    /// scroll lands, so growth in the same frame can't pull the view back down.
    pub(crate) fn release(&mut self, cx: &mut Context<Self>) {
        self.stop_easing();
        if self.model.sticking && self.view.metrics.y.position > 0.0 {
            self.model.sticking = false;
            cx.notify();
        }
    }

    pub(crate) fn stop_easing(&mut self) {
        self.view.easing = None;
    }

    // Eases toward a target on animation frames: a fifth of the way each
    // frame, at least a pixel. The end is read every frame, since content may
    // still be growing. The original leaves eases to a position to the
    // browser's smooth scrolling.
    fn ease(&mut self, axis: Axis, target: EaseTarget, window: &mut Window, cx: &mut Context<Self>) {
        self.view.eases += 1;
        let ease = Ease {
            serial: self.view.eases,
            axis,
            target,
        };
        self.view.easing = Some(ease);
        self.next_ease_step(ease.serial, window, cx);
    }

    fn next_ease_step(&mut self, serial: u64, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            entity
                .update(cx, |this, cx| this.ease_step(serial, window, cx))
                .ok();
        });
    }

    fn ease_step(&mut self, serial: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ease) = self.view.easing.filter(|ease| ease.serial == serial) else {
            return;
        };
        if !self.viewport_bound() {
            self.view.easing = None;
            return;
        }
        self.measure(cx);
        let metrics = self.view.metrics.get(ease.axis);
        let target = match ease.target {
            EaseTarget::End => max_scroll(&metrics),
            EaseTarget::Position(position) => position.max(0.0).min(max_scroll(&metrics)),
        };
        let gap = target - metrics.position;
        if gap.abs() < EDGE_TOLERANCE {
            self.view.easing = None;
            self.scroll_to(ease.axis, target, cx);
            self.update(false, window, cx);
            return;
        }
        let step = (gap.abs() * 0.2).max(1.0) * gap.signum();
        self.scroll_to(ease.axis, metrics.position + step, cx);
        self.next_ease_step(serial, window, cx);
    }

    /// Scrolls to an edge of the main axis.
    pub(crate) fn scroll_to_edge(
        &mut self,
        end: bool,
        options: ScrollAreaScrollOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.viewport_bound() {
            return;
        }
        let axis = main_axis(self.model.overflow_x, self.model.overflow_y);
        self.measure(cx);
        let smooth = options.behavior == ScrollBehavior::Smooth && !prefers_reduced_motion(cx);
        self.stop_easing();
        if end && axis == Axis::Y && self.model.stick_to_end && !self.model.sticking {
            self.model.sticking = true;
            cx.notify();
        }
        if end && axis == Axis::Y && smooth {
            return self.ease(Axis::Y, EaseTarget::End, window, cx);
        }
        let target = if end {
            max_scroll(&self.view.metrics.get(axis))
        } else {
            0.0
        };
        if smooth {
            self.ease(axis, EaseTarget::Position(target), window, cx);
        } else {
            self.scroll_to(axis, target, cx);
        }
    }

    /// A scroll event on the viewport, after gpui scrolled it: a finger's
    /// pan, or a wheel or trackpad.
    pub(crate) fn viewport_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.source == ScrollSource::Touch {
            return self.viewport_touch(event, window, cx);
        }
        // gpui's deltas point the way the content moves, a wheel's the way
        // the view does.
        let wheel_y = -f32::from(event.delta.pixel_delta(window.line_height()).y);
        if wheel_y >= 0.0 {
            self.stop_easing();
        } else {
            self.release(cx);
        }
    }

    // A finger on the viewport: down, moving, or lifted. A finger moving down
    // scrolls the content toward its start.
    fn viewport_touch(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.touch_phase {
            TouchPhase::Started => self.view.touching = true,
            TouchPhase::Moved => {
                let ScrollDelta::Pixels(delta) = event.delta else {
                    return;
                };
                if self.view.touching && delta.y > px(0.) {
                    self.release(cx);
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if self.view.touching {
                    self.view.touching = false;
                    self.update(true, window, cx);
                }
            }
        }
    }

    /// A button pressed in the viewport: growth doesn't move the content
    /// until it lifts.
    pub(crate) fn viewport_press(&mut self) {
        self.view.pressed = true;
        self.stop_easing();
    }

    /// The button lifted, anywhere.
    pub(crate) fn lift(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.view.pressed {
            return;
        }
        self.view.pressed = false;
        self.update(true, window, cx);
    }

    /// A key on the viewport or inside it. Keys that scroll toward the start
    /// let go of the end, unless a widget inside used them. The keys a
    /// browser scrolls a focused scroller with scroll it here: arrows by a
    /// line, Page Up and Down by a page, Home and End to the ends of the main
    /// axis, and Space and Shift-Space by a page on the viewport itself, so a
    /// button inside keeps its press.
    pub(crate) fn viewport_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // A widget inside, like a listbox, used the key.
        if window.default_prevented() {
            return;
        }
        let key = Key::of(&event.keystroke);
        let modifiers = event.keystroke.modifiers;
        let editing = is_editing(window);
        if scrolls_toward_start(&key, modifiers.shift, editing) {
            self.release(cx);
        }
        if editing || modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return;
        }
        self.measure(cx);
        let own = self.view.focus_handle().is_focused(window);
        let rtl = self.view.rtl;
        let x = self.view.metrics.x;
        let y = self.view.metrics.y;
        let main = main_axis(self.model.overflow_x, self.model.overflow_y);
        let (axis, target) = match key {
            Key::ArrowDown => (Axis::Y, y.position + LINE_STEP),
            Key::ArrowUp => (Axis::Y, y.position - LINE_STEP),
            Key::ArrowRight => (Axis::X, x.position + if rtl { -LINE_STEP } else { LINE_STEP }),
            Key::ArrowLeft => (Axis::X, x.position + if rtl { LINE_STEP } else { -LINE_STEP }),
            Key::PageDown => (Axis::Y, y.position + page_length(y.client)),
            Key::PageUp => (Axis::Y, y.position - page_length(y.client)),
            Key::Space if own && modifiers.shift => (Axis::Y, y.position - page_length(y.client)),
            Key::Space if own => (Axis::Y, y.position + page_length(y.client)),
            Key::Home => (main, 0.0),
            Key::End => (main, max_scroll(&self.view.metrics.get(main))),
            _ => return,
        };
        let before = self.view.handle.offset();
        self.stop_easing();
        self.scroll_to(axis, target, cx);
        // A scroller that moved takes the key; one at its edge leaves it to
        // the one around it.
        if self.view.handle.offset() != before {
            window.prevent_default();
            cx.stop_propagation();
        }
    }

    /// The pointer went over the root, or left it.
    pub(crate) fn set_hovering(&mut self, hovering: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.hovering == hovering {
            return;
        }
        self.model.hovering = hovering;
        cx.notify();
        self.refresh_visibility(window, cx);
    }

    // How many scrollbar parts of each axis are on the page.
    fn count_scrollbars(&mut self, cx: &mut Context<Self>) {
        let keys = key_while_bound(&self.instance, "scrollbar");
        let x = keys.iter().filter(|key| key.as_ref() == Axis::X.as_str()).count();
        let y = keys.iter().filter(|key| key.as_ref() == Axis::Y.as_str()).count();
        if (x, y) != (self.model.scrollbars_x, self.model.scrollbars_y) {
            self.model.scrollbars_x = x;
            self.model.scrollbars_y = y;
            cx.notify();
        }
    }

    pub(crate) fn set_dragging(&mut self, dragging: Option<Axis>, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.dragging != dragging {
            self.model.dragging = dragging;
            cx.notify();
        }
        self.refresh_visibility(window, cx);
    }

    /// Whether the view is at an edge now. The state catches up after the
    /// frame, so a handler reading it would get where the view was. `None`
    /// without a viewport.
    pub(crate) fn is_at(&self, end: bool, cx: &App) -> Option<bool> {
        if !self.viewport_bound() {
            return None;
        }
        let metrics = metrics_of(&self.view.handle, is_rtl(cx));
        let axis = main_axis(overflows(&metrics.x), overflows(&metrics.y));
        let edges = edges_of(&metrics.get(axis));
        Some(if end { edges.at_end } else { edges.at_start })
    }

    /// Moves focus to the viewport. gpui focuses a tracked element whether
    /// or not it's a tab stop.
    pub(crate) fn focus_viewport(&self, window: &mut Window, cx: &mut App) {
        window.focus(self.view.focus_handle(), cx);
    }

    /// Applies changed options, like `stick_to_end` or `type`.
    pub(crate) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.viewport_bound() {
            self.update(true, window, cx);
        }
        self.refresh_visibility(window, cx);
    }

    /// Runs after each frame that draws the scroll area, the stand-in for the
    /// original's resize and mutation observers and its frame callback: what
    /// the frame showed is measured, and what changed is patched.
    pub(crate) fn after_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The pointer may already be over the root when it mounts, or the
        // layout moved under a still pointer.
        let hitbox = self.view.hitbox.borrow().clone();
        if let Some(hitbox) = hitbox
            && !window.last_input_was_touch()
        {
            let hovering = hitbox.is_hovered_at(window.mouse_position(), window);
            self.set_hovering(hovering, window, cx);
        }
        self.count_scrollbars(cx);
        // A part whose press is held left the page.
        if let Some(holder) = self.input.holder
            && !held(&self.instance, holder.slot(), Some(holder.axis().as_str()))
        {
            self.end_press(holder, window, cx);
        }
        if !self.viewport_bound() {
            self.view.seen = None;
            return;
        }
        let rtl = is_rtl(cx);
        let handle = &self.view.handle;
        let tracks = *self.view.tracks.borrow();
        let seen = Seen {
            offset: handle.offset(),
            max: handle.max_offset(),
            size: handle.bounds().size,
            tracks: [
                tracks.length(Axis::X),
                tracks.length(Axis::Y),
                tracks.thickness(Axis::X),
                tracks.thickness(Axis::Y),
            ],
            rtl,
        };
        if self.view.seen == Some(seen) {
            return;
        }
        let before = self.view.seen.replace(seen);
        let resized = before.is_none_or(|before| {
            before.max != seen.max || before.size != seen.size || before.rtl != seen.rtl
        });
        // Right to left, the view starts at the inline start, the right end
        // of gpui's range, where a browser starts a scroller.
        if !rtl {
            self.view.placed = false;
        } else if !self.view.placed && seen.max.x > px(0.) {
            self.view.placed = true;
            if seen.offset.x == px(0.) {
                let mut offset = seen.offset;
                offset.x = -seen.max.x;
                self.view.handle.set_offset(offset);
                self.view.previous[index(Axis::X)] = f64::NAN;
                cx.notify();
            }
        }
        // A track that just got its length sizes its thumb.
        if before.is_none_or(|before| before.tracks != seen.tracks) {
            cx.notify();
        }
        self.update(resized, window, cx);
    }
}

/// Whether a text field has focus, where keys move the caret instead.
fn is_editing(window: &Window) -> bool {
    ui_sdk_native::is_typing(window)
}
