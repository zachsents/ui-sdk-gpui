//! Port of `foundation/direct-manipulation/swipe-actions.ts`: actions behind
//! a list's rows, as mail apps have: swiping a row aside shows the buttons
//! under it, a release past half of them leaves it open, a swipe that keeps
//! going, or a flick, opens or closes it the way it went, and a long swipe
//! runs the first action at once, like archiving a message. From the
//! keyboard, Left and Right open a row's actions and move into them, and
//! Escape closes them, so nothing needs a pointer.
//!
//! The row's offset is a draft the swipe keeps, the original's `draft`
//! state key, and the open row is its `key`; [`SwipeActions::content`] draws
//! the row's content moved by the offset, as a translate. A mouse swipes
//! through a drag session that waits for the pointer to move across, a press
//! that moves up or down first being a scroll. A finger swipes through the
//! pan gpui makes of it, which gpui keeps to the axis it starts along, as the
//! original's `pick` does: a row takes the pans across it
//! ([`SwipeActions::scroll`]) and leaves those up and down to the list,
//! which is the original's `touch-action: pan-y`. Rows and their actions join
//! as they render ([`SwipeActions::bind_row`],
//! [`SwipeActions::bind_actions`]), which records where they lay out, for
//! the actions' widths and the presses outside an open row; the actions'
//! buttons are their tab stops, which the original queries as
//! `button, [href], [tabindex]`.

use std::cell::{Cell, RefCell};
use std::hash::Hash;
use std::rc::{Rc, Weak};

use gpui::{
    App, Context, DispatchPhase, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseDownEvent, ParentElement, Pixels, Point, ScrollEvent, ScrollSource, Styled,
    TouchDragEvent, TouchPhase, WeakEntity, Window, canvas, deferred, div, point, px,
};
use ui_sdk_drag_and_drop::{
    BindDragOptions, DragSession, KeyFn, PointerEvent, PointerKind, ReadFn, StartOnMoveOptions,
    on_pointer_down,
};
use ui_sdk_focus::tabbables;
use ui_sdk_kernel::{InputEvent, Key};
use ui_sdk_motion::{VelocityTracker, rubber_band, timestamp};
use ui_sdk_platform::is_rtl;

use crate::js;
use crate::target::Bound;
use crate::translate::Translate;

// How far a press moves before it's a swipe, and how fast a release, in px
// per ms, is a flick.
const SLOP: f32 = 8.;
const FLICK: f64 = 0.3;

/// The side of a row its actions sit on (`SwipeSide`): `Start` shows by
/// swiping toward the end, left to right in English.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SwipeSide {
    Start,
    End,
}

impl SwipeSide {
    /// The original's name for it, its `data-side`.
    pub fn as_str(self) -> &'static str {
        match self {
            SwipeSide::Start => "start",
            SwipeSide::End => "end",
        }
    }
}

/// The row whose actions show, and on which side (`SwipeOpen`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SwipeOpen<K> {
    pub key: K,
    pub side: SwipeSide,
}

/// A row a pointer is swiping (`SwipeDraft`).
#[derive(Clone, Debug, PartialEq)]
pub struct SwipeDraft<K> {
    pub key: K,
    /// How far the row has moved, in pixels, positive to the right.
    pub x: f64,
    /// Whether letting go now runs the first action.
    pub full: bool,
}

/// What `on_full_swipe` runs: a side's first action for a row, with the
/// event that let go. True when it ran.
pub type FullSwipeFn<T, K> =
    Rc<dyn Fn(&mut T, &K, SwipeSide, &InputEvent, &mut Window, &mut Context<T>) -> bool>;

/// How [`create_swipe_actions`] keeps its rows (`SwipeActionsOptions`). `T`
/// is the component, `K` a row's key.
pub struct SwipeActionsOptions<T: 'static, K> {
    /// Runs a side's first action when a swipe goes past `full_swipe` of the
    /// row's width, and returns whether it did. Without it, a long swipe
    /// only opens the row.
    pub on_full_swipe: Option<FullSwipeFn<T, K>>,
    /// How far across the row a swipe runs the first action, as a fraction
    /// of its width. Defaults to 0.6.
    pub full_swipe: Option<ReadFn<T, f64>>,
    /// Whether a row can't be swiped.
    pub disabled: Option<KeyFn<T, K, bool>>,
}

impl<T: 'static, K> Default for SwipeActionsOptions<T, K> {
    fn default() -> Self {
        Self {
            on_full_swipe: None,
            full_swipe: None,
            disabled: None,
        }
    }
}

impl<T: 'static, K> Clone for SwipeActionsOptions<T, K> {
    fn clone(&self) -> Self {
        Self {
            on_full_swipe: self.on_full_swipe.clone(),
            full_swipe: self.full_swipe.clone(),
            disabled: self.disabled.clone(),
        }
    }
}

impl<T: 'static, K> SwipeActionsOptions<T, K> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_full_swipe(
        mut self,
        on_full_swipe: impl Fn(&mut T, &K, SwipeSide, &InputEvent, &mut Window, &mut Context<T>) -> bool
        + 'static,
    ) -> Self {
        self.on_full_swipe = Some(Rc::new(on_full_swipe));
        self
    }

    pub fn full_swipe(mut self, full_swipe: impl Fn(&T) -> f64 + 'static) -> Self {
        self.full_swipe = Some(Rc::new(full_swipe));
        self
    }

    pub fn disabled(mut self, disabled: impl Fn(&T, &K) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(disabled));
        self
    }
}

/// The live attributes of a row's moving content (`contentAttrs`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwipeContentAttrs {
    /// Its offset, in pixels, positive to the right, the original's `--swipe-x`.
    pub x: f64,
    /// The side it shows, to show only that side's actions (`data-swipe`).
    pub swipe: Option<SwipeSide>,
    /// Whether a pointer moves it, to turn off a transition (`data-swiping`).
    pub swiping: bool,
    /// Whether letting go would run the first action (`data-full`).
    pub full: bool,
}

/// The attributes of the actions on one side of a row (`actionsAttrs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwipeActionsAttrs {
    /// Which side they sit on, the original's `data-side`.
    pub side: SwipeSide,
    /// Whether they're hidden, unless the row is open on that side, so Tab
    /// skips them. gpui can't make a subtree inert, so take their buttons
    /// out of the tab order and their handlers off while it's set.
    pub inert: bool,
}

// What rows and their actions bind as.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SwipeTarget<K> {
    Row(K),
    Actions(K, SwipeSide),
}

// A swipe in progress (`pointerdown`'s closure).
struct Gesture<K> {
    key: K,
    // Where the row was when it was pressed, open or not.
    from: f64,
    row_width: f64,
    fraction: f64,
    velocity: VelocityTracker,
    // Where the pointer pressed, and how far a pan has gone since.
    origin: f64,
    travel: f64,
    // The last place: the offset, the side it shows and whether it's full.
    last: (f64, SwipeSide, bool),
}

type GestureRef<K> = Rc<RefCell<Gesture<K>>>;

struct SwipeInner<T: 'static, K> {
    options: RefCell<SwipeActionsOptions<T, K>>,
    entity: WeakEntity<T>,
    drags: DragSession<T>,
    bound: Bound<SwipeTarget<K>>,
    open: RefCell<Option<SwipeOpen<K>>>,
    draft: RefCell<Option<SwipeDraft<K>>>,
    // A finger's pan across a row.
    pan: RefCell<Option<GestureRef<K>>>,
    // A row whose pan ended, whose coasting it keeps from scrolling anything.
    coasting: RefCell<Option<K>>,
    // Where and when the last finger pressed a row, which its pan starts from.
    touch_down: Cell<Option<(Point<Pixels>, f64)>>,
}

/// Lets users swipe a list's rows aside to their actions
/// (`createSwipeActions`'s return). Cloning it gives another handle to the
/// same rows.
pub struct SwipeActions<T: 'static, K> {
    inner: Rc<SwipeInner<T, K>>,
}

impl<T: 'static, K> Clone for SwipeActions<T, K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

fn length(pixels: Pixels) -> f64 {
    f64::from(f32::from(pixels))
}

/// Lets users swipe a list's rows aside to their actions
/// (`createSwipeActions`). Make it in the component's constructor, keep it
/// in a field, render its [`listener`](SwipeActions::listener) on every
/// frame, and wire a row, its moving content, and the actions on each side
/// under it:
///
/// ```ignore
/// let row = swipe.bind_row(key.clone(), Some(&handle), row_part, cx)
///     .on_keys(&["ArrowLeft", "ArrowRight", "Escape"], move |event, window, cx| {
///         entity.update(cx, |this, cx| { this.swipe.clone().keydown(this, event, &key, window, cx); }).ok();
///     });
/// let actions = swipe.bind_actions(key.clone(), SwipeSide::End, actions_part, cx);
/// let content = swipe.content(&key, content_part, cx);
/// ```
///
/// Give the row `overflow_hidden()` and its actions their place under the
/// content, at its start or end; [`content_attrs`](SwipeActions::content_attrs)
/// says which side shows.
pub fn create_swipe_actions<T: 'static, K: Clone + Eq + Hash + 'static>(
    options: SwipeActionsOptions<T, K>,
    cx: &Context<T>,
) -> SwipeActions<T, K> {
    SwipeActions {
        inner: Rc::new(SwipeInner {
            options: RefCell::new(options),
            entity: cx.weak_entity(),
            drags: DragSession::new(cx),
            bound: Bound::new(),
            open: RefCell::new(None),
            draft: RefCell::new(None),
            pan: RefCell::new(None),
            coasting: RefCell::new(None),
            touch_down: Cell::new(None),
        }),
    }
}

impl<T: 'static, K: Clone + Eq + Hash + 'static> SwipeActions<T, K> {
    fn upgrade(inner: &Weak<SwipeInner<T, K>>) -> Option<Self> {
        inner.upgrade().map(|inner| Self { inner })
    }

    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: SwipeActionsOptions<T, K>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn is_disabled(&self, this: &T, key: &K) -> bool {
        let disabled = self.inner.options.borrow().disabled.clone();
        disabled.is_some_and(|disabled| disabled(this, key))
    }

    // Which way a side's actions push the row: `Start` to the right in
    // left-to-right text.
    fn sign(side: SwipeSide, rtl: bool) -> f64 {
        if (side == SwipeSide::Start) != rtl {
            1.0
        } else {
            -1.0
        }
    }

    fn side_of(x: f64, rtl: bool) -> SwipeSide {
        if (x > 0.0) != rtl {
            SwipeSide::Start
        } else {
            SwipeSide::End
        }
    }

    fn width(&self, key: &K, side: SwipeSide) -> f64 {
        self.inner
            .bound
            .region(&SwipeTarget::Actions(key.clone(), side))
            .and_then(|region| region.bounds())
            .map_or(0.0, |bounds| length(bounds.size.width))
    }

    // The actions' buttons, in Tab order.
    fn buttons(
        &self,
        key: &K,
        side: SwipeSide,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<FocusHandle> {
        match self
            .inner
            .bound
            .region(&SwipeTarget::Actions(key.clone(), side))
            .and_then(|region| region.focus_handle())
        {
            Some(container) => tabbables(&container, window, cx),
            None => Vec::new(),
        }
    }

    fn set_open(&self, next: Option<SwipeOpen<K>>, cx: &mut Context<T>) {
        *self.inner.open.borrow_mut() = next;
        *self.inner.draft.borrow_mut() = None;
        cx.notify();
    }

    /// The row whose actions show, if any.
    pub fn open(&self) -> Option<SwipeOpen<K>> {
        self.inner.open.borrow().clone()
    }

    /// The row a pointer is swiping, if any.
    pub fn draft(&self) -> Option<SwipeDraft<K>> {
        self.inner.draft.borrow().clone()
    }

    /// How far a row has moved, in pixels, positive to the right: the
    /// swipe's, or the open side's width.
    pub fn offset(&self, key: &K, cx: &App) -> f64 {
        if let Some(draft) = self.inner.draft.borrow().as_ref()
            && draft.key == *key
        {
            return draft.x;
        }
        match self.open() {
            Some(open) if open.key == *key => {
                Self::sign(open.side, is_rtl(cx)) * self.width(key, open.side)
            }
            _ => 0.0,
        }
    }

    /// The live attributes of the row's moving content: its offset, the
    /// side it shows, whether a pointer moves it, and whether letting go
    /// would run the first action (`contentAttrs`).
    pub fn content_attrs(&self, key: &K, cx: &App) -> SwipeContentAttrs {
        let x = self.offset(key, cx);
        let (swiping, full) = match self.inner.draft.borrow().as_ref() {
            Some(draft) if draft.key == *key => (true, draft.full),
            _ => (false, false),
        };
        SwipeContentAttrs {
            x,
            swipe: (x != 0.0 && !x.is_nan()).then(|| Self::side_of(x, is_rtl(cx))),
            swiping,
            full,
        }
    }

    /// The row's moving content, drawn moved by its offset, as the
    /// original's `transform: translateX(var(--swipe-x))`.
    pub fn content<E: IntoElement>(
        &self,
        key: &K,
        el: E,
        cx: &App,
    ) -> impl IntoElement + use<T, K, E> {
        let x = self.offset(key, cx);
        Translate {
            offset: point(px(x as f32), px(0.)),
            child: el.into_any_element(),
        }
    }

    /// The actions on a side: which side, and whether they're hidden
    /// (`actionsAttrs`).
    pub fn actions_attrs(&self, key: &K, side: SwipeSide) -> SwipeActionsAttrs {
        let visible = self
            .open()
            .is_some_and(|open| open.key == *key && open.side == side);
        SwipeActionsAttrs {
            side,
            inert: !visible,
        }
    }

    /// Joins a row as it renders, with the focus handle it tracks, which
    /// takes focus back when its actions close, and wires its presses
    /// ([`pointer_down`](Self::pointer_down)) and a finger's pans across it
    /// ([`scroll`](Self::scroll)). A press elsewhere closes it while open.
    pub fn bind_row<E: InteractiveElement + ParentElement + Styled>(
        &self,
        key: K,
        focus: Option<&FocusHandle>,
        el: E,
        cx: &App,
    ) -> E {
        let el = self
            .inner
            .bound
            .bind(SwipeTarget::Row(key.clone()), focus, el, cx);
        let swipe = self.clone();
        let pressed = key.clone();
        let el = on_pointer_down(el, move |event, window, cx| {
            swipe.in_update(cx, |swipe, this, cx| {
                swipe.pointer_down(this, event, &pressed, window, cx)
            });
        });
        let swipe = self.clone();
        el.on_scroll(move |event, window, cx| {
            swipe.in_update(cx, |swipe, this, cx| {
                swipe.scroll(this, event, &key, window, cx)
            });
        })
    }

    /// Joins the actions on one side of a row, which a swipe measures to
    /// know how far it opens, and whose buttons the keyboard moves through.
    pub fn bind_actions<E: InteractiveElement + ParentElement + Styled>(
        &self,
        key: K,
        side: SwipeSide,
        el: E,
        cx: &App,
    ) -> E {
        let target = SwipeTarget::Actions(key, side);
        let container = self
            .inner
            .bound
            .focus_of(&target)
            .unwrap_or_else(|| cx.focus_handle());
        let el = el.track_focus(&container);
        self.inner.bound.bind(target, Some(&container), el, cx)
    }

    fn in_update(&self, cx: &mut App, f: impl FnOnce(&Self, &mut T, &mut Context<T>)) {
        if let Some(entity) = self.inner.entity.upgrade() {
            entity.update(cx, |this, cx| f(self, this, cx));
        }
    }

    // Focuses the first of a side's actions, if one takes Tab now.
    fn focus_first(&self, key: &K, side: SwipeSide, window: &mut Window, cx: &mut App) -> bool {
        match self.buttons(key, side, window, cx).into_iter().next() {
            Some(first) => {
                window.focus(&first, cx);
                true
            }
            None => false,
        }
    }

    /// Opens a row's actions on a side, closing any other row's. With
    /// `focus`, its first action takes focus, once the actions render as
    /// shown. Returns whether it opened.
    pub fn show(
        &self,
        this: &T,
        key: &K,
        side: SwipeSide,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        if self.width(key, side) == 0.0 || self.is_disabled(this, key) {
            return false;
        }
        self.set_open(
            Some(SwipeOpen {
                key: key.clone(),
                side,
            }),
            cx,
        );
        // The actions take Tab once they render as shown, after this frame.
        if focus && !self.focus_first(key, side, window, cx) {
            let swipe = self.clone();
            let key = key.clone();
            window.on_next_frame(move |window, _cx| {
                window.on_next_frame(move |window, cx| {
                    swipe.focus_first(&key, side, window, cx);
                });
            });
        }
        true
    }

    /// Closes the open row. With `focus`, the row takes focus back, as
    /// after Escape. Returns whether a row was open.
    pub fn close(&self, focus: bool, window: &mut Window, cx: &mut Context<T>) -> bool {
        let Some(shown) = self.open() else {
            return false;
        };
        if focus
            && let Some(row) = self
                .inner
                .bound
                .region(&SwipeTarget::Row(shown.key))
                .and_then(|region| region.focus_handle())
        {
            window.focus(&row, cx);
        }
        self.set_open(None, cx);
        true
    }

    // A press on a row that may become a swipe, or `None` when it can't.
    // `pressed` is when the pointer pressed there, if known.
    fn gesture(
        &self,
        this: &T,
        key: &K,
        at: Point<Pixels>,
        pressed: Option<f64>,
        window: &Window,
        cx: &App,
    ) -> Option<Gesture<K>> {
        let rtl = is_rtl(cx);
        let shown = self.open().filter(|open| open.key == *key);
        // A press inside the open actions is a click on one.
        if let Some(shown) = &shown
            && self
                .inner
                .bound
                .region(&SwipeTarget::Actions(key.clone(), shown.side))
                .is_some_and(|actions| actions.contains_point(at, window))
        {
            return None;
        }
        let from = shown.map_or(0.0, |shown| {
            Self::sign(shown.side, rtl) * self.width(key, shown.side)
        });
        if (from == 0.0 || from.is_nan())
            && self.width(key, SwipeSide::Start) == 0.0
            && self.width(key, SwipeSide::End) == 0.0
        {
            return None;
        }
        let row_width = self
            .inner
            .bound
            .region(&SwipeTarget::Row(key.clone()))
            .and_then(|region| region.bounds())
            .map_or(0.0, |bounds| length(bounds.size.width));
        let full_swipe = self.inner.options.borrow().full_swipe.clone();
        let mut velocity = VelocityTracker::default();
        if let Some(pressed) = pressed {
            velocity.add(pressed, length(at.x));
        }
        Some(Gesture {
            key: key.clone(),
            from,
            row_width: if row_width == 0.0 || row_width.is_nan() {
                1.0
            } else {
                row_width
            },
            fraction: full_swipe.map_or(0.6, |full_swipe| full_swipe(this)),
            velocity,
            origin: length(at.x),
            travel: 0.0,
            last: (from, Self::side_of(from, rtl), false),
        })
    }

    // Moves the row as far as the pointer went: past its actions it moves
    // on toward a full swipe, or resists without one.
    fn place(&self, gesture: &mut Gesture<K>, dx: f64, cx: &mut Context<T>) {
        let raw = gesture.from + dx;
        let side = Self::side_of(raw, is_rtl(cx));
        let room = self.width(&gesture.key, side);
        let full = self.inner.options.borrow().on_full_swipe.is_some() && room > 0.0;
        let limit = if full { gesture.row_width } else { room };
        let past = raw.abs() - limit;
        let x = if past > 0.0 {
            js::sign(raw) * (limit + rubber_band(past, gesture.row_width))
        } else {
            raw
        };
        gesture.last = (
            x,
            side,
            full && raw.abs() >= gesture.row_width * gesture.fraction,
        );
        *self.inner.draft.borrow_mut() = Some(SwipeDraft {
            key: gesture.key.clone(),
            x,
            full: gesture.last.2,
        });
        cx.notify();
    }

    // Lets go: runs the first action for a full swipe, or opens or closes
    // the row the way a flick went, or by how far it moved.
    fn finish(
        &self,
        gesture: &mut Gesture<K>,
        this: &mut T,
        end: &InputEvent,
        canceled: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let key = gesture.key.clone();
        if self.is_disabled(this, &key) {
            return self.set_open(None, cx);
        }
        let (x, side, full) = gesture.last;
        let on_full_swipe = self.inner.options.borrow().on_full_swipe.clone();
        if !canceled
            && full
            && let Some(on_full_swipe) = on_full_swipe
            && on_full_swipe(this, &key, side, end, window, cx)
        {
            return self.set_open(None, cx);
        }
        let room = self.width(&key, side);
        // A flick decides whichever way it went; otherwise past half the actions stays open.
        let speed = if canceled {
            0.0
        } else {
            gesture.velocity.velocity(timestamp())
        };
        let opening = if speed.abs() >= FLICK {
            js::sign(speed) == js::sign(x)
        } else {
            x.abs() >= room / 2.0
        };
        let truthy = |n: f64| n != 0.0 && !n.is_nan();
        let next = (opening && truthy(room) && truthy(x)).then_some(SwipeOpen { key, side });
        self.set_open(next, cx);
    }

    /// Follows a mouse press on a row that may become a swipe across it. The
    /// press stays a press until the pointer moves a few pixels across, so
    /// clicks and scrolls keep working, and a click on an open row closes it.
    /// A finger's press is left to the pan gpui makes of it
    /// ([`scroll`](Self::scroll)), so the list still scrolls.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if event.kind == PointerKind::Touch {
            self.inner
                .touch_down
                .set(Some((event.position, timestamp())));
            return;
        }
        let drags = self.inner.drags.clone();
        if !drags.accepts(this, event, window, cx) || self.is_disabled(this, key) {
            return;
        }
        let Some(gesture) = self.gesture(this, key, event.position, Some(timestamp()), window, cx)
        else {
            return;
        };
        let from = gesture.from;
        let gesture = Rc::new(RefCell::new(gesture));
        let weak = Rc::downgrade(&self.inner);
        let (picked, moved, ended, tapped) = (weak.clone(), weak.clone(), weak.clone(), weak);
        let (picking, moving, ending) = (gesture.clone(), gesture.clone(), gesture);
        let down = event.position;
        drags.start_on_move(
            event,
            StartOnMoveOptions::new(
                BindDragOptions::new()
                    .on_move(move |_this: &mut T, sample, _window, cx| {
                        let Some(swipe) = Self::upgrade(&moved) else {
                            return;
                        };
                        let mut gesture = moving.borrow_mut();
                        let x = length(sample.x);
                        gesture.velocity.add(timestamp(), x);
                        let dx = x - length(down.x);
                        swipe.place(&mut gesture, dx, cx);
                    })
                    .on_end(move |this: &mut T, end, canceled, window, cx| {
                        let Some(swipe) = Self::upgrade(&ended) else {
                            return;
                        };
                        let mut gesture = ending.borrow_mut();
                        swipe.finish(&mut gesture, this, &end.event, canceled, window, cx);
                    }),
            )
            .pick(move |_this: &mut T, next, _window, cx| {
                let dx = f32::from(next.x() - down.x);
                let dy = f32::from(next.y() - down.y);
                if dx.hypot(dy) < SLOP {
                    return None;
                }
                // A press that moves up or down first is a scroll, not a swipe.
                if dy.abs() >= dx.abs() {
                    return Some(false);
                }
                let Some(swipe) = Self::upgrade(&picked) else {
                    return Some(false);
                };
                let mut gesture = picking.borrow_mut();
                gesture.velocity.add(timestamp(), length(next.x()));
                swipe.place(&mut gesture, f64::from(dx), cx);
                Some(true)
            })
            // A tap on an open row closes it, as a tap on its content would on a phone.
            .on_tap(move |_this: &mut T, _up, window, cx| {
                if let Some(swipe) = Self::upgrade(&tapped)
                    && from != 0.0
                    && !from.is_nan()
                {
                    swipe.close(false, window, cx);
                }
            }),
            window,
            cx,
        );
    }

    /// Starts following a finger's pan across a row, which gpui reports as
    /// scrolling that starts where the finger pressed. A pan across a row
    /// with actions swipes it, and stops there; one up or down, which gpui
    /// keeps to its axis, scrolls the list. Call it from the row's
    /// `on_scroll`, as [`bind_row`](Self::bind_row) does; the
    /// [`listener`](Self::listener) follows the rest of the pan.
    pub fn scroll(
        &self,
        this: &mut T,
        event: &ScrollEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if event.source != ScrollSource::Touch || event.touch_phase != TouchPhase::Started {
            return;
        }
        let dx = length(event.delta.pixel_delta(px(1.)).x);
        // gpui keeps a pan to the axis it starts along.
        if dx == 0.0 || self.is_disabled(this, key) {
            return;
        }
        // The pan starts where the finger pressed, which said when.
        let pressed = self
            .inner
            .touch_down
            .take()
            .filter(|(at, _)| *at == event.position)
            .map(|(_, time)| time);
        let Some(mut gesture) = self.gesture(this, key, event.position, pressed, window, cx) else {
            return;
        };
        gesture.travel = dx;
        let at = gesture.origin + dx;
        gesture.velocity.add(timestamp(), at);
        self.place(&mut gesture, dx, cx);
        *self.inner.pan.borrow_mut() = Some(Rc::new(RefCell::new(gesture)));
        cx.stop_propagation();
    }

    // A step of a finger's pan, wherever the row has gone since it started,
    // or of the coasting after one, which reaches nothing else.
    fn panned(
        inner: &Weak<SwipeInner<T, K>>,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut App,
    ) {
        if event.source != ScrollSource::Touch {
            return;
        }
        let Some(swipe) = Self::upgrade(inner) else {
            return;
        };
        let pan = swipe.inner.pan.borrow().clone();
        let dx = length(event.delta.pixel_delta(px(1.)).x);
        match (event.touch_phase, pan) {
            (TouchPhase::Started, pan) => {
                swipe.inner.coasting.borrow_mut().take();
                // A pan whose end never came puts its row back.
                if let Some(pan) = pan {
                    swipe.inner.pan.borrow_mut().take();
                    swipe.in_update(cx, |swipe, this, cx| {
                        let mut gesture = pan.borrow_mut();
                        swipe.finish(
                            &mut gesture,
                            this,
                            &InputEvent::from(event),
                            true,
                            window,
                            cx,
                        );
                    });
                }
            }
            (TouchPhase::Moved, Some(pan)) => {
                swipe.in_update(cx, |swipe, _this, cx| {
                    let mut gesture = pan.borrow_mut();
                    gesture.travel += dx;
                    let (at, travel) = (gesture.origin + gesture.travel, gesture.travel);
                    gesture.velocity.add(timestamp(), at);
                    swipe.place(&mut gesture, travel, cx);
                });
                cx.stop_propagation();
            }
            (TouchPhase::Ended | TouchPhase::Cancelled, Some(pan)) => {
                swipe.inner.pan.borrow_mut().take();
                let canceled = event.touch_phase == TouchPhase::Cancelled;
                swipe.in_update(cx, |swipe, this, cx| {
                    let mut gesture = pan.borrow_mut();
                    if !canceled {
                        gesture.travel += dx;
                        let travel = gesture.travel;
                        swipe.place(&mut gesture, travel, cx);
                        // A fling coasts on where the finger pressed.
                        *swipe.inner.coasting.borrow_mut() = Some(gesture.key.clone());
                    }
                    swipe.finish(
                        &mut gesture,
                        this,
                        &InputEvent::from(event),
                        canceled,
                        window,
                        cx,
                    );
                });
                cx.stop_propagation();
            }
            (phase, None) if swipe.inner.coasting.borrow().is_some() => {
                if phase != TouchPhase::Moved {
                    swipe.inner.coasting.borrow_mut().take();
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// Handles a key on a row or its actions and returns whether it did.
    /// Left and Right open the actions on the side the row would swipe
    /// toward and focus the first, then move between them; toward the row's
    /// content from the last one, or Escape, closes them and puts focus back
    /// on the row. A key it handles calls `window.prevent_default()`.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented()
            || modifiers.alt
            || modifiers.control
            || modifiers.platform
            || modifiers.shift
        {
            return false;
        }
        let shown = self.open().filter(|open| open.key == *key);
        let pressed = Key::of(&event.keystroke);
        if pressed == Key::Escape && shown.is_some() {
            window.prevent_default();
            return self.close(true, window, cx);
        }
        if pressed != Key::ArrowLeft && pressed != Key::ArrowRight {
            return false;
        }
        let rtl = is_rtl(cx);
        // Which way on screen: Left would swipe the row left, showing the actions on its right.
        let dx: isize = if pressed == Key::ArrowRight { 1 } else { -1 };
        let Some(shown) = shown else {
            if !self.show(this, key, Self::side_of(dx as f64, rtl), true, window, cx) {
                return false;
            }
            window.prevent_default();
            return true;
        };
        window.prevent_default();
        let list = self.buttons(key, shown.side, window, cx);
        let focused = window.focused(cx);
        let index = focused.and_then(|focused| list.iter().position(|button| *button == focused));
        let sign = Self::sign(shown.side, rtl) as isize;
        let next = index.map_or(0, |index| index as isize + if rtl { -dx } else { dx });
        let off = match index {
            None => dx == sign,
            Some(_) => next < 0 || next >= list.len() as isize,
        };
        if off {
            // The row's content lies the way it moved, so heading there closes the actions.
            if dx == sign {
                self.close(true, window, cx);
            }
            return true;
        }
        if let Some(button) = list.get(next as usize) {
            window.focus(button, cx);
        }
        true
    }

    /// The element that follows the pointer and a finger's pan while a row
    /// is swiped, and that closes the open row on a press anywhere else.
    /// Render it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T, K> {
        let weak = Rc::downgrade(&self.inner);
        let outside = deferred(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let swipe = weak.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            Self::pressed(&swipe, event.position, window, cx);
                        }
                    });
                    let swipe = weak.clone();
                    window.on_mouse_event(move |event: &TouchDragEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture && event.phase == TouchPhase::Started {
                            Self::pressed(&swipe, event.position, window, cx);
                        }
                    });
                    // Last in the capture phase, so a pan it takes reaches no
                    // scroller's bubble phase.
                    let swipe = weak.clone();
                    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            Self::panned(&swipe, event, window, cx);
                        }
                    });
                },
            )
            .absolute(),
        )
        .with_priority(usize::MAX);
        div()
            .absolute()
            .child(self.inner.drags.listener())
            .child(outside)
    }

    // A press anywhere but the open row closes it.
    fn pressed(
        inner: &Weak<SwipeInner<T, K>>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(swipe) = Self::upgrade(inner) else {
            return;
        };
        let Some(open) = swipe.open() else {
            return;
        };
        let inside = swipe
            .inner
            .bound
            .region(&SwipeTarget::Row(open.key))
            .is_some_and(|row| row.contains_point(at, window));
        if !inside {
            swipe.in_update(cx, |swipe, _this, cx| {
                swipe.close(false, window, cx);
            });
        }
    }
}
