//! Port of `foundation/lists/listbox.ts`: filtering for the listbox engine
//! behind Select, Combobox, Autocomplete, Command and TriggerMenu, ranked by
//! score with case and accents folded. Listbox, which nobody types into,
//! uses the engine alone.

use std::cmp::Ordering;
use std::fmt::Debug;

use gpui::{Context, Window};
use ui_sdk_text::{MatchMode, fold_text, js_trim, leading_spaces};

use crate::listbox_engine::{
    ListData, ListFilter, ListFilterResult, ListFiltering, ListGroup, ListItem, ListNode, ListSeparator,
    ListboxEngine, ListboxEngineConfig, create_listbox_engine,
};

/// Whether text matches a query the way a filter asks: `StartsWith` from
/// its start, after leading spaces, and anything else wherever it appears.
/// Case and accents are ignored the way the user's language ignores them.
pub fn text_matches(text: &str, query: &str, mode: MatchMode) -> bool {
    let needle = fold_text(js_trim(query));
    if needle.is_empty() {
        return true;
    }
    let haystack = fold_text(text);
    match mode {
        MatchMode::StartsWith => haystack[leading_spaces(&haystack)..].starts_with(needle.as_ref()),
        _ => haystack.contains(needle.as_ref()),
    }
}

/// How well an item matches a query: 0 hides it, and higher ranks it
/// first.
pub fn item_score<T, X>(item: &ListItem<T, X>, query: &str, filter: Option<&ListFilter<T, X>>) -> f64 {
    let filter = match filter {
        None | Some(ListFilter::Off) => return 1.0,
        Some(filter) => filter,
    };
    if js_trim(query).is_empty() {
        return 1.0;
    }
    let mode = match filter {
        ListFilter::Custom(filter) => {
            return match filter(item, query) {
                ListFilterResult::Keep(keep) => {
                    if keep {
                        1.0
                    } else {
                        0.0
                    }
                }
                ListFilterResult::Score(score) => score.max(0.0),
            };
        }
        ListFilter::StartsWith => MatchMode::StartsWith,
        _ => MatchMode::Contains,
    };
    let matched = text_matches(&item.label, query, mode)
        || item.keywords.iter().any(|keyword| text_matches(keyword, query, mode));
    if matched { 1.0 } else { 0.0 }
}

/// Whether an item shows for a query.
pub fn item_matches<T, X>(item: &ListItem<T, X>, query: &str, filter: Option<&ListFilter<T, X>>) -> bool {
    item_score(item, query, filter) > 0.0
}

/// The data a list shows for a query: matching items, groups that still
/// have some, and separators between what's left. Ranking puts the best
/// first, and drops separators once it moves an entry out of the sections
/// they divide.
pub fn filter_data<T: Clone, X: Clone>(
    data: &ListData<T, X>,
    query: &str,
    filter: Option<&ListFilter<T, X>>,
    rank: bool,
) -> ListData<T, X> {
    let filter = match filter {
        None | Some(ListFilter::Off) => return data.clone(),
        Some(filter) => filter,
    };
    if js_trim(query).is_empty() {
        return data.clone();
    }
    if rank {
        return rank_data(data, query, filter);
    }
    let mut result: Vec<ListNode<T, X>> = Vec::new();
    // A separator waits for the next kept entry, so none leads, trails or doubles.
    let mut separator = false;
    let keep = |result: &mut Vec<ListNode<T, X>>, node: ListNode<T, X>, separator: &mut bool| {
        if *separator && !result.is_empty() {
            result.push(ListNode::Separator(ListSeparator));
        }
        *separator = false;
        result.push(node);
    };
    for node in data.iter() {
        match node {
            ListNode::Separator(_) => separator = true,
            ListNode::Group(group) => {
                let items: Vec<ListItem<T, X>> = group
                    .items
                    .iter()
                    .filter(|item| item_matches(item, query, Some(filter)))
                    .cloned()
                    .collect();
                if !items.is_empty() {
                    keep(
                        &mut result,
                        ListNode::Group(ListGroup {
                            label: group.label.clone(),
                            items,
                        }),
                        &mut separator,
                    );
                }
            }
            ListNode::Item(item) => {
                if item_matches(item, query, Some(filter)) {
                    keep(&mut result, node.clone(), &mut separator);
                }
            }
        }
    }
    result.into()
}

struct Ranked<T, X> {
    node: ListNode<T, X>,
    score: f64,
    // Its place before sorting.
    place: usize,
    // Whether a separator came before it.
    separator: bool,
}

// Sorts by score, best first. Equal scores keep their order, since the sort
// is stable.
fn by_score<T, X>(a: &Ranked<T, X>, b: &Ranked<T, X>) -> Ordering {
    b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal)
}

fn rank_data<T: Clone, X: Clone>(data: &ListData<T, X>, query: &str, filter: &ListFilter<T, X>) -> ListData<T, X> {
    let mut ranked: Vec<Ranked<T, X>> = Vec::new();
    let mut separator = false;
    let keep = |ranked: &mut Vec<Ranked<T, X>>, node: ListNode<T, X>, score: f64, separator: &mut bool| {
        let place = ranked.len();
        ranked.push(Ranked {
            node,
            score,
            place,
            separator: place > 0 && *separator,
        });
        *separator = false;
    };
    for node in data.iter() {
        match node {
            ListNode::Separator(_) => separator = true,
            ListNode::Group(group) => {
                let mut items: Vec<(ListItem<T, X>, f64)> = Vec::new();
                for item in &group.items {
                    let score = item_score(item, query, Some(filter));
                    if score > 0.0 {
                        items.push((item.clone(), score));
                    }
                }
                if items.is_empty() {
                    continue;
                }
                items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
                let best = items[0].1;
                keep(
                    &mut ranked,
                    ListNode::Group(ListGroup {
                        label: group.label.clone(),
                        items: items.into_iter().map(|(item, _)| item).collect(),
                    }),
                    best,
                    &mut separator,
                );
            }
            ListNode::Item(item) => {
                let score = item_score(item, query, Some(filter));
                if score > 0.0 {
                    keep(&mut ranked, node.clone(), score, &mut separator);
                }
            }
        }
    }
    let mut sorted: Vec<&Ranked<T, X>> = ranked.iter().collect();
    sorted.sort_by(|a, b| by_score(a, b));
    if sorted.iter().enumerate().any(|(index, item)| item.place != index) {
        return sorted.into_iter().map(|item| item.node.clone()).collect::<Vec<_>>().into();
    }
    let mut result = Vec::new();
    for item in ranked {
        if item.separator {
            result.push(ListNode::Separator(ListSeparator));
        }
        result.push(item.node);
    }
    result.into()
}

/// The filtering the engine runs with (`filtering`): [`filter_data`] and
/// [`item_matches`].
pub fn filtering<T: Clone, X: Clone>() -> ListFiltering<T, X> {
    ListFiltering {
        data: filter_data::<T, X>,
        matches: item_matches::<T, X>,
    }
}

/// The listbox engine, filtering by `query` the way `filter` asks
/// (`createFilteredListbox`).
pub fn create_filtered_listbox<C, T, X>(
    config: ListboxEngineConfig<C, T, X>,
    window: &Window,
    cx: &mut Context<C>,
) -> ListboxEngine<C, T, X>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    create_listbox_engine(config, filtering(), window, cx)
}
