//! Port of `foundation/kernel/elements.ts`: the bound parts of a slot, in
//! page order, with the props each rendered with.
//!
//! The original reads page order from the DOM. gpui has none, so a component
//! keeps a [`Parts`] per slot it has many of: each part registers its key as
//! it renders, which is page order for parts rendered in order, with what it
//! rendered with, like `elements<Props>(context, slot)` names it, and its
//! bounds once laid out, which is what spatial navigation and positioning
//! read. A part that didn't render this frame is no longer bound.
//! `order.rs` compares and sorts by it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

use gpui::{
    App, Bounds, Div, FocusHandle, IntoElement, ParentElement, Pixels, Stateful, Styled, Window,
    canvas,
};

/// What a bound part reports besides its key, when its slot keeps nothing
/// of its own.
#[derive(Clone, Debug, Default)]
pub struct PartInfo {
    pub disabled: bool,
    /// The text typeahead and agents match, its accessible name.
    pub text: Option<String>,
}

struct Entry<K, P> {
    key: K,
    info: P,
}

struct Inner<K, P> {
    frame: u64,
    order: Vec<Entry<K, P>>,
    focus: HashMap<K, FocusHandle>,
    bounds: HashMap<K, Bounds<Pixels>>,
}

/// The parts of one slot, keyed, in render order, with what each rendered
/// with (`P`, `PartInfo` unless the slot names its props), their focus
/// handles and bounds.
pub struct Parts<K, P = PartInfo> {
    inner: Rc<RefCell<Inner<K, P>>>,
}

impl<K, P> Clone for Parts<K, P> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<K: Clone + Eq + Hash + 'static, P: Clone + 'static> Default for Parts<K, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Clone + Eq + Hash + 'static, P: Clone + 'static> Parts<K, P> {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                frame: 0,
                order: Vec::new(),
                focus: HashMap::new(),
                bounds: HashMap::new(),
            })),
        }
    }

    /// Starts a new render of the slot's parts. The part that contains them,
    /// like a list, calls it before they render.
    pub fn begin(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.frame += 1;
        inner.order.clear();
    }

    /// Registers a part as it renders. A key registered twice in one render
    /// keeps its first place, with what it rendered with last.
    pub fn register(&self, key: K, info: P) {
        let mut inner = self.inner.borrow_mut();
        if let Some(entry) = inner.order.iter_mut().find(|entry| entry.key == key) {
            entry.info = info;
            return;
        }
        inner.order.push(Entry { key, info });
    }

    /// The bound parts' keys in page order.
    pub fn keys(&self) -> Vec<K> {
        self.inner.borrow().order.iter().map(|entry| entry.key.clone()).collect()
    }

    /// The bound parts with what they rendered with, in page order
    /// (`elements`).
    pub fn entries(&self) -> Vec<(K, P)> {
        self.inner
            .borrow()
            .order
            .iter()
            .map(|entry| (entry.key.clone(), entry.info.clone()))
            .collect()
    }

    /// What the part with `key` rendered with (`context.props(slot, key)`).
    pub fn info(&self, key: &K) -> Option<P> {
        self.inner
            .borrow()
            .order
            .iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.info.clone())
    }

    pub fn contains(&self, key: &K) -> bool {
        self.inner.borrow().order.iter().any(|entry| &entry.key == key)
    }

    pub fn index_of(&self, key: &K) -> Option<usize> {
        self.inner.borrow().order.iter().position(|entry| &entry.key == key)
    }

    pub fn len(&self) -> usize {
        self.inner.borrow().order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The part's focus handle, made the first time it's asked for and kept
    /// while the component lives (`context.element(slot, key)` for focus).
    pub fn focus_handle(&self, key: &K, cx: &App) -> FocusHandle {
        let mut inner = self.inner.borrow_mut();
        inner
            .focus
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// The part's focus handle if it has one.
    pub fn existing_focus_handle(&self, key: &K) -> Option<FocusHandle> {
        self.inner.borrow().focus.get(key).cloned()
    }

    /// The key of the part that holds focus in `window`, if one does.
    pub fn focused(&self, window: &Window) -> Option<K> {
        let inner = self.inner.borrow();
        inner
            .order
            .iter()
            .find(|entry| {
                inner
                    .focus
                    .get(&entry.key)
                    .is_some_and(|handle| handle.is_focused(window))
            })
            .map(|entry| entry.key.clone())
    }

    /// Whether focus is on one of the parts or inside one.
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        let inner = self.inner.borrow();
        inner.focus.values().any(|handle| handle.contains_focused(window, cx))
    }

    /// The part's bounds as of its last layout.
    pub fn bounds(&self, key: &K) -> Option<Bounds<Pixels>> {
        self.inner.borrow().bounds.get(key).copied()
    }

    /// Drops focus handles and bounds of keys no longer bound, like removed
    /// items.
    pub fn retain_bound(&self) {
        let mut inner = self.inner.borrow_mut();
        let keys: Vec<K> = inner.order.iter().map(|entry| entry.key.clone()).collect();
        inner.focus.retain(|key, _| keys.contains(key));
        inner.bounds.retain(|key, _| keys.contains(key));
    }

    /// Adds a layer to `el` that records its bounds each layout.
    pub fn measure(&self, key: K, el: Stateful<Div>) -> Stateful<Div> {
        let inner = self.inner.clone();
        ensure_positioned(el).child(
            canvas(
                move |bounds, _window, _cx| {
                    inner.borrow_mut().bounds.insert(key, bounds);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    #[doc(hidden)]
    pub fn frame(&self) -> u64 {
        self.inner.borrow().frame
    }
}

/// Bounds of a single part, for parts there's one of.
#[derive(Clone, Default)]
pub struct Measured {
    bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
}

impl Measured {
    pub fn get(&self) -> Option<Bounds<Pixels>> {
        *self.bounds.borrow()
    }

    pub fn measure<E: ParentElement + Styled>(&self, el: E) -> E {
        let cell = self.bounds.clone();
        ensure_positioned(el).child(
            canvas(
                move |bounds, _window, _cx| {
                    *cell.borrow_mut() = Some(bounds);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into_any_element(),
        )
    }
}

/// A slot's bound parts with what they rendered with, in page order
/// (`elements`).
pub fn elements<K: Clone + Eq + Hash + 'static, P: Clone + 'static>(parts: &Parts<K, P>) -> Vec<(K, P)> {
    parts.entries()
}

/// Makes `el` the containing block for a measuring layer: `relative`, unless
/// it's already positioned, so an `absolute` element stays absolute.
pub fn ensure_positioned<E: Styled>(mut el: E) -> E {
    if el.style().position.is_some() {
        el
    } else {
        el.relative()
    }
}
