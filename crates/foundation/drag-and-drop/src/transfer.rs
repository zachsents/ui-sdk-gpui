//! Port of `foundation/drag-and-drop/transfer.ts`: moving items between
//! lists: cards across a kanban board's columns, files between folders,
//! people between teams. It's reordering across containers, with the same
//! rules as [`create_reorder`](crate::create_reorder): the moving item and
//! every list's order live in a draft that renders as it changes, only the
//! drop commits, and screen readers hear each step. Arrows up and down move
//! within a list, left and right to the next list.
//!
//! The lists are the component's value, `(list, items)` pairs in the order
//! the original's record keeps them. Lists join as they render with
//! [`Transfer::bind_list`] and record their bounds with
//! [`Transfer::measure_list`]; items the same with
//! [`Transfer::bind_item`] and [`Transfer::measure_item`].

use std::cell::RefCell;
use std::fmt::Display;
use std::hash::Hash;
use std::rc::{Rc, Weak};

use gpui::{
    App, Bounds, Context, FocusHandle, IntoElement, KeyDownEvent, ParentElement, Pixels, Point,
    SharedString, Styled, Window,
};
use ui_sdk_kernel::{ChangeOptions, InputEvent, Key, Reason};
use ui_sdk_platform::{AnnouncePriority, announce_with, is_rtl};
use ui_sdk_text::label_of;

use crate::auto_scroll::{AutoScroll, AutoScrollOptions, create_auto_scroll};
use crate::drag_preview::{DragPreview, bind_drag_preview};
use crate::draft_move::{self, DraftMove, DraftOps, Items, MessageType, MoveDraft};
use crate::pointer::{BindDragOptions, DRAG_THRESHOLD, DragSession, PointerEvent, PointerSample};
use crate::reorder::{KeyFn, MessageFn, ReadFn, drop_index};

/// Every list's item keys, in order, by the list's key, in the order the
/// lists come in (`TransferLists`).
pub type TransferLists<K> = Vec<(K, Vec<K>)>;

/// The item being moved and where every item is now (`TransferDraft`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferDraft<K> {
    /// The key of the item being moved.
    pub key: K,
    /// The list it started in.
    pub from: K,
    /// Every list as it shows now, the moving item included.
    pub lists: TransferLists<K>,
    /// Whether a pointer is dragging it, rather than the keyboard moving it.
    pub pointer: bool,
}

impl<K: Clone + PartialEq + 'static> MoveDraft for TransferDraft<K> {
    type Key = K;

    fn key(&self) -> &K {
        &self.key
    }

    fn pointer(&self) -> bool {
        self.pointer
    }
}

/// Why the lists changed (`TransferReason`): a pointer drag (`Drag`), a
/// keyboard move (`MoveKey`), or a call from code (`Imperative`).
pub type TransferReason = Reason;

/// What happened, for the message screen readers hear (`TransferMessage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferMessage<K> {
    /// `Grab` when an item is picked up, `Move` for each step, `Drop` when it
    /// lands, `Cancel` when it goes back.
    pub r#type: MessageType,
    /// The item's key.
    pub key: K,
    /// What the item is called, from `label`, else its text.
    pub label: SharedString,
    /// What the list it's in is called, from `list_label`, else its key.
    pub list: SharedString,
    /// Its position in that list, from 1.
    pub position: usize,
    /// How many items that list has.
    pub total: usize,
}

/// What a change of the lists moved, from where to where: what `set`'s
/// details carry besides the reason and the event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferMove<K> {
    /// The item that moved.
    pub key: K,
    /// The list it was in.
    pub from: K,
    /// The list it's in now.
    pub to: K,
    /// Its place in that list, from 0.
    pub index: usize,
}

/// The English messages [`create_transfer`] announces unless you pass
/// `message` (`transferMessage`).
pub fn transfer_message<K>(message: &TransferMessage<K>) -> String {
    let at = format!(
        "{}, position {} of {}",
        message.list, message.position, message.total
    );
    let label = &message.label;
    match message.r#type {
        MessageType::Grab => format!("Picked up {label}, {at}."),
        MessageType::Move => format!("{label}, {at}."),
        MessageType::Drop => format!("Dropped {label} in {at}."),
        MessageType::Cancel => format!("Moved {label} back to {at}."),
    }
}

fn items_of<'a, K: PartialEq>(lists: &'a TransferLists<K>, list: &K) -> Option<&'a Vec<K>> {
    lists.iter().find(|(name, _)| name == list).map(|(_, keys)| keys)
}

// The lists with `key` taken out and put in `list` at `index`.
fn transferred<K: Clone + PartialEq>(
    lists: &TransferLists<K>,
    key: &K,
    list: &K,
    index: usize,
) -> TransferLists<K> {
    let mut next: TransferLists<K> = lists
        .iter()
        .map(|(name, keys)| (name.clone(), keys.iter().filter(|k| *k != key).cloned().collect()))
        .collect();
    let at = match next.iter().position(|(name, _)| name == list) {
        Some(at) => at,
        None => {
            next.push((list.clone(), Vec::new()));
            next.len() - 1
        }
    };
    let target = &mut next[at].1;
    let index = index.min(target.len());
    target.insert(index, key.clone());
    next
}

fn list_with<'a, K: PartialEq>(lists: &'a TransferLists<K>, key: &K) -> Option<&'a K> {
    lists
        .iter()
        .find(|(_, keys)| keys.contains(key))
        .map(|(name, _)| name)
}

fn same_lists<K: PartialEq>(a: &TransferLists<K>, b: &TransferLists<K>) -> bool {
    a.len() == b.len()
        && a.iter().all(|(list, keys)| {
            items_of(b, list).is_some_and(|other| {
                keys.len() == other.len() && keys.iter().zip(other).all(|(a, b)| a == b)
            })
        })
}

/// What an item tells [`Transfer::bind_item`] about itself as it renders.
#[derive(Clone, Debug, Default)]
pub struct TransferItem {
    /// What the item says, for the message screen readers hear when no
    /// `label` names it (the original reads the element's text).
    pub text: Option<SharedString>,
    /// The focus handle its element tracks, which focus follows to the
    /// item's new list.
    pub focus: Option<FocusHandle>,
}

impl TransferItem {
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

/// Asks the component for new lists, with what moved and why, and returns
/// whether they went through. It runs inside the component's update.
pub type SetListsFn<T, K> = Rc<
    dyn Fn(&mut T, TransferLists<K>, TransferMove<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool,
>;

/// Whether a list takes an item: `(this, item, list)`.
pub type AcceptsFn<T, K> = Rc<dyn Fn(&T, &K, &K) -> bool>;

/// How [`create_transfer`] reads, renders and changes the lists
/// (`TransferOptions`). `T` is the component, `K` the key of a list or an
/// item.
pub struct TransferOptions<T: 'static, K> {
    /// Every list, the component's value (`key`).
    pub value: ReadFn<T, TransferLists<K>>,
    /// Asks for the new lists and returns whether they went through. Pass
    /// the component's change. The [`TransferMove`] says what moved, from
    /// where to where; `details.reason` is `Drag`, `MoveKey` or
    /// `Imperative`.
    pub set: SetListsFn<T, K>,
    /// The lists in the order Left and Right go through them. Defaults to
    /// the order of the value's lists.
    pub order: Option<ReadFn<T, Vec<K>>>,
    /// Whether a list takes an item, like a column at its limit. Lists that
    /// don't are skipped.
    pub accepts: Option<AcceptsFn<T, K>>,
    /// Whether an item can't be picked up.
    pub disabled: Option<KeyFn<T, K, bool>>,
    /// The name screen readers hear for an item. Defaults to its text.
    pub label: Option<KeyFn<T, K, Option<SharedString>>>,
    /// The name screen readers hear for a list. Defaults to its key.
    pub list_label: Option<KeyFn<T, K, Option<SharedString>>>,
    /// The text screen readers hear for each step. Defaults to English
    /// ([`transfer_message`]).
    pub message: Option<MessageFn<TransferMessage<K>>>,
    /// How a drag near a scroller's edge scrolls it, with the scrollers it
    /// may scroll, or `None` not to (`createAutoScroll: false`).
    pub auto_scroll: Option<AutoScrollOptions>,
}

impl<T: 'static, K> Clone for TransferOptions<T, K> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            set: self.set.clone(),
            order: self.order.clone(),
            accepts: self.accepts.clone(),
            disabled: self.disabled.clone(),
            label: self.label.clone(),
            list_label: self.list_label.clone(),
            message: self.message.clone(),
            auto_scroll: self.auto_scroll.clone(),
        }
    }
}

impl<T: 'static, K: 'static> TransferOptions<T, K> {
    /// Options with the lists and how to change them.
    pub fn new(
        value: impl Fn(&T) -> TransferLists<K> + 'static,
        set: impl Fn(&mut T, TransferLists<K>, TransferMove<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool
        + 'static,
    ) -> Self {
        Self {
            value: Rc::new(value),
            set: Rc::new(set),
            order: None,
            accepts: None,
            disabled: None,
            label: None,
            list_label: None,
            message: None,
            auto_scroll: Some(AutoScrollOptions::default()),
        }
    }

    pub fn order(mut self, order: impl Fn(&T) -> Vec<K> + 'static) -> Self {
        self.order = Some(Rc::new(order));
        self
    }

    pub fn accepts(mut self, accepts: impl Fn(&T, &K, &K) -> bool + 'static) -> Self {
        self.accepts = Some(Rc::new(accepts));
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

    pub fn list_label(mut self, label: impl Fn(&T, &K) -> Option<SharedString> + 'static) -> Self {
        self.list_label = Some(Rc::new(label));
        self
    }

    pub fn message(mut self, message: impl Fn(&TransferMessage<K>) -> String + 'static) -> Self {
        self.message = Some(Rc::new(message));
        self
    }

    pub fn auto_scroll(mut self, auto_scroll: Option<AutoScrollOptions>) -> Self {
        self.auto_scroll = auto_scroll;
        self
    }
}

// The pointer drag in progress.
struct PointerDrag<K> {
    key: K,
    start: Point<Pixels>,
    at: Point<Pixels>,
    dragging: bool,
    // Whether the item had focus, which its element in another list takes over.
    focused: bool,
    last_move: Option<PointerEvent>,
    preview: Option<DragPreview>,
    scroller: Option<AutoScroll>,
}

struct Inner<T: 'static, K: Clone + PartialEq + 'static> {
    options: RefCell<TransferOptions<T, K>>,
    moves: RefCell<DraftMove<TransferDraft<K>, TransferLists<K>>>,
    items: Items<K, TransferItem>,
    lists: Items<K, ()>,
    drags: DragSession<T>,
    pointer: RefCell<Option<PointerDrag<K>>>,
}

/// The copy of the item a pointer drags, for the component to render.
#[derive(Clone, Debug)]
pub struct TransferPreview<K> {
    pub key: K,
    pub preview: DragPreview,
}

impl<K> TransferPreview<K> {
    /// The copy: `content` where the preview is now. Render it without the
    /// item's dragging state, since the copy is the item in flight, not the
    /// place it would drop.
    pub fn render<E: IntoElement>(&self, content: E) -> impl IntoElement + use<E, K> {
        self.preview.render(content)
    }
}

/// Lets users move a slot's parts between lists (`createTransfer`'s
/// return). Cloning it gives another handle to the same transfer.
pub struct Transfer<T: 'static, K: Clone + PartialEq + 'static> {
    inner: Rc<Inner<T, K>>,
}

impl<T: 'static, K: Clone + PartialEq + 'static> Clone for Transfer<T, K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users move a slot's parts between lists, by dragging them and with
/// the keyboard (`createTransfer`). Make it in the component's constructor,
/// keep it in a field, render each list's items from
/// [`lists`](Transfer::lists), render [`listener`](Transfer::listener) on
/// every frame and the [`preview`](Transfer::preview) while there is one,
/// and wire both parts: each list part calls `bind_list` and
/// `measure_list`, each item part `bind_item` and `measure_item`, runs
/// `transfer.keydown(..)` before its roving focus's, and hands its presses
/// to `transfer.pointer_down(..)` through `on_pointer_down`.
///
/// While a pointer drags, a copy of the item follows it above the page, the
/// item itself shows where it would drop (`dragging`), and a drag near a
/// scroller's edge scrolls it.
pub fn create_transfer<T: 'static, K: Clone + Eq + Hash + Display + 'static>(
    options: TransferOptions<T, K>,
    cx: &Context<T>,
) -> Transfer<T, K> {
    Transfer {
        inner: Rc::new(Inner {
            options: RefCell::new(options),
            moves: RefCell::new(DraftMove::default()),
            items: Items::new(),
            lists: Items::new(),
            drags: DragSession::new(cx),
            pointer: RefCell::new(None),
        }),
    }
}

impl<T: 'static, K: Clone + Eq + Hash + Display + 'static> DraftOps<T> for Transfer<T, K> {
    type Draft = TransferDraft<K>;
    type Value = TransferLists<K>;

    fn moves(&self) -> &RefCell<DraftMove<TransferDraft<K>, TransferLists<K>>> {
        &self.inner.moves
    }

    fn value(&self, this: &T) -> TransferLists<K> {
        let value = self.inner.options.borrow().value.clone();
        value(this)
    }

    fn shown(&self, draft: &TransferDraft<K>) -> TransferLists<K> {
        draft.lists.clone()
    }

    fn same(&self, a: &TransferLists<K>, b: &TransferLists<K>) -> bool {
        same_lists(a, b)
    }

    fn rebase(
        &self,
        this: &T,
        current: &TransferDraft<K>,
        now: &TransferLists<K>,
        moved: bool,
    ) -> Option<TransferDraft<K>> {
        let from = list_with(now, &current.key)?.clone();
        if !moved {
            return Some(TransferDraft {
                from,
                lists: now.clone(),
                ..current.clone()
            });
        }
        let list = list_with(&current.lists, &current.key)?.clone();
        let now_list = items_of(now, &list)?;
        if list != from && !self.accepted(this, &current.key, &list) {
            return None;
        }
        let keys = items_of(&current.lists, &list)?;
        let at = keys.iter().position(|key| *key == current.key).unwrap_or(0);
        let index = keys[..at].iter().filter(|key| now_list.contains(key)).count();
        Some(TransferDraft {
            from,
            lists: transferred(now, &current.key, &list, index),
            ..current.clone()
        })
    }

    fn commit(
        &self,
        this: &mut T,
        current: &TransferDraft<K>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let to = list_with(&current.lists, &current.key)
            .cloned()
            .unwrap_or_else(|| current.from.clone());
        let index = items_of(&current.lists, &to)
            .and_then(|keys| keys.iter().position(|key| *key == current.key))
            .unwrap_or(0);
        let moved = TransferMove {
            key: current.key.clone(),
            from: current.from.clone(),
            to,
            index,
        };
        let set = self.inner.options.borrow().set.clone();
        set(this, current.lists.clone(), moved, options, window, cx)
    }

    fn say(
        &self,
        this: &T,
        r#type: MessageType,
        current: &TransferDraft<K>,
        lists: &TransferLists<K>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.say_in(this, r#type, &current.key, lists, window, cx);
    }

    fn element(&self, key: &K) -> Option<FocusHandle> {
        self.inner.items.get(key).and_then(|item| item.focus)
    }
}

impl<T: 'static, K: Clone + Eq + Hash + Display + 'static> Transfer<T, K> {
    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: TransferOptions<T, K>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn options(&self) -> TransferOptions<T, K> {
        self.inner.options.borrow().clone()
    }

    fn accepted(&self, this: &T, key: &K, list: &K) -> bool {
        let accepts = self.inner.options.borrow().accepts.clone();
        accepts.is_none_or(|accepts| accepts(this, key, list))
    }

    /// Starts a render of the lists and items, from the part that holds
    /// them, every frame it renders. Binding one starts one too.
    pub fn begin(&self, cx: &App) {
        self.inner.lists.begin(cx);
        self.inner.items.begin(cx);
    }

    /// Joins a list as it renders, so drags find it (`bindList`).
    pub fn bind_list(&self, list: K, cx: &App) {
        self.inner.lists.bind(list, (), cx);
    }

    /// Adds a layer to a list's element that records its bounds each
    /// layout, which a drag finds the list under the pointer by.
    pub fn measure_list<E: ParentElement + Styled>(&self, list: K, el: E) -> E {
        self.inner.lists.measure(list, el)
    }

    /// Joins an item as it renders (`bindItem`).
    pub fn bind_item(&self, key: K, item: TransferItem, cx: &App) {
        self.inner.items.bind(key, item, cx);
    }

    /// Adds a layer to an item's element that records its bounds each
    /// layout, which a drag finds its place by.
    pub fn measure_item<E: ParentElement + Styled>(&self, key: K, el: E) -> E {
        self.inner.items.measure(key, el)
    }

    /// The element that follows the pointer while an item is dragged. Render
    /// it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T, K> {
        self.inner.drags.listener()
    }

    /// The copy of the dragged item to render, while a pointer drags one.
    pub fn preview(&self) -> Option<TransferPreview<K>> {
        let pointer = self.inner.pointer.borrow();
        let drag = pointer.as_ref()?;
        Some(TransferPreview {
            key: drag.key.clone(),
            preview: drag.preview?,
        })
    }

    /// The moving item and the lists it shows, while one is picked up.
    pub fn draft(&self, this: &T) -> Option<TransferDraft<K>> {
        draft_move::current(self, this)
    }

    /// The lists to render: the draft's while an item moves, else the value.
    pub fn lists(&self, this: &T) -> TransferLists<K> {
        match self.draft(this) {
            Some(draft) => draft.lists,
            None => self.value(this),
        }
    }

    /// Whether an item is being moved.
    pub fn moving(&self, this: &T, key: &K) -> bool {
        self.draft(this).is_some_and(|draft| draft.key == *key)
    }

    /// Whether a pointer is dragging the item.
    pub fn dragging(&self, this: &T, key: &K) -> bool {
        self.draft(this)
            .is_some_and(|draft| draft.key == *key && draft.pointer)
    }

    fn say_in(
        &self,
        this: &T,
        r#type: MessageType,
        key: &K,
        current: &TransferLists<K>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let options = self.options();
        let list = list_with(current, key).cloned();
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
        let list_name: SharedString = match &list {
            Some(list) => options
                .list_label
                .as_ref()
                .and_then(|label| label(this, list))
                .unwrap_or_else(|| list.to_string().into()),
            None => SharedString::default(),
        };
        let keys = list.as_ref().and_then(|list| items_of(current, list));
        let message = TransferMessage {
            r#type,
            key: key.clone(),
            label,
            list: list_name,
            position: keys
                .and_then(|keys| keys.iter().position(|item| item == key))
                .unwrap_or(0)
                + 1,
            total: keys.map_or(0, Vec::len),
        };
        let text = match &options.message {
            Some(message_fn) => message_fn(&message),
            None => transfer_message(&message),
        };
        announce_with(&text, AnnouncePriority::Assertive, window, cx);
    }

    fn grab_with(&self, this: &T, key: &K, pointer: bool, window: &mut Window, cx: &mut App) -> bool {
        let value = self.value(this);
        let Some(from) = list_with(&value, key).cloned() else {
            return false;
        };
        let disabled = self.inner.options.borrow().disabled.clone();
        if self.draft(this).is_some() || disabled.is_some_and(|disabled| disabled(this, key)) {
            return false;
        }
        draft_move::set_draft(
            self,
            this,
            TransferDraft {
                key: key.clone(),
                from,
                lists: value.clone(),
                pointer,
            },
            window,
            cx,
        );
        self.say_in(this, MessageType::Grab, key, &value, window, cx);
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

    /// Moves the picked-up item to `index` in `list`, in the draft. Returns
    /// whether it moved.
    pub fn move_to(
        &self,
        this: &mut T,
        list: &K,
        index: usize,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        self.move_to_in(this, list, index, window, cx)
    }

    fn move_to_in(
        &self,
        this: &T,
        list: &K,
        index: usize,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let Some(current) = self.draft(this) else {
            return false;
        };
        let changes_list = list_with(&current.lists, &current.key) != Some(list);
        if changes_list && !self.accepted(this, &current.key, list) {
            return false;
        }
        let next = transferred(&current.lists, &current.key, list, index);
        if same_lists(&next, &current.lists) {
            return false;
        }
        let key = current.key.clone();
        let pointer = current.pointer;
        draft_move::set_draft(
            self,
            this,
            TransferDraft {
                lists: next.clone(),
                ..current
            },
            window,
            cx,
        );
        self.say_in(this, MessageType::Move, &key, &next, window, cx);
        if !pointer {
            draft_move::focus_later(self, &key, window, cx);
        }
        cx.notify();
        true
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

    /// Moves an item to `index` in `list` at once, like a "Move to Done"
    /// menu item. Reports `Imperative`. Returns whether it went through.
    pub fn r#move(
        &self,
        this: &mut T,
        key: &K,
        list: &K,
        index: usize,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let value = self.value(this);
        let Some(from) = list_with(&value, key).cloned() else {
            return false;
        };
        if from != *list && !self.accepted(this, key, list) {
            return false;
        }
        let next = transferred(&value, key, list, index);
        if same_lists(&next, &value) {
            return false;
        }
        let moved = TransferMove {
            key: key.clone(),
            from,
            to: list.clone(),
            index,
        };
        let set = self.inner.options.borrow().set.clone();
        if !set(this, next.clone(), moved, ChangeOptions::reason(Reason::Imperative), window, cx) {
            return false;
        }
        self.say_in(this, MessageType::Drop, key, &next, window, cx);
        cx.notify();
        true
    }

    /// Handles a key pressed on an item and returns whether it did. Space or
    /// Enter picks the item up; while it's up, Up and Down move it in its
    /// list, Home and End to its ends, Left and Right to the next list that
    /// takes it, Space or Enter drops it, Escape puts it back and Tab puts it
    /// back and moves on. Every other key is swallowed while it's up.
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
        let list = list_with(&current.lists, &current.key)
            .cloned()
            .unwrap_or_else(|| current.from.clone());
        let keys = items_of(&current.lists, &list).cloned().unwrap_or_default();
        let index = keys.iter().position(|item| *item == current.key).unwrap_or(0);
        let rtl = is_rtl(cx);
        let across: isize = match pressed {
            Key::ArrowLeft => {
                if rtl {
                    1
                } else {
                    -1
                }
            }
            Key::ArrowRight => {
                if rtl {
                    -1
                } else {
                    1
                }
            }
            _ => 0,
        };
        if pressed == Key::Space || pressed == Key::Enter {
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
        } else if pressed == Key::ArrowUp || pressed == Key::ArrowDown {
            window.prevent_default();
            let to = if pressed == Key::ArrowUp {
                index.saturating_sub(1)
            } else {
                index + 1
            };
            self.move_to_in(this, &list, to, window, cx);
        } else if pressed == Key::Home || pressed == Key::End {
            window.prevent_default();
            let to = if pressed == Key::Home { 0 } else { keys.len() };
            self.move_to_in(this, &list, to, window, cx);
        } else if across != 0 && !modified {
            window.prevent_default();
            let order = self.inner.options.borrow().order.clone();
            let names = match order {
                Some(order) => order(this),
                None => self.value(this).into_iter().map(|(name, _)| name).collect(),
            };
            // The next list that takes it, at about the same height.
            let mut at = names
                .iter()
                .position(|name| *name == list)
                .map_or(-1, |at| at as isize)
                + across;
            while at >= 0 && (at as usize) < names.len() {
                if self.move_to_in(this, &names[at as usize], index, window, cx) {
                    break;
                }
                at += across;
            }
        }
        true
    }

    // The list under a point: the one containing it, else the nearest across.
    fn list_at(&self, point: Point<Pixels>) -> Option<K> {
        let mut best: Option<(K, f32)> = None;
        for list in self.inner.lists.keys() {
            let Some(rect) = self.inner.lists.bounds(&list) else {
                continue;
            };
            let dx = if point.x < rect.left() {
                rect.left() - point.x
            } else if point.x > rect.right() {
                point.x - rect.right()
            } else {
                Pixels::ZERO
            };
            let dy = if point.y < rect.top() {
                rect.top() - point.y
            } else if point.y > rect.bottom() {
                point.y - rect.bottom()
            } else {
                Pixels::ZERO
            };
            let distance = f32::from(dx).hypot(f32::from(dy));
            if best.as_ref().is_none_or(|(_, best)| distance < *best) {
                best = Some((list, distance));
            }
        }
        best.map(|(list, _)| list)
    }

    // Puts the dragged item in the list under the pointer, among its items.
    fn track(&self, this: &T, window: &mut Window, cx: &mut Context<T>) {
        let Some((key, at)) = self
            .inner
            .pointer
            .borrow()
            .as_ref()
            .map(|drag| (drag.key.clone(), drag.at))
        else {
            return;
        };
        let Some(current) = self.draft(this) else {
            return;
        };
        let Some(list) = self.list_at(at) else {
            return;
        };
        let middles: Vec<Pixels> = items_of(&current.lists, &list)
            .map(|keys| {
                keys.iter()
                    .filter(|item| **item != key)
                    .map(|item| {
                        self.inner
                            .items
                            .bounds(item)
                            .map_or(Pixels::ZERO, |rect: Bounds<Pixels>| rect.top() + rect.size.height / 2.)
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.move_to_in(this, &list, drop_index(&middles, at.y, false), window, cx);
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
        let scroller = self.options().auto_scroll.map(|auto_scroll| {
            let transfer = Rc::downgrade(&self.inner);
            let entity = cx.weak_entity();
            create_auto_scroll(auto_scroll.on_scroll(move |window, cx| {
                let Some(transfer) = Self::upgrade(&transfer) else {
                    return;
                };
                entity
                    .update(cx, |this, cx| transfer.track(this, window, cx))
                    .ok();
            }))
        });
        *self.inner.pointer.borrow_mut() = Some(PointerDrag {
            key: key.clone(),
            start: event.position,
            at: event.position,
            dragging: false,
            focused: false,
            last_move: None,
            preview: None,
            scroller,
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
                    Self::upgrade(&started).is_some_and(|transfer| transfer.drag_started(this, window, cx))
                })
                .on_move(move |this: &mut T, sample, window, cx| {
                    if let Some(transfer) = Self::upgrade(&moved) {
                        transfer.drag_moved(this, sample, window, cx);
                    }
                })
                .on_end(move |this: &mut T, end, canceled, window, cx| {
                    if let Some(transfer) = Self::upgrade(&ended) {
                        transfer.drag_ended(this, end, canceled, window, cx);
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
        let Some((key, start)) = self
            .inner
            .pointer
            .borrow()
            .as_ref()
            .map(|drag| (drag.key.clone(), drag.start))
        else {
            return false;
        };
        if !self.grab_with(this, &key, true, window, cx) {
            *self.inner.pointer.borrow_mut() = None;
            return false;
        }
        // The copy is the item in flight, not the place it would drop.
        let bounds = self
            .inner
            .items
            .bounds(&key)
            .unwrap_or(Bounds::new(start, Default::default()));
        let focused = self
            .element(&key)
            .is_some_and(|handle| handle.contains_focused(window, cx));
        if let Some(drag) = self.inner.pointer.borrow_mut().as_mut() {
            drag.preview = Some(bind_drag_preview(bounds, start));
            drag.dragging = true;
            drag.focused = focused;
        }
        cx.notify();
        true
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
            if let Some(preview) = &mut drag.preview {
                preview.r#move(sample.x, sample.y);
            }
            drag.scroller.clone()
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
        if let Some(scroller) = &drag.scroller {
            scroller.stop();
        }
        if !drag.dragging {
            return;
        }
        if canceled {
            draft_move::cancel(self, this, true, window, cx);
        } else {
            let event = drag.last_move.unwrap_or_else(|| end.clone()).event;
            draft_move::drop(self, this, Reason::Drag, Some(event), window, cx);
        }
        // The item renders in its new list; focus follows it there.
        if drag.focused {
            draft_move::focus_later(self, &drag.key, window, cx);
        }
        cx.notify();
    }
}
