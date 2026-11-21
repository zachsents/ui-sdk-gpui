//! Port of `foundation/direct-manipulation/hold.ts`: press and hold: a long
//! press that opens a context menu on a touch screen, a stepper's button
//! that repeats while held, a button held to confirm, a key held to talk. A
//! short press clicks as usual; a press that's held doesn't.
//!
//! The original listens on the element, and on its document while a press
//! lasts. Here [`Hold::bind`] wires the element, and the hold follows a
//! press through window listeners its [`listener`](Hold::listener) registers
//! as it paints, as a drag session does. Its timers are a task it keeps,
//! which dropping cancels.
//!
//! gpui decides what a touch is: a tap, which it turns into a mouse press
//! and release as the finger lifts, a pan, which scrolls, or a drag or long
//! press an element claims. A hold claims nothing, so a short tap still
//! clicks and a finger that starts to scroll cancels the press, as the
//! browser's `pointercancel` does. It hears the touch start as gpui offers
//! it as a drag, its release as the tap's, and its moves and release
//! directly when an element claims it. A press handler that calls
//! `window.prevent_default()` on a touch claims it, so no tap or scroll
//! follows.
//!
//! The click a short key press makes is gpui's own: it clicks a focused
//! element as Enter or Space comes up, which the hold lets through after a
//! short press and stops, in the release's capture phase, after a held or
//! canceled one.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, Bounds, Context, DispatchPhase, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, KeyUpEvent, KeystrokeEvent, LongPressEvent, Modifiers, MouseButton,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, ScrollEvent, ScrollSource, Styled,
    Subscription, Task, TouchDragEvent, TouchPhase, WeakEntity, Window, canvas, deferred, div, px,
};
use ui_sdk_drag_and_drop::{
    PointerEvent, PointerKind, ReadFn, on_pointer_down, swallow_listener, swallow_next_click,
};
use ui_sdk_kernel::focus_events::on_blur;
use ui_sdk_kernel::modality::focus_event;
use ui_sdk_kernel::{InputEvent, Key, Measured, is_composing};
use ui_sdk_motion::timestamp;


/// What pressed (`HoldInput`): a mouse or trackpad, a finger, or a key.
/// gpui reports a pen as a mouse or a finger, so there's no `Pen`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HoldInput {
    Mouse,
    Touch,
    Keyboard,
}

impl HoldInput {
    /// The original's name for it, like `"touch"`.
    pub fn as_str(self) -> &'static str {
        match self {
            HoldInput::Mouse => "mouse",
            HoldInput::Touch => "touch",
            HoldInput::Keyboard => "keyboard",
        }
    }

    fn of(kind: PointerKind) -> Self {
        match kind {
            PointerKind::Mouse => HoldInput::Mouse,
            PointerKind::Touch => HoldInput::Touch,
        }
    }
}

/// The event that started a press: a pointer's press or a key's.
#[derive(Clone, Debug)]
pub enum HoldEvent {
    Pointer(PointerEvent),
    Key(KeyDownEvent),
}

impl HoldEvent {
    /// The gpui event behind it, for a change's details.
    pub fn input_event(&self) -> InputEvent {
        match self {
            HoldEvent::Pointer(event) => event.event.clone(),
            HoldEvent::Key(event) => InputEvent::from(event),
        }
    }

    /// The pointer's press, like for a drag session's `accepts`.
    pub fn pointer(&self) -> Option<&PointerEvent> {
        match self {
            HoldEvent::Pointer(event) => Some(event),
            HoldEvent::Key(_) => None,
        }
    }

    /// Where the pointer pressed, in window coordinates (`clientX`, `clientY`).
    pub fn position(&self) -> Option<Point<Pixels>> {
        self.pointer().map(|event| event.position)
    }

    /// The modifier keys held as it pressed, like Shift for a larger step.
    pub fn modifiers(&self) -> Modifiers {
        match self {
            HoldEvent::Pointer(event) => event.modifiers,
            HoldEvent::Key(event) => event.keystroke.modifiers,
        }
    }
}

/// What [`bind_hold`]'s callbacks receive (`HoldDetails`).
#[derive(Clone, Debug)]
pub struct HoldDetails {
    /// The event that started the press: its pointer's press or its key.
    pub event: HoldEvent,
    /// What pressed.
    pub input: HoldInput,
}

/// What [`bind_hold`] reports when a press ends (`HoldEndDetails`).
#[derive(Clone, Debug)]
pub struct HoldEndDetails {
    /// The event that ended the press.
    pub event: InputEvent,
    /// What pressed.
    pub input: HoldInput,
    /// Whether the press lasted past `delay`.
    pub held: bool,
    /// Whether it ended without a release: the pointer moved past
    /// `tolerance`, a finger started to scroll, Escape was pressed, or focus
    /// left the element or the window. A canceled press doesn't click.
    pub canceled: bool,
}

impl HoldEndDetails {
    /// The modifier keys held as it ended.
    pub fn modifiers(&self) -> Modifiers {
        modifiers_of(&self.event)
    }
}

// The modifier keys an event carries; none for a touch or a focus change.
pub(crate) fn modifiers_of(event: &InputEvent) -> Modifiers {
    match event {
        InputEvent::Click(click) => click.modifiers(),
        InputEvent::KeyDown(event) => event.keystroke.modifiers,
        InputEvent::KeyUp(event) => event.keystroke.modifiers,
        InputEvent::MouseDown(event) => event.modifiers,
        InputEvent::MouseMove(event) => event.modifiers,
        InputEvent::MouseUp(event) => event.modifiers,
        InputEvent::Scroll(event) => event.modifiers,
        _ => Modifiers::default(),
    }
}

/// What `on_press` runs as a press starts. False ignores it.
pub type HoldPressFn<T> = Rc<dyn Fn(&mut T, &HoldDetails, &mut Window, &mut Context<T>) -> bool>;

/// What `on_hold` runs once the press is held, then every `repeat`, with
/// which time this is, from 1. False stops repeating.
pub type HoldFn<T> = Rc<dyn Fn(&mut T, &HoldDetails, usize, &mut Window, &mut Context<T>) -> bool>;

/// What `on_end` runs once a press ends.
pub type HoldEndFn<T> = Rc<dyn Fn(&mut T, &HoldEndDetails, &mut Window, &mut Context<T>)>;

/// How [`bind_hold`] treats a press (`BindHoldOptions`). `T` is the
/// component the callbacks run in; they run inside its update.
pub struct BindHoldOptions<T: 'static> {
    /// How long a press lasts before it's held. Defaults to 500ms.
    pub delay: Duration,
    /// Runs `on_hold` again this often while held, like a stepper's button.
    /// Without it, or at zero, `on_hold` runs once.
    pub repeat: Option<Duration>,
    /// How far a pointer can move before it's held, before the press is
    /// canceled, like when a finger starts to scroll. Defaults to 10.
    pub tolerance: Pixels,
    /// The keys that press it while it has focus. A short press clicks it,
    /// as a pointer's does. An empty list leaves keys alone. Defaults to
    /// Space and Enter.
    pub keys: Vec<Key>,
    /// Whether a press can't start, and a held one stops repeating.
    pub disabled: Option<ReadFn<T, bool>>,
    /// Runs as a press starts. Return false to ignore it, like a mouse
    /// press where only touch holds.
    pub on_press: Option<HoldPressFn<T>>,
    /// Runs once the press is held, then every `repeat`. Return false to
    /// stop repeating, like at a bound; the press goes on until it ends.
    pub on_hold: Option<HoldFn<T>>,
    /// Runs once when the press ends, released or canceled.
    pub on_end: Option<HoldEndFn<T>>,
}

impl<T: 'static> Default for BindHoldOptions<T> {
    fn default() -> Self {
        Self {
            delay: Duration::from_millis(500),
            repeat: None,
            tolerance: px(10.),
            keys: vec![Key::Space, Key::Enter],
            disabled: None,
            on_press: None,
            on_hold: None,
            on_end: None,
        }
    }
}

impl<T: 'static> Clone for BindHoldOptions<T> {
    fn clone(&self) -> Self {
        Self {
            delay: self.delay,
            repeat: self.repeat,
            tolerance: self.tolerance,
            keys: self.keys.clone(),
            disabled: self.disabled.clone(),
            on_press: self.on_press.clone(),
            on_hold: self.on_hold.clone(),
            on_end: self.on_end.clone(),
        }
    }
}

impl<T: 'static> BindHoldOptions<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn repeat(mut self, repeat: Duration) -> Self {
        self.repeat = Some(repeat);
        self
    }

    pub fn tolerance(mut self, tolerance: Pixels) -> Self {
        self.tolerance = tolerance;
        self
    }

    pub fn keys(mut self, keys: impl IntoIterator<Item = Key>) -> Self {
        self.keys = keys.into_iter().collect();
        self
    }

    pub fn disabled(mut self, disabled: impl Fn(&T) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(disabled));
        self
    }

    pub fn on_press(
        mut self,
        on_press: impl Fn(&mut T, &HoldDetails, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        self.on_press = Some(Rc::new(on_press));
        self
    }

    pub fn on_hold(
        mut self,
        on_hold: impl Fn(&mut T, &HoldDetails, usize, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        self.on_hold = Some(Rc::new(on_hold));
        self
    }

    pub fn on_end(
        mut self,
        on_end: impl Fn(&mut T, &HoldEndDetails, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        self.on_end = Some(Rc::new(on_end));
        self
    }
}

// How a press ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    // The pointer or the key came up (`pointerup`, `keyup`).
    Release,
    // A mouse moved with no button down: its release went unheard.
    Missed,
    // Canceled while still down: it moved past `tolerance`, or Escape, or
    // focus left.
    Cancel,
    // The system took the pointer, like a finger that started to scroll
    // (`pointercancel`), so no click comes.
    Lost,
}

// A press under way (`Press`).
struct Press {
    serial: u64,
    details: HoldDetails,
    // The pointer that pressed and where (`pointerId`).
    pointer: Option<(PointerKind, MouseButton, Point<Pixels>)>,
    key: Option<Key>,
    window: AnyWindowHandle,
    // When it pressed, on the input clock, in ms.
    started: f64,
    held: bool,
    count: usize,
    timer: Task<()>,
    _cancels: Vec<Subscription>,
}

// What the hold's window listeners hear and hand on.
enum Heard {
    MouseMove(MouseMoveEvent),
    MouseUp(MouseUpEvent),
    TouchDrag(TouchDragEvent),
    LongPress(LongPressEvent),
    Scroll(ScrollEvent),
}

// What a moving pointer tells a press, by kind.
type MoveHook = Rc<dyn Fn(Point<Pixels>)>;

struct HoldInner<T: 'static> {
    entity: WeakEntity<T>,
    focus: RefCell<Option<FocusHandle>>,
    options: RefCell<BindHoldOptions<T>>,
    press: RefCell<Option<Press>>,
    // Each press, so a timer or a listener can tell its press still runs.
    serial: Cell<u64>,
    bounds: Measured,
    // The key of a press canceled while it was down, whose repeats and
    // release reach nothing until it comes up (`swallowKey`).
    swallowed: RefCell<Option<Key>>,
    on_move: RefCell<Option<MoveHook>>,
}

/// A press that's held (`bindHold`'s binding): its press under way, which
/// `holding` and `held` report, and how it follows the pointer. Cloning it
/// gives another handle to the same hold; dropping the last one stops a
/// press under way without `on_end`.
pub struct Hold<T: 'static> {
    inner: Rc<HoldInner<T>>,
}

impl<T: 'static> Clone for Hold<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// A hold as its own callbacks keep it, so they don't keep it alive.
pub(crate) struct WeakHold<T: 'static>(Weak<HoldInner<T>>);

impl<T: 'static> Clone for WeakHold<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: 'static> WeakHold<T> {
    pub(crate) fn upgrade(&self) -> Option<Hold<T>> {
        self.0.upgrade().map(|inner| Hold { inner })
    }
}

/// Makes an element respond to a press that's held (`bindHold`): after
/// `delay`, `on_hold` runs, and runs again every `repeat` when set. Works
/// with a mouse, touch, and Space or Enter while the element has focus.
/// Only a short press that's released clicks, so a held button keeps its
/// click; a held or canceled one doesn't. `focus` is the focus handle the
/// element tracks, which keys need and whose blur cancels a press.
///
/// While pressed, [`holding`](Hold::holding) is true, and
/// [`held`](Hold::held) once held, the original's `data-holding` and
/// `data-held`, and [`delay`](Hold::delay) is the original's `--hold-delay`,
/// so a part can fill a hold-to-confirm button over it. Make it in the
/// component's constructor, keep it in a field, and wire the element with
/// [`bind`](Hold::bind):
///
/// ```ignore
/// self.hold = bind_hold(Some(&focus), BindHoldOptions::new()
///     .delay(Duration::from_millis(1000))
///     .on_hold(|this: &mut Self, _details, _count, window, cx| { this.delete_project(window, cx); true }),
///     cx);
/// // render: self.hold.bind(Part::new(..).on_press(..))
/// ```
pub fn bind_hold<T: 'static>(
    focus: Option<&FocusHandle>,
    options: BindHoldOptions<T>,
    cx: &Context<T>,
) -> Hold<T> {
    Hold::new(cx.weak_entity(), focus, options)
}

impl<T: 'static> Hold<T> {
    /// A hold for the component `entity`, for one made outside its
    /// constructor, like in a part method that has the entity.
    pub fn new(
        entity: WeakEntity<T>,
        focus: Option<&FocusHandle>,
        options: BindHoldOptions<T>,
    ) -> Self {
        Self {
            inner: Rc::new(HoldInner {
                entity,
                focus: RefCell::new(focus.cloned()),
                options: RefCell::new(options),
                press: RefCell::new(None),
                serial: Cell::new(0),
                bounds: Measured::default(),
                swallowed: RefCell::new(None),
                on_move: RefCell::new(None),
            }),
        }
    }

    fn upgrade(inner: &Weak<HoldInner<T>>) -> Option<Self> {
        inner.upgrade().map(|inner| Self { inner })
    }

    pub(crate) fn downgrade(&self) -> WeakHold<T> {
        WeakHold(Rc::downgrade(&self.inner))
    }

    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: BindHoldOptions<T>) {
        *self.inner.options.borrow_mut() = options;
    }

    /// The focus handle the element tracks.
    pub fn set_focus(&self, focus: Option<&FocusHandle>) {
        *self.inner.focus.borrow_mut() = focus.cloned();
    }

    pub(crate) fn focus(&self) -> Option<FocusHandle> {
        self.inner.focus.borrow().clone()
    }

    // The element's bounds as of its last layout.
    pub(crate) fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.inner.bounds.get()
    }

    pub(crate) fn set_on_move(&self, on_move: impl Fn(Point<Pixels>) + 'static) {
        *self.inner.on_move.borrow_mut() = Some(Rc::new(on_move));
    }

    /// Whether a press is under way, the original's `data-holding`.
    pub fn holding(&self) -> bool {
        self.inner.press.borrow().is_some()
    }

    /// Whether the press under way lasted past `delay`, `data-held`.
    pub fn held(&self) -> bool {
        self.inner
            .press
            .borrow()
            .as_ref()
            .is_some_and(|press| press.held)
    }

    /// How long a press lasts before it's held, the original's
    /// `--hold-delay`.
    pub fn delay(&self) -> Duration {
        self.inner.options.borrow().delay
    }

    /// Stops a press under way without calling `on_end`, as the original's
    /// cleanup does. Presses after it start as usual.
    pub fn stop(&self) {
        let press = self.inner.press.borrow_mut().take();
        drop(press);
    }

    fn is_disabled(&self, this: &T) -> bool {
        let disabled = self.inner.options.borrow().disabled.clone();
        disabled.is_some_and(|disabled| disabled(this))
    }

    // Whether the press `serial` still runs.
    fn runs(&self, serial: u64) -> bool {
        self.inner
            .press
            .borrow()
            .as_ref()
            .is_some_and(|press| press.serial == serial)
    }

    fn start(
        &self,
        this: &mut T,
        details: HoldDetails,
        pointer: Option<(PointerKind, MouseButton, Point<Pixels>)>,
        key: Option<Key>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        if self.holding() || self.is_disabled(this) {
            return false;
        }
        let on_press = self.inner.options.borrow().on_press.clone();
        if let Some(on_press) = on_press
            && !on_press(this, &details, window, cx)
        {
            return false;
        }
        if self.holding() {
            return false;
        }
        let serial = self.inner.serial.get() + 1;
        self.inner.serial.set(serial);
        let delay = self.inner.options.borrow().delay;
        let timer = self.schedule(serial, delay, window, cx);
        let cancels = self.cancels(serial, window, cx);
        *self.inner.press.borrow_mut() = Some(Press {
            serial,
            details,
            pointer,
            key,
            window: window.window_handle(),
            started: timestamp(),
            held: false,
            count: 0,
            timer,
            _cancels: cancels,
        });
        cx.notify();
        true
    }

    // Runs `tick` after `delay`, then as often as it asks.
    fn schedule(
        &self,
        serial: u64,
        delay: Duration,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Task<()> {
        let weak = Rc::downgrade(&self.inner);
        cx.spawn_in(window, async move |this, cx| {
            let mut wait = delay;
            loop {
                cx.background_executor().timer(wait).await;
                let next = this.update_in(cx, |this, window, cx| {
                    Self::upgrade(&weak).and_then(|hold| hold.tick(this, serial, window, cx))
                });
                match next {
                    Ok(Some(next)) => wait = next,
                    _ => break,
                }
            }
        })
    }

    // The press is held: runs `on_hold`, and returns when to run it again.
    fn tick(
        &self,
        this: &mut T,
        serial: u64,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Option<Duration> {
        if !self.runs(serial) || self.is_disabled(this) {
            return None;
        }
        let (details, count) = {
            let mut press = self.inner.press.borrow_mut();
            let current = press.as_mut()?;
            current.held = true;
            current.count += 1;
            (current.details.clone(), current.count)
        };
        cx.notify();
        let on_hold = self.inner.options.borrow().on_hold.clone();
        let repeats = on_hold.is_none_or(|on_hold| on_hold(this, &details, count, window, cx));
        // No repeat, or none of 0ms, runs it once.
        let repeat = self
            .inner
            .options
            .borrow()
            .repeat
            .filter(|repeat| !repeat.is_zero());
        // `on_hold` may have ended the press, like by moving focus.
        if repeats && self.runs(serial) {
            repeat
        } else {
            None
        }
    }

    // Whether the press `serial` is held, or will be within a frame or two.
    fn due(&self, serial: u64) -> bool {
        let delay = self.inner.options.borrow().delay.as_secs_f64() * 1000.0;
        self.inner
            .press
            .borrow()
            .as_ref()
            .filter(|press| press.serial == serial)
            .is_some_and(|press| press.held || timestamp() - press.started + 50.0 >= delay)
    }

    // Holds the press now rather than when its timer runs, and times its
    // repeats from here.
    fn hold_now(&self, this: &mut T, serial: u64, window: &mut Window, cx: &mut Context<T>) {
        if self.held() {
            return;
        }
        let next = self.tick(this, serial, window, cx);
        let timer = match next {
            Some(wait) => self.schedule(serial, wait, window, cx),
            None => Task::ready(()),
        };
        if let Some(press) = self
            .inner
            .press
            .borrow_mut()
            .as_mut()
            .filter(|press| press.serial == serial)
        {
            press.timer = timer;
        }
    }

    // Escape cancels a press without reaching a popup or dialog around it,
    // and so does focus leaving the element or the window.
    fn cancels(&self, serial: u64, window: &mut Window, cx: &mut Context<T>) -> Vec<Subscription> {
        let mut cancels = Vec::new();
        let weak = Rc::downgrade(&self.inner);
        let handle = window.window_handle();
        cancels.push(
            cx.intercept_keystrokes(move |event: &KeystrokeEvent, window, cx| {
                if Key::of(&event.keystroke) != Key::Escape
                    || window.window_handle() != handle
                    || is_composing(window, cx)
                {
                    return;
                }
                let Some(hold) = Self::upgrade(&weak) else {
                    return;
                };
                if !hold.runs(serial) {
                    return;
                }
                window.prevent_default();
                cx.stop_propagation();
                let escape = KeyDownEvent {
                    keystroke: event.keystroke.clone(),
                    is_held: false,
                    prefer_character_input: false,
                };
                hold.end_in(InputEvent::from(&escape), Ending::Cancel, window, cx);
            }),
        );
        if let Some(focus) = self.focus() {
            let weak = Rc::downgrade(&self.inner);
            cancels.push(on_blur(&focus, window, cx, move |this, window, cx| {
                if let Some(hold) = Self::upgrade(&weak)
                    && hold.runs(serial)
                {
                    hold.end(this, focus_event(window), Ending::Cancel, window, cx);
                }
            }));
        }
        let weak = Rc::downgrade(&self.inner);
        cancels.push(
            cx.observe_window_activation(window, move |this, window, cx| {
                if window.is_window_active() {
                    return;
                }
                if let Some(hold) = Self::upgrade(&weak)
                    && hold.runs(serial)
                {
                    hold.end(this, focus_event(window), Ending::Cancel, window, cx);
                }
            }),
        );
        cancels
    }

    // Ends the press from outside the component's update.
    fn end_in(&self, event: InputEvent, ending: Ending, window: &mut Window, cx: &mut App) {
        let Some(entity) = self.inner.entity.upgrade() else {
            self.stop();
            return;
        };
        entity.update(cx, |this, cx| self.end(this, event, ending, window, cx));
    }

    fn end(
        &self,
        this: &mut T,
        event: InputEvent,
        ending: Ending,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let Some(press) = self.inner.press.borrow_mut().take() else {
            return;
        };
        let (input, pointer, key, held) = (
            press.details.input,
            press.pointer,
            press.key.clone(),
            press.held,
        );
        drop(press);
        let canceled = matches!(ending, Ending::Cancel | Ending::Lost);
        // A release still to come, after Escape or focus leaving, waits for
        // it, a finger's tap included.
        if pointer.is_some() && (held || canceled) && ending != Ending::Lost {
            swallow_next_click(ending != Ending::Release, window, cx);
        }
        if let Some(key) = key
            && canceled
            && self.focus().is_some_and(|focus| focus.is_focused(window))
        {
            *self.inner.swallowed.borrow_mut() = Some(key);
        }
        cx.notify();
        let on_end = self.inner.options.borrow().on_end.clone();
        if let Some(on_end) = on_end {
            on_end(
                this,
                &HoldEndDetails {
                    event,
                    input,
                    held,
                    canceled,
                },
                window,
                cx,
            );
        }
    }

    /// Starts a press from a pointer's press on the element. [`bind`](Self::bind)
    /// calls it; call it yourself from
    /// [`on_pointer_down`] when you wire the
    /// element by hand. Only the main button presses.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        match event.kind {
            // A touch another element claimed as it started isn't this press's.
            PointerKind::Touch if window.default_prevented() => return,
            // A tap's press, which gpui makes as the finger lifts, belongs to its touch.
            PointerKind::Mouse if window.last_input_was_touch() => return,
            _ => {}
        }
        let details = HoldDetails {
            event: HoldEvent::Pointer(event.clone()),
            input: HoldInput::of(event.kind),
        };
        self.start(
            this,
            details,
            Some((event.kind, event.button, event.position)),
            None,
            window,
            cx,
        );
    }

    /// Starts a press from a key on the element while it has focus.
    /// [`bind`](Self::bind) calls it from the element's `on_key_down`.
    pub fn key_down(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if !self.focus().is_some_and(|focus| focus.is_focused(window)) {
            return;
        }
        let key = Key::of(&event.keystroke);
        {
            let mut swallowed = self.inner.swallowed.borrow_mut();
            if swallowed.as_ref() == Some(&key) {
                if event.is_held {
                    window.prevent_default();
                    return;
                }
                // It came up where the element didn't hear it.
                *swallowed = None;
            }
        }
        if !self.inner.options.borrow().keys.contains(&key) {
            return;
        }
        // The keyboard's own repeats would click a button again and again.
        if self
            .inner
            .press
            .borrow()
            .as_ref()
            .is_some_and(|press| press.details.input == HoldInput::Keyboard)
        {
            window.prevent_default();
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if event.is_held
            || window.default_prevented()
            || modifiers.alt
            || modifiers.control
            || modifiers.platform
        {
            return;
        }
        let details = HoldDetails {
            event: HoldEvent::Key(event.clone()),
            input: HoldInput::Keyboard,
        };
        if self.start(this, details, None, Some(key), window, cx) {
            window.prevent_default();
        }
    }

    /// Ends a key's press as it comes up. Call it in the release's capture
    /// phase, the element's `capture_key_up`, as [`bind`](Self::bind) does,
    /// so the click gpui makes as the key comes up can be stopped.
    pub fn key_up(
        &self,
        this: &mut T,
        event: &KeyUpEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let key = Key::of(&event.keystroke);
        let swallowed = self
            .inner
            .swallowed
            .borrow_mut()
            .take_if(|swallowed| *swallowed == key);
        if swallowed.is_some() {
            window.prevent_default();
            return;
        }
        let (ours, held) = match self.inner.press.borrow().as_ref() {
            Some(press) if press.details.input == HoldInput::Keyboard => {
                (press.key.as_ref() == Some(&key), press.held)
            }
            _ => return,
        };
        if !ours {
            // Another of the keys came up mid-press; it pressed nothing.
            if self.inner.options.borrow().keys.contains(&key) {
                window.prevent_default();
            }
            return;
        }
        self.end(this, InputEvent::from(event), Ending::Release, window, cx);
        if held {
            window.prevent_default();
        }
    }

    fn moved(&self, at: Point<Pixels>) {
        let on_move = self.inner.on_move.borrow().clone();
        if let Some(on_move) = on_move {
            on_move(at);
        }
    }

    // Whether the pointer moved past `tolerance` from where it pressed.
    fn strayed(&self, from: Point<Pixels>, at: Point<Pixels>) -> bool {
        let tolerance = f32::from(self.inner.options.borrow().tolerance);
        f32::from(at.x - from.x).hypot(f32::from(at.y - from.y)) > tolerance
    }

    fn hear(inner: &Weak<HoldInner<T>>, heard: Heard, window: &mut Window, cx: &mut App) {
        let Some(hold) = Self::upgrade(inner) else {
            return;
        };
        let (press, serial) = hold
            .inner
            .press
            .borrow()
            .as_ref()
            .filter(|press| press.window == window.window_handle())
            .map_or((None, 0), |press| {
                (Some((press.pointer, press.held)), press.serial)
            });
        let touching = window.last_input_was_touch();
        let touch = match press {
            Some((Some((PointerKind::Touch, _, from)), held)) => Some((from, held)),
            _ => None,
        };
        match heard {
            Heard::MouseMove(event) => {
                let Some((Some((PointerKind::Mouse, _, from)), held)) = press else {
                    return;
                };
                hold.moved(event.position);
                // A mouse released where the window missed the release.
                if event.pressed_button.is_none() {
                    hold.end_in(InputEvent::from(&event), Ending::Missed, window, cx);
                } else if !held && hold.strayed(from, event.position) {
                    hold.end_in(InputEvent::from(&event), Ending::Cancel, window, cx);
                }
            }
            Heard::MouseUp(event) => match press {
                Some((Some((PointerKind::Mouse, button, _)), _))
                    if !touching && event.button == button =>
                {
                    hold.end_in(InputEvent::from(&event), Ending::Release, window, cx);
                }
                // A tap: gpui's release for the finger that pressed.
                Some((Some((PointerKind::Touch, _, _)), _)) if touching => {
                    hold.end_in(InputEvent::from(&event), Ending::Release, window, cx);
                }
                _ => {}
            },
            Heard::TouchDrag(event) => {
                let input = InputEvent::from(&event);
                hold.touched(touch, event.phase, event.position, input, true, window, cx);
            }
            Heard::LongPress(event) if event.phase == TouchPhase::Started => {
                if touch.is_none() {
                    return;
                }
                hold.in_update(cx, |this, cx| {
                    if hold.due(serial) && !hold.is_disabled(this) {
                        // A held press claims the long press gpui offers, so
                        // the finger lifting taps nothing, like a menu the
                        // hold opened.
                        window.prevent_default();
                        hold.hold_now(this, serial, window, cx);
                    } else {
                        // A long press opens a tooltip, which a press keeps
                        // closed as the original keeps the long-press menu
                        // closed; the touch can still tap.
                        cx.stop_propagation();
                    }
                });
            }
            Heard::LongPress(event) => {
                // A long press is the same touch; gpui reports its moves and
                // release once an element claims it.
                let input = InputEvent::TouchDrag(TouchDragEvent {
                    phase: event.phase,
                    start_position: event.start_position,
                    position: event.position,
                });
                hold.touched(touch, event.phase, event.position, input, false, window, cx);
            }
            Heard::Scroll(event) => {
                if event.source == ScrollSource::Touch && event.touch_phase == TouchPhase::Started {
                    // The finger started to scroll: the browser's `pointercancel`.
                    if touch.is_some() {
                        hold.end_in(InputEvent::from(&event), Ending::Lost, window, cx);
                    }
                }
            }
        }
    }

    // A step of the touch: a drag or long press an element claimed, or a new touch.
    #[allow(clippy::too_many_arguments)]
    fn touched(
        &self,
        touch: Option<(Point<Pixels>, bool)>,
        phase: TouchPhase,
        at: Point<Pixels>,
        event: InputEvent,
        starts: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        match phase {
            TouchPhase::Started => {
                if !starts {
                    return;
                }
                // A new touch means the last one ended where nothing said so.
                if touch.is_some() {
                    self.end_in(event, Ending::Lost, window, cx);
                }
            }
            TouchPhase::Moved => {
                let Some((from, held)) = touch else {
                    return;
                };
                self.moved(at);
                if !held && self.strayed(from, at) {
                    self.end_in(event, Ending::Cancel, window, cx);
                }
            }
            TouchPhase::Ended => {
                if touch.is_some() {
                    self.end_in(event, Ending::Release, window, cx);
                }
            }
            TouchPhase::Cancelled => {
                if touch.is_some() {
                    self.end_in(event, Ending::Lost, window, cx);
                }
            }
        }
    }

    /// The element that follows a press wherever the pointer goes, and
    /// swallows the click a held press's release would make. Render it on
    /// every frame, anywhere in the component; [`bind`](Self::bind) renders
    /// it inside the element.
    pub fn listener(&self) -> impl IntoElement + use<T> {
        let weak = Rc::downgrade(&self.inner);
        let listen = deferred(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let hold = weak.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            Self::hear(&hold, Heard::MouseMove(event.clone()), window, cx);
                        }
                    });
                    let hold = weak.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            Self::hear(&hold, Heard::MouseUp(event.clone()), window, cx);
                        }
                    });
                    let hold = weak.clone();
                    window.on_mouse_event(move |event: &TouchDragEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            Self::hear(&hold, Heard::TouchDrag(event.clone()), window, cx);
                        }
                    });
                    let hold = weak.clone();
                    window.on_mouse_event(move |event: &LongPressEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            Self::hear(&hold, Heard::LongPress(event.clone()), window, cx);
                        }
                    });
                    let hold = weak.clone();
                    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            Self::hear(&hold, Heard::Scroll(event.clone()), window, cx);
                        }
                    });
                },
            )
            .absolute(),
        )
        .with_priority(usize::MAX);
        div().absolute().child(listen).child(swallow_listener())
    }

    /// Adds the element's bounds as of each layout, which a repeat reads.
    pub(crate) fn measure<E: ParentElement + Styled>(&self, el: E) -> E {
        self.inner.bounds.measure(el)
    }

    // Runs `f` inside the component's update.
    pub(crate) fn in_update(&self, cx: &mut App, f: impl FnOnce(&mut T, &mut Context<T>)) {
        if let Some(entity) = self.inner.entity.upgrade() {
            entity.update(cx, f);
        }
    }

    /// Wires the element: its pointer presses
    /// ([`on_pointer_down`]), its
    /// keys (`on_key_down`, and `capture_key_up` so a held press's key never
    /// clicks), its bounds, and the [`listener`](Self::listener) inside it.
    /// The element is usually a part with an `on_press`, whose click a short
    /// press keeps.
    pub fn bind<E: InteractiveElement + ParentElement + Styled>(&self, el: E) -> E {
        let el = self.measure(el);
        let hold = self.clone();
        let el = on_pointer_down(el, move |event, window, cx| {
            hold.in_update(cx, |this, cx| hold.pointer_down(this, event, window, cx));
        });
        let hold = self.clone();
        let el = el.on_key_down(move |event, window, cx| {
            if is_composing(window, cx) {
                return;
            }
            hold.in_update(cx, |this, cx| hold.key_down(this, event, window, cx));
        });
        let hold = self.clone();
        let el = el.capture_key_up(move |event, window, cx| {
            hold.in_update(cx, |this, cx| hold.key_up(this, event, window, cx));
        });
        el.child(self.listener())
    }
}
