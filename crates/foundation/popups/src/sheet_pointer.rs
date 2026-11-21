//! Port of `foundation/popups/sheet-pointer.ts`: the pointer driver. Touch,
//! mouse and pen input into the machine, a spring that starts at the
//! release velocity, and the sheet's offset written once per frame while a
//! finger drives.
//!
//! The original listens to touches on the document so the sheet can take a
//! gesture over from content the browser already scrolls. gpui decides what
//! a touch is as it starts, so the sheet follows its touches through a drag
//! session from the start, and the machine scrolls the content under the
//! finger itself, through the scroll areas the sheet knows: the hand-off is
//! the same, in either direction. Settles are springs the driver samples on
//! every frame.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gpui::{Context, IntoElement, MouseButton, SharedString, WeakEntity, Window};
use ui_sdk_drag_and_drop::{
    BindDragOptions, DragSession, PointerEvent, PointerKind, PointerSample, is_drag_press,
    swallow_next_click,
};
use ui_sdk_kernel::InputEvent;
use ui_sdk_motion::{
    PointerType, SettleTimeOptions, SheetEffect, SheetEvent, SheetPhase, SheetSnapReason,
    SheetState, VelocityTracker, VelocityTrackerOptions, initial_state, nearest_index, next,
    settle_time, solve_spring, timestamp,
};

use crate::sheet_axis::{ScrollArea, axis_point, scroll_context, scroll_forward};
use crate::sheet_driver::{
    OPEN_HOLD, REDUCED_FADE, REDUCED_SETTLE, SettleMotion, SheetDriverOptions, after_first_frame,
};

// UIScrollView's normal deceleration, for content flung past the sheet's
// largest snap point.
const CONTENT_DECELERATION_RATE: f64 = 0.998;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Closing {
    // A swipe's close holds until the popup confirms it: a refused one snaps
    // back.
    Swipe,
    Close,
}

struct Coast {
    velocity: f64,
    start_time: f64,
    start_scroll: f64,
    area: ScrollArea,
}

struct PointerState {
    machine: SheetState,
    shown: f64,
    pending_extent: Option<f64>,
    pending_scroll: f64,
    frame: bool,
    motion: Option<SettleMotion>,
    run: u64,
    closing: Option<Closing>,
    coast: Option<Coast>,
    area: Option<ScrollArea>,
    dragging: bool,
    mouse_dragged: bool,
    following: Option<PointerKind>,
    // Kept to see whether a drag inside took the press, like a slider's: it
    // isn't the sheet's.
    press: Option<PointerEvent>,
    target_id: Option<SharedString>,
    caught: Option<SharedString>,
    tracker: VelocityTracker,
    destroyed: bool,
}

struct PointerInner<T: 'static> {
    options: SheetDriverOptions<T>,
    drags: DragSession<T>,
    entity: WeakEntity<T>,
    state: RefCell<PointerState>,
    me: Weak<PointerInner<T>>,
}

/// The pointer driver (`createPointerDriver`'s return). Clones share it.
pub struct PointerDriver<T: 'static> {
    inner: Rc<PointerInner<T>>,
}

impl<T: 'static> Clone for PointerDriver<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// The pointer driver for a sheet of the component being made
/// (`createPointerDriver`).
pub fn create_pointer_driver<T: 'static>(options: SheetDriverOptions<T>, cx: &Context<T>) -> PointerDriver<T> {
    let horizon = (options.config)().physics.velocity_horizon;
    PointerDriver {
        inner: Rc::new_cyclic(|me| PointerInner {
            options,
            drags: DragSession::new(cx),
            entity: cx.weak_entity(),
            state: RefCell::new(PointerState {
                machine: initial_state(0.0, None),
                shown: 0.0,
                pending_extent: None,
                pending_scroll: 0.0,
                frame: false,
                motion: None,
                run: 0,
                closing: None,
                coast: None,
                area: None,
                dragging: false,
                mouse_dragged: false,
                following: None,
                press: None,
                target_id: None,
                caught: None,
                tracker: VelocityTracker::new(VelocityTrackerOptions::default().horizon(horizon)),
                destroyed: false,
            }),
            me: me.clone(),
        }),
    }
}

fn settle_of(effect: &SheetEffect) -> Option<(f64, f64, f64, f64, f64)> {
    match *effect {
        SheetEffect::Settle {
            from,
            to,
            velocity,
            response,
            damping_ratio,
        } => Some((from, to, velocity, response, damping_ratio)),
        _ => None,
    }
}

impl<T: 'static> PointerDriver<T> {
    fn from_weak(weak: &Weak<PointerInner<T>>) -> Option<Self> {
        weak.upgrade().map(|inner| PointerDriver { inner })
    }

    /// The element that follows the pointer once a press on the sheet is
    /// followed. The sheet renders it on every frame.
    pub fn listener(&self) -> impl IntoElement + use<T> {
        self.inner.drags.listener()
    }

    /// The extent the sheet shows now, mid-settle too.
    pub fn rendered(&self) -> f64 {
        let state = self.inner.state.borrow();
        match &state.motion {
            Some(motion) if motion.fade.is_none() => motion.extent(timestamp()),
            _ => state.shown,
        }
    }

    /// The sheet's opacity now: below 1 while it fades in or out under
    /// reduced motion.
    pub fn opacity(&self) -> f64 {
        let state = self.inner.state.borrow();
        state
            .motion
            .as_ref()
            .and_then(|motion| motion.opacity(timestamp()))
            .unwrap_or(1.0)
    }

    /// When the settle in progress started and how long it runs, in ms.
    pub fn motion_window(&self) -> Option<(f64, f64)> {
        let state = self.inner.state.borrow();
        state.motion.as_ref().map(|motion| (motion.start_time, motion.duration))
    }

    /// The snap point it rests on or is settling to, or `None` when closed or
    /// closing.
    pub fn target(&self) -> Option<SharedString> {
        let state = self.inner.state.borrow();
        if state.closing.is_some() {
            None
        } else {
            state.target_id.clone()
        }
    }

    /// Whether a gesture or a settle is in progress.
    pub fn is_moving(&self) -> bool {
        !matches!(self.inner.state.borrow().machine.phase, SheetPhase::Idle)
    }

    /// Whether a finger or the mouse drags it now.
    pub fn is_dragging(&self) -> bool {
        self.inner.state.borrow().dragging
    }

    fn write(&self, extent: f64) {
        let mut state = self.inner.state.borrow_mut();
        if state.destroyed {
            return;
        }
        state.shown = extent;
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
                .update(cx, |this, cx| driver.flush(this, window, cx))
                .ok();
        });
    }

    // One frame: content scrolls by what the machine asked, a fling coasts,
    // a settle moves on, and a finger's newest extent shows.
    fn flush(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let now = timestamp();
        let (scroll, area) = {
            let mut state = self.inner.state.borrow_mut();
            state.frame = false;
            let scroll = std::mem::take(&mut state.pending_scroll);
            (scroll, state.area.clone())
        };
        let side = (self.inner.options.side)();
        if scroll != 0.0
            && let Some(area) = &area
        {
            scroll_forward(area, side, scroll);
        }
        self.step_coast(now, window);
        let finished = {
            let state = self.inner.state.borrow();
            state.motion.as_ref().map(|motion| motion.finished(now))
        };
        match finished {
            Some(true) => self.finish_settle(this, window, cx),
            Some(false) => {
                let extent = self.rendered();
                (self.inner.options.on_frame)(extent, cx);
                self.schedule(window);
            }
            None => {}
        }
        let pending = self.inner.state.borrow_mut().pending_extent.take();
        if let Some(extent) = pending {
            self.write(extent);
            (self.inner.options.on_frame)(extent, cx);
        }
        cx.notify();
    }

    fn step_coast(&self, now: f64, window: &Window) {
        let mut state = self.inner.state.borrow_mut();
        let side = (self.inner.options.side)();
        let Some(coast) = state.coast.as_mut() else {
            return;
        };
        let elapsed = (now - coast.start_time).max(0.0);
        let travelled = (coast.velocity * (CONTENT_DECELERATION_RATE.powf(elapsed) - 1.0))
            / CONTENT_DECELERATION_RATE.ln();
        scroll_forward(&coast.area, side, travelled - coast.start_scroll);
        coast.start_scroll = travelled;
        if (coast.velocity * CONTENT_DECELERATION_RATE.powf(elapsed)).abs() < 0.01 {
            state.coast = None;
        } else {
            drop(state);
            self.schedule(window);
        }
    }

    fn dispatch(
        &self,
        this: &mut T,
        event: SheetEvent,
        source: Option<&InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let (effects, time) = {
            let mut state = self.inner.state.borrow_mut();
            if state.destroyed {
                return;
            }
            let config = (self.inner.options.config)();
            let (next_state, effects) = next(&state.machine, &event, &config);
            state.machine = next_state;
            let time = match event {
                SheetEvent::Down { time, .. }
                | SheetEvent::Move { time, .. }
                | SheetEvent::Up { time, .. }
                | SheetEvent::SnapTo { time, .. }
                | SheetEvent::Close { time }
                | SheetEvent::Nudge { time } => time,
                _ => timestamp(),
            };
            (effects, time)
        };
        for effect in effects {
            self.run(this, effect, time, source, window, cx);
        }
        let changed = {
            let mut state = self.inner.state.borrow_mut();
            let dragging = matches!(state.machine.phase, SheetPhase::Dragging { .. });
            (dragging != state.dragging).then(|| {
                state.dragging = dragging;
                dragging
            })
        };
        if let Some(dragging) = changed {
            (self.inner.options.on_drag_change)(this, dragging, window, cx);
        }
    }

    fn run(
        &self,
        this: &mut T,
        effect: SheetEffect,
        time: f64,
        source: Option<&InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        match effect {
            SheetEffect::Move { extent } => {
                let animating = self.inner.state.borrow().motion.is_some();
                if animating {
                    self.stop_animation();
                    self.write(extent);
                    (self.inner.options.on_frame)(extent, cx);
                } else {
                    self.inner.state.borrow_mut().pending_extent = Some(extent);
                    self.schedule(window);
                }
            }
            SheetEffect::Settle { .. } => self.start_settle(this, &effect, time, window, cx),
            SheetEffect::Scroll { delta } => {
                self.inner.state.borrow_mut().pending_scroll += delta;
                self.schedule(window);
            }
            SheetEffect::FlingScroll { velocity } => {
                {
                    let mut state = self.inner.state.borrow_mut();
                    if let Some(area) = state.area.clone() {
                        state.coast = Some(Coast {
                            velocity,
                            start_time: time,
                            start_scroll: 0.0,
                            area,
                        });
                    }
                }
                self.schedule(window);
            }
            SheetEffect::Snap { id, reason } => {
                self.inner.state.borrow_mut().target_id = Some(id.clone());
                (self.inner.options.on_snap)(this, id, reason, source, window, cx);
            }
            SheetEffect::Dismiss { reason } => {
                if reason != ui_sdk_motion::SheetDismissReason::Swipe {
                    return;
                }
                self.inner.state.borrow_mut().closing = Some(Closing::Swipe);
                (self.inner.options.on_dismiss)(this, source, window, cx);
            }
            SheetEffect::AttemptDismiss => (self.inner.options.on_dismiss_attempt)(this, source, window, cx),
        }
    }

    fn start_settle(&self, this: &mut T, effect: &SheetEffect, start_time: f64, window: &mut Window, cx: &mut Context<T>) {
        let Some((from, to, velocity, response, damping_ratio)) = settle_of(effect) else {
            return;
        };
        self.stop_animation();
        self.inner.state.borrow_mut().pending_extent = None;
        let closed = (self.inner.options.closed_extent)();
        let from = if from <= 0.0 { closed } else { from };
        let to = if to <= 0.0 { closed } else { to };
        let reduced = (self.inner.options.reduced_motion)(cx);
        let fade = reduced && (from == closed || to == closed);
        let (duration, extent_at): (f64, Rc<dyn Fn(f64) -> f64>) = if reduced {
            let duration = if fade { REDUCED_FADE } else { REDUCED_SETTLE };
            let ease = |t: f64| 1.0 - (1.0 - t).powi(3);
            (
                duration,
                Rc::new(move |elapsed: f64| from + (to - from) * ease((elapsed / duration).min(1.0))),
            )
        } else {
            let solver = solve_spring(response, damping_ratio, from - to, velocity);
            let duration = settle_time(&solver, SettleTimeOptions::default());
            (duration, Rc::new(move |elapsed: f64| to + solver.position(elapsed)))
        };
        self.write(if fade { to.max(0.0) } else { to });
        if ((to - from).abs() < 0.5 && velocity == 0.0) || duration == 0.0 {
            return self.finish_settle(this, window, cx);
        }
        let motion = SettleMotion {
            extent_at,
            start_time,
            duration,
            fade: fade.then_some(if from == closed { (0.0, 1.0) } else { (1.0, 0.0) }),
        };
        (self.inner.options.on_motion)(&motion);
        {
            let mut state = self.inner.state.borrow_mut();
            state.run += 1;
            state.motion = Some(motion);
        }
        self.schedule(window);
    }

    fn stop_animation(&self) {
        self.inner.state.borrow_mut().motion = None;
    }

    fn finish_settle(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        self.stop_animation();
        self.dispatch(this, SheetEvent::Settled, None, window, cx);
        let rest = {
            let mut state = self.inner.state.borrow_mut();
            if !matches!(state.machine.phase, SheetPhase::Idle) {
                return;
            }
            match state.machine.resting_index {
                None => {
                    state.closing = None;
                    state.target_id = None;
                    None
                }
                Some(index) => {
                    let id = (self.inner.options.config)().snap_points.get(index).map(|point| point.id.clone());
                    state.target_id = id.clone();
                    Some(id)
                }
            }
        };
        match rest {
            None => (self.inner.options.on_rest)(this, None, window, cx),
            Some(id) => (self.inner.options.on_rest)(this, id, window, cx),
        }
    }

    fn track(&self, position: gpui::Point<gpui::Pixels>, time: f64) -> (f64, f64) {
        let side = (self.inner.options.side)();
        let (x, y) = axis_point(side, position);
        self.inner.state.borrow_mut().tracker.add(time, y);
        (x, y)
    }

    /// A press on the sheet: a mouse button or a touch the window offers as
    /// a drag (`onPointerDown`, `onTouchStart`).
    pub fn press(&self, this: &mut T, event: &PointerEvent, window: &mut Window, cx: &mut Context<T>) {
        let (closing, following, phase, target_id) = {
            let mut state = self.inner.state.borrow_mut();
            state.press = Some(event.clone());
            (state.closing, state.following, state.machine.phase, state.target_id.clone())
        };
        if closing.is_some() || following.is_some() || is_drag_press(event) {
            return;
        }
        if !(self.inner.options.owns)(event.position, window) {
            return;
        }
        let pointer = match event.kind {
            PointerKind::Touch => PointerType::Touch,
            PointerKind::Mouse => {
                if event.button != MouseButton::Left || !(self.inner.options.can_mouse_drag)(event.position, window, cx) {
                    return;
                }
                PointerType::Mouse
            }
        };
        let drags = self.inner.drags.clone();
        if !drags.accepts(this, event, window, cx) {
            return;
        }
        let time = timestamp();
        {
            let mut state = self.inner.state.borrow_mut();
            state.mouse_dragged = false;
            state.caught = match phase {
                SheetPhase::Settling {
                    target_index: Some(_),
                    ..
                } if pointer == PointerType::Mouse => target_id,
                _ => None,
            };
            state.coast = None;
            state.tracker.reset();
            state.following = Some(event.kind);
        }
        let (x, y) = self.track(event.position, time);
        self.dispatch(
            this,
            SheetEvent::Down {
                time,
                x,
                y,
                pointer,
            },
            Some(&event.event),
            window,
            cx,
        );
        let moved = self.inner.me.clone();
        let ended = self.inner.me.clone();
        drags.start(
            event,
            BindDragOptions::new()
                .on_move(move |this: &mut T, sample: &PointerSample, window, cx| {
                    if let Some(driver) = Self::from_weak(&moved) {
                        driver.pointer_move(this, sample, window, cx);
                    }
                })
                .on_end(move |this: &mut T, up: &PointerEvent, canceled, window, cx| {
                    if let Some(driver) = Self::from_weak(&ended) {
                        driver.pointer_up(this, up, canceled, window, cx);
                    }
                }),
            window,
            cx,
        );
    }

    fn pointer_move(&self, this: &mut T, sample: &PointerSample, window: &mut Window, cx: &mut Context<T>) {
        let (press, caught, following) = {
            let state = self.inner.state.borrow();
            (state.press.clone(), state.caught.clone(), state.following)
        };
        if following.is_none() {
            return;
        }
        if press.as_ref().is_some_and(is_drag_press) {
            return self.release(this, &sample.event, 0.0, caught, window, cx);
        }
        let time = timestamp();
        let position = sample.position();
        let (x, y) = self.track(position, time);
        let side = (self.inner.options.side)();
        let areas = (self.inner.options.scroll_areas)();
        let context = scroll_context(position, &areas, side, window);
        self.inner.state.borrow_mut().area = context.as_ref().map(|(_, area)| area.clone());
        self.dispatch(
            this,
            SheetEvent::Move {
                time,
                x,
                y,
                scroll: context.map(|(context, _)| context),
            },
            Some(&sample.event.event),
            window,
            cx,
        );
        let mut state = self.inner.state.borrow_mut();
        if matches!(state.machine.phase, SheetPhase::Dragging { .. }) && following == Some(PointerKind::Mouse) {
            state.mouse_dragged = true;
        }
    }

    fn pointer_up(&self, this: &mut T, up: &PointerEvent, canceled: bool, window: &mut Window, cx: &mut Context<T>) {
        let (press, caught, following) = {
            let state = self.inner.state.borrow();
            (state.press.clone(), state.caught.clone(), state.following)
        };
        if following.is_none() {
            return;
        }
        if press.as_ref().is_some_and(is_drag_press) {
            return self.release(this, up, 0.0, caught, window, cx);
        }
        let velocity = if canceled {
            0.0
        } else {
            self.inner.state.borrow_mut().tracker.velocity(timestamp())
        };
        self.release(this, up, velocity, None, window, cx);
    }

    // `resume` is the snap point a settle was heading to when the press
    // caught it: the sheet goes on there, since letting go mid-settle could
    // dismiss it.
    fn release(
        &self,
        this: &mut T,
        event: &PointerEvent,
        velocity: f64,
        resume: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let mouse_dragged = {
            let mut state = self.inner.state.borrow_mut();
            state.following = None;
            state.press = None;
            state.mouse_dragged
        };
        let time = timestamp();
        let index = resume.as_ref().and_then(|resume| {
            (self.inner.options.config)()
                .snap_points
                .iter()
                .position(|point| &point.id == resume)
        });
        match (resume, index) {
            (Some(resume), Some(index)) => {
                {
                    let mut state = self.inner.state.borrow_mut();
                    state.machine = initial_state(state.machine.extent, Some(index));
                }
                self.dispatch(
                    this,
                    SheetEvent::SnapTo {
                        time,
                        id: resume,
                        reason: SheetSnapReason::Imperative,
                    },
                    None,
                    window,
                    cx,
                );
            }
            _ => self.dispatch(this, SheetEvent::Up { time, velocity }, Some(&event.event), window, cx),
        }
        if mouse_dragged {
            swallow_next_click(false, window, cx);
        }
    }

    /// Shows the sheet at a snap point: rising from closed, or there at once.
    pub fn open(&self, this: &mut T, id: SharedString, animate: bool, window: &mut Window, cx: &mut Context<T>) {
        {
            let mut state = self.inner.state.borrow_mut();
            state.closing = None;
            state.target_id = Some(id.clone());
        }
        self.stop_animation();
        let config = (self.inner.options.config)();
        let index = config.snap_points.iter().position(|point| point.id == id);
        if !animate && let Some(index) = index {
            let extent = config.snap_points[index].extent;
            self.inner.state.borrow_mut().machine = initial_state(extent, Some(index));
            self.write(extent);
            return (self.inner.options.on_rest)(this, Some(id), window, cx);
        }
        self.inner.state.borrow_mut().machine = initial_state(0.0, None);
        self.write((self.inner.options.closed_extent)());
        self.dispatch(
            this,
            SheetEvent::SnapTo {
                time: timestamp() + OPEN_HOLD,
                id,
                reason: SheetSnapReason::Imperative,
            },
            None,
            window,
            cx,
        );
        let run = {
            let state = self.inner.state.borrow();
            state.motion.as_ref().map(|_| state.run)
        };
        let Some(run) = run else {
            return;
        };
        let me = self.inner.me.clone();
        after_first_frame(window, move |frame_time, window, _cx| {
            let Some(driver) = Self::from_weak(&me) else {
                return;
            };
            let mut state = driver.inner.state.borrow_mut();
            if state.run != run || !matches!(state.machine.phase, SheetPhase::Settling { .. }) {
                return;
            }
            if let Some(motion) = state.motion.as_mut() {
                motion.start_time = frame_time;
            }
            if let SheetPhase::Settling { start_time, .. } = &mut state.machine.phase {
                *start_time = frame_time;
            }
            drop(state);
            driver.schedule(window);
        });
    }

    /// Settles closed.
    pub fn close(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let was_closing = self.inner.state.borrow_mut().closing.replace(Closing::Close);
        if was_closing.is_none() {
            self.dispatch(this, SheetEvent::Close { time: timestamp() }, None, window, cx);
        }
    }

    pub fn snap_to(&self, this: &mut T, id: SharedString, reason: SheetSnapReason, window: &mut Window, cx: &mut Context<T>) {
        {
            let mut state = self.inner.state.borrow_mut();
            if state.closing == Some(Closing::Close) {
                return;
            }
            state.closing = None;
        }
        self.dispatch(
            this,
            SheetEvent::SnapTo {
                time: timestamp(),
                id,
                reason,
            },
            None,
            window,
            cx,
        );
    }

    /// A refused dismissal: a small push toward closing that springs back.
    pub fn nudge(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let closing = self.inner.state.borrow().closing;
        if closing.is_none() && !(self.inner.options.reduced_motion)(cx) {
            self.dispatch(this, SheetEvent::Nudge { time: timestamp() }, None, window, cx);
        }
    }

    /// The geometry changed: keeps the resting snap point and moves to its
    /// new extent, at once when `immediate`. If that point is gone,
    /// `fallback`, or else the nearest.
    pub fn relayout(
        &self,
        this: &mut T,
        immediate: bool,
        fallback: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let points = (self.inner.options.config)().snap_points;
        let (closing, target_id, phase, shown) = {
            let state = self.inner.state.borrow();
            (state.closing, state.target_id.clone(), state.machine.phase, state.shown)
        };
        let Some(target_id) = target_id.filter(|_| {
            closing.is_none() && !matches!(phase, SheetPhase::Dragging { .. } | SheetPhase::Pending { .. })
        }) else {
            return self.write(shown);
        };
        if points.is_empty() {
            return;
        }
        let kept = points.iter().position(|point| point.id == target_id);
        let wanted = fallback
            .as_ref()
            .and_then(|fallback| points.iter().position(|point| &point.id == fallback));
        let index = kept.or(wanted).unwrap_or_else(|| {
            nearest_index(&points.iter().map(|point| point.extent).collect::<Vec<_>>(), shown)
        });
        let (id, extent) = (points[index].id.clone(), points[index].extent);
        if matches!(phase, SheetPhase::Idle) {
            self.inner.state.borrow_mut().machine = initial_state(shown, kept);
            if immediate || (extent - shown).abs() < 0.5 {
                self.inner.state.borrow_mut().machine = initial_state(extent, Some(index));
                self.write(extent);
                if !immediate {
                    return;
                }
                if id != target_id {
                    self.run(
                        this,
                        SheetEffect::Snap {
                            id: id.clone(),
                            reason: SheetSnapReason::ContentResize,
                        },
                        timestamp(),
                        None,
                        window,
                        cx,
                    );
                }
                return (self.inner.options.on_rest)(this, Some(id), window, cx);
            }
        }
        self.dispatch(
            this,
            SheetEvent::SnapTo {
                time: timestamp(),
                id,
                reason: SheetSnapReason::ContentResize,
            },
            None,
            window,
            cx,
        );
    }

    /// Stops it: no more frames, presses or callbacks, and a drag in progress
    /// ends.
    pub fn destroy(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let dragging = {
            let mut state = self.inner.state.borrow_mut();
            state.destroyed = true;
            state.motion = None;
            state.coast = None;
            state.following = None;
            std::mem::replace(&mut state.dragging, false)
        };
        let drags = self.inner.drags.clone();
        drags.cancel(this, window, cx);
        if dragging {
            (self.inner.options.on_drag_change)(this, false, window, cx);
        }
    }
}
