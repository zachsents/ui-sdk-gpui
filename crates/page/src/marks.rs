//! Port of `page/observe/marks.ts`: the marks an app writes on its own
//! elements for the agent bridge: a change a person must make
//! (`data-agent-confirm`), a region agents stay out of (`data-agent-hidden`)
//! and values that never reach the model (`data-agent-secret`). Each covers
//! its element and everything inside it, so one mark on a section covers
//! every control in it. People see no difference: the marks change no
//! behavior, style or ARIA, and only the bridge reads them.
//!
//! The original's marks are attributes on DOM elements, found with
//! `closest`. Here they're the kernel's [`AgentMarks`], which an app puts on
//! a part with `Part::agent_marks` and on any other element with an id with
//! `with_agent_marks`; the kernel records each marked element's place in the
//! page, its path of element ids, and an element is inside every marked one
//! whose path starts its own.

use gpui::{ElementId, WindowId};
use indexmap::IndexSet;
use ui_sdk_kernel::accesskit::{Node, Role as AccessRole};
use ui_sdk_kernel::{BoundPart, PageElement, page_ancestors};
use ui_sdk_text::collapse;

pub use ui_sdk_kernel::{AgentMarks, with_agent_marks};

/// An element the bridge reads, by its place in the page: a part, or an
/// element the kernel recorded.
pub trait Placed {
    fn window(&self) -> WindowId;
    /// Its element id and its ancestors', from the window down.
    fn path(&self) -> &[ElementId];
}

impl Placed for BoundPart {
    fn window(&self) -> WindowId {
        self.window
    }

    fn path(&self) -> &[ElementId] {
        &self.path
    }
}

impl Placed for PageElement {
    fn window(&self) -> WindowId {
        self.window
    }

    fn path(&self) -> &[ElementId] {
        &self.path
    }
}

/// Whether `outer` holds `inner`, or is it (`contains`).
pub(crate) fn contains(outer: &impl Placed, inner: &impl Placed) -> bool {
    outer.window() == inner.window() && inner.path().starts_with(outer.path())
}

// The marks at and around an element, nearest first (`composedClosest`).
fn marks_around(el: &impl Placed) -> impl Iterator<Item = AgentMarks> {
    page_ancestors(el.window(), el.path())
        .into_iter()
        .map(|element| element.marks.clone())
}

/// What the confirm marks a step touches say. Confirm text is page text that
/// reaches the model and the person, so it's cut short, like a label would
/// be. Every distinct mark a step touches is shown, so approving one warning
/// never approves another unseen.
pub fn confirm_of<'a, P: Placed + 'a>(elements: impl IntoIterator<Item = &'a P>) -> Option<String> {
    let mut texts = IndexSet::new();
    for el in elements {
        let text = marks_around(el).find_map(|marks| marks.confirm);
        if let Some(text) = text {
            let cut: String = text.trim().chars().take(200).collect();
            texts.insert(if cut.is_empty() { "This needs a person.".to_string() } else { cut });
        }
    }
    (!texts.is_empty()).then(|| texts.into_iter().collect::<Vec<_>>().join("; "))
}

/// Whether any of the elements is inside a region agents stay out of.
pub fn is_off_limits<'a, P: Placed + 'a>(elements: impl IntoIterator<Item = &'a P>) -> bool {
    elements
        .into_iter()
        .any(|el| marks_around(el).any(|marks| marks.hidden))
}

/// Whether any of the elements is inside a region whose values never reach
/// the model.
pub fn is_secret<'a, P: Placed + 'a>(elements: impl IntoIterator<Item = &'a P>) -> bool {
    elements
        .into_iter()
        .any(|el| marks_around(el).any(|marks| marks.secret))
}

/// An element's text as it reads, with its spaces collapsed: what it's
/// named with, `aria_label`, or a label's text. gpui keeps no text an element
/// draws where it can be read back, so a part's text is the name the app
/// gives it, and an element's own text the original walks to, like a check
/// mark a screen reader skips, never reaches it.
pub fn text_of(node: Option<&Node>) -> String {
    let Some(node) = node else {
        return String::new();
    };
    let text = match node.label() {
        Some(label) if !label.trim().is_empty() => label,
        _ if node.role() == AccessRole::Label => node.value().unwrap_or_default(),
        _ => "",
    };
    collapse(text)
}
