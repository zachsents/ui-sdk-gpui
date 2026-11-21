//! Port of `foundation/selection/selection-model.ts`: single and multiple
//! selection over keys: choosing, toggling, Shift ranges from an anchor and
//! toggling everything, as a listbox, a tree or a grid selects. A listbox
//! runs on [`create_key_selection`]; [`create_selection`] is the same model
//! for a component's own value.
//!
//! Both are handles a component makes in its constructor and keeps in a
//! field. They read the component through the getters they're given and ask
//! for changes through its `set`, so the calls that may change the
//! selection take the component itself, `this`, after a clone of the handle:
//!
//! ```ignore
//! let selection = self.selection.clone();
//! selection.toggle(self, &key, ChangeOptions::event(Reason::ItemPress, event), window, cx);
//! ```
//!
//! The details a change reports are the kernel's [`ChangeOptions`], so `set`
//! hands them straight to `Controllable::change`.

use std::cell::RefCell;
use std::fmt;
use std::rc::{Rc, Weak};

use gpui::{Context, Window};
use ui_sdk_kernel::ChangeOptions;

use crate::listbox_range::{RangeAnchor, all_toggled, is_same_keys, range_selection, toggled_keys};

/// Reads keys from the component, like its selection or its items in list
/// order.
pub type KeysFn<T, K> = Rc<dyn Fn(&T) -> Vec<K>>;

/// Asks the component for a new selection, with why, and returns whether it
/// went through, so a refused one moves no anchor. It runs inside the
/// component's update, with the component.
pub type SetKeysFn<T, K> =
    Rc<dyn Fn(&mut T, Vec<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool>;

/// Whether the component lets a key be chosen.
pub type SelectableFn<T, K> = Rc<dyn Fn(&T, &K) -> bool>;

/// Reads whether the component allows more than one item to be selected.
pub type MultipleFn<T> = Rc<dyn Fn(&T) -> bool>;

/// What [`create_key_selection`] reads and writes the keys through.
pub struct KeySelectionConfig<T: 'static, K> {
    /// The selected keys now.
    pub keys: KeysFn<T, K>,
    /// Asks for a new selection and returns whether it went through.
    pub set: SetKeysFn<T, K>,
    /// Every key in list order, for ranges and select all.
    pub order: KeysFn<T, K>,
    /// Whether a key can be chosen, toggled, or added to a range.
    pub selectable: SelectableFn<T, K>,
}

impl<T: 'static, K> Clone for KeySelectionConfig<T, K> {
    fn clone(&self) -> Self {
        Self {
            keys: self.keys.clone(),
            set: self.set.clone(),
            order: self.order.clone(),
            selectable: self.selectable.clone(),
        }
    }
}

impl<T: 'static, K: 'static> KeySelectionConfig<T, K> {
    pub fn new(
        keys: impl Fn(&T) -> Vec<K> + 'static,
        set: impl Fn(&mut T, Vec<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool + 'static,
        order: impl Fn(&T) -> Vec<K> + 'static,
        selectable: impl Fn(&T, &K) -> bool + 'static,
    ) -> Self {
        Self {
            keys: Rc::new(keys),
            set: Rc::new(set),
            order: Rc::new(order),
            selectable: Rc::new(selectable),
        }
    }
}

struct KeyInner<T: 'static, K> {
    config: KeySelectionConfig<T, K>,
    // Where Shift ranges start, set by every choice made without Shift.
    anchor: RefCell<Option<RangeAnchor<K>>>,
}

/// The selection model on keys it reads and writes through callbacks
/// (`createKeySelection`'s return). Cloning it gives another handle to the
/// same model.
pub struct KeySelection<T: 'static, K> {
    inner: Rc<KeyInner<T, K>>,
}

impl<T: 'static, K> Clone for KeySelection<T, K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: 'static, K: fmt::Debug> fmt::Debug for KeySelection<T, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeySelection")
            .field("anchor", &*self.inner.anchor.borrow())
            .finish()
    }
}

/// The model on keys it reads and writes through callbacks
/// (`createKeySelection`). `set` returns whether the change went through,
/// so a refused one moves no anchor.
pub fn create_key_selection<T: 'static, K: Clone + PartialEq + 'static>(
    config: KeySelectionConfig<T, K>,
) -> KeySelection<T, K> {
    KeySelection {
        inner: Rc::new(KeyInner {
            config,
            anchor: RefCell::new(None),
        }),
    }
}

impl<T: 'static, K: Clone + PartialEq + 'static> KeySelection<T, K> {
    /// Adds or removes one item, and starts the next range there when it
    /// went through.
    pub fn toggle(
        &self,
        this: &mut T,
        key: &K,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let config = self.inner.config.clone();
        if !(config.selectable)(this, key) {
            return;
        }
        let next = toggled_keys(&(config.keys)(this), key);
        if (config.set)(this, next.clone(), why, window, cx) {
            *self.inner.anchor.borrow_mut() = Some(RangeAnchor::new(key.clone(), next));
        }
    }

    /// Selects only this item, and starts the next range there.
    pub fn replace(
        &self,
        this: &mut T,
        key: &K,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let config = self.inner.config.clone();
        if !(config.selectable)(this, key) {
            return;
        }
        let only = vec![key.clone()];
        if !is_same_keys(&(config.keys)(this), &only)
            && !(config.set)(this, only.clone(), why, window, cx)
        {
            return;
        }
        *self.inner.anchor.borrow_mut() = Some(RangeAnchor::new(key.clone(), only));
    }

    /// Selects from the anchor to `to`. Without an anchor, the range starts
    /// at `from`, the item focus was on, or at `to` itself. `additive: false`
    /// drops what was selected when the anchor was set.
    #[allow(clippy::too_many_arguments)]
    pub fn extend(
        &self,
        this: &mut T,
        to: &K,
        from: Option<&K>,
        additive: bool,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let config = self.inner.config.clone();
        let list = (config.order)(this);
        let keys = (config.keys)(this);
        let anchor = {
            let mut anchor = self.inner.anchor.borrow_mut();
            if anchor.as_ref().is_none_or(|anchor| !list.contains(&anchor.key)) {
                *anchor = Some(RangeAnchor::new(from.unwrap_or(to).clone(), keys.clone()));
            }
            anchor.clone()
        };
        let Some(anchor) = anchor else {
            return;
        };
        let next = range_selection(&list, &anchor, to, additive, |key| (config.selectable)(this, key));
        if !is_same_keys(&next, &keys) {
            (config.set)(this, next, why, window, cx);
        }
    }

    /// Selects every item that can be chosen, or none once they all are,
    /// like Ctrl+A.
    pub fn toggle_all(
        &self,
        this: &mut T,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let config = self.inner.config.clone();
        let keys = (config.keys)(this);
        let selectable: Vec<K> = (config.order)(this)
            .into_iter()
            .filter(|key| (config.selectable)(this, key))
            .collect();
        let next = all_toggled(&keys, &selectable);
        if !is_same_keys(&next, &keys) {
            (config.set)(this, next, why, window, cx);
        }
    }

    /// The key ranges start from, or `None`.
    pub fn anchor(&self) -> Option<K> {
        self.inner.anchor.borrow().as_ref().map(|anchor| anchor.key.clone())
    }

    /// Starts the next range at `key` without changing the selection; the
    /// range keeps what's selected now. `None` clears it.
    pub fn set_anchor(&self, this: &T, key: Option<K>) {
        let anchor = key.map(|key| RangeAnchor::new(key, (self.inner.config.keys)(this)));
        *self.inner.anchor.borrow_mut() = anchor;
    }

    /// Forgets the anchor, so the next range starts over.
    pub fn reset_anchor(&self) {
        *self.inner.anchor.borrow_mut() = None;
    }
}

// Outside state may hand the same keys back as a new list, or in another order.
fn same_members<K: PartialEq>(a: &[K], b: &[K]) -> bool {
    a.len() == b.len() && a.iter().all(|key| b.contains(key))
}

/// How [`create_selection`] reads and changes a selection
/// (`SelectionOptions`).
pub struct SelectionOptions<T: 'static, K> {
    /// The selected keys, the component's value (`key`).
    pub keys: KeysFn<T, K>,
    /// Asks for a new selection and returns whether it went through. Pass
    /// the component's change, so `on_*_change`, `cancel()` and controlled
    /// values work: `|this, value, details, window, cx|
    /// this.value.change(&this.instance, value, details, window, cx)`.
    pub set: SetKeysFn<T, K>,
    /// Every key in list order, for ranges and select all, like the visible
    /// items' keys.
    pub order: KeysFn<T, K>,
    /// Whether more than one item can be selected. Defaults to `false`.
    pub multiple: Option<MultipleFn<T>>,
    /// Whether a key can be chosen, toggled, or added to a range. Replacing
    /// the selection can still remove it. Defaults to every item.
    pub selectable: Option<SelectableFn<T, K>>,
}

impl<T: 'static, K> Clone for SelectionOptions<T, K> {
    fn clone(&self) -> Self {
        Self {
            keys: self.keys.clone(),
            set: self.set.clone(),
            order: self.order.clone(),
            multiple: self.multiple.clone(),
            selectable: self.selectable.clone(),
        }
    }
}

impl<T: 'static, K: 'static> SelectionOptions<T, K> {
    /// Options with the selection, how to change it, and the keys in list
    /// order.
    pub fn new(
        keys: impl Fn(&T) -> Vec<K> + 'static,
        set: impl Fn(&mut T, Vec<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool + 'static,
        order: impl Fn(&T) -> Vec<K> + 'static,
    ) -> Self {
        Self {
            keys: Rc::new(keys),
            set: Rc::new(set),
            order: Rc::new(order),
            multiple: None,
            selectable: None,
        }
    }

    pub fn multiple(mut self, multiple: impl Fn(&T) -> bool + 'static) -> Self {
        self.multiple = Some(Rc::new(multiple));
        self
    }

    pub fn selectable(mut self, selectable: impl Fn(&T, &K) -> bool + 'static) -> Self {
        self.selectable = Some(Rc::new(selectable));
        self
    }
}

/// How [`Selection::extend`] reaches: where a range starts without an
/// anchor, and whether it keeps what was selected before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendOptions<K> {
    /// Where the range starts without an anchor, like the item focus was on.
    pub from: Option<K>,
    /// Whether the range keeps what was selected when the anchor was set.
    /// Defaults to `true`.
    pub additive: bool,
}

impl<K> Default for ExtendOptions<K> {
    fn default() -> Self {
        Self {
            from: None,
            additive: true,
        }
    }
}

impl<K> ExtendOptions<K> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from(mut self, from: Option<K>) -> Self {
        self.from = from;
        self
    }

    pub fn additive(mut self, additive: bool) -> Self {
        self.additive = additive;
        self
    }
}

struct SelectionInner<T: 'static, K> {
    options: RefCell<SelectionOptions<T, K>>,
    model: KeySelection<T, K>,
    // The keys this selection last asked for, so a controlled value coming
    // back is known as its own.
    requested: RefCell<Option<Vec<K>>>,
    // The keys it saw last, `None` until it first reads them.
    previous: RefCell<Option<Vec<K>>>,
}

/// The selection rules of a listbox, for any collection whose selection is
/// a list of keys the component keeps (`createSelection`'s return, the
/// original's `SelectionModel`). Cloning it gives another handle to the same
/// selection.
pub struct Selection<T: 'static, K> {
    inner: Rc<SelectionInner<T, K>>,
}

/// What [`create_selection`] returns (`SelectionModel`).
pub type SelectionModel<T, K> = Selection<T, K>;

impl<T: 'static, K> Clone for Selection<T, K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: 'static, K: fmt::Debug> fmt::Debug for Selection<T, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Selection")
            .field("model", &self.inner.model)
            .field("requested", &*self.inner.requested.borrow())
            .finish()
    }
}

/// The selection rules of a listbox, for any collection whose selection is
/// a list of keys in the component (`createSelection`): `choose` selects one
/// item, `toggle` adds or removes one (Ctrl+click, Space), `extend` selects a
/// Shift range from the anchor, and `toggle_all` is Ctrl+A. Every choice
/// without Shift moves the anchor. A selection set from outside, like a
/// controlled value or an api call, starts ranges over. In single mode,
/// `toggle` unselects the selected item, and `extend` and `toggle_all`
/// choose.
///
/// The original hears the value change through the store. A component keeps
/// its own value here, so the selection compares the keys with the ones it
/// saw last as each call starts, and after its own `set`; a component calls
/// [`sync`](Selection::sync) when its value arrives from outside, like a
/// controlled value its app passed.
///
/// ```ignore
/// let selection = create_selection(SelectionOptions::new(
///     |this: &Tree| this.selected.get().clone(),
///     |this, value, details, window, cx| this.selected.change(&this.instance, value, details, window, cx),
///     |this| this.visible_keys(),
/// ).multiple(|this| this.multiple));
/// ```
pub fn create_selection<T: 'static, K: Clone + PartialEq + 'static>(
    options: SelectionOptions<T, K>,
) -> Selection<T, K> {
    let inner = Rc::new_cyclic(|weak: &Weak<SelectionInner<T, K>>| {
        let keys_of = weak.clone();
        let order_of = weak.clone();
        let selectable_of = weak.clone();
        let set_of = weak.clone();
        let config = KeySelectionConfig {
            keys: Rc::new(move |this: &T| {
                keys_of
                    .upgrade()
                    .map(|inner| Selection { inner }.keys(this))
                    .unwrap_or_default()
            }),
            order: Rc::new(move |this: &T| {
                let Some(inner) = order_of.upgrade() else {
                    return Vec::new();
                };
                let order = inner.options.borrow().order.clone();
                order(this)
            }),
            selectable: Rc::new(move |this: &T, key: &K| {
                selectable_of
                    .upgrade()
                    .is_some_and(|inner| Selection { inner }.selectable(this, key))
            }),
            set: Rc::new(
                move |this: &mut T,
                      next: Vec<K>,
                      why: ChangeOptions,
                      window: &mut Window,
                      cx: &mut Context<T>| {
                    let Some(inner) = set_of.upgrade() else {
                        return false;
                    };
                    Selection { inner }.request(this, next, why, window, cx)
                },
            ),
        };
        SelectionInner {
            options: RefCell::new(options),
            model: create_key_selection(config),
            requested: RefCell::new(None),
            previous: RefCell::new(None),
        }
    });
    Selection { inner }
}

impl<T: 'static, K: Clone + PartialEq + 'static> Selection<T, K> {
    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: SelectionOptions<T, K>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn keys(&self, this: &T) -> Vec<K> {
        let keys = self.inner.options.borrow().keys.clone();
        keys(this)
    }

    fn multiple(&self, this: &T) -> bool {
        let multiple = self.inner.options.borrow().multiple.clone();
        multiple.is_some_and(|multiple| multiple(this))
    }

    fn selectable(&self, this: &T, key: &K) -> bool {
        let selectable = self.inner.options.borrow().selectable.clone();
        selectable.is_none_or(|selectable| selectable(this, key))
    }

    // Asks for keys through `set`, noting them as this selection's own.
    fn request(
        &self,
        this: &mut T,
        next: Vec<K>,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        *self.inner.requested.borrow_mut() = Some(next.clone());
        let set = self.inner.options.borrow().set.clone();
        let done = set(this, next, why, window, cx);
        if done {
            // An uncontrolled value has changed already: hear it now, as the
            // store's subscriber does during the change.
            self.sync(this);
        } else {
            *self.inner.requested.borrow_mut() = None;
        }
        done
    }

    /// Hears the selection the component holds now, as the original's store
    /// subscription does: a selection set from outside starts ranges over,
    /// unless it's the one this selection asked for, and an empty one does
    /// too. Every call here hears it first; call it when the value arrives
    /// from outside, like a controlled value.
    pub fn sync(&self, this: &T) {
        let current = self.keys(this);
        let Some(previous) = self.inner.previous.replace(Some(current.clone())) else {
            return;
        };
        if is_same_keys(&current, &previous) || same_members(&current, &previous) {
            return;
        }
        let requested = self.inner.requested.borrow_mut().take();
        let own = requested.is_some_and(|requested| same_members(&requested, &current));
        if !own || current.is_empty() {
            self.inner.model.reset_anchor();
        }
    }

    /// Whether an item is selected.
    pub fn is_selected(&self, this: &T, key: &K) -> bool {
        self.keys(this).contains(key)
    }

    /// Selects only this item, and starts the next range there.
    pub fn choose(
        &self,
        this: &mut T,
        key: &K,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.sync(this);
        self.inner.model.replace(this, key, why, window, cx);
    }

    /// Adds or removes one item, and starts the next range there. In single
    /// mode, selects it or clears it.
    pub fn toggle(
        &self,
        this: &mut T,
        key: &K,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.sync(this);
        if self.multiple(this) {
            return self.inner.model.toggle(this, key, why, window, cx);
        }
        if !self.selectable(this, key) {
            return;
        }
        if is_same_keys(&self.keys(this), std::slice::from_ref(key)) {
            self.inner.model.toggle(this, key, why, window, cx);
        } else {
            self.inner.model.replace(this, key, why, window, cx);
        }
    }

    /// Selects the range from the anchor to `key`, keeping what was selected
    /// when the anchor was set. Without an anchor, the range starts at
    /// `from`, like the item focus was on. `additive: false` drops what was
    /// selected before. In single mode, chooses `key`.
    pub fn extend(
        &self,
        this: &mut T,
        key: &K,
        options: ExtendOptions<K>,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.sync(this);
        if !self.multiple(this) {
            return self.inner.model.replace(this, key, why, window, cx);
        }
        self.inner.model.extend(
            this,
            key,
            options.from.as_ref(),
            options.additive,
            why,
            window,
            cx,
        );
    }

    /// Selects every selectable item in `order`, or none once they all are,
    /// like Ctrl+A. Only in multiple mode.
    pub fn toggle_all(
        &self,
        this: &mut T,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.sync(this);
        if self.multiple(this) {
            self.inner.model.toggle_all(this, why, window, cx);
        }
    }

    /// The key ranges start from, or `None`.
    pub fn anchor(&self, this: &T) -> Option<K> {
        self.sync(this);
        self.inner.model.anchor()
    }

    /// Starts the next range at `key` without changing the selection, like
    /// when arrow keys move focus but don't select. The range keeps what's
    /// selected now. `None` clears it.
    pub fn set_anchor(&self, this: &T, key: Option<K>) {
        self.sync(this);
        self.inner.model.set_anchor(this, key);
    }
}
