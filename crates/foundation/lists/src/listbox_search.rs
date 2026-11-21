//! Port of `foundation/lists/listbox-search.ts`: what the lists people
//! search share beyond filtering: an item's text with what matched marked,
//! result counts said to screen readers once typing pauses, the next page
//! loading when the end of the list is reached, and whether opening moves
//! real focus into the list for touch screen readers, which ignore an active
//! descendant.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, Context, InteractiveElement, ScrollHandle, SharedString, Task, WeakEntity,
    Window, px,
};
use ui_sdk_kernel::{ChangeSource, InputEvent, Instance, Part};
use ui_sdk_platform::announce;
use ui_sdk_text::{MatchMode, MatchSegment, js_trim, match_ranges, match_segments};

use crate::async_options::AsyncOptionsStatus;
use crate::listbox_engine::ListFilter;

/// What a list announces to screen readers: how many items match after
/// typing, or a removed value (`ListAnnouncement`).
#[derive(Clone, Debug, PartialEq)]
pub enum ListAnnouncement {
    Results {
        /// How many items match.
        count: usize,
        /// The text they match.
        query: SharedString,
        /// Whether results are still loading.
        loading: bool,
    },
    Removed {
        /// The label of the value that was removed.
        label: SharedString,
    },
}

/// What a list announces by default, in English: "3 results", "No results",
/// "Loading" or "Pear removed" (`describeList`).
pub fn describe_list(change: &ListAnnouncement) -> SharedString {
    match change {
        ListAnnouncement::Removed { label } => format!("{label} removed").into(),
        ListAnnouncement::Results { loading: true, .. } => "Loading".into(),
        ListAnnouncement::Results { count: 0, .. } => "No results".into(),
        ListAnnouncement::Results { count: 1, .. } => "1 result".into(),
        ListAnnouncement::Results { count, .. } => format!("{count} results").into(),
    }
}

/// What an item's text part styles from: its text split where it matched
/// what was typed (`ListItemTextState`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItemTextState {
    /// The text in pieces, each marked when it matched. Joined, they're the
    /// whole text.
    pub segments: Vec<MatchSegment>,
    /// Whether any piece matched (`data-matched`).
    pub matched: bool,
}

/// What an item's text part takes (`ListItemTextProps`): the text to mark,
/// its own or its item's label.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListItemTextProps {
    pub text: SharedString,
}

impl ListItemTextProps {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

/// How a filter marks matches, or `None` for one that can't say, like a
/// function (`matchMode`).
pub fn match_mode<T, X>(filter: Option<&ListFilter<T, X>>) -> Option<MatchMode> {
    match filter {
        Some(ListFilter::Contains) => Some(MatchMode::Contains),
        Some(ListFilter::StartsWith) => Some(MatchMode::StartsWith),
        _ => None,
    }
}

fn unmatched(text: &SharedString) -> ListItemTextState {
    ListItemTextState {
        segments: vec![MatchSegment {
            text: text.clone(),
            matched: false,
        }],
        matched: false,
    }
}

struct Marked {
    text: SharedString,
    query: SharedString,
    state: ListItemTextState,
}

/// The marked texts of a component's items, by key (`itemTextPart`'s memo):
/// the same text and query give the same state, worked out once. Keep one in
/// a field.
#[derive(Default)]
pub struct ItemTexts {
    cache: RefCell<HashMap<SharedString, Marked>>,
}

impl ItemTexts {
    pub fn new() -> Self {
        Self::default()
    }

    /// An item's text, split where `query` matched the way `mode` matches,
    /// so the parts can be marked. With no `mode`, nothing is marked.
    pub fn state(&self, key: &SharedString, text: &SharedString, query: &str, mode: Option<MatchMode>) -> ListItemTextState {
        let query: SharedString = match mode {
            Some(_) => SharedString::from(query.to_string()),
            None => SharedString::default(),
        };
        let mut cache = self.cache.borrow_mut();
        if let Some(last) = cache.get(key)
            && &last.text == text
            && last.query == query
        {
            return last.state.clone();
        }
        let ranges = match mode {
            Some(mode) if !js_trim(&query).is_empty() => match_ranges(text, &query, mode),
            _ => Vec::new(),
        };
        let state = if !ranges.is_empty() {
            ListItemTextState {
                segments: match_segments(text, &ranges),
                matched: true,
            }
        } else {
            match cache.get(key) {
                Some(last) if !last.state.matched && &last.text == text => last.state.clone(),
                _ => unmatched(text),
            }
        };
        cache.insert(
            key.clone(),
            Marked {
                text: text.clone(),
                query,
                state: state.clone(),
            },
        );
        state
    }
}

/// An item's text, split where the query matched, so the parts can be
/// marked (`itemTextPart`), by its item's key. Render `state.segments`, like
/// with gpui's `StyledText::with_highlights`.
pub fn item_text_part(instance: &Instance, key: &str, state: ListItemTextState) -> Part<ListItemTextState> {
    Part::new(instance, "item-text", Some(key), state)
}

/// Whether opening from this event should move real focus into the list
/// (`opensIntoList`). VoiceOver on iOS ignores an active descendant, so a
/// screen reader's tap, which arrives as a click with no pointer or key
/// behind it, gets real focus.
pub fn opens_into_list(event: Option<&InputEvent>) -> bool {
    event.is_some_and(|event| event.source() == ChangeSource::Virtual)
}

/// How long typing must pause before result counts are announced.
pub const ANNOUNCE_DELAY: Duration = Duration::from_millis(500);

/// A list's own announcement in place of [`describe_list`], or `None` to say
/// nothing.
pub type ListAnnouncementFn = Rc<dyn Fn(&ListAnnouncement) -> Option<SharedString>>;

/// What [`announce_results`] reads from the component.
pub struct AnnounceResultsOptions<C> {
    /// The text the list is filtered by.
    pub query: Rc<dyn Fn(&C) -> SharedString>,
    /// How many items match.
    pub count: Rc<dyn Fn(&C) -> usize>,
    /// Whether results are loading.
    pub loading: Rc<dyn Fn(&C) -> bool>,
    /// Whether the list is on screen.
    pub shown: Rc<dyn Fn(&C) -> bool>,
    /// Whether the component says it itself, like with a status part.
    pub silent: Rc<dyn Fn(&C) -> bool>,
    /// The component's `announcement` option.
    pub announcement: Rc<dyn Fn(&C) -> Option<ListAnnouncementFn>>,
}

/// Says how many items match once typing pauses (`announceResults`). Make
/// it in the component's constructor, keep it in a field, and call
/// [`AnnounceResults::changed`] after the component's state changes, as the
/// original hears every change to the store.
pub struct AnnounceResults<C: 'static> {
    options: Rc<AnnounceResultsOptions<C>>,
    entity: WeakEntity<C>,
    window: AnyWindowHandle,
    // The text and whether results were loading, as last heard, from where
    // the component starts.
    previous: Rc<RefCell<Option<(SharedString, bool)>>>,
    // Results still loading are said again once they arrive.
    waiting: Rc<Cell<bool>>,
    timer: RefCell<Option<Task<()>>>,
}

/// Says how many items match once typing pauses, through the page's shared
/// announcer, unless the component renders a status part, which says it
/// instead (`announceResults`). Results that were loading are said again
/// once they arrive. It takes in where the component starts once the
/// component is made, as the original reads it as it's called.
pub fn announce_results<C: 'static>(
    options: AnnounceResultsOptions<C>,
    window: &Window,
    cx: &mut Context<C>,
) -> AnnounceResults<C> {
    let options = Rc::new(options);
    let previous: Rc<RefCell<Option<(SharedString, bool)>>> = Rc::default();
    {
        let options = options.clone();
        let previous = previous.clone();
        cx.defer_in(window, move |this, _window, _cx| {
            previous
                .borrow_mut()
                .get_or_insert_with(|| ((options.query)(this), (options.loading)(this)));
        });
    }
    AnnounceResults {
        options,
        entity: cx.weak_entity(),
        window: window.window_handle(),
        previous,
        waiting: Rc::new(Cell::new(false)),
        timer: RefCell::new(None),
    }
}

impl<C: 'static> AnnounceResults<C> {
    /// Hears the component's state now. Typing, or results that were
    /// loading arriving, says the count once typing pauses.
    pub fn changed(&self, this: &C, cx: &App) {
        let next = (self.options.query)(this);
        let loading = (self.options.loading)(this);
        // A change heard before the start was taken in is where it starts.
        let Some((previous_query, previous_loading)) = self.previous.borrow_mut().replace((next.clone(), loading))
        else {
            return;
        };
        let typed = next != previous_query;
        let arrived = previous_loading && !loading && self.waiting.get();
        if !typed && !arrived {
            return;
        }
        let options = self.options.clone();
        let entity = self.entity.clone();
        let window = self.window;
        let waiting = self.waiting.clone();
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(ANNOUNCE_DELAY).await;
            window
                .update(cx, |_, window, cx| {
                    let Some(entity) = entity.upgrade() else {
                        return;
                    };
                    let message = {
                        let this = entity.read(cx);
                        let query = (options.query)(this);
                        // An empty field shows everything, or suggestions,
                        // which aren't results to count.
                        if !(options.shown)(this) || (options.silent)(this) || js_trim(&query).is_empty() {
                            return;
                        }
                        let change = ListAnnouncement::Results {
                            count: (options.count)(this),
                            query,
                            loading: (options.loading)(this),
                        };
                        let message = match (options.announcement)(this) {
                            Some(announcement) => announcement(&change),
                            None => Some(describe_list(&change)),
                        };
                        waiting.set(matches!(change, ListAnnouncement::Results { loading: true, .. }));
                        message
                    };
                    if let Some(message) = message {
                        announce(&message, window, cx);
                    }
                })
                .ok();
        });
        *self.timer.borrow_mut() = Some(task);
    }
}

/// Opens the next page, from the component's `on_load_more`.
pub type LoadMoreFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// Asks for the next page once the end of the list is reached, by the
/// highlight or by scrolling, at most once per page: not while one loads,
/// and not again for the same items unless the last load failed
/// (`createLoadMore`). Cloning it gives another handle to the same one.
pub struct LoadMore<C: 'static> {
    count: Rc<dyn Fn(&C) -> usize>,
    status: Rc<dyn Fn(&C) -> AsyncOptionsStatus>,
    on_load_more: Rc<dyn Fn(&C) -> Option<LoadMoreFn>>,
    requested_at: Rc<Cell<Option<usize>>>,
}

impl<C: 'static> Clone for LoadMore<C> {
    fn clone(&self) -> Self {
        Self {
            count: self.count.clone(),
            status: self.status.clone(),
            on_load_more: self.on_load_more.clone(),
            requested_at: self.requested_at.clone(),
        }
    }
}

/// Asks for the next page once the end of the list is reached
/// (`createLoadMore`): `count` is how many items show, `status` whether a
/// page is loading or failed, and `on_load_more` the component's option.
pub fn create_load_more<C: 'static>(
    count: impl Fn(&C) -> usize + 'static,
    status: impl Fn(&C) -> AsyncOptionsStatus + 'static,
    on_load_more: impl Fn(&C) -> Option<LoadMoreFn> + 'static,
) -> LoadMore<C> {
    LoadMore {
        count: Rc::new(count),
        status: Rc::new(status),
        on_load_more: Rc::new(on_load_more),
        requested_at: Rc::new(Cell::new(None)),
    }
}

impl<C: 'static> LoadMore<C> {
    /// The end of the list was reached.
    pub fn reached(&self, this: &C, window: &mut Window, cx: &mut App) {
        let Some(load) = (self.on_load_more)(this) else {
            return;
        };
        let current = (self.status)(this);
        if current == AsyncOptionsStatus::Loading {
            return;
        }
        let count = (self.count)(this);
        if self.requested_at.get() == Some(count) && current != AsyncOptionsStatus::Error {
            return;
        }
        self.requested_at.set(Some(count));
        load(window, cx);
    }

    /// Hears scrolling near the end of the list's scroller (`bind`), within
    /// the height of a row or so of it. `entity` is the component's. gpui
    /// can't hear every scroller inside an element, so the component names
    /// its list's.
    pub fn bind<E: InteractiveElement>(&self, el: E, scroll: ScrollHandle, entity: WeakEntity<C>) -> E {
        let load_more = self.clone();
        el.on_scroll(move |_event, window, cx| {
            let load_more = load_more.clone();
            let scroll = scroll.clone();
            let entity = entity.clone();
            // The scroller moves as it hears the wheel too; look once it has.
            window.defer(cx, move |window, cx| {
                let max = scroll.max_offset().y;
                if -scroll.offset().y < max - px(40.) {
                    return;
                }
                entity
                    .update(cx, |this, cx| load_more.reached(this, window, cx))
                    .ok();
            });
        })
    }
}
