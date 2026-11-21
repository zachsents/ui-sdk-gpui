//! Port of `foundation/text/name-check.ts` and `name-check-stub.ts`.
//!
//! Development warnings for parts nobody can name. The original swaps in a
//! stub for production builds through a conditional import; here release
//! builds compile the same functions down to nothing, `cfg!(debug_assertions)`
//! being the switch.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{ElementId, SharedString, Window};
use ui_sdk_kernel::dev::{self, Origin};

use crate::accessible_name::{NameSources, has_accessible_name};

/// Where a warning comes from: the component and its instance's id, kept
/// until the check runs (the kernel's [`Origin`], owned).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameFrom {
    pub component: &'static str,
    pub id: Option<SharedString>,
}

thread_local! {
    // Elements another component names, like an icon button its tooltip
    // names, and whether it does now.
    static NAMERS: RefCell<HashMap<ElementId, Rc<dyn Fn() -> bool>>> = RefCell::new(HashMap::new());
    // Elements already checked, so a part that renders every frame warns once,
    // as the original warns once when it binds.
    static CHECKED: RefCell<HashSet<ElementId>> = RefCell::new(HashSet::new());
}

/// Tells [`check_name`] that a component names the element `id` while
/// `names()` says so, like a tooltip naming an icon trigger it shares with a
/// popover. Returns a cleanup.
pub fn name_from(id: ElementId, names: Rc<dyn Fn() -> bool>) -> impl FnOnce() {
    if cfg!(debug_assertions) {
        NAMERS.with(|namers| namers.borrow_mut().insert(id.clone(), names.clone()));
    }
    move || {
        if cfg!(debug_assertions) {
            NAMERS.with(|namers| {
                let mut namers = namers.borrow_mut();
                if namers.get(&id).is_some_and(|current| Rc::ptr_eq(current, &names)) {
                    namers.remove(&id);
                }
            });
        }
    }
}

/// Warns when a part has no accessible name and no component names it
/// through [`name_from`]. Names come from content or ARIA, never a `label`
/// option, and every part warns through here, so the inspector shows it.
/// Hidden text, like an icon marked hidden, names nothing. It checks after
/// the frame, once every part on the element has rendered, so a tooltip that
/// names the same element counts. Each element is checked once.
pub fn check_name(id: &ElementId, sources: &NameSources, part: &str, from: NameFrom, window: &Window) {
    if !cfg!(debug_assertions) {
        return;
    }
    let first = CHECKED.with(|checked| checked.borrow_mut().insert(id.clone()));
    if !first || has_accessible_name(sources) {
        return;
    }
    let id = id.clone();
    let part = part.to_string();
    window.on_next_frame(move |_window, _cx| {
        let named = NAMERS.with(|namers| namers.borrow().get(&id).cloned());
        if named.is_some_and(|names| names()) {
            return;
        }
        dev::warn(
            &format!("{part} has no accessible name. Add text or an aria-label."),
            Some(Origin {
                component: from.component,
                id: from.id.as_deref(),
            }),
        );
    });
}
