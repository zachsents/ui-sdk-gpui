//! Port of `foundation/motion/presence.ts`: enter and exit animations
//! without a framework.
//!
//! The original gives an element `data-starting-style` for its first frame
//! and `data-ending-style` and `inert` while it animates out, and waits for
//! every CSS transition and animation running on it before it unmounts, with
//! a timeout from the computed durations so a stuck one can't block it. gpui
//! has no CSS to wait on, so a [`Presence`] is that state for one part: the
//! app supplies the timing its CSS would declare ([`PresenceTiming`]), the
//! presence runs it on animation frames, and the part exposes
//! [`PresenceState`] for the app to style each phase from, where the original
//! styles `[data-starting-style]` and `[data-ending-style]`.
//!
//! What a part styles from:
//! - `phase`: closed, entering, open or exiting.
//! - `starting`: the enter's first frame, which shows the starting look.
//! - `ending`: the exit, and two frames after it if the part stays.
//! - `inert`: while it animates out, the part takes no input: its owner
//!   leaves its handlers and focus off.
//! - `progress`: how far it shows, from 0 (the starting or ending look) to 1
//!   (open), eased by the app's timing. It is what CSS transitions between,
//!   so the app maps it to opacity, scale or anything else.
//!
//! `progress` moves as a CSS transition does. A presence interrupted halfway
//! eases back from where it is: reversing a transition that hadn't finished
//! takes the share of the duration it had covered, as CSS's reversing
//! shortening factor does. An enter on a part already shown and at rest
//! jumps to the starting look first, as the original finishes the
//! transitions its starting style would start. Stopping an exit takes its
//! ending style off, so a part that stays eases back to its open look. What
//! a stop changes moves from the next frame, as the browser styles once per
//! frame: a stop and an enter in one update are one change.
//!
//! Timing is read as a transition starts, as the original reads computed
//! durations. When gpui's `App::reduce_motion` is set, the app's timing
//! counts as zero, as the original's examples turn transitions off under
//! `prefers-reduced-motion: reduce`, so someone who prefers reduced motion
//! gets the end at once, after the same events in the same order. Script
//! animations a driver tracks, like a sheet's spring, keep their own length:
//! the driver decides how it moves under reduced motion.
//!
//! The presence runs on `Window::on_next_frame`, which
//! `Window::request_animation_frame` is built on, so a transition can start
//! from a handler: each frame it advances, notifies the entity that owns it
//! so the views that read it render again, and runs what's due. A fallback
//! timer stands in for the original's timeout, for windows that stop
//! drawing.

use std::cell::{Ref, RefCell, RefMut};
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{App, Context, EntityId, Task, Window};

/// A timing function: input progress from 0 to 1 to output progress, like
/// CSS's `transition-timing-function`. It may overshoot. gpui's easing
/// functions, like `gpui::ease_in_out`, fit.
pub type Easing = Rc<dyn Fn(f32) -> f32>;

/// One transition's timing, what CSS's `transition-duration` and
/// `transition-timing-function` declare.
#[derive(Clone)]
pub struct Timing {
    pub duration: Duration,
    pub easing: Easing,
}

impl Timing {
    /// A transition of `duration`, linear until `easing` says otherwise.
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            easing: Rc::new(linear),
        }
    }

    /// A transition of `ms` milliseconds.
    pub fn ms(ms: u64) -> Self {
        Self::new(Duration::from_millis(ms))
    }

    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    pub fn easing(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.easing = Rc::new(easing);
        self
    }
}

/// No transition, as with no CSS for it: nothing animates, and an exit
/// finishes at once.
impl Default for Timing {
    fn default() -> Self {
        Self::new(Duration::ZERO)
    }
}

impl fmt::Debug for Timing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timing")
            .field("duration", &self.duration)
            .finish_non_exhaustive()
    }
}

fn linear(progress: f32) -> f32 {
    progress
}

/// How a part animates in and out: the transitions its CSS would declare,
/// `enter` toward its open look and `exit` toward its ending look. The
/// default has none, so it shows and hides at once.
#[derive(Clone, Debug, Default)]
pub struct PresenceTiming {
    pub enter: Timing,
    pub exit: Timing,
}

impl PresenceTiming {
    /// The same transition both ways, like one `transition` declaration.
    pub fn new(timing: Timing) -> Self {
        Self {
            enter: timing.clone(),
            exit: timing,
        }
    }

    pub fn enter(mut self, timing: Timing) -> Self {
        self.enter = timing;
        self
    }

    pub fn exit(mut self, timing: Timing) -> Self {
        self.exit = timing;
        self
    }
}

/// Where a part is in its life on the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PresencePhase {
    /// Hidden, at rest.
    #[default]
    Closed,
    /// Animating in, from its starting frame until the enter has finished.
    Entering,
    /// Shown, at rest.
    Open,
    /// Animating out, until the exit has finished and its owner was told.
    Exiting,
}

/// What a part shows of its presence, in place of the attributes the
/// original writes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PresenceState {
    pub phase: PresencePhase,
    /// `data-starting-style`: the enter's first frame, which shows the
    /// starting look.
    pub starting: bool,
    /// `data-ending-style`: animating out, and two frames past the end while
    /// the part stays.
    pub ending: bool,
    /// `inert`: the part takes no input while it animates out.
    pub inert: bool,
    /// How far it shows, eased: 0 is the starting or ending look and 1 open.
    pub progress: f32,
}

/// A script animation a presence counts, from [`Presence::track_animation`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnimationId(u64);

/// One part's enter and exit animations, the state the original keeps on its
/// element. Cheap to clone: clones share it. The owner keeps one per part,
/// like a popup's content and its backdrop, or a panel per tab.
///
/// Its operations take the context of the entity that owns it, which they
/// notify each frame while something moves, and run `done` callbacks inside
/// that entity's update with its state. `animate_out` takes that state as
/// `owner` too, for an exit with nothing to animate, which finishes at once:
///
/// ```text
/// let presence = self.presence.clone();
/// presence.animate_out(self, |this, window, cx| this.finish_exit(window, cx), window, cx);
/// ```
#[derive(Clone)]
pub struct Presence {
    inner: Rc<RefCell<Inner>>,
}

type Done = Box<dyn FnOnce(&mut Window, &mut App)>;

/// What a panel runs as its starting frame ends, before the enter waits: it
/// measures and starts its size transitions (`panel.rs`).
pub(crate) type Started = Box<dyn FnOnce(&mut Inner, Instant, bool)>;

enum Step {
    Idle,
    /// The starting style is on until `frames` more frames have passed.
    Starting {
        frames: u8,
        started: Option<Started>,
    },
    /// Waiting for these animations to finish or be replaced, then `done`
    /// (`whenFinished`).
    Waiting {
        run: u64,
        animations: Vec<(u64, Instant)>,
        exit: bool,
    },
    /// The exit finished. Its ending style and `inert` come off after
    /// `frames` more frames, unless something starts first.
    Resetting { frames: u8 },
}

/// A presence's state, which `panel.rs` also reads and moves.
pub(crate) struct Inner {
    pub(crate) timing: PresenceTiming,
    open: bool,
    starting: bool,
    ending: bool,
    inert: bool,
    shown: Transitioned,
    /// More of the element's properties in transition, which enters, waits
    /// and frames count with `shown`: a panel's height and width.
    pub(crate) properties: Vec<Transitioned>,
    scripts: Vec<(u64, Instant)>,
    next_id: u64,
    run: u64,
    step: Step,
    /// A stop took a starting or ending style off: the part transitions to
    /// its open look on the next frame, unless a new style comes first.
    restyle: bool,
    done: Option<Done>,
    /// The time values are read at, the start of the frame being drawn.
    pub(crate) frame_time: Instant,
    owner: Option<EntityId>,
    looping: bool,
    fallback: Option<Task<()>>,
}

impl Presence {
    /// A presence, open at rest or closed at rest, with no timing.
    pub fn new(open: bool) -> Self {
        let value = if open { 1.0 } else { 0.0 };
        Self {
            inner: Rc::new(RefCell::new(Inner {
                timing: PresenceTiming::default(),
                open,
                starting: false,
                ending: false,
                inert: false,
                shown: Transitioned::new(value),
                properties: Vec::new(),
                scripts: Vec::new(),
                next_id: 0,
                run: 0,
                step: Step::Idle,
                restyle: false,
                done: None,
                frame_time: Instant::now(),
                owner: None,
                looping: false,
                fallback: None,
            })),
        }
    }

    pub fn with_timing(self, timing: PresenceTiming) -> Self {
        self.set_timing(timing);
        self
    }

    /// Sets the timing transitions read as they start, as an app changes its
    /// CSS. A transition already running keeps its own.
    pub fn set_timing(&self, timing: PresenceTiming) {
        self.inner.borrow_mut().timing = timing;
    }

    pub fn timing(&self) -> PresenceTiming {
        self.inner.borrow().timing.clone()
    }

    /// What the part styles from, as of the frame being drawn.
    pub fn state(&self) -> PresenceState {
        self.inner.borrow().state()
    }

    /// Whether the last `animate_in`, `animate_out` or `set_open` showed it,
    /// or a `stop` took an exit's ending style off: open or entering.
    pub fn is_open(&self) -> bool {
        self.inner.borrow().open
    }

    /// Starts the part's enter animation (`animateIn`): the starting style for
    /// its first frame, then a transition to its open look. It stops the
    /// enter or exit running, as calling its cleanup would.
    pub fn animate_in<T: 'static>(&self, window: &mut Window, cx: &mut Context<T>) {
        self.enter(None, None, window, cx);
    }

    /// `animate_in`, then `done` once the enter has finished, for parts that
    /// hold a measured size while animating. `done` runs inside the owner's
    /// update, never before the starting frame has passed, and not at all if
    /// something stops the enter first.
    pub fn animate_in_then<T: 'static>(
        &self,
        done: impl FnOnce(&mut T, &mut Window, &mut Context<T>) + 'static,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.enter(Some(erase(done, cx)), None, window, cx);
    }

    /// Plays the part's exit animation (`animateOut`): the ending style and
    /// `inert` while it animates out, then `done`, once every running
    /// animation has finished, or a little after the longest should have.
    /// Unmount the part in `done`: after an animation it keeps the ending
    /// style until then, and loses it and `inert` two frames later if it
    /// stays. With nothing to animate, `done` runs now, with `owner`, before
    /// this returns. It stops the enter or exit running, as calling its
    /// cleanup would.
    pub fn animate_out<T: 'static>(
        &self,
        owner: &mut T,
        done: impl FnOnce(&mut T, &mut Window, &mut Context<T>) + 'static,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let now = Instant::now();
        let reduce = cx.reduce_motion();
        let waiting = {
            let mut inner = self.inner.borrow_mut();
            inner.owner = Some(cx.entity_id());
            inner.stop_run(false);
            inner.open = false;
            inner.starting = false;
            inner.ending = true;
            inner.inert = true;
            let timing = inner.timing.exit.clone();
            inner.retarget_shown(0.0, &timing, reduce, now);
            // Transitions just started count, as reading animations flushes
            // styles in the original.
            let animations = inner.snapshot(now);
            if animations.is_empty() {
                inner.reset();
                None
            } else {
                inner.run += 1;
                let run = inner.run;
                let delay = fallback_delay(&animations, now);
                inner.step = Step::Waiting {
                    run,
                    animations,
                    exit: true,
                };
                Some((run, delay))
            }
        };
        // Without animations `done` runs now, before anything painted the
        // ending style.
        let Some((run, delay)) = waiting else {
            cx.notify();
            done(owner, window, cx);
            return;
        };
        self.inner.borrow_mut().done = Some(erase(done, cx));
        self.start_fallback(run, delay, window, cx);
        self.ensure_frames(window);
        cx.notify();
    }

    /// Stops the enter or exit running, as calling the cleanup `animateIn` or
    /// `animateOut` returned does: its `done` won't run, a starting style
    /// comes off so the part transitions to its open look, and an ending
    /// style and `inert` come off now, also when an exit just finished. A
    /// part that was animating out shows again, easing back to its open look
    /// from where the exit left it; unmount it, or `set_open(false)`, to hide
    /// it instead. The transition starts on the next frame, unless an enter,
    /// exit or `set_open` comes first.
    pub fn stop<T: 'static>(&self, cx: &mut Context<T>) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.owner = Some(cx.entity_id());
            if inner.stop_run(true) {
                inner.open = true;
            }
        }
        cx.notify();
    }

    /// Shows or hides the part at once, with nothing animating, like a part
    /// open with the page. It stops the enter or exit running first, whose
    /// `done` won't run.
    pub fn set_open<T: 'static>(&self, open: bool, cx: &mut Context<T>) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.owner = Some(cx.entity_id());
            inner.stop_run(false);
            inner.open = open;
            inner.starting = false;
            inner.ending = false;
            inner.inert = false;
            inner.shown.jump(if open { 1.0 } else { 0.0 });
        }
        cx.notify();
    }

    /// Counts a script animation running on the part for `duration` from
    /// now, like a sheet's spring: an exit, or an enter with `done`, that
    /// starts waiting while it runs waits for it too, as the original waits
    /// for animations it has no CSS duration for. End it early with
    /// `end_animation` when it's canceled or replaced.
    pub fn track_animation(&self, duration: Duration) -> AnimationId {
        let mut inner = self.inner.borrow_mut();
        let id = inner.allocate_id();
        inner.scripts.push((id, Instant::now() + duration));
        AnimationId(id)
    }

    /// Stops counting a script animation, as one canceled or finished early.
    pub fn end_animation(&self, id: AnimationId) {
        self.inner
            .borrow_mut()
            .scripts
            .retain(|&(script, _)| script != id.0);
    }

    pub(crate) fn inner(&self) -> Ref<'_, Inner> {
        self.inner.borrow()
    }

    pub(crate) fn inner_mut(&self) -> RefMut<'_, Inner> {
        self.inner.borrow_mut()
    }

    /// `animateIn`, with what a panel adds: `started` runs as the starting
    /// frame ends, before the enter waits for `done`.
    pub(crate) fn enter<T: 'static>(
        &self,
        done: Option<Done>,
        started: Option<Started>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let now = Instant::now();
        {
            let mut inner = self.inner.borrow_mut();
            inner.owner = Some(cx.entity_id());
            inner.stop_run(false);
            // A part already shown would transition to its starting style and
            // back, so it jumps there. Anything already moving eases back
            // instead.
            let settled = !inner.transitioning(now);
            inner.open = true;
            inner.starting = true;
            if settled {
                inner.shown.jump(0.0);
            }
            inner.step = Step::Starting { frames: 1, started };
            inner.done = done;
        }
        self.ensure_frames(window);
        cx.notify();
    }

    fn ensure_frames(&self, window: &Window) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.looping {
                return;
            }
            inner.looping = true;
        }
        let presence = Rc::downgrade(&self.inner);
        window.on_next_frame(move |window, cx| {
            if let Some(inner) = presence.upgrade() {
                Presence { inner }.tick(window, cx);
            }
        });
    }

    // One frame: values move to now, what a stop took off restyles the part,
    // the starting style comes off after its frame, a wait ends, a finished
    // exit's ending style counts down.
    fn tick(&self, window: &mut Window, cx: &mut App) {
        let now = Instant::now();
        let reduce = cx.reduce_motion();
        let mut done = None;
        let mut fallback = None;
        let owner = {
            let mut inner = self.inner.borrow_mut();
            inner.looping = false;
            inner.frame_time = now;
            inner.settle(now);
            if std::mem::take(&mut inner.restyle) {
                let timing = inner.timing.enter.clone();
                inner.retarget_shown(1.0, &timing, reduce, now);
            }
            match std::mem::replace(&mut inner.step, Step::Idle) {
                Step::Idle => {}
                Step::Starting { frames, started } if frames > 0 => {
                    inner.step = Step::Starting {
                        frames: frames - 1,
                        started,
                    };
                }
                Step::Starting { started, .. } => {
                    // The starting frame has painted. Without the starting
                    // style, the part transitions to its open look.
                    inner.starting = false;
                    let timing = inner.timing.enter.clone();
                    inner.retarget_shown(1.0, &timing, reduce, now);
                    if let Some(started) = started {
                        started(&mut *inner, now, reduce);
                    }
                    if inner.done.is_some() {
                        let animations = inner.snapshot(now);
                        if animations.is_empty() {
                            done = inner.done.take();
                        } else {
                            inner.run += 1;
                            let run = inner.run;
                            fallback = Some((run, fallback_delay(&animations, now)));
                            inner.step = Step::Waiting {
                                run,
                                animations,
                                exit: false,
                            };
                        }
                    }
                }
                Step::Waiting {
                    run,
                    animations,
                    exit,
                } => {
                    let finished = animations
                        .iter()
                        .all(|&(id, end)| now >= end || !inner.is_live(id));
                    if finished {
                        inner.fallback = None;
                        done = inner.finish_wait(exit);
                    } else {
                        inner.step = Step::Waiting {
                            run,
                            animations,
                            exit,
                        };
                    }
                }
                Step::Resetting { frames } => {
                    if frames > 1 {
                        inner.step = Step::Resetting { frames: frames - 1 };
                    } else {
                        inner.reset();
                    }
                }
            }
            inner.owner
        };
        if let Some((run, delay)) = fallback {
            self.start_fallback(run, delay, window, cx);
        }
        if let Some(done) = done {
            done(window, cx);
        }
        self.after_step(owner, window, cx);
    }

    // Keeps frames coming while something moves or is due, and tells the
    // owner its parts look different.
    fn after_step(&self, owner: Option<EntityId>, window: &mut Window, cx: &mut App) {
        let needs_frames = self.inner.borrow().needs_frames(Instant::now());
        if needs_frames {
            self.ensure_frames(window);
        }
        if let Some(owner) = owner {
            cx.notify(owner);
        }
    }

    // The original's timeout: a wait ends a little after its longest
    // animation should have, even when frames stop coming.
    fn start_fallback(&self, run: u64, delay: Duration, window: &Window, cx: &App) {
        let presence = Rc::downgrade(&self.inner);
        let task = window.spawn(cx, async move |cx| {
            cx.background_executor().timer(delay).await;
            cx.update(|window, cx| {
                if let Some(inner) = presence.upgrade() {
                    Presence { inner }.time_out(run, window, cx);
                }
            })
            .ok();
        });
        self.inner.borrow_mut().fallback = Some(task);
    }

    fn time_out(&self, run: u64, window: &mut Window, cx: &mut App) {
        let (done, owner) = {
            let mut inner = self.inner.borrow_mut();
            let Step::Waiting { run: waiting, exit, .. } = inner.step else {
                return;
            };
            if waiting != run {
                return;
            }
            // This runs inside the fallback's own task, which ends with it.
            if let Some(task) = inner.fallback.take() {
                task.detach();
            }
            (inner.finish_wait(exit), inner.owner)
        };
        if let Some(done) = done {
            done(window, cx);
        }
        self.after_step(owner, window, cx);
    }
}

impl Default for Presence {
    fn default() -> Self {
        Self::new(false)
    }
}

impl fmt::Debug for Presence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Presence").field(&self.state()).finish()
    }
}

// A `done` for the owner, as the frame loop calls it: inside the owner's
// update, if the owner still lives.
pub(crate) fn erase<T: 'static>(
    done: impl FnOnce(&mut T, &mut Window, &mut Context<T>) + 'static,
    cx: &Context<T>,
) -> Done {
    let owner = cx.weak_entity();
    Box::new(move |window, cx| {
        owner.update(cx, |owner, cx| done(owner, window, cx)).ok();
    })
}

fn fallback_delay(animations: &[(u64, Instant)], now: Instant) -> Duration {
    let longest = animations
        .iter()
        .map(|&(_, end)| end.saturating_duration_since(now))
        .max()
        .unwrap_or_default();
    longest + Duration::from_millis(50)
}

impl Inner {
    fn allocate_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn state(&self) -> PresenceState {
        let now = self.frame_time;
        PresenceState {
            phase: self.phase(now),
            starting: self.starting,
            ending: self.ending,
            inert: self.inert,
            progress: self.shown.value(now),
        }
    }

    fn phase(&self, now: Instant) -> PresencePhase {
        let moving = self.restyle || self.shown.is_running(now);
        let waiting = match self.step {
            Step::Waiting { exit, .. } => Some(exit),
            _ => None,
        };
        if self.open {
            if self.starting || moving || waiting == Some(false) {
                PresencePhase::Entering
            } else {
                PresencePhase::Open
            }
        } else if moving || waiting == Some(true) {
            PresencePhase::Exiting
        } else {
            PresencePhase::Closed
        }
    }

    fn all(&self) -> impl Iterator<Item = &Transitioned> {
        std::iter::once(&self.shown).chain(&self.properties)
    }

    /// Whether a transition runs on the element now.
    pub(crate) fn transitioning(&self, now: Instant) -> bool {
        self.all().any(|transitioned| transitioned.is_running(now))
    }

    // Every animation running now, as `getAnimations` lists them.
    fn snapshot(&self, now: Instant) -> Vec<(u64, Instant)> {
        let transitions = self
            .all()
            .filter_map(|transitioned| transitioned.running.as_ref())
            .map(|running| (running.id, running.end()));
        transitions
            .chain(self.scripts.iter().copied())
            .filter(|&(_, end)| now < end)
            .collect()
    }

    // Whether an animation is still the one running: one replaced or canceled
    // has settled, as its `finished` promise does.
    fn is_live(&self, id: u64) -> bool {
        self.all()
            .any(|transitioned| transitioned.running.as_ref().is_some_and(|running| running.id == id))
            || self.scripts.iter().any(|&(script, _)| script == id)
    }

    fn settle(&mut self, now: Instant) {
        self.shown.settle(now);
        for property in &mut self.properties {
            property.settle(now);
        }
        self.scripts.retain(|&(_, end)| now < end);
    }

    fn needs_frames(&self, now: Instant) -> bool {
        self.restyle || !matches!(self.step, Step::Idle) || self.transitioning(now)
    }

    fn reset(&mut self) {
        self.ending = false;
        self.inert = false;
    }

    // A wait ended: an exit's ending style stays two frames more, since a
    // frame requested as animations finish can still run before this frame's
    // paint, and `done` runs after that's set up.
    fn finish_wait(&mut self, exit: bool) -> Option<Done> {
        self.step = if exit {
            Step::Resetting { frames: 2 }
        } else {
            Step::Idle
        };
        self.done.take()
    }

    // What the cleanup of the run in progress does. Returns whether it took
    // an exit's ending style off. With `restyle`, what it took off moves the
    // part on the next frame; a caller that styles the part again at once
    // passes false, as the browser sees both changes in one frame.
    fn stop_run(&mut self, restyle: bool) -> bool {
        self.done = None;
        self.fallback = None;
        let pending = std::mem::take(&mut self.restyle);
        let (changed, exited) = match std::mem::replace(&mut self.step, Step::Idle) {
            Step::Starting { .. } => {
                self.starting = false;
                (true, false)
            }
            Step::Waiting { exit: true, .. } | Step::Resetting { .. } => {
                self.reset();
                (true, true)
            }
            Step::Waiting { exit: false, .. } | Step::Idle => (false, false),
        };
        self.restyle = restyle && (pending || changed);
        exited
    }

    fn retarget_shown(&mut self, target: f32, timing: &Timing, reduce: bool, now: Instant) {
        let Inner { shown, next_id, .. } = self;
        shown.retarget(target, timing, reduce, now, next_id);
    }

    /// A style change of one of `properties` to `target`, as CSS transitions it.
    pub(crate) fn retarget_property(
        &mut self,
        index: usize,
        target: f32,
        timing: &Timing,
        reduce: bool,
        now: Instant,
    ) {
        let Inner {
            properties,
            next_id,
            ..
        } = self;
        if let Some(property) = properties.get_mut(index) {
            property.retarget(target, timing, reduce, now, next_id);
        }
    }
}

/// A number CSS transitions: the value it rests at, or the transition moving
/// it.
#[derive(Clone)]
pub(crate) struct Transitioned {
    rest: f32,
    running: Option<Running>,
}

#[derive(Clone)]
struct Running {
    id: u64,
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
    easing: Easing,
    reversing_adjusted_start: f32,
    shortening: f32,
}

impl Running {
    fn end(&self) -> Instant {
        self.start + self.duration
    }

    // The timing function's output at `now`.
    fn output(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        let duration = self.duration.as_secs_f32();
        let input = if duration > 0.0 {
            (elapsed / duration).clamp(0.0, 1.0)
        } else {
            1.0
        };
        (self.easing)(input)
    }

    fn value(&self, now: Instant) -> f32 {
        if now >= self.end() {
            self.to
        } else {
            self.from + (self.to - self.from) * self.output(now)
        }
    }
}

impl Transitioned {
    pub(crate) fn new(value: f32) -> Self {
        Self {
            rest: value,
            running: None,
        }
    }

    pub(crate) fn value(&self, now: Instant) -> f32 {
        match &self.running {
            Some(running) => running.value(now),
            None => self.rest,
        }
    }

    pub(crate) fn is_running(&self, now: Instant) -> bool {
        self.running.as_ref().is_some_and(|running| now < running.end())
    }

    /// Shows `value` with nothing moving, like a finished or canceled
    /// transition.
    pub(crate) fn jump(&mut self, value: f32) {
        self.running = None;
        self.rest = value;
    }

    fn settle(&mut self, now: Instant) {
        if let Some(running) = &self.running
            && now >= running.end()
        {
            self.rest = running.to;
            self.running = None;
        }
    }

    // A style change to `target`, by CSS Transitions' rules: a transition
    // starts from the current value, one already heading there carries on,
    // and one heading back to where it came from is a reversal, shortened by
    // how far it had gone.
    fn retarget(
        &mut self,
        target: f32,
        timing: &Timing,
        reduce: bool,
        now: Instant,
        next_id: &mut u64,
    ) {
        self.settle(now);
        let duration = if reduce { Duration::ZERO } else { timing.duration };
        let current = self.value(now);
        let mut start = |from: f32, duration: Duration, reversing_adjusted_start: f32, shortening: f32| {
            *next_id += 1;
            Running {
                id: *next_id,
                from,
                to: target,
                start: now,
                duration,
                easing: timing.easing.clone(),
                reversing_adjusted_start,
                shortening,
            }
        };
        let Some(running) = &self.running else {
            if current != target && !duration.is_zero() {
                self.running = Some(start(current, duration, current, 1.0));
            } else {
                self.rest = target;
            }
            return;
        };
        if running.to == target {
            return;
        }
        if current == target || duration.is_zero() {
            self.jump(target);
            return;
        }
        if target == running.reversing_adjusted_start {
            let shortening = (running.output(now) * running.shortening + (1.0 - running.shortening))
                .abs()
                .clamp(0.0, 1.0);
            let shortening = if shortening.is_finite() { shortening } else { 1.0 };
            let shortened = duration.mul_f32(shortening);
            if shortened.is_zero() {
                self.jump(target);
                return;
            }
            let reversing_adjusted_start = running.to;
            self.running = Some(start(current, shortened, reversing_adjusted_start, shortening));
            return;
        }
        self.running = Some(start(current, duration, current, 1.0));
    }
}
