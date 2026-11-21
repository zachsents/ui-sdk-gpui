//! Port of `foundation/drag-and-drop/reorder.ts`: reordering a list or a
//! grid: an item picked up with the keyboard or dragged with a pointer moves
//! through a draft order, which renders as it moves, and only the drop
//! commits it through the component's change. A group, like the selection,
//! moves as one block. No WAI-ARIA pattern covers it, so the keys follow what
//! sortable lists commonly use: Space or Enter picks an item up, arrows,
//! Home and End move it, Space or Enter drops it, and Escape puts it back.
//!
//! The behavior keeps the draft (the original's `draft` state key) and reads
//! the order through `value` (its `key`). Items join as they render with
//! [`Reorder::bind`], the original's slot `bind`, and record their bounds
//! with [`Reorder::measure`]. While a pointer drags an item,
//! [`Reorder::follow`] moves it so it stays under the pointer as the list
//! reorders around it, which the original's `--drag-x` and `--drag-y` do;
//! with `preview`, [`Reorder::preview`] is the copy to render instead.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt::Display;
use std::hash::Hash;
use std::rc::{Rc, Weak};

use gpui::{
    AnyElement, App, Bounds, Context, Element, ElementId, FocusHandle, GlobalElementId,
    InspectorElementId, IntoElement, KeyDownEvent, LayoutId, ParentElement, Pixels, Point,
    SharedString, Styled, Window, px,
};
use ui_sdk_focus::{Arrows, Step, move_index, step_for};
use ui_sdk_kernel::{ChangeOptions, InputEvent, Key, Reason};
use ui_sdk_platform::{AnnouncePriority, announce_with, is_rtl};
use ui_sdk_text::label_of;

use crate::auto_scroll::{AutoScroll, AutoScrollOptions, create_auto_scroll};
use crate::drag_preview::{DragPreview, bind_drag_preview};
use crate::draft_move::{self, DraftMove, DraftOps, Items, MessageType, MoveDraft};
use crate::pointer::{BindDragOptions, DRAG_THRESHOLD, DragSession, PointerEvent, PointerSample};

/// Whether two key lists hold the same keys in the same order, so an
/// unchanged selection fires no change (`isSameKeys`, from selection's
/// `listbox-range.ts`). It lives here since selection builds on drag and
/// drop; `ui_sdk_selection::listbox_range` re-exports it.
pub fn is_same_keys<K: PartialEq>(a: &[K], b: &[K]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a == b)
}

/// The item being moved and the order it's in now (`ReorderDraft`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReorderDraft<K> {
    /// The key of the item picked up.
    pub key: K,
    /// Every key moving together, in list order, the picked-up item included.
    pub keys: Vec<K>,
    /// Every key in the order the list shows now, the moving items included.
    pub order: Vec<K>,
    /// Whether a pointer is dragging it, rather than the keyboard moving it.
    pub pointer: bool,
}

impl<K: Clone + PartialEq + 'static> MoveDraft for ReorderDraft<K> {
    type Key = K;

    fn key(&self) -> &K {
        &self.key
    }

    fn pointer(&self) -> bool {
        self.pointer
    }
}

/// What happened, for the message screen readers hear (`ReorderMessage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReorderMessage<K> {
    /// `Grab` when an item is picked up, `Move` for each step, `Drop` when it
    /// lands, `Cancel` when it goes back.
    pub r#type: MessageType,
    /// The item's key.
    pub key: K,
    /// What the item is called, from `label`, else its text.
    pub label: SharedString,
    /// How many items move together.
    pub count: usize,
    /// Its position now, from 1.
    pub position: usize,
    /// How many items the list has.
    pub total: usize,
}

/// Why an order changed (`ReorderReason`): a pointer drag (`Drag`), a
/// keyboard move (`MoveKey`), or a call from code (`Imperative`).
pub type ReorderReason = Reason;

/// How the items of a reorderable list run (`ReorderOrientation`): along a
/// line, or wrapping in rows like a grid of photos.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ReorderOrientation {
    Horizontal,
    #[default]
    Vertical,
    Grid,
}

impl ReorderOrientation {
    /// The original's name for it, like `"grid"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ReorderOrientation::Horizontal => "horizontal",
            ReorderOrientation::Vertical => "vertical",
            ReorderOrientation::Grid => "grid",
        }
    }
}

/// The English messages [`create_reorder`] announces unless you pass
/// `message` (`reorderMessage`).
pub fn reorder_message<K>(message: &ReorderMessage<K>) -> String {
    let at = format!("position {} of {}", message.position, message.total);
    let label = if message.count > 1 {
        format!("{} items", message.count)
    } else {
        message.label.to_string()
    };
    match message.r#type {
        MessageType::Grab => format!("Picked up {label}, {at}."),
        MessageType::Move => format!("{label}, {at}."),
        MessageType::Drop => format!("Dropped {label} at {at}."),
        MessageType::Cancel => format!("Moved {label} back to {at}."),
    }
}

/// The index among the other items a dragged item goes to: after every item
/// whose middle the pointer has passed (`dropIndex`). `reversed` reads the
/// middles right to left. Pure, so it needs no layout.
pub fn drop_index(middles: &[Pixels], pointer: Pixels, reversed: bool) -> usize {
    middles
        .iter()
        .filter(|middle| {
            if reversed {
                **middle > pointer
            } else {
                **middle < pointer
            }
        })
        .count()
}

/// The index among the other items a dragged item goes to in a grid that
/// wraps in rows: after every item on a row above the pointer, and every
/// item on the pointer's row whose middle it has passed (`gridDropIndex`).
pub fn grid_drop_index(rects: &[Bounds<Pixels>], point: Point<Pixels>, reversed: bool) -> usize {
    rects
        .iter()
        .filter(|rect| {
            let middle = (rect.left() + rect.right()) / 2.;
            let passed = if reversed {
                point.x < middle
            } else {
                point.x > middle
            };
            point.y > rect.bottom() || (point.y >= rect.top() && passed)
        })
        .count()
}

/// How many items the first row of a grid holds, from their tops
/// (`columnsOf`).
pub fn columns_of(tops: &[Pixels]) -> usize {
    let mut count = 0;
    while count < tops.len() && (tops[count] - tops[0]).abs() < px(1.) {
        count += 1;
    }
    count.max(1)
}

/// The order with every key of `keys` moved to `index` among the rest, as
/// one block in list order (`movedTo`). The same order when it's already
/// there.
pub fn moved_to<K: Clone + PartialEq>(order: &[K], keys: &[K], index: usize) -> Vec<K> {
    let keys: Vec<K> = order.iter().filter(|item| keys.contains(item)).cloned().collect();
    placed(order, keys, index)
}

/// The order with `key` moved to `index` among the rest (`movedTo` with one
/// key).
pub fn moved_key_to<K: Clone + PartialEq>(order: &[K], key: &K, index: usize) -> Vec<K> {
    placed(order, vec![key.clone()], index)
}

fn placed<K: Clone + PartialEq>(order: &[K], keys: Vec<K>, index: usize) -> Vec<K> {
    let mut rest: Vec<K> = order.iter().filter(|item| !keys.contains(item)).cloned().collect();
    let to = index.min(rest.len());
    if keys.iter().enumerate().all(|(i, item)| order.get(to + i) == Some(item)) {
        return order.to_vec();
    }
    rest.splice(to..to, keys);
    rest
}

// The order with `keys` gathered into one block where `key` is.
fn gathered<K: Clone + PartialEq>(order: &[K], key: &K, keys: &[K]) -> Vec<K> {
    let at = order.iter().position(|item| item == key).unwrap_or(order.len());
    let before = order[..at].iter().filter(|item| !keys.contains(item)).count();
    if keys.len() > 1 {
        moved_to(order, keys, before)
    } else {
        order.to_vec()
    }
}

/// What an item tells [`Reorder::bind`] about itself as it renders.
#[derive(Clone, Debug, Default)]
pub struct ReorderItem {
    /// What the item says, for the message screen readers hear when no
    /// `label` names it (the original reads the element's text).
    pub text: Option<SharedString>,
    /// The focus handle its element tracks, which a keyboard move keeps
    /// focus on, or on the handle inside it that had focus.
    pub focus: Option<FocusHandle>,
}

impl ReorderItem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn focus(mut self, handle: &FocusHandle) -> Self {
        self.focus = Some(handle.clone());
        self
    }
}

/// Reads a value from the component.
pub type ReadFn<T, V> = Rc<dyn Fn(&T) -> V>;

/// Asks the component for a new order, with why, and returns whether it
/// went through. It runs inside the component's update, with the component.
pub type SetOrderFn<T, K> =
    Rc<dyn Fn(&mut T, Vec<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool>;

/// Reads something about a key from the component.
pub type KeyFn<T, K, V> = Rc<dyn Fn(&T, &K) -> V>;

/// The text screen readers hear for a step.
pub type MessageFn<M> = Rc<dyn Fn(&M) -> String>;

/// How [`create_reorder`] reads, renders and changes an order
/// (`ReorderOptions`). `T` is the component, `K` an item's key.
pub struct ReorderOptions<T: 'static, K> {
    /// The order, the component's value of item keys (`key`).
    pub value: ReadFn<T, Vec<K>>,
    /// Asks for a new order and returns whether it went through. Pass the
    /// component's change, so `on_*_change`, `cancel()` and controlled
    /// values work: `|this, value, details, window, cx|
    /// this.value.change(&this.instance, value, details, window, cx)`.
    /// `details.reason` is `Drag`, `MoveKey` or `Imperative`.
    pub set: SetOrderFn<T, K>,
    /// Whether the items run top to bottom, along a line, or wrap in rows
    /// like a grid of photos, which picks the arrows and how a drag finds
    /// its place. Defaults to `Vertical`.
    pub orientation: Option<ReadFn<T, ReorderOrientation>>,
    /// The keys that move with the one picked up, like the selection when it
    /// holds it. Defaults to that one key.
    pub group: Option<KeyFn<T, K, Vec<K>>>,
    /// Drags a copy of the item above the page while the item itself shows
    /// where it would drop, so a list that scrolls neither clips it nor
    /// grows under it. A drag near a scroller's edge scrolls it. Defaults to
    /// false.
    pub preview: bool,
    /// How a drag with `preview` near a scroller's edge scrolls it, with the
    /// scrollers it may scroll, or `None` not to (`createAutoScroll: false`).
    /// Its `on_scroll` is the reorder's own.
    pub auto_scroll: Option<AutoScrollOptions>,
    /// Whether an item can't be picked up. It still moves aside for others.
    pub disabled: Option<KeyFn<T, K, bool>>,
    /// The name screen readers hear for an item. Return `None` for the
    /// default, its text.
    pub label: Option<KeyFn<T, K, Option<SharedString>>>,
    /// The text screen readers hear for each step. Defaults to English
    /// ([`reorder_message`]).
    pub message: Option<MessageFn<ReorderMessage<K>>>,
}

impl<T: 'static, K> Clone for ReorderOptions<T, K> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            set: self.set.clone(),
            orientation: self.orientation.clone(),
            group: self.group.clone(),
            preview: self.preview,
            auto_scroll: self.auto_scroll.clone(),
            disabled: self.disabled.clone(),
            label: self.label.clone(),
            message: self.message.clone(),
        }
    }
}

impl<T: 'static, K: 'static> ReorderOptions<T, K> {
    /// Options with the order and how to change it.
    pub fn new(
        value: impl Fn(&T) -> Vec<K> + 'static,
        set: impl Fn(&mut T, Vec<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        Self {
            value: Rc::new(value),
            set: Rc::new(set),
            orientation: None,
            group: None,
            preview: false,
            auto_scroll: Some(AutoScrollOptions::default()),
            disabled: None,
            label: None,
            message: None,
        }
    }

    pub fn orientation(mut self, orientation: impl Fn(&T) -> ReorderOrientation + 'static) -> Self {
        self.orientation = Some(Rc::new(orientation));
        self
    }

    pub fn group(mut self, group: impl Fn(&T, &K) -> Vec<K> + 'static) -> Self {
        self.group = Some(Rc::new(group));
        self
    }

    pub fn preview(mut self, preview: bool) -> Self {
        self.preview = preview;
        self
    }

    pub fn auto_scroll(mut self, auto_scroll: Option<AutoScrollOptions>) -> Self {
        self.auto_scroll = auto_scroll;
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

    pub fn message(mut self, message: impl Fn(&ReorderMessage<K>) -> String + 'static) -> Self {
        self.message = Some(Rc::new(message));
        self
    }
}

// How a pointer drag shows the item: moving the item itself, or a copy.
enum Shown {
    Follow,
    Float {
        preview: DragPreview,
        count: usize,
        scroller: Option<AutoScroll>,
    },
}

// The pointer drag in progress.
struct PointerDrag<K> {
    key: K,
    // Where the press was, and where the item was laid out then.
    start: Point<Pixels>,
    origin: Point<Pixels>,
    at: Point<Pixels>,
    // The move that placed the item last, which the drop reports.
    last_move: Option<PointerEvent>,
    // Set once the drag started past the threshold and picked the item up.
    shown: Option<Shown>,
}

type PointerCell<K> = Rc<RefCell<Option<PointerDrag<K>>>>;

struct Inner<T: 'static, K: Clone + PartialEq + 'static> {
    options: RefCell<ReorderOptions<T, K>>,
    moves: RefCell<DraftMove<ReorderDraft<K>, Vec<K>>>,
    items: Items<K, ReorderItem>,
    // The pointer drag in progress, which the component ends if it's
    // destroyed mid-drag.
    drags: DragSession<T>,
    pointer: PointerCell<K>,
}

/// A copy of the item a pointer drags, for the component to render
/// (`preview: true`): which item, how many move with it, and where the copy
/// is.
#[derive(Clone, Debug)]
pub struct ReorderPreview<K> {
    pub key: K,
    /// How many items move together, the original's `data-drag-count` and
    /// `--drag-count` when more than one.
    pub count: usize,
    pub preview: DragPreview,
}

impl<K> ReorderPreview<K> {
    /// The copy: `content` where the preview is now. Render it without the
    /// item's moving or dragging state, since the copy is the item in
    /// flight, not the place it would drop.
    pub fn render<E: IntoElement>(&self, content: E) -> impl IntoElement + use<E, K> {
        self.preview.render(content)
    }
}

/// Lets users reorder a slot's parts by dragging them and with the keyboard
/// (`createReorder`'s return). Cloning it gives another handle to the same
/// reorder.
pub struct Reorder<T: 'static, K: Clone + PartialEq + 'static> {
    inner: Rc<Inner<T, K>>,
}

impl<T: 'static, K: Clone + PartialEq + 'static> Clone for Reorder<T, K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users reorder a slot's parts by dragging them and with the keyboard
/// (`createReorder`). The moving item and its order live in the reorder, so
/// the list renders [`order`](Reorder::order) as it changes; only dropping
/// commits it through `set`, and screen readers hear each step through
/// `announce`. Make it in the component's constructor, keep it in a field,
/// render the items in `reorder.order(this)`, render
/// [`listener`](Reorder::listener) on every frame, and wire the item part:
///
/// ```ignore
/// let reorder = this.reorder.clone();
/// reorder.bind(key.clone(), ReorderItem::new().text(label).focus(&handle), cx);
/// let part = reorder.measure(key.clone(), part)
///     .on_keys(&[..], move |event, window, cx| {
///         entity.update(cx, |this, cx| {
///             let reorder = this.reorder.clone();
///             if !reorder.keydown(this, event, &key, window, cx) {
///                 this.roving.keydown(event, &key, window, cx);
///             }
///         }).ok();
///     });
/// let part = on_pointer_down(part, move |event, window, cx| {
///     entity.update(cx, |this, cx| this.reorder.clone().pointer_down(this, event, &key, window, cx)).ok();
/// });
/// reorder.follow(key, part)
/// ```
///
/// Wire the pointer on a handle part instead, with the item's key, to drag
/// by a handle.
pub fn create_reorder<T: 'static, K: Clone + Eq + Hash + Display + 'static>(
    options: ReorderOptions<T, K>,
    cx: &Context<T>,
) -> Reorder<T, K> {
    Reorder {
        inner: Rc::new(Inner {
            options: RefCell::new(options),
            moves: RefCell::new(DraftMove::default()),
            items: Items::new(),
            drags: DragSession::new(cx),
            pointer: Rc::new(RefCell::new(None)),
        }),
    }
}

impl<T: 'static, K: Clone + Eq + Hash + Display + 'static> DraftOps<T> for Reorder<T, K> {
    type Draft = ReorderDraft<K>;
    type Value = Vec<K>;

    fn moves(&self) -> &RefCell<DraftMove<ReorderDraft<K>, Vec<K>>> {
        &self.inner.moves
    }

    fn value(&self, this: &T) -> Vec<K> {
        let value = self.inner.options.borrow().value.clone();
        value(this)
    }

    fn shown(&self, draft: &ReorderDraft<K>) -> Vec<K> {
        draft.order.clone()
    }

    fn same(&self, a: &Vec<K>, b: &Vec<K>) -> bool {
        is_same_keys(a, b)
    }

    fn rebase(
        &self,
        _this: &T,
        current: &ReorderDraft<K>,
        now: &Vec<K>,
        moved: bool,
    ) -> Option<ReorderDraft<K>> {
        if !now.contains(&current.key) {
            return None;
        }
        let keys: Vec<K> = now.iter().filter(|key| current.keys.contains(key)).cloned().collect();
        let order = gathered(now, &current.key, &keys);
        if !moved {
            return Some(ReorderDraft {
                keys,
                order,
                ..current.clone()
            });
        }
        let at = current
            .keys
            .first()
            .and_then(|first| current.order.iter().position(|key| key == first))
            .unwrap_or(0);
        let index = current.order[..at].iter().filter(|key| now.contains(key)).count();
        let order = moved_to(&order, &keys, index);
        Some(ReorderDraft {
            keys,
            order,
            ..current.clone()
        })
    }

    fn commit(
        &self,
        this: &mut T,
        current: &ReorderDraft<K>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let set = self.inner.options.borrow().set.clone();
        set(this, current.order.clone(), options, window, cx)
    }

    fn say(
        &self,
        this: &T,
        r#type: MessageType,
        current: &ReorderDraft<K>,
        order: &Vec<K>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.say_in(this, r#type, &current.key, order, current.keys.len(), window, cx);
    }

    fn element(&self, key: &K) -> Option<FocusHandle> {
        self.inner.items.get(key).and_then(|item| item.focus)
    }
}

impl<T: 'static, K: Clone + Eq + Hash + Display + 'static> Reorder<T, K> {
    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: ReorderOptions<T, K>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn options(&self) -> ReorderOptions<T, K> {
        self.inner.options.borrow().clone()
    }

    /// Starts a render of the items, from the part that holds them, every
    /// frame it renders. Binding an item starts one too; this is what tells
    /// a list that renders no items that they've all left.
    pub fn begin(&self, cx: &App) {
        self.inner.items.begin(cx);
    }

    /// Joins an item to the list as it renders (`bind`). An item that stops
    /// rendering leaves.
    pub fn bind(&self, key: K, item: ReorderItem, cx: &App) {
        self.inner.items.bind(key, item, cx);
    }

    /// Adds a layer to an item's element that records its bounds each
    /// layout, which a drag finds its place by.
    pub fn measure<E: ParentElement + Styled>(&self, key: K, el: E) -> E {
        self.inner.items.measure(key, el)
    }

    /// The item's element, moved while a pointer drags it without `preview`
    /// so it stays under the pointer as the list reorders around it, the
    /// original's `--drag-x` and `--drag-y`. Its layout doesn't move, as a
    /// CSS translate doesn't: the move is taken from where it's laid out now.
    pub fn follow<E: IntoElement>(&self, key: K, el: E) -> impl IntoElement + use<T, K, E> {
        Follow {
            key,
            pointer: self.inner.pointer.clone(),
            child: el.into_any_element(),
        }
    }

    /// The element that follows the pointer while an item is dragged. Render
    /// it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T, K> {
        self.inner.drags.listener()
    }

    /// The copy of the dragged item to render, while a pointer drags one with
    /// `preview`.
    pub fn preview(&self) -> Option<ReorderPreview<K>> {
        let pointer = self.inner.pointer.borrow();
        let drag = pointer.as_ref()?;
        match &drag.shown {
            Some(Shown::Float { preview, count, .. }) => Some(ReorderPreview {
                key: drag.key.clone(),
                count: *count,
                preview: *preview,
            }),
            _ => None,
        }
    }

    /// The moving item and its order, while one is picked up.
    pub fn draft(&self, this: &T) -> Option<ReorderDraft<K>> {
        draft_move::current(self, this)
    }

    /// The order to render: the draft's while an item moves, else the value.
    pub fn order(&self, this: &T) -> Vec<K> {
        match self.draft(this) {
            Some(draft) => draft.order,
            None => self.value(this),
        }
    }

    /// Whether an item is picked up, by itself or with its group.
    pub fn moving(&self, this: &T, key: &K) -> bool {
        self.draft(this).is_some_and(|draft| draft.keys.contains(key))
    }

    /// Whether a pointer is dragging the item, by itself or with its group.
    pub fn dragging(&self, this: &T, key: &K) -> bool {
        self.draft(this)
            .is_some_and(|draft| draft.pointer && draft.keys.contains(key))
    }

    fn orientation_of(&self, this: &T) -> ReorderOrientation {
        let orientation = self.inner.options.borrow().orientation.clone();
        orientation.map(|orientation| orientation(this)).unwrap_or_default()
    }

    #[allow(clippy::too_many_arguments)]
    fn say_in(
        &self,
        this: &T,
        r#type: MessageType,
        key: &K,
        order: &[K],
        count: usize,
        window: &mut Window,
        cx: &mut App,
    ) {
        let options = self.options();
        let label = options
            .label
            .as_ref()
            .and_then(|label| label(this, key))
            .or_else(|| {
                self.inner
                    .items
                    .get(key)
                    .and_then(|item| item.text)
                    .map(|text| label_of(&text, None).into())
            })
            .unwrap_or_else(|| key.to_string().into());
        let message = ReorderMessage {
            r#type,
            key: key.clone(),
            label,
            count,
            position: order.iter().position(|item| item == key).map_or(0, |at| at + 1),
            total: order.len(),
        };
        let text = match &options.message {
            Some(message_fn) => message_fn(&message),
            None => reorder_message(&message),
        };
        announce_with(&text, AnnouncePriority::Assertive, window, cx);
    }

    fn is_disabled(&self, this: &T, key: &K) -> bool {
        if !self.inner.items.contains(key) {
            return false;
        }
        let disabled = self.inner.options.borrow().disabled.clone();
        disabled.is_some_and(|disabled| disabled(this, key))
    }

    // Picks up an item and its group, gathered into one block where the item is.
    fn grab_with(
        &self,
        this: &T,
        key: &K,
        pointer: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let current = self.value(this);
        if self.draft(this).is_some() || !current.contains(key) || self.is_disabled(this, key) {
            return false;
        }
        let group_fn = self.inner.options.borrow().group.clone();
        let group: HashSet<K> = group_fn
            .map(|group| group(this, key))
            .unwrap_or_default()
            .into_iter()
            .collect();
        let keys: Vec<K> = current
            .iter()
            .filter(|item| *item == key || (group.contains(*item) && !self.is_disabled(this, item)))
            .cloned()
            .collect();
        let order = gathered(&current, key, &keys);
        let count = keys.len();
        draft_move::set_draft(
            self,
            this,
            ReorderDraft {
                key: key.clone(),
                keys,
                order: order.clone(),
                pointer,
            },
            window,
            cx,
        );
        self.say_in(this, MessageType::Grab, key, &order, count, window, cx);
        if !pointer && count > 1 {
            draft_move::focus_later(self, key, window, cx);
        }
        true
    }

    /// Picks an item up, as Space does. Returns whether it did.
    pub fn grab(&self, this: &mut T, key: &K, window: &mut Window, cx: &mut Context<T>) -> bool {
        let grabbed = self.grab_with(this, key, false, window, cx);
        if grabbed {
            cx.notify();
        }
        grabbed
    }

    /// Moves the picked-up item to `index` in the draft, counting from 0
    /// among the items left after taking out the moving ones.
    pub fn move_to(&self, this: &mut T, index: usize, window: &mut Window, cx: &mut Context<T>) {
        self.move_to_in(this, index, window, cx);
    }

    fn move_to_in(&self, this: &T, index: usize, window: &mut Window, cx: &mut Context<T>) {
        let Some(current) = self.draft(this) else {
            return;
        };
        let order = moved_to(&current.order, &current.keys, index);
        if is_same_keys(&order, &current.order) {
            return;
        }
        let count = current.keys.len();
        let key = current.key.clone();
        let pointer = current.pointer;
        draft_move::set_draft(
            self,
            this,
            ReorderDraft {
                order: order.clone(),
                ..current
            },
            window,
            cx,
        );
        self.say_in(this, MessageType::Move, &key, &order, count, window, cx);
        if !pointer {
            draft_move::focus_later(self, &key, window, cx);
        }
        cx.notify();
    }

    /// Drops the picked-up item where it is, committing the draft, with the
    /// event behind it (`drop`). Reports `MoveKey`.
    pub fn drop(
        &self,
        this: &mut T,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        draft_move::drop(self, this, Reason::MoveKey, event, window, cx);
    }

    /// Puts the picked-up item back where it was.
    pub fn cancel(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        draft_move::cancel(self, this, true, window, cx);
    }

    /// Moves an item to `index` at once, like a "Move to top" menu item.
    /// Reports `Imperative`. Returns whether it went through.
    pub fn r#move(
        &self,
        this: &mut T,
        key: &K,
        index: usize,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let order = self.value(this);
        if !order.contains(key) {
            return false;
        }
        let next = moved_key_to(&order, key, index);
        if is_same_keys(&next, &order) {
            return false;
        }
        let set = self.inner.options.borrow().set.clone();
        if !set(this, next.clone(), ChangeOptions::reason(Reason::Imperative), window, cx) {
            return false;
        }
        self.say_in(this, MessageType::Drop, key, &next, 1, window, cx);
        cx.notify();
        true
    }

    // How far a key moves the picked-up block: along the line, or in a grid,
    // Left and Right by one and Up and Down by a row.
    fn step_of(&self, this: &T, key: &Key, rtl: bool) -> Option<Step> {
        match self.orientation_of(this) {
            ReorderOrientation::Vertical => step_for(key, Arrows::Vertical, rtl),
            ReorderOrientation::Horizontal => step_for(key, Arrows::Horizontal, rtl),
            ReorderOrientation::Grid => {
                if *key != Key::ArrowUp && *key != Key::ArrowDown {
                    return step_for(key, Arrows::Horizontal, rtl);
                }
                let tops: Vec<Pixels> = self
                    .inner
                    .items
                    .keys()
                    .iter()
                    .map(|item| self.inner.items.bounds(item).map_or(Pixels::ZERO, |b| b.top()))
                    .collect();
                let columns = columns_of(&tops) as isize;
                Some(Step::By(if *key == Key::ArrowUp { -columns } else { columns }))
            }
        }
    }

    /// Handles a key pressed on an item and returns whether it did. Space or
    /// Enter picks the item up; while it's up, arrows, Home and End move it,
    /// Space or Enter drops it, Escape puts it back and Tab puts it back and
    /// moves on. Every other key is swallowed while an item is up, so pass
    /// keys on only when this returns false. A key it handles calls
    /// `window.prevent_default()`; Escape stops propagating too, since a
    /// popup around the list would close on the same press.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let current = self.draft(this);
        if window.default_prevented() || current.as_ref().is_some_and(|draft| draft.pointer) {
            return current.is_some();
        }
        let modifiers = event.keystroke.modifiers;
        let modified = modifiers.alt || modifiers.control || modifiers.platform;
        let pressed = Key::of(&event.keystroke);
        let Some(current) = current else {
            if modified || (pressed != Key::Space && pressed != Key::Enter) {
                return false;
            }
            if !self.grab_with(this, key, false, window, cx) {
                return false;
            }
            window.prevent_default();
            cx.notify();
            return true;
        };
        if pressed == Key::Tab {
            draft_move::cancel(self, this, false, window, cx);
            return true;
        }
        let step = if modified {
            None
        } else {
            self.step_of(this, &pressed, is_rtl(cx))
        };
        if let Some(step) = step {
            window.prevent_default();
            // The block's place among the other items, and how many places
            // there are.
            let index = current
                .keys
                .first()
                .and_then(|first| current.order.iter().position(|item| item == first))
                .unwrap_or(0) as isize;
            let count = current.order.len() - current.keys.len() + 1;
            // A row up or down from the first or last row stays put.
            let off = matches!(step, Step::By(by) if by.abs() > 1 && (index + by < 0 || index + by >= count as isize));
            if !off {
                self.move_to_in(this, move_index(index, step, count, false), window, cx);
            }
        } else if pressed == Key::Space || pressed == Key::Enter {
            window.prevent_default();
            draft_move::drop(
                self,
                this,
                Reason::MoveKey,
                Some(InputEvent::from(event)),
                window,
                cx,
            );
        } else if pressed == Key::Escape {
            window.prevent_default();
            cx.stop_propagation();
            draft_move::cancel(self, this, true, window, cx);
        }
        true
    }

    // Where the moving block goes for a pointer at a point, among the other
    // items.
    fn index_at(&self, this: &T, point: Point<Pixels>, cx: &App) -> usize {
        let moving = self.draft(this).map(|draft| draft.keys).unwrap_or_default();
        let rects: Vec<Bounds<Pixels>> = self
            .inner
            .items
            .keys()
            .iter()
            .filter(|key| !moving.contains(key))
            .map(|key| self.inner.items.bounds(key).unwrap_or_default())
            .collect();
        let kind = self.orientation_of(this);
        let reversed = kind != ReorderOrientation::Vertical && !rects.is_empty() && is_rtl(cx);
        if kind == ReorderOrientation::Grid {
            return grid_drop_index(&rects, point, reversed);
        }
        let vertical = kind == ReorderOrientation::Vertical;
        let middles: Vec<Pixels> = rects
            .iter()
            .map(|rect| {
                if vertical {
                    rect.top() + rect.size.height / 2.
                } else {
                    rect.left() + rect.size.width / 2.
                }
            })
            .collect();
        drop_index(&middles, if vertical { point.y } else { point.x }, reversed)
    }

    /// Starts a pointer drag of an item once the pointer moves a few pixels,
    /// so a click stays a click (`pointerdown`). Escape puts it back.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drags = self.inner.drags.clone();
        if !self.inner.items.contains(key)
            || !drags.accepts(this, event, window, cx)
            || self.draft(this).is_some()
        {
            return;
        }
        let origin = self
            .inner
            .items
            .bounds(key)
            .map_or(event.position, |bounds| bounds.origin);
        *self.inner.pointer.borrow_mut() = Some(PointerDrag {
            key: key.clone(),
            start: event.position,
            origin,
            at: event.position,
            last_move: None,
            shown: None,
        });
        let weak = Rc::downgrade(&self.inner);
        let started = weak.clone();
        let moved = weak.clone();
        let ended = weak;
        drags.start(
            event,
            BindDragOptions::new()
                .threshold(DRAG_THRESHOLD)
                .cancel_on_escape(true)
                .on_start(move |this: &mut T, _event, window, cx| {
                    Self::upgrade(&started).is_some_and(|reorder| reorder.drag_started(this, window, cx))
                })
                .on_move(move |this: &mut T, sample, window, cx| {
                    if let Some(reorder) = Self::upgrade(&moved) {
                        reorder.drag_moved(this, sample, window, cx);
                    }
                })
                .on_end(move |this: &mut T, end, canceled, window, cx| {
                    if let Some(reorder) = Self::upgrade(&ended) {
                        reorder.drag_ended(this, end, canceled, window, cx);
                    }
                }),
            window,
            cx,
        );
    }

    fn upgrade(inner: &Weak<Inner<T, K>>) -> Option<Self> {
        inner.upgrade().map(|inner| Self { inner })
    }

    fn drag_started(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) -> bool {
        let Some(key) = self.inner.pointer.borrow().as_ref().map(|drag| drag.key.clone()) else {
            return false;
        };
        if !self.grab_with(this, &key, true, window, cx) {
            *self.inner.pointer.borrow_mut() = None;
            return false;
        }
        let options = self.options();
        let shown = if options.preview {
            // The copy is the item in flight, not the place it would drop.
            let (start, origin) = {
                let pointer = self.inner.pointer.borrow();
                let drag = pointer.as_ref().expect("set by pointer_down");
                (drag.start, drag.origin)
            };
            let bounds = self
                .inner
                .items
                .bounds(&key)
                .unwrap_or(Bounds::new(origin, Default::default()));
            let count = self.draft(this).map_or(1, |draft| draft.keys.len());
            let scroller = options.auto_scroll.map(|auto_scroll| {
                let reorder = Rc::downgrade(&self.inner);
                let entity = cx.weak_entity();
                create_auto_scroll(auto_scroll.on_scroll(move |window, cx| {
                    let Some(reorder) = Self::upgrade(&reorder) else {
                        return;
                    };
                    entity
                        .update(cx, |this, cx| reorder.track(this, window, cx))
                        .ok();
                }))
            });
            Shown::Float {
                preview: bind_drag_preview(bounds, start),
                count,
                scroller,
            }
        } else {
            Shown::Follow
        };
        if let Some(drag) = self.inner.pointer.borrow_mut().as_mut() {
            drag.shown = Some(shown);
        }
        cx.notify();
        true
    }

    // Puts the dragged block where the pointer is now.
    fn track(&self, this: &T, window: &mut Window, cx: &mut Context<T>) {
        let Some(at) = self.inner.pointer.borrow().as_ref().map(|drag| drag.at) else {
            return;
        };
        let index = self.index_at(this, at, cx);
        self.move_to_in(this, index, window, cx);
    }

    fn drag_moved(
        &self,
        this: &mut T,
        sample: &PointerSample,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let scroller = {
            let mut pointer = self.inner.pointer.borrow_mut();
            let Some(drag) = pointer.as_mut() else {
                return;
            };
            drag.last_move = Some(sample.event.clone());
            drag.at = sample.position();
            match &mut drag.shown {
                Some(Shown::Float {
                    preview, scroller, ..
                }) => {
                    preview.r#move(sample.x, sample.y);
                    scroller.clone()
                }
                _ => None,
            }
        };
        if let Some(scroller) = scroller {
            scroller.update(sample.x, sample.y, window);
        }
        self.track(this, window, cx);
        cx.notify();
    }

    fn drag_ended(
        &self,
        this: &mut T,
        end: &PointerEvent,
        canceled: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let Some(drag) = self.inner.pointer.borrow_mut().take() else {
            return;
        };
        let Some(shown) = drag.shown else {
            return;
        };
        if let Shown::Float {
            scroller: Some(scroller),
            ..
        } = shown
        {
            scroller.stop();
        }
        if canceled {
            draft_move::cancel(self, this, true, window, cx);
        } else {
            let event = drag.last_move.unwrap_or_else(|| end.clone()).event;
            draft_move::drop(self, this, Reason::Drag, Some(event), window, cx);
        }
        cx.notify();
    }
}

// The dragged item, laid out where the list puts it and drawn where the
// pointer keeps it, the original's translate by `--drag-x` and `--drag-y`.
struct Follow<K> {
    key: K,
    pointer: PointerCell<K>,
    child: AnyElement,
}

impl<K: PartialEq + 'static> Element for Follow<K> {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // Layout offsets ignore the translate, so it doesn't count itself.
        let offset = self
            .pointer
            .borrow()
            .as_ref()
            .filter(|drag| drag.key == self.key && matches!(drag.shown, Some(Shown::Follow)))
            .map(|drag| (drag.at - drag.start) - (bounds.origin - drag.origin))
            .unwrap_or_default();
        window.with_element_offset(offset, |window| {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
