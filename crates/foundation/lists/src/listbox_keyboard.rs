//! Port of `foundation/lists/listbox-keyboard.ts`: the keys every list with
//! a highlight shares: the step a key takes through it, and the keys on an
//! item that has real focus.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt::Debug;

use gpui::{Context, ElementId, KeyDownEvent, Keystroke, SharedString, Window};
use ui_sdk_focus::Step;
use ui_sdk_kernel::{InputEvent, Key};
use ui_sdk_platform::is_apple;
use ui_sdk_text::is_typeahead_key;

use crate::listbox_engine::ListboxEngine;

/// How far PageUp and PageDown move the highlight.
pub const PAGE: isize = 10;

thread_local! {
    // Lists whose PageUp and PageDown something else handles, by element id.
    static PAGED_ELSEWHERE: RefCell<HashSet<ElementId>> = RefCell::new(HashSet::new());
}

/// Whether something else handles a list's PageUp and PageDown, like a
/// scroll picker that pages by what fits in view (`pagedElsewhere.has`).
/// `list` is the list part's element id.
pub fn paged_elsewhere(list: &ElementId) -> bool {
    PAGED_ELSEWHERE.with(|lists| lists.borrow().contains(list))
}

/// Says whether something else handles a list's PageUp and PageDown
/// (`pagedElsewhere.add` and `delete`).
pub fn set_paged_elsewhere(list: &ElementId, paged: bool) {
    PAGED_ELSEWHERE.with(|lists| {
        let mut lists = lists.borrow_mut();
        if paged {
            lists.insert(list.clone());
        } else {
            lists.remove(list);
        }
    });
}

/// The step a key takes through a list: 1 or -1 for the arrows, and for
/// Ctrl+N and Ctrl+P on Apple platforms, as in their text fields; the first
/// or last item for ⌘ with an arrow; a page for PageUp and PageDown. `None`
/// for any other key. Alt with an arrow opens or closes a list, which each
/// component decides before asking for a step. Home and End are left out,
/// since a text field keeps them for its caret (`listStep`).
pub fn list_step(keystroke: &Keystroke, apple: bool) -> Option<Step> {
    let modifiers = keystroke.modifiers;
    match Key::of(keystroke) {
        Key::ArrowDown => return Some(if modifiers.platform { Step::Last } else { Step::By(1) }),
        Key::ArrowUp => return Some(if modifiers.platform { Step::First } else { Step::By(-1) }),
        Key::PageDown => return Some(Step::By(PAGE)),
        Key::PageUp => return Some(Step::By(-PAGE)),
        _ => {}
    }
    let emacs = apple && modifiers.control && !modifiers.platform && !modifiers.alt && !modifiers.shift;
    match keystroke.key.as_str() {
        "n" if emacs => Some(Step::By(1)),
        "p" if emacs => Some(Step::By(-1)),
        _ => None,
    }
}

/// Keys on an item that has real focus, after a screen reader's tap opened
/// the list (`listKeyDown`): arrows, Home and End move focus, Enter and Space
/// choose, and typing goes back to the input, which has focus by the time
/// the character lands. Escape closes through the list's layer. `choose`
/// and `focus_input` run inside the component's update, with it.
#[allow(clippy::too_many_arguments)]
pub fn list_key_down<C, T, X>(
    this: &mut C,
    event: &KeyDownEvent,
    listbox: &ListboxEngine<C, T, X>,
    highlighted: Option<SharedString>,
    choose: impl FnOnce(&mut C, SharedString, InputEvent, &mut Window, &mut Context<C>),
    focus_input: impl FnOnce(&mut C, &mut Window, &mut Context<C>),
    window: &mut Window,
    cx: &mut Context<C>,
) where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    let key = Key::of(&event.keystroke);
    let step = match key {
        Key::Home => Some(Step::First),
        Key::End => Some(Step::Last),
        _ => list_step(&event.keystroke, is_apple(cx)),
    };
    if let Some(step) = step {
        window.prevent_default();
        listbox.r#move(this, step, Some(InputEvent::from(event)), window, cx);
        return;
    }
    if key == Key::Enter || key == Key::Space {
        // Links follow themselves, so routers and modifier keys work, and the
        // press chooses: gpui clicks the focused link as Enter comes up.
        if key == Key::Enter
            && let Some(highlighted) = &highlighted
            && listbox.element(highlighted).is_some_and(|element| element.link)
        {
            return;
        }
        window.prevent_default();
        let Some(highlighted) = highlighted else {
            return;
        };
        choose(this, highlighted, InputEvent::from(event), window, cx);
        return;
    }
    if is_typeahead_key(&event.keystroke) {
        focus_input(this, window, cx);
    }
}
