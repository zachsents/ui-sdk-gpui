//! Port of `foundation/popups/popup-elements.ts`: the elements that are a
//! popup's content, open or closed, for code that asks whether a target sits
//! in one, like a shortcut deciding whose it is, and for keys that bubble
//! from a popup nested in another.
//!
//! The original marks DOM elements in a registry every copy of the library
//! shares. Here a content is its [`Region`], which knows its bounds and the
//! focus handle its element tracks, registered with its slot's name, the
//! original's `data-slot`.

use std::cell::{Cell, RefCell};

use gpui::{App, SharedString, Window};
use ui_sdk_platform::{Region, Target};

struct Entry {
    id: u64,
    region: Region,
    slot: SharedString,
}

thread_local! {
    static REGISTRY: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    static NEXT: Cell<u64> = const { Cell::new(1) };
}

/// A popup's content in the registry, until released or dropped.
#[must_use = "dropping it unregisters the element"]
pub struct PopupElement {
    id: u64,
    done: Cell<bool>,
}

impl PopupElement {
    /// Undoes [`register_popup_element`]. Safe to call twice.
    pub fn release(&self) {
        if self.done.replace(true) {
            return;
        }
        let id = self.id;
        REGISTRY.with(|registry| registry.borrow_mut().retain(|entry| entry.id != id));
    }
}

impl Drop for PopupElement {
    fn drop(&mut self) {
        self.release();
    }
}

/// Marks a popup's content, by its region and its slot's name, like
/// `"menu-content"` (`registerPopupElement`). The returned guard undoes it.
pub fn register_popup_element(region: &Region, slot: impl Into<SharedString>) -> PopupElement {
    let id = NEXT.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    REGISTRY.with(|registry| {
        registry.borrow_mut().push(Entry {
            id,
            region: region.clone(),
            slot: slot.into(),
        })
    });
    PopupElement {
        id,
        done: Cell::new(false),
    }
}

/// Whether a target is inside a popup's content, open or closed
/// (`inPopupElement`): a point on one, or a focus handle inside one.
pub fn in_popup_element(target: &Target, window: &Window) -> bool {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .iter()
            .any(|entry| entry.region.contains(target, window))
    })
}

/// Whether the key the focused element hears is `el`'s own, not one bubbling
/// up from a part like it nested inside, like a submenu's keys through its
/// parent menu (`ownsEvent`). Parts of `el`'s own slot count as nested ones.
pub fn owns_event(el: &Region, window: &Window, cx: &App) -> bool {
    let slot = REGISTRY.with(|registry| {
        registry
            .borrow()
            .iter()
            .find(|entry| entry.region == *el)
            .map(|entry| entry.slot.clone())
    });
    match slot {
        Some(slot) => owns_event_among(el, &[slot.as_ref()], window, cx),
        None => owns_event_among(el, &[], window, cx),
    }
}

/// [`owns_event`], with the slots whose parts count as nested ones named,
/// like every dialog content's (`ownsEvent`'s `same`).
pub fn owns_event_among(el: &Region, same: &[&str], window: &Window, cx: &App) -> bool {
    let Some(focused) = window.focused(cx) else {
        return true;
    };
    let candidates: Vec<Region> = REGISTRY.with(|registry| {
        registry
            .borrow()
            .iter()
            .filter(|entry| entry.region != *el && same.contains(&entry.slot.as_ref()))
            .map(|entry| entry.region.clone())
            .collect()
    });
    // The innermost of them around focus is the one the key is for.
    !candidates.iter().any(|other| {
        let Some(handle) = other.focus_handle() else {
            return false;
        };
        let around = handle == focused || handle.contains(&focused, window);
        let inside_el = el
            .focus_handle()
            .is_some_and(|own| own != handle && own.contains(&handle, window));
        around && inside_el
    })
}
