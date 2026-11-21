//! Port of `foundation/popups/hover.ts`: opening a popup by resting the
//! pointer on its trigger: a delay before it opens, a delay before it closes,
//! and a bridge spanning the trigger and the popup, so the pointer can cross
//! from one to the other.
//!
//! gpui reports a hover's start and end without the pointer event, so a
//! [`HoverEvent`] reads the pointer from the window as it changes. While the
//! pointer crosses the bridge, the popup's content paints
//! [`HoverIntent::listener`], which hears every move in the window, as the
//! original's document listener does.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Bounds, Context, DispatchPhase, IntoElement, MouseMoveEvent, Pixels, Point, Styled, Task,
    WeakEntity, Window, canvas,
};
use ui_sdk_kernel::InputEvent;

use crate::bridge::{Bridge, bridge_between};

/// A pointer entering or leaving, as gpui's hover reports it with the window's
/// pointer.
#[derive(Clone, Debug)]
pub struct HoverEvent {
    /// Where the pointer is.
    pub position: Point<Pixels>,
    /// Whether the last input was a touch. Touch has no hover, so a tap never
    /// counts.
    pub touch: bool,
    /// The pointer as an event, for change details: a move to `position`.
    pub event: InputEvent,
}

impl HoverEvent {
    /// The pointer now, as the window has it.
    pub fn from_window(window: &Window) -> Self {
        let position = window.mouse_position();
        Self {
            position,
            touch: window.last_input_was_touch(),
            event: InputEvent::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: window.modifiers(),
            }),
        }
    }

    /// A move the window heard.
    pub fn from_move(event: &MouseMoveEvent, window: &Window) -> Self {
        Self {
            position: event.position,
            touch: window.last_input_was_touch(),
            event: InputEvent::MouseMove(event.clone()),
        }
    }
}

/// Reads bounds that may change from the component, looked up live.
pub type BoundsFn<T> = Rc<dyn Fn(&T) -> Option<Bounds<Pixels>>>;

/// Closes the popup, after the delay, inside the component's update.
pub type HoverCloseFn<T> = Rc<dyn Fn(&mut T, &InputEvent, &mut Window, &mut Context<T>)>;

/// What [`create_hover_intent`] reads.
pub struct HoverIntentOptions<T: 'static> {
    /// The two elements the pointer may move between, looked up live.
    pub trigger: BoundsFn<T>,
    pub popup: BoundsFn<T>,
    /// Whether leaving should close the popup now: it's open, and the
    /// pointer opened it.
    pub closes_on_leave: Rc<dyn Fn(&T) -> bool>,
    /// How long before closing once the pointer has left both. Read on every
    /// leave.
    pub close_delay: Rc<dyn Fn(&T) -> Duration>,
    /// Closes the popup, after the delay.
    pub close: HoverCloseFn<T>,
}

struct Inner<T: 'static> {
    options: HoverIntentOptions<T>,
    entity: WeakEntity<T>,
    open_timer: RefCell<Option<Task<()>>>,
    close_timer: RefCell<Option<Task<()>>>,
    bridge: RefCell<Option<Bridge>>,
}

/// Hover opening and closing for one trigger and popup (`createHoverIntent`).
/// Clones share it.
pub struct HoverIntent<T: 'static> {
    inner: Rc<Inner<T>>,
}

impl<T: 'static> Clone for HoverIntent<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Hover intent for the component being made.
pub fn create_hover_intent<T: 'static>(options: HoverIntentOptions<T>, cx: &Context<T>) -> HoverIntent<T> {
    HoverIntent {
        inner: Rc::new(Inner {
            options,
            entity: cx.weak_entity(),
            open_timer: RefCell::new(None),
            close_timer: RefCell::new(None),
            bridge: RefCell::new(None),
        }),
    }
}

impl<T: 'static> HoverIntent<T> {
    fn stop_watching(&self) {
        *self.inner.bridge.borrow_mut() = None;
    }

    /// Forgets any pending open or close, like after a press.
    pub fn clear(&self) {
        *self.inner.open_timer.borrow_mut() = None;
        *self.inner.close_timer.borrow_mut() = None;
        self.stop_watching();
    }

    fn schedule_close(&self, this: &T, event: &InputEvent, window: &mut Window, cx: &mut Context<T>) {
        self.stop_watching();
        let delay = (self.inner.options.close_delay)(this);
        let close = self.inner.options.close.clone();
        let event = event.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            this.update_in(cx, |this, window, cx| close(this, &event, window, cx)).ok();
        });
        *self.inner.close_timer.borrow_mut() = Some(task);
    }

    /// The pointer entered the trigger or the popup. Entering either keeps
    /// an open popup open. From the trigger, `open` runs after `delay`, at
    /// once for zero. Touch has no hover, so a tap never counts.
    pub fn enter(
        &self,
        this: &mut T,
        event: &HoverEvent,
        open: Option<Box<dyn FnOnce(&mut T, &mut Window, &mut Context<T>)>>,
        delay: Duration,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if event.touch {
            return;
        }
        *self.inner.close_timer.borrow_mut() = None;
        self.stop_watching();
        let Some(open) = open else {
            return;
        };
        *self.inner.open_timer.borrow_mut() = None;
        if delay.is_zero() {
            open(this, window, cx);
            return;
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| open(this, window, cx)).ok();
        });
        *self.inner.open_timer.borrow_mut() = Some(task);
    }

    /// The pointer left the trigger or the popup. A pending open is dropped.
    /// On its way between the two, the popup stays open until the pointer
    /// leaves the area that spans both.
    pub fn leave(&self, this: &T, event: &HoverEvent, window: &mut Window, cx: &mut Context<T>) {
        if event.touch {
            return;
        }
        *self.inner.open_timer.borrow_mut() = None;
        if !(self.inner.options.closes_on_leave)(this) {
            return;
        }
        let trigger = (self.inner.options.trigger)(this);
        let popup = (self.inner.options.popup)(this);
        let (Some(trigger), Some(popup)) = (trigger, popup) else {
            return self.schedule_close(this, &event.event, window, cx);
        };
        let bridge = bridge_between(trigger, popup);
        if !bridge.contains(event.position) {
            return self.schedule_close(this, &event.event, window, cx);
        }
        *self.inner.bridge.borrow_mut() = Some(bridge);
        // The listener hears moves from the next frame; it draws one.
        window.refresh();
    }

    /// Whether the pointer is crossing the bridge, so moves are heard.
    pub fn is_watching(&self) -> bool {
        self.inner.bridge.borrow().is_some()
    }

    /// The pointer moved while it crosses the bridge: off it, the popup closes
    /// after the delay.
    pub fn pointer_move(&self, this: &T, event: &HoverEvent, window: &mut Window, cx: &mut Context<T>) {
        let outside = self
            .inner
            .bridge
            .borrow()
            .as_ref()
            .is_some_and(|bridge| !bridge.contains(event.position));
        if outside {
            self.schedule_close(this, &event.event, window, cx);
        }
    }

    /// Hears the window's pointer moves while the pointer crosses the bridge,
    /// wherever it goes. The popup's content paints it every frame.
    pub fn listener(&self) -> impl IntoElement + use<T> {
        let intent = self.clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                if !intent.is_watching() {
                    return;
                }
                let intent = intent.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !intent.is_watching() {
                        return;
                    }
                    let hover = HoverEvent::from_move(event, window);
                    let move_intent = intent.clone();
                    intent
                        .inner
                        .entity
                        .update(cx, |this, cx| move_intent.pointer_move(this, &hover, window, cx))
                        .ok();
                });
            },
        )
        .absolute()
    }
}
