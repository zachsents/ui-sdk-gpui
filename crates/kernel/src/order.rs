//! Port of `foundation/kernel/order.ts`: document order.
//!
//! The original sorts parts by where their elements are in the document,
//! since frameworks register them in any order. A gpui tree renders in
//! order, so page order is render order: the [`Parts`] of a slot keep keys
//! as they render, which is the original's `createOrder`, the shared, kept
//! list menus, roving groups, toggle groups and listboxes read. What's here
//! compares and sorts by it.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::Hash;

use super::elements::Parts;

/// Keyed items in page order, kept while they render (`createOrder`).
pub fn create_order<K: Clone + Eq + Hash + 'static>() -> Parts<K> {
    Parts::new()
}

/// Whether `later` comes after `key` in the page (`isAfter`). Keys not in
/// the page come after every key that is.
pub fn is_after<K: Clone + Eq + Hash + 'static, P: Clone + 'static>(
    parts: &Parts<K, P>,
    key: &K,
    later: &K,
) -> bool {
    document_order(parts, key, later) == Ordering::Less
}

/// Compares two keys for a sort into page order (`documentOrder`).
pub fn document_order<K: Clone + Eq + Hash + 'static, P: Clone + 'static>(
    parts: &Parts<K, P>,
    a: &K,
    b: &K,
) -> Ordering {
    let place = |key| parts.index_of(key).unwrap_or(usize::MAX);
    place(a).cmp(&place(b))
}

/// Entries in page order, by the key each has (`inDocumentOrder`). The sort
/// is stable, so entries the page doesn't hold keep their order at the end.
pub fn in_document_order<K: Clone + Eq + Hash + 'static, P: Clone + 'static, E>(
    parts: &Parts<K, P>,
    entries: &mut [E],
    key_of: impl Fn(&E) -> &K,
) {
    let places: HashMap<K, usize> = parts
        .keys()
        .into_iter()
        .enumerate()
        .map(|(place, key)| (key, place))
        .collect();
    entries.sort_by_key(|entry| places.get(key_of(entry)).copied().unwrap_or(usize::MAX));
}
