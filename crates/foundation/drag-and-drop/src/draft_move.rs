//! Port of `foundation/drag-and-drop/draft-move.ts`: what every move through
//! a draft shares, in a list, a tree or between lists: the draft, which
//! renders as the item moves, a drop that commits it through the
//! component's change and says where it landed, and a cancel that puts it
//! back. Focus follows a keyboard move, since an item rendered anew, like in
//! another list, can drop its focus.
//!
//! The original keeps the draft in the primitive's store and rebases it
//! whenever the store changes (`deriveState`). Here the behavior keeps the
//! draft, and reads the component's value through its options, so a value
//! the owner changed mid-move is noticed the next time the behavior reads
//! the draft, which rendering and every change do. Items join as they
//! render, the original's `bind`, through `Items`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gpui::{
    App, Bounds, Context, FocusHandle, ParentElement, Pixels, Styled, Task, Window, canvas,
};
use ui_sdk_focus::keep_focus;
use ui_sdk_kernel::{ChangeOptions, InputEvent, Reason};

/// Which step of a move a message is for (a message's `type`): `Grab` when
/// an item is picked up, `Move` for each step, `Drop` when it lands, `Cancel`
/// when it goes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageType {
    Grab,
    Move,
    Drop,
    Cancel,
}

impl MessageType {
    /// The original's name for it, like `"grab"`.
    pub fn as_str(self) -> &'static str {
        match self {
            MessageType::Grab => "grab",
            MessageType::Move => "move",
            MessageType::Drop => "drop",
            MessageType::Cancel => "cancel",
        }
    }
}

/// What a draft says about its item: its key, and whether a pointer drags
/// it rather than the keyboard moving it.
pub(crate) trait MoveDraft: Clone + 'static {
    type Key: Clone + PartialEq + 'static;
    fn key(&self) -> &Self::Key;
    fn pointer(&self) -> bool;
}

// A dropped draft waiting for the value it committed (`settleDraft`).
struct Settling<K, V> {
    before: V,
    key: K,
    generation: u64,
}

/// What a move through a draft keeps (`createDraftMove`'s closure): the
/// draft, whether it's held, whether it moved since it was picked up, where
/// focus was in the item then, and the value it last saw.
pub(crate) struct DraftMove<D: MoveDraft, V> {
    draft: Option<D>,
    // A dropped draft waits for its value without being held.
    held: bool,
    moved: bool,
    // Read when the item is picked up, since a drag can render it anew
    // before it ends: the handle focus was on, the item's or one inside it.
    grabbed: Option<FocusHandle>,
    seen: Option<V>,
    settling: Option<Settling<D::Key, V>>,
    generation: u64,
    _settle: Option<Task<()>>,
}

impl<D: MoveDraft, V> Default for DraftMove<D, V> {
    fn default() -> Self {
        Self {
            draft: None,
            held: false,
            moved: false,
            grabbed: None,
            seen: None,
            settling: None,
            generation: 0,
            _settle: None,
        }
    }
}

/// What a behavior gives the moves through its draft: its value, the value
/// a draft shows, how a draft follows a value the owner changed, how it
/// commits, what screen readers hear, and where its items are.
pub(crate) trait DraftOps<T: 'static>: Clone + 'static {
    type Draft: MoveDraft;
    type Value: Clone + 'static;

    fn moves(&self) -> &RefCell<DraftMove<Self::Draft, Self::Value>>;
    /// The value the draft commits to.
    fn value(&self, this: &T) -> Self::Value;
    /// The value as the draft shows it.
    fn shown(&self, draft: &Self::Draft) -> Self::Value;
    /// Whether two values are the same, so a drop that moved nothing
    /// commits nothing.
    fn same(&self, a: &Self::Value, b: &Self::Value) -> bool;
    /// The draft over a value the owner changed mid-move: picked up from it
    /// again, put where the user moved it if they did, or `None` when the
    /// item is gone.
    fn rebase(
        &self,
        this: &T,
        draft: &Self::Draft,
        value: &Self::Value,
        moved: bool,
    ) -> Option<Self::Draft>;
    /// Commits a moved draft. Returns false when the change was canceled.
    fn commit(
        &self,
        this: &mut T,
        draft: &Self::Draft,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool;
    /// Tells screen readers the item dropped or went back, in the value it's
    /// shown in.
    fn say(
        &self,
        this: &T,
        r#type: MessageType,
        draft: &Self::Draft,
        shown: &Self::Value,
        window: &mut Window,
        cx: &mut App,
    );
    /// The focus handle the item's element tracks (`element`).
    fn element(&self, key: &KeyOf<T, Self>) -> Option<FocusHandle>;
}

pub(crate) type KeyOf<T, O> = <<O as DraftOps<T>>::Draft as MoveDraft>::Key;

/// The draft now (`draft()`), over the value now: a settling draft whose
/// value arrived clears, and a held one follows a value the owner changed,
/// as `deriveState` keeps it.
pub(crate) fn current<T: 'static, O: DraftOps<T>>(ops: &O, this: &T) -> Option<O::Draft> {
    let value = ops.value(this);
    {
        let mut moves = ops.moves().borrow_mut();
        let arrived = moves
            .settling
            .as_ref()
            .is_some_and(|settling| !ops.same(&settling.before, &value));
        if arrived && let Some(settling) = moves.settling.take() {
            moves._settle = None;
            if moves.draft.as_ref().is_some_and(|draft| *draft.key() == settling.key) {
                clear_in(&mut moves);
            }
        }
    }
    let stale = {
        let mut moves = ops.moves().borrow_mut();
        let changed = moves.seen.as_ref().is_none_or(|seen| !ops.same(seen, &value));
        // Seen before rebasing, so a callback that reads the draft again
        // doesn't rebase it again.
        moves.seen = Some(value.clone());
        match &moves.draft {
            Some(draft) if moves.held && changed => Some((draft.clone(), moves.moved)),
            _ => None,
        }
    };
    if let Some((draft, moved)) = stale {
        let next = ops.rebase(this, &draft, &value, moved);
        let kept = next
            .as_ref()
            .is_some_and(|next| ops.same(&ops.shown(next), &ops.shown(&draft)));
        if !kept {
            ops.moves().borrow_mut().draft = next;
        }
    }
    ops.moves().borrow().draft.clone()
}

// Where focus is in an item: the handle that has it, the item's or one
// inside it, or `None` when it's elsewhere.
fn focus_in<T: 'static, O: DraftOps<T>>(
    ops: &O,
    key: &KeyOf<T, O>,
    window: &Window,
    cx: &App,
) -> Option<FocusHandle> {
    let item = ops.element(key)?;
    if !item.contains_focused(window, cx) {
        return None;
    }
    window.focused(cx)
}

fn clear_in<D: MoveDraft, V>(moves: &mut DraftMove<D, V>) {
    moves.draft = None;
    moves.held = false;
    moves.moved = false;
}

/// Starts or moves the draft (`setDraft`). Picking an item up notes where
/// focus is in it.
pub(crate) fn set_draft<T: 'static, O: DraftOps<T>>(
    ops: &O,
    this: &T,
    next: O::Draft,
    window: &Window,
    cx: &App,
) {
    let value = ops.value(this);
    let picking_up = ops.moves().borrow().draft.is_none();
    let grabbed = if picking_up {
        focus_in(ops, next.key(), window, cx)
    } else {
        None
    };
    let mut moves = ops.moves().borrow_mut();
    if picking_up {
        moves.grabbed = grabbed;
    }
    moves.moved = moves.draft.is_some();
    moves.held = true;
    moves.seen = Some(value);
    // A draft held again leaves the one settling behind.
    moves.settling = None;
    moves._settle = None;
    moves.draft = Some(next);
}

/// Ends the draft (`setDraft(null)`).
pub(crate) fn clear<T: 'static, O: DraftOps<T>>(ops: &O, this: &T) {
    let value = ops.value(this);
    let mut moves = ops.moves().borrow_mut();
    clear_in(&mut moves);
    moves.seen = Some(value);
}

/// Keeps focus where it is in the item, or where it was when it was picked
/// up, or on the item, once it renders anew (`focusLater`). gpui keeps focus
/// on a handle, so the handle that had it is the place.
pub(crate) fn focus_later<T: 'static, O: DraftOps<T>>(
    ops: &O,
    key: &KeyOf<T, O>,
    window: &Window,
    cx: &mut App,
) {
    let at = focus_in(ops, key, window, cx).or_else(|| ops.moves().borrow().grabbed.clone());
    let ops = ops.clone();
    let key = key.clone();
    keep_focus(
        move |window, _cx| {
            at.clone()
                .filter(|handle| handle.is_rendered(window))
                .or_else(|| ops.element(&key))
        },
        window,
        cx,
    );
}

/// Commits the draft where it is and says where it landed, or puts it back
/// if the change was canceled (`drop`).
pub(crate) fn drop<T: 'static, O: DraftOps<T>>(
    ops: &O,
    this: &mut T,
    reason: Reason,
    event: Option<InputEvent>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let Some(current) = current(ops, this) else {
        return;
    };
    ops.moves().borrow_mut().held = false;
    let before = ops.value(this);
    let changed = !ops.same(&ops.shown(&current), &before);
    let options = ChangeOptions {
        reason: Some(reason),
        source: None,
        event,
    };
    if changed && !ops.commit(this, &current, options, window, cx) {
        clear(ops, this);
        let now = ops.value(this);
        ops.say(this, MessageType::Cancel, &current, &now, window, cx);
    } else {
        let shown = ops.shown(&current);
        ops.say(this, MessageType::Drop, &current, &shown, window, cx);
        // Keeps the draft until a controlled value's new value arrives.
        if !changed {
            clear(ops, this);
        } else {
            settle(ops, this, before, current.key().clone(), cx);
        }
    }
    if !current.pointer() {
        focus_later(ops, current.key(), window, cx);
    }
    cx.notify();
}

// Clears the dropped draft once its value arrives, or a task later, as
// `settleDraft` does: an uncontrolled value has already moved on, so it
// clears now, and an app that kept the old value shows it again. A draft
// picked up again meanwhile stays.
fn settle<T: 'static, O: DraftOps<T>>(
    ops: &O,
    this: &T,
    before: O::Value,
    key: KeyOf<T, O>,
    cx: &mut Context<T>,
) {
    if !ops.same(&ops.value(this), &before) {
        if ops.moves().borrow().draft.as_ref().is_some_and(|draft| *draft.key() == key) {
            clear(ops, this);
        }
        return;
    }
    let generation = {
        let mut moves = ops.moves().borrow_mut();
        moves.generation += 1;
        moves.generation
    };
    let waiting = ops.clone();
    let task = cx.spawn(async move |entity, cx| {
        cx.background_executor().timer(Duration::ZERO).await;
        entity
            .update(cx, |this, cx| {
                let settled = {
                    let mut moves = waiting.moves().borrow_mut();
                    let ours = moves
                        .settling
                        .as_ref()
                        .is_some_and(|settling| settling.generation == generation);
                    match moves.settling.take_if(|_| ours) {
                        Some(settling) => {
                            let same_key = moves
                                .draft
                                .as_ref()
                                .is_some_and(|draft| *draft.key() == settling.key);
                            if same_key {
                                clear_in(&mut moves);
                            }
                            true
                        }
                        None => false,
                    }
                };
                if settled {
                    let value = waiting.value(this);
                    waiting.moves().borrow_mut().seen = Some(value);
                    cx.notify();
                }
            })
            .ok();
    });
    let mut moves = ops.moves().borrow_mut();
    moves.settling = Some(Settling {
        before,
        key,
        generation,
    });
    moves._settle = Some(task);
}

/// Puts the draft's item back where it was (`cancel`). `refocus` keeps
/// focus on it after a keyboard move; Tab, which moves on, doesn't.
pub(crate) fn cancel<T: 'static, O: DraftOps<T>>(
    ops: &O,
    this: &mut T,
    refocus: bool,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let Some(current) = current(ops, this) else {
        return;
    };
    clear(ops, this);
    let now = ops.value(this);
    ops.say(this, MessageType::Cancel, &current, &now, window, cx);
    if !current.pointer() && refocus {
        focus_later(ops, current.key(), window, cx);
    }
    cx.notify();
}

struct ItemsInner<K, P> {
    // Whether a render of the items is under way.
    open: bool,
    order: Vec<(K, P)>,
    bounds: HashMap<K, Bounds<Pixels>>,
}

/// The items of a behavior that rendered last, in render order, with what
/// each said and its bounds as of its last layout (`createOrder` and
/// `bind`). The first item to join in a frame starts a new render of them,
/// which ends once the frame is drawn, so an item that stops rendering
/// leaves; `begin` starts one from the part that holds them, which is how a
/// render with no items at all is told.
pub(crate) struct Items<K, P> {
    inner: Rc<RefCell<ItemsInner<K, P>>>,
}

impl<K, P> Clone for Items<K, P> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<K: Clone + Eq + Hash + 'static, P: Clone + 'static> Items<K, P> {
    pub(crate) fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(ItemsInner {
                open: false,
                order: Vec::new(),
                bounds: HashMap::new(),
            })),
        }
    }

    /// Starts a new render of the items, unless one is under way.
    pub(crate) fn begin(&self, cx: &App) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.open {
                return;
            }
            inner.open = true;
            let ItemsInner { order, bounds, .. } = &mut *inner;
            let kept: HashSet<&K> = order.iter().map(|(key, _)| key).collect();
            bounds.retain(|key, _| kept.contains(key));
            order.clear();
        }
        let inner: Weak<RefCell<ItemsInner<K, P>>> = Rc::downgrade(&self.inner);
        cx.spawn(async move |_cx| {
            if let Some(inner) = inner.upgrade() {
                inner.borrow_mut().open = false;
            }
        })
        .detach();
    }

    /// Joins an item as it renders. One bound twice in a render keeps its
    /// first place, with what it said last.
    pub(crate) fn bind(&self, key: K, item: P, cx: &App) {
        self.begin(cx);
        let mut inner = self.inner.borrow_mut();
        if let Some(entry) = inner.order.iter_mut().find(|(bound, _)| *bound == key) {
            entry.1 = item;
            return;
        }
        inner.order.push((key, item));
    }

    pub(crate) fn keys(&self) -> Vec<K> {
        self.inner.borrow().order.iter().map(|(key, _)| key.clone()).collect()
    }

    pub(crate) fn contains(&self, key: &K) -> bool {
        self.inner.borrow().order.iter().any(|(bound, _)| bound == key)
    }

    pub(crate) fn get(&self, key: &K) -> Option<P> {
        self.inner
            .borrow()
            .order
            .iter()
            .find(|(bound, _)| bound == key)
            .map(|(_, item)| item.clone())
    }

    /// The bounds of a bound item as of its last layout.
    pub(crate) fn bounds(&self, key: &K) -> Option<Bounds<Pixels>> {
        let inner = self.inner.borrow();
        if !inner.order.iter().any(|(bound, _)| bound == key) {
            return None;
        }
        inner.bounds.get(key).copied()
    }

    /// Adds a layer to `el` that records its bounds each layout.
    pub(crate) fn measure<E: ParentElement + Styled>(&self, key: K, el: E) -> E {
        let inner: Weak<RefCell<ItemsInner<K, P>>> = Rc::downgrade(&self.inner);
        ui_sdk_kernel::ensure_positioned(el).child(
            canvas(
                move |bounds, _window, _cx| {
                    if let Some(inner) = inner.upgrade()
                        && let Ok(mut inner) = inner.try_borrow_mut()
                    {
                        inner.bounds.insert(key, bounds);
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }
}
