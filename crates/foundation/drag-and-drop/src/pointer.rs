//! Port of `foundation/drag-and-drop/pointer.ts`: pointer gestures that
//! sliders, scrubbers and steppers share: following one pointer until it
//! lifts, and repeating an action while a press is held.
//!
//! The original captures the pointer and listens on the document. gpui hears
//! a window's pointer only through listeners an element registers while it
//! paints, so a drag follows the pointer through its
//! [`listener`](DragSession::listener): an element the component renders on
//! every frame, anywhere in its tree, which registers window listeners each
//! time it paints. They hear every move and the release wherever the pointer
//! goes, inside the window and out of it, as pointer capture does. They paint
//! last, so they hear the release in its capture phase after every element,
//! and in its bubble phase before any.
//!
//! A press is a [`PointerEvent`]: a mouse button going down, or a touch gpui
//! offers as a drag (a [`TouchDragEvent`] starting). gpui decides what a
//! touch is as it starts, so following one claims it: it never turns into a
//! tap or a scroll, the original's `touch-action: none`. [`on_pointer_down`]
//! hears both kinds on a part. Every handler that hears the same press sees
//! the same press, as the original's handlers share one event object, which
//! is how a drag tells its own press bubbling to a part around it from a new
//! one, and how [`is_drag_press`] knows a press a drag follows.
//!
//! Callbacks run inside the component's update with the component itself,
//! like gpui's `cx.listener`, so they read and change its state directly.
//! The calls that can end a drag at once, [`DragSession::accepts`] and
//! [`DragSession::cancel`], take the component too, since they run its
//! `on_end` before they return.
//!
//! A drag's release would click what it ends on. gpui fires a click in the
//! release's bubble phase, so the listener [`swallow_next_click`] arms, which
//! every session's element and [`swallow_listener`] paint last, stops the
//! release before any element hears it: every `on_press` and `on_click`
//! stays as it is and never sees it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, Context, DispatchPhase, HitboxBehavior, InteractiveElement, IntoElement,
    KeystrokeEvent, LongPressEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, ScrollEvent, ScrollSource, Styled, Subscription,
    Task, TouchDragEvent, TouchPhase, WeakEntity, Window, WindowId, canvas, deferred, px,
};
use ui_sdk_kernel::{InputEvent, Key, is_composing};
use web_time::Instant;

/// What kind of pointer it is (`pointerType`): a mouse or trackpad, or a
/// finger on a touch screen. gpui reports pens as one or the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerKind {
    Mouse,
    Touch,
}

impl PointerKind {
    /// The original's name for it, like `"mouse"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PointerKind::Mouse => "mouse",
            PointerKind::Touch => "touch",
        }
    }
}

/// What the pointer did, the event's `type`: pressed, moved, lifted, or was
/// taken away, like a touch the system took over (`pointercancel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    Cancel,
}

/// A pointer event as a drag sees it (`PointerEvent`): a mouse's press, move
/// or release, or a step of a touch gpui offers as a drag. `event` is the
/// gpui event behind it, for a change's details.
///
/// Make a press with [`PointerEvent::mouse_down`] or
/// [`PointerEvent::touch_drag`], which know the press every handler of it
/// hears; moves and releases convert with `From`.
#[derive(Clone, Debug)]
pub struct PointerEvent {
    pub kind: PointerKind,
    pub phase: PointerPhase,
    /// Where the pointer is, in window coordinates (`clientX`, `clientY`).
    pub position: Point<Pixels>,
    /// The button pressed or released. A touch is the left button.
    pub button: MouseButton,
    /// The modifier keys held. A touch holds none.
    pub modifiers: Modifiers,
    /// How many presses in a row this one is, like 2 for a double click.
    pub click_count: usize,
    /// When it arrived (`timeStamp`). gpui's events carry no time, so it's
    /// when this was made from one.
    pub time: Instant,
    pub event: InputEvent,
    // Which press this is, the same for every handler that hears it; 0 for
    // events that aren't presses.
    press: u64,
}

impl PointerEvent {
    /// A mouse press, as every handler of it in `window` sees it. A tap's
    /// press, which gpui hands over as a mouse press once the finger lifts,
    /// works as one, but its event is a finger's (`InputEvent::TapDown`),
    /// so what it changes reports a touch.
    pub fn mouse_down(event: &MouseDownEvent, window: &Window, cx: &mut App) -> Self {
        let mut pointer = Self {
            kind: PointerKind::Mouse,
            phase: PointerPhase::Down,
            position: event.position,
            button: event.button,
            modifiers: event.modifiers,
            click_count: event.click_count,
            time: Instant::now(),
            event: InputEvent::mouse_down(event, window),
            press: 0,
        };
        pointer.press = press_serial(&pointer, window, cx);
        pointer
    }

    /// A step of a touch gpui offers as a drag. Its start is a press, as
    /// every handler of it in `window` sees it.
    pub fn touch_drag(event: &TouchDragEvent, window: &Window, cx: &mut App) -> Self {
        let mut pointer = Self::touch(event);
        if pointer.phase == PointerPhase::Down {
            pointer.press = press_serial(&pointer, window, cx);
        }
        pointer
    }

    // A step of a touch drag, with no press identity.
    fn touch(event: &TouchDragEvent) -> Self {
        Self {
            kind: PointerKind::Touch,
            phase: match event.phase {
                TouchPhase::Started => PointerPhase::Down,
                TouchPhase::Moved => PointerPhase::Move,
                TouchPhase::Ended => PointerPhase::Up,
                TouchPhase::Cancelled => PointerPhase::Cancel,
            },
            position: event.position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
            time: Instant::now(),
            event: InputEvent::from(event),
            press: 0,
        }
    }

    /// The horizontal position, `clientX`.
    pub fn x(&self) -> Pixels {
        self.position.x
    }

    /// The vertical position, `clientY`.
    pub fn y(&self) -> Pixels {
        self.position.y
    }

    /// Whether `other` is the same event, as two handlers of one press see
    /// it: the same press, or for moves and releases, the same values.
    pub fn is_same(&self, other: &PointerEvent) -> bool {
        if self.press != 0 || other.press != 0 {
            return self.press == other.press;
        }
        self.kind == other.kind
            && self.phase == other.phase
            && self.position == other.position
            && self.button == other.button
            && self.modifiers == other.modifiers
            && self.click_count == other.click_count
    }
}

impl From<&MouseMoveEvent> for PointerEvent {
    fn from(event: &MouseMoveEvent) -> Self {
        Self {
            kind: PointerKind::Mouse,
            phase: PointerPhase::Move,
            position: event.position,
            button: event.pressed_button.unwrap_or(MouseButton::Left),
            modifiers: event.modifiers,
            click_count: 0,
            time: Instant::now(),
            event: InputEvent::from(event),
            press: 0,
        }
    }
}

impl From<&MouseUpEvent> for PointerEvent {
    fn from(event: &MouseUpEvent) -> Self {
        Self {
            kind: PointerKind::Mouse,
            phase: PointerPhase::Up,
            position: event.position,
            button: event.button,
            modifiers: event.modifiers,
            click_count: event.click_count,
            time: Instant::now(),
            event: InputEvent::from(event),
            press: 0,
        }
    }
}

// The press being dispatched now, in the window it's dispatched to, so
// every handler that hears it makes the same press of it. It's forgotten
// once the dispatch is over.
struct CurrentPress {
    window: WindowId,
    kind: PointerKind,
    position: Point<Pixels>,
    button: MouseButton,
    modifiers: Modifiers,
    click_count: usize,
    serial: u64,
}

thread_local! {
    static CURRENT_PRESS: RefCell<Option<CurrentPress>> = const { RefCell::new(None) };
    static NEXT_PRESS: Cell<u64> = const { Cell::new(1) };
}

fn press_serial(press: &PointerEvent, window: &Window, cx: &mut App) -> u64 {
    let window_id = window.window_handle().window_id();
    let known = CURRENT_PRESS.with(|current| {
        current
            .borrow()
            .as_ref()
            .filter(|current| {
                current.window == window_id
                    && current.kind == press.kind
                    && current.position == press.position
                    && current.button == press.button
                    && current.modifiers == press.modifiers
                    && current.click_count == press.click_count
            })
            .map(|current| current.serial)
    });
    if let Some(serial) = known {
        return serial;
    }
    let serial = NEXT_PRESS.with(|next| {
        let serial = next.get();
        next.set(serial + 1);
        serial
    });
    CURRENT_PRESS.with(|current| {
        *current.borrow_mut() = Some(CurrentPress {
            window: window_id,
            kind: press.kind,
            position: press.position,
            button: press.button,
            modifiers: press.modifiers,
            click_count: press.click_count,
            serial,
        });
    });
    window.defer(cx, move |_, _| {
        CURRENT_PRESS.with(|current| {
            let mut current = current.borrow_mut();
            if current.as_ref().is_some_and(|current| current.serial == serial) {
                *current = None;
            }
        });
    });
    serial
}

/// Where the pointer is, and how far it moved since the last report. A
/// callback gets it by reference each report, so read it, don't keep it.
#[derive(Clone, Debug)]
pub struct PointerSample {
    pub x: Pixels,
    pub y: Pixels,
    /// Accumulated since the previous report. Under pointer lock, the only
    /// movement there is.
    pub movement_x: Pixels,
    pub movement_y: Pixels,
    /// Whether the pointer is locked, so the real cursor is hidden and still.
    /// gpui can't lock the pointer, so it never is.
    pub locked: bool,
    /// The latest event, a move or the press itself.
    pub event: PointerEvent,
}

impl PointerSample {
    /// Where the pointer is.
    pub fn position(&self) -> Point<Pixels> {
        Point::new(self.x, self.y)
    }

    /// How far it moved since the last report.
    pub fn movement(&self) -> Point<Pixels> {
        Point::new(self.movement_x, self.movement_y)
    }
}

/// What `on_move` runs: at most once per frame, with the latest position and
/// the movement since the last call.
pub type DragMoveCallback<T> = Rc<dyn Fn(&mut T, &PointerSample, &mut Window, &mut Context<T>)>;

/// What `on_end` runs: with the event that ended it, and whether it was
/// canceled.
pub type DragEndCallback<T> =
    Rc<dyn Fn(&mut T, &PointerEvent, bool, &mut Window, &mut Context<T>)>;

/// What `on_start` runs, on the first move past the threshold. False stops
/// following the pointer.
pub type DragStartCallback<T> =
    Rc<dyn Fn(&mut T, &PointerEvent, &mut Window, &mut Context<T>) -> bool>;

/// What [`DragSession::start_on_move`]'s `pick` runs for each move while it
/// waits: `None` to keep waiting, `Some(false)` to let the press go, like a
/// scroll, or `Some(true)` to follow it from that move.
pub type DragPickCallback<T> =
    Rc<dyn Fn(&mut T, &PointerEvent, &mut Window, &mut Context<T>) -> Option<bool>>;

/// What [`DragSession::start_on_move`]'s `on_tap` runs, for a press that
/// lifted before `pick` decided.
pub type DragTapCallback<T> = Rc<dyn Fn(&mut T, &PointerEvent, &mut Window, &mut Context<T>)>;

/// What [`bind_drag`] reports, and when (`BindDragOptions`). `T` is the
/// component the callbacks run in.
pub struct BindDragOptions<T: 'static> {
    /// At most once per frame, with the latest position and the movement
    /// since the last call.
    pub on_move: Option<DragMoveCallback<T>>,
    /// The pointer lifted, or the system took it over, like a touch it
    /// canceled. A move still pending runs first, so the last position is
    /// never lost.
    pub on_end: Option<DragEndCallback<T>>,
    /// How far the pointer moves along either axis before moves are
    /// reported, so a press that barely moves stays a click. Defaults to 0.
    pub threshold: Pixels,
    /// Runs once, on the first move past `threshold`, before its `on_move`.
    /// Return false to stop following the pointer, like when a drag can't
    /// start; `on_end` doesn't run then.
    pub on_start: Option<DragStartCallback<T>>,
    /// Locks the pointer once it has moved past [`DRAG_THRESHOLD`], so it can
    /// travel without limit, like a scrub, while a plain click never hides
    /// the cursor. gpui can't lock the pointer, so this falls back to plain
    /// movement, as the original does where the browser refuses.
    pub lock: bool,
    /// Ends the gesture as canceled when Escape is pressed once it has
    /// started, and keeps that press from reaching anything else, like a
    /// popup around it. `on_end` gets the last pointer event. Defaults to
    /// false.
    pub cancel_on_escape: bool,
}

impl<T: 'static> Default for BindDragOptions<T> {
    fn default() -> Self {
        Self {
            on_move: None,
            on_end: None,
            threshold: px(0.),
            on_start: None,
            lock: false,
            cancel_on_escape: false,
        }
    }
}

impl<T: 'static> Clone for BindDragOptions<T> {
    fn clone(&self) -> Self {
        Self {
            on_move: self.on_move.clone(),
            on_end: self.on_end.clone(),
            threshold: self.threshold,
            on_start: self.on_start.clone(),
            lock: self.lock,
            cancel_on_escape: self.cancel_on_escape,
        }
    }
}

impl<T: 'static> BindDragOptions<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_move(
        mut self,
        on_move: impl Fn(&mut T, &PointerSample, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        self.on_move = Some(Rc::new(on_move));
        self
    }

    pub fn on_end(
        mut self,
        on_end: impl Fn(&mut T, &PointerEvent, bool, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        self.on_end = Some(Rc::new(on_end));
        self
    }

    pub fn threshold(mut self, threshold: Pixels) -> Self {
        self.threshold = threshold;
        self
    }

    pub fn on_start(
        mut self,
        on_start: impl Fn(&mut T, &PointerEvent, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        self.on_start = Some(Rc::new(on_start));
        self
    }

    pub fn lock(mut self, lock: bool) -> Self {
        self.lock = lock;
        self
    }

    pub fn cancel_on_escape(mut self, cancel_on_escape: bool) -> Self {
        self.cancel_on_escape = cancel_on_escape;
        self
    }
}

/// What [`DragSession::start_on_move`] takes: the drag's options, with how a
/// move decides whether it's a drag and what a tap does.
pub struct StartOnMoveOptions<T: 'static> {
    pub drag: BindDragOptions<T>,
    /// Decides for each move while it waits: `None` to keep waiting,
    /// `Some(false)` to let it go, like a scroll, or `Some(true)` to follow
    /// it from that move. Without it, the first move follows.
    pub pick: Option<DragPickCallback<T>>,
    /// The press lifted before `pick` decided.
    pub on_tap: Option<DragTapCallback<T>>,
}

impl<T: 'static> Default for StartOnMoveOptions<T> {
    fn default() -> Self {
        Self {
            drag: BindDragOptions::default(),
            pick: None,
            on_tap: None,
        }
    }
}

impl<T: 'static> StartOnMoveOptions<T> {
    pub fn new(drag: BindDragOptions<T>) -> Self {
        Self {
            drag,
            ..Self::default()
        }
    }

    pub fn pick(
        mut self,
        pick: impl Fn(&mut T, &PointerEvent, &mut Window, &mut Context<T>) -> Option<bool> + 'static,
    ) -> Self {
        self.pick = Some(Rc::new(pick));
        self
    }

    pub fn on_tap(
        mut self,
        on_tap: impl Fn(&mut T, &PointerEvent, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        self.on_tap = Some(Rc::new(on_tap));
        self
    }
}

/// How far a press moves, in pixels, before it counts as a drag rather than
/// a click (`dragThreshold`).
pub const DRAG_THRESHOLD: Pixels = px(3.);

/// Whether a movement from where the press started is a drag (`isDrag`).
pub fn is_drag(dx: Pixels, dy: Pixels) -> bool {
    dx.abs() >= DRAG_THRESHOLD || dy.abs() >= DRAG_THRESHOLD
}

// The paint priority of the listeners a drag's element registers: above
// every other deferred draw, so the release reaches them last in the capture
// phase and first in the bubble phase, before any element's click.
const LISTENER_PRIORITY: usize = usize::MAX;

// One pointer followed until it lifts (`bindDrag`'s closure state).
struct Drag<T: 'static> {
    entity: WeakEntity<T>,
    window: AnyWindowHandle,
    // The session it's the running drag of, which it lets go of as it stops.
    session: Weak<RefCell<SessionInner<T>>>,
    down: PointerEvent,
    options: BindDragOptions<T>,
    // `pending`, reported once per frame while `has_pending`.
    pending: PointerSample,
    has_pending: bool,
    // The generation of the flush the next frame runs; 0 when none is asked for.
    frame: u64,
    frames: u64,
    last: Point<Pixels>,
    started: bool,
    dragged: bool,
    stopped: bool,
    escape: Option<Subscription>,
}

type DragRef<T> = Rc<RefCell<Drag<T>>>;

impl<T: 'static> Drag<T> {
    fn new(
        down: &PointerEvent,
        options: BindDragOptions<T>,
        session: Weak<RefCell<SessionInner<T>>>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> DragRef<T> {
        follow(down);
        // gpui gives a touch to the first listener that claims it as it
        // starts; following it claims it.
        if down.kind == PointerKind::Touch {
            window.prevent_default();
        }
        let started = options.threshold <= px(0.) && options.on_start.is_none();
        let escape = started && options.cancel_on_escape;
        let drag = Rc::new(RefCell::new(Drag {
            entity: cx.weak_entity(),
            window: window.window_handle(),
            session,
            down: down.clone(),
            pending: PointerSample {
                x: px(0.),
                y: px(0.),
                movement_x: px(0.),
                movement_y: px(0.),
                locked: false,
                event: down.clone(),
            },
            has_pending: false,
            frame: 0,
            frames: 0,
            last: down.position,
            started,
            dragged: false,
            stopped: false,
            escape: None,
            options,
        }));
        if escape {
            Self::listen_for_escape(&drag, cx);
        }
        drag
    }

    // Listens for Escape ahead of every element and binding, like a
    // document's keydown listener in the capture phase.
    fn listen_for_escape(drag: &DragRef<T>, cx: &mut App) {
        let weak = Rc::downgrade(drag);
        let subscription = cx.intercept_keystrokes(move |event: &KeystrokeEvent, window, cx| {
            if Key::of(&event.keystroke) != Key::Escape || is_composing(window, cx) {
                return;
            }
            let Some(drag) = weak.upgrade() else {
                return;
            };
            let (dragged, last) = {
                let state = drag.borrow();
                if state.stopped || state.window != window.window_handle() {
                    return;
                }
                (state.dragged, state.pending.event.clone())
            };
            window.prevent_default();
            cx.stop_propagation();
            if dragged {
                swallow_next_click(true, window, cx);
            }
            Self::end(&drag, last, true, window, cx);
        });
        drag.borrow_mut().escape = Some(subscription);
    }

    fn moved(drag: &DragRef<T>, event: PointerEvent, window: &mut Window, cx: &mut App) {
        let (threshold, start, from) = {
            let state = drag.borrow();
            if state.stopped {
                return;
            }
            (state.options.threshold, !state.started, state.down.position)
        };
        if start {
            let dx = (event.position.x - from.x).abs();
            let dy = (event.position.y - from.y).abs();
            if threshold > px(0.) && dx < threshold && dy < threshold {
                return;
            }
            let (on_start, entity, escape) = {
                let mut state = drag.borrow_mut();
                state.started = true;
                (
                    state.options.on_start.clone(),
                    state.entity.clone(),
                    state.options.cancel_on_escape,
                )
            };
            if let Some(on_start) = on_start {
                let go = entity
                    .update(cx, |this, cx| on_start(this, &event, window, cx))
                    .unwrap_or(false);
                if !go {
                    Self::stop(drag);
                    return;
                }
                if drag.borrow().stopped {
                    return;
                }
            }
            if escape {
                Self::listen_for_escape(drag, cx);
            }
        }
        let schedule = {
            let mut state = drag.borrow_mut();
            if !state.dragged && is_drag(event.position.x - from.x, event.position.y - from.y) {
                // `lock` asks for pointer lock here in the original, which
                // gpui doesn't have.
                state.dragged = true;
            }
            let last = state.last;
            state.pending.movement_x += event.position.x - last.x;
            state.pending.movement_y += event.position.y - last.y;
            state.pending.x = event.position.x;
            state.pending.y = event.position.y;
            state.last = event.position;
            state.pending.locked = false;
            state.pending.event = event;
            state.has_pending = true;
            if state.frame == 0 {
                state.frames += 1;
                state.frame = state.frames;
                Some(state.frame)
            } else {
                None
            }
        };
        if let Some(frame) = schedule {
            let weak = Rc::downgrade(drag);
            window.on_next_frame(move |window, cx| {
                let Some(drag) = weak.upgrade() else {
                    return;
                };
                if drag.borrow().frame != frame {
                    return;
                }
                let entity = drag.borrow().entity.clone();
                entity
                    .update(cx, |this, cx| Self::flush(&drag, this, window, cx))
                    .ok();
            });
        }
    }

    // Reports the pending move, if there is one.
    fn flush(drag: &DragRef<T>, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let (on_move, sample) = {
            let mut state = drag.borrow_mut();
            state.frame = 0;
            if !state.has_pending || state.stopped {
                return;
            }
            state.has_pending = false;
            let sample = state.pending.clone();
            state.pending.movement_x = px(0.);
            state.pending.movement_y = px(0.);
            (state.options.on_move.clone(), sample)
        };
        if let Some(on_move) = on_move {
            on_move(this, &sample, window, cx);
        }
    }

    fn released(drag: &DragRef<T>, event: PointerEvent, window: &mut Window, cx: &mut App) {
        let dragged = {
            let state = drag.borrow();
            if state.stopped {
                return;
            }
            state.dragged
        };
        if dragged {
            swallow_next_click(false, window, cx);
        }
        Self::end(drag, event, false, window, cx);
    }

    // Stops following the pointer without `on_end`, and leaves the way
    // clear for the session's next press.
    fn stop(drag: &DragRef<T>) {
        let (escape, session) = {
            let mut state = drag.borrow_mut();
            state.stopped = true;
            state.frame = 0;
            state.has_pending = false;
            (state.escape.take(), state.session.clone())
        };
        drop(escape);
        if let Some(session) = session.upgrade()
            && let Ok(mut session) = session.try_borrow_mut()
            && session
                .running
                .as_ref()
                .is_some_and(|running| matches!(running, Running::Drag(running) if Rc::ptr_eq(running, drag)))
        {
            session.running = None;
        }
    }

    fn end(drag: &DragRef<T>, event: PointerEvent, canceled: bool, window: &mut Window, cx: &mut App) {
        let entity = drag.borrow().entity.clone();
        entity
            .update(cx, |this, cx| Self::end_in(drag, this, &event, canceled, window, cx))
            .ok();
    }

    fn end_in(
        drag: &DragRef<T>,
        this: &mut T,
        event: &PointerEvent,
        canceled: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if drag.borrow().stopped {
            return;
        }
        Self::flush(drag, this, window, cx);
        let on_end = drag.borrow().options.on_end.clone();
        Self::stop(drag);
        if let Some(on_end) = on_end {
            on_end(this, event, canceled, window, cx);
        }
    }

    fn hear(drag: &DragRef<T>, event: Heard, window: &mut Window, cx: &mut App) {
        let (kind, button, own) = {
            let state = drag.borrow();
            (state.down.kind, state.down.button, state.window == window.window_handle())
        };
        if !own {
            return;
        }
        match event {
            Heard::MouseMove(event) if kind == PointerKind::Mouse => {
                // A mouse released where the window missed the release.
                if event.pressed_button.is_none() {
                    return Self::end(drag, PointerEvent::from(&event), false, window, cx);
                }
                Self::moved(drag, PointerEvent::from(&event), window, cx)
            }
            Heard::MouseUp(event) if kind == PointerKind::Mouse && event.button == button => {
                Self::released(drag, PointerEvent::from(&event), window, cx)
            }
            Heard::Touch(event) if kind == PointerKind::Touch => match event.phase {
                TouchPhase::Moved => Self::moved(drag, PointerEvent::touch(&event), window, cx),
                TouchPhase::Ended => Self::released(drag, PointerEvent::touch(&event), window, cx),
                TouchPhase::Cancelled => {
                    Self::end(drag, PointerEvent::touch(&event), true, window, cx)
                }
                TouchPhase::Started => {}
            },
            _ => {}
        }
    }
}

// What a drag's element hears and hands on.
enum Heard {
    MouseMove(MouseMoveEvent),
    MouseUp(MouseUpEvent),
    Touch(TouchDragEvent),
}

thread_local! {
    // The presses drags follow, newest last, so a gesture around one, like
    // a sheet's swipe, leaves it alone.
    static FOLLOWED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

fn follow(down: &PointerEvent) {
    if down.press == 0 {
        return;
    }
    FOLLOWED.with(|followed| {
        let mut followed = followed.borrow_mut();
        if followed.len() >= 32 {
            followed.remove(0);
        }
        followed.push(down.press);
    });
}

/// Whether a drag follows the press `event` (`isDragPress`), so a gesture
/// around it, like a sheet's swipe, leaves it alone. `event` is the press,
/// as [`PointerEvent::mouse_down`] or [`PointerEvent::touch_drag`] made it.
pub fn is_drag_press(event: &PointerEvent) -> bool {
    event.press != 0 && FOLLOWED.with(|followed| followed.borrow().contains(&event.press))
}

/// Follows the pointer that pressed, for a drag of your own: a slider, a
/// splitter, an item dragged to a new place (`bindDrag`). The drag keeps
/// going outside the element and outside the window, as long as the
/// returned handle's [`listener`](BoundDrag::listener) renders. With a
/// `threshold`, nothing is reported until the pointer passes it. Moves
/// arrive at most once per frame. A release after the pointer moved past
/// [`DRAG_THRESHOLD`] swallows the click it sends, so a drag never presses
/// what it ends on. Keep the handle in a field: dropping it, or calling
/// [`stop`](BoundDrag::stop), ends the drag without `on_end`, the
/// original's returned cleanup. Any button can drag.
///
/// ```ignore
/// fn press(&mut self, event: &PointerEvent, window: &mut Window, cx: &mut Context<Self>) {
///     self.drag = Some(bind_drag(event, BindDragOptions::new()
///         .threshold(DRAG_THRESHOLD)
///         .cancel_on_escape(true)
///         .on_move(|this: &mut Self, sample, _window, cx| this.resize_to(sample.x, cx))
///         .on_end(|this, _event, canceled, _window, cx| {
///             if canceled { this.restore(cx) } else { this.commit(cx) }
///         }),
///         window, cx));
/// }
/// ```
pub fn bind_drag<T: 'static>(
    down: &PointerEvent,
    options: BindDragOptions<T>,
    window: &mut Window,
    cx: &mut Context<T>,
) -> BoundDrag<T> {
    let inner = Rc::new(RefCell::new(SessionInner {
        entity: cx.weak_entity(),
        running: None,
    }));
    let drag = Drag::new(down, options, Rc::downgrade(&inner), window, cx);
    inner.borrow_mut().running = Some(Running::Drag(drag));
    BoundDrag { inner }
}

/// A drag [`bind_drag`] follows. Dropping it ends the drag without `on_end`.
pub struct BoundDrag<T: 'static> {
    inner: Rc<RefCell<SessionInner<T>>>,
}

impl<T: 'static> BoundDrag<T> {
    /// Ends the drag without calling `on_end`, the original's cleanup.
    pub fn stop(&self) {
        let running = self.inner.borrow_mut().running.take();
        if let Some(running) = running {
            running.stop();
        }
    }

    /// Whether the pointer is still followed.
    pub fn is_running(&self) -> bool {
        self.inner.borrow().running.is_some()
    }

    /// The element that follows the pointer for the drag. Render it on
    /// every frame while the drag lasts, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T> {
        listener_for(Rc::downgrade(&self.inner))
    }
}

impl<T: 'static> Drop for BoundDrag<T> {
    fn drop(&mut self) {
        self.stop();
    }
}

// A press waiting for `pick` to decide (`startOnMove`'s waiting state).
struct Waiting<T: 'static> {
    down: PointerEvent,
    window: AnyWindowHandle,
    options: BindDragOptions<T>,
    pick: Option<DragPickCallback<T>>,
    on_tap: Option<DragTapCallback<T>>,
    stopped: bool,
}

type WaitingRef<T> = Rc<RefCell<Waiting<T>>>;

enum Running<T: 'static> {
    Drag(DragRef<T>),
    Waiting(WaitingRef<T>),
}

impl<T: 'static> Clone for Running<T> {
    fn clone(&self) -> Self {
        match self {
            Running::Drag(drag) => Running::Drag(drag.clone()),
            Running::Waiting(waiting) => Running::Waiting(waiting.clone()),
        }
    }
}

impl<T: 'static> Running<T> {
    fn down(&self) -> PointerEvent {
        match self {
            Running::Drag(drag) => drag.borrow().down.clone(),
            Running::Waiting(waiting) => waiting.borrow().down.clone(),
        }
    }

    fn stop(&self) {
        match self {
            Running::Drag(drag) => Drag::stop(drag),
            Running::Waiting(waiting) => waiting.borrow_mut().stopped = true,
        }
    }
}

struct SessionInner<T: 'static> {
    entity: WeakEntity<T>,
    running: Option<Running<T>>,
}

impl<T: 'static> Drop for SessionInner<T> {
    // The component went away mid-drag. The original ends the drag as
    // canceled so its draft goes back; here the draft goes with the
    // component, and there's no window to call `on_end` with, so the drag
    // just stops.
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop();
        }
    }
}

impl<T: 'static> SessionInner<T> {
    fn hear(inner: &Weak<RefCell<SessionInner<T>>>, event: Heard, window: &mut Window, cx: &mut App) {
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let running = inner.borrow().running.clone();
        match running {
            Some(Running::Drag(drag)) => Drag::hear(&drag, event, window, cx),
            Some(Running::Waiting(waiting)) => {
                DragSession::hear_waiting(&inner, &waiting, event, window, cx)
            }
            None => {}
        }
    }
}

// The element a session or a bound drag renders: each frame it paints, it
// registers the window listeners that follow its pointer, and the ones that
// swallow a click, which run first in the release's bubble phase.
fn listener_for<T: 'static>(inner: Weak<RefCell<SessionInner<T>>>) -> impl IntoElement {
    deferred(
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                listen_for_swallow(window);
                let session = inner.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        SessionInner::hear(&session, Heard::MouseMove(event.clone()), window, cx);
                    }
                });
                let session = inner.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        SessionInner::hear(&session, Heard::MouseUp(event.clone()), window, cx);
                    }
                });
                let session = inner;
                window.on_mouse_event(move |event: &TouchDragEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        SessionInner::hear(&session, Heard::Touch(event.clone()), window, cx);
                    }
                });
            },
        )
        .absolute(),
    )
    .with_priority(LISTENER_PRIORITY)
}

/// One drag at a time for a component (`createDragSession`). A press starts
/// one only with the main button, while no other runs. A drag cut short,
/// like by [`cancel`](Self::cancel), ends as canceled, so its draft goes
/// back. Make it in the component's constructor, or for its entity anywhere
/// ([`for_entity`](Self::for_entity)), keep it in a field, and render its
/// [`listener`](Self::listener) on every frame. Dropping it, as the
/// component goes, stops a running drag without `on_end`, since there's
/// nothing left to put back. Cloning it gives another handle to the same
/// session.
///
/// ```ignore
/// fn press(&mut self, event: &PointerEvent, window: &mut Window, cx: &mut Context<Self>) {
///     let drags = self.drags.clone();
///     if !drags.accepts(self, event, window, cx) || self.disabled {
///         return;
///     }
///     drags.start(event, BindDragOptions::new()
///         .on_move(|this: &mut Self, sample, window, cx| this.move_to(sample.position(), window, cx))
///         .on_end(|this, event, canceled, window, cx| this.end(event, canceled, window, cx)),
///         window, cx);
/// }
/// ```
pub struct DragSession<T: 'static> {
    inner: Rc<RefCell<SessionInner<T>>>,
}

impl<T: 'static> Clone for DragSession<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// A [`DragSession`] for the component `cx` is for (`createDragSession`).
pub fn create_drag_session<T: 'static>(cx: &Context<T>) -> DragSession<T> {
    DragSession::new(cx)
}

impl<T: 'static> DragSession<T> {
    /// A session for the component `cx` is for.
    pub fn new(cx: &Context<T>) -> Self {
        Self::for_entity(cx.weak_entity())
    }

    /// A session for the component `entity`, made where there's no context
    /// for it, like a behavior a part asks for as it renders.
    pub fn for_entity(entity: WeakEntity<T>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(SessionInner {
                entity,
                running: None,
            })),
        }
    }

    /// The element that follows the session's pointer. Render it on every
    /// frame, anywhere in the component, like inside its root part: drags
    /// hear the pointer only while it paints.
    pub fn listener(&self) -> impl IntoElement + use<T> {
        listener_for(Rc::downgrade(&self.inner))
    }

    /// Adds [`listener`](Self::listener) to `el` as a child.
    pub fn track<E: ParentElement>(&self, el: E) -> E {
        el.child(self.listener())
    }

    /// Whether a drag runs, or a press waits to become one.
    pub fn is_running(&self) -> bool {
        self.inner.borrow().running.is_some()
    }

    /// Whether a press can start a drag now. Check it first, before the
    /// component's own guards: ending a stale drag can change what they
    /// read, which is why it takes the component, whose `on_end` that runs
    /// with.
    ///
    /// Only the main button starts a drag. A touch another drag already
    /// claimed, which gpui reports as `window.default_prevented()`, can't.
    /// A mouse press an inner element handled stops propagating in gpui, so
    /// it never gets here; `default_prevented()` during a mouse press means
    /// a focusable element took focus, which doesn't stop a drag. gpui
    /// follows one touch at a time, so a second finger never presses.
    pub fn accepts(
        &self,
        this: &mut T,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        if event.button != MouseButton::Left {
            return false;
        }
        if event.kind == PointerKind::Touch && window.default_prevented() {
            return false;
        }
        // A new press of the same kind means the running drag missed its
        // release. The press that started it, bubbling to a part around,
        // like a handle's to its box, doesn't.
        let stale = self.inner.borrow().running.as_ref().is_some_and(|running| {
            let down = running.down();
            !down.is_same(event) && down.kind == event.kind
        });
        if stale {
            self.cancel(this, window, cx);
        }
        self.inner.borrow().running.is_none()
    }

    /// Follows the press, like [`bind_drag`], as this component's one drag.
    /// Call it from the press's handler: a touch is claimed as it starts.
    pub fn start(
        &self,
        down: &PointerEvent,
        options: BindDragOptions<T>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drag = Drag::new(down, options, Rc::downgrade(&self.inner), window, cx);
        let replaced = self.inner.borrow_mut().running.replace(Running::Drag(drag));
        if let Some(replaced) = replaced {
            replaced.stop();
        }
    }

    /// Waits for the press to move until `pick` decides: `None` to keep
    /// waiting, `Some(false)` to let it go, like a scroll, or `Some(true)` to
    /// follow it from that move, like [`start`](Self::start). A mouse press
    /// that lifts first stays a click, and `on_tap` hears it. A touch is
    /// claimed as it starts, since gpui decides then, so one `pick` lets go
    /// doesn't scroll, and a touch that lifts first doesn't click: `on_tap`
    /// hears it.
    pub fn start_on_move(
        &self,
        down: &PointerEvent,
        options: StartOnMoveOptions<T>,
        window: &mut Window,
        _cx: &mut Context<T>,
    ) {
        if down.kind == PointerKind::Touch {
            window.prevent_default();
        }
        let waiting = Rc::new(RefCell::new(Waiting {
            down: down.clone(),
            window: window.window_handle(),
            options: options.drag,
            pick: options.pick,
            on_tap: options.on_tap,
            stopped: false,
        }));
        let replaced = self
            .inner
            .borrow_mut()
            .running
            .replace(Running::Waiting(waiting));
        if let Some(replaced) = replaced {
            replaced.stop();
        }
    }

    // Stops waiting, and lets go of the session's running entry if it's
    // this one.
    fn stop_waiting(inner: &Rc<RefCell<SessionInner<T>>>, waiting: &WaitingRef<T>) {
        waiting.borrow_mut().stopped = true;
        let mut state = inner.borrow_mut();
        if state
            .running
            .as_ref()
            .is_some_and(|running| matches!(running, Running::Waiting(running) if Rc::ptr_eq(running, waiting)))
        {
            state.running = None;
        }
    }

    fn hear_waiting(
        inner: &Rc<RefCell<SessionInner<T>>>,
        waiting: &WaitingRef<T>,
        event: Heard,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (kind, button, own, stopped) = {
            let state = waiting.borrow();
            (
                state.down.kind,
                state.down.button,
                state.window == window.window_handle(),
                state.stopped,
            )
        };
        if !own || stopped {
            return;
        }
        let entity = inner.borrow().entity.clone();
        let session = DragSession {
            inner: inner.clone(),
        };
        match event {
            Heard::MouseMove(event) if kind == PointerKind::Mouse => {
                if event.pressed_button.is_none() {
                    Self::stop_waiting(inner, waiting);
                    return;
                }
                Self::waiting_moved(&session, waiting, PointerEvent::from(&event), entity, window, cx)
            }
            Heard::Touch(event) if kind == PointerKind::Touch => match event.phase {
                TouchPhase::Moved => Self::waiting_moved(
                    &session,
                    waiting,
                    PointerEvent::touch(&event),
                    entity,
                    window,
                    cx,
                ),
                TouchPhase::Ended => {
                    Self::stop_waiting(inner, waiting);
                    Self::tapped(waiting, PointerEvent::touch(&event), entity, window, cx);
                }
                TouchPhase::Cancelled => Self::stop_waiting(inner, waiting),
                TouchPhase::Started => {}
            },
            Heard::MouseUp(event) if kind == PointerKind::Mouse && event.button == button => {
                Self::stop_waiting(inner, waiting);
                Self::tapped(waiting, PointerEvent::from(&event), entity, window, cx);
            }
            _ => {}
        }
    }

    fn tapped(
        waiting: &WaitingRef<T>,
        up: PointerEvent,
        entity: WeakEntity<T>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let on_tap = waiting.borrow().on_tap.clone();
        if let Some(on_tap) = on_tap {
            entity.update(cx, |this, cx| on_tap(this, &up, window, cx)).ok();
        }
    }

    fn waiting_moved(
        session: &DragSession<T>,
        waiting: &WaitingRef<T>,
        event: PointerEvent,
        entity: WeakEntity<T>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pick = waiting.borrow().pick.clone();
        let session = session.clone();
        let waiting = waiting.clone();
        entity
            .update(cx, |this, cx| {
                let picked = match &pick {
                    Some(pick) => pick(this, &event, window, cx),
                    None => Some(true),
                };
                let Some(picked) = picked else {
                    return;
                };
                if waiting.borrow().stopped {
                    return;
                }
                Self::stop_waiting(&session.inner, &waiting);
                if !picked {
                    return;
                }
                // The swipe follows from the move that picked it, but counts
                // as a drag from the press.
                let (down, mut options) = {
                    let state = waiting.borrow();
                    (state.down.clone(), state.options.clone())
                };
                let on_end = options.on_end.take();
                let from_move = event.clone();
                options.on_end = Some(Rc::new(
                    move |this: &mut T,
                          up: &PointerEvent,
                          canceled: bool,
                          window: &mut Window,
                          cx: &mut Context<T>| {
                        let from = |event: &PointerEvent| {
                            is_drag(up.position.x - event.position.x, up.position.y - event.position.y)
                        };
                        if !canceled && from(&down) && !from(&from_move) {
                            swallow_next_click(false, window, cx);
                        }
                        if let Some(on_end) = &on_end {
                            on_end(this, up, canceled, window, cx);
                        }
                    },
                ));
                session.start(&event, options, window, cx);
            })
            .ok();
    }

    /// Ends the running drag, if any, as canceled, calling its `on_end` with
    /// the press that started it. A press still waiting just stops.
    pub fn cancel(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let running = self.inner.borrow_mut().running.take();
        match running {
            Some(Running::Drag(drag)) => {
                let (down, on_end, stopped) = {
                    let state = drag.borrow();
                    (state.down.clone(), state.options.on_end.clone(), state.stopped)
                };
                Drag::stop(&drag);
                if !stopped && let Some(on_end) = on_end {
                    on_end(this, &down, true, window, cx);
                }
            }
            Some(Running::Waiting(waiting)) => waiting.borrow_mut().stopped = true,
            None => {}
        }
    }
}

// What swallowing a click waits for, per window.
#[derive(Default)]
struct Swallow {
    armed: bool,
    until_release: bool,
    // Whether a press ends it: not the press it was armed during.
    press_ends: bool,
    // Each time it's armed, so a forget meant for one arming leaves the next be.
    serial: u64,
}

thread_local! {
    static SWALLOW: RefCell<HashMap<WindowId, Swallow>> = RefCell::new(HashMap::new());
}

fn with_swallow<R>(window: WindowId, f: impl FnOnce(&mut Swallow) -> R) -> R {
    SWALLOW.with(|swallow| f(swallow.borrow_mut().entry(window).or_default()))
}

// Lets go of an arming once the event being dispatched is over.
fn forget_after(serial: u64, window: &mut Window, cx: &mut App) {
    window.defer(cx, move |window, _| {
        let id = window.window_handle().window_id();
        with_swallow(id, |swallow| {
            if swallow.serial == serial {
                swallow.armed = false;
            }
        });
    });
}

/// Swallows the click a drag's release sends, so the drag doesn't press
/// what it ends on, like a link (`swallowNextClick`). Call it in the
/// release's capture phase, as a drag's `on_end` runs, a
/// `capture_any_mouse_up` handler or a window listener's
/// `DispatchPhase::Capture`, since gpui fires the click in the same
/// release's bubble phase; or on the press with `until_release`. It stops
/// after the release, or at the next press, so a later click goes through.
/// A tap's press, which gpui makes as the finger lifts, isn't the next press
/// but the touch's own: its release, and the click it makes, end it.
///
/// The release then reaches no element's bubble phase: the listener a
/// drag's element paints last, [`swallow_listener`] or a session's, stops it
/// before any element's click forms, so every `on_press` and `on_click`
/// respects it. gpui reports one pointer at a time, so the original's
/// `pointerId`, which keeps it armed through another finger's presses, has
/// nothing to tell apart.
pub fn swallow_next_click(until_release: bool, window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let serial = with_swallow(id, |swallow| {
        swallow.armed = true;
        swallow.until_release = until_release;
        swallow.press_ends = false;
        swallow.serial += 1;
        swallow.serial
    });
    if !until_release {
        forget_after(serial, window, cx);
        return;
    }
    // The press being dispatched now, if it's armed on one, isn't the next.
    window.defer(cx, move |window, _| {
        let id = window.window_handle().window_id();
        with_swallow(id, |swallow| {
            if swallow.serial == serial {
                swallow.press_ends = true;
            }
        });
    });
}

// The next press ends an arming made until the release.
fn pressed(window: &Window) {
    with_swallow(window.window_handle().window_id(), |swallow| {
        if swallow.press_ends {
            swallow.armed = false;
        }
    });
}

// The pointer let go: an arming made until the release ends once this event
// is over.
fn released(window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let release = with_swallow(id, |swallow| {
        (swallow.armed && swallow.until_release).then_some(swallow.serial)
    });
    if let Some(serial) = release {
        forget_after(serial, window, cx);
    }
}

// Registers this frame's listeners that swallow an armed click. Call it
// while painting, last of all, so its bubble listener runs first.
//
// A finger presses as its touch starts, which gpui offers as a drag. gpui
// hands a tap over as a mouse press and release once the finger lifts, so a
// tap's press belongs to the touch that began before it, and its release
// lets go, as a touch an element claimed does as it ends, and as one that
// turns into a scroll does (`pointercancel`).
fn listen_for_swallow(window: &mut Window) {
    window.on_mouse_event(|event: &MouseDownEvent, phase, window, _cx| {
        if phase == DispatchPhase::Capture
            && !matches!(InputEvent::mouse_down(event, window), InputEvent::TapDown(_))
        {
            pressed(window);
        }
    });
    window.on_mouse_event(|_: &MouseUpEvent, phase, window, cx| match phase {
        DispatchPhase::Capture => released(window, cx),
        DispatchPhase::Bubble => {
            let id = window.window_handle().window_id();
            let armed = with_swallow(id, |swallow| std::mem::replace(&mut swallow.armed, false));
            if armed {
                cx.stop_propagation();
            }
        }
    });
    window.on_mouse_event(|event: &TouchDragEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture {
            return;
        }
        match event.phase {
            TouchPhase::Started => pressed(window),
            TouchPhase::Ended | TouchPhase::Cancelled => released(window, cx),
            TouchPhase::Moved => {}
        }
    });
    window.on_mouse_event(|event: &LongPressEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture
            && matches!(event.phase, TouchPhase::Ended | TouchPhase::Cancelled)
        {
            released(window, cx);
        }
    });
    window.on_mouse_event(|event: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture
            && event.source == ScrollSource::Touch
            && event.touch_phase == TouchPhase::Started
        {
            released(window, cx);
        }
    });
}

/// The element that swallows a click [`swallow_next_click`] asked for, for
/// a component that swallows clicks without a [`DragSession`], like after a
/// swipe it follows itself. Render it on every frame, anywhere in the
/// component. A session's listener already does it.
pub fn swallow_listener() -> impl IntoElement {
    deferred(canvas(|_, _, _| {}, |_, _, window, _| listen_for_swallow(window)).absolute())
        .with_priority(LISTENER_PRIORITY)
}

/// Hears the presses on `el` that can start a drag (`pointerdown`): a mouse
/// button going down on it, and a touch on it that gpui offers as a drag.
/// Inner elements hear a press first, as it bubbles; one that handles it
/// itself stops it with `cx.stop_propagation()`, and a touch one claims by
/// following it is no longer offered to the ones around it. Hearing touches
/// takes a layer over the element, which makes it `relative` unless it's
/// already placed, so an `absolute()` element stays where it is.
pub fn on_pointer_down<E>(
    el: E,
    handler: impl Fn(&PointerEvent, &mut Window, &mut App) + 'static,
) -> E
where
    E: InteractiveElement + ParentElement + Styled,
{
    let handler = Rc::new(handler);
    let mouse = handler.clone();
    ui_sdk_kernel::ensure_positioned(el.on_any_mouse_down(move |event, window, cx| {
        let event = PointerEvent::mouse_down(event, window, cx);
        mouse(&event, window, cx)
    }))
    .child(
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                let handler = handler.clone();
                window.on_mouse_event(move |event: &TouchDragEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble
                        && event.phase == TouchPhase::Started
                        && hitbox.is_hovered(window)
                    {
                        let event = PointerEvent::touch_drag(event, window, cx);
                        handler(&event, window, cx);
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
}

/// How [`repeat_while_held`] repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatOptions {
    /// How long before the first repeat. Defaults to 400ms.
    pub delay: Duration,
    /// How long between repeats. Defaults to 60ms.
    pub interval: Duration,
}

impl Default for RepeatOptions {
    fn default() -> Self {
        Self {
            delay: Duration::from_millis(400),
            interval: Duration::from_millis(60),
        }
    }
}

impl RepeatOptions {
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

/// Runs `step` now, then again every `interval` after `delay`, until the
/// returned task is dropped or `step` returns false, like at a bound
/// (`repeatWhileHeld`). Keep the task while the press is held; dropping it
/// stops the repeats, the original's returned stop.
pub fn repeat_while_held<T: 'static>(
    this: &mut T,
    step: impl Fn(&mut T, &mut Window, &mut Context<T>) -> bool + 'static,
    options: RepeatOptions,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Task<()> {
    if !step(this, window, cx) {
        return Task::ready(());
    }
    cx.spawn_in(window, async move |this, cx| {
        let mut wait = options.delay;
        loop {
            cx.background_executor().timer(wait).await;
            let again = this
                .update_in(cx, |this, window, cx| step(this, window, cx))
                .unwrap_or(false);
            if !again {
                break;
            }
            wait = options.interval;
        }
    })
}
