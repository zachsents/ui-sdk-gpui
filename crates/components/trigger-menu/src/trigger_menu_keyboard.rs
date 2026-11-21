//! Port of `components/trigger-menu/trigger-menu-keyboard.ts`: the trigger
//! menu's keys, heard on the editor while a match is active. Focus never
//! leaves the editor: arrows move a virtual highlight, Enter and Tab choose,
//! and Escape dismisses. Keys an input method owns never reach here.

use gpui::{Context, KeyDownEvent, Window};
use ui_sdk_focus::Step;
use ui_sdk_kernel::roles::HandledKey;
use ui_sdk_kernel::{InputEvent, Key, Reason};
use ui_sdk_lists::list_step;
use ui_sdk_platform::is_apple;

use crate::trigger_menu_api::TriggerMenu;

/// The keys the input part handles. Home and End move the caret, which ends
/// the match.
pub const INPUT_KEYS: &[HandledKey] = &[
    "ArrowDown",
    "ArrowUp",
    "PageDown",
    "PageUp",
    "ctrl+n",
    "ctrl+p",
    "Enter",
    "Tab",
    "Escape",
    "Home",
    "End",
];

impl TriggerMenu {
    // The first arrow press is when screen readers hear about the highlight.
    fn move_highlight(&mut self, event: &KeyDownEvent, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        window.prevent_default();
        self.navigated = true;
        cx.notify();
        let listbox = self.listbox.clone();
        listbox.r#move(self, step, Some(InputEvent::from(event)), window, cx);
    }

    /// The menu's keys on the editor (`onKeyDown`), whether a match is
    /// active, open or not, like a free-text argument with no list.
    pub(crate) fn keyboard_keydown(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(level) = self.levels.current.as_ref() else {
            return;
        };
        let in_arguments = level.item.is_some();
        let open = self.popup.is_open();
        let highlighted = self.listbox.highlighted();
        let modifiers = event.keystroke.modifiers;
        let key = Key::of(&event.keystroke);
        // Alt and ⌘ with an arrow stay the editor's, which moves its caret
        // with them.
        let arrow = matches!(key, Key::ArrowDown | Key::ArrowUp);
        if let Some(step) = list_step(&event.keystroke, is_apple(cx)) {
            if open && !(arrow && (modifiers.alt || modifiers.platform)) {
                self.move_highlight(event, step, window, cx);
            }
            return;
        }
        let modified = modifiers.shift || modifiers.alt || modifiers.control || modifiers.platform;
        let input_event = Some(InputEvent::from(event));
        match key {
            Key::Enter => {
                if modified {
                    return;
                }
                // An open menu's Enter wins over the editor's, like a
                // prompt's submit.
                if !open && !in_arguments {
                    return;
                }
                window.prevent_default();
                if in_arguments {
                    return self.accept(input_event, false, window, cx);
                }
                if let Some(highlighted) = highlighted {
                    self.choose(&highlighted, input_event, false, window, cx);
                }
            }
            Key::Tab => {
                if modified {
                    return;
                }
                if in_arguments {
                    window.prevent_default();
                    return self.accept(input_event, true, window, cx);
                }
                if open && let Some(highlighted) = highlighted {
                    window.prevent_default();
                    self.choose(&highlighted, input_event, true, window, cx);
                }
            }
            Key::Escape => {
                // An open list's Escape is its layer's. A match with no
                // list, like a free-text argument, dismisses here.
                if open {
                    return;
                }
                window.prevent_default();
                self.end(Reason::EscapeKey, input_event, true, window, cx);
            }
            Key::Home | Key::End => self.end(Reason::CaretMove, input_event, false, window, cx),
            _ => {}
        }
    }
}
