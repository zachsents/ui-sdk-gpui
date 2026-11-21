//! Port of `foundation/popups/sheet-scroll-snap.ts`: the scroll-snap driver.
//! The host is a full-window scroller and the sheet sits in it, so dragging
//! is native scrolling: the platform's momentum and bounce, and the sheet
//! keeps moving while the app is busy. Scroll position `s` shows extent
//! `s + extent_at_top`. Bottom sheets only; other sides use the pointer
//! driver.
//!
//! How it moves:
//! - Programmatic moves (open, close, the handle) set the scroll position at
//!   once and play the difference back as a spring on the sheet's offset.
//! - A sheet that can't be dismissed starts the scroll at its smallest snap
//!   point, so the native bounce past it is the resistance.
//! - While the sheet isn't at its largest snap point, its scroll areas are
//!   locked ([`ScrollSnapDriver::scroll_areas_locked`]), so a swipe on them
//!   moves the sheet.
//! - Scrolled out of view, the sheet is dismissed.
//! - Resting where the window stays usable, the host lets the pointer
//!   through beside the sheet ([`ScrollSnapDriver::host_blocks`]).
//!
//! gpui's scrollers don't snap, so where the original's browser snaps to
//! markers, the driver settles on the snap point the machine picks once the
//! scroll rests: no scroll for a moment, with no finger down.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gpui::{Context, Pixels, ScrollHandle, SharedString, Task, TouchPhase, WeakEntity, Window, point, px};
use ui_sdk_motion::{
    SettleTimeOptions, SheetEffect, SheetEvent, SheetSnapReason, SheetState, initial_state,
    nearest_index, next, settle_time, solve_spring, timestamp,
};

use crate::sheet_driver::{OPEN_HOLD, SettleMotion, SheetDriverOptions, TOOLBAR_BLEED, after_first_frame};

// No scroll for this long, with no finger down, counts as rest.
const REST_TIMEOUT: Duration = Duration::from_millis(120);

// Room past the largest snap point, so a stretch shows before snapping back.
const OVERSCROLL_ROOM: f64 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Closing {
    Swipe,
    Close,
}

struct Flip {
    run: u64,
    motion: SettleMotion,
    to: f64,
    // The scroll position the flip set, which the sheet's offset plays back
    // from.
    scrolled: f64,
    // Settles at the end.
    settle: Option<SharedString>,
}

struct SnapState {
    machine: SheetState,
    extent_at_top: f64,
    flip: Option<Flip>,
    run: u64,
    touching: bool,
    rest_missed: bool,
    dragging: bool,
    closing: Option<Closing>,
    rest_timer: Option<Task<()>>,
    frame: bool,
    target_id: Option<SharedString>,
    last_offset: Option<f64>,
    locked: bool,
    host_blocks: bool,
    destroyed: bool,
}

struct SnapInner<T: 'static> {
    options: SheetDriverOptions<T>,
    entity: WeakEntity<T>,
    scroll: ScrollHandle,
    state: RefCell<SnapState>,
    me: Weak<SnapInner<T>>,
}

/// The scroll-snap driver (`createScrollSnapDriver`'s return). Clones share
/// it.
pub struct ScrollSnapDriver<T: 'static> {
    inner: Rc<SnapInner<T>>,
}

impl<T: 'static> Clone for ScrollSnapDriver<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// The scroll-snap driver for a sheet of the component being made
/// (`createScrollSnapDriver`).
pub fn create_scroll_snap_driver<T: 'static>(options: SheetDriverOptions<T>, cx: &Context<T>) -> ScrollSnapDriver<T> {
    ScrollSnapDriver {
        inner: Rc::new_cyclic(|me| SnapInner {
            options,
            entity: cx.weak_entity(),
            scroll: ScrollHandle::new(),
            state: RefCell::new(SnapState {
                machine: initial_state(0.0, None),
                extent_at_top: 0.0,
                flip: None,
                run: 0,
                touching: false,
                rest_missed: false,
                dragging: false,
                closing: None,
                rest_timer: None,
                frame: false,
                target_id: None,
                last_offset: None,
                locked: true,
                host_blocks: true,
                destroyed: false,
            }),
            me: me.clone(),
        }),
    }
}

fn px_f64(value: Pixels) -> f64 {
    f64::from(f32::from(value))
}

impl<T: 'static> ScrollSnapDriver<T> {
    fn from_weak(weak: &Weak<SnapInner<T>>) -> Option<Self> {
        weak.upgrade().map(|inner| ScrollSnapDriver { inner })
    }

    /// The host's scroll handle: the host element tracks it and scrolls
    /// vertically.
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.inner.scroll
    }

    fn container(&self) -> f64 {
        (self.inner.options.config)().dimension
    }

    fn largest_extent(&self) -> f64 {
        (self.inner.options.config)()
            .snap_points
            .last()
            .map_or(0.0, |point| point.extent)
    }

    /// How far the host can scroll: past the largest snap point by a little.
    pub fn max_scroll(&self) -> f64 {
        self.largest_extent() + OVERSCROLL_ROOM - self.inner.state.borrow().extent_at_top
    }

    fn scroll_top(&self) -> f64 {
        px_f64(-self.inner.scroll.offset().y)
    }

    fn set_scroll_top(&self, top: f64) {
        let offset = self.inner.scroll.offset();
        self.inner.scroll.set_offset(point(offset.x, px(-(top as f32))));
        self.inner.state.borrow_mut().last_offset = Some(top);
    }

    fn scroll_extent(&self) -> f64 {
        (self.scroll_top() + self.inner.state.borrow().extent_at_top).min(self.largest_extent())
    }

    /// Where the sheet's top sits in the host's scroll content.
    pub fn sheet_top(&self) -> f64 {
        self.container() - self.inner.state.borrow().extent_at_top
    }

    /// How tall the host's scroll content is: the most it scrolls, and a
    /// screen.
    pub fn content_height(&self) -> f64 {
        self.max_scroll() + self.container()
    }

    /// The sheet's offset from its place in the scroller: the spring a
    /// programmatic move plays back.
    pub fn flip_offset(&self) -> f64 {
        let state = self.inner.state.borrow();
        match &state.flip {
            Some(flip) => flip.scrolled - flip.motion.extent(timestamp()),
            None => 0.0,
        }
    }

    /// The extent the sheet shows now.
    pub fn rendered(&self) -> f64 {
        let state = self.inner.state.borrow();
        match &state.flip {
            Some(flip) => flip.motion.extent(timestamp()),
            None => {
                drop(state);
                self.scroll_extent()
            }
        }
    }

    /// Whether the sheet's scroll areas stay still, so a swipe on them moves
    /// the sheet: everywhere but its largest snap point.
    pub fn scroll_areas_locked(&self) -> bool {
        self.inner.state.borrow().locked
    }

    /// Whether the host takes the pointer beside the sheet: not while the
    /// sheet rests where the window stays usable, when it hears the pointer
    /// only through the sheet. A scroll that moves the sheet keeps the host
    /// until it rests, as a scroll stays with the scroller it started on.
    pub fn host_blocks(&self) -> bool {
        self.inner.state.borrow().host_blocks
    }

    // A scroll or a touch started moving the sheet: the host takes the
    // pointer until it rests.
    fn hold_host(&self, cx: &mut Context<T>) {
        if !std::mem::replace(&mut self.inner.state.borrow_mut().host_blocks, true) {
            cx.notify();
        }
    }

    /// When the move in progress started and how long it runs, in ms.
    pub fn motion_window(&self) -> Option<(f64, f64)> {
        let state = self.inner.state.borrow();
        state
            .flip
            .as_ref()
            .map(|flip| (flip.motion.start_time, flip.motion.duration))
    }

    pub fn target(&self) -> Option<SharedString> {
        let state = self.inner.state.borrow();
        if state.closing.is_some() {
            None
        } else {
            state.target_id.clone()
        }
    }

    pub fn is_moving(&self) -> bool {
        let state = self.inner.state.borrow();
        state.flip.is_some() || state.dragging
    }

    pub fn is_dragging(&self) -> bool {
        self.inner.state.borrow().dragging
    }

    fn layout(&self) {
        let config = (self.inner.options.config)();
        let extent_at_top = if config.dismissible {
            -TOOLBAR_BLEED
        } else {
            config.snap_points.first().map_or(0.0, |point| point.extent)
        };
        self.inner.state.borrow_mut().extent_at_top = extent_at_top;
    }

    fn schedule(&self, window: &Window) {
        {
            let mut state = self.inner.state.borrow_mut();
            if state.frame {
                return;
            }
            state.frame = true;
        }
        let me = self.inner.me.clone();
        window.on_next_frame(move |window, cx| {
            let Some(driver) = Self::from_weak(&me) else {
                return;
            };
            let entity = driver.inner.entity.clone();
            entity
                .update(cx, |this, cx| driver.frame(this, window, cx))
                .ok();
        });
    }

    fn frame(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        self.inner.state.borrow_mut().frame = false;
        let now = timestamp();
        let done = {
            let state = self.inner.state.borrow();
            state
                .flip
                .as_ref()
                .map(|flip| (flip.motion.finished(now), flip.run, flip.settle.clone()))
        };
        match done {
            Some((true, run, settle)) => {
                {
                    let mut state = self.inner.state.borrow_mut();
                    if state.flip.as_ref().is_some_and(|flip| flip.run == run) {
                        state.flip = None;
                    }
                }
                match settle {
                    Some(id) => self.settle(this, id, window, cx),
                    None => (self.inner.options.on_rest)(this, None, window, cx),
                }
            }
            Some((false, _, _)) => {
                (self.inner.options.on_frame)(self.rendered(), cx);
                self.schedule(window);
            }
            None => {}
        }
        cx.notify();
    }

    /// The host scrolled, as gpui laid it out: what the sheet shows follows,
    /// and a rest is due once it stops (`onScroll`).
    pub fn scrolled(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let top = self.scroll_top();
        let (flip, closing, touching, dragging) = {
            let mut state = self.inner.state.borrow_mut();
            if state.destroyed || state.last_offset == Some(top) {
                return;
            }
            state.last_offset = Some(top);
            (state.flip.is_some(), state.closing, state.touching, state.dragging)
        };
        if flip || closing.is_some() {
            return;
        }
        self.hold_host(cx);
        if touching && !dragging {
            self.set_dragging(this, true, window, cx);
        }
        (self.inner.options.on_frame)(self.scroll_extent(), cx);
        // Scrolled out of view, it's dismissed.
        let (target, dismissible) = {
            let state = self.inner.state.borrow();
            (state.target_id.clone(), (self.inner.options.config)().dismissible)
        };
        if self.scroll_extent() < 1.0 && target.is_some() && dismissible {
            {
                let mut state = self.inner.state.borrow_mut();
                state.closing = Some(Closing::Swipe);
                state.target_id = None;
            }
            (self.inner.options.on_dismiss)(this, None, window, cx);
            return;
        }
        if !touching {
            self.rest_later(window, cx);
        }
    }

    fn rest_later(&self, window: &mut Window, cx: &mut Context<T>) {
        let me = self.inner.me.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(REST_TIMEOUT).await;
            this.update_in(cx, |this, window, cx| {
                if let Some(driver) = Self::from_weak(&me) {
                    driver.on_rest(this, window, cx);
                }
            })
            .ok();
        });
        self.inner.state.borrow_mut().rest_timer = Some(task);
    }

    /// A finger touched the host or lifted, from its scroll events' touch
    /// phase.
    pub fn touch(&self, this: &mut T, phase: TouchPhase, window: &mut Window, cx: &mut Context<T>) {
        match phase {
            TouchPhase::Started => {
                let flip = {
                    let mut state = self.inner.state.borrow_mut();
                    state.touching = true;
                    state.rest_missed = false;
                    state.flip.is_some() && state.closing.is_none()
                };
                self.hold_host(cx);
                if flip {
                    self.catch_flip(window, cx);
                }
            }
            TouchPhase::Moved => {}
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.inner.state.borrow_mut().touching = false;
                self.set_dragging(this, false, window, cx);
                // The native bounce past the smallest snap point is the only
                // sign of a refused swipe.
                if !(self.inner.options.config)().dismissible && self.scroll_top() < -2.0 {
                    (self.inner.options.on_dismiss_attempt)(this, None, window, cx);
                }
                self.inner.state.borrow_mut().rest_missed = false;
                self.rest_later(window, cx);
            }
        }
    }

    fn set_dragging(&self, this: &mut T, value: bool, window: &mut Window, cx: &mut Context<T>) {
        let changed = {
            let mut state = self.inner.state.borrow_mut();
            (state.dragging != value).then(|| state.dragging = value)
        };
        if changed.is_some() {
            (self.inner.options.on_drag_change)(this, value, window, cx);
        }
    }

    fn on_rest(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let (touching, busy) = {
            let mut state = self.inner.state.borrow_mut();
            if state.touching {
                state.rest_missed = true;
            }
            (state.touching, state.flip.is_some() || state.closing.is_some())
        };
        if touching || busy {
            return;
        }
        let config = (self.inner.options.config)();
        let (next_state, effects) = {
            let state = self.inner.state.borrow();
            next(&state.machine, &SheetEvent::Rest { extent: self.scroll_extent() }, &config)
        };
        self.inner.state.borrow_mut().machine = next_state;
        for effect in effects {
            match effect {
                SheetEffect::Snap { id, reason } => {
                    self.inner.state.borrow_mut().target_id = Some(id.clone());
                    (self.inner.options.on_snap)(this, id, reason, None, window, cx);
                }
                SheetEffect::Dismiss { .. } => {
                    {
                        let mut state = self.inner.state.borrow_mut();
                        state.closing = Some(Closing::Swipe);
                        state.target_id = None;
                    }
                    (self.inner.options.on_dismiss)(this, None, window, cx);
                    return;
                }
                _ => {}
            }
        }
        let resting = self.inner.state.borrow().machine.resting_index;
        if let Some(index) = resting
            && let Some(point) = config.snap_points.get(index)
        {
            // gpui doesn't snap: the sheet settles where the machine rests it.
            let id = point.id.clone();
            let from = self.scroll_extent();
            if (point.extent - from).abs() < 0.5 {
                self.settle(this, id, window, cx);
            } else {
                self.animate_to(point.extent, from, timestamp(), Some(id), window, cx);
            }
        }
    }

    fn catch_flip(&self, window: &mut Window, cx: &mut Context<T>) {
        let extent = {
            let state = self.inner.state.borrow();
            let Some(flip) = state.flip.as_ref() else {
                return;
            };
            flip.motion.extent(timestamp())
        };
        let top = (extent - self.inner.state.borrow().extent_at_top).max(0.0);
        self.inner.state.borrow_mut().flip = None;
        self.set_scroll_top(top);
        (self.inner.options.on_frame)(self.scroll_extent(), cx);
        cx.notify();
        self.schedule(window);
    }

    fn animate_to(
        &self,
        target: f64,
        from: f64,
        start_time: f64,
        settle: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let physics = (self.inner.options.config)().physics;
        let solver = solve_spring(physics.response, physics.damping_ratio, from - target, 0.0);
        let duration = settle_time(&solver, SettleTimeOptions::default()).max(1.0);
        let extent_at_top = self.inner.state.borrow().extent_at_top;
        let scrolled = extent_at_top.max(target);
        self.set_scroll_top(scrolled - extent_at_top);
        let motion = SettleMotion {
            extent_at: Rc::new(move |elapsed: f64| target + solver.position(elapsed.max(0.0))),
            start_time,
            duration,
            fade: None,
        };
        (self.inner.options.on_motion)(&motion);
        {
            let mut state = self.inner.state.borrow_mut();
            state.run += 1;
            let run = state.run;
            state.flip = Some(Flip {
                run,
                motion,
                to: target,
                scrolled,
                settle,
            });
        }
        self.schedule(window);
        cx.notify();
    }

    fn settle(&self, this: &mut T, id: SharedString, window: &mut Window, cx: &mut Context<T>) {
        let config = (self.inner.options.config)();
        let Some(index) = config.snap_points.iter().position(|point| point.id == id) else {
            return;
        };
        let interactive = (self.inner.options.is_interactive)(&id);
        {
            let mut state = self.inner.state.borrow_mut();
            state.machine = initial_state(config.snap_points[index].extent, Some(index));
            state.target_id = Some(id.clone());
            state.host_blocks = !interactive;
            state.locked = index + 1 != config.snap_points.len();
        }
        (self.inner.options.on_rest)(this, Some(id), window, cx);
        cx.notify();
    }

    fn move_to(&self, this: &mut T, id: SharedString, reason: SheetSnapReason, window: &mut Window, cx: &mut Context<T>) {
        let config = (self.inner.options.config)();
        let Some(point) = config.snap_points.iter().find(|point| point.id == id).cloned() else {
            return;
        };
        let (closing, target) = {
            let state = self.inner.state.borrow();
            (state.closing, state.target_id.clone())
        };
        if closing == Some(Closing::Close) {
            return;
        }
        self.inner.state.borrow_mut().closing = None;
        if target.as_ref() != Some(&id) {
            self.inner.state.borrow_mut().target_id = Some(id.clone());
            (self.inner.options.on_snap)(this, id.clone(), reason, None, window, cx);
        }
        let from = self.rendered();
        self.animate_to(point.extent, from, timestamp(), Some(id), window, cx);
    }

    /// Shows the sheet at a snap point: rising from closed, or there at once.
    pub fn open(&self, this: &mut T, id: SharedString, animate: bool, window: &mut Window, cx: &mut Context<T>) {
        {
            let mut state = self.inner.state.borrow_mut();
            state.closing = None;
            state.target_id = Some(id.clone());
        }
        self.layout();
        let config = (self.inner.options.config)();
        let Some(point) = config.snap_points.iter().find(|point| point.id == id).cloned() else {
            return;
        };
        if !animate {
            let extent_at_top = self.inner.state.borrow().extent_at_top;
            self.set_scroll_top(point.extent - extent_at_top);
            return self.settle(this, id, window, cx);
        }
        self.animate_to(point.extent, -TOOLBAR_BLEED, timestamp() + OPEN_HOLD, Some(id), window, cx);
        let run = self.inner.state.borrow().run;
        let me = self.inner.me.clone();
        after_first_frame(window, move |frame_time, window, _cx| {
            let Some(driver) = Self::from_weak(&me) else {
                return;
            };
            {
                let mut state = driver.inner.state.borrow_mut();
                let Some(flip) = state.flip.as_mut().filter(|flip| flip.run == run) else {
                    return;
                };
                flip.motion.start_time = frame_time;
            }
            driver.schedule(window);
        });
    }

    /// Settles closed.
    pub fn close(&self, _this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let was_closing = self.inner.state.borrow_mut().closing.replace(Closing::Close);
        if was_closing.is_some() {
            return;
        }
        self.inner.state.borrow_mut().target_id = None;
        let from = self.rendered();
        self.animate_to(-TOOLBAR_BLEED, from, timestamp(), None, window, cx);
    }

    pub fn snap_to(&self, this: &mut T, id: SharedString, reason: SheetSnapReason, window: &mut Window, cx: &mut Context<T>) {
        self.move_to(this, id, reason, window, cx);
    }

    /// A refused dismissal: a small push toward closing that springs back.
    pub fn nudge(&self, _this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let (closing, target) = {
            let state = self.inner.state.borrow();
            (state.closing, state.target_id.clone())
        };
        if closing.is_some() || (self.inner.options.reduced_motion)(cx) {
            return;
        }
        let Some(target) = target else {
            return;
        };
        let config = (self.inner.options.config)();
        if let Some(point) = config.snap_points.iter().find(|point| point.id == target) {
            let extent = point.extent;
            self.animate_to(extent, extent - 20.0, timestamp(), Some(target), window, cx);
        }
    }

    /// The geometry changed: keeps the resting snap point, or `fallback`, or
    /// the nearest.
    pub fn relayout(
        &self,
        this: &mut T,
        _immediate: bool,
        fallback: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let (closing, touching, target_id, top) = {
            let state = self.inner.state.borrow();
            (state.closing, state.touching, state.target_id.clone(), state.extent_at_top)
        };
        if closing.is_some() || touching {
            return;
        }
        let points = (self.inner.options.config)().snap_points;
        if points.is_empty() {
            return;
        }
        let kept = target_id
            .as_ref()
            .and_then(|target| points.iter().position(|point| &point.id == target));
        let wanted = fallback
            .as_ref()
            .and_then(|fallback| points.iter().position(|point| &point.id == fallback));
        let index = kept.or(wanted).unwrap_or_else(|| {
            nearest_index(&points.iter().map(|point| point.extent).collect::<Vec<_>>(), self.scroll_extent())
        });
        self.layout();
        let point = points[index].clone();
        if target_id.as_ref() != Some(&point.id) {
            self.inner.state.borrow_mut().target_id = Some(point.id.clone());
            (self.inner.options.on_snap)(this, point.id.clone(), SheetSnapReason::ContentResize, None, window, cx);
        }
        // A sheet on its way is aimed at where it rests now.
        let moving = {
            let state = self.inner.state.borrow();
            state.flip.as_ref().map(|flip| (flip.to, flip.motion.extent(timestamp()), flip.motion.start_time))
        };
        if let Some((to, current, start)) = moving {
            let extent_at_top = self.inner.state.borrow().extent_at_top;
            if to != point.extent || extent_at_top != top {
                self.animate_to(point.extent, current, start.max(timestamp()), Some(point.id), window, cx);
            }
            return;
        }
        let extent_at_top = self.inner.state.borrow().extent_at_top;
        self.set_scroll_top(point.extent - extent_at_top);
        self.settle(this, point.id, window, cx);
    }

    /// Stops it: no more frames, rests or callbacks.
    pub fn destroy(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let dragging = {
            let mut state = self.inner.state.borrow_mut();
            state.destroyed = true;
            state.flip = None;
            state.rest_timer = None;
            std::mem::replace(&mut state.dragging, false)
        };
        if dragging {
            (self.inner.options.on_drag_change)(this, false, window, cx);
        }
    }
}
