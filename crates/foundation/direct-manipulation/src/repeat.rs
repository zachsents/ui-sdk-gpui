//! Port of `foundation/direct-manipulation/repeat.ts`: a press that steps
//! once, then keeps stepping while held, like a stepper's arrows or a
//! remote's volume button, built on [`Hold`].
//!
//! [`bind_repeat_presses`] is the presses alone, for a part whose own press
//! handles the clicks no pointer is behind, like a number input's buttons.
//! [`bind_repeat`] adds those clicks: the part's `on_press` hands its event
//! to [`Repeat::click`], where the original listens for `click` itself.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gpui::{
    App, Bounds, ClickEvent, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    KeyUpEvent, Modifiers, ParentElement, Pixels, Point, Styled, WeakEntity, Window, px,
};
use ui_sdk_drag_and_drop::{PointerEvent, ReadFn, on_pointer_down};
use ui_sdk_kernel::modality::focus_event;
use ui_sdk_kernel::{InputEvent, Key, is_composing};

use crate::hold::{
    BindHoldOptions, Hold, HoldDetails, HoldEndDetails, HoldEvent, HoldInput, HoldPressFn,
    modifiers_of,
};

/// What pressed (`HoldInput | "click"`): a mouse or trackpad, a finger, a
/// key, or a click with no press behind it, like a screen reader's, Enter
/// or Space when `keys` is empty, or gpui's keyboard click.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RepeatInput {
    Mouse,
    Touch,
    Keyboard,
    Click,
}

impl RepeatInput {
    /// The original's name for it, like `"click"`.
    pub fn as_str(self) -> &'static str {
        match self {
            RepeatInput::Mouse => "mouse",
            RepeatInput::Touch => "touch",
            RepeatInput::Keyboard => "keyboard",
            RepeatInput::Click => "click",
        }
    }
}

impl From<HoldInput> for RepeatInput {
    fn from(input: HoldInput) -> Self {
        match input {
            HoldInput::Mouse => RepeatInput::Mouse,
            HoldInput::Touch => RepeatInput::Touch,
            HoldInput::Keyboard => RepeatInput::Keyboard,
        }
    }
}

/// What `on_step` receives (`RepeatDetails`).
#[derive(Clone, Debug)]
pub struct RepeatDetails {
    /// The event behind the step: the press's pointer press or key, the
    /// release of a tap, or a click.
    pub event: InputEvent,
    /// What pressed.
    pub input: RepeatInput,
    /// Which step of the press this is, starting at 1.
    pub count: usize,
}

impl RepeatDetails {
    /// The modifier keys `event` carries, like Shift for a larger step.
    pub fn modifiers(&self) -> Modifiers {
        modifiers_of(&self.event)
    }
}

/// What `on_end` receives (`RepeatEndDetails`).
#[derive(Clone, Debug)]
pub struct RepeatEndDetails {
    /// The event that ended the press.
    pub event: InputEvent,
    /// What pressed.
    pub input: RepeatInput,
    /// How many times the press stepped.
    pub count: usize,
}

/// What `on_step` runs. False stops repeating, like at a bound.
pub type RepeatStepFn<T> = Rc<dyn Fn(&mut T, &RepeatDetails, &mut Window, &mut Context<T>) -> bool>;

/// What `on_end` runs.
pub type RepeatEndFn<T> = Rc<dyn Fn(&mut T, &RepeatEndDetails, &mut Window, &mut Context<T>)>;

/// How [`bind_repeat`] and [`bind_repeat_presses`] treat a press
/// (`BindRepeatOptions`). `T` is the component the callbacks run in.
pub struct BindRepeatOptions<T: 'static> {
    /// How long a press lasts before it starts repeating. Defaults to 400ms.
    pub delay: Duration,
    /// How often it steps again while held. Defaults to 60ms; at zero it
    /// steps once more, after `delay`, and stops.
    pub repeat: Duration,
    /// The keys that press it while it has focus. An empty list leaves keys
    /// to gpui, which clicks a focused element once per press of Enter or
    /// Space. Defaults to Space and Enter.
    pub keys: Vec<Key>,
    /// Whether a press can't start, and a held one stops repeating.
    pub disabled: Option<ReadFn<T, bool>>,
    /// Runs as a press starts. Return false to ignore it.
    pub on_press: Option<HoldPressFn<T>>,
    /// Runs once for each press, then every `repeat` while it's held.
    /// Return false to stop repeating, like at a bound.
    pub on_step: Option<RepeatStepFn<T>>,
    /// Runs once when a press ends, released or canceled, and after a
    /// click's step.
    pub on_end: Option<RepeatEndFn<T>>,
}

impl<T: 'static> Default for BindRepeatOptions<T> {
    fn default() -> Self {
        Self {
            delay: Duration::from_millis(400),
            repeat: Duration::from_millis(60),
            keys: vec![Key::Space, Key::Enter],
            disabled: None,
            on_press: None,
            on_step: None,
            on_end: None,
        }
    }
}

impl<T: 'static> Clone for BindRepeatOptions<T> {
    fn clone(&self) -> Self {
        Self {
            delay: self.delay,
            repeat: self.repeat,
            keys: self.keys.clone(),
            disabled: self.disabled.clone(),
            on_press: self.on_press.clone(),
            on_step: self.on_step.clone(),
            on_end: self.on_end.clone(),
        }
    }
}

impl<T: 'static> BindRepeatOptions<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn repeat(mut self, repeat: Duration) -> Self {
        self.repeat = repeat;
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

    pub fn on_step(
        mut self,
        on_step: impl Fn(&mut T, &RepeatDetails, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        self.on_step = Some(Rc::new(on_step));
        self
    }

    pub fn on_end(
        mut self,
        on_end: impl Fn(&mut T, &RepeatEndDetails, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        self.on_end = Some(Rc::new(on_end));
        self
    }
}

// What runs when a press is refused, for the clicks a key press that didn't
// start leaves to gpui.
type RefuseFn = Rc<dyn Fn(&HoldDetails, &mut Window, &mut App)>;

struct RepeatState<T: 'static> {
    options: RefCell<BindRepeatOptions<T>>,
    count: Cell<usize>,
    stopped: Cell<bool>,
    // Whether the pointer is on the element, which repeats wait for.
    inside: Cell<bool>,
    // The element's bounds while a pointer press follows the pointer.
    following: Cell<Option<Bounds<Pixels>>>,
    // Whether `stop` ran during the press's first step.
    aborted: Cell<bool>,
    on_refuse: Option<RefuseFn>,
}

impl<T: 'static> RepeatState<T> {
    fn step(
        &self,
        this: &mut T,
        event: InputEvent,
        input: RepeatInput,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        if self.stopped.get() {
            return false;
        }
        self.count.set(self.count.get() + 1);
        let on_step = self.options.borrow().on_step.clone();
        let details = RepeatDetails {
            event,
            input,
            count: self.count.get(),
        };
        let go = on_step.is_none_or(|on_step| on_step(this, &details, window, cx));
        self.stopped.set(!go);
        go
    }

    fn end(
        &self,
        this: &mut T,
        event: InputEvent,
        input: RepeatInput,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let on_end = self.options.borrow().on_end.clone();
        if let Some(on_end) = on_end {
            let details = RepeatEndDetails {
                event,
                input,
                count: self.count.get(),
            };
            on_end(this, &details, window, cx);
        }
    }

    fn refuse(&self, press: &HoldDetails, window: &mut Window, cx: &mut App) {
        if let Some(on_refuse) = &self.on_refuse {
            on_refuse(press, window, cx);
        }
    }

    fn follow(&self, bounds: Option<Bounds<Pixels>>) {
        self.following.set(bounds);
    }

    fn moved(&self, at: Point<Pixels>) {
        if let Some(bounds) = self.following.get() {
            self.inside.set(
                at.x >= bounds.left()
                    && at.x <= bounds.right()
                    && at.y >= bounds.top()
                    && at.y <= bounds.bottom(),
            );
        }
    }

    fn reset(&self) {
        self.following.set(None);
        self.inside.set(true);
    }
}

/// A press that steps, then keeps stepping while held, without the click
/// listener (`bindRepeatPresses`'s binding). Cloning it gives another handle
/// to the same presses.
pub struct RepeatPresses<T: 'static> {
    hold: Hold<T>,
    state: Rc<RepeatState<T>>,
}

impl<T: 'static> Clone for RepeatPresses<T> {
    fn clone(&self) -> Self {
        Self {
            hold: self.hold.clone(),
            state: self.state.clone(),
        }
    }
}

/// Presses that step once, then keep stepping while held, for a part that
/// takes clicks through its own press, where the component's handler runs
/// first and can refuse the step (`bindRepeatPresses`). A mouse or a key
/// steps as it goes down; a finger steps when it lifts, or once held, so a
/// scroll that starts on the element never steps. Repeats wait while the
/// pointer is off the element and go on when it comes back. `focus` is the
/// focus handle the element tracks. Make it in the component's constructor,
/// keep it in a field, and wire the element with
/// [`bind`](RepeatPresses::bind):
///
/// ```ignore
/// self.increment = bind_repeat_presses(None, BindRepeatOptions::new()
///     .keys([])
///     .on_press(|this: &mut Self, press, window, cx| {
///         let drags = this.drags.clone();
///         press.event.pointer().is_none_or(|event| drags.accepts(this, event, window, cx))
///     })
///     .on_step(|this, step, window, cx| this.step_by(1.0, step.modifiers().shift, &step.event, window, cx))
///     .on_end(|this, end, window, cx| this.commit(&end.event, window, cx)),
///     cx);
/// ```
pub fn bind_repeat_presses<T: 'static>(
    focus: Option<&FocusHandle>,
    options: BindRepeatOptions<T>,
    cx: &Context<T>,
) -> RepeatPresses<T> {
    RepeatPresses::new(cx.weak_entity(), focus, options)
}

impl<T: 'static> RepeatPresses<T> {
    /// Presses for the component `entity`, for ones made outside its
    /// constructor.
    pub fn new(
        entity: WeakEntity<T>,
        focus: Option<&FocusHandle>,
        options: BindRepeatOptions<T>,
    ) -> Self {
        Self::with_refuse(entity, focus, options, None)
    }

    fn with_refuse(
        entity: WeakEntity<T>,
        focus: Option<&FocusHandle>,
        options: BindRepeatOptions<T>,
        on_refuse: Option<RefuseFn>,
    ) -> Self {
        let state = Rc::new(RepeatState {
            options: RefCell::new(options),
            count: Cell::new(0),
            stopped: Cell::new(false),
            inside: Cell::new(true),
            following: Cell::new(None),
            aborted: Cell::new(false),
            on_refuse,
        });
        let hold = Hold::new(entity, focus, BindHoldOptions::default());
        let moved = Rc::downgrade(&state);
        hold.set_on_move(move |at| {
            if let Some(state) = moved.upgrade() {
                state.moved(at);
            }
        });
        let presses = Self { hold, state };
        presses.sync();
        presses
    }

    // Hands the hold the options it presses with.
    fn sync(&self) {
        let options = self.state.options.borrow().clone();
        let state = Rc::downgrade(&self.state);
        let hold = self.hold.downgrade();
        let pressed = state.clone();
        let stepped = state.clone();
        let ended = state;
        self.hold.set_options(BindHoldOptions {
            delay: options.delay,
            repeat: Some(options.repeat),
            keys: options.keys,
            tolerance: px(f32::INFINITY),
            disabled: options.disabled,
            on_press: Some(Rc::new(
                move |this: &mut T,
                      press: &HoldDetails,
                      window: &mut Window,
                      cx: &mut Context<T>| {
                    let (Some(state), Some(hold)) = (pressed.upgrade(), hold.upgrade()) else {
                        return false;
                    };
                    Self::pressed(&state, &hold, this, press, window, cx)
                },
            )),
            on_hold: Some(Rc::new(
                move |this: &mut T,
                      details: &HoldDetails,
                      _count,
                      window: &mut Window,
                      cx: &mut Context<T>| {
                    let Some(state) = stepped.upgrade() else {
                        return false;
                    };
                    !state.inside.get()
                        || state.step(
                            this,
                            details.event.input_event(),
                            details.input.into(),
                            window,
                            cx,
                        )
                },
            )),
            on_end: Some(Rc::new(
                move |this: &mut T,
                      end: &HoldEndDetails,
                      window: &mut Window,
                      cx: &mut Context<T>| {
                    let Some(state) = ended.upgrade() else {
                        return;
                    };
                    let disabled = state.options.borrow().disabled.clone();
                    let tapped = end.input == HoldInput::Touch
                        && !end.held
                        && !end.canceled
                        && state.inside.get()
                        && !disabled.is_some_and(|disabled| disabled(this));
                    state.reset();
                    if tapped {
                        state.step(this, end.event.clone(), RepeatInput::Touch, window, cx);
                    }
                    state.end(this, end.event.clone(), end.input.into(), window, cx);
                },
            )),
        });
    }

    fn pressed(
        state: &RepeatState<T>,
        hold: &Hold<T>,
        this: &mut T,
        press: &HoldDetails,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let on_press = state.options.borrow().on_press.clone();
        if let Some(on_press) = on_press
            && !on_press(this, press, window, cx)
        {
            state.refuse(press, window, cx);
            return false;
        }
        state.count.set(0);
        state.stopped.set(false);
        state.aborted.set(false);
        if press.input == HoldInput::Touch {
            state.follow(hold.bounds());
            return true;
        }
        // The hold starts only once this returns, so a first step that stops
        // it or moves focus ends the press here instead of leaving it repeating.
        let focus = hold.focus();
        let focused = focus.as_ref().is_some_and(|focus| focus.is_focused(window));
        state.step(
            this,
            press.event.input_event(),
            press.input.into(),
            window,
            cx,
        );
        if state.aborted.get() {
            return false;
        }
        if focused && !focus.as_ref().is_some_and(|focus| focus.is_focused(window)) {
            state.refuse(press, window, cx);
            state.end(this, focus_event(window), press.input.into(), window, cx);
            return false;
        }
        if press.input == HoldInput::Mouse {
            state.follow(hold.bounds());
        }
        true
    }

    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: BindRepeatOptions<T>) {
        *self.state.options.borrow_mut() = options;
        self.sync();
    }

    /// The focus handle the element tracks.
    pub fn set_focus(&self, focus: Option<&FocusHandle>) {
        self.hold.set_focus(focus);
    }

    /// Whether a press is under way, the original's `data-holding`.
    pub fn holding(&self) -> bool {
        self.hold.holding()
    }

    /// Whether the press under way repeats, `data-held`.
    pub fn held(&self) -> bool {
        self.hold.held()
    }

    /// How long a press lasts before it starts repeating, `--hold-delay`.
    pub fn delay(&self) -> Duration {
        self.hold.delay()
    }

    /// Stops a press under way without calling `on_end`, as the original's
    /// cleanup does. Called from a press's first step, it ends that press.
    pub fn stop(&self) {
        self.state.aborted.set(true);
        self.hold.stop();
        self.state.reset();
    }

    /// Starts a press from a pointer's press on the element.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.hold.pointer_down(this, event, window, cx);
    }

    /// Starts a press from a key on the element while it has focus.
    pub fn key_down(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.hold.key_down(this, event, window, cx);
    }

    /// Ends a key's press. Call it in the release's capture phase.
    pub fn key_up(
        &self,
        this: &mut T,
        event: &KeyUpEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.hold.key_up(this, event, window, cx);
    }

    /// The element that follows a press wherever the pointer goes. Render it
    /// on every frame; [`bind`](Self::bind) renders it inside the element.
    pub fn listener(&self) -> impl IntoElement + use<T> {
        self.hold.listener()
    }

    /// Wires the element: its presses, its keys, its bounds and the
    /// [`listener`](Self::listener).
    pub fn bind<E: InteractiveElement + ParentElement + Styled>(&self, el: E) -> E {
        self.hold.bind(el)
    }
}

// What keeps a press's click from stepping twice (`bindRepeat`'s closure).
#[derive(Default)]
struct Clicks {
    // A key press just ended, and gpui clicks after a short one, which
    // stepped as the key went down.
    after_key: Cell<bool>,
    // A key whose press was refused, which leaves gpui its click as it
    // comes up.
    refused: RefCell<Option<Key>>,
}

/// A press that steps, then keeps stepping while held, and a click with no
/// press behind it (`bindRepeat`'s binding). Cloning it gives another
/// handle to the same repeat.
pub struct Repeat<T: 'static> {
    presses: RepeatPresses<T>,
    clicks: Rc<Clicks>,
}

impl<T: 'static> Clone for Repeat<T> {
    fn clone(&self) -> Self {
        Self {
            presses: self.presses.clone(),
            clicks: self.clicks.clone(),
        }
    }
}

/// Makes an element step once when pressed, then keep stepping while held,
/// like a stepper's arrows or a remote's volume button (`bindRepeat`). A
/// mouse or a key steps as it goes down. A finger steps on release, or once
/// held, so a scroll that starts on the element never steps. Repeats wait
/// while the pointer is off the element and go on when it comes back. A
/// click with no press behind it steps once, and the click that follows a
/// press doesn't step again. Wire the element with [`bind`](Repeat::bind)
/// and hand its part's `on_press` event to [`click`](Repeat::click):
///
/// ```ignore
/// self.zoom_in = bind_repeat(Some(&focus), BindRepeatOptions::new()
///     .on_step(|this: &mut Self, _step, _window, cx| this.zoom_by(5.0, cx)), cx);
/// // render:
/// let repeat = this.zoom_in.clone();
/// repeat.bind(Part::new(..).on_press(move |event, window, cx| {
///     entity.update(cx, |this, cx| repeat.click(this, &event, window, cx)).ok();
/// }))
/// ```
pub fn bind_repeat<T: 'static>(
    focus: Option<&FocusHandle>,
    options: BindRepeatOptions<T>,
    cx: &Context<T>,
) -> Repeat<T> {
    Repeat::new(cx.weak_entity(), focus, options)
}

impl<T: 'static> Repeat<T> {
    /// A repeat for the component `entity`, for one made outside its
    /// constructor.
    pub fn new(
        entity: WeakEntity<T>,
        focus: Option<&FocusHandle>,
        options: BindRepeatOptions<T>,
    ) -> Self {
        let clicks = Rc::new(Clicks::default());
        let refused = Rc::downgrade(&clicks);
        // A key press that didn't start leaves gpui its click, as the key
        // comes up.
        let on_refuse: RefuseFn = Rc::new(move |press: &HoldDetails, _window, _cx| {
            if press.input != HoldInput::Keyboard {
                return;
            }
            if let (Some(clicks), Some(event)) = (refused.upgrade(), key_of(press)) {
                *clicks.refused.borrow_mut() = Some(Key::of(&event.keystroke));
            }
        });
        let options = Self::wrap(options, Rc::downgrade(&clicks));
        Self {
            presses: RepeatPresses::with_refuse(entity, focus, options, Some(on_refuse)),
            clicks,
        }
    }

    // The options, with an `on_end` that notes a key press ending.
    fn wrap(mut options: BindRepeatOptions<T>, clicks: Weak<Clicks>) -> BindRepeatOptions<T> {
        let on_end = options.on_end.take();
        options.on_end = Some(Rc::new(
            move |this: &mut T,
                  details: &RepeatEndDetails,
                  window: &mut Window,
                  cx: &mut Context<T>| {
                // gpui clicks after a short key press, which stepped as the key went down.
                if details.input == RepeatInput::Keyboard
                    && let Some(clicks) = clicks.upgrade()
                {
                    clicks.after_key.set(true);
                    let later = Rc::downgrade(&clicks);
                    window.defer(cx, move |_window, _cx| {
                        if let Some(clicks) = later.upgrade() {
                            clicks.after_key.set(false);
                        }
                    });
                }
                if let Some(on_end) = &on_end {
                    on_end(this, details, window, cx);
                }
            },
        ));
        options
    }

    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: BindRepeatOptions<T>) {
        self.presses
            .set_options(Self::wrap(options, Rc::downgrade(&self.clicks)));
    }

    /// The focus handle the element tracks.
    pub fn set_focus(&self, focus: Option<&FocusHandle>) {
        self.presses.set_focus(focus);
    }

    /// Whether a press is under way, the original's `data-holding`.
    pub fn holding(&self) -> bool {
        self.presses.holding()
    }

    /// Whether the press under way repeats, `data-held`.
    pub fn held(&self) -> bool {
        self.presses.held()
    }

    /// How long a press lasts before it starts repeating, `--hold-delay`.
    pub fn delay(&self) -> Duration {
        self.presses.delay()
    }

    /// Stops a press under way without calling `on_end`, as the original's
    /// cleanup does.
    pub fn stop(&self) {
        self.presses.stop();
        self.clicks.refused.borrow_mut().take();
    }

    /// Starts a press from a pointer's press on the element.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.presses.pointer_down(this, event, window, cx);
    }

    /// Starts a press from a key on the element while it has focus.
    pub fn key_down(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        // A refused key that came up where the element didn't hear it.
        if !event.is_held {
            self.clicks.refused.borrow_mut().take();
        }
        self.presses.key_down(this, event, window, cx);
    }

    /// Ends a key's press. Call it in the release's capture phase.
    pub fn key_up(
        &self,
        this: &mut T,
        event: &KeyUpEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let key = Key::of(&event.keystroke);
        if self.clicks.refused.borrow().as_ref() == Some(&key) {
            // The click gpui makes as this release bubbles is still the refused press's.
            let clicks = Rc::downgrade(&self.clicks);
            window.defer(cx, move |_window, _cx| {
                if let Some(clicks) = clicks.upgrade() {
                    clicks.refused.borrow_mut().take();
                }
            });
        }
        self.presses.key_up(this, event, window, cx);
    }

    /// Steps once for a click with no press behind it: a keyboard click, as
    /// gpui makes for Enter and Space when `keys` is empty, or a screen
    /// reader's. Hand it the event of the part's `on_press`; a pointer's
    /// click, which its press already stepped for, does nothing.
    pub fn click(
        &self,
        this: &mut T,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let pressless = matches!(
            event,
            InputEvent::Click(ClickEvent::Keyboard(_)) | InputEvent::Accessibility
        );
        // A refused key's click comes only as it comes up on the focused
        // element; once focus has gone, nothing is left to keep.
        let focused = self
            .presses
            .hold
            .focus()
            .is_some_and(|focus| focus.is_focused(window));
        if !focused {
            self.clicks.refused.borrow_mut().take();
        }
        let state = &self.presses.state;
        let disabled = state.options.borrow().disabled.clone();
        if !pressless
            || window.default_prevented()
            || self.clicks.after_key.get()
            || self.clicks.refused.borrow().is_some()
            || disabled.is_some_and(|disabled| disabled(this))
        {
            return;
        }
        let on_step = state.options.borrow().on_step.clone();
        if let Some(on_step) = on_step {
            let details = RepeatDetails {
                event: event.clone(),
                input: RepeatInput::Click,
                count: 1,
            };
            on_step(this, &details, window, cx);
        }
        let on_end = state.options.borrow().on_end.clone();
        if let Some(on_end) = on_end {
            let details = RepeatEndDetails {
                event: event.clone(),
                input: RepeatInput::Click,
                count: 1,
            };
            on_end(this, &details, window, cx);
        }
    }

    /// The element that follows a press wherever the pointer goes. Render it
    /// on every frame; [`bind`](Self::bind) renders it inside the element.
    pub fn listener(&self) -> impl IntoElement + use<T> {
        self.presses.listener()
    }

    /// Wires the element: its presses, its keys, its bounds and the
    /// [`listener`](Self::listener). Its clicks come through
    /// [`click`](Self::click).
    pub fn bind<E: InteractiveElement + ParentElement + Styled>(&self, el: E) -> E {
        let hold = &self.presses.hold;
        let el = hold.measure(el);
        let repeat = self.clone();
        let el = on_pointer_down(el, move |event, window, cx| {
            let presses = repeat.presses.clone();
            presses
                .hold
                .in_update(cx, |this, cx| presses.pointer_down(this, event, window, cx));
        });
        let repeat = self.clone();
        let el = el.on_key_down(move |event, window, cx| {
            if is_composing(window, cx) {
                return;
            }
            let inner = repeat.clone();
            repeat
                .presses
                .hold
                .in_update(cx, |this, cx| inner.key_down(this, event, window, cx));
        });
        let repeat = self.clone();
        let el = el.capture_key_up(move |event, window, cx| {
            let inner = repeat.clone();
            repeat
                .presses
                .hold
                .in_update(cx, |this, cx| inner.key_up(this, event, window, cx));
        });
        el.child(self.listener())
    }
}

fn key_of(press: &HoldDetails) -> Option<&KeyDownEvent> {
    match &press.event {
        HoldEvent::Key(event) => Some(event),
        HoldEvent::Pointer(_) => None,
    }
}
