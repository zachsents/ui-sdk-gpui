//! Port of `foundation/lists/inline-edit.ts`: editing a value where it's
//! shown: a cell in a table, a title on a card, a file's name in a list.
//! Enter or F2 starts editing and keeps the text, typing a character starts
//! over with it, like a spreadsheet; Enter or leaving saves, Escape puts the
//! text back, and focus goes back to where editing started either way.
//!
//! The input is a native text field ([`ui_sdk_native::TextField`]): the
//! component makes one for the id [`InlineEdit::editing`] returns, hands it
//! to [`InlineEdit::bind_input`], sends its key presses to
//! [`InlineEdit::input_keydown`], and calls [`InlineEdit::end`] with
//! `FocusOut` when focus leaves it.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, Context, Entity, FocusHandle, KeyDownEvent, SharedString, Window};
use ui_sdk_kernel::{InputEvent, Key, Reason, is_composing};
use ui_sdk_native::{SelectionDirection, TextField};

/// Why an edit started or ended (`InlineEditReason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InlineEditReason {
    EditKey,
    Type,
    DoubleClick,
    EnterKey,
    EscapeKey,
    FocusOut,
    TabKey,
    Imperative,
}

impl InlineEditReason {
    /// The shared reason it is.
    pub fn reason(self) -> Reason {
        match self {
            InlineEditReason::EditKey => Reason::EditKey,
            InlineEditReason::Type => Reason::Type,
            InlineEditReason::DoubleClick => Reason::DoubleClick,
            InlineEditReason::EnterKey => Reason::EnterKey,
            InlineEditReason::EscapeKey => Reason::EscapeKey,
            InlineEditReason::FocusOut => Reason::FocusOut,
            InlineEditReason::TabKey => Reason::TabKey,
            InlineEditReason::Imperative => Reason::Imperative,
        }
    }

    /// The original's name for it, like `"edit-key"`.
    pub fn as_str(self) -> &'static str {
        self.reason().as_str()
    }
}

/// What `commit` hears besides the id and the text.
#[derive(Clone, Debug)]
pub struct InlineEditDetails {
    pub reason: InlineEditReason,
    pub event: Option<InputEvent>,
}

/// How an edit starts: why, the event behind it, and the text the input
/// starts with instead of the value, as typing does.
#[derive(Clone, Debug)]
pub struct InlineEditStart {
    pub reason: InlineEditReason,
    pub event: Option<InputEvent>,
    pub text: Option<SharedString>,
}

impl Default for InlineEditStart {
    fn default() -> Self {
        Self {
            reason: InlineEditReason::Imperative,
            event: None,
            text: None,
        }
    }
}

/// Saves the text for an id and returns whether it went through. It runs
/// inside the component's update, with it.
pub type InlineEditCommitFn<C> =
    Rc<dyn Fn(&mut C, &SharedString, &SharedString, &InlineEditDetails, &mut Window, &mut Context<C>) -> bool>;

/// How [`create_inline_edit`] reads and saves values (`InlineEditOptions`).
/// The original's `key`, the state key that holds the id being edited, is
/// the edit's own here: [`InlineEdit::editing`].
pub struct InlineEditOptions<C: 'static> {
    /// The text an id shows now, put in the input when editing starts.
    pub value: Rc<dyn Fn(&C, &SharedString) -> SharedString>,
    /// Saves the text and returns whether it went through. Return false to
    /// keep editing, like when the text isn't valid.
    pub commit: InlineEditCommitFn<C>,
    /// Whether an id can't be edited.
    pub disabled: Option<Rc<dyn Fn(&C, &SharedString) -> bool>>,
}

impl<C: 'static> InlineEditOptions<C> {
    pub fn new(
        value: impl Fn(&C, &SharedString) -> SharedString + 'static,
        commit: impl Fn(&mut C, &SharedString, &SharedString, &InlineEditDetails, &mut Window, &mut Context<C>) -> bool
        + 'static,
    ) -> Self {
        Self {
            value: Rc::new(value),
            commit: Rc::new(commit),
            disabled: None,
        }
    }

    pub fn disabled(mut self, disabled: impl Fn(&C, &SharedString) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(disabled));
        self
    }
}

#[derive(Default)]
struct EditData {
    editing: Option<SharedString>,
    input: Option<Entity<TextField>>,
    // Where focus was when editing started, to go back to, and the text the
    // input starts with.
    origin: Option<FocusHandle>,
    text: SharedString,
    typed: bool,
    // The Rename button or menu item that started editing may be gone by the
    // end, so focus can fall back to the focusable element around the input.
    holder: Option<FocusHandle>,
}

struct EditInner<C: 'static> {
    options: InlineEditOptions<C>,
    data: RefCell<EditData>,
}

/// Lets users edit a value in place (`createInlineEdit`'s return). Cloning
/// it gives another handle to the same edit.
pub struct InlineEdit<C: 'static> {
    inner: Rc<EditInner<C>>,
}

impl<C: 'static> Clone for InlineEdit<C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users edit a value in place (`createInlineEdit`). Make it in the
/// component's constructor and keep it in a field. Tab saves too and
/// returns true, with focus back where editing started, so a grid can move
/// on to the next cell, as a spreadsheet does.
pub fn create_inline_edit<C: 'static>(options: InlineEditOptions<C>) -> InlineEdit<C> {
    InlineEdit {
        inner: Rc::new(EditInner {
            options,
            data: RefCell::new(EditData::default()),
        }),
    }
}

// Whether focus fell to the window: nothing holds it, or what holds it isn't
// in the page.
fn focus_lost(window: &Window, cx: &App) -> bool {
    window
        .focused(cx)
        .is_none_or(|focused| !focused.is_rendered(window))
}

impl<C: 'static> InlineEdit<C> {
    /// The id being edited, or `None`.
    pub fn editing(&self) -> Option<SharedString> {
        self.inner.data.borrow().editing.clone()
    }

    fn set(&self, id: Option<SharedString>, cx: &mut Context<C>) {
        let mut data = self.inner.data.borrow_mut();
        if data.editing != id {
            data.editing = id;
            cx.notify();
        }
    }

    fn finish(&self, refocus: bool, window: &mut Window, cx: &mut Context<C>) {
        self.set(None, cx);
        let targets = {
            let mut data = self.inner.data.borrow_mut();
            data.input = None;
            [data.origin.take(), data.holder.take()]
        };
        // Only when focus was lost with the input: code that moved it on,
        // like a grid going to the next cell after Tab, keeps its place. The
        // input leaves with the next frame, so look once it's drawn.
        if refocus {
            let targets = Rc::new(targets);
            let back = move |window: &mut Window, cx: &mut App| {
                let Some(back) = targets.iter().flatten().find(|target| target.is_rendered(window)) else {
                    return;
                };
                if focus_lost(window, cx) {
                    window.focus(back, cx);
                }
            };
            window.on_next_frame(move |window, _cx| {
                window.on_next_frame(back);
            });
        }
    }

    /// Starts editing an id (`start`). With `text`, the input starts with it
    /// instead of the value, as typing does. Returns whether it started.
    pub fn start(&self, this: &C, id: SharedString, start: InlineEditStart, window: &mut Window, cx: &mut Context<C>) -> bool {
        let options = &self.inner.options;
        if options.disabled.as_ref().is_some_and(|disabled| disabled(this, &id)) || self.editing().as_ref() == Some(&id) {
            return false;
        }
        {
            let mut data = self.inner.data.borrow_mut();
            data.origin = window.focused(cx);
            match start.text {
                Some(text) => {
                    data.text = text;
                    data.typed = true;
                }
                None => {
                    data.text = (options.value)(this, &id);
                    data.typed = false;
                }
            }
        }
        self.set(Some(id), cx);
        true
    }

    /// Handles a key pressed where the value shows and returns whether it
    /// started editing (`keydown`): Enter or F2 with the text, a printable
    /// character with that character. A key it takes calls
    /// `window.prevent_default()`.
    pub fn keydown(&self, this: &C, event: &KeyDownEvent, id: SharedString, window: &mut Window, cx: &mut Context<C>) -> bool {
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented()
            || self.editing().is_some()
            || modifiers.control
            || modifiers.platform
            || modifiers.alt
        {
            return false;
        }
        let key = Key::of(&event.keystroke);
        let reason_event = Some(InputEvent::from(event));
        if key == Key::Enter || key == Key::Function(2) {
            window.prevent_default();
            return self.start(
                this,
                id,
                InlineEditStart {
                    reason: InlineEditReason::EditKey,
                    event: reason_event,
                    text: None,
                },
                window,
                cx,
            );
        }
        if let Key::Character(typed) = &key
            && typed.encode_utf16().count() == 1
            && typed != " "
        {
            window.prevent_default();
            return self.start(
                this,
                id,
                InlineEditStart {
                    reason: InlineEditReason::Type,
                    event: reason_event,
                    text: Some(SharedString::from(typed.clone())),
                },
                window,
                cx,
            );
        }
        false
    }

    /// Starts editing on a double click, as most tables do (`dblclick`).
    pub fn dblclick(&self, this: &C, event: Option<InputEvent>, id: SharedString, window: &mut Window, cx: &mut Context<C>) -> bool {
        self.start(
            this,
            id,
            InlineEditStart {
                reason: InlineEditReason::DoubleClick,
                event,
                text: None,
            },
            window,
            cx,
        )
    }

    /// Fills and focuses the input (`bindInput`): call it once the input's
    /// text field is made. `holder` is the focusable element around it, like
    /// its row, which focus falls back to when where editing started is gone.
    /// Typing replaces, so the caret goes after the typed character; Enter
    /// keeps the text selected.
    pub fn bind_input(&self, input: &Entity<TextField>, holder: Option<FocusHandle>, window: &mut Window, cx: &mut App) {
        let (text, typed) = {
            let mut data = self.inner.data.borrow_mut();
            data.input = Some(input.clone());
            data.holder = holder;
            (data.text.clone(), data.typed)
        };
        input.update(cx, |field, cx| {
            field.set_value(text, window, cx);
            if typed {
                let len = field.len();
                field.set_selection_range(len, len, SelectionDirection::None, window, cx);
            } else {
                field.select(window, cx);
            }
            field.focus(window, cx);
        });
    }

    /// Handles a key in the input and returns whether it handled it
    /// (`inputKeydown`): Enter and Tab save, Escape puts the text back. A
    /// key it takes calls `window.prevent_default()`; Escape also stops,
    /// since a popup around the grid would close on the same press.
    pub fn input_keydown(&self, this: &mut C, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<C>) -> bool {
        if is_composing(window, cx) {
            return false;
        }
        let key = Key::of(&event.keystroke);
        let multi_line = self
            .inner
            .data
            .borrow()
            .input
            .as_ref()
            .is_some_and(|input| input.read(cx).is_multi_line());
        if key == Key::Enter && !(multi_line && event.keystroke.modifiers.shift) {
            window.prevent_default();
            return self.end(this, InlineEditReason::EnterKey, Some(InputEvent::from(event)), window, cx);
        }
        if key == Key::Escape {
            window.prevent_default();
            cx.stop_propagation();
            self.cancel(window, cx);
            return true;
        }
        if key == Key::Tab {
            window.prevent_default();
            return self.end(this, InlineEditReason::TabKey, Some(InputEvent::from(event)), window, cx);
        }
        false
    }

    /// Saves what the input holds and stops editing (`end`). Returns whether
    /// it saved. Unchanged text finishes without calling `commit`.
    pub fn end(
        &self,
        this: &mut C,
        reason: InlineEditReason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<C>,
    ) -> bool {
        let Some(id) = self.editing() else {
            return false;
        };
        let (text, input) = {
            let data = self.inner.data.borrow();
            let text = match &data.input {
                Some(input) => input.read(cx).value().clone(),
                None => data.text.clone(),
            };
            (text, data.input.clone())
        };
        let options = &self.inner.options;
        let saved = text == (options.value)(this, &id) || {
            let commit = options.commit.clone();
            commit(this, &id, &text, &InlineEditDetails { reason, event }, window, cx)
        };
        if !saved {
            if reason != InlineEditReason::FocusOut
                && let Some(input) = input
            {
                input.update(cx, |field, cx| field.focus(window, cx));
            }
            return false;
        }
        self.finish(reason != InlineEditReason::FocusOut, window, cx);
        true
    }

    /// Stops editing without saving (`cancel`).
    pub fn cancel(&self, window: &mut Window, cx: &mut Context<C>) {
        if self.editing().is_some() {
            self.finish(true, window, cx);
        }
    }
}
