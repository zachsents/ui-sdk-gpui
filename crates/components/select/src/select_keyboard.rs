//! Port of `components/select/select-keyboard.ts`: select's keyboard and
//! pointer input: keys on the closed trigger, including typeahead that
//! selects like a native select, keys in the open list and in its search
//! field, and a press on the trigger that drags or holds to an item and
//! releases to choose it.

use std::fmt::Debug;
use std::time::{Duration, Instant};

use gpui::{Context, KeyDownEvent, Keystroke, MouseDownEvent, MouseUpEvent, SharedString, Window};
use ui_sdk_focus::Step;
use ui_sdk_kernel::{ChangeOptions, InputEvent, Key, Reason, is_composing, press_of};
use ui_sdk_lists::listbox_engine::{HighlightOptions, ItemKey};
use ui_sdk_lists::listbox_keyboard::list_step;
use ui_sdk_lists::listbox_value::ListValue;
use ui_sdk_platform::is_apple;
use ui_sdk_popups::{HideRequest, PopupOpener, ReturnMode};
use ui_sdk_text::is_typeahead_key;

use crate::select_api::{InitialHighlight, Press, Select, Show, TRIGGER};

/// How long a press before releasing over an item chooses it.
pub const HOLD_MS: Duration = Duration::from_millis(200);

/// How far a drag, in pixels, before releasing over an item chooses it.
pub const DRAG_PX: f32 = 4.0;

impl<T: ItemKey + Clone + PartialEq + Debug + 'static> Select<T> {
    // Typing on the closed trigger selects the next item that starts with the
    // text, like a native select. Space continues a search that still
    // matches. Returns whether the key was used.
    fn trigger_typeahead(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.multiple || self.read_only || self.is_disabled() {
            return false;
        }
        let key = Key::of(&event.keystroke);
        let listbox = self.listbox.clone();
        let from = self.selection.keys(self).first().cloned();
        let Some(found) = listbox.search(self, &key, from.as_deref()) else {
            return key != Key::Space;
        };
        let entry = listbox.find(self, Some(&found.key)).unwrap_or(found);
        let Some(value) = entry.value.clone() else {
            return true;
        };
        listbox.remember(entry.key.clone(), entry.label.clone(), value.clone(), None);
        let why = ChangeOptions::event_of(Reason::ListNavigation, InputEvent::from(event));
        self.change_value(ListValue::Single(Some(value)), why, window, cx);
        true
    }

    /// Keys on the trigger (`onTriggerKeyDown`). On the closed trigger,
    /// arrows, Home and End open the list on an item, and Enter and Space
    /// open it through the trigger's press, which gpui clicks as they come
    /// up.
    pub(crate) fn on_trigger_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = Key::of(&event.keystroke);
        if self.popup.is_open() {
            // With `virtual_focus`, focus stays here and the open list takes the keys.
            if self.virtual_focus && !self.searchable() && !self.focus_in_list {
                if let Some(modality) = self.modality.as_mut()
                    && modality.key(&event.keystroke)
                {
                    cx.notify();
                }
                self.on_list_key_down(event, true, window, cx);
            }
            return;
        }
        if key == Key::Space && self.listbox.typing() {
            window.prevent_default();
            if self.trigger_typeahead(event, window, cx) {
                return;
            }
        }
        if let Some(focus) = initial_highlight_for(&key) {
            window.prevent_default();
            let show = Show {
                event: Some(InputEvent::from(event)),
                by: Some(PopupOpener::Keyboard),
                reason: None,
                focus: Some(focus),
            };
            self.show(show, window, cx);
            return;
        }
        if is_typeahead_key(&event.keystroke) && key != Key::Space {
            self.prepare(cx);
            if self.trigger_typeahead(event, window, cx) {
                window.prevent_default();
            }
        }
    }

    /// Whether a key on the trigger is one gpui mustn't click it for: Space
    /// that continues a search on the closed trigger, and Enter and Space
    /// the open list takes with `virtual_focus`. The original prevents the
    /// native button's click; gpui decides it as the key goes down, so the
    /// trigger asks before.
    pub(crate) fn takes_activation_key(&self, keystroke: &Keystroke) -> bool {
        let key = Key::of(keystroke);
        if key != Key::Enter && key != Key::Space {
            return false;
        }
        if self.popup.is_open() {
            return self.virtual_focus && !self.searchable() && !self.focus_in_list;
        }
        key == Key::Space && self.listbox.typing()
    }

    /// Keys in the open list (`onContentKeyDown`). The search field takes
    /// its own.
    pub(crate) fn on_content_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(modality) = self.modality.as_mut()
            && modality.key(&event.keystroke)
        {
            cx.notify();
        }
        if self.search.read(cx).is_focused(window) {
            self.on_search_key_down(event, window, cx);
            return;
        }
        self.on_list_key_down(event, false, window, cx);
    }

    // Keys for the open list, from an item, or from the trigger with
    // `virtual_focus`. Tab from the trigger closes the list and lets focus
    // move on. Tab and Escape from the list are the popup's own.
    fn on_list_key_down(&mut self, event: &KeyDownEvent, trigger: bool, window: &mut Window, cx: &mut Context<Self>) {
        let highlighted = self.listbox.highlighted();
        let modifiers = event.keystroke.modifiers;
        let key = Key::of(&event.keystroke);
        if modifiers.alt && key == Key::ArrowDown {
            return;
        }
        if modifiers.alt && key == Key::ArrowUp {
            self.choose_and_close(event, window, cx);
            return;
        }
        if let Some(step) = select_step(&event.keystroke, false, is_apple(cx)) {
            self.move_highlight(event, step, window, cx);
            return;
        }
        match key {
            Key::Tab => {
                if trigger {
                    let request = HideRequest::new()
                        .reason(Reason::FocusOut)
                        .event(Some(InputEvent::from(event)))
                        .focus(ReturnMode::Leave);
                    self.hide(request, window, cx);
                }
                return;
            }
            Key::Escape => return,
            Key::Enter | Key::Space => {
                self.activate(event, highlighted, window, cx);
                return;
            }
            _ => {}
        }
        if is_typeahead_key(&event.keystroke) {
            self.highlight_typed(event, highlighted, window, cx);
        }
    }

    // Keys in the search field, which keeps focus while arrows move the
    // highlight. Typing filters instead of searching by first letters, and
    // Home, End and Space stay the field's, as the APG's combobox asks.
    fn on_search_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if is_composing(window, cx) {
            return;
        }
        let highlighted = self.listbox.highlighted();
        let modifiers = event.keystroke.modifiers;
        let key = Key::of(&event.keystroke);
        if modifiers.alt && key == Key::ArrowUp {
            self.choose_and_close(event, window, cx);
            return;
        }
        if let Some(step) = select_step(&event.keystroke, true, is_apple(cx)) {
            self.move_highlight(event, step, window, cx);
            return;
        }
        if key == Key::Enter {
            window.prevent_default();
            let Some(highlighted) = highlighted else {
                return;
            };
            // Links follow themselves, so routers and modifier keys work.
            if self.click_link(&highlighted, event, window, cx) {
                return;
            }
            self.choose(&highlighted, Some(InputEvent::from(event)), window, cx);
        }
    }

    // Clicks a link item the select only points at, as `el.click()`: the
    // app's press follows it and the item's press chooses it. Returns
    // whether it did.
    fn click_link(&self, key: &SharedString, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.listbox.element(key).is_some_and(|element| element.link) {
            return false;
        }
        let Some(press) = press_of(&self.instance, "item", Some(key)) else {
            return false;
        };
        press.click(InputEvent::from(event), window, cx);
        true
    }

    fn move_highlight(&mut self, event: &KeyDownEvent, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        window.prevent_default();
        let listbox = self.listbox.clone();
        listbox.r#move(self, step, Some(InputEvent::from(event)), window, cx);
    }

    // Alt+ArrowUp chooses the highlighted item and closes, as the APG asks.
    // With `multiple` it only closes.
    fn choose_and_close(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.prevent_default();
        let input = InputEvent::from(event);
        match self.listbox.highlighted() {
            Some(highlighted) if !self.multiple => self.choose(&highlighted, Some(input), window, cx),
            _ => {
                self.hide(HideRequest::new().reason(Reason::ItemPress).event(Some(input)), window, cx);
            }
        }
    }

    // Enter or Space on the highlighted item chooses it. Space continues a
    // search while it still matches.
    fn activate(&mut self, event: &KeyDownEvent, highlighted: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let key = Key::of(&event.keystroke);
        if key == Key::Space && self.listbox.typing() {
            if self.highlight_typed(event, highlighted.clone(), window, cx) {
                return;
            }
            self.listbox.reset_typing();
        }
        // Links follow themselves, so routers and modifier keys work, and
        // their press chooses: gpui clicks the focused link as Enter comes
        // up, and the select clicks one it only points at.
        let focused = highlighted.as_ref().is_some_and(|highlighted| {
            self.listbox
                .existing_focus_handle(highlighted)
                .is_some_and(|handle| handle.is_focused(window))
        });
        if key == Key::Enter
            && focused
            && let Some(highlighted) = &highlighted
            && self.listbox.element(highlighted).is_some_and(|element| element.link)
        {
            return;
        }
        window.prevent_default();
        let Some(highlighted) = highlighted else {
            return;
        };
        if !focused && self.click_link(&highlighted, event, window, cx) {
            return;
        }
        self.choose(&highlighted, Some(InputEvent::from(event)), window, cx);
    }

    // Highlights the next item that starts with what the user typed.
    // Returns whether one did.
    fn highlight_typed(
        &mut self,
        event: &KeyDownEvent,
        highlighted: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let listbox = self.listbox.clone();
        let Some(found) = listbox.search(self, &Key::of(&event.keystroke), highlighted.as_deref()) else {
            return false;
        };
        window.prevent_default();
        listbox.highlight(self, Some(found.key), HighlightOptions::event(Some(InputEvent::from(event))), window, cx);
        true
    }

    /// A mouse press on the trigger (`pointerPress`): it opens, like a
    /// native select. Releasing over an item after dragging or holding
    /// chooses it. A quick click never chooses, even when the list covers the
    /// trigger.
    pub(crate) fn pointer_press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.press_click = true;
        let opening = !self.popup.is_open();
        let input = InputEvent::from(event);
        if opening {
            let show = Show {
                event: Some(input),
                by: Some(PopupOpener::Pointer),
                reason: None,
                focus: None,
            };
            self.show(show, window, cx);
        } else {
            self.hide(HideRequest::new().reason(Reason::TriggerPress).event(Some(input)), window, cx);
        }
        self.press = Some(Press {
            position: event.position,
            time: Instant::now(),
            opening,
        });
        cx.notify();
    }

    /// The press that opened or closed the list ended, anywhere in the
    /// window.
    pub(crate) fn pointer_release(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else {
            return;
        };
        let on_trigger = self
            .popup
            .trigger_region(&TRIGGER.into())
            .is_some_and(|region| region.contains_point(event.position, window));
        // A release off the trigger sends it no click, so the next click,
        // like Enter, is a new press.
        if !on_trigger {
            self.press_click = false;
        }
        if !press.opening {
            return;
        }
        let held = press.time.elapsed() >= HOLD_MS;
        let dx = f32::from(event.position.x - press.position.x);
        let dy = f32::from(event.position.y - press.position.y);
        let moved = dx.hypot(dy) > DRAG_PX;
        if (!held && !moved) || on_trigger {
            return;
        }
        let listbox = self.listbox.clone();
        let option = listbox
            .entries(self)
            .iter()
            .find(|entry| {
                listbox.is_connected(&entry.key)
                    && listbox
                        .bounds(&entry.key)
                        .is_some_and(|bounds| bounds.contains(&event.position))
            })
            .map(|entry| entry.key.clone());
        if let Some(key) = option {
            self.choose(&key, Some(InputEvent::from(event)), window, cx);
        }
    }

    /// Whether the click that just happened follows a press that already
    /// opened or closed the list (`consumePressClick`).
    pub(crate) fn consume_press_click(&mut self) -> bool {
        std::mem::replace(&mut self.press_click, false)
    }
}

/// The step a key takes through the open list (`selectStep`). PageUp and
/// PageDown go to the ends, like Home and End in the list, while the search
/// field keeps Home and End for its caret.
pub fn select_step(keystroke: &Keystroke, field: bool, apple: bool) -> Option<Step> {
    match Key::of(keystroke) {
        Key::PageUp => return Some(Step::First),
        Key::Home if !field => return Some(Step::First),
        Key::PageDown => return Some(Step::Last),
        Key::End if !field => return Some(Step::Last),
        _ => {}
    }
    list_step(keystroke, apple)
}

/// The item an arrow, Home or End key on the closed trigger highlights once
/// the list opens (`initialHighlightFor`).
pub fn initial_highlight_for(key: &Key) -> Option<InitialHighlight> {
    match key {
        Key::ArrowDown | Key::ArrowUp => Some(InitialHighlight::Selected),
        Key::Home => Some(InitialHighlight::First),
        Key::End => Some(InitialHighlight::Last),
        _ => None,
    }
}
