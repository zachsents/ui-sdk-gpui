//! Port of `foundation/lists/listbox-value.ts`: the selected value of a
//! listbox, shared by Select and Combobox: which items it holds, their
//! labels, toggling one, and the keys it submits as. A value is one item's
//! value or nothing, or a list with `multiple` ([`ListValue`]), and values
//! may be any type: keys are their strings, from `item_to_value` or
//! [`ItemKey`](crate::ItemKey).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::rc::Rc;

use gpui::{App, Context, EntityId, SharedString};
use icu_list::ListFormatter;
use icu_list::options::{ListFormatterOptions, ListLength};
use icu_locale_core::Locale;

use crate::listbox_engine::ListboxEngine;

thread_local! {
    static LIST_FORMATS: RefCell<HashMap<String, Option<Rc<ListFormatter>>>> = RefCell::new(HashMap::new());
}

// `Intl.ListFormat(locale, { type: "conjunction" })`, kept per locale.
fn list_format(locale: &str) -> Option<Rc<ListFormatter>> {
    LIST_FORMATS.with(|formats| {
        formats
            .borrow_mut()
            .entry(locale.to_string())
            .or_insert_with(|| {
                let locale = Locale::try_from_str(locale).ok()?;
                ListFormatter::try_new_and(
                    (&locale).into(),
                    ListFormatterOptions::default().with_length(ListLength::Wide),
                )
                .ok()
                .map(Rc::new)
            })
            .clone()
    })
}

/// How [`format_labels`] shortens a long list of labels.
#[derive(Clone, Default)]
pub struct FormatLabelsOptions {
    /// How many labels to show before a count of the rest.
    pub max_labels: Option<usize>,
    /// The count of the rest, like `"2 more"` by default, in English.
    pub get_more_label: Option<Rc<dyn Fn(usize) -> SharedString>>,
}

impl FormatLabelsOptions {
    pub fn max_labels(mut self, max: usize) -> Self {
        self.max_labels = Some(max);
        self
    }

    pub fn get_more_label(mut self, label: impl Fn(usize) -> SharedString + 'static) -> Self {
        self.get_more_label = Some(Rc::new(label));
        self
    }
}

/// Labels as one text (`formatLabels`). Without `max_labels`, joined with
/// commas. With it, the first `max_labels` and a count of the rest, joined
/// the way `locale` joins a list, or in English with the default count:
/// "Apple, Banana, and 2 more". `locale` defaults to `"en-US"`.
pub fn format_labels(labels: &[SharedString], options: &FormatLabelsOptions, locale: Option<&str>) -> SharedString {
    let Some(max) = options.max_labels else {
        return labels.join(", ").into();
    };
    let shown: Vec<SharedString> = if labels.len() > max {
        let rest = labels.len() - max;
        let more = match &options.get_more_label {
            Some(more) => more(rest),
            None => format!("{rest} more").into(),
        };
        labels[..max].iter().cloned().chain(std::iter::once(more)).collect()
    } else {
        labels.to_vec()
    };
    // The default count is English, so its list joins in English too.
    let joined = if options.get_more_label.is_some() {
        locale.unwrap_or("en-US")
    } else {
        "en-US"
    };
    match list_format(joined) {
        Some(format) => format
            .format_to_string(shown.iter().map(|label| label.as_ref()))
            .into(),
        None => shown.join(", ").into(),
    }
}

/// A listbox's value: one item's value or nothing, or every selected item's
/// with `multiple` (the original's `value`, which is either).
#[derive(Clone, Debug, PartialEq)]
pub enum ListValue<T> {
    Single(Option<T>),
    Multiple(Vec<T>),
}

impl<T> Default for ListValue<T> {
    fn default() -> Self {
        ListValue::Single(None)
    }
}

impl<T: Clone> ListValue<T> {
    /// What selecting nothing is: nothing, or an empty list with `multiple`.
    pub fn empty(multiple: bool) -> Self {
        if multiple {
            ListValue::Multiple(Vec::new())
        } else {
            ListValue::Single(None)
        }
    }

    /// The value of `values`: the first one, or all with `multiple`.
    pub fn from_values(values: Vec<T>, multiple: bool) -> Self {
        if multiple {
            ListValue::Multiple(values)
        } else {
            ListValue::Single(values.into_iter().next())
        }
    }

    /// Whether it holds several (a list).
    pub fn is_multiple(&self) -> bool {
        matches!(self, ListValue::Multiple(_))
    }

    /// Its values as a list (`listOf`).
    pub fn as_slice(&self) -> &[T] {
        match self {
            ListValue::Single(value) => value.as_slice(),
            ListValue::Multiple(values) => values,
        }
    }

    pub fn to_vec(&self) -> Vec<T> {
        self.as_slice().to_vec()
    }

    pub fn into_vec(self) -> Vec<T> {
        match self {
            ListValue::Single(value) => value.into_iter().collect(),
            ListValue::Multiple(values) => values,
        }
    }

    /// The one value, or the first of several.
    pub fn first(&self) -> Option<&T> {
        self.as_slice().first()
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    /// The value in the shape `multiple` asks for: a list that starts or
    /// stops holding several keeps what was chosen, as the original does
    /// when options change `multiple` and the value isn't controlled.
    pub fn shaped(self, multiple: bool) -> Self {
        match (self, multiple) {
            (ListValue::Single(value), true) => ListValue::Multiple(value.into_iter().collect()),
            (ListValue::Multiple(values), false) => ListValue::Single(values.into_iter().next()),
            (value, _) => value,
        }
    }
}

impl<T> From<Option<T>> for ListValue<T> {
    fn from(value: Option<T>) -> Self {
        ListValue::Single(value)
    }
}

impl<T> From<Vec<T>> for ListValue<T> {
    fn from(values: Vec<T>) -> Self {
        ListValue::Multiple(values)
    }
}

/// One value, nothing, or a list, as a list (`listOf`).
pub fn list_of<T: Clone>(value: &ListValue<T>) -> &[T] {
    value.as_slice()
}

/// The key of a value of the component's own, `item_to_value`.
pub type ItemToValue<T> = Rc<dyn Fn(&T) -> SharedString>;

/// What the selection reads from the component's options: `item_to_value`,
/// `item_to_label` and `is_item_equal_to_value`.
pub struct ValueOptions<T> {
    pub item_to_value: Option<ItemToValue<T>>,
    pub item_to_label: Option<Rc<dyn Fn(&T) -> SharedString>>,
    pub is_item_equal_to_value: Option<Rc<dyn Fn(&T, &T) -> bool>>,
}

impl<T> Default for ValueOptions<T> {
    fn default() -> Self {
        Self {
            item_to_value: None,
            item_to_label: None,
            is_item_equal_to_value: None,
        }
    }
}

impl<T> Clone for ValueOptions<T> {
    fn clone(&self) -> Self {
        Self {
            item_to_value: self.item_to_value.clone(),
            item_to_label: self.item_to_label.clone(),
            is_item_equal_to_value: self.is_item_equal_to_value.clone(),
        }
    }
}

/// Reads the value from the component, as a list.
pub type ValuesFn<C, T> = Rc<dyn for<'a> Fn(&'a C) -> &'a [T]>;

/// What [`create_value_selection`] reads.
pub struct ValueSelectionConfig<C: 'static, T: 'static = SharedString, X: 'static = ()> {
    /// The value now, as a list: `ListValue::as_slice` of the component's.
    pub value: ValuesFn<C, T>,
    /// Whether the component holds several values.
    pub multiple: Rc<dyn Fn(&C) -> bool>,
    /// The component's options.
    pub options: Rc<dyn Fn(&C) -> ValueOptions<T>>,
    /// The list, whose `to_key` gives values their keys.
    pub listbox: ListboxEngine<C, T, X>,
}

impl<C: 'static, T: 'static, X: 'static> ValueSelectionConfig<C, T, X> {
    pub fn new(
        listbox: ListboxEngine<C, T, X>,
        value: impl for<'a> Fn(&'a C) -> &'a [T] + 'static,
        multiple: impl Fn(&C) -> bool + 'static,
        options: impl Fn(&C) -> ValueOptions<T> + 'static,
    ) -> Self {
        Self {
            value: Rc::new(value),
            multiple: Rc::new(multiple),
            options: Rc::new(options),
            listbox,
        }
    }
}

// The keys of the last value read, so items asking whether they're selected
// share one lookup instead of mapping the value each.
struct KeysMemo<T> {
    values: Vec<T>,
    item_to_value: Option<ItemToValue<T>>,
    keys: Rc<[SharedString]>,
    set: Rc<HashSet<SharedString>>,
}

fn same_fn<T: ?Sized>(a: &Option<Rc<T>>, b: &Option<Rc<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

/// The selected value of a listbox (`createValueSelection`'s return).
/// Cloning it gives another handle to the same selection.
pub struct ValueSelection<C: 'static, T: 'static = SharedString, X: 'static = ()> {
    inner: Rc<SelectionInner<C, T, X>>,
}

struct SelectionInner<C: 'static, T: 'static, X: 'static> {
    config: ValueSelectionConfig<C, T, X>,
    entity: EntityId,
    keys_memo: RefCell<Option<KeysMemo<T>>>,
    last_labels: RefCell<Rc<[SharedString]>>,
    labels_queued: Rc<Cell<bool>>,
    // The labels' version the component last rendered for (`store.labels`).
    labels_seen: Cell<u64>,
    learned: Cell<bool>,
}

impl<C: 'static, T: 'static, X: 'static> Clone for ValueSelection<C, T, X> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// The selected value of a listbox, for the component of `cx`, which it
/// renders again when it learns labels (`createValueSelection`).
pub fn create_value_selection<C, T, X>(config: ValueSelectionConfig<C, T, X>, cx: &Context<C>) -> ValueSelection<C, T, X>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    ValueSelection {
        inner: Rc::new(SelectionInner {
            config,
            entity: cx.entity_id(),
            keys_memo: RefCell::new(None),
            last_labels: RefCell::new(Vec::new().into()),
            labels_queued: Rc::new(Cell::new(false)),
            labels_seen: Cell::new(0),
            learned: Cell::new(false),
        }),
    }
}

impl<C, T, X> ValueSelection<C, T, X>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    fn listbox(&self) -> &ListboxEngine<C, T, X> {
        &self.inner.config.listbox
    }

    fn options(&self, this: &C) -> ValueOptions<T> {
        (self.inner.config.options)(this)
    }

    // The keys of `value`, worked out again only when it holds other values
    // or `item_to_value` changed.
    fn memo_for(&self, this: &C, value: &[T]) -> (Rc<[SharedString]>, Rc<HashSet<SharedString>>) {
        let item_to_value = self.options(this).item_to_value;
        if let Some(memo) = self.inner.keys_memo.borrow().as_ref()
            && same_fn(&memo.item_to_value, &item_to_value)
            && memo.values.as_slice() == value
        {
            return (memo.keys.clone(), memo.set.clone());
        }
        let keys: Rc<[SharedString]> = value
            .iter()
            .map(|item| self.listbox().key_of(this, item))
            .collect::<Vec<_>>()
            .into();
        let set: Rc<HashSet<SharedString>> = Rc::new(keys.iter().cloned().collect());
        *self.inner.keys_memo.borrow_mut() = Some(KeysMemo {
            values: value.to_vec(),
            item_to_value,
            keys: keys.clone(),
            set: set.clone(),
        });
        (keys, set)
    }

    /// The selected values, in order.
    pub fn values(&self, this: &C) -> Vec<T> {
        (self.inner.config.value)(this).to_vec()
    }

    /// The selected values' keys, in order.
    pub fn keys(&self, this: &C) -> Rc<[SharedString]> {
        let value = (self.inner.config.value)(this);
        self.memo_for(this, value).0
    }

    /// The keys of a value, for attributes that render from another value
    /// than the component's now (`keysOf`).
    pub fn keys_of(&self, this: &C, value: &[T]) -> Rc<[SharedString]> {
        self.memo_for(this, value).0
    }

    /// Whether an item is selected: by key, or by `is_item_equal_to_value`
    /// for values that are equal without being the same.
    pub fn is_selected(&self, this: &C, key: &str, item_value: Option<&T>) -> bool {
        let options = self.options(this);
        let value = (self.inner.config.value)(this);
        if let (Some(equal), Some(item_value)) = (&options.is_item_equal_to_value, item_value) {
            return value.iter().any(|value| equal(item_value, value));
        }
        self.memo_for(this, value).1.contains(key)
    }

    /// The text a value shows: from `item_to_label`, its item, or the value's
    /// key.
    pub fn label_of(&self, this: &C, value: &T) -> SharedString {
        if let Some(item_to_label) = self.options(this).item_to_label {
            return item_to_label(value);
        }
        let key = self.listbox().key_of(this, value);
        self.listbox().label_of(this, &key).unwrap_or(key)
    }

    /// Each value's label, the same `Rc` while nothing changed, so a part
    /// that compares it renders only for a change.
    pub fn labels(&self, this: &C) -> Rc<[SharedString]> {
        let next: Vec<SharedString> = (self.inner.config.value)(this)
            .iter()
            .map(|value| self.label_of(this, value))
            .collect();
        let mut last = self.inner.last_labels.borrow_mut();
        if next.as_slice() != &last[..] {
            *last = next.into();
        }
        last.clone()
    }

    /// Renders the component again once its items have told labels the
    /// value shows, batched, since a long list registers all at once
    /// (`queueLabels`). Only labels that changed render it again, as
    /// `store.set({ labels })` does: items call it each time they render.
    pub fn queue_labels(&self, cx: &App) {
        if self.inner.labels_queued.replace(true) {
            return;
        }
        let selection = self.clone();
        cx.spawn(async move |cx| {
            selection.inner.labels_queued.set(false);
            let version = selection.listbox().labels_version();
            if selection.inner.labels_seen.replace(version) != version {
                let entity = selection.inner.entity;
                cx.update(|cx| cx.notify(entity));
            }
        })
        .detach();
    }

    // The keys of the value whose items never rendered, so no label is known.
    fn unlabeled(&self, this: &C) -> Vec<SharedString> {
        self.keys(this)
            .iter()
            .filter(|key| self.listbox().label_of(this, key).is_none())
            .cloned()
            .collect()
    }

    /// Learns the labels of values whose items aren't rendered, like in a
    /// closed list, from what `find` knows of them, like the component's own
    /// data, so a label that changes while closed still shows (`learn`).
    /// Returns whether it learned one.
    pub fn learn(
        &self,
        this: &C,
        find: impl FnOnce(&HashSet<SharedString>) -> Vec<(T, SharedString)>,
        cx: &App,
    ) -> bool {
        if self.options(this).item_to_label.is_some() {
            return false;
        }
        let unrendered: HashSet<SharedString> = self
            .keys(this)
            .iter()
            .filter(|key| self.listbox().element(key).is_none())
            .cloned()
            .collect();
        if unrendered.is_empty() {
            return false;
        }
        let version = self.listbox().labels_version();
        for (value, label) in find(&unrendered) {
            let key = self.listbox().key_of(this, &value);
            self.listbox().remember(key, label, value, None);
        }
        let changed = self.listbox().labels_version() != version;
        if changed && self.inner.learned.get() {
            self.queue_labels(cx);
        }
        self.inner.learned.set(true);
        changed
    }

    /// Whether a closed list must render its items hidden for the value: for
    /// a label only its item knows, or for a value icon in the page, which
    /// copies its item's icon. Not for a label `learn` found.
    pub fn needs_items(&self, this: &C, has_value_icon: bool) -> bool {
        !self.unlabeled(this).is_empty() || (has_value_icon && !self.keys(this).is_empty())
    }

    /// The value with an item added, or removed when it's there, for
    /// `multiple`.
    pub fn toggled(&self, this: &C, key: &str, item_value: &T) -> Vec<T> {
        let values = self.values(this);
        if !self.is_selected(this, key, Some(item_value)) {
            let mut values = values;
            values.push(item_value.clone());
            return values;
        }
        values
            .into_iter()
            .filter(|value| self.listbox().key_of(this, value) != key)
            .collect()
    }

    /// The value without one item's key, for removing a chip.
    pub fn without(&self, this: &C, key: &str) -> Vec<T> {
        self.values(this)
            .into_iter()
            .filter(|value| self.listbox().key_of(this, value) != key)
            .collect()
    }

    /// What selecting nothing is: nothing, or an empty list with `multiple`.
    pub fn empty(&self, this: &C) -> ListValue<T> {
        ListValue::empty((self.inner.config.multiple)(this))
    }
}
