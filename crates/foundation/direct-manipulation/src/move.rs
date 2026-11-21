//! Port of `foundation/direct-manipulation/move.ts`: moving items in two
//! dimensions: cards on a board, nodes on a canvas, windows on a desktop. A
//! pointer drags one item, or every selected item with it, through a draft
//! offset the move keeps; the drop commits every position in one change, and
//! Escape puts them back. The keyboard nudges with Alt and the arrows, and
//! screen readers hear where each item went.
//!
//! The original writes the draft to the elements as a `translate`, so a drag
//! renders nothing; here [`Move::follow`] draws the dragged items moved as
//! far as the pointer went, their layout where it was, as a translate does.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;
use std::rc::{Rc, Weak};

use gpui::{
    Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Point,
    SharedString, Styled, WeakEntity, Window,
};
use ui_sdk_drag_and_drop::{
    BindDragOptions, DRAG_THRESHOLD, DragSession, KeyFn, MessageFn, PointerEvent, ReadFn,
    on_pointer_down,
};
use ui_sdk_kernel::{ChangeOptions, Key, Reason};
use ui_sdk_platform::{AnnouncePriority, announce_with};

use crate::js;
use crate::translate::Translate;

/// Where an item is, in the units your positions use, usually pixels
/// (`MovePoint`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MovePoint {
    pub x: f64,
    pub y: f64,
}

impl MovePoint {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// The items a pointer is dragging and how far (`MoveDraft`).
#[derive(Clone, Debug, PartialEq)]
pub struct MoveDraft<K> {
    pub keys: Vec<K>,
    pub dx: f64,
    pub dy: f64,
}

/// Why positions changed: a pointer drag (`Drag`), a keyboard nudge
/// (`MoveKey`), or a call from code (`Imperative`).
pub type MoveReason = Reason;

/// What happened, for the message screen readers hear (`MoveMessage`).
#[derive(Clone, Debug, PartialEq)]
pub struct MoveMessage<K> {
    /// The keys that moved.
    pub keys: Vec<K>,
    /// What the first is called, from `label`, else its key.
    pub label: SharedString,
    /// Where the first is now.
    pub at: MovePoint,
}

/// The English messages [`create_move`] announces unless you pass `message`
/// (`moveMessage`).
pub fn move_message<K>(message: &MoveMessage<K>) -> String {
    let what = if message.keys.len() > 1 {
        format!("{} items", message.keys.len())
    } else {
        message.label.to_string()
    };
    format!(
        "{what} moved to {}, {}.",
        js::number(message.at.x),
        js::number(message.at.y)
    )
}

/// A box positions stay inside, like the board's size minus a card's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveBounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl MoveBounds {
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }
}

// How far each arrow nudges.
fn arrow(key: &Key) -> Option<(f64, f64)> {
    match key {
        Key::ArrowUp => Some((0.0, -1.0)),
        Key::ArrowDown => Some((0.0, 1.0)),
        Key::ArrowLeft => Some((-1.0, 0.0)),
        Key::ArrowRight => Some((1.0, 0.0)),
        _ => None,
    }
}

/// Asks the component for new positions, with why, and returns whether
/// they went through. It runs inside the component's update, with the
/// component.
pub type SetPositionsFn<T, K> =
    Rc<dyn Fn(&mut T, HashMap<K, MovePoint>, ChangeOptions, &mut Window, &mut Context<T>) -> bool>;

/// How [`create_move`] reads and changes positions (`MoveOptions`). `T` is
/// the component, `K` an item's key.
pub struct MoveOptions<T: 'static, K> {
    /// Every item's position, the component's value (`key`).
    pub value: ReadFn<T, HashMap<K, MovePoint>>,
    /// Asks for new positions and returns whether they went through. Pass
    /// the component's change: `|this, value, details, window, cx|
    /// this.positions.change(&this.instance, value, details, window, cx)`.
    pub set: SetPositionsFn<T, K>,
    /// The keys that move together with the one pressed, like the selection
    /// when it holds it. Defaults to that one key.
    pub group: Option<KeyFn<T, K, Vec<K>>>,
    /// The grid a dropped position lands on, and how far Alt with an arrow
    /// nudges. Defaults to 1.
    pub step: Option<ReadFn<T, f64>>,
    /// How far Alt with Shift and an arrow nudges. Defaults to ten steps.
    pub large_step: Option<ReadFn<T, f64>>,
    /// How many screen pixels one unit of position takes, like a canvas's
    /// zoom, so a drag follows the pointer when the items are scaled.
    /// Defaults to 1.
    pub scale: Option<ReadFn<T, f64>>,
    /// Keeps positions inside a box, like the board's size minus a card's.
    pub bounds: Option<KeyFn<T, K, Option<MoveBounds>>>,
    /// Whether an item can't be moved.
    pub disabled: Option<KeyFn<T, K, bool>>,
    /// The name screen readers hear for an item. Return `None` for its key.
    pub label: Option<KeyFn<T, K, Option<SharedString>>>,
    /// The text screen readers hear after a move. Defaults to English
    /// ([`move_message`]).
    pub message: Option<MessageFn<MoveMessage<K>>>,
}

impl<T: 'static, K> Clone for MoveOptions<T, K> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            set: self.set.clone(),
            group: self.group.clone(),
            step: self.step.clone(),
            large_step: self.large_step.clone(),
            scale: self.scale.clone(),
            bounds: self.bounds.clone(),
            disabled: self.disabled.clone(),
            label: self.label.clone(),
            message: self.message.clone(),
        }
    }
}

impl<T: 'static, K: 'static> MoveOptions<T, K> {
    /// Options with the positions and how to change them.
    pub fn new(
        value: impl Fn(&T) -> HashMap<K, MovePoint> + 'static,
        set: impl Fn(&mut T, HashMap<K, MovePoint>, ChangeOptions, &mut Window, &mut Context<T>) -> bool
        + 'static,
    ) -> Self {
        Self {
            value: Rc::new(value),
            set: Rc::new(set),
            group: None,
            step: None,
            large_step: None,
            scale: None,
            bounds: None,
            disabled: None,
            label: None,
            message: None,
        }
    }

    pub fn group(mut self, group: impl Fn(&T, &K) -> Vec<K> + 'static) -> Self {
        self.group = Some(Rc::new(group));
        self
    }

    pub fn step(mut self, step: impl Fn(&T) -> f64 + 'static) -> Self {
        self.step = Some(Rc::new(step));
        self
    }

    pub fn large_step(mut self, large_step: impl Fn(&T) -> f64 + 'static) -> Self {
        self.large_step = Some(Rc::new(large_step));
        self
    }

    pub fn scale(mut self, scale: impl Fn(&T) -> f64 + 'static) -> Self {
        self.scale = Some(Rc::new(scale));
        self
    }

    pub fn bounds(mut self, bounds: impl Fn(&T, &K) -> Option<MoveBounds> + 'static) -> Self {
        self.bounds = Some(Rc::new(bounds));
        self
    }

    pub fn disabled(mut self, disabled: impl Fn(&T, &K) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(disabled));
        self
    }

    pub fn label(mut self, label: impl Fn(&T, &K) -> Option<SharedString> + 'static) -> Self {
        self.label = Some(Rc::new(label));
        self
    }

    pub fn message(mut self, message: impl Fn(&MoveMessage<K>) -> String + 'static) -> Self {
        self.message = Some(Rc::new(message));
        self
    }
}

/// The dragged item's offset (`style`): the original's `--move-x` and
/// `--move-y`, in units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveStyle {
    pub x: f64,
    pub y: f64,
}

// A drag's draft, and how far the pointer went on screen, which the dragged
// items are drawn moved by.
struct Dragged<K> {
    draft: MoveDraft<K>,
    screen: Point<Pixels>,
}

struct MoveInner<T: 'static, K> {
    options: RefCell<MoveOptions<T, K>>,
    entity: WeakEntity<T>,
    drags: DragSession<T>,
    draft: RefCell<Option<Dragged<K>>>,
}

/// Lets users move a component's items in two dimensions (`createMove`'s
/// return). Cloning it gives another handle to the same move.
pub struct Move<T: 'static, K> {
    inner: Rc<MoveInner<T, K>>,
}

impl<T: 'static, K> Clone for Move<T, K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users move a component's items in two dimensions, by dragging them
/// and with Alt and the arrows (`createMove`). Make it in the component's
/// constructor, keep it in a field, render its
/// [`listener`](Move::listener) on every frame, and wire the item part:
///
/// ```ignore
/// let at = this.positions.get()[&key];
/// let card = Part::new(&instance, "card", Some(key.as_ref()), ()).absolute().left(px(at.x)).top(px(at.y))
///     .on_keys(&["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"], move |event, window, cx| {
///         entity.update(cx, |this, cx| {
///             if !this.moving.clone().keydown(this, event, &key, window, cx) {
///                 this.roving.keydown(event, &key, window, cx);
///             }
///         }).ok();
///     });
/// this.moving.follow(&key, this.moving.bind_item(key.clone(), card))
/// ```
///
/// While a pointer drags, [`follow`](Move::follow) draws the dragged items
/// where the pointer has taken them and [`dragging`](Move::dragging) says
/// which are moving. The drop commits once, with every moved key, through
/// `set`.
pub fn create_move<T: 'static, K: Clone + Eq + Hash + Display + 'static>(
    options: MoveOptions<T, K>,
    cx: &Context<T>,
) -> Move<T, K> {
    Move {
        inner: Rc::new(MoveInner {
            options: RefCell::new(options),
            entity: cx.weak_entity(),
            drags: DragSession::new(cx),
            draft: RefCell::new(None),
        }),
    }
}

impl<T: 'static, K: Clone + Eq + Hash + Display + 'static> Move<T, K> {
    fn upgrade(inner: &Weak<MoveInner<T, K>>) -> Option<Self> {
        inner.upgrade().map(|inner| Self { inner })
    }

    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: MoveOptions<T, K>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn options(&self) -> MoveOptions<T, K> {
        self.inner.options.borrow().clone()
    }

    fn positions(&self, this: &T) -> HashMap<K, MovePoint> {
        let value = self.inner.options.borrow().value.clone();
        value(this)
    }

    fn is_disabled(&self, this: &T, key: &K) -> bool {
        let disabled = self.inner.options.borrow().disabled.clone();
        disabled.is_some_and(|disabled| disabled(this, key))
    }

    fn step(&self, this: &T) -> f64 {
        let step = self.inner.options.borrow().step.clone();
        step.map_or(1.0, |step| step(this))
    }

    // The keys that move with `key`, but the disabled ones.
    fn group(&self, this: &T, key: &K) -> Vec<K> {
        let group = self.inner.options.borrow().group.clone();
        group
            .map_or_else(|| vec![key.clone()], |group| group(this, key))
            .into_iter()
            .filter(|key| !self.is_disabled(this, key))
            .collect()
    }

    // A position on the grid of `round`, inside the item's bounds.
    fn place(&self, this: &T, key: &K, x: f64, y: f64, round: f64) -> MovePoint {
        let mut point = MovePoint {
            x: js::round(x / round) * round,
            y: js::round(y / round) * round,
        };
        let bounds = self.inner.options.borrow().bounds.clone();
        if let Some(bounds) = bounds.and_then(|bounds| bounds(this, key)) {
            point = MovePoint {
                x: js::min(bounds.max_x, js::max(bounds.min_x, point.x)),
                y: js::min(bounds.max_y, js::max(bounds.min_y, point.y)),
            };
        }
        point
    }

    /// Moves items by an offset, in one change, within their bounds, and
    /// tells screen readers where the first went. A drag lands on the step's
    /// grid. Returns whether it went through.
    #[allow(clippy::too_many_arguments)]
    pub fn move_by(
        &self,
        this: &mut T,
        keys: &[K],
        dx: f64,
        dy: f64,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let current = self.positions(this);
        let mut next = current.clone();
        let round = if options.reason == Some(Reason::Drag) {
            self.step(this)
        } else {
            1.0
        };
        for key in keys {
            let at = current.get(key).copied().unwrap_or_default();
            next.insert(
                key.clone(),
                self.place(this, key, at.x + dx, at.y + dy, round),
            );
        }
        let unchanged = keys.iter().all(|key| {
            current
                .get(key)
                .is_some_and(|at| next.get(key).is_some_and(|to| to.x == at.x && to.y == at.y))
        });
        if unchanged {
            return false;
        }
        let set = self.inner.options.borrow().set.clone();
        let at = keys.first().and_then(|first| next.get(first).copied());
        let done = set(this, next, options, window, cx);
        if done && let (Some(first), Some(at)) = (keys.first(), at) {
            let moving = self.options();
            let label = moving
                .label
                .as_ref()
                .and_then(|label| label(this, first))
                .unwrap_or_else(|| first.to_string().into());
            let message = MoveMessage {
                keys: keys.to_vec(),
                label,
                at,
            };
            let text = match &moving.message {
                Some(message_fn) => message_fn(&message),
                None => move_message(&message),
            };
            announce_with(&text, AnnouncePriority::Assertive, window, cx);
            cx.notify();
        }
        done
    }

    /// Moves one item to a point.
    pub fn move_to(
        &self,
        this: &mut T,
        key: &K,
        point: MovePoint,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let at = self.positions(this).get(key).copied().unwrap_or_default();
        self.move_by(
            this,
            std::slice::from_ref(key),
            point.x - at.x,
            point.y - at.y,
            options,
            window,
            cx,
        )
    }

    /// Alt with an arrow nudges the item's group a step, or `large_step`
    /// with Shift. Reports `MoveKey`. Returns whether it handled the key,
    /// which calls `window.prevent_default()`.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let modifiers = event.keystroke.modifiers;
        let Some((x, y)) = arrow(&Key::of(&event.keystroke)) else {
            return false;
        };
        if !modifiers.alt || self.is_disabled(this, key) {
            return false;
        }
        window.prevent_default();
        let step = self.step(this);
        let large_step = self.inner.options.borrow().large_step.clone();
        let by = if modifiers.shift {
            large_step.map_or(step * 10.0, |large_step| large_step(this))
        } else {
            step
        };
        let group = self.group(this, key);
        self.move_by(
            this,
            &group,
            x * by,
            y * by,
            ChangeOptions::event(Reason::MoveKey, event),
            window,
            cx,
        );
        true
    }

    /// Starts a drag once the pointer moves past [`DRAG_THRESHOLD`], so a
    /// click stays a click. Escape puts the items back. The drop lands on
    /// the step's grid and reports `Drag`.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drags = self.inner.drags.clone();
        if !drags.accepts(this, event, window, cx) || self.is_disabled(this, key) {
            return;
        }
        // Offsets count from the press, so the item keeps up with the pointer past the threshold.
        let start = event.position;
        let weak = Rc::downgrade(&self.inner);
        let (started, moved, ended) = (weak.clone(), weak.clone(), weak);
        let pressed = key.clone();
        drags.start(
            event,
            BindDragOptions::new()
                .threshold(DRAG_THRESHOLD)
                .cancel_on_escape(true)
                .on_start(move |this: &mut T, _event, _window, cx| {
                    if let Some(moving) = Self::upgrade(&started) {
                        let keys = moving.group(this, &pressed);
                        *moving.inner.draft.borrow_mut() = Some(Dragged {
                            draft: MoveDraft {
                                keys,
                                dx: 0.0,
                                dy: 0.0,
                            },
                            screen: Point::default(),
                        });
                        cx.notify();
                    }
                    true
                })
                .on_move(move |this: &mut T, sample, _window, cx| {
                    let Some(moving) = Self::upgrade(&moved) else {
                        return;
                    };
                    let scale = moving.inner.options.borrow().scale.clone();
                    let scale = scale.map_or(1.0, |scale| scale(this));
                    let screen = sample.position() - start;
                    if let Some(dragged) = moving.inner.draft.borrow_mut().as_mut() {
                        dragged.draft.dx = f64::from(f32::from(screen.x)) / scale;
                        dragged.draft.dy = f64::from(f32::from(screen.y)) / scale;
                        dragged.screen = screen;
                    }
                    cx.notify();
                })
                .on_end(move |this: &mut T, end, canceled, window, cx| {
                    let Some(moving) = Self::upgrade(&ended) else {
                        return;
                    };
                    let dragged = moving.inner.draft.borrow_mut().take();
                    cx.notify();
                    let truthy = |n: f64| n != 0.0 && !n.is_nan();
                    if let Some(Dragged { draft, .. }) = dragged
                        && !canceled
                        && (truthy(draft.dx) || truthy(draft.dy))
                    {
                        moving.move_by(
                            this,
                            &draft.keys,
                            draft.dx,
                            draft.dy,
                            ChangeOptions::event_of(Reason::Drag, end.event.clone()),
                            window,
                            cx,
                        );
                    }
                }),
            window,
            cx,
        );
    }

    /// Whether a pointer is dragging the item.
    pub fn dragging(&self, key: &K) -> bool {
        self.inner
            .draft
            .borrow()
            .as_ref()
            .is_some_and(|dragged| dragged.draft.keys.contains(key))
    }

    /// The draft while a pointer drags.
    pub fn draft(&self) -> Option<MoveDraft<K>> {
        self.inner
            .draft
            .borrow()
            .as_ref()
            .map(|dragged| dragged.draft.clone())
    }

    /// The dragged item's offset, in units, the original's `--move-x` and
    /// `--move-y`, or `None` when it isn't dragged (`style`).
    pub fn style(&self, key: &K) -> Option<MoveStyle> {
        self.inner
            .draft
            .borrow()
            .as_ref()
            .filter(|dragged| dragged.draft.keys.contains(key))
            .map(|dragged| MoveStyle {
                x: dragged.draft.dx,
                y: dragged.draft.dy,
            })
    }

    /// The item's element, drawn as far as the pointer has dragged it while
    /// it's dragged, the original's `translate`. Its layout doesn't move.
    pub fn follow<E: IntoElement>(&self, key: &K, el: E) -> impl IntoElement + use<T, K, E> {
        let offset = self
            .inner
            .draft
            .borrow()
            .as_ref()
            .filter(|dragged| dragged.draft.keys.contains(key))
            .map(|dragged| dragged.screen)
            .unwrap_or_default();
        Translate {
            offset,
            child: el.into_any_element(),
        }
    }

    /// The element that follows the pointer while items are dragged. Render
    /// it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T, K> {
        self.inner.drags.listener()
    }

    /// Wires an item's presses, which drag it and its group
    /// ([`pointer_down`](Self::pointer_down)).
    pub fn bind_item<E: InteractiveElement + ParentElement + Styled>(&self, key: K, el: E) -> E {
        let moving = self.clone();
        on_pointer_down(el, move |event, window, cx| {
            let Some(entity) = moving.inner.entity.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                moving.pointer_down(this, event, &key, window, cx)
            });
        })
    }
}
