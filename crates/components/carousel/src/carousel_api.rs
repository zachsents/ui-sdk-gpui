//! Port of `components/carousel/carousel-api.ts`: what a carousel does:
//! paging and rotation, measuring the slides and moving them. The
//! indicators are in `carousel_indicators`, the keys in `carousel_keyboard`,
//! the seamless loop in `carousel_loop`, layout reads and slide shifts in
//! `carousel_dom`, and the parts in `carousel`.
//!
//! The original measures from resize observers and binds parts as they mount.
//! Here each frame that draws the carousel reports what it laid out, and
//! `Carousel::after_frame` measures when that changed, starts and stops
//! what the root and the viewport own as they come and go, and follows hover,
//! visibility and focus.

use std::collections::HashMap;
use std::mem;

use gpui::{Context, SharedString, Window};
use ui_sdk_drag_and_drop::PointerEvent;
use ui_sdk_kernel::modality::focus_event;
use ui_sdk_kernel::{ChangeOptions, InputEvent, Reason, held, is_focus_from_keyboard};
use ui_sdk_motion::SheetPhysics;
use ui_sdk_number::clamp;
use ui_sdk_platform::{is_rtl, prefers_reduced_motion};

use crate::carousel::{Carousel, CarouselPageOptions, CarouselPlayingOptions, Reacted};
use crate::carousel_autoplay::CarouselPause;
use crate::carousel_dom::{LaidOut, read_carousel_layout, slide_shifts, write_slide_order};
use crate::carousel_layout::{
    CarouselGeometry, CarouselLoopView, CarouselSlideBox, carousel_pages, is_slide_in_view,
    nearest_page, step_page,
};
use crate::carousel_motion::{CarouselAxis, CarouselMotion};

/// A measured layout, read from where the parts laid out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CarouselMeasure {
    /// Each slide's key and place, in order along the axis.
    pub slides: Vec<(SharedString, CarouselSlideBox)>,
    /// The viewport's size along the axis.
    pub viewport_size: f64,
    /// The largest scroll position.
    pub max_scroll: f64,
    /// The viewport's scroll padding at each end.
    pub padding_start: f64,
    pub padding_end: f64,
    /// The slide shown first, when the slides rotate for a seamless loop.
    pub first: Option<usize>,
}

// The reasons that are the user moving the carousel, which stop rotation.
const USER_REASONS: [Reason; 6] = [
    Reason::PreviousPress,
    Reason::NextPress,
    Reason::ItemPress,
    Reason::ListNavigation,
    Reason::Swipe,
    Reason::Scroll,
];

impl Carousel {
    pub(crate) fn axis(&self) -> CarouselAxis {
        CarouselAxis {
            vertical: self.orientation == gpui::Orientation::Vertical,
            rtl: self.is_rtl,
        }
    }

    pub(crate) fn read_direction(&mut self, cx: &gpui::App) -> bool {
        self.is_rtl = is_rtl(cx);
        self.is_rtl
    }

    fn turned(&self, cx: &gpui::App) -> bool {
        self.measured && self.is_rtl != is_rtl(cx)
    }

    /// Measures again when the direction turned, and returns whether it runs
    /// right to left. The slides lay out the new way in the next frame, which
    /// measures again.
    pub(crate) fn follow_direction(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.turned(cx) {
            self.measure(window, cx);
            self.queue_measure(cx);
        }
        self.is_rtl
    }

    pub(crate) fn can_step(&self, by: i32) -> bool {
        let page = *self.page.get();
        match self.page_count {
            None => by == 1 || page > 1,
            Some(count) => step_page(page, by, count, self.r#loop) != page,
        }
    }

    // The next or previous page, wrapping with `loop`. Before the pages are
    // measured, it steps freely forward and down to the first.
    fn step(&mut self, by: i32, options: CarouselPageOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let page = *self.page.get();
        let target = match self.page_count {
            None => page as i64 + i64::from(by),
            Some(count) => step_page(page, by, count, self.r#loop) as i64,
        };
        let reason = options.reason.unwrap_or(Reason::Imperative);
        target != page as i64 && target >= 1 && self.go(target as usize, reason, options.event, Some(by), window, cx)
    }

    /// Changes the page for a reason. A change the user made stops rotation.
    /// `by` is the step that led here, which a loop moves along.
    pub(crate) fn go(
        &mut self,
        page: usize,
        reason: Reason,
        event: Option<InputEvent>,
        by: Option<i32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.follow_direction(window, cx);
        let target = match self.page_count {
            None => page.max(1),
            Some(count) => page.max(1).min(count),
        };
        if USER_REASONS.contains(&reason) && *self.playing.get() {
            let options = ChangeOptions {
                reason: Some(reason),
                source: None,
                event: event.clone(),
            };
            if self.playing.change(&self.instance, false, options, window, cx) {
                cx.notify();
            }
            self.react(window, cx);
        }
        if target == *self.page.get() {
            return false;
        }
        self.heading = by;
        let options = ChangeOptions {
            reason: Some(reason),
            source: None,
            event,
        };
        let changed = self.page.change(&self.instance, target, options, window, cx);
        if changed {
            cx.notify();
        }
        self.react(window, cx);
        self.heading = None;
        // A canceled change moves back. So does a controlled page that
        // stayed, once the app's next render has had the chance to pass it.
        if !changed {
            cx.defer_in(window, |this, window, cx| this.follow_page(true, window, cx));
        } else if *self.page.get() != target {
            self.awaited = Some(target);
            cx.notify();
        }
        changed
    }

    // Reacts to page and rotation changes from any source: a part, a key, a
    // scroll, an api call or a controlled value (the store subscription).
    pub(crate) fn react(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = Reacted {
            page: *self.page.get(),
            playing: *self.playing.get(),
            interval: self.interval,
            orientation: self.orientation,
            align: self.align,
            slides_per_page: self.slides_per_page,
            r#loop: self.r#loop,
        };
        let before = mem::replace(&mut self.reacted, state);
        if state.playing != before.playing && self.autoplay.set_running(state.playing, self.interval, window, cx) {
            cx.notify();
        }
        if state.interval != before.interval {
            cx.notify();
        }
        if state.page != before.page {
            self.autoplay.restart(self.interval, window, cx);
            // A controlled page can arrive as the options are handed over, so a
            // turn measures later, which follows the page.
            if self.turned(cx) {
                self.queue_measure(cx);
            } else {
                self.follow_page(true, window, cx);
            }
        }
        if state.slides_per_page != before.slides_per_page || state.align != before.align {
            self.queue_measure(cx);
        }
        if state.orientation != before.orientation || state.r#loop != before.r#loop {
            self.queue_measure(cx);
        }
    }

    fn set_playing_with(&mut self, playing: bool, options: CarouselPlayingOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        let changed = self.playing.change(&self.instance, playing, options, window, cx);
        if changed {
            cx.notify();
        }
        self.react(window, cx);
        changed
    }

    /// Scrolls to the current page, if the slides aren't there. A spring on
    /// its way somewhere else turns toward it; the user's own scroll or drag
    /// settles it later.
    pub(crate) fn follow_page(&mut self, animate: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(motion) = &self.motion else {
            return;
        };
        if self.seamless.is_rotating() || !self.measured || motion.is_held() || (!animate && motion.is_moving()) {
            return;
        }
        let index = self.page.get().wrapping_sub(1);
        if self.layout.get(index).is_none() {
            return;
        }
        if animate {
            self.rotate_toward(index, self.heading, window, cx);
        }
        let Some(page) = self.layout.get(index) else {
            return;
        };
        let position = clamp(page.position, 0.0, self.max_scroll);
        if (self.motion_position() - position).abs() >= 1.0 || !self.snapping {
            if let Some(visual) = self.visual.iter().position(|page| *page == index) {
                self.motion_move_to(visual, animate, window, cx);
            }
        }
    }

    /// Reads the slides' layout from what the last frame laid out.
    pub(crate) fn measure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.measure_queued = false;
        let Some(laid_out) = self.laid_out.clone() else {
            return;
        };
        self.read_direction(cx);
        let keys: Vec<SharedString> = laid_out.slides.iter().map(|(key, _)| key.clone()).collect();
        // A loop rotates the slides, unless they don't lay out as an even
        // row, and then it rewinds.
        let loops = self.r#loop && keys.len() > 1 && self.motion.is_some();
        let first = self.seamless.prepare(&keys, loops);
        let read = read_carousel_layout(&laid_out, self.axis());
        let laid: Vec<CarouselSlideBox> = read.slides.iter().map(|(_, slide)| *slide).collect();
        let ordered = match first {
            Some(first) if self.seamless.is_ordered() => write_slide_order(&laid, first),
            _ => laid.clone(),
        };
        self.shifts = if self.seamless.is_ordered() {
            slide_shifts(&keys, &laid, &ordered)
        } else {
            HashMap::new()
        };
        self.measured_from = Some(laid_out);
        self.apply_layout(
            CarouselMeasure {
                slides: keys.into_iter().zip(ordered).collect(),
                viewport_size: read.viewport_size,
                max_scroll: read.max_scroll,
                padding_start: read.padding_start,
                padding_end: read.padding_end,
                first,
            },
            window,
            cx,
        );
        // Slides that didn't rotate, or a track too short to, lay out in their own order.
        if first.is_some_and(|first| first > 0) && !self.seamless.is_active() {
            return self.measure(window, cx);
        }
        self.recenter(window, cx);
        cx.notify();
    }

    // Works out the pages from a measured layout, and keeps the current page in view.
    fn apply_layout(&mut self, measure: CarouselMeasure, window: &mut Window, cx: &mut Context<Self>) {
        let CarouselMeasure {
            slides,
            viewport_size,
            max_scroll,
            padding_start,
            padding_end,
            first,
        } = measure;
        self.viewport_size = viewport_size;
        self.max_scroll = max_scroll;
        self.boxes = slides.iter().map(|(_, slide)| *slide).collect();
        let snapport = viewport_size - padding_start - padding_end;
        let view = CarouselLoopView {
            snapport,
            padding_start,
            max_scroll,
            align: self.align,
            slides_per_page: self.slides_per_page,
        };
        self.layout = self.seamless.layout(&self.boxes, view, first).unwrap_or_else(|| {
            carousel_pages(
                CarouselGeometry {
                    slides: &self.boxes,
                    snapport,
                    padding_start,
                    max_scroll,
                },
                self.align,
                self.slides_per_page,
            )
        });
        let mut visual: Vec<usize> = (0..self.layout.len()).collect();
        visual.sort_by(|a, b| self.layout[*a].position.total_cmp(&self.layout[*b].position));
        self.visual = visual;
        let keys: Vec<SharedString> = slides.into_iter().map(|(key, _)| key).collect();
        self.page_count = (!self.layout.is_empty()).then_some(self.layout.len());
        self.pages = self
            .layout
            .iter()
            .map(|page| page.slides.iter().map(|index| keys[*index].clone()).collect())
            .collect();
        self.order = keys;
        cx.notify();
        let page = *self.page.get();
        let count = self.layout.len();
        if count > 0 && page > count {
            // The first measure picks a default, which is not a change.
            if self.measured {
                self.go(count, Reason::ContentResize, None, None, window, cx);
            } else {
                self.page.set_held(count);
                self.react(window, cx);
            }
        }
        self.measured = count > 0;
        // A rotation keeps the slides in view, and moves the scroll position itself.
        if self.seamless.is_rotating() {
            return;
        }
        // New layout keeps the current page in place, without animating.
        self.follow_page(false, window, cx);
        self.update_in_view(cx);
    }

    /// Measures once the frame that's drawing has laid the slides out.
    pub(crate) fn queue_measure(&mut self, cx: &mut Context<Self>) {
        if self.measure_queued {
            return;
        }
        self.measure_queued = true;
        cx.notify();
    }

    pub(crate) fn update_in_view(&mut self, cx: &mut Context<Self>) {
        let Some(motion) = &self.motion else {
            return;
        };
        let at = motion.position;
        let in_view: Vec<SharedString> = self
            .order
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                self.boxes
                    .get(*index)
                    .is_some_and(|slide| is_slide_in_view(slide, at, self.viewport_size))
            })
            .map(|(_, key)| key.clone())
            .collect();
        if in_view != self.in_view {
            self.in_view = in_view;
            cx.notify();
        }
    }

    /// How far the slides have scrolled: by scroll position, or round a
    /// looping track by page, since its scroll position moves with each
    /// rotation.
    pub fn progress(&self) -> f64 {
        let Some(motion) = &self.motion else {
            return 0.0;
        };
        if self.layout.is_empty() {
            return 0.0;
        }
        let at = motion.position;
        if !self.seamless.is_active() {
            return if self.max_scroll > 0.0 {
                clamp(at / self.max_scroll, 0.0, 1.0)
            } else {
                0.0
            };
        }
        // Positions keep sub-pixel offsets, and a scroll can stop a hair
        // short of one, which would read as the slide before it, all but
        // finished.
        let visual = &self.visual;
        let mut step = 0;
        while step < visual.len() - 1 && self.layout[visual[step + 1]].position <= at + 0.5 {
            step += 1;
        }
        let from = self.layout[visual[step]].position;
        let to = if step < visual.len() - 1 {
            self.layout[visual[step + 1]].position
        } else {
            from
        };
        let fraction = if to > from {
            clamp((at - from) / (to - from), 0.0, 1.0)
        } else {
            0.0
        };
        ((visual[step] as f64 + fraction) / self.layout.len() as f64) % 1.0
    }

    // The slides came to rest: a looping track recenters, and the page is complete.
    fn rested(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.motion.as_ref().is_none_or(|motion| motion.is_moving()) {
            return;
        }
        self.recenter(window, cx);
        if let Some(complete) = self.on_page_change_complete.clone() {
            complete(*self.page.get(), window, cx);
        }
    }

    /// The user's own scroll came to rest. `touch` says whether a finger drove it.
    pub(crate) fn on_scroll_rest(
        &mut self,
        position: f64,
        touch: bool,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_rtl = self.is_rtl;
        let position = if self.follow_direction(window, cx) != was_rtl && self.motion.is_some() {
            self.motion_position()
        } else {
            position
        };
        self.update_in_view(cx);
        let Some(index) = nearest_page(self.layout.iter().map(|page| page.position), position) else {
            return;
        };
        let reason = if touch { Reason::Swipe } else { Reason::Scroll };
        if index + 1 != *self.page.get() {
            self.go(index + 1, reason, event, None, window, cx);
        }
        if !self.drag_free {
            self.follow_page(true, window, cx);
        }
        self.rested(window, cx);
    }

    // In the order they rest, within the scroll range.
    pub(crate) fn motion_positions(&self) -> Vec<f64> {
        self.visual
            .iter()
            .map(|index| clamp(self.layout[*index].position, 0.0, self.max_scroll))
            .collect()
    }

    // The sheet's springs, with UIScrollView's rubber band against the
    // viewport's own size.
    pub(crate) fn motion_physics(&self) -> SheetPhysics {
        SheetPhysics::IOS
            .rubber_band(0.55)
            .rubber_band_dimension(None)
            .skip_snap_points(self.skip_snaps)
    }

    /// Snapping goes off while the carousel moves itself, and back on once it
    /// rests on a page.
    pub(crate) fn set_snapping(&mut self, snapping: bool, _cx: &mut Context<Self>) {
        self.snapping = snapping;
    }

    /// A drag was released, and it's heading for this page index.
    pub(crate) fn on_release(&mut self, index: usize, event: &PointerEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(page) = self.visual.get(index).copied() {
            self.go(page + 1, Reason::Swipe, Some(event.event.clone()), None, window, cx);
        }
    }

    pub(crate) fn on_drag_change(&mut self, dragging: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.dragging = dragging;
        cx.notify();
        self.set_paused(CarouselPause::Drag, dragging, window, cx);
    }

    /// A spring came to rest, from a drag's release or a programmatic move.
    pub(crate) fn on_settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.drag_free {
            self.follow_page(true, window, cx);
        }
        self.rested(window, cx);
    }

    /// The scroll position changed: a frame of a spring or a drag, or the user's scroll.
    pub(crate) fn on_scroll(&mut self, cx: &mut Context<Self>) {
        self.update_in_view(cx);
    }

    // The viewport came on the page: what moves its slides.
    fn bind_viewport(&mut self, cx: &mut Context<Self>) {
        self.motion = Some(CarouselMotion::new(0.0));
        self.measured = false;
        self.queue_measure(cx);
    }

    // The viewport left the page.
    fn unbind_viewport(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.motion_destroy(window, cx);
        self.motion = None;
        self.seamless.reset();
        self.shifts.clear();
        cx.notify();
    }

    // Starts rotating if it should, once the root is on the page. Rotation
    // that would start by default doesn't when the user prefers less motion.
    fn run_rotation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if *self.playing.get() && !self.playing.is_controlled() && prefers_reduced_motion(cx) {
            if self.playing.set_held(false) {
                cx.notify();
            }
            self.react(window, cx);
        }
        if self.autoplay.set_running(*self.playing.get(), self.interval, window, cx) {
            cx.notify();
        }
    }

    // Whether focus is in the carousel: on one of its parts, or inside the viewport.
    fn contains_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        self.tab_stop.handle().contains_focused(window, cx)
            || self.previous_focus.is_focused(window)
            || self.next_focus.is_focused(window)
            || self.play_focus.is_focused(window)
            || self.roving.focused(window).is_some()
    }

    /// The root or the viewport left the page: rotation stops while the root
    /// is gone, and the slides stop moving while the viewport is.
    pub(crate) fn check_bound(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rotating && !held(&self.instance, "root", None) {
            self.rotating = false;
            if self.autoplay.set_running(false, self.interval, window, cx) {
                cx.notify();
            }
        }
        if self.motion.is_some() && !held(&self.instance, "viewport", None) {
            self.unbind_viewport(window, cx);
        }
    }

    /// The timer ran out on a page: the next one. Rotation always wraps:
    /// it's a rotation, whatever `loop` says about the buttons.
    pub(crate) fn autoplay_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.autoplay.ticked(self.interval);
        if let Some(count) = self.page_count {
            let next = step_page(*self.page.get(), 1, count, true);
            self.go(next, Reason::Timeout, None, Some(1), window, cx);
        }
        self.autoplay.arm(window, cx);
    }

    /// Runs after each frame that draws the carousel: what it laid out, the
    /// parts that came and went, hover, and focus.
    pub(crate) fn after_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (slides, indicators, viewport, content, items, hitbox, on_screen) = {
            let mut probes = self.probes.borrow_mut();
            (
                mem::take(&mut probes.item_order),
                mem::take(&mut probes.indicator_order),
                probes.viewport.take(),
                probes.content.take(),
                mem::take(&mut probes.items),
                probes.hitbox.clone(),
                probes.on_screen.take(),
            )
        };
        // Rotation pauses under the mouse, and while scrolled out of view.
        if let Some(hitbox) = hitbox
            && !window.last_input_was_touch()
        {
            let hovering = hitbox.is_hovered_at(window.mouse_position(), window);
            self.set_paused(CarouselPause::Hover, hovering, window, cx);
        }
        if let Some(on_screen) = on_screen {
            self.set_paused(CarouselPause::Offscreen, !on_screen, window, cx);
        }
        // Indicators without a `page` take theirs from their order.
        if indicators != self.indicators {
            self.indicators = indicators;
            cx.notify();
        }
        let shown = &self.indicators;
        self.indicator_pages.borrow_mut().retain(|key, _| shown.contains(key));
        let slides_changed = slides != self.slides;
        self.slides = slides;
        let present = &self.slides;
        self.slide_handles.borrow_mut().retain(|key, _| present.contains(key));
        // A slide holding focus stays in the tree until focus leaves it.
        let focused = self.focused_slide(window, cx);
        if focused != self.focused {
            self.focused = focused;
            cx.notify();
        }
        // Keyboard focus moving in stops rotation.
        let inside = self.contains_focus(window, cx);
        if inside && !self.focus_inside && *self.playing.get() && is_focus_from_keyboard(window, cx) {
            let options = ChangeOptions::event_of(Reason::FocusIn, focus_event(window));
            self.set_playing_with(false, options, window, cx);
        }
        self.focus_inside = inside;
        match (viewport.is_some(), self.motion.is_some()) {
            (true, false) => self.bind_viewport(cx),
            (false, true) => self.unbind_viewport(window, cx),
            _ => {}
        }
        if !self.rotating {
            self.rotating = true;
            self.run_rotation(window, cx);
        }
        match viewport {
            Some(viewport) => {
                let laid_out = LaidOut {
                    viewport,
                    content,
                    slides: self
                        .slides
                        .iter()
                        .filter_map(|key| items.get(key).map(|bounds| (key.clone(), *bounds)))
                        .collect(),
                };
                // What a resize observer would hear.
                if slides_changed || self.measured_from.as_ref().is_none_or(|from| !from.same_as(&laid_out)) {
                    self.measure_queued = true;
                }
                self.laid_out = Some(laid_out);
            }
            None => self.laid_out = None,
        }
        if self.measure_queued {
            self.measure(window, cx);
        }
        if let Some(target) = self.awaited.take()
            && *self.page.get() != target
        {
            self.follow_page(true, window, cx);
        }
    }

    /// The current page, from 1.
    pub fn page(&self) -> usize {
        *self.page.get()
    }

    /// How many pages there are, or `None` before the carousel has measured its slides.
    pub fn page_count(&self) -> Option<usize> {
        self.page_count
    }

    /// Goes to a page, from 1, clamped to the pages there are. Returns false
    /// when canceled or already there.
    pub fn set_page(&mut self, page: usize, options: CarouselPageOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.go(page, reason, options.event, None, window, cx)
    }

    /// Goes to the next page, or the first one after the last with `loop`.
    /// Returns false at the end.
    pub fn next(&mut self, options: CarouselPageOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.step(1, options, window, cx)
    }

    /// Goes to the previous page, or the last one before the first with
    /// `loop`. Returns false at the start.
    pub fn previous(&mut self, options: CarouselPageOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.step(-1, options, window, cx)
    }

    /// Whether there's a next page, always with `loop`.
    pub fn can_next(&self) -> bool {
        self.can_step(1)
    }

    /// Whether there's a previous page, always with `loop`.
    pub fn can_previous(&self) -> bool {
        self.can_step(-1)
    }

    /// Whether it rotates on its own.
    pub fn is_playing(&self) -> bool {
        *self.playing.get()
    }

    /// Starts or stops rotation. Returns false when canceled or unchanged.
    pub fn set_playing(&mut self, playing: bool, options: CarouselPlayingOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.set_playing_with(playing, options, window, cx)
    }

    /// Starts rotation.
    pub fn play(&mut self, options: CarouselPlayingOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.set_playing_with(true, options, window, cx)
    }

    /// Stops rotation.
    pub fn pause(&mut self, options: CarouselPlayingOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.set_playing_with(false, options, window, cx)
    }

    /// Passes the controlled page, or `None` to let the carousel keep its own.
    pub fn set_controlled_page(&mut self, page: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.page.set_controlled(page) {
            cx.notify();
        }
        self.react(window, cx);
    }

    /// Passes the controlled `playing`, or `None` to let the carousel keep its own.
    pub fn set_controlled_playing(&mut self, playing: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        if self.playing.set_controlled(playing) {
            cx.notify();
        }
        self.react(window, cx);
    }
}
