//! Port of `foundation/selection/lasso.ts`: rectangle selection: drag on
//! empty space and every item the rectangle touches is selected, like on a
//! desktop or a design canvas. Shift adds to what was selected. The
//! rectangle lives in the lasso, so it renders like any part, and the
//! selection commits once, when the pointer lifts. Escape lets go without
//! selecting.
//!
//! The items are the parts of one slot, which the component keeps in a
//! kernel [`Parts`] and measures with `Parts::measure`: what the original
//! reads with `elements(context, slot)`. Item boxes are read once, when the
//! drag starts, so drawing measures nothing.

use std::cell::RefCell;
use std::hash::Hash;
use std::rc::Rc;

use gpui::{
    Bounds, Context, IntoElement, ParentElement, Pixels, Point, Styled, Window, point,
};
use ui_sdk_drag_and_drop::{BindDragOptions, DRAG_THRESHOLD, DragSession, PointerEvent};
use ui_sdk_kernel::{ChangeOptions, InputEvent, Measured, PartInfo, Parts, Reason};
use ui_sdk_platform::announce;

use crate::selection_model::{KeysFn, SelectableFn};

/// The rectangle being drawn, relative to the surface, and what it selects
/// so far (`LassoDraft`).
#[derive(Clone, Debug, PartialEq)]
pub struct LassoDraft<K> {
    pub left: Pixels,
    pub top: Pixels,
    pub right: Pixels,
    pub bottom: Pixels,
    pub keys: Vec<K>,
}

impl<K> LassoDraft<K> {
    /// The rectangle, relative to the surface.
    pub fn bounds(&self) -> Bounds<Pixels> {
        Bounds::from_corners(point(self.left, self.top), point(self.right, self.bottom))
    }
}

/// Why the selection changed: the rectangle lifted, the original's
/// `Lasso` reason, with the release behind it.
#[derive(Clone, Debug)]
pub struct LassoDetails {
    pub event: Option<InputEvent>,
}

impl LassoDetails {
    /// The original's name for the reason.
    pub const REASON: &'static str = "lasso";

    /// The details as a kernel change reports them, for
    /// `Controllable::change`.
    pub fn change_options(&self) -> ChangeOptions {
        ChangeOptions {
            reason: Some(Reason::Lasso),
            source: None,
            event: self.event.clone(),
        }
    }
}

/// Asks the component for a new selection and returns whether it went
/// through. It runs inside the component's update.
pub type SetLassoFn<T, K> =
    Rc<dyn Fn(&mut T, Vec<K>, LassoDetails, &mut Window, &mut Context<T>) -> bool>;

/// What screen readers hear when the rectangle lifts, by how many items it
/// selected.
pub type LassoMessageFn = Rc<dyn Fn(usize) -> String>;

/// How [`create_lasso`] finds items and changes the selection
/// (`LassoOptions`). `T` is the component, `K` an item's key, `P` what its
/// parts register with.
pub struct LassoOptions<T: 'static, K, P = PartInfo> {
    /// The parts the rectangle selects (`slot`), measured with
    /// `Parts::measure`.
    pub parts: Parts<K, P>,
    /// The selection now, which Shift adds to.
    pub selected: KeysFn<T, K>,
    /// Asks for a new selection. Pass the component's change.
    pub set: SetLassoFn<T, K>,
    /// Whether an item can be selected. Defaults to every item.
    pub selectable: Option<SelectableFn<T, K>>,
    /// What screen readers hear when the rectangle lifts. Defaults to
    /// English, like "3 items selected."
    pub message: Option<LassoMessageFn>,
}

impl<T: 'static, K, P> Clone for LassoOptions<T, K, P> {
    fn clone(&self) -> Self {
        Self {
            parts: self.parts.clone(),
            selected: self.selected.clone(),
            set: self.set.clone(),
            selectable: self.selectable.clone(),
            message: self.message.clone(),
        }
    }
}

impl<T: 'static, K: 'static, P> LassoOptions<T, K, P> {
    /// Options with the parts, the selection and how to change it.
    pub fn new(
        parts: Parts<K, P>,
        selected: impl Fn(&T) -> Vec<K> + 'static,
        set: impl Fn(&mut T, Vec<K>, LassoDetails, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        Self {
            parts,
            selected: Rc::new(selected),
            set: Rc::new(set),
            selectable: None,
            message: None,
        }
    }

    pub fn selectable(mut self, selectable: impl Fn(&T, &K) -> bool + 'static) -> Self {
        self.selectable = Some(Rc::new(selectable));
        self
    }

    pub fn message(mut self, message: impl Fn(usize) -> String + 'static) -> Self {
        self.message = Some(Rc::new(message));
        self
    }
}

fn touches(a: &Bounds<Pixels>, b: &Bounds<Pixels>) -> bool {
    a.left() < b.right() && a.right() > b.left() && a.top() < b.bottom() && a.bottom() > b.top()
}

// A rectangle being drawn: where it started on the surface, what Shift
// keeps, and the items' boxes, all relative to the surface.
struct Gesture<K> {
    origin: Point<Pixels>,
    start: Point<Pixels>,
    base: Vec<K>,
    items: Vec<(K, Bounds<Pixels>)>,
}

struct Inner<T: 'static, K, P> {
    options: RefCell<LassoOptions<T, K, P>>,
    draft: RefCell<Option<LassoDraft<K>>>,
    gesture: RefCell<Option<Gesture<K>>>,
    surface: Measured,
    drags: DragSession<T>,
}

/// Selects a slot's parts by drawing a rectangle around them
/// (`createLasso`'s return). Cloning it gives another handle to the same
/// lasso.
pub struct Lasso<T: 'static, K, P = PartInfo> {
    inner: Rc<Inner<T, K, P>>,
}

impl<T: 'static, K, P> Clone for Lasso<T, K, P> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users select a slot's parts by drawing a rectangle on the surface
/// around them (`createLasso`). Make it in the component's constructor and
/// keep it in a field. Measure the surface with
/// [`surface`](Lasso::surface), render [`listener`](Lasso::listener) on
/// every frame and the rectangle part through [`style`](Lasso::style),
/// which hides it while none is drawn, and start it from the surface's
/// presses, through `on_pointer_down`. Items that handle their own presses
/// stop them with `cx.stop_propagation()`, so only presses on empty space
/// reach the surface, the original's `event.target === el`.
pub fn create_lasso<T: 'static, K: Clone + Eq + Hash + 'static, P: Clone + 'static>(
    options: LassoOptions<T, K, P>,
    cx: &Context<T>,
) -> Lasso<T, K, P> {
    Lasso {
        inner: Rc::new(Inner {
            options: RefCell::new(options),
            draft: RefCell::new(None),
            gesture: RefCell::new(None),
            surface: Measured::default(),
            drags: DragSession::new(cx),
        }),
    }
}

impl<T: 'static, K: Clone + Eq + Hash + 'static, P: Clone + 'static> Lasso<T, K, P> {
    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: LassoOptions<T, K, P>) {
        *self.inner.options.borrow_mut() = options;
    }

    /// Measures the surface the rectangle is drawn on, which its coordinates
    /// are relative to.
    pub fn surface<E: ParentElement + Styled>(&self, el: E) -> E {
        self.inner.surface.measure(el)
    }

    /// The element that follows the pointer while a rectangle is drawn.
    /// Render it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T, K, P> {
        self.inner.drags.listener()
    }

    /// The rectangle being drawn, if one is.
    pub fn draft(&self) -> Option<LassoDraft<K>> {
        self.inner.draft.borrow().clone()
    }

    /// Whether the rectangle touches an item now, for a highlight. With
    /// Shift, that includes the items selected before the drag.
    pub fn touches(&self, key: &K) -> bool {
        self.inner
            .draft
            .borrow()
            .as_ref()
            .is_some_and(|draft| draft.keys.contains(key))
    }

    /// Places the rectangle's part where the rectangle is, inside the
    /// surface, or hides it while none is drawn (`style`).
    pub fn style<E: Styled>(&self, el: E) -> E {
        match self.inner.draft.borrow().as_ref() {
            Some(draft) => el
                .absolute()
                .left(draft.left)
                .top(draft.top)
                .w(draft.right - draft.left)
                .h(draft.bottom - draft.top),
            None => el.hidden(),
        }
    }

    /// Starts a rectangle on the surface once the pointer moves past
    /// [`DRAG_THRESHOLD`] (`pointerdown`). Escape lets go without selecting.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drags = self.inner.drags.clone();
        if !drags.accepts(this, event, window, cx) {
            return;
        }
        let options = self.inner.options.borrow().clone();
        let origin = self.inner.surface.get().map_or(Point::default(), |surface| surface.origin);
        let base = if event.modifiers.shift {
            (options.selected)(this)
        } else {
            Vec::new()
        };
        let items = options
            .parts
            .keys()
            .into_iter()
            .filter(|key| options.selectable.as_ref().is_none_or(|selectable| selectable(this, key)))
            .filter_map(|key| {
                let rect = options.parts.bounds(&key)?;
                Some((key, Bounds::new(rect.origin - origin, rect.size)))
            })
            .collect();
        *self.inner.gesture.borrow_mut() = Some(Gesture {
            origin,
            start: event.position - origin,
            base,
            items,
        });
        let moved = Rc::downgrade(&self.inner);
        let ended = moved.clone();
        drags.start(
            event,
            BindDragOptions::new()
                .threshold(DRAG_THRESHOLD)
                .cancel_on_escape(true)
                .on_move(move |_this: &mut T, sample, _window, cx| {
                    if let Some(inner) = moved.upgrade() {
                        Self::drew(&inner, sample.position());
                        cx.notify();
                    }
                })
                .on_end(move |this: &mut T, end, canceled, window, cx| {
                    if let Some(inner) = ended.upgrade() {
                        Self::lifted(&inner, this, end, canceled, window, cx);
                    }
                }),
            window,
            cx,
        );
    }

    fn drew(inner: &Rc<Inner<T, K, P>>, at: Point<Pixels>) {
        let next = {
            let gesture = inner.gesture.borrow();
            let Some(gesture) = gesture.as_ref() else {
                return;
            };
            let end = at - gesture.origin;
            let rect = Bounds::from_corners(
                point(gesture.start.x.min(end.x), gesture.start.y.min(end.y)),
                point(gesture.start.x.max(end.x), gesture.start.y.max(end.y)),
            );
            let mut keys = gesture.base.clone();
            for (key, item) in &gesture.items {
                if touches(&rect, item) && !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
            LassoDraft {
                left: rect.left(),
                top: rect.top(),
                right: rect.right(),
                bottom: rect.bottom(),
                keys,
            }
        };
        *inner.draft.borrow_mut() = Some(next);
    }

    fn lifted(
        inner: &Rc<Inner<T, K, P>>,
        this: &mut T,
        end: &PointerEvent,
        canceled: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        inner.gesture.borrow_mut().take();
        let draft = inner.draft.borrow_mut().take();
        cx.notify();
        let Some(draft) = draft else {
            return;
        };
        if canceled {
            return;
        }
        let options = inner.options.borrow().clone();
        let count = draft.keys.len();
        let details = LassoDetails {
            event: Some(end.event.clone()),
        };
        if (options.set)(this, draft.keys, details, window, cx) {
            let text = match &options.message {
                Some(message) => message(count),
                None => format!("{count} {} selected.", if count == 1 { "item" } else { "items" }),
            };
            announce(&text, window, cx);
        }
    }
}
