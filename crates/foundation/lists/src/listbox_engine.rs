//! Port of `foundation/lists/listbox-engine.ts`: the collection and highlight
//! engine behind Listbox, Select, Combobox, Autocomplete, Command and
//! TriggerMenu. It knows items from data or from the parts that registered,
//! in order, keeps every label it has seen, finds items by typing, and moves
//! the highlight with real focus or with an active descendant. It works on
//! indexes over the data, so a virtualized list never needs its items
//! rendered. Filtering plugs in from `listbox.rs`, so a list nobody types
//! into, like Listbox, runs without it ([`ListFiltering::none`]).
//!
//! The engine is a handle a component makes in its constructor and keeps in
//! a field, like the selection models. It reads the component through the
//! getters its config gives, so reads take the component, `this`, and calls
//! that move the highlight take it mutably, after a clone of the handle:
//!
//! ```ignore
//! let listbox = self.listbox.clone();
//! listbox.r#move(self, Step::By(1), Some(event.into()), window, cx);
//! ```
//!
//! Items register as they render, which is page order: the part that holds
//! them calls [`ListboxEngine::begin`], and each item calls
//! [`ListboxEngine::bind_item`] (or [`ListboxEngine::register`] and
//! [`ListboxEngine::track`]). An item that stops rendering has left. Once the
//! frame that rendered them is drawn, the engine settles, as the original's
//! microtasks after items bind: it hears which joined and which left, gives
//! focus to a highlighted item that rendered late, and drops the highlight of
//! one that left for good. Reads during a render see the items of the last
//! frame, as the original's do before its new items bind.

use std::cell::{Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Debug};
use std::ops::Deref;
use std::rc::{Rc, Weak};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    AnyWindowHandle, App, Bounds, Context, Element, ElementId, FocusHandle, GlobalElementId,
    InspectorElementId, LayoutId, ParentElement, Pixels, Refineable, ScrollHandle,
    SharedString, Style, StyleRefinement, Styled, Task, WeakEntity, Window, point, px,
};
use ui_sdk_focus::{Step, move_index, scroll_to_show};
use ui_sdk_kernel::dev::{self, Origin};
use ui_sdk_kernel::{InputEvent, Key, Part};
use ui_sdk_text::{Typeahead, collapse, js_trim};

use crate::async_options::AsyncOptionsStatus;

/// An item in a select or combobox, as data (`ListItem`). `X` holds a
/// component's own fields, like a command's shortcut (`CommandDataItem`),
/// and is `()` for none.
#[derive(Clone, Debug, PartialEq)]
pub struct ListItem<T = SharedString, X = ()> {
    /// Identifies the item, and is what choosing it reports. A string, or any
    /// value with `item_to_value` or an [`ItemKey`] of its own.
    pub value: T,
    /// The text shown and matched by typeahead and filtering. A select's
    /// trigger shows it once chosen.
    pub label: SharedString,
    /// More words filtering matches, like `["team"]` for a plan called Pro.
    pub keywords: Vec<SharedString>,
    /// Keeps the item from being chosen. The keyboard skips it, like in a
    /// native select.
    pub disabled: bool,
    /// A component's own fields, like a command's shortcut.
    pub extra: X,
}

impl<T, X: Default> ListItem<T, X> {
    pub fn new(value: T, label: impl Into<SharedString>) -> Self {
        Self {
            value,
            label: label.into(),
            keywords: Vec::new(),
            disabled: false,
            extra: X::default(),
        }
    }
}

impl<T, X> ListItem<T, X> {
    pub fn keywords<K: Into<SharedString>>(mut self, keywords: impl IntoIterator<Item = K>) -> Self {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn extra(mut self, extra: X) -> Self {
        self.extra = extra;
        self
    }
}

/// A plain string is its own value and label (`toListItem`).
impl<X: Default> From<&str> for ListItem<SharedString, X> {
    fn from(text: &str) -> Self {
        let text = SharedString::from(text.to_string());
        Self::new(text.clone(), text)
    }
}

impl<X: Default> From<String> for ListItem<SharedString, X> {
    fn from(text: String) -> Self {
        let text = SharedString::from(text);
        Self::new(text.clone(), text)
    }
}

impl<X: Default> From<SharedString> for ListItem<SharedString, X> {
    fn from(text: SharedString) -> Self {
        Self::new(text.clone(), text)
    }
}

/// A plain string as an item, its own value and label (`toListItem`).
pub fn to_list_item<X: Default>(text: impl Into<SharedString>) -> ListItem<SharedString, X> {
    ListItem::from(text.into())
}

/// A labelled group of items, as data (`ListGroup`).
#[derive(Clone, Debug, PartialEq)]
pub struct ListGroup<T = SharedString, X = ()> {
    /// The group's heading, which names it.
    pub label: Option<SharedString>,
    pub items: Vec<ListItem<T, X>>,
}

impl<T, X> ListGroup<T, X> {
    pub fn new<I: Into<ListItem<T, X>>>(items: impl IntoIterator<Item = I>) -> Self {
        Self {
            label: None,
            items: items.into_iter().map(Into::into).collect(),
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// A line between items, as data (`ListSeparator`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ListSeparator;

/// One entry of a list as data: an item, a labelled group or a separator.
#[derive(Clone, Debug, PartialEq)]
pub enum ListNode<T = SharedString, X = ()> {
    Item(ListItem<T, X>),
    Group(ListGroup<T, X>),
    Separator(ListSeparator),
}

impl<T, X> ListNode<T, X> {
    /// A line between items.
    pub fn separator() -> Self {
        ListNode::Separator(ListSeparator)
    }

    /// Whether it's a group (`isGroup`).
    pub fn is_group(&self) -> bool {
        matches!(self, ListNode::Group(_))
    }

    /// Whether it's a separator (`isSeparator`).
    pub fn is_separator(&self) -> bool {
        matches!(self, ListNode::Separator(_))
    }
}

impl<T, X> From<ListItem<T, X>> for ListNode<T, X> {
    fn from(item: ListItem<T, X>) -> Self {
        ListNode::Item(item)
    }
}

impl<T, X> From<ListGroup<T, X>> for ListNode<T, X> {
    fn from(group: ListGroup<T, X>) -> Self {
        ListNode::Group(group)
    }
}

impl<T, X> From<ListSeparator> for ListNode<T, X> {
    fn from(_: ListSeparator) -> Self {
        ListNode::Separator(ListSeparator)
    }
}

impl<X: Default> From<&str> for ListNode<SharedString, X> {
    fn from(text: &str) -> Self {
        ListNode::Item(text.into())
    }
}

impl<X: Default> From<String> for ListNode<SharedString, X> {
    fn from(text: String) -> Self {
        ListNode::Item(text.into())
    }
}

impl<X: Default> From<SharedString> for ListNode<SharedString, X> {
    fn from(text: SharedString) -> Self {
        ListNode::Item(text.into())
    }
}

/// Whether an entry of data is a group (`isGroup`).
pub fn is_group<T, X>(node: &ListNode<T, X>) -> bool {
    node.is_group()
}

/// Whether an entry of data is a separator (`isSeparator`).
pub fn is_separator<T, X>(node: &ListNode<T, X>) -> bool {
    node.is_separator()
}

/// A list as data: items, plain strings, groups and separators, the same
/// shape every component renders (`ListData`). Clones share their entries,
/// so the engine keeps what it worked out from data until it gets other
/// data: pass the same `ListData` again rather than building it anew each
/// render.
pub struct ListData<T = SharedString, X = ()>(Rc<[ListNode<T, X>]>);

impl<T, X> ListData<T, X> {
    pub fn new<N: Into<ListNode<T, X>>>(nodes: impl IntoIterator<Item = N>) -> Self {
        Self(nodes.into_iter().map(Into::into).collect::<Vec<_>>().into())
    }

    /// Data with no entries.
    pub fn empty() -> Self {
        Self(Vec::new().into())
    }

    /// Whether both are the same data, not only equal data, which is how the
    /// engine tells it already worked them out.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    pub fn nodes(&self) -> &[ListNode<T, X>] {
        &self.0
    }
}

impl<T, X> Clone for ListData<T, X> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T, X> Default for ListData<T, X> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T, X> Deref for ListData<T, X> {
    type Target = [ListNode<T, X>];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Debug, X: Debug> Debug for ListData<T, X> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.iter()).finish()
    }
}

impl<T: PartialEq, X: PartialEq> PartialEq for ListData<T, X> {
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other) || self.0[..] == other.0[..]
    }
}

impl<T, X> From<Vec<ListNode<T, X>>> for ListData<T, X> {
    fn from(nodes: Vec<ListNode<T, X>>) -> Self {
        Self(nodes.into())
    }
}

impl<T, X, N: Into<ListNode<T, X>>> FromIterator<N> for ListData<T, X> {
    fn from_iter<I: IntoIterator<Item = N>>(iter: I) -> Self {
        Self::new(iter)
    }
}

/// What a filter function says about an item: kept or not, or a score that
/// ranks it, best first, where 0 hides it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ListFilterResult {
    Keep(bool),
    Score(f64),
}

impl From<bool> for ListFilterResult {
    fn from(keep: bool) -> Self {
        ListFilterResult::Keep(keep)
    }
}

impl From<f64> for ListFilterResult {
    fn from(score: f64) -> Self {
        ListFilterResult::Score(score)
    }
}

impl From<f32> for ListFilterResult {
    fn from(score: f32) -> Self {
        ListFilterResult::Score(score as f64)
    }
}

/// A filter of your own: what it says about an item for the query.
pub type ListFilterFn<T, X> = Rc<dyn Fn(&ListItem<T, X>, &str) -> ListFilterResult>;

/// How typing narrows the list (`ListFilter`). `Contains` keeps items whose
/// label or keywords contain the text, and `StartsWith` those that start
/// with it, ignoring case and accents. A function decides for itself: true
/// or false, or a score above 0 that ranks the item, and 0 to hide it. `Off`
/// keeps every item, for results a server already filtered (`false`).
pub enum ListFilter<T = SharedString, X = ()> {
    Contains,
    StartsWith,
    Off,
    Custom(ListFilterFn<T, X>),
}

impl<T, X> ListFilter<T, X> {
    /// A filter of your own, returning true or false, or a score.
    pub fn custom<R: Into<ListFilterResult>>(filter: impl Fn(&ListItem<T, X>, &str) -> R + 'static) -> Self {
        ListFilter::Custom(Rc::new(move |item, query| filter(item, query).into()))
    }

    /// Whether it's a function, which ranks data items by its scores.
    pub fn is_custom(&self) -> bool {
        matches!(self, ListFilter::Custom(_))
    }
}

impl<T, X> Clone for ListFilter<T, X> {
    fn clone(&self) -> Self {
        match self {
            ListFilter::Contains => ListFilter::Contains,
            ListFilter::StartsWith => ListFilter::StartsWith,
            ListFilter::Off => ListFilter::Off,
            ListFilter::Custom(filter) => ListFilter::Custom(filter.clone()),
        }
    }
}

impl<T, X> Debug for ListFilter<T, X> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ListFilter::Contains => write!(f, "Contains"),
            ListFilter::StartsWith => write!(f, "StartsWith"),
            ListFilter::Off => write!(f, "Off"),
            ListFilter::Custom(_) => write!(f, "Custom(..)"),
        }
    }
}

/// The same filter: the same mode, or the same function.
impl<T, X> PartialEq for ListFilter<T, X> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ListFilter::Contains, ListFilter::Contains)
            | (ListFilter::StartsWith, ListFilter::StartsWith)
            | (ListFilter::Off, ListFilter::Off) => true,
            (ListFilter::Custom(a), ListFilter::Custom(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// An item the engine can navigate, from data or from a rendered part
/// (`ListEntry`).
#[derive(Clone, Debug, PartialEq)]
pub struct ListEntry<T = SharedString> {
    pub key: SharedString,
    /// What choosing it reports. `None` only for a built-in option, like a
    /// create or select-all option, which has no value of its own.
    pub value: Option<T>,
    pub label: SharedString,
    pub keywords: Vec<SharedString>,
    pub disabled: bool,
    /// The key of the group it's in.
    pub group: Option<SharedString>,
    /// Never filtered and never counted as a result, like a create or
    /// select-all option.
    pub pinned: bool,
}

/// A string for any value, which ids, form values and lookups use
/// (`itemKey`). Strings pass through and numbers write as JavaScript writes
/// them. A type of your own returns its id, as the original reads an
/// object's `value` or `id`; a component's `item_to_value` wins over it.
pub trait ItemKey {
    fn item_key(&self) -> SharedString;
}

impl ItemKey for str {
    fn item_key(&self) -> SharedString {
        SharedString::from(self.to_string())
    }
}

impl ItemKey for String {
    fn item_key(&self) -> SharedString {
        SharedString::from(self.clone())
    }
}

impl ItemKey for SharedString {
    fn item_key(&self) -> SharedString {
        self.clone()
    }
}

impl ItemKey for char {
    fn item_key(&self) -> SharedString {
        SharedString::from(self.to_string())
    }
}

impl ItemKey for bool {
    fn item_key(&self) -> SharedString {
        SharedString::new_static(if *self { "true" } else { "false" })
    }
}

macro_rules! integer_keys {
    ($($ty:ty),*) => {$(
        impl ItemKey for $ty {
            fn item_key(&self) -> SharedString {
                SharedString::from(self.to_string())
            }
        }
    )*};
}

integer_keys!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

// `String(number)`: whole numbers without a fraction, `NaN` and `Infinity`.
fn js_number(value: f64) -> SharedString {
    if value.is_nan() {
        return SharedString::new_static("NaN");
    }
    if value.is_infinite() {
        return SharedString::new_static(if value > 0.0 { "Infinity" } else { "-Infinity" });
    }
    if value == 0.0 {
        return SharedString::new_static("0");
    }
    SharedString::from(value.to_string())
}

impl ItemKey for f64 {
    fn item_key(&self) -> SharedString {
        js_number(*self)
    }
}

impl ItemKey for f32 {
    fn item_key(&self) -> SharedString {
        js_number(*self as f64)
    }
}

impl<K: ItemKey + ?Sized> ItemKey for &K {
    fn item_key(&self) -> SharedString {
        (**self).item_key()
    }
}

impl<K: ItemKey + ?Sized> ItemKey for Rc<K> {
    fn item_key(&self) -> SharedString {
        (**self).item_key()
    }
}

impl<K: ItemKey + ?Sized> ItemKey for Box<K> {
    fn item_key(&self) -> SharedString {
        (**self).item_key()
    }
}

impl<K: ItemKey + ?Sized> ItemKey for std::sync::Arc<K> {
    fn item_key(&self) -> SharedString {
        (**self).item_key()
    }
}

/// Nothing is the empty string.
impl<K: ItemKey> ItemKey for Option<K> {
    fn item_key(&self) -> SharedString {
        self.as_ref().map(ItemKey::item_key).unwrap_or_default()
    }
}

/// A string for any value (`itemKey`): `item_to_value`'s when there is one,
/// else the value's own [`ItemKey`].
pub fn item_key<T: ItemKey + ?Sized>(value: &T, item_to_value: Option<&dyn Fn(&T) -> SharedString>) -> SharedString {
    match item_to_value {
        Some(item_to_value) => item_to_value(value),
        None => value.item_key(),
    }
}

/// Every item in data, in order, with the group it's in (`flattenData`).
pub fn flatten_data<T: Clone, X>(data: &[ListNode<T, X>], to_key: &dyn Fn(&T) -> SharedString) -> Vec<ListEntry<T>> {
    let mut entries = Vec::new();
    for (index, node) in data.iter().enumerate() {
        match node {
            ListNode::Separator(_) => {}
            ListNode::Group(group) => {
                let key: SharedString = format!("group-{index}").into();
                for item in &group.items {
                    entries.push(entry_of(item, to_key, Some(key.clone())));
                }
            }
            ListNode::Item(item) => entries.push(entry_of(item, to_key, None)),
        }
    }
    entries
}

fn entry_of<T: Clone, X>(
    item: &ListItem<T, X>,
    to_key: &dyn Fn(&T) -> SharedString,
    group: Option<SharedString>,
) -> ListEntry<T> {
    ListEntry {
        key: to_key(&item.value),
        value: Some(item.value.clone()),
        label: item.label.clone(),
        keywords: item.keywords.clone(),
        disabled: item.disabled,
        group,
        pinned: false,
    }
}

/// The element a list's items scroll in, which the original finds by
/// walking up from an item: its scroll handle, and the scroll padding it
/// keeps clear at its top and bottom (`scroll-padding-top` and
/// `scroll-padding-bottom`). gpui can't find a scrolling ancestor, so the
/// component names it.
#[derive(Clone, Debug, Default)]
pub struct ListScroller {
    pub handle: ScrollHandle,
    pub padding_top: Pixels,
    pub padding_bottom: Pixels,
}

impl ListScroller {
    pub fn new(handle: ScrollHandle) -> Self {
        Self {
            handle,
            padding_top: px(0.),
            padding_bottom: px(0.),
        }
    }

    pub fn padding(mut self, top: impl Into<Pixels>, bottom: impl Into<Pixels>) -> Self {
        self.padding_top = top.into();
        self.padding_bottom = bottom.into();
        self
    }
}

impl From<ScrollHandle> for ListScroller {
    fn from(handle: ScrollHandle) -> Self {
        Self::new(handle)
    }
}

/// Scrolls an item into view in its list's scroller, honoring the scroll
/// padding, without scrolling anything around it the way `scrollIntoView`
/// would (`revealInList`). `item` is where the item last laid out. Returns
/// whether it scrolled; the scroll shows on the next frame.
pub fn reveal_in_list(item: Bounds<Pixels>, scroller: &ListScroller) -> bool {
    let handle = &scroller.handle;
    let max = handle.max_offset().y;
    if max <= px(0.) {
        return false;
    }
    let viewport = handle.bounds();
    let offset = handle.offset();
    let scroll_top = -offset.y;
    let top = item.top() - viewport.top() + scroll_top;
    let shown = scroll_to_show(
        scroll_top,
        viewport.size.height,
        top,
        item.size.height,
        scroller.padding_top,
        scroller.padding_bottom,
    );
    // The last option's padding asks past the end, which would write on every move.
    let next = if shown > max { max } else { shown };
    if next == scroll_top {
        return false;
    }
    handle.set_offset(point(offset.x, -next));
    true
}

/// What a rendered item says about itself as it registers (`ItemMeta`),
/// each render, so a changed label or disabled state applies at once. A
/// pinned entry, like a combobox's create option, is never filtered, and
/// comes after data items.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemMeta<T = SharedString> {
    /// What choosing it reports. `None` for a built-in option, like a create
    /// or select-all option ([`ItemMeta::built_in`]).
    pub value: Option<T>,
    /// Its own label (the `label` prop), which names it over its text.
    pub label: Option<SharedString>,
    /// The text it shows, which names it without a `label`, as the
    /// original reads its element's text (`labelOf`): without hidden parts
    /// like shortcuts, whitespace collapsed as the page shows it, or its
    /// `aria_label` when it shows none.
    pub text: Option<SharedString>,
    pub keywords: Vec<SharedString>,
    pub disabled: bool,
    pub group: Option<SharedString>,
    pub pinned: bool,
    /// Its place among all items, in a virtualized list.
    pub index: Option<usize>,
    /// It's a link (`a[href]`): Enter clicks it rather than choosing it, so
    /// it follows itself and its press chooses. gpui clicks it while it
    /// holds focus; one the list only points at, its component clicks
    /// (`part::press_of`, `el.click()`).
    pub link: bool,
}

impl<T> ItemMeta<T> {
    pub fn new(value: T) -> Self {
        Self::with_value(Some(value))
    }

    /// A built-in option, like a create or select-all option: pinned, never
    /// filtered and never counted as a result, with no value of its own.
    /// `text` is its label, for typeahead.
    pub fn built_in(text: Option<SharedString>) -> Self {
        Self {
            text,
            pinned: true,
            ..Self::with_value(None)
        }
    }

    fn with_value(value: Option<T>) -> Self {
        Self {
            value,
            label: None,
            text: None,
            keywords: Vec::new(),
            disabled: false,
            group: None,
            pinned: false,
            index: None,
            link: false,
        }
    }

    pub fn label(mut self, label: Option<SharedString>) -> Self {
        self.label = label;
        self
    }

    pub fn text(mut self, text: Option<SharedString>) -> Self {
        self.text = text;
        self
    }

    pub fn keywords<K: Into<SharedString>>(mut self, keywords: impl IntoIterator<Item = K>) -> Self {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn group(mut self, group: Option<SharedString>) -> Self {
        self.group = group;
        self
    }

    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    pub fn index(mut self, index: Option<usize>) -> Self {
        self.index = index;
        self
    }

    pub fn link(mut self, link: bool) -> Self {
        self.link = link;
        self
    }
}

// A label the engine remembers, after its item leaves too (`KnownItem`).
#[derive(Clone, Debug)]
struct KnownItem<T> {
    label: SharedString,
    value: T,
    keywords: Vec<SharedString>,
    group: Option<SharedString>,
}

/// What the empty part of a filtered list styles from: whether it shows,
/// and why (`ListEmptyState`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListEmptyState {
    /// Whether results are loading.
    pub loading: bool,
    /// Whether no item matches.
    pub empty: bool,
    /// The text the list is filtered by.
    pub query: SharedString,
}

/// What the status part of a filtered list styles from, for loading and
/// result counts (`ListStatusState`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListStatusState {
    /// Whether results are loading or failed.
    pub status: AsyncOptionsStatus,
    /// Whether results are loading.
    pub loading: bool,
    /// How many items are visible.
    pub count: usize,
}

/// What `on_highlight` hears besides the value: where the item is
/// (`ListHighlightDetails`).
#[derive(Clone, Debug, Default)]
pub struct ListHighlightDetails {
    /// The item's index among the visible items, for a virtualizer to scroll
    /// to. `None` when nothing is highlighted (`-1`).
    pub index: Option<usize>,
    /// The event behind it, if any.
    pub event: Option<InputEvent>,
}

/// How a list narrows its items: by a query with `listbox.rs`, or never with
/// [`ListFiltering::none`] (`ListFiltering`).
pub struct ListFiltering<T, X> {
    /// The data that shows for a query, ranked by score when the last
    /// argument says so.
    pub data: fn(&ListData<T, X>, &str, Option<&ListFilter<T, X>>, bool) -> ListData<T, X>,
    /// Whether an item shows for a query.
    pub matches: fn(&ListItem<T, X>, &str, Option<&ListFilter<T, X>>) -> bool,
}

impl<T, X> Clone for ListFiltering<T, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T, X> Copy for ListFiltering<T, X> {}

impl<T, X> ListFiltering<T, X> {
    /// For a list nobody types into, like Listbox: every item shows
    /// (Listbox's `noFiltering`).
    pub fn none() -> Self {
        fn data<T, X>(data: &ListData<T, X>, _: &str, _: Option<&ListFilter<T, X>>, _: bool) -> ListData<T, X> {
            data.clone()
        }
        fn matches<T, X>(_: &ListItem<T, X>, _: &str, _: Option<&ListFilter<T, X>>) -> bool {
            true
        }
        Self {
            data: data::<T, X>,
            matches: matches::<T, X>,
        }
    }
}

/// Where the highlight's focus goes: `Real` moves focus to items, `Virtual`
/// leaves it where it is, and the owner points an active descendant at the
/// highlighted item (`"real" | "virtual"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ListFocus {
    #[default]
    Real,
    Virtual,
}

/// Reads something from the component.
pub type ListReadFn<C, R> = Rc<dyn Fn(&C) -> R>;

/// Hears the highlight move: the key, its index among the visible items, and
/// the event behind it. It runs inside the component's update, with it.
pub type ListHighlightFn<C> =
    Rc<dyn Fn(&mut C, Option<&SharedString>, Option<usize>, Option<&InputEvent>, &mut Window, &mut Context<C>)>;

/// Hears about one item, inside the component's update, with it.
pub type ListKeyFn<C> = Rc<dyn Fn(&mut C, &SharedString, &mut Window, &mut Context<C>)>;

/// Runs inside the component's update, with it.
pub type ListHookFn<C> = Rc<dyn Fn(&mut C, &mut Window, &mut Context<C>)>;

/// What [`create_listbox_engine`] reads from its component. Getters read the
/// component (`this`) as it is when the engine asks; callbacks run inside
/// the component's update.
pub struct ListboxEngineConfig<C, T = SharedString, X = ()> {
    /// `Real` moves focus to items, `Virtual` leaves it where it is. Read
    /// each time, so it can change per open, like real focus for a touch
    /// screen reader. Defaults to `Real`.
    pub focus: ListReadFn<C, ListFocus>,
    /// The items as data, or `None` for the parts that register.
    pub items: ListReadFn<C, Option<ListData<T, X>>>,
    /// Data shown instead of `items` while the query is empty, like recent
    /// searches.
    pub empty_items: Option<ListReadFn<C, Option<ListData<T, X>>>>,
    /// Whether `empty_items` show now. By default, while the query is empty.
    pub show_empty: Option<ListReadFn<C, bool>>,
    /// The key of a value, `item_key` with the component's `item_to_value`.
    pub to_key: Rc<dyn Fn(&C, &T) -> SharedString>,
    /// How the filtering narrows the list. `None` keeps every item.
    pub filter: ListReadFn<C, Option<ListFilter<T, X>>>,
    /// The text to filter by. Empty shows everything.
    pub query: ListReadFn<C, SharedString>,
    /// Whether data items are sorted by how well they match, best first.
    pub rank: Option<ListReadFn<C, bool>>,
    /// Whether the arrows wrap around at either end. Defaults to true.
    pub r#loop: ListReadFn<C, bool>,
    /// Whether keys pass over disabled items, like a native select.
    pub skip_disabled: bool,
    /// Whether highlighting may move focus now, like while the popup is
    /// open. Defaults to true.
    pub active: ListReadFn<C, bool>,
    /// An item can't be chosen for a reason beyond its own, like a full
    /// multiple selection.
    pub blocked: Option<Rc<dyn Fn(&C, &ListEntry<T>) -> bool>>,
    /// Tells a virtualizer which index to reveal.
    pub on_highlight: Option<ListHighlightFn<C>>,
    /// The list's focus handle, which takes focus back when its focused item
    /// leaves.
    pub container: Option<ListReadFn<C, Option<FocusHandle>>>,
    /// The scroller items reveal in, like the popup content around the list,
    /// so nothing around it scrolls. The original finds the item's scrolling
    /// ancestor up to the container; without one here, highlighting scrolls
    /// nothing.
    pub scroll_root: Option<ListReadFn<C, Option<ListScroller>>>,
    /// Whether highlighting scrolls the item into view. Defaults to true.
    pub keep_in_view: Option<ListReadFn<C, bool>>,
    /// Runs when an item joins or leaves, like a highlighted one a
    /// virtualizer just rendered, so parts that depend on the list can
    /// update. It runs once the frame that rendered the items is drawn.
    pub on_register: Option<ListKeyFn<C>>,
    /// Runs when the highlight reaches the last item, so the next page can
    /// load.
    pub on_last_reached: Option<ListHookFn<C>>,
}

impl<C, T, X> Clone for ListboxEngineConfig<C, T, X> {
    fn clone(&self) -> Self {
        Self {
            focus: self.focus.clone(),
            items: self.items.clone(),
            empty_items: self.empty_items.clone(),
            show_empty: self.show_empty.clone(),
            to_key: self.to_key.clone(),
            filter: self.filter.clone(),
            query: self.query.clone(),
            rank: self.rank.clone(),
            r#loop: self.r#loop.clone(),
            skip_disabled: self.skip_disabled,
            active: self.active.clone(),
            blocked: self.blocked.clone(),
            on_highlight: self.on_highlight.clone(),
            container: self.container.clone(),
            scroll_root: self.scroll_root.clone(),
            keep_in_view: self.keep_in_view.clone(),
            on_register: self.on_register.clone(),
            on_last_reached: self.on_last_reached.clone(),
        }
    }
}

impl<C: 'static, T: 'static, X: 'static> ListboxEngineConfig<C, T, X> {
    /// A config with `to_key`, real focus, items from the parts that
    /// register, no filter, no query, looping, and always active.
    pub fn new(to_key: impl Fn(&C, &T) -> SharedString + 'static) -> Self {
        Self {
            focus: Rc::new(|_| ListFocus::Real),
            items: Rc::new(|_| None),
            empty_items: None,
            show_empty: None,
            to_key: Rc::new(to_key),
            filter: Rc::new(|_| None),
            query: Rc::new(|_| SharedString::default()),
            rank: None,
            r#loop: Rc::new(|_| true),
            skip_disabled: false,
            active: Rc::new(|_| true),
            blocked: None,
            on_highlight: None,
            container: None,
            scroll_root: None,
            keep_in_view: None,
            on_register: None,
            on_last_reached: None,
        }
    }

    pub fn focus(mut self, focus: impl Fn(&C) -> ListFocus + 'static) -> Self {
        self.focus = Rc::new(focus);
        self
    }

    pub fn items(mut self, items: impl Fn(&C) -> Option<ListData<T, X>> + 'static) -> Self {
        self.items = Rc::new(items);
        self
    }

    pub fn empty_items(mut self, items: impl Fn(&C) -> Option<ListData<T, X>> + 'static) -> Self {
        self.empty_items = Some(Rc::new(items));
        self
    }

    pub fn show_empty(mut self, show: impl Fn(&C) -> bool + 'static) -> Self {
        self.show_empty = Some(Rc::new(show));
        self
    }

    pub fn filter(mut self, filter: impl Fn(&C) -> Option<ListFilter<T, X>> + 'static) -> Self {
        self.filter = Rc::new(filter);
        self
    }

    pub fn query(mut self, query: impl Fn(&C) -> SharedString + 'static) -> Self {
        self.query = Rc::new(query);
        self
    }

    pub fn rank(mut self, rank: impl Fn(&C) -> bool + 'static) -> Self {
        self.rank = Some(Rc::new(rank));
        self
    }

    pub fn r#loop(mut self, r#loop: impl Fn(&C) -> bool + 'static) -> Self {
        self.r#loop = Rc::new(r#loop);
        self
    }

    pub fn skip_disabled(mut self, skip: bool) -> Self {
        self.skip_disabled = skip;
        self
    }

    pub fn active(mut self, active: impl Fn(&C) -> bool + 'static) -> Self {
        self.active = Rc::new(active);
        self
    }

    pub fn blocked(mut self, blocked: impl Fn(&C, &ListEntry<T>) -> bool + 'static) -> Self {
        self.blocked = Some(Rc::new(blocked));
        self
    }

    pub fn on_highlight(
        mut self,
        callback: impl Fn(&mut C, Option<&SharedString>, Option<usize>, Option<&InputEvent>, &mut Window, &mut Context<C>)
        + 'static,
    ) -> Self {
        self.on_highlight = Some(Rc::new(callback));
        self
    }

    pub fn container(mut self, container: impl Fn(&C) -> Option<FocusHandle> + 'static) -> Self {
        self.container = Some(Rc::new(container));
        self
    }

    pub fn scroll_root(mut self, scroll_root: impl Fn(&C) -> Option<ListScroller> + 'static) -> Self {
        self.scroll_root = Some(Rc::new(scroll_root));
        self
    }

    pub fn keep_in_view(mut self, keep: impl Fn(&C) -> bool + 'static) -> Self {
        self.keep_in_view = Some(Rc::new(keep));
        self
    }

    pub fn on_register(
        mut self,
        callback: impl Fn(&mut C, &SharedString, &mut Window, &mut Context<C>) + 'static,
    ) -> Self {
        self.on_register = Some(Rc::new(callback));
        self
    }

    pub fn on_last_reached(mut self, callback: impl Fn(&mut C, &mut Window, &mut Context<C>) + 'static) -> Self {
        self.on_last_reached = Some(Rc::new(callback));
        self
    }
}

/// How [`ListboxEngine::highlight`] moves the highlight: with focus, the
/// default, for real focus, and with the event behind it.
#[derive(Clone, Debug)]
pub struct HighlightOptions {
    /// Whether real focus follows. Focus landing on an item another way,
    /// like a press, highlights it without moving focus again.
    pub focus: bool,
    pub event: Option<InputEvent>,
}

impl Default for HighlightOptions {
    fn default() -> Self {
        Self {
            focus: true,
            event: None,
        }
    }
}

impl HighlightOptions {
    /// Moves focus with it, for `event`.
    pub fn event(event: Option<InputEvent>) -> Self {
        Self { focus: true, event }
    }

    /// Leaves focus where it is.
    pub fn without_focus(event: Option<InputEvent>) -> Self {
        Self { focus: false, event }
    }
}

/// A rendered item as the list knows it (`element(key)`): the focus handle
/// it tracks, where it last laid out, and whether it's in the page.
#[derive(Clone, Debug)]
pub struct ListElement {
    pub key: SharedString,
    /// The focus handle made for it with [`ListboxEngine::focus_handle`].
    pub handle: Option<FocusHandle>,
    /// Its bounds as of its last layout, once [`ListboxEngine::track`] is on
    /// its element.
    pub bounds: Option<Bounds<Pixels>>,
    /// It's a link (`a[href]`).
    pub link: bool,
    /// It rendered last frame (`isConnected`).
    pub connected: bool,
}

/// The data to render for a query, with its items flattened
/// ([`ListboxEngine::visible_data`]).
pub struct VisibleData<T, X> {
    /// The data that matches, groups and separators included.
    pub list: ListData<T, X>,
    /// Its items, in order.
    pub flat: Rc<[ListEntry<T>]>,
}

impl<T, X> Clone for VisibleData<T, X> {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            flat: self.flat.clone(),
        }
    }
}

/// Everything a listing depends on besides a component's own state
/// ([`ListboxEngine::listing_inputs`]). The listing changes only when one of
/// these does.
pub struct ListingInputs<T, X> {
    count: u64,
    labels: u64,
    query: SharedString,
    filter: Option<ListFilter<T, X>>,
    items: Option<ListData<T, X>>,
    empty_items: Option<ListData<T, X>>,
    show_empty: Option<bool>,
    rank: Option<bool>,
}

impl<T, X> Clone for ListingInputs<T, X> {
    fn clone(&self) -> Self {
        Self {
            count: self.count,
            labels: self.labels,
            query: self.query.clone(),
            filter: self.filter.clone(),
            items: self.items.clone(),
            empty_items: self.empty_items.clone(),
            show_empty: self.show_empty,
            rank: self.rank,
        }
    }
}

// Data compares by identity, as the original's `sameValue` does.
fn same_data<T, X>(a: &Option<ListData<T, X>>, b: &Option<ListData<T, X>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.ptr_eq(b),
        _ => false,
    }
}

impl<T, X> PartialEq for ListingInputs<T, X> {
    fn eq(&self, other: &Self) -> bool {
        self.count == other.count
            && self.labels == other.labels
            && self.query == other.query
            && self.filter == other.filter
            && same_data(&self.items, &other.items)
            && same_data(&self.empty_items, &other.empty_items)
            && self.show_empty == other.show_empty
            && self.rank == other.rank
    }
}

// A data list, flattened and looked up by key (`DataMemo`).
struct DataView<T, X> {
    data: ListData<T, X>,
    flat: Rc<[ListEntry<T>]>,
    by_key: Rc<HashMap<SharedString, usize>>,
}

impl<T, X> Clone for DataView<T, X> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            flat: self.flat.clone(),
            by_key: self.by_key.clone(),
        }
    }
}

impl<T: Clone, X> DataView<T, X> {
    fn entry(&self, key: &str) -> Option<ListEntry<T>> {
        self.by_key.get(key).map(|&index| self.flat[index].clone())
    }
}

struct VisibleMemo<T, X> {
    data: ListData<T, X>,
    query: SharedString,
    filter: Option<ListFilter<T, X>>,
    rank: bool,
    view: VisibleData<T, X>,
}

struct ComposedMemo<T, X> {
    version: u64,
    query: SharedString,
    filter: Option<ListFilter<T, X>>,
    labels: u64,
    list: Rc<[ListEntry<T>]>,
}

// Data entries with the pinned ones around them.
struct PinnedMemo<T> {
    flat: Rc<[ListEntry<T>]>,
    version: u64,
    labels: u64,
    list: Rc<[ListEntry<T>]>,
}

#[derive(Clone)]
struct Registered<T> {
    key: SharedString,
    meta: ItemMeta<T>,
}

struct State<T, X> {
    highlighted: Option<SharedString>,
    // Rendered items in page order, as the last settled frame drew them.
    registry: Vec<Registered<T>>,
    index: HashMap<SharedString, usize>,
    // The render of the items under way.
    pass: Option<Vec<Registered<T>>>,
    pass_index: HashMap<SharedString, usize>,
    // How many times each key registered during the pass, which can span
    // more than one render.
    pass_counts: HashMap<SharedString, usize>,
    // Every label seen, kept after its item leaves, so a chosen item's label
    // survives a list that no longer has it, like a new page of async results.
    known: HashMap<SharedString, KnownItem<T>>,
    // Items that asked whether they match, with what they asked with. An
    // item asks as it renders, before it registers, so the listing has its
    // entry even before then.
    asked: HashMap<SharedString, ItemMeta<T>>,
    // Keys of entries that are never filtered, like a create option, in the
    // order they joined.
    pinned: Vec<SharedString>,
    typeahead: Typeahead,
    data_memo: Option<DataView<T, X>>,
    empty_memo: Option<DataView<T, X>>,
    visible_memo: Option<VisibleMemo<T, X>>,
    composed_memo: Option<ComposedMemo<T, X>>,
    pinned_memo: Option<PinnedMemo<T>>,
    labels_version: u64,
    // Counts items joining and leaving, so the listing knows when to look
    // again.
    mounts_version: u64,
    // Counts changes to the registry's order or its items' props, which the
    // composed entries are kept for.
    registry_version: u64,
    handles: HashMap<SharedString, FocusHandle>,
}

impl<T: Clone + PartialEq, X> State<T, X> {
    // Starts a render of the items unless one is under way. Says whether it
    // did, so its settling is asked for once.
    fn open_pass(&mut self) -> bool {
        if self.pass.is_some() {
            return false;
        }
        self.pass = Some(Vec::new());
        self.pass_index.clear();
        self.pass_counts.clear();
        true
    }

    // The props an item rendered with last: the render under way, else the
    // last frame's.
    fn meta(&self, key: &str) -> Option<&ItemMeta<T>> {
        if let (Some(pass), Some(&index)) = (&self.pass, self.pass_index.get(key)) {
            return Some(&pass[index].meta);
        }
        self.index.get(key).map(|&index| &self.registry[index].meta)
    }

    fn in_registry(&self, key: &str) -> bool {
        self.index.contains_key(key) || self.pass_index.contains_key(key)
    }

    fn reindex(&mut self) {
        self.index = self
            .registry
            .iter()
            .enumerate()
            .map(|(index, registered)| (registered.key.clone(), index))
            .collect();
    }

    // A rendered item's label: its own, or its text now, since parts inside
    // it, like an indicator, may hide themselves after it renders.
    fn live_label(&mut self, key: &SharedString, meta: &ItemMeta<T>) -> SharedString {
        let label = meta.label.clone().or_else(|| meta.text.clone()).unwrap_or_default();
        // Pinned entries, like a create option, aren't items to remember.
        if meta.pinned {
            return label;
        }
        let changed = self.known.get(key).is_none_or(|known| known.label != label);
        if changed && let Some(value) = meta.value.clone() {
            let previous = self.known.remove(key);
            self.known.insert(
                key.clone(),
                KnownItem {
                    label: label.clone(),
                    value,
                    keywords: previous.as_ref().map(|known| known.keywords.clone()).unwrap_or_default(),
                    group: previous.and_then(|known| known.group),
                },
            );
            self.labels_version += 1;
        }
        label
    }

    fn registered_entry(&mut self, key: &SharedString, meta: &ItemMeta<T>) -> ListEntry<T> {
        ListEntry {
            key: key.clone(),
            value: meta.value.clone(),
            label: self.live_label(key, meta),
            keywords: meta.keywords.clone(),
            disabled: meta.disabled,
            group: meta.group.clone(),
            pinned: meta.pinned,
        }
    }
}

// What an item's element tells the list while it renders: that it's in the
// page, and where it laid out.
struct Live {
    counts: RefCell<HashMap<SharedString, usize>>,
    bounds: RefCell<HashMap<SharedString, Bounds<Pixels>>>,
    departures: UnboundedSender<SharedString>,
}

impl Live {
    fn connected(&self, key: &str) -> bool {
        self.counts.borrow().get(key).is_some_and(|count| *count > 0)
    }
}

// An item's place in the page, kept as the element state of its probe: gpui
// drops it once a frame doesn't draw the item, which tells the list the item
// left, even when the whole list stopped rendering.
struct ProbeToken {
    live: Weak<Live>,
    key: SharedString,
}

impl ProbeToken {
    fn new(live: &Rc<Live>, key: SharedString) -> Self {
        if let Ok(mut counts) = live.counts.try_borrow_mut() {
            *counts.entry(key.clone()).or_default() += 1;
        }
        Self {
            live: Rc::downgrade(live),
            key,
        }
    }
}

impl Drop for ProbeToken {
    fn drop(&mut self) {
        let Some(live) = self.live.upgrade() else {
            return;
        };
        let gone = match live.counts.try_borrow_mut() {
            Ok(mut counts) => match counts.get_mut(&self.key) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                Some(_) => {
                    counts.remove(&self.key);
                    true
                }
                None => false,
            },
            Err(_) => false,
        };
        if gone {
            if let Ok(mut bounds) = live.bounds.try_borrow_mut() {
                bounds.remove(&self.key);
            }
            live.departures.unbounded_send(self.key.clone()).ok();
        }
    }
}

// What `track` puts in an item's element: it takes no space and draws
// nothing.
struct ItemProbe {
    id: ElementId,
    key: SharedString,
    live: Rc<Live>,
    style: StyleRefinement,
}

impl Styled for ItemProbe {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Element for ItemProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        if let Some(id) = id {
            let live = self.live.clone();
            let key = self.key.clone();
            window.with_element_state::<ProbeToken, _>(id, |token, _window| {
                ((), token.unwrap_or_else(|| ProbeToken::new(&live, key)))
            });
        }
        let mut style = Style::default();
        style.refine(&self.style);
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        if let Ok(mut all) = self.live.bounds.try_borrow_mut() {
            all.insert(self.key.clone(), bounds);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

struct Inner<C: 'static, T: 'static, X: 'static> {
    config: ListboxEngineConfig<C, T, X>,
    filtering: ListFiltering<T, X>,
    entity: WeakEntity<C>,
    window: AnyWindowHandle,
    state: RefCell<State<T, X>>,
    live: Rc<Live>,
    // Runs after every settle, for what a behavior learns from where focus
    // landed.
    settled: RefCell<Option<ListHookFn<C>>>,
    _departures: RefCell<Option<Task<()>>>,
}

/// The collection and highlight engine (`createListboxEngine`'s return,
/// which the original calls `Listbox`). Cloning it gives another handle to
/// the same engine.
pub struct ListboxEngine<C: 'static, T: 'static = SharedString, X: 'static = ()> {
    inner: Rc<Inner<C, T, X>>,
}

/// The original's name for the engine's type.
pub type Listbox<C, T = SharedString, X = ()> = ListboxEngine<C, T, X>;

impl<C: 'static, T: 'static, X: 'static> Clone for ListboxEngine<C, T, X> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<C: 'static, T: 'static, X: 'static> ListboxEngine<C, T, X> {
    /// A handle that doesn't keep the engine alive, for what the engine's
    /// own callbacks hold.
    pub fn downgrade(&self) -> WeakListboxEngine<C, T, X> {
        WeakListboxEngine {
            inner: Rc::downgrade(&self.inner),
        }
    }
}

/// A [`ListboxEngine`] held weakly.
pub struct WeakListboxEngine<C: 'static, T: 'static = SharedString, X: 'static = ()> {
    inner: Weak<Inner<C, T, X>>,
}

impl<C: 'static, T: 'static, X: 'static> Clone for WeakListboxEngine<C, T, X> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<C: 'static, T: 'static, X: 'static> WeakListboxEngine<C, T, X> {
    /// The engine, while it's alive.
    pub fn upgrade(&self) -> Option<ListboxEngine<C, T, X>> {
        self.inner.upgrade().map(|inner| ListboxEngine { inner })
    }
}

impl<C: 'static, T: Debug + 'static, X: 'static> Debug for ListboxEngine<C, T, X> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.inner.state.borrow();
        f.debug_struct("ListboxEngine")
            .field("highlighted", &state.highlighted)
            .field("items", &state.registry.iter().map(|item| &item.key).collect::<Vec<_>>())
            .finish()
    }
}

/// The engine for a component, `cx` being the component's, which callbacks
/// run with, and `window` the one it renders in (`createListboxEngine`).
/// `filtering` is how a query narrows the list: [`crate::create_filtered_listbox`]
/// passes the shared filtering, and a list nobody types into passes
/// [`ListFiltering::none`].
pub fn create_listbox_engine<C, T, X>(
    config: ListboxEngineConfig<C, T, X>,
    filtering: ListFiltering<T, X>,
    window: &Window,
    cx: &mut Context<C>,
) -> ListboxEngine<C, T, X>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    let (departures, mut receiver) = unbounded::<SharedString>();
    let inner = Rc::new(Inner {
        config,
        filtering,
        entity: cx.weak_entity(),
        window: window.window_handle(),
        state: RefCell::new(State {
            highlighted: None,
            registry: Vec::new(),
            index: HashMap::new(),
            pass: None,
            pass_index: HashMap::new(),
            pass_counts: HashMap::new(),
            known: HashMap::new(),
            asked: HashMap::new(),
            pinned: Vec::new(),
            typeahead: Typeahead::default(),
            data_memo: None,
            empty_memo: None,
            visible_memo: None,
            composed_memo: None,
            pinned_memo: None,
            labels_version: 0,
            mounts_version: 0,
            registry_version: 0,
            handles: HashMap::new(),
        }),
        live: Rc::new(Live {
            counts: RefCell::new(HashMap::new()),
            bounds: RefCell::new(HashMap::new()),
            departures,
        }),
        settled: RefCell::new(None),
        _departures: RefCell::new(None),
    });
    let weak = Rc::downgrade(&inner);
    let window_handle = inner.window;
    let task = cx.spawn(async move |_, cx| {
        while let Some(first) = receiver.next().await {
            let mut keys = vec![first];
            while let Ok(key) = receiver.try_recv() {
                keys.push(key);
            }
            let Some(inner) = weak.upgrade() else {
                break;
            };
            let engine = ListboxEngine { inner };
            window_handle
                .update(cx, |_, window, cx| engine.departed(keys, window, cx))
                .ok();
        }
    });
    *inner._departures.borrow_mut() = Some(task);
    ListboxEngine { inner }
}

impl<C, T, X> ListboxEngine<C, T, X>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    fn state(&self) -> Ref<'_, State<T, X>> {
        self.inner.state.borrow()
    }

    fn state_mut(&self) -> RefMut<'_, State<T, X>> {
        self.inner.state.borrow_mut()
    }

    fn config(&self) -> &ListboxEngineConfig<C, T, X> {
        &self.inner.config
    }

    /// The key of a value, through the component's `to_key`.
    pub fn key_of(&self, this: &C, value: &T) -> SharedString {
        (self.inner.config.to_key)(this, value)
    }

    /// The highlighted item's key (`store.get().highlighted`).
    pub fn highlighted(&self) -> Option<SharedString> {
        self.state().highlighted.clone()
    }

    /// Writes the highlight with nothing else, as a component writes its own
    /// state (`store.set({ highlighted })`), like clearing it as its popup
    /// closes. Returns whether it changed, so the component notifies.
    pub fn set_highlighted(&self, key: Option<SharedString>) -> bool {
        let mut state = self.state_mut();
        if state.highlighted == key {
            return false;
        }
        state.highlighted = key;
        true
    }

    fn memo_of(&self, this: &C, data: ListData<T, X>) -> DataView<T, X> {
        let to_key = &self.inner.config.to_key;
        let flat = flatten_data(&data, &|value| to_key(this, value));
        let by_key = flat
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.key.clone(), index))
            .collect();
        DataView {
            data,
            flat: flat.into(),
            by_key: Rc::new(by_key),
        }
    }

    fn data(&self, this: &C) -> Option<DataView<T, X>> {
        let items = (self.config().items)(this)?;
        if let Some(memo) = &self.state().data_memo
            && memo.data.ptr_eq(&items)
        {
            return Some(memo.clone());
        }
        let memo = self.memo_of(this, items);
        self.state_mut().data_memo = Some(memo.clone());
        Some(memo)
    }

    fn empty_source(&self, this: &C) -> Option<DataView<T, X>> {
        let items = self.config().empty_items.as_ref().and_then(|items| items(this))?;
        if let Some(memo) = &self.state().empty_memo
            && memo.data.ptr_eq(&items)
        {
            return Some(memo.clone());
        }
        let memo = self.memo_of(this, items);
        self.state_mut().empty_memo = Some(memo.clone());
        Some(memo)
    }

    // The data shown while the query is empty, when there's any. It needs
    // `items` too.
    fn empty_data(&self, this: &C) -> Option<DataView<T, X>> {
        let config = self.config();
        if (config.items)(this).is_none() {
            return None;
        }
        let show = match &config.show_empty {
            Some(show) => show(this),
            None => js_trim(&(config.query)(this)).is_empty(),
        };
        if !show {
            return None;
        }
        self.empty_source(this)
    }

    // A data item by key, among the items and the ones shown while the
    // query is empty.
    fn data_entry(&self, this: &C, key: &str) -> Option<ListEntry<T>> {
        self.data(this)
            .and_then(|data| data.entry(key))
            .or_else(|| self.empty_source(this).and_then(|data| data.entry(key)))
    }

    /// The data to render: the items that match the current query, or the
    /// ones shown while it's empty. `None` for items from parts.
    pub fn visible_data(&self, this: &C) -> Option<VisibleData<T, X>> {
        let source = self.empty_data(this).or_else(|| self.data(this))?;
        let config = self.config();
        let query = (config.query)(this);
        let filter = (config.filter)(this);
        let rank = config.rank.as_ref().is_some_and(|rank| rank(this));
        if let Some(memo) = &self.state().visible_memo
            && memo.data.ptr_eq(&source.data)
            && memo.query == query
            && memo.filter == filter
            && memo.rank == rank
        {
            return Some(memo.view.clone());
        }
        let list = (self.inner.filtering.data)(&source.data, &query, filter.as_ref(), rank);
        let flat: Rc<[ListEntry<T>]> = if list.ptr_eq(&source.data) {
            source.flat.clone()
        } else {
            let to_key = &config.to_key;
            flatten_data(&list, &|value| to_key(this, value)).into()
        };
        let view = VisibleData { list, flat };
        self.state_mut().visible_memo = Some(VisibleMemo {
            data: source.data,
            query,
            filter,
            rank,
            view: view.clone(),
        });
        Some(view)
    }

    // Whether an item matches the query the way the filtering asks.
    fn item_matches(
        &self,
        value: &T,
        label: &SharedString,
        keywords: &[SharedString],
        query: &str,
        filter: Option<&ListFilter<T, X>>,
    ) -> bool {
        let item = ListItem {
            value: value.clone(),
            label: label.clone(),
            keywords: keywords.to_vec(),
            disabled: false,
            extra: X::default(),
        };
        (self.inner.filtering.matches)(&item, query, filter)
    }

    // Whether a composed item matches the query. Its label comes from its
    // props or from its text when it last rendered.
    fn composed_matches(&self, this: &C, key: &SharedString, meta: &ItemMeta<T>) -> bool {
        // Built-in options are never filtered.
        let Some(value) = meta.value.as_ref() else {
            return true;
        };
        let label = match meta.label.clone().or_else(|| meta.text.clone()) {
            Some(label) => label,
            None => {
                let registered = self.state().meta(key).cloned();
                match registered {
                    Some(registered) => self.state_mut().live_label(key, &registered),
                    None => {
                        let known = self.state().known.get(key).map(|known| known.label.clone());
                        known.unwrap_or_else(|| key.clone())
                    }
                }
            }
        };
        let config = self.config();
        let query = (config.query)(this);
        let filter = (config.filter)(this);
        self.item_matches(value, &label, &meta.keywords, &query, filter.as_ref())
    }

    // Data items that match, with pinned entries that are in the page where
    // they sit: before the items, like a select-all option, or after them,
    // like a create option.
    fn data_entries(&self, flat: Rc<[ListEntry<T>]>) -> Rc<[ListEntry<T>]> {
        {
            let state = self.state();
            if state.pinned.is_empty() {
                return flat;
            }
            if let Some(memo) = &state.pinned_memo
                && Rc::ptr_eq(&memo.flat, &flat)
                && memo.version == state.registry_version
                && memo.labels == state.labels_version
            {
                return memo.list.clone();
            }
        }
        let live = &self.inner.live;
        let (pinned, first) = {
            let state = self.state();
            let first = flat
                .first()
                .filter(|entry| live.connected(&entry.key))
                .and_then(|entry| state.index.get(&entry.key).copied());
            let pinned: Vec<(usize, Registered<T>)> = state
                .pinned
                .iter()
                .filter_map(|key| {
                    let index = *state.index.get(key)?;
                    live.connected(key).then(|| (index, state.registry[index].clone()))
                })
                .collect();
            (pinned, first)
        };
        let mut before = Vec::new();
        let mut after = Vec::new();
        for (index, registered) in pinned {
            let entry = self.state_mut().registered_entry(&registered.key, &registered.meta);
            if first.is_some_and(|first| index < first) {
                before.push(entry);
            } else {
                after.push(entry);
            }
        }
        let list: Rc<[ListEntry<T>]> = if before.is_empty() && after.is_empty() {
            flat.clone()
        } else {
            before
                .into_iter()
                .chain(flat.iter().cloned())
                .chain(after)
                .collect::<Vec<_>>()
                .into()
        };
        let mut state = self.state_mut();
        state.pinned_memo = Some(PinnedMemo {
            flat,
            version: state.registry_version,
            labels: state.labels_version,
            list: list.clone(),
        });
        list
    }

    // Rendered items that match, kept until the items, the query or a label
    // change.
    fn composed_entries(&self, this: &C) -> Rc<[ListEntry<T>]> {
        let config = self.config();
        let query = (config.query)(this);
        let filter = (config.filter)(this);
        let registered = {
            let state = self.state();
            if let Some(memo) = &state.composed_memo
                && memo.version == state.registry_version
                && memo.query == query
                && memo.filter == filter
                && memo.labels == state.labels_version
            {
                return memo.list.clone();
            }
            state.registry.clone()
        };
        let mut list = Vec::with_capacity(registered.len());
        for item in &registered {
            let entry = self.state_mut().registered_entry(&item.key, &item.meta);
            let matched = match &entry.value {
                Some(value) => self.item_matches(value, &entry.label, &entry.keywords, &query, filter.as_ref()),
                None => true,
            };
            if item.meta.pinned || matched {
                list.push(entry);
            }
        }
        let list: Rc<[ListEntry<T>]> = list.into();
        let mut state = self.state_mut();
        state.composed_memo = Some(ComposedMemo {
            version: state.registry_version,
            query,
            filter,
            labels: state.labels_version,
            list: list.clone(),
        });
        list
    }

    /// The items a user can move through, in order: data items that match,
    /// or rendered items that match.
    pub fn entries(&self, this: &C) -> Rc<[ListEntry<T>]> {
        match self.visible_data(this) {
            Some(visible) => self.data_entries(visible.flat),
            None => self.composed_entries(this),
        }
    }

    /// Every data item, whatever the query. `None` for items from parts.
    pub fn data_items(&self, this: &C) -> Option<Rc<[ListEntry<T>]>> {
        self.data(this).map(|data| data.flat)
    }

    /// Whether an entry can't be chosen: disabled itself, or `blocked`.
    pub fn disabled(&self, this: &C, entry: &ListEntry<T>) -> bool {
        entry.disabled || self.config().blocked.as_ref().is_some_and(|blocked| blocked(this, entry))
    }

    /// An item by key: from data, a rendered item, or one whose label was
    /// remembered, like a chosen item that left the list.
    pub fn find(&self, this: &C, key: Option<&str>) -> Option<ListEntry<T>> {
        let key = key?;
        if let Some(entry) = self.data_entry(this, key) {
            return Some(entry);
        }
        let registered = self.state().meta(key).cloned();
        if let Some(meta) = registered {
            let key = SharedString::from(key.to_string());
            return Some(self.state_mut().registered_entry(&key, &meta));
        }
        let state = self.state();
        state.known.get(key).map(|known| ListEntry {
            key: SharedString::from(key.to_string()),
            value: Some(known.value.clone()),
            label: known.label.clone(),
            keywords: Vec::new(),
            disabled: false,
            group: None,
            pinned: false,
        })
    }

    /// Whether an item is disabled by itself, which keys and the pointer
    /// pass over. A `blocked` item can't be chosen but stays reachable.
    pub fn is_item_disabled(&self, this: &C, key: &str) -> bool {
        self.find(this, Some(key)).is_some_and(|entry| entry.disabled)
    }

    /// The focus handle of an item, made the first time it's asked for and
    /// kept while it renders. An item with real focus tracks it.
    pub fn focus_handle(&self, key: &SharedString, cx: &App) -> FocusHandle {
        self.state_mut()
            .handles
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// The focus handle of an item, if one was made.
    pub fn existing_focus_handle(&self, key: &str) -> Option<FocusHandle> {
        self.state().handles.get(key).cloned()
    }

    /// The key of the item that holds focus, if one does.
    pub fn focused(&self, window: &Window) -> Option<SharedString> {
        let state = self.state();
        state
            .registry
            .iter()
            .find(|item| state.handles.get(&item.key).is_some_and(|handle| handle.is_focused(window)))
            .map(|item| item.key.clone())
    }

    /// Where an item last laid out, once [`track`](Self::track) is on its
    /// element.
    pub fn bounds(&self, key: &str) -> Option<Bounds<Pixels>> {
        self.inner.live.bounds.borrow().get(key).copied()
    }

    /// Whether an item rendered last frame (`isConnected`).
    pub fn is_connected(&self, key: &str) -> bool {
        self.inner.live.connected(key)
    }

    /// What a rendered item said about itself as it last registered, like
    /// its props for a component's `itemProps`.
    pub fn item_meta(&self, key: &str) -> Option<ItemMeta<T>> {
        self.state().meta(key).cloned()
    }

    /// A rendered item, by key (`element`).
    pub fn element(&self, key: &str) -> Option<ListElement> {
        let state = self.state();
        let meta = state.meta(key)?;
        Some(ListElement {
            key: SharedString::from(key.to_string()),
            handle: state.handles.get(key).cloned(),
            bounds: self.bounds(key),
            link: meta.link,
            connected: self.is_connected(key),
        })
    }

    // Focuses a rendered item, or only scrolls it into view with virtual
    // focus.
    fn reveal(&self, this: &C, key: &SharedString, focus: bool, window: &mut Window, cx: &mut App) {
        let config = self.config();
        if focus
            && (config.focus)(this) == ListFocus::Real
            && (config.active)(this)
            && let Some(handle) = self.existing_focus_handle(key)
        {
            window.focus(&handle, cx);
        }
        if config.keep_in_view.as_ref().is_none_or(|keep| keep(this))
            && let Some(bounds) = self.bounds(key)
            && let Some(scroller) = config.scroll_root.as_ref().and_then(|root| root(this))
        {
            reveal_in_list(bounds, &scroller);
        }
    }

    /// Moves the highlight to an item, or off it with `None`, telling
    /// `on_highlight` its index, and `on_last_reached` when it's the last
    /// item. A rendered item takes focus with real focus, and scrolls into
    /// view.
    pub fn highlight(
        &self,
        this: &mut C,
        key: Option<SharedString>,
        options: HighlightOptions,
        window: &mut Window,
        cx: &mut Context<C>,
    ) {
        if self.set_highlighted(key.clone()) {
            cx.notify();
        }
        let list = self.entries(this);
        let index = key
            .as_ref()
            .and_then(|key| list.iter().position(|entry| &entry.key == key));
        if let Some(on_highlight) = self.config().on_highlight.clone() {
            on_highlight(this, key.as_ref(), index, options.event.as_ref(), window, cx);
        }
        // Skipped items at the end can't be reached, so the last one left counts.
        let mut last = list.len() as isize - 1;
        if self.config().skip_disabled {
            while last > 0 && list[last as usize].disabled {
                last -= 1;
            }
        }
        if index.is_some_and(|index| index as isize == last)
            && let Some(on_last_reached) = self.config().on_last_reached.clone()
        {
            on_last_reached(this, window, cx);
        }
        let Some(key) = key else {
            return;
        };
        if self.is_connected(&key) {
            self.reveal(this, &key, options.focus, window, cx);
        }
    }

    /// The key a step from the highlight lands on, or the first or last
    /// item's, without moving there. `None` for an empty list, or one where
    /// every item is disabled while it skips them. `r#loop` defaults to the
    /// config's for a step of one: a page stops at the ends, as the APG asks,
    /// though the arrows wrap.
    pub fn step_key(&self, this: &C, step: Step, r#loop: Option<bool>) -> Option<SharedString> {
        let r#loop = r#loop
            .unwrap_or_else(|| (self.config().r#loop)(this) && matches!(step, Step::By(1) | Step::By(-1)));
        let all = self.entries(this);
        let list: Vec<&ListEntry<T>> = if self.config().skip_disabled {
            all.iter().filter(|entry| !entry.disabled).collect()
        } else {
            all.iter().collect()
        };
        if list.is_empty() {
            return None;
        }
        let current = self.highlighted();
        let index = list.iter().position(|entry| Some(&entry.key) == current.as_ref());
        let by = match step {
            Step::By(by) => by,
            _ => {
                let from = index.map_or(-1, |index| index as isize);
                return Some(list[move_index(from, step, list.len(), r#loop)].key.clone());
            }
        };
        if let Some(index) = index {
            return Some(list[move_index(index as isize, step, list.len(), r#loop)].key.clone());
        }
        // From a skipped item, like one a press focused, a step counts from where it sits.
        if let Some(skipped) = all.iter().position(|entry| Some(&entry.key) == current.as_ref()) {
            let before = all[..skipped].iter().filter(|entry| !entry.disabled).count() as isize;
            let from = if by > 0 { before - 1 } else { before };
            return Some(list[move_index(from, step, list.len(), r#loop)].key.clone());
        }
        // From no highlight, a step forward lands on the first item and a step back on the last.
        Some(if by > 0 { list[0].key.clone() } else { list[list.len() - 1].key.clone() })
    }

    /// Moves the highlight by a step, or to the first or last item.
    pub fn r#move(
        &self,
        this: &mut C,
        step: Step,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<C>,
    ) {
        if let Some(key) = self.step_key(this, step, None) {
            self.highlight(this, Some(key), HighlightOptions::event(event), window, cx);
        }
    }

    /// The first item that can be chosen, for highlighting the best match.
    pub fn first_enabled(&self, this: &C) -> Option<SharedString> {
        self.entries(this)
            .iter()
            .find(|entry| !entry.pinned && !self.disabled(this, entry))
            .map(|entry| entry.key.clone())
    }

    /// The next enabled item whose label starts with what the user typed,
    /// after `from`.
    pub fn search(&self, this: &C, key: &Key, from: Option<&str>) -> Option<ListEntry<T>> {
        let list: Vec<ListEntry<T>> = self.entries(this).iter().filter(|entry| !entry.pinned).cloned().collect();
        let labels: Vec<SharedString> = list
            .iter()
            .map(|entry| {
                if self.disabled(this, entry) {
                    SharedString::default()
                } else {
                    entry.label.clone()
                }
            })
            .collect();
        let current = from.and_then(|from| list.iter().position(|entry| entry.key == from));
        let found = self.state_mut().typeahead.search(key, &labels, current)?;
        list.get(found).cloned()
    }

    /// Whether a typeahead search is under way, so Space continues it.
    pub fn typing(&self) -> bool {
        self.state().typeahead.active()
    }

    /// Ends a typeahead search.
    pub fn reset_typing(&self) {
        self.state_mut().typeahead.reset();
    }

    /// Joins an item to the list as it renders, with what it says about
    /// itself (`register`). Render order is page order. The item has left
    /// once a frame doesn't render it, which [`track`](Self::track) on its
    /// element tells. A label the item gives itself is known at once.
    pub fn register(&self, key: impl Into<SharedString>, mut meta: ItemMeta<T>, cx: &App) {
        let key = key.into();
        if let Some(text) = &meta.text {
            meta.text = Some(SharedString::from(collapse(text)));
        }
        let opened = {
            let mut state = self.state_mut();
            let opened = state.open_pass();
            *state.pass_counts.entry(key.clone()).or_default() += 1;
            let existing = state.pass_index.get(&key).copied();
            match existing {
                Some(index) => {
                    if let Some(pass) = state.pass.as_mut() {
                        pass[index].meta = meta.clone();
                    }
                }
                None => {
                    let index = state.pass.as_ref().map_or(0, Vec::len);
                    state.pass_index.insert(key.clone(), index);
                    if let Some(pass) = state.pass.as_mut() {
                        pass.push(Registered {
                            key: key.clone(),
                            meta: meta.clone(),
                        });
                    }
                }
            }
            // The last frame's item reads what it renders with now.
            if let Some(&index) = state.index.get(&key)
                && state.registry[index].meta != meta
            {
                state.registry[index].meta = meta.clone();
                state.registry_version += 1;
            }
            if meta.pinned {
                if !state.pinned.contains(&key) {
                    state.pinned.push(key.clone());
                }
            } else {
                let previous = state.known.get(&key).cloned();
                let own = meta.label.clone().or_else(|| meta.text.clone());
                if let Some(label) = &own
                    && previous.as_ref().is_none_or(|previous| &previous.label != label)
                {
                    state.labels_version += 1;
                }
                let label = own
                    .or_else(|| previous.as_ref().map(|previous| previous.label.clone()))
                    .unwrap_or_default();
                if let Some(value) = meta.value.clone() {
                    state.known.insert(
                        key.clone(),
                        KnownItem {
                            label,
                            value,
                            keywords: meta.keywords.clone(),
                            group: meta.group.clone(),
                        },
                    );
                }
            }
            opened
        };
        if opened {
            self.queue_settle(cx);
        }
    }

    /// Puts what the list reads from an item's element on it: whether it's
    /// in the page, and where it lays out, which revealing it reads. Call it
    /// on the element of every item that registers.
    pub fn track<E: ParentElement + Styled>(&self, key: impl Into<SharedString>, el: E) -> E {
        let key = key.into();
        let probe = ItemProbe {
            id: ElementId::Name(format!("ui-sdk-list-item-{key}").into()),
            key,
            live: self.inner.live.clone(),
            style: StyleRefinement::default(),
        }
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        el.relative().child(probe)
    }

    /// Registers an item part as it renders and tracks its element
    /// ([`register`](Self::register) and [`track`](Self::track)).
    pub fn bind_item<S>(&self, key: impl Into<SharedString>, meta: ItemMeta<T>, part: Part<S>, cx: &App) -> Part<S> {
        let key = key.into();
        self.register(key.clone(), meta, cx);
        self.track(key, part)
    }

    /// Starts a render of the items, from the part that holds them, every
    /// frame it renders. Registering an item starts one too, so the order the
    /// parts are built in doesn't matter; this is what tells a list that
    /// renders no items that they've all left.
    pub fn begin(&self, cx: &App) {
        let opened = self.state_mut().open_pass();
        if opened {
            self.queue_settle(cx);
        }
    }

    // Asks to settle the render under way once its frame is drawn.
    fn queue_settle(&self, cx: &App) {
        let engine = self.clone();
        let window = self.inner.window;
        cx.spawn(async move |cx| {
            window.update(cx, |_, window, cx| engine.settle(window, cx)).ok();
        })
        .detach();
    }

    // Once the items have rendered: the registry takes their order, and the
    // component hears which joined and which left.
    fn settle(&self, window: &mut Window, cx: &mut App) {
        let (joined, left) = {
            let Ok(mut state) = self.inner.state.try_borrow_mut() else {
                return;
            };
            let Some(pass) = state.pass.take() else {
                return;
            };
            let pass_index = std::mem::take(&mut state.pass_index);
            let pass_counts = std::mem::take(&mut state.pass_counts);
            if dev::DEV {
                warn_shared(&pass_counts);
            }
            let joined: Vec<SharedString> = pass
                .iter()
                .filter(|item| !state.index.contains_key(&item.key))
                .map(|item| item.key.clone())
                .collect();
            let left: Vec<SharedString> = state
                .registry
                .iter()
                .filter(|item| !pass_index.contains_key(&item.key))
                .map(|item| item.key.clone())
                .collect();
            let same = state.registry.len() == pass.len()
                && state.registry.iter().zip(&pass).all(|(a, b)| a.key == b.key && a.meta == b.meta);
            if !same {
                state.registry_version += 1;
            }
            state.mounts_version += (joined.len() + left.len()) as u64;
            for key in &left {
                state.pinned.retain(|pinned| pinned != key);
                state.handles.remove(key);
            }
            state.registry = pass;
            state.index = pass_index;
            (joined, left)
        };
        self.changed(joined, left, window, cx);
    }

    // Items whose elements stopped rendering, like a list that stopped
    // rendering altogether.
    fn departed(&self, keys: Vec<SharedString>, window: &mut Window, cx: &mut App) {
        let left = {
            let Ok(mut state) = self.inner.state.try_borrow_mut() else {
                return;
            };
            let mut left = Vec::new();
            for key in keys {
                if self.inner.live.connected(&key) || !state.index.contains_key(&key) || left.contains(&key) {
                    continue;
                }
                left.push(key);
            }
            if !left.is_empty() {
                state.registry.retain(|item| !left.contains(&item.key));
                state.reindex();
                state.registry_version += 1;
                state.mounts_version += left.len() as u64;
                for key in &left {
                    state.pinned.retain(|pinned| pinned != key);
                    state.handles.remove(key);
                }
            }
            left
        };
        if !left.is_empty() {
            self.changed(Vec::new(), left, window, cx);
        }
    }

    // Tells the component which items joined and left, gives focus to a
    // highlighted item that rendered late, and drops the highlight of one
    // that left for good.
    fn changed(&self, joined: Vec<SharedString>, left: Vec<SharedString>, window: &mut Window, cx: &mut App) {
        let Some(entity) = self.inner.entity.upgrade() else {
            return;
        };
        let hook = self.inner.settled.borrow().clone();
        if joined.is_empty() && left.is_empty() && hook.is_none() {
            return;
        }
        let engine = self.clone();
        entity.update(cx, |this, cx| {
            if let Some(on_register) = engine.config().on_register.clone() {
                for key in joined.iter().chain(&left) {
                    on_register(this, key, window, cx);
                }
            }
            for key in &joined {
                engine.focus_when_mounted(this, key, window, cx);
            }
            for key in &left {
                engine.left(this, key, window, cx);
            }
            if let Some(hook) = hook {
                hook(this, window, cx);
            }
            // Parts that read the items, like an empty message, render
            // again with the ones that joined.
            if !joined.is_empty() || !left.is_empty() {
                cx.notify();
            }
        });
    }

    // A highlighted item that renders late, like in a virtualized list,
    // takes focus.
    fn focus_when_mounted(&self, this: &mut C, key: &SharedString, window: &mut Window, cx: &mut Context<C>) {
        let config = self.config();
        if (config.focus)(this) != ListFocus::Real
            || self.highlighted().as_ref() != Some(key)
            || !(config.active)(this)
        {
            return;
        }
        if self.existing_focus_handle(key).is_some_and(|handle| handle.is_focused(window)) {
            return;
        }
        if self.is_connected(key) {
            self.reveal(this, key, true, window, cx);
        }
    }

    // An item that left the list for good, like one its new items dropped
    // while open, takes the highlight with it. Focus that went with it
    // returns to the list, so the keys keep working and the list stays open.
    // An item still in the data only scrolled out of a virtualized list, and
    // stays highlighted.
    fn left(&self, this: &mut C, key: &SharedString, window: &mut Window, cx: &mut Context<C>) {
        if self.state().in_registry(key)
            || self.data_entry(this, key).is_some()
            || self.highlighted().as_ref() != Some(key)
        {
            return;
        }
        if self.set_highlighted(None) {
            cx.notify();
        }
        if let Some(on_highlight) = self.config().on_highlight.clone() {
            on_highlight(this, None, None, None, window, cx);
        }
        let config = self.config();
        if (config.focus)(this) == ListFocus::Real && (config.active)(this) && focus_lost(window, cx) {
            if let Some(container) = config.container.as_ref().and_then(|container| container(this)) {
                window.focus(&container, cx);
            }
        }
    }

    /// Runs `hook` after every settle, once a frame that rendered the items
    /// is drawn, inside the component's update, after it heard which items
    /// joined and left: for what a component or a behavior learns from where
    /// the items and focus landed, like an option that took focus by Tab or
    /// a press. A list has one; a later call replaces it, so a component
    /// whose behavior or core sets one leaves it to them.
    pub fn set_settled_hook(&self, hook: Option<ListHookFn<C>>) {
        *self.inner.settled.borrow_mut() = hook;
    }

    /// The label of an item, from data, a rendered item, or one that
    /// rendered before.
    pub fn label_of(&self, this: &C, key: &str) -> Option<SharedString> {
        if let Some(entry) = self.data_entry(this, key) {
            return Some(entry.label);
        }
        let registered = self.state().meta(key).cloned();
        if let Some(meta) = registered {
            let key = SharedString::from(key.to_string());
            return Some(self.state_mut().live_label(&key, &meta));
        }
        self.state().known.get(key).map(|known| known.label.clone())
    }

    /// The value of an item, from data, a rendered item, or one that
    /// rendered before.
    pub fn value_of(&self, this: &C, key: &str) -> Option<T> {
        if let Some(entry) = self.data_entry(this, key) {
            return entry.value;
        }
        let state = self.state();
        match state.meta(key) {
            Some(meta) => meta.value.clone(),
            None => state.known.get(key).map(|known| known.value.clone()),
        }
    }

    /// Whether an item shows for the current query, as it renders, which
    /// the listing then knows about (`matches`). Data items that don't
    /// aren't rendered.
    pub fn matches(&self, this: &C, key: &SharedString, meta: &ItemMeta<T>) -> bool {
        self.state_mut().asked.insert(key.clone(), meta.clone());
        self.shows(this, key, meta)
    }

    fn shows(&self, this: &C, key: &SharedString, meta: &ItemMeta<T>) -> bool {
        self.data(this).is_some() || self.composed_matches(this, key, meta)
    }

    /// What each item shows from the list as a whole, by key, as a short
    /// string: `"-"` while the query hides it, then the list's size for an
    /// item placed by `index`, then what `more` adds, like `"s"` while it's
    /// selected. Every item that rendered or was remembered, and every data
    /// item, has an entry. See `listbox_listing.rs`.
    pub fn listing(
        &self,
        this: &C,
        more: Option<&dyn Fn(&SharedString, Option<&T>) -> SharedString>,
    ) -> HashMap<SharedString, SharedString> {
        let data = self.data(this);
        let query = (self.config().query)(this);
        // Without a query every item shows. With one, the list already
        // filtered the items in the page for the highlight, so only items
        // that aren't, like one that rendered and hasn't registered, need
        // checking.
        let filtering = data.is_none() && !js_trim(&query).is_empty();
        let shown: Option<HashSet<SharedString>> =
            filtering.then(|| self.composed_entries(this).iter().map(|entry| entry.key.clone()).collect());
        let mut listing: HashMap<SharedString, SharedString> = HashMap::new();
        let mut size: Option<usize> = None;
        let as_meta = |entry: &ListEntry<T>| ItemMeta {
            value: entry.value.clone(),
            label: Some(entry.label.clone()),
            text: None,
            keywords: entry.keywords.clone(),
            disabled: entry.disabled,
            group: entry.group.clone(),
            pinned: entry.pinned,
            index: None,
            link: false,
        };
        let mut sources: Vec<(SharedString, ItemMeta<T>, bool)> = Vec::new();
        {
            let state = self.state();
            for (key, meta) in &state.asked {
                sources.push((key.clone(), meta.clone(), true));
            }
            for (key, known) in &state.known {
                let meta = ItemMeta {
                    value: Some(known.value.clone()),
                    label: Some(known.label.clone()),
                    text: None,
                    keywords: known.keywords.clone(),
                    disabled: false,
                    group: known.group.clone(),
                    pinned: false,
                    index: None,
                    link: false,
                };
                sources.push((key.clone(), meta, false));
            }
        }
        if let Some(data) = &data {
            for entry in data.flat.iter() {
                sources.push((entry.key.clone(), as_meta(entry), false));
            }
        }
        if let Some(empty) = self.empty_source(this) {
            for entry in empty.flat.iter() {
                sources.push((entry.key.clone(), as_meta(entry), false));
            }
        }
        // `latest` says the item holds the props it last rendered with, so
        // its registration needn't be asked for them again.
        for (key, item, latest) in sources {
            if listing.contains_key(&key) {
                continue;
            }
            let registered = self.state().meta(&key).cloned();
            let meta = if latest { item } else { registered.unwrap_or(item) };
            let matched = shown.as_ref().is_none_or(|shown| shown.contains(&key))
                || (!self.is_connected(&key) && self.composed_matches(this, &key, &meta));
            let mut entry = String::from(if matched { "" } else { "-" });
            if meta.index.is_some() {
                let size = *size.get_or_insert_with(|| self.entries(this).len());
                entry.push_str(&size.to_string());
            }
            if let Some(more) = more {
                entry.push_str(&more(&key, meta.value.as_ref()));
            }
            listing.insert(key, entry.into());
        }
        listing
    }

    /// Everything the listing depends on besides a component's own state,
    /// like the selected value. A data list's entries don't depend on which
    /// items are rendered or on their labels, only on its pinned items,
    /// which count toward its size, so rows scrolling into a virtualized
    /// list don't make it list every item again.
    pub fn listing_inputs(&self, this: &C) -> ListingInputs<T, X> {
        let config = self.config();
        let items = (config.items)(this);
        let state = self.state();
        ListingInputs {
            count: if items.is_some() {
                state.pinned.len() as u64
            } else {
                state.mounts_version
            },
            labels: if items.is_some() { 0 } else { state.labels_version },
            query: (config.query)(this),
            filter: (config.filter)(this),
            items,
            empty_items: config.empty_items.as_ref().and_then(|items| items(this)),
            show_empty: config.show_empty.as_ref().map(|show| show(this)),
            rank: config.rank.as_ref().map(|rank| rank(this)),
        }
    }

    /// The keys of the items that match and can be chosen, in one group or
    /// in all, leaving out pinned entries, for selecting all of them at once.
    pub fn selectable(&self, this: &C, group: Option<&str>) -> Vec<SharedString> {
        let mut keys = Vec::new();
        for entry in self.entries(this).iter() {
            // Its own disabled state, not a full selection, which choosing
            // all would exceed anyway.
            if entry.pinned || entry.disabled {
                continue;
            }
            if let Some(group) = group {
                let own = self.state().meta(&entry.key).and_then(|meta| meta.group.clone());
                if own.or_else(|| entry.group.clone()).as_deref() != Some(group) {
                    continue;
                }
            }
            keys.push(entry.key.clone());
        }
        keys
    }

    /// How many items match, leaving out pinned entries like a create
    /// option.
    pub fn count(&self, this: &C) -> usize {
        self.entries(this).iter().filter(|entry| !entry.pinned).count()
    }

    /// Whether any item of a composed group matches. Data groups always do,
    /// since the data already left the others out.
    pub fn group_matches(&self, this: &C, group: &str) -> bool {
        if self.data(this).is_some() {
            return true;
        }
        let items: Vec<Registered<T>> = self
            .state()
            .registry
            .iter()
            .filter(|item| item.meta.group.as_deref() == Some(group))
            .cloned()
            .collect();
        items
            .iter()
            .any(|item| self.composed_matches(this, &item.key, &item.meta))
    }

    /// Counts every label change, so a part showing a label can render
    /// again.
    pub fn labels_version(&self) -> u64 {
        self.state().labels_version
    }

    /// Remembers a label, like the one an item had when it was chosen.
    /// `keywords` of `None` keeps the ones it knew.
    pub fn remember(&self, key: SharedString, label: SharedString, value: T, keywords: Option<Vec<SharedString>>) {
        let mut state = self.state_mut();
        let current = state.known.get(&key).cloned();
        let same_text = current.as_ref().is_some_and(|current| {
            current.label == label && keywords.as_ref().is_none_or(|keywords| &current.keywords == keywords)
        });
        // A new value under the same key and text, like an object made
        // again, still replaces the old one.
        if let Some(current) = current.as_ref()
            && same_text
        {
            if current.value != value
                && let Some(known) = state.known.get_mut(&key)
            {
                known.value = value;
            }
            return;
        }
        let (old_keywords, group) = current
            .map(|current| (current.keywords, current.group))
            .unwrap_or_default();
        state.known.insert(
            key,
            KnownItem {
                label,
                value,
                keywords: keywords.unwrap_or(old_keywords),
                group,
            },
        );
        state.labels_version += 1;
    }
}

// Whether focus fell to the window, like when its item left: nothing holds
// it, or what holds it isn't in the page.
fn focus_lost(window: &Window, cx: &App) -> bool {
    window
        .focused(cx)
        .is_none_or(|focused| !focused.is_rendered(window))
}

/// An item a controller can highlight: enabled, and shown for the query
/// (`highlightable`).
pub fn highlightable<C, T, X>(listbox: &ListboxEngine<C, T, X>, this: &C, key: &str) -> Option<ListEntry<T>>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    listbox
        .entries(this)
        .iter()
        .find(|entry| entry.key == key && !entry.pinned && !listbox.disabled(this, entry))
        .cloned()
}

/// A controller's highlight: onto an item it can highlight, or off with
/// `None`. Returns whether it moved (`highlightFromCode`).
pub fn highlight_from_code<C, T, X>(
    listbox: &ListboxEngine<C, T, X>,
    this: &mut C,
    key: Option<SharedString>,
    current: Option<&SharedString>,
    window: &mut Window,
    cx: &mut Context<C>,
) -> bool
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    if key.as_ref() == current {
        return false;
    }
    if let Some(key) = &key
        && highlightable(listbox, this, key).is_none()
    {
        return false;
    }
    listbox.highlight(this, key, HighlightOptions::default(), window, cx);
    true
}

/// The highlighted item's value for a controller. `None` for an action like
/// Create or Select All (`highlightedValue`).
pub fn highlighted_value<C, T, X>(listbox: &ListboxEngine<C, T, X>, this: &C, key: Option<&str>) -> Option<T>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    listbox
        .find(this, key)
        .filter(|entry| !entry.pinned)
        .and_then(|entry| entry.value)
}

// Items are keyed by value, so two with one value are one item to the list:
// arrow keys skip one, and Enter or a press can reach the other. A pass can
// span more than one render, which registers every item again, so a key is
// shared when it registered more often than most keys did.
fn warn_shared(counts: &HashMap<SharedString, usize>) {
    let mut tally: HashMap<usize, usize> = HashMap::new();
    for &count in counts.values() {
        *tally.entry(count).or_default() += 1;
    }
    let Some(renders) = tally
        .iter()
        .max_by_key(|&(&count, &keys)| (keys, std::cmp::Reverse(count)))
        .map(|(&count, _)| count)
    else {
        return;
    };
    for (key, &count) in counts {
        if count > renders {
            dev::warn_once(
                format!("listbox-shared-{key}"),
                &format!(
                    "Two items have the value \"{key}\". Give each item its own value, or arrow keys skip one and choosing it picks the other."
                ),
                Some(Origin {
                    component: "listbox",
                    id: None,
                }),
            );
        }
    }
}
