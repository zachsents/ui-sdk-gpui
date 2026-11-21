//! Port of `components/combobox/combobox-keyboard.ts`: the combobox's keys:
//! in the input, which keeps focus while arrows move a virtual highlight
//! through the list; in the list, when a screen reader moved real focus
//! there; and on chips, which move between them and remove them.

use gpui::{Context, Focusable, KeyDownEvent, SharedString, Window};
use ui_sdk_focus::Step;
use ui_sdk_kernel::{InputEvent, Key, Reason, is_composing, press_of};
use ui_sdk_lists::{list_key_down, list_step};
use ui_sdk_platform::{is_apple, is_rtl};
use ui_sdk_popups::{PopupOpener, ShowRequest};
use ui_sdk_text::js_trim;

use crate::combobox_api::{Combobox, ComboboxItemValue, Edge, item_labelled};

impl<T: ComboboxItemValue> Combobox<T> {
    /// Keys in the input (`onInputKeyDown`). Escape closes an open list
    /// through its layer; a closed one with chosen text has nothing to do.
    pub(crate) fn on_input_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(modality) = self.modality.as_mut()
            && modality.key(&event.keystroke)
        {
            cx.notify();
        }
        let open = self.popup.is_open();
        let key = Key::of(&event.keystroke);
        let alt = event.keystroke.modifiers.alt;
        // Alt+ArrowDown opens without moving the highlight, and Alt+ArrowUp
        // closes, as the APG asks.
        if alt && key == Key::ArrowDown && !open {
            window.prevent_default();
            self.show(
                ShowRequest::new()
                    .event(Some(InputEvent::from(event)))
                    .by(PopupOpener::Keyboard),
                window,
                cx,
            );
            return;
        }
        if alt && key == Key::ArrowUp {
            window.prevent_default();
            self.hide(Reason::EscapeKey, Some(InputEvent::from(event)), window, cx);
            return;
        }
        if let Some(step) = list_step(&event.keystroke, is_apple(cx)) {
            return self.move_highlight(event, step, open, window, cx);
        }
        match key {
            Key::Enter => {
                // Chips with no suggestions may keep the list closed, and
                // Enter still adds the text.
                let adds = self.multiple && self.options.allow_custom_value && !js_trim(self.query.get()).is_empty();
                if open || (adds && !is_composing(window, cx)) {
                    self.on_enter(event, window, cx);
                }
            }
            Key::Tab => {
                if open {
                    self.hide(Reason::FocusOut, Some(InputEvent::from(event)), window, cx);
                }
            }
            Key::Backspace => {
                // Backspace in an empty input removes the last chip.
                let field = self.field.read(cx);
                let empty = field.value().is_empty() && field.selection_start() == 0;
                if self.multiple
                    && empty
                    && let Some(last) = self.value.get().as_slice().last().cloned()
                {
                    window.prevent_default();
                    self.remove(&last, Some(InputEvent::from(event)), window, cx);
                }
            }
            Key::ArrowLeft | Key::ArrowRight => self.focus_last_chip(event, &key, window, cx),
            _ => {}
        }
    }

    // Moves the highlight through the open list. A step to the next or
    // previous item opens a closed one at its first or last item, once it
    // has rendered (`moveHighlight`).
    fn move_highlight(&mut self, event: &KeyDownEvent, step: Step, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open {
            window.prevent_default();
            let listbox = self.listbox.clone();
            listbox.r#move(self, step, Some(InputEvent::from(event)), window, cx);
            return;
        }
        let edge = match step {
            Step::By(1) => Edge::First,
            Step::By(-1) => Edge::Last,
            _ => return,
        };
        window.prevent_default();
        self.pending_highlight = Some(edge);
        self.show(
            ShowRequest::new()
                .event(Some(InputEvent::from(event)))
                .by(PopupOpener::Keyboard),
            window,
            cx,
        );
    }

    // Enter: the item highlighted in the open list, else an item whose label
    // is the text, else the create option, else the text itself with
    // `allow_custom_value` (`onEnter`).
    fn on_enter(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.popup.is_open();
        let highlighted = self.listbox.highlighted();
        let multiple = self.multiple;
        let text: SharedString = js_trim(self.query.get()).to_string().into();
        let input_event = InputEvent::from(event);
        window.prevent_default();
        // In a list kept closed, `auto_highlight` highlights an item nobody
        // saw.
        if open && let Some(highlighted) = highlighted {
            // Links follow themselves, so routers and modifier keys work.
            if self.listbox.element(&highlighted).is_some_and(|element| element.link)
                && let Some(press) = press_of(&self.instance, "item", Some(&highlighted))
            {
                press.click(input_event, window, cx);
                return;
            }
            return self.choose(highlighted, Some(input_event), window, cx);
        }
        let exact = if text.is_empty() {
            None
        } else {
            item_labelled(&text, &self.listbox.entries(self))
        };
        if let Some(exact) = exact {
            return self.choose(exact.key, Some(input_event), window, cx);
        }
        if ui_sdk_kernel::held(&self.instance, "create", None) {
            return self.create_item(Some(input_event), window, cx);
        }
        if self.options.allow_custom_value
            && self.commit_custom(Reason::InputChange, Some(input_event.clone()), window, cx)
            && !multiple
        {
            self.hide(Reason::ItemPress, Some(input_event), window, cx);
        }
    }

    /// Keys in the list, when a screen reader's tap moved real focus there
    /// (`onListKeyDown`).
    pub(crate) fn on_list_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_in_list || self.field.read(cx).is_focused(window) {
            return;
        }
        let listbox = self.listbox.clone();
        let highlighted = listbox.highlighted();
        list_key_down(
            self,
            event,
            &listbox,
            highlighted,
            |this, key, event, window, cx| this.choose(key, Some(event), window, cx),
            |this, window, cx| {
                this.focus_in_list = false;
                let handle = this.field.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            },
            window,
            cx,
        );
    }

    // The chips in the page, in order, leaving out those collapsed into an
    // overflow part, which can't be reached.
    pub(crate) fn reachable_chips(&self) -> Vec<SharedString> {
        self.chip_parts
            .keys()
            .into_iter()
            .filter(|key| !self.collapsed(key))
            .collect()
    }

    // The arrow toward the chips, with the caret at the input's start, moves
    // to the last chip (`focusLastChip`).
    fn focus_last_chip(&mut self, event: &KeyDownEvent, key: &Key, window: &mut Window, cx: &mut Context<Self>) {
        let back = (*key == Key::ArrowLeft) != is_rtl(cx);
        let field = self.field.read(cx);
        if !back || field.selection_start() != 0 || field.selection_end() != 0 {
            return;
        }
        let Some(last) = self.reachable_chips().pop() else {
            return;
        };
        let _ = event;
        window.prevent_default();
        let handle = self.chip_parts.focus_handle(&last, cx);
        window.focus(&handle, cx);
    }

    /// Arrows move between chips and back to the input. Backspace and Delete
    /// remove a chip and move to its neighbour, and Escape returns to the
    /// input (`onChipKeyDown`).
    pub(crate) fn on_chip_key_down(
        &mut self,
        event: &KeyDownEvent,
        chip: &SharedString,
        value: &T,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let list = self.reachable_chips();
        let index = list.iter().position(|key| key == chip);
        let input = self.field.read(cx).focus_handle(cx);
        match Key::of(&event.keystroke) {
            key @ (Key::ArrowLeft | Key::ArrowRight) => {
                window.prevent_default();
                let previous = (key == Key::ArrowLeft) != is_rtl(cx);
                let next = index.and_then(|index| {
                    if previous {
                        index.checked_sub(1).and_then(|index| list.get(index))
                    } else {
                        list.get(index + 1)
                    }
                });
                if let Some(next) = next {
                    let handle = self.chip_parts.focus_handle(next, cx);
                    window.focus(&handle, cx);
                } else if !previous {
                    window.focus(&input, cx);
                }
            }
            Key::Backspace | Key::Delete => {
                window.prevent_default();
                let neighbour = index.and_then(|index| {
                    index
                        .checked_sub(1)
                        .and_then(|index| list.get(index))
                        .or_else(|| list.get(index + 1))
                        .cloned()
                });
                self.remove(value, Some(InputEvent::from(event)), window, cx);
                match neighbour {
                    Some(neighbour) => {
                        let handle = self.chip_parts.focus_handle(&neighbour, cx);
                        window.focus(&handle, cx);
                    }
                    None => window.focus(&input, cx),
                }
            }
            Key::Escape => {
                window.prevent_default();
                window.focus(&input, cx);
            }
            _ => {}
        }
    }

    /// Like a select's button, ArrowDown and ArrowUp on the trigger open the
    /// list at its first or last item. Enter and Space press it
    /// (`onTriggerKeyDown`).
    pub(crate) fn on_trigger_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = Key::of(&event.keystroke);
        if self.popup.is_open() || (key != Key::ArrowDown && key != Key::ArrowUp) {
            return;
        }
        window.prevent_default();
        self.pending_highlight = Some(if key == Key::ArrowUp { Edge::Last } else { Edge::First });
        self.show(
            ShowRequest::new()
                .event(Some(InputEvent::from(event)))
                .by(PopupOpener::Keyboard),
            window,
            cx,
        );
    }
}
