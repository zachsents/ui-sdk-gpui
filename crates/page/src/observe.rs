//! Port of `page/observe/observe.ts`: every change to a ui-sdk component on
//! the page, from a person or an agent, with the component's name, the
//! reason and the source, for logs, analytics, audit trails and replays,
//! settled once per commit or step by step. Opt in: an app that never calls
//! it, or the agent, pays only the kernel's empty hooks.
//!
//! Listeners run as the change goes through, inside the component's update,
//! without the app: keep what they hear, or send it on, and update entities
//! from somewhere else, like a task that drains a channel.

use std::rc::Rc;

use gpui::{App, ElementId, Subscription};
use ui_sdk_kernel::BoundPart;

pub use crate::registry::ObservedChange;
pub use ui_sdk_kernel::with_source;

use crate::marks::Placed;
use crate::registry::{elements_of, listen};

/// The element `observe` and `create_history` listen inside, like one panel
/// of a page: an ancestor's element id, or a function that finds one, which
/// can return one that renders later.
#[derive(Clone)]
pub enum Within {
    Element(ElementId),
    Find(Rc<dyn Fn() -> Option<ElementId>>),
}

impl Within {
    /// The element with `id` around the components to hear.
    pub fn element(id: impl Into<ElementId>) -> Self {
        Within::Element(id.into())
    }

    /// The element `find` returns when a change comes.
    pub fn find(find: impl Fn() -> Option<ElementId> + 'static) -> Self {
        Within::Find(Rc::new(find))
    }

    fn root(&self) -> Option<ElementId> {
        match self {
            Within::Element(id) => Some(id.clone()),
            Within::Find(find) => find(),
        }
    }

    /// Whether a part is inside the element: the element is the part or one
    /// of its ancestors (`root.contains(el)`).
    pub(crate) fn holds(&self, parts: &[BoundPart]) -> bool {
        let Some(root) = self.root() else {
            return false;
        };
        parts.iter().any(|part| part.path().iter().any(|id| *id == root))
    }
}

impl From<ElementId> for Within {
    fn from(id: ElementId) -> Self {
        Within::Element(id)
    }
}

impl From<&'static str> for Within {
    fn from(id: &'static str) -> Self {
        Within::Element(id.into())
    }
}

/// What `observe` hears.
#[derive(Clone, Default)]
pub struct ObserveOptions {
    /// Hears only components with a part inside this element, or the element
    /// it finds.
    pub within: Option<Within>,
    /// Hears every step of an interaction, like each move of a drag, instead
    /// of one change when it commits.
    pub live: bool,
}

impl ObserveOptions {
    pub fn within(mut self, within: impl Into<Within>) -> Self {
        self.within = Some(within.into());
        self
    }

    pub fn live(mut self, live: bool) -> Self {
        self.live = live;
        self
    }
}

/// Calls `listener` with every change to a component that goes through,
/// made by a person or an agent, until the returned subscription drops. Each
/// names the component the way the agent's `snapshot` does, says why it
/// changed and who changed it, and carries the value before it. A change an
/// interaction makes in steps and then commits, like a slider's drag, a
/// number input's typing or a resizable's drag, is reported once, when it
/// commits, with the value from before it began; pass `live` to hear every
/// step instead, and no commits. Password and one-time-code values read
/// `"[redacted]"`. Pass `within` to hear only components inside an element,
/// like one panel of a page. Call it before the page renders: it names
/// components it heard being made.
///
/// ```ignore
/// observe(|change| log::info!("{:?}: {} {} → {} ({}, {})", change.label, change.key, change.previous, change.value, change.reason.as_str(), change.source.as_str()), ObserveOptions::default(), cx)
///     .detach();
/// // Visibility: value private → public (item-press, mouse)
/// // Volume: value 20 → 65 (drag, mouse), once the drag ends
/// ```
pub fn observe(
    listener: impl Fn(&ObservedChange) + 'static,
    options: ObserveOptions,
    cx: &mut App,
) -> Subscription {
    let ObserveOptions { within, live } = options;
    match within {
        None => listen(move |change, _parts| listener(change), live, cx),
        Some(within) => listen(
            move |change, parts| {
                if within.holds(parts) {
                    listener(change)
                }
            },
            live,
            cx,
        ),
    }
}

/// The element of a component, by the ref `observe` and the agent's
/// `snapshot` give it: its root, or its first part on the page, as the part
/// the kernel recorded (its bounds, element id and place). For pointing at a
/// component, like a highlight around what an agent just changed.
pub fn element_of(r#ref: &str) -> Option<BoundPart> {
    let elements = elements_of(r#ref);
    let outermost = elements.iter().find(|el| {
        !elements.iter().any(|other| {
            !std::ptr::eq(*el, other) && other.window == el.window && el.path.starts_with(&other.path)
        })
    });
    outermost.or_else(|| elements.first()).cloned()
}
