//! Port of `components/listbox/listbox-root-keyboard.ts`: Listbox's keys and
//! presses, as the APG's listbox pattern lays them out. Arrows, Home and End
//! move focus, and with `activation: Automatic` the selection follows. Space
//! and Enter choose. With `multiple`, Shift extends a range from the anchor,
//! Ctrl or ⌘ moves or toggles without replacing, and Ctrl or ⌘ with A
//! selects everything. Typing finds an option by its label.

use std::cell::RefCell;
use std::fmt::Debug;

use gpui::{App, Context, FocusHandle, KeyDownEvent, Modifiers, SharedString, WeakFocusHandle, Window};
use ui_sdk_focus::{Step, step_for};
use ui_sdk_kernel::{ChangeOptions, ChangeSource, InputEvent, Key, Reason, press_of};
use ui_sdk_lists::listbox_engine::{HighlightOptions, ItemKey};
use ui_sdk_lists::listbox_keyboard::{PAGE, paged_elsewhere};
use ui_sdk_platform::is_rtl;
use ui_sdk_text::is_typeahead_key;

use crate::listbox_root::ListboxActivation;
use crate::listbox_root_api::Listbox;

// Ctrl on Windows and Linux, ⌘ on Apple platforms: both count, since neither
// means anything else in a list.
fn with_modifier(modifiers: &Modifiers) -> bool {
    modifiers.control || modifiers.platform
}

thread_local! {
    // Every listbox root in the page, so a key from a list nested inside
    // another is handled only by its own.
    static ROOTS: RefCell<Vec<WeakFocusHandle>> = const { RefCell::new(Vec::new()) };
}

/// Counts a listbox's root among the roots a key may come from.
pub(crate) fn register_root(root: &FocusHandle) {
    let weak = root.downgrade();
    ROOTS.with(|roots| {
        let mut roots = roots.borrow_mut();
        roots.retain(|other| other.upgrade().is_some());
        if !roots.contains(&weak) {
            roots.push(weak);
        }
    });
}

// Whether the focused element's nearest listbox is `root`: keys from a list
// nested inside bubble through, and only the owner handles them.
fn owns_key(root: &FocusHandle, window: &Window, cx: &App) -> bool {
    let Some(focused) = window.focused(cx) else {
        return true;
    };
    ROOTS.with(|roots| {
        !roots.borrow().iter().filter_map(WeakFocusHandle::upgrade).any(|other| {
            other != *root
                && (other == focused || other.contains(&focused, window))
                && root.contains(&other, window)
        })
    })
}

impl<T: ItemKey + Clone + PartialEq + Debug + 'static> Listbox<T> {
    // Moves focus by a step, and the selection with it as the activation
    // asks. A step with Shift extends the range instead, and never wraps.
    fn navigate(
        &mut self,
        event: &KeyDownEvent,
        step: Step,
        current: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        let extending = modifiers.shift && self.multiple;
        let listbox = self.listbox.clone();
        let next = if extending {
            listbox.step_key(self, step, Some(false))
        } else {
            listbox.step_key(self, step, None)
        };
        let Some(next) = next else {
            return;
        };
        window.prevent_default();
        let input = InputEvent::from(event);
        if Some(&next) != current.as_ref() {
            listbox.highlight(self, Some(next.clone()), HighlightOptions::event(Some(input.clone())), window, cx);
        }
        let why = ChangeOptions::event_of(Reason::ListNavigation, input);
        if extending {
            let additive = self.additive(&modifiers);
            self.extend(&next, current.as_ref(), additive, why, window, cx);
            return;
        }
        if self.activation != ListboxActivation::Automatic || with_modifier(&modifiers) {
            return;
        }
        if self.multiple {
            self.replace(&next, why, window, cx);
        } else {
            self.choose(&next, why, window, cx);
        }
    }

    // Whether a range adds to the selection: always when choosing waits for
    // Space, and with Ctrl or ⌘ when a press replaces it.
    fn additive(&self, modifiers: &Modifiers) -> bool {
        self.activation == ListboxActivation::Manual || with_modifier(modifiers)
    }

    // Space or Enter on the focused option.
    fn activate(&mut self, event: &KeyDownEvent, current: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = current else {
            return;
        };
        // Links follow themselves, so routers and modifier keys work, and the
        // press chooses: gpui clicks the focused link as Enter comes up, and
        // the list clicks one it only points at.
        if Key::of(&event.keystroke) == Key::Enter && self.listbox.element(&current).is_some_and(|element| element.link) {
            if self.virtual_focus
                && let Some(press) = press_of(&self.instance, "item", Some(&current))
            {
                window.prevent_default();
                press.click(InputEvent::from(event), window, cx);
            }
            return;
        }
        window.prevent_default();
        let modifiers = event.keystroke.modifiers;
        let why = ChangeOptions::event_of(Reason::ItemPress, InputEvent::from(event));
        if !self.multiple {
            self.choose(&current, why, window, cx);
            return;
        }
        if modifiers.shift {
            let anchor = self.key_selection.anchor();
            let additive = self.additive(&modifiers);
            self.extend(&current, anchor.as_ref(), additive, why, window, cx);
            return;
        }
        if self.activation == ListboxActivation::Manual || with_modifier(&modifiers) {
            self.toggle(&current, why, window, cx);
            return;
        }
        self.replace(&current, why, window, cx);
    }

    // Focuses the next option whose label starts with what was typed, and
    // selects it when the selection follows focus. Returns whether one
    // matched.
    fn typed(&mut self, event: &KeyDownEvent, current: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let listbox = self.listbox.clone();
        let Some(found) = listbox.search(self, &Key::of(&event.keystroke), current.as_deref()) else {
            return false;
        };
        window.prevent_default();
        let input = InputEvent::from(event);
        listbox.highlight(self, Some(found.key.clone()), HighlightOptions::event(Some(input.clone())), window, cx);
        if self.activation != ListboxActivation::Automatic {
            return true;
        }
        let why = ChangeOptions::event_of(Reason::ListNavigation, input);
        if self.multiple {
            self.replace(&found.key, why, window, cx);
        } else {
            self.choose(&found.key, why, window, cx);
        }
        true
    }

    /// Keys on the list, from the option that has focus or, with
    /// `virtual_focus`, from the list itself (`onKeyDown`).
    pub(crate) fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys from a list nested inside bubble through. Only the owner handles them.
        if !owns_key(&self.root_handle, window, cx) {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if self.is_disabled() || modifiers.alt {
            return;
        }
        let current = self.listbox.highlighted().or_else(|| self.stop.clone());
        let key = Key::of(&event.keystroke);
        if with_modifier(&modifiers)
            && !modifiers.shift
            && key.typed().is_some_and(|typed| typed.eq_ignore_ascii_case("a"))
        {
            if !self.multiple {
                return;
            }
            window.prevent_default();
            self.toggle_all(ChangeOptions::event_of(Reason::ListNavigation, InputEvent::from(event)), window, cx);
            return;
        }
        if let Some(step) = step_for(&key, self.orientation, is_rtl(cx)) {
            self.navigate(event, step, current, window, cx);
            return;
        }
        let page = match key {
            Key::PageDown => PAGE,
            Key::PageUp => -PAGE,
            _ => 0,
        };
        let root = self.instance.element_id("root", None);
        if page != 0 && !paged_elsewhere(&root) {
            self.navigate(event, Step::By(page), current, window, cx);
            return;
        }
        if key == Key::Space && self.listbox.typing() {
            // Space continues a search while it still matches.
            if self.typed(event, current.clone(), window, cx) {
                return;
            }
            self.listbox.reset_typing();
        }
        if key == Key::Space || key == Key::Enter {
            self.activate(event, current, window, cx);
            return;
        }
        if is_typeahead_key(&event.keystroke) && !with_modifier(&modifiers) {
            self.typed(event, current, window, cx);
        }
    }

    /// A click, a tap or a screen reader's activation on an option
    /// (`press`). Touch and screen readers have no modifier keys, so where a
    /// press would replace the selection, theirs toggles.
    pub(crate) fn press(&mut self, key: &SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_disabled() {
            return;
        }
        // A press that didn't focus the option, like a screen reader's, does.
        if !self.virtual_focus
            && let Some(handle) = self.listbox.existing_focus_handle(key)
            && !handle.is_focused(window)
            && self.listbox.is_connected(key)
        {
            window.focus(&handle, cx);
        }
        let why = ChangeOptions {
            reason: Some(Reason::ItemPress),
            event: event.clone(),
            source: None,
        };
        if !self.multiple {
            self.choose(key, why, window, cx);
            return;
        }
        let source = match &event {
            Some(event) => event.source(),
            None => ChangeSource::Touch,
        };
        let pointer = match &event {
            Some(InputEvent::Click(click)) => Some(click.modifiers()),
            _ => None,
        };
        let has_modifiers = pointer.is_some() && source != ChangeSource::Touch && source != ChangeSource::Virtual;
        let modifiers = pointer.filter(|_| has_modifiers).unwrap_or_default();
        // A range with no anchor, like after the selection was cleared,
        // starts at the option pressed.
        if has_modifiers && modifiers.shift {
            let additive = self.additive(&modifiers);
            self.extend(key, None, additive, why, window, cx);
            return;
        }
        if self.activation == ListboxActivation::Manual || !has_modifiers || with_modifier(&modifiers) {
            self.toggle(key, why, window, cx);
            return;
        }
        self.replace(key, why, window, cx);
    }
}
