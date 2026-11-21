//! Port of `components/carousel/carousel-indicators.ts`: a carousel's
//! indicators: a tablist with one tab stop, arrows that move between them
//! and show their page, and pages inferred from their order.

use gpui::{App, Context, KeyDownEvent, SharedString, Window};
use ui_sdk_focus::{
    Arrows, RovingFocus, RovingFocusMoveContext, RovingFocusOptions, Step, move_index, radio_step,
};
use ui_sdk_kernel::{InputEvent, Key, Reason};

use crate::carousel::Carousel;

/// The keys the indicator group handles.
pub(crate) const INDICATOR_KEYS: &[&str] = &["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"];

/// The indicators' roving tab stop. Tab lands on the selected indicator; it
/// doesn't remember the one focused last. Both axes move, since dots under
/// a vertical carousel usually sit in a row, and the arrows wrap.
pub(crate) fn indicator_roving(window: &Window, cx: &Context<Carousel>) -> RovingFocus<SharedString> {
    RovingFocus::new(
        RovingFocusOptions::default()
            .orientation(Arrows::Both)
            .remember(false)
            .r#move(|event: &KeyDownEvent, context: &RovingFocusMoveContext<SharedString>, _cx: &App| {
                let modifiers = event.keystroke.modifiers;
                if modifiers.control || modifiers.platform || modifiers.shift {
                    return None;
                }
                let key = Key::of(&event.keystroke);
                let step = match key {
                    Key::Home => Step::First,
                    Key::End => Step::Last,
                    _ => Step::By(radio_step(&key, context.rtl)?),
                };
                let from = context.items.iter().position(|item| item.key == context.from.key)? as isize;
                context
                    .items
                    .get(move_index(from, step, context.items.len(), true))
                    .map(|item| item.key.clone())
            }),
        window,
        cx,
    )
}

impl Carousel {
    /// The page an indicator stands for: its `page`, or its place among the
    /// indicators, or 0 before it's known.
    pub(crate) fn indicator_page(&self, key: &SharedString, page: Option<usize>) -> usize {
        if let Some(page) = page {
            return page;
        }
        self.indicators
            .iter()
            .position(|indicator| indicator == key)
            .map_or(0, |index| index + 1)
    }

    /// A press on an indicator shows its page, and focuses it, so arrow keys
    /// go on from there.
    pub(crate) fn press_indicator(
        &mut self,
        key: &SharedString,
        page: Option<usize>,
        event: InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = self.indicator_page(key, page);
        let handle = self.roving.focus_handle(key, cx);
        if !handle.is_focused(window) {
            window.focus(&handle, cx);
        }
        if page > 0 {
            self.go(page, Reason::ItemPress, Some(event), None, window, cx);
        }
    }

    /// Arrow keys, Home and End on an indicator move to another one and show
    /// its page.
    pub(crate) fn on_indicator_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.roving.focused(window) else {
            return;
        };
        let Some(moved) = self.roving.keydown(event, &from, window, cx) else {
            return;
        };
        if moved.key == moved.from {
            return;
        }
        let page = self.indicator_page(&moved.key, self.indicator_pages.borrow().get(&moved.key).copied().flatten());
        if page > 0 {
            self.go(page, Reason::ListNavigation, Some(InputEvent::from(event)), None, window, cx);
        }
    }
}
