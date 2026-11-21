//! Port of `foundation/platform/announce.ts`: a polite and an assertive live
//! region per window, for text that changes without a region of its own, like
//! a toggletip's when a press opens it, files arriving in a drop zone, or a
//! toast. Live regions stay live over modal layers, so announcements reach a
//! screen reader from anywhere.
//!
//! The original appends hidden live regions to the document's body. gpui has
//! no body to append to, so the regions are an element the app renders once
//! in each window, anywhere in its tree: [`live_regions`]. Each region is an
//! accessibility node with `live` and `atomic` set (the kernel's
//! [`Aria::live`]) and the announcement as its value and label. AccessKit
//! announces a live node whenever its value changes, which on macOS posts
//! `AnnouncementRequested` to VoiceOver. gpui builds the accessibility tree
//! only while assistive technology is active, so announcing costs nothing
//! otherwise.
//!
//! Hooking it up: render `live_regions(window)` as a child of each window's
//! root element, then call [`announce`] from any handler with that window:
//!
//! ```text
//! div().size_full().child(content).child(live_regions(window))
//! announce("3 files added", window, cx);
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, EntityId, InteractiveElement, IntoElement, ParentElement, Role, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Task, Window, WindowId, canvas, div, px,
};
use ui_sdk_kernel::part::{PartElement, with_aria};
use ui_sdk_kernel::{Aria, Live, dev};

/// How urgently a screen reader reads an announcement: `Polite` waits for a
/// pause, `Assertive` interrupts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AnnouncePriority {
    #[default]
    Polite,
    Assertive,
}

impl AnnouncePriority {
    /// The original's name for it, its `aria-live`.
    pub fn as_str(self) -> &'static str {
        match self {
            AnnouncePriority::Polite => "polite",
            AnnouncePriority::Assertive => "assertive",
        }
    }
}

/// Announces text instead of the live regions, the original's `setAnnouncer`.
pub type Announcer = Rc<dyn Fn(&str, AnnouncePriority, &mut Window, &mut App)>;

#[derive(Default)]
struct LiveText {
    text: Option<SharedString>,
    // The text waiting to replace the cleared one. Dropping it cancels it.
    pending: Option<Task<()>>,
}

#[derive(Default)]
struct Regions {
    polite: LiveText,
    assertive: LiveText,
    rendered: bool,
    // The view the regions last drew in, which draws them again as their
    // text changes.
    view: Option<EntityId>,
}

impl Regions {
    fn get(&mut self, priority: AnnouncePriority) -> &mut LiveText {
        match priority {
            AnnouncePriority::Polite => &mut self.polite,
            AnnouncePriority::Assertive => &mut self.assertive,
        }
    }
}

thread_local! {
    static REGIONS: RefCell<HashMap<WindowId, Regions>> = RefCell::new(HashMap::new());
    static ANNOUNCER: RefCell<Option<Announcer>> = const { RefCell::new(None) };
}

fn with_regions<R>(window: WindowId, f: impl FnOnce(&mut Regions) -> R) -> R {
    REGIONS.with(|regions| f(regions.borrow_mut().entry(window).or_default()))
}

// Draws the regions again with their new text: only the view they're in, so
// announcing doesn't render the whole window again, which a key pressed
// meanwhile would wait for.
fn redraw_regions(window: &mut Window, cx: &mut App) {
    match with_regions(window.window_handle().window_id(), |regions| regions.view) {
        Some(view) => cx.notify(view),
        None => window.refresh(),
    }
}

/// Replaces how text is announced, like routing it through another channel.
/// `None` goes back to the live regions.
pub fn set_announcer(announcer: Option<Announcer>) {
    ANNOUNCER.with(|current| *current.borrow_mut() = announcer);
}

/// Creates both live regions ahead of time, so the first announcement isn't
/// missed by screen readers that only notice regions present before a change.
/// The regions render empty wherever the window renders [`live_regions`], so
/// this only sets up the window's state.
pub fn prepare_announcements(window: &Window) {
    if ANNOUNCER.with(|announcer| announcer.borrow().is_some()) {
        return;
    }
    with_regions(window.window_handle().window_id(), |_| ());
}

/// Has screen readers read `text` once they're idle. The same text twice is
/// read twice. It takes what `AnnouncePending` and other callers pass:
/// `|text, window, cx| ..`.
pub fn announce(text: &str, window: &mut Window, cx: &mut App) {
    announce_with(text, AnnouncePriority::Polite, window, cx);
}

/// [`announce`] with a priority: `Assertive` is read at once (the original's
/// `priority` option).
pub fn announce_with(text: &str, priority: AnnouncePriority, window: &mut Window, cx: &mut App) {
    if text.is_empty() {
        return;
    }
    if let Some(announcer) = ANNOUNCER.with(|announcer| announcer.borrow().clone()) {
        return announcer(text, priority, window, cx);
    }
    let handle = window.window_handle();
    let id = handle.window_id();
    if dev::DEV && !with_regions(id, |regions| regions.rendered) {
        dev::warn_once(
            format!("announce-{id:?}"),
            "announce: this window renders no `live_regions`, so screen readers don't hear its announcements",
            None,
        );
    }
    let text = SharedString::from(text);
    // Screen readers only notice text that changes after the region exists,
    // and the same text twice only after it was cleared. A newer text replaces
    // one still waiting at the same priority.
    let pending = cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        handle
            .update(cx, |_, window, cx| {
                with_regions(id, |regions| regions.get(priority).text = Some(text));
                redraw_regions(window, cx);
            })
            .ok();
    });
    with_regions(id, |regions| {
        let region = regions.get(priority);
        region.text = None;
        region.pending = Some(pending);
    });
    redraw_regions(window, cx);
}

/// The window's two live regions, rendered once per window, anywhere in its
/// tree. They take no space and paint nothing.
pub fn live_regions(window: &Window) -> impl IntoElement + use<> {
    let id = window.window_handle().window_id();
    let (polite, assertive) = with_regions(id, |regions| {
        regions.rendered = true;
        (regions.polite.text.clone(), regions.assertive.text.clone())
    });
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_0()
        .child(region(AnnouncePriority::Polite, polite))
        .child(region(AnnouncePriority::Assertive, assertive))
        .child(canvas(
            move |_, window, _| {
                let view = window.current_view();
                with_regions(id, |regions| regions.view = Some(view));
            },
            |_, _, _, _| {},
        ))
}

// One live region: `role="status"` for polite text and `role="alert"` for
// assertive, read out as its value changes. macOS reads the value, other
// platforms the name.
fn region(priority: AnnouncePriority, text: Option<SharedString>) -> PartElement<Stateful<gpui::Div>> {
    let (id, role, live) = match priority {
        AnnouncePriority::Polite => ("ui-sdk-announcer", Role::Status, Live::Polite),
        AnnouncePriority::Assertive => ("ui-sdk-announcer-assertive", Role::Alert, Live::Assertive),
    };
    let mut el = div().id(id).role(role).absolute().size(px(1.));
    if let Some(text) = text {
        el = el.aria_label(text.clone()).aria_value(text);
    }
    with_aria(el, Aria::new().live(live).atomic(true))
}
