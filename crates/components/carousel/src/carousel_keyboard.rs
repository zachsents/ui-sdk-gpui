//! Port of `components/carousel/carousel-keyboard.ts`: a carousel's keys on
//! its viewport: arrows page along the orientation, Home and End go to the
//! ends, and focus in a slide that leaves moves to the page that came in.

use gpui::{Context, KeyDownEvent, Window};
use ui_sdk_focus::{Step, step_for};
use ui_sdk_kernel::{InputEvent, Key, Reason};

use crate::carousel::Carousel;
use crate::carousel_layout::step_page;

/// The keys the viewport handles.
pub(crate) const VIEWPORT_KEYS: &[&str] = &["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"];

impl Carousel {
    /// Arrow keys, Home and End on the viewport page, unless something inside
    /// keeps them: a text field moves its caret, and a widget that moves
    /// between its items with them, like a slider or a listbox, prevents the
    /// keys it uses.
    pub(crate) fn on_viewport_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented() || modifiers.alt || modifiers.control || modifiers.platform || modifiers.shift {
            return;
        }
        if ui_sdk_native::is_typing(window) {
            return;
        }
        let rtl = self.follow_direction(window, cx);
        let step = step_for(&Key::of(&event.keystroke), self.orientation, rtl);
        let (Some(step), Some(count)) = (step, self.page_count) else {
            return;
        };
        window.prevent_default();
        let page = *self.page.get();
        let next = match step {
            Step::First => 1,
            Step::Last => count,
            Step::By(by) => step_page(page, if by > 0 { 1 } else { -1 }, count, self.r#loop),
        };
        let slide = self.focused_slide(window, cx);
        let by = match step {
            Step::By(by) => Some(if by > 0 { 1 } else { -1 }),
            Step::First | Step::Last => None,
        };
        if !self.go(next, Reason::ListNavigation, Some(InputEvent::from(event)), by, window, cx) {
            return;
        }
        let Some(slide) = slide else {
            return;
        };
        // Focus was in a slide that's leaving: it moves to the page that came in.
        let page = *self.page.get();
        let Some(first) = self.pages.get(page.wrapping_sub(1)).and_then(|slides| slides.first()).cloned() else {
            return;
        };
        if first == slide {
            return;
        }
        let Some(handle) = self.slide_focus(&first) else {
            return;
        };
        // It's still out of view until the viewport gets there. Holding focus
        // keeps it in the tree.
        self.focused = Some(first);
        cx.notify();
        window.focus(&handle, cx);
    }
}
