//! Port of `foundation/kernel/bound-keys.ts`: what the parts of one kind that
//! are in the page say about it, so a component points only at parts that
//! exist: whether a title is in the page, or the keys of the group labels
//! that are.
//!
//! The original keeps these in state, written a microtask after the parts
//! bind. Here a part is in the page while gpui draws it, which the registry
//! in `part.rs` records, and a component asks. Asking about a slot makes the
//! component re-render when one of its parts comes or goes, so what it read
//! is current the frame after. Relations already leave out parts that aren't
//! in the page; these are for what else depends on it, like a list part's
//! role or a popup named by its trigger when it has no title.

use gpui::SharedString;

use super::primitive::Instance;

/// Whether a part of `slot` is in the page (`flagWhileBound`), or `None`
/// until a frame has drawn the component's parts, like a title before its
/// popup first renders (`settleFlag`: null until the parts bind, false once
/// they have without it).
pub fn flag_while_bound(instance: &Instance, slot: &'static str) -> Option<bool> {
    let shared = instance.shared();
    shared.watched.borrow_mut().insert(slot);
    if !shared.drawn.get() {
        return None;
    }
    Some(shared.bound.borrow().values().any(|part| part.slot == slot))
}

/// Whether a part of `slot` is in the page, `false` until one is.
pub fn bound(instance: &Instance, slot: &'static str) -> bool {
    flag_while_bound(instance, slot).unwrap_or(false)
}

/// The keys of `slot`'s parts in the page, in the order they bound
/// (`keyWhileBound`), like the groups whose labels render.
pub fn key_while_bound(instance: &Instance, slot: &'static str) -> Vec<SharedString> {
    let shared = instance.shared();
    shared.watched.borrow_mut().insert(slot);
    shared
        .bound
        .borrow()
        .values()
        .filter(|part| part.slot == slot)
        .filter_map(|part| part.key.clone())
        .collect()
}
