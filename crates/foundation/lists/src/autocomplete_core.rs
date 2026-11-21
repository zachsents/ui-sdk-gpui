//! Port of `foundation/lists/autocomplete-core.ts`: what every "an input
//! filters a collection" component shares: the listbox engine with virtual
//! focus, the text that filters it, where the highlight goes as the list
//! changes, arrow keys, and the field and option parts. Autocomplete adds a
//! popup, forms and inline completion on top, and Command adds running
//! commands and pages. Combobox stays apart, since it owns a selected value.
//!
//! The original hears every change to its component's store. Here the core
//! looks once the frame that rendered the items is drawn, and the component
//! calls [`AutocompleteCore::observe`] after it changes what the core reads,
//! like the query, its items or whether it's open, and
//! [`AutocompleteCore::options_changed`] when it's handed new options. Make
//! the core in the constructor and keep it in a field. It takes in where the
//! component starts once the component is made, as the original reads it as
//! the core is made.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, FocusHandle, InteractiveElement, KeyDownEvent, MouseMoveEvent, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use ui_sdk_focus::{Step, hold_focus};
use ui_sdk_kernel::roles::{HandledKey, Role};
use ui_sdk_kernel::{Aria, FocusMode, InputEvent, Instance, Key, Part, Reason};
use ui_sdk_platform::is_apple;
use ui_sdk_text::{MatchMode, fuzzy_filter};

use crate::async_options::AsyncOptionsStatus;
use crate::listbox::create_filtered_listbox;
use crate::listbox_engine::{
    ItemMeta, ListData, ListFilter, ListFilterFn, ListFilterResult, ListFocus, ListHighlightFn, ListItem,
    ListScroller, ListboxEngine, ListboxEngineConfig, WeakListboxEngine,
};
use crate::listbox_keyboard::list_step;
use crate::listbox_keys::option_id;
use crate::listbox_listing::{KeepListing, Listing, keep_listing};
use crate::listbox_parts::position_attrs;
use crate::listbox_search::{
    AnnounceResults, AnnounceResultsOptions, ListAnnouncementFn, LoadMore, LoadMoreFn, announce_results,
    create_load_more,
};

/// What the shared core reads from a component's state
/// (`AutocompleteCoreState`). The rest of the original's, the highlight,
/// the items, the listing and the counts, the core keeps or reads itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutocompleteCoreState {
    pub open: bool,
    pub status: AsyncOptionsStatus,
    /// The list has rendered and its items have registered, so an empty list
    /// is really empty ([`ListSettled`](crate::ListSettled)).
    pub settled: bool,
    /// Real focus moved into the list for a touch screen reader, instead of
    /// an active descendant.
    pub focus_in_list: bool,
}

/// How typing narrows the list (`SearchFilter`): `Contains` and
/// `StartsWith` match the text itself, `Fuzzy` matches its letters in order
/// and ranks the best first, a function returns a score or true or false,
/// and `Off` keeps everything.
pub enum SearchFilter<X = ()> {
    Contains,
    StartsWith,
    Fuzzy,
    Off,
    Custom(ListFilterFn<SharedString, X>),
}

impl<X> SearchFilter<X> {
    /// A filter of your own, returning true or false, or a score.
    pub fn custom<R: Into<ListFilterResult>>(filter: impl Fn(&ListItem<SharedString, X>, &str) -> R + 'static) -> Self {
        SearchFilter::Custom(Rc::new(move |item, query| filter(item, query).into()))
    }

    /// Whether it ranks data items by score: fuzzy, or a function.
    pub fn ranks(&self) -> bool {
        matches!(self, SearchFilter::Fuzzy | SearchFilter::Custom(_))
    }

    /// How it marks matches, or `None` for a function, which can't say
    /// (`matchMode`).
    pub fn match_mode(&self) -> Option<MatchMode> {
        match self {
            SearchFilter::Contains => Some(MatchMode::Contains),
            SearchFilter::StartsWith => Some(MatchMode::StartsWith),
            SearchFilter::Fuzzy => Some(MatchMode::Fuzzy),
            _ => None,
        }
    }
}

impl<X> Default for SearchFilter<X> {
    fn default() -> Self {
        SearchFilter::Contains
    }
}

impl<X> Clone for SearchFilter<X> {
    fn clone(&self) -> Self {
        match self {
            SearchFilter::Contains => SearchFilter::Contains,
            SearchFilter::StartsWith => SearchFilter::StartsWith,
            SearchFilter::Fuzzy => SearchFilter::Fuzzy,
            SearchFilter::Off => SearchFilter::Off,
            SearchFilter::Custom(filter) => SearchFilter::Custom(filter.clone()),
        }
    }
}

impl<X> fmt::Debug for SearchFilter<X> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchFilter::Contains => write!(f, "Contains"),
            SearchFilter::StartsWith => write!(f, "StartsWith"),
            SearchFilter::Fuzzy => write!(f, "Fuzzy"),
            SearchFilter::Off => write!(f, "Off"),
            SearchFilter::Custom(_) => write!(f, "Custom(..)"),
        }
    }
}

impl<X> PartialEq for SearchFilter<X> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (SearchFilter::Custom(a), SearchFilter::Custom(b)) => Rc::ptr_eq(a, b),
            (SearchFilter::Contains, SearchFilter::Contains)
            | (SearchFilter::StartsWith, SearchFilter::StartsWith)
            | (SearchFilter::Fuzzy, SearchFilter::Fuzzy)
            | (SearchFilter::Off, SearchFilter::Off) => true,
            _ => false,
        }
    }
}

impl<X> From<ListFilter<SharedString, X>> for SearchFilter<X> {
    fn from(filter: ListFilter<SharedString, X>) -> Self {
        match filter {
            ListFilter::Contains => SearchFilter::Contains,
            ListFilter::StartsWith => SearchFilter::StartsWith,
            ListFilter::Off => SearchFilter::Off,
            ListFilter::Custom(filter) => SearchFilter::Custom(filter),
        }
    }
}

// The engine's filter for a search filter, with the fuzzy one kept so its
// identity holds.
fn engine_filter<X>(filter: SearchFilter<X>, fuzzy: &ListFilter<SharedString, X>) -> ListFilter<SharedString, X> {
    match filter {
        SearchFilter::Contains => ListFilter::Contains,
        SearchFilter::StartsWith => ListFilter::StartsWith,
        SearchFilter::Fuzzy => fuzzy.clone(),
        SearchFilter::Off => ListFilter::Off,
        SearchFilter::Custom(filter) => ListFilter::Custom(filter),
    }
}

/// When the first match is highlighted: always, like a command palette;
/// while typing; or never (`AutoHighlight`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AutoHighlight {
    Always,
    Typing,
    #[default]
    Off,
}

/// What moved the highlight, for components that report it
/// (`HighlightCause`): `ListNavigation`, `ItemHover`, `ItemRemoved` or
/// `Imperative` from the core, or the reason a component gave
/// [`AutocompleteCore::cause`].
#[derive(Clone, Debug)]
pub struct HighlightCause {
    pub reason: Reason,
    pub event: Option<InputEvent>,
}

impl HighlightCause {
    pub fn new(reason: Reason, event: Option<InputEvent>) -> Self {
        Self { reason, event }
    }
}

/// The props an item registers with (`AutocompleteCoreItem`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutocompleteCoreItem {
    pub value: SharedString,
    pub label: Option<SharedString>,
    /// The text it shows, which names it without a `label`.
    pub text: Option<SharedString>,
    pub keywords: Vec<SharedString>,
    pub disabled: bool,
    pub index: Option<usize>,
    pub group: Option<SharedString>,
    /// It's a link (`a[href]`): Enter clicks it, so its press follows it.
    pub link: bool,
}

impl AutocompleteCoreItem {
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            ..Self::default()
        }
    }

    /// What the item registers with the list.
    pub fn meta(&self) -> ItemMeta<SharedString> {
        ItemMeta {
            value: Some(self.value.clone()),
            label: self.label.clone(),
            text: self.text.clone(),
            keywords: self.keywords.clone(),
            disabled: self.disabled,
            group: self.group.clone(),
            pinned: false,
            index: self.index,
            link: self.link,
        }
    }
}

/// Which end of the list an arrow key that opens it highlights.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListEdge {
    First,
    Last,
}

/// Moves the highlight, with the cause, `None` for a default the component
/// picks, which reports nothing. It runs inside the component's update.
pub type CoreHighlightFn<C> =
    Rc<dyn Fn(&mut C, Option<SharedString>, Option<HighlightCause>, &mut Window, &mut Context<C>)>;

/// Opens a closed list from an arrow key, highlighting its first or last
/// item once it renders.
pub type CoreOpenFn<C> = Rc<dyn Fn(&mut C, &KeyDownEvent, ListEdge, &mut Window, &mut Context<C>)>;

/// The highlight typing prefers over the first match, like an inline
/// completion's item.
pub type CorePreferredFn<C, X> = Rc<dyn Fn(&C, &ListboxEngine<C, SharedString, X>) -> Option<SharedString>>;

/// What [`create_autocomplete_core`] reads from its component.
pub struct AutocompleteCoreConfig<C: 'static, X: 'static = ()> {
    /// The component's instance id, which option ids start with.
    pub id: SharedString,
    /// What the core reads from the component's state.
    pub state: Rc<dyn Fn(&C) -> AutocompleteCoreState>,
    /// The text that filters the list.
    pub query: Rc<dyn Fn(&C) -> SharedString>,
    /// The items as data, or `None` for composed items.
    pub items: Rc<dyn Fn(&C) -> Option<ListData<SharedString, X>>>,
    /// Items shown instead while the text is empty, like recent searches.
    pub empty_items: Option<Rc<dyn Fn(&C) -> Option<ListData<SharedString, X>>>>,
    pub filter: Rc<dyn Fn(&C) -> SearchFilter<X>>,
    pub auto_highlight: Rc<dyn Fn(&C) -> AutoHighlight>,
    /// Moves the highlight. The component calls the core's listbox here.
    pub highlight: CoreHighlightFn<C>,
    pub preferred: Option<CorePreferredFn<C, X>>,
    pub open: Option<CoreOpenFn<C>>,
    /// Notifies a virtualizer which index to reveal.
    pub on_highlight: Option<ListHighlightFn<C>>,
    pub container: Option<Rc<dyn Fn(&C) -> Option<FocusHandle>>>,
    pub scroll_root: Option<Rc<dyn Fn(&C) -> Option<ListScroller>>>,
    /// Whether the component settles the highlight itself once the items
    /// render, like a command page that just opened, with its own reason.
    pub settles_mount: Option<Rc<dyn Fn(&C) -> bool>>,
    /// The component's `loop` option. Defaults to true.
    pub r#loop: Option<Rc<dyn Fn(&C) -> bool>>,
    /// The component's `on_load_more` option.
    pub on_load_more: Option<Rc<dyn Fn(&C) -> Option<LoadMoreFn>>>,
    /// The component's `announcement` option.
    pub announcement: Option<Rc<dyn Fn(&C) -> Option<ListAnnouncementFn>>>,
    /// Whether a status part is in the page, which says result counts
    /// instead: `flag_while_bound(instance, "status")`.
    pub silent: Option<Rc<dyn Fn(&C) -> bool>>,
}

// What the core saw last, to tell what changed.
struct Seen<X: 'static> {
    query: SharedString,
    mounts: u64,
    items: Option<ListData<SharedString, X>>,
    settled: bool,
    highlighted: Option<SharedString>,
}

struct CoreInner<C: 'static, X: 'static> {
    config: AutocompleteCoreConfig<C, X>,
    // What changes the text next, so the highlight that follows reports it.
    cause: RefCell<Option<HighlightCause>>,
    // Text that was asked for and hasn't arrived, while `settle_for` reads
    // the list.
    asked: Rc<RefCell<Option<SharedString>>>,
    // The text held when `settle_for` ran, until the options that answer it
    // arrive.
    held_query: RefCell<Option<SharedString>>,
    mounts: Rc<Cell<u64>>,
    seen: Rc<RefCell<Option<Seen<X>>>>,
    auto_mode: Cell<Option<AutoHighlight>>,
    listing: KeepListing<SharedString, X>,
    announcer: AnnounceResults<C>,
}

/// The shared core of a search list (`createAutocompleteCore`'s return).
/// Cloning it gives another handle to the same core.
pub struct AutocompleteCore<C: 'static, X: 'static = ()> {
    pub listbox: ListboxEngine<C, SharedString, X>,
    pub load_more: LoadMore<C>,
    inner: Rc<CoreInner<C, X>>,
}

impl<C: 'static, X: 'static> Clone for AutocompleteCore<C, X> {
    fn clone(&self) -> Self {
        Self {
            listbox: self.listbox.clone(),
            load_more: self.load_more.clone(),
            inner: self.inner.clone(),
        }
    }
}

/// The shared core for the component of `cx`, rendering in `window`
/// (`createAutocompleteCore`).
pub fn create_autocomplete_core<C, X>(
    config: AutocompleteCoreConfig<C, X>,
    window: &Window,
    cx: &mut Context<C>,
) -> AutocompleteCore<C, X>
where
    C: 'static,
    X: Clone + Default + 'static,
{
    let engine_cell: Rc<RefCell<Option<WeakListboxEngine<C, SharedString, X>>>> = Rc::new(RefCell::new(None));
    let count = {
        let engine_cell = engine_cell.clone();
        move |this: &C| {
            engine_cell
                .borrow()
                .as_ref()
                .and_then(WeakListboxEngine::upgrade)
                .map_or(0, |listbox| listbox.count(this))
        }
    };
    let load_more = {
        let state = config.state.clone();
        let on_load_more = config.on_load_more.clone();
        create_load_more(
            count.clone(),
            move |this| state(this).status,
            move |this| on_load_more.as_ref().and_then(|on_load_more| on_load_more(this)),
        )
    };
    let asked: Rc<RefCell<Option<SharedString>>> = Rc::new(RefCell::new(None));
    let mounts = Rc::new(Cell::new(0u64));
    let seen: Rc<RefCell<Option<Seen<X>>>> = Rc::new(RefCell::new(None));
    let fuzzy: ListFilter<SharedString, X> = ListFilter::custom(|item: &ListItem<SharedString, X>, query: &str| {
        fuzzy_filter(&item.label, item.keywords.iter(), query)
    });

    let mut engine = ListboxEngineConfig::new(|_: &C, value: &SharedString| value.clone())
        .skip_disabled(true);
    {
        let state = config.state.clone();
        engine = engine.focus(move |this| {
            if state(this).focus_in_list {
                ListFocus::Real
            } else {
                ListFocus::Virtual
            }
        });
    }
    {
        let items = config.items.clone();
        engine = engine.items(move |this| items(this));
    }
    if let Some(empty_items) = config.empty_items.clone() {
        engine = engine.empty_items(move |this| empty_items(this));
    }
    {
        let filter = config.filter.clone();
        engine = engine.filter(move |this| Some(engine_filter(filter(this), &fuzzy)));
    }
    {
        // Scores rank data items. Composed ones keep their order, since
        // their component owns it.
        let filter = config.filter.clone();
        engine = engine.rank(move |this| filter(this).ranks());
    }
    {
        let query = config.query.clone();
        let asked = asked.clone();
        engine = engine.query(move |this| asked.borrow().clone().unwrap_or_else(|| query(this)));
    }
    {
        let r#loop = config.r#loop.clone();
        engine = engine.r#loop(move |this| r#loop.as_ref().is_none_or(|r#loop| r#loop(this)));
    }
    {
        let state = config.state.clone();
        engine = engine.active(move |this| state(this).open);
    }
    {
        let seen = seen.clone();
        let on_highlight = config.on_highlight.clone();
        engine = engine.on_highlight(move |this, key, index, event, window, cx| {
            if let Some(seen) = seen.borrow_mut().as_mut() {
                seen.highlighted = key.cloned();
            }
            if let Some(on_highlight) = &on_highlight {
                on_highlight(this, key, index, event, window, cx);
            }
        });
    }
    {
        let load_more = load_more.clone();
        engine = engine.on_last_reached(move |this, window, cx| load_more.reached(this, window, cx));
    }
    if let Some(container) = config.container.clone() {
        engine = engine.container(move |this| container(this));
    }
    if let Some(scroll_root) = config.scroll_root.clone() {
        engine = engine.scroll_root(move |this| scroll_root(this));
    }
    {
        let mounts = mounts.clone();
        engine = engine.on_register(move |_this, _key, _window, _cx| mounts.set(mounts.get() + 1));
    }
    let listbox = create_filtered_listbox(engine, window, cx);
    *engine_cell.borrow_mut() = Some(listbox.downgrade());
    let announcer = {
        let state = config.state.clone();
        let loading = config.state.clone();
        let silent = config.silent.clone();
        let announcement = config.announcement.clone();
        announce_results(
            AnnounceResultsOptions {
                query: config.query.clone(),
                count: Rc::new(count),
                loading: Rc::new(move |this| loading(this).status == AsyncOptionsStatus::Loading),
                shown: Rc::new(move |this| state(this).open),
                silent: Rc::new(move |this| silent.as_ref().is_some_and(|silent| silent(this))),
                announcement: Rc::new(move |this| announcement.as_ref().and_then(|announcement| announcement(this))),
            },
            window,
            cx,
        )
    };

    let core = AutocompleteCore {
        listbox: listbox.clone(),
        load_more,
        inner: Rc::new(CoreInner {
            config,
            cause: RefCell::new(None),
            asked,
            held_query: RefCell::new(None),
            mounts,
            seen,
            auto_mode: Cell::new(None),
            listing: keep_listing(),
            announcer,
        }),
    };
    // Where the component starts, once it's made.
    {
        let core = core.clone();
        cx.defer_in(window, move |this, _window, _cx| core.start(this));
    }
    // Items rendering, data arriving and the list binding: the core looks
    // once each frame that rendered the items is drawn.
    let weak = Rc::downgrade(&core.inner);
    let weak_listbox = listbox.downgrade();
    let load_more = core.load_more.clone();
    listbox.set_settled_hook(Some(Rc::new(move |this: &mut C, window: &mut Window, cx: &mut Context<C>| {
        let (Some(inner), Some(listbox)) = (weak.upgrade(), weak_listbox.upgrade()) else {
            return;
        };
        let core = AutocompleteCore {
            listbox,
            load_more: load_more.clone(),
            inner,
        };
        core.observe(this, window, cx);
    })));
    core
}

impl<C, X> AutocompleteCore<C, X>
where
    C: 'static,
    X: Clone + Default + 'static,
{
    fn config(&self) -> &AutocompleteCoreConfig<C, X> {
        &self.inner.config
    }

    // The text the list is filtered by now: one asked for, or the component's.
    fn query(&self, this: &C) -> SharedString {
        self.inner
            .asked
            .borrow()
            .clone()
            .unwrap_or_else(|| (self.config().query)(this))
    }

    fn seen_now(&self, this: &C) -> Seen<X> {
        Seen {
            query: (self.config().query)(this),
            mounts: self.inner.mounts.get(),
            items: (self.config().items)(this),
            settled: (self.config().state)(this).settled,
            highlighted: self.listbox.highlighted(),
        }
    }

    // Takes in where the component starts, as the original reads it as the
    // core is made, once the component is made. A change heard before then
    // already took it in.
    fn start(&self, this: &C) {
        if self.inner.seen.borrow().is_none() {
            *self.inner.seen.borrow_mut() = Some(self.seen_now(this));
        }
        if self.inner.auto_mode.get().is_none() {
            self.inner.auto_mode.set(Some((self.config().auto_highlight)(this)));
        }
    }

    /// How many times items have joined or left the list, which tells a
    /// component that composed items rendered (`mounts`).
    pub fn mounts(&self) -> u64 {
        self.inner.mounts.get()
    }

    /// The props an item rendered with last (`itemProps`).
    pub fn item_props(&self, key: &str) -> Option<ItemMeta<SharedString>> {
        self.listbox.item_meta(key)
    }

    /// Where the highlight belongs after the list changed (`settledHighlight`).
    /// New text highlights its best match; otherwise the same item stays
    /// while it's still there, like when async results arrive, and one that
    /// left goes. It writes nothing, so a component can work it out with the
    /// options that carry new data.
    pub fn settled_highlight(&self, this: &C, fresh: bool) -> Option<SharedString> {
        let highlighted = self.listbox.highlighted();
        let state = (self.config().state)(this);
        // Composed items aren't known until the list renders.
        if (self.config().items)(this).is_none() && !state.settled {
            return highlighted;
        }
        let mode = (self.config().auto_highlight)(this);
        let auto = mode == AutoHighlight::Always || (mode == AutoHighlight::Typing && !self.query(this).is_empty());
        let kept = highlighted
            .as_ref()
            .is_some_and(|highlighted| self.listbox.entries(this).iter().any(|entry| &entry.key == highlighted));
        let best = if auto {
            self.config()
                .preferred
                .as_ref()
                .and_then(|preferred| preferred(this, &self.listbox))
                .or_else(|| self.listbox.first_enabled(this))
        } else {
            None
        };
        if fresh && auto {
            best
        } else if kept {
            highlighted
        } else {
            best
        }
    }

    /// Puts the highlight where it belongs after the list changed
    /// (`settle`).
    pub fn settle(&self, this: &mut C, fresh: bool, why: Option<HighlightCause>, window: &mut Window, cx: &mut Context<C>) {
        let target = self.settled_highlight(this, fresh);
        if target != self.listbox.highlighted() {
            let highlight = self.config().highlight.clone();
            highlight(this, target, why, window, cx);
        }
    }

    /// Moves the highlight by the step a key takes ([`list_step`]),
    /// skipping disabled items (`navigate`). On a closed list, a step to the
    /// next or previous item opens it. Returns whether the key moved; a key
    /// it takes calls `window.prevent_default()`.
    pub fn navigate(&self, this: &mut C, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<C>) -> bool {
        let Some(step) = list_step(&event.keystroke, is_apple(cx)) else {
            return false;
        };
        if event.keystroke.modifiers.alt && Key::of(&event.keystroke) == Key::ArrowUp {
            return false;
        }
        if !(self.config().state)(this).open {
            // Only a step to the next or previous item opens the list.
            let Some(open) = self.config().open.clone() else {
                return false;
            };
            let edge = match step {
                Step::By(1) => ListEdge::First,
                Step::By(-1) => ListEdge::Last,
                _ => return false,
            };
            window.prevent_default();
            open(this, event, edge, window, cx);
            return true;
        }
        window.prevent_default();
        if let Some(key) = self.listbox.step_key(this, step, None) {
            let highlight = self.config().highlight.clone();
            highlight(
                this,
                Some(key),
                Some(HighlightCause::new(Reason::ListNavigation, Some(InputEvent::from(event)))),
                window,
                cx,
            );
        }
        true
    }

    /// Looks at what changed since it last looked (the original's store
    /// subscription): new text settles the highlight on its best match, and
    /// items rendering, data arriving and the list settling keep it where it
    /// is if they can. Call it after changing what the core reads; it also
    /// runs once each frame that renders the items is drawn.
    pub fn observe(&self, this: &mut C, window: &mut Window, cx: &mut Context<C>) {
        let now = self.seen_now(this);
        let before = self.inner.seen.borrow_mut().replace(Seen {
            query: now.query.clone(),
            mounts: now.mounts,
            items: now.items.clone(),
            settled: now.settled,
            highlighted: now.highlighted.clone(),
        });
        if let Some(before) = before {
            let items_changed = match (&now.items, &before.items) {
                (None, None) => false,
                (Some(a), Some(b)) => !a.ptr_eq(b),
                _ => true,
            };
            if now.query != before.query {
                let why = self
                    .inner
                    .cause
                    .borrow_mut()
                    .take()
                    .unwrap_or_else(|| HighlightCause::new(Reason::Imperative, None));
                self.settle(this, true, Some(why), window, cx);
            } else if (now.mounts != before.mounts
                && !self.config().settles_mount.as_ref().is_some_and(|settles| settles(this)))
                || items_changed
                || now.settled != before.settled
            {
                let why = before
                    .highlighted
                    .as_ref()
                    .map(|_| HighlightCause::new(Reason::ItemRemoved, None));
                // Data that settled the highlight in the same pass moved it
                // already, so this only reports where it went.
                if now.highlighted != before.highlighted {
                    let highlight = self.config().highlight.clone();
                    highlight(this, now.highlighted.clone(), why, window, cx);
                } else {
                    self.settle(this, false, why, window, cx);
                }
            }
        }
        if let Some(seen) = self.inner.seen.borrow_mut().as_mut() {
            seen.highlighted = self.listbox.highlighted();
        }
        self.inner.announcer.changed(this, cx);
    }

    /// The component was handed new options (the original's `onOptions`):
    /// an app that kept its text, like one that strips digits from it, was
    /// asked for a highlight that fits text it never took, so it's asked
    /// again; and auto highlight turned on puts the highlight on the best
    /// match if nothing has it, while turned off it stays where it is.
    pub fn options_changed(&self, this: &C, window: &Window, cx: &mut Context<C>) {
        if let Some(held) = self.inner.held_query.borrow_mut().take() {
            let why = self.inner.cause.borrow().clone();
            let core = self.clone();
            cx.defer_in(window, move |this, window, cx| {
                if (core.config().query)(this) == held {
                    core.settle(this, true, why, window, cx);
                }
            });
        }
        let mode = (self.config().auto_highlight)(this);
        if self.inner.auto_mode.get() != Some(mode) {
            let first = self.inner.auto_mode.replace(Some(mode)).is_none();
            if !first {
                let core = self.clone();
                cx.defer_in(window, move |this, window, cx| {
                    core.settle(this, false, Some(HighlightCause::new(Reason::Imperative, None)), window, cx);
                });
            }
        }
    }

    /// Says what changes the text next, before it changes (`cause`).
    pub fn cause(&self, reason: Reason, event: Option<InputEvent>) {
        *self.inner.cause.borrow_mut() = Some(HighlightCause::new(reason, event));
    }

    /// Settles the highlight for text a controlled query was asked to
    /// become, so a controlled highlight is asked for with the text, not a
    /// render later (`settleFor`).
    pub fn settle_for(&self, this: &mut C, next: SharedString, window: &mut Window, cx: &mut Context<C>) {
        *self.inner.held_query.borrow_mut() = Some((self.config().query)(this));
        *self.inner.asked.borrow_mut() = Some(next);
        let why = self.inner.cause.borrow().clone();
        self.settle(this, true, why, window, cx);
        *self.inner.asked.borrow_mut() = None;
    }

    /// The pointer highlights what it moves onto (`hover`). gpui's mouse
    /// moves never come from a finger, which scrolling past isn't choosing.
    pub fn hover(&self, this: &mut C, key: &SharedString, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<C>) {
        if !(self.config().state)(this).open || self.item_props(key).is_some_and(|props| props.disabled) {
            return;
        }
        if self.listbox.highlighted().as_ref() != Some(key) {
            let highlight = self.config().highlight.clone();
            highlight(
                this,
                Some(key.clone()),
                Some(HighlightCause::new(Reason::ItemHover, Some(InputEvent::from(event)))),
                window,
                cx,
            );
        }
    }

    /// Whether nothing matches. Composed items register once the list
    /// renders, so until then it's not known (`isEmpty`).
    pub fn is_empty(&self, this: &C) -> bool {
        ((self.config().items)(this).is_some() || (self.config().state)(this).settled) && self.listbox.count(this) == 0
    }

    /// How many items match (`itemCount`).
    pub fn item_count(&self, this: &C) -> usize {
        self.listbox.count(this)
    }

    /// The text the list filters by (`filterQuery` and `matchQuery`).
    pub fn filter_query(&self, this: &C) -> SharedString {
        (self.config().query)(this)
    }

    /// How the filter marks matches, for the item text parts (`matchMode`).
    pub fn match_mode(&self, this: &C) -> Option<MatchMode> {
        (self.config().filter)(this).match_mode()
    }

    /// What each item shows from the list as a whole: whether it matches
    /// (`listing`).
    pub fn listing(&self, this: &C) -> Listing {
        self.inner.listing.listing(this, &self.listbox, None)
    }

    /// The id the field's active descendant points at, once that option is in
    /// the page (`activeDescendant`).
    pub fn active_descendant(&self, this: &C) -> Option<SharedString> {
        let state = (self.config().state)(this);
        let highlighted = self.listbox.highlighted()?;
        if !state.open || state.focus_in_list {
            return None;
        }
        if !self.listbox.is_connected(&highlighted) {
            return None;
        }
        Some(option_id(&self.config().id, &highlighted))
    }
}

/// The text field (`fieldPart`): a combobox whose focus stays put while its
/// active descendant points at the highlighted option. `field` is the
/// field's part, like a native text field's root; the component adds its own
/// ARIA, and to `keys` the ones its `on_input_key_down` takes past the
/// core's steps ([`LIST_STEP_KEYS`](crate::LIST_STEP_KEYS)). It turns off
/// the field's own completion, capitals and spelling marks in its options.
pub fn field_part<C: 'static, S>(
    entity: &Entity<C>,
    instance: &Instance,
    field: Part<S>,
    open: bool,
    active_descendant: Option<SharedString>,
    keys: &'static [HandledKey],
    on_input_key_down: impl Fn(&mut C, &KeyDownEvent, &mut Window, &mut Context<C>) + 'static,
) -> Part<S> {
    let entity = entity.downgrade();
    field
        .with_element_id(instance.element_id("input", None))
        .aria_role(Role::Combobox)
        .aria_expanded(open)
        .aria(Aria::new().active_descendant(active_descendant))
        .on_keys(keys, move |event, window, cx| {
            entity
                .update(cx, |this, cx| on_input_key_down(this, event, window, cx))
                .ok();
        })
}

/// What an option part styles from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutocompleteOptionState {
    /// The highlighted option is the one Enter picks (`data-highlighted`).
    pub highlighted: bool,
    pub disabled: bool,
    /// The query hides it.
    pub hidden: bool,
}

/// Picks an option, inside the component's update.
pub type OptionPressFn<C> = Rc<dyn Fn(&mut C, &SharedString, InputEvent, &mut Window, &mut Context<C>)>;

/// An option (`optionPart`), by its key, for the slot `slot`, like
/// `"item"`. Nothing is selected in these lists: the highlighted option is
/// the one Enter picks, so it's the selected one, as the APG's list
/// autocomplete marks it. Real focus reaches options only after a screen
/// reader's tap moved it into the list. Typing hides an option the query
/// doesn't match; a press on one leaves focus in the field. A component
/// builds its item on it, with its own state from `Part::map_state`, as the
/// original spreads it into its item slot.
#[allow(clippy::too_many_arguments)]
pub fn option_part<C, X>(
    entity: &Entity<C>,
    instance: &Instance,
    slot: &'static str,
    core: &AutocompleteCore<C, X>,
    this: &C,
    key: &SharedString,
    props: &AutocompleteCoreItem,
    press: OptionPressFn<C>,
    cx: &App,
) -> Part<AutocompleteOptionState>
where
    C: 'static,
    X: Clone + Default + 'static,
{
    let state = (core.config().state)(this);
    let meta = props.meta();
    let hidden = !core.listbox.matches(this, key, &meta);
    let highlighted = core.listbox.highlighted().as_ref() == Some(key);
    let option = AutocompleteOptionState {
        highlighted,
        disabled: props.disabled,
        hidden,
    };
    let mut part = Part::new(instance, slot, Some(key), option)
        .with_element_id(option_id(instance.id(), key))
        .aria_role(Role::Option);
    part = if state.focus_in_list {
        let handle = core.listbox.focus_handle(key, cx);
        part.focus_mode(FocusMode::Roving { tab_stop: false }, Some(&handle), false)
    } else {
        part.focus_mode(FocusMode::ActiveDescendant, None, false)
    };
    if let Some(position) = position_attrs(props.index, &core.listbox, this) {
        part = position.apply(part);
    }
    let hovered = entity.downgrade();
    let pressed = entity.downgrade();
    let hover_core = core.clone();
    let hover_key = key.clone();
    let press_key = key.clone();
    let part = part
        .aria_selected(highlighted)
        .aria(Aria::new().disabled(props.disabled).hidden(hidden))
        .on_any_mouse_down(hold_focus)
        .on_mouse_move(move |event, window, cx| {
            let core = hover_core.clone();
            hovered
                .update(cx, |this, cx| core.hover(this, &hover_key, event, window, cx))
                .ok();
        })
        .on_press(move |event, window, cx| {
            pressed
                .update(cx, |this, cx| press(this, &press_key, event, window, cx))
                .ok();
        });
    let part = core.listbox.bind_item(key.clone(), meta, part, cx);
    if hidden { part.hidden() } else { part }
}
