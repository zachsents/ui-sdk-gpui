//! What the kernel records of the page for `observe` and the agent bridge,
//! which the original reads from the DOM: each part's and each element's
//! place, role, accessible node, labels and agent marks.
//!
//! gpui keeps no tree of elements to walk, so every part, and every element
//! given ARIA or marks with `with_aria` or `with_agent_marks`, records itself
//! here as it lays out, while a watcher is installed, and leaves when gpui
//! drops it. An element's place is its path, its id and its ancestors' with
//! ids, so one element holds another whose path starts with its own.
//!
//! The records are kept per window and per thread rather than in an app
//! global, so watchers, which hear changes without the app, can read them.
//! No TypeScript file corresponds.

use std::borrow::Borrow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use accesskit::{Node, Role as AccessRole};
use gpui::{ElementId, GlobalElementId, WindowId};

use super::attrs::AgentMarks;

/// An element in the page as it last laid out: a part, or an element given
/// ARIA or agent marks.
#[derive(Clone, Debug)]
pub struct PageElement {
    pub window: WindowId,
    /// Its element and every ancestor's with an id, from the window down.
    pub path: GlobalElementId,
    /// The element id its slot makes, for a part, else its own.
    pub element_id: Option<ElementId>,
    /// Its accesskit role.
    pub role: Option<AccessRole>,
    /// Its accessibility as gpui and the kernel write it, relations aside.
    pub accessible: Node,
    /// The elements that name it (`aria-labelledby`), by element id.
    pub labelled_by: Vec<ElementId>,
    /// The agent marks on it.
    pub marks: AgentMarks,
    /// When it last laid out, relative to the window's other elements, so
    /// sorting by it is page order among those that laid out together.
    pub order: u64,
    serial: u64,
}

// A path as a map key that a slice of element ids looks up.
#[derive(Clone, Debug)]
struct PathKey(GlobalElementId);

impl PartialEq for PathKey {
    fn eq(&self, other: &Self) -> bool {
        self.0[..] == other.0[..]
    }
}

impl Eq for PathKey {}

impl Hash for PathKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0[..].hash(state);
    }
}

impl Borrow<[ElementId]> for PathKey {
    fn borrow(&self) -> &[ElementId] {
        &self.0
    }
}

#[derive(Default)]
struct WindowPage {
    by_path: HashMap<PathKey, Rc<PageElement>>,
    by_id: HashMap<ElementId, PathKey>,
    order: u64,
}

thread_local! {
    static PAGE: RefCell<HashMap<WindowId, WindowPage>> = RefCell::new(HashMap::new());
}

/// What an element records as it lays out.
pub(crate) struct Recorded {
    pub window: WindowId,
    pub path: GlobalElementId,
    pub element_id: Option<ElementId>,
    pub role: Option<AccessRole>,
    pub accessible: Node,
    pub labelled_by: Vec<ElementId>,
    pub marks: AgentMarks,
    pub serial: u64,
}

pub(crate) fn record(recorded: Recorded) {
    PAGE.with(|page| {
        let Ok(mut page) = page.try_borrow_mut() else {
            return;
        };
        let window = page.entry(recorded.window).or_default();
        window.order += 1;
        let key = PathKey(recorded.path.clone());
        if let Some(id) = &recorded.element_id {
            window.by_id.insert(id.clone(), key.clone());
        }
        window.by_path.insert(
            key,
            Rc::new(PageElement {
                window: recorded.window,
                path: recorded.path,
                element_id: recorded.element_id,
                role: recorded.role,
                accessible: recorded.accessible,
                labelled_by: recorded.labelled_by,
                marks: recorded.marks,
                order: window.order,
                serial: recorded.serial,
            }),
        );
    });
}

// The element gpui dropped leaves, unless another took its place since.
pub(crate) fn forget(window: WindowId, path: &GlobalElementId, serial: u64) {
    PAGE.with(|page| {
        let Ok(mut page) = page.try_borrow_mut() else {
            return;
        };
        let Some(window) = page.get_mut(&window) else {
            return;
        };
        let Some(element) = window.by_path.get(&path[..]) else {
            return;
        };
        if element.serial != serial {
            return;
        }
        let id = element.element_id.clone();
        window.by_path.remove(&path[..]);
        if let Some(id) = id
            && window.by_id.get(&id).is_some_and(|key| key.0[..] == path[..])
        {
            window.by_id.remove(&id);
        }
    });
}

/// The element at `path` in `window`, if one recorded itself there.
pub fn page_element(window: WindowId, path: &[ElementId]) -> Option<Rc<PageElement>> {
    PAGE.with(|page| {
        page.borrow()
            .get(&window)
            .and_then(|window| window.by_path.get(path).cloned())
    })
}

/// The element in `window` with element id `id`: the one a part's slot
/// makes, or an element's own.
pub fn page_element_by_id(window: WindowId, id: &ElementId) -> Option<Rc<PageElement>> {
    PAGE.with(|page| {
        let page = page.borrow();
        let window = page.get(&window)?;
        let key = window.by_id.get(id)?;
        window.by_path.get(key).cloned()
    })
}

/// The recorded elements at and around `path`: itself first, when it's one,
/// then its ancestors, nearest first (`closest`).
pub fn page_ancestors(window: WindowId, path: &[ElementId]) -> Vec<Rc<PageElement>> {
    PAGE.with(|page| {
        let page = page.borrow();
        let Some(window) = page.get(&window) else {
            return Vec::new();
        };
        (1..=path.len())
            .rev()
            .filter_map(|end| window.by_path.get(&path[..end]).cloned())
            .collect()
    })
}

/// Every element recorded in `window`, in page order as of their last
/// layout.
pub fn page_elements(window: WindowId) -> Vec<Rc<PageElement>> {
    PAGE.with(|page| {
        let page = page.borrow();
        let Some(window) = page.get(&window) else {
            return Vec::new();
        };
        let mut elements: Vec<Rc<PageElement>> = window.by_path.values().cloned().collect();
        elements.sort_by_key(|element| element.order);
        elements
    })
}

/// The elements recorded at and inside `path` in `window`, in page order as
/// of their last layout (`querySelectorAll`, with the element itself).
pub fn page_elements_in(window: WindowId, path: &[ElementId]) -> Vec<Rc<PageElement>> {
    PAGE.with(|page| {
        let page = page.borrow();
        let Some(window) = page.get(&window) else {
            return Vec::new();
        };
        let mut elements: Vec<Rc<PageElement>> = window
            .by_path
            .values()
            .filter(|element| element.path.starts_with(path))
            .cloned()
            .collect();
        elements.sort_by_key(|element| element.order);
        elements
    })
}

/// When the element at `path` last laid out, relative to `window`'s others.
pub fn page_order(window: WindowId, path: &[ElementId]) -> Option<u64> {
    page_element(window, path).map(|element| element.order)
}
