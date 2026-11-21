//! Port of `foundation/selection/listbox-range.ts`: the selection math of a
//! listbox with `multiple`, on keys in list order: toggling one, replacing
//! the selection with one, and Shift ranges. A range runs from the anchor,
//! the option chosen last without Shift, to the option pressed, and adds to
//! the base, the selection when the anchor was set. So extending a range
//! again replaces the range, not the base, like a file manager. Pure, so it
//! needs no window.

/// Whether two key lists hold the same keys in the same order, so an
/// unchanged selection fires no change (`isSameKeys`). Defined in drag and
/// drop, which uses it too and which selection builds on.
pub use ui_sdk_drag_and_drop::is_same_keys;

/// Where Shift ranges start, and what they add to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeAnchor<K> {
    pub key: K,
    pub base: Vec<K>,
}

impl<K> RangeAnchor<K> {
    pub fn new(key: K, base: Vec<K>) -> Self {
        Self { key, base }
    }
}

/// The keys from `from` to `to` in list order, both included, in either
/// direction (`keysBetween`). Empty when either isn't in the list.
pub fn keys_between<K: Clone + PartialEq>(order: &[K], from: &K, to: &K) -> Vec<K> {
    let (Some(start), Some(end)) = (
        order.iter().position(|key| key == from),
        order.iter().position(|key| key == to),
    ) else {
        return Vec::new();
    };
    order[start.min(end)..=start.max(end)].to_vec()
}

/// The selection a Shift range makes: the anchor's base, unless the range
/// replaces the selection, plus every key between the anchor and `to` that
/// can be chosen (`rangeSelection`). Set the anchor (`RangeAnchor { key,
/// base: selection }`) on every choice made without Shift, so extending a
/// range again replaces the range, not the base, like a file manager.
pub fn range_selection<K: Clone + PartialEq>(
    order: &[K],
    anchor: &RangeAnchor<K>,
    to: &K,
    additive: bool,
    selectable: impl Fn(&K) -> bool,
) -> Vec<K> {
    let range: Vec<K> = keys_between(order, &anchor.key, to)
        .into_iter()
        .filter(|key| selectable(key))
        .collect();
    let base: &[K] = if additive { &anchor.base } else { &[] };
    let mut next = base.to_vec();
    next.extend(range.into_iter().filter(|key| !base.contains(key)));
    next
}

/// The selection with `key` added, or removed when it's there, like
/// Ctrl+Space (`toggledKeys`).
pub fn toggled_keys<K: Clone + PartialEq>(selected: &[K], key: &K) -> Vec<K> {
    if selected.contains(key) {
        selected.iter().filter(|item| *item != key).cloned().collect()
    } else {
        let mut next = selected.to_vec();
        next.push(key.clone());
        next
    }
}

/// Every key that can be chosen, or none once they all are, like Ctrl+A
/// pressed twice (`allToggled`). Keys that can't be chosen keep whatever
/// state they had.
pub fn all_toggled<K: Clone + PartialEq>(selected: &[K], selectable: &[K]) -> Vec<K> {
    if !selectable.is_empty() && selectable.iter().all(|key| selected.contains(key)) {
        return selected
            .iter()
            .filter(|key| !selectable.contains(key))
            .cloned()
            .collect();
    }
    let mut next = selected.to_vec();
    next.extend(selectable.iter().filter(|key| !selected.contains(key)).cloned());
    next
}
