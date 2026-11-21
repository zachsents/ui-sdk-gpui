//! Port of `foundation/text/caret.ts`.
//!
//! The original adapts an `<input>` or a `<textarea>`: it listens to the
//! field's DOM events, measures the caret with a hidden mirror of the field,
//! and edits through `execCommand` so undo reverts the edit. gpui has no
//! fields of its own, so a field is anything that implements [`TextField`],
//! like `ui-sdk-native`'s text field: it reports its events, lays its text out
//! and knows where its caret is, and edits as one undoable step. What the
//! original adds on top is here: the diff of each edit, composition, telling a
//! program's edits from a person's, and selection changes reported once.
//!
//! Dropped: the mirror measurement and Chromium's value ranges
//! (`measureInMirror`, `nativeRect`, `createCaretRects`' cache). A gpui field
//! has its last layout, so [`TextField::caret_rect`] reads it.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, FocusHandle, SharedString, Subscription, Window};

use crate::text_editor::{
    EditorChange, EditorContent, EditorListener, EditorReplaceOptions, TextBefore, TextEditorAdapter,
    TextRange, TextRect,
};

/// The kind of a text field, which decides what its arrow keys do
/// (`input.type`, or a `<textarea>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextFieldKind {
    #[default]
    Text,
    Search,
    Url,
    Tel,
    Email,
    Password,
    Number,
    /// Several lines, like a `<textarea>`.
    TextArea,
}

/// What happens in a text field, as the original hears it from the DOM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextFieldEvent {
    /// An edit is about to change the text (`beforeinput`).
    BeforeInput,
    /// The text changed (`input`). `composing` while an input method is
    /// mid-composition; `user` when a person typed, pasted or dictated it,
    /// never for edits [`TextField::replace_range`] or a program makes.
    Input { composing: bool, user: bool },
    /// An input method started composing.
    CompositionStart,
    /// An input method committed what it composed.
    CompositionEnd,
    /// The field lost focus.
    Blur,
    /// The caret or the selection may have moved: after a selection change, a
    /// key or a pointer release, or focus.
    Selection,
}

/// What a text field listens with.
pub type TextFieldListener = Rc<dyn Fn(&TextFieldEvent, &mut Window, &mut App)>;

/// A single or multi line text field, what an `<input>` or a `<textarea>` is
/// to the original. Offsets are byte offsets into its text.
pub trait TextField: 'static {
    /// Its focus handle.
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    /// Its text.
    fn value(&self, cx: &App) -> SharedString;
    /// The selection, or the caret as an empty range.
    fn selection_range(&self, cx: &App) -> TextRange;
    /// Selects a range, or puts the caret at an empty one.
    fn set_selection_range(&self, range: TextRange, window: &mut Window, cx: &mut App);
    /// Replaces a range with text as one edit its undo reverts, puts the caret
    /// after it, and reports it through [`TextField::subscribe`] as an
    /// `Input` that isn't the user's. Writing its whole value instead would
    /// empty its undo stack, earlier typing included.
    fn replace_range(&self, range: TextRange, text: &str, window: &mut Window, cx: &mut App);
    /// The rectangle of the caret at an offset, in window coordinates, from
    /// its last layout. `None` while it isn't laid out.
    fn caret_rect(&self, offset: usize, window: &Window, cx: &App) -> Option<TextRect>;
    /// Its kind.
    fn kind(&self, cx: &App) -> TextFieldKind;
    /// Whether it's read-only.
    fn read_only(&self, cx: &App) -> bool;
    /// Whether a popup it controls is open, so its arrow keys belong to the
    /// popup, like a suggestion menu's (`aria-expanded`, or `data-open` on a
    /// text area).
    fn expanded(&self, cx: &App) -> bool;
    /// Submits the form it's in, if any (`form.requestSubmit()`).
    fn submit(&self, window: &mut Window, cx: &mut App);
    /// Calls `listener` with what happens in it, until the subscription drops.
    fn subscribe(&self, listener: TextFieldListener, cx: &mut App) -> Subscription;
}

/// Where two texts differ: the range of the text before that was replaced,
/// and what replaced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextDiff {
    pub range: TextRange,
    pub inserted: String,
}

/// Where two texts differ: the range of `before` that was replaced, and what
/// replaced it, in byte offsets that fall between characters.
pub fn text_diff(before: &str, after: &str) -> TextDiff {
    let mut start = 0;
    for (a, b) in before.chars().zip(after.chars()) {
        if a != b {
            break;
        }
        start += a.len_utf8();
    }
    let mut end = 0;
    for (a, b) in before[start..].chars().rev().zip(after[start..].chars().rev()) {
        if a != b {
            break;
        }
        end += a.len_utf8();
    }
    TextDiff {
        range: TextRange::new(start, before.len() - end),
        inserted: after[start..after.len() - end].to_string(),
    }
}

// The offset, or the start of the character it falls inside, within the text.
pub(crate) fn char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

// Replaces a range as one edit the field's undo reverts, focusing the field
// first so the edit lands where the user is.
fn insert_text(field: &dyn TextField, range: TextRange, text: &str, window: &mut Window, cx: &mut App) {
    let handle = field.focus_handle(cx);
    if !handle.is_focused(window) {
        window.focus(&handle, cx);
    }
    field.replace_range(range, text, window, cx);
}

struct AdapterState {
    listeners: Vec<(usize, EditorListener)>,
    next_listener: usize,
    previous: SharedString,
    composing: bool,
    // Our own insertion reports an input too. It's a program's edit, never
    // the user's.
    programmatic: bool,
    last_selection: Option<TextRange>,
    attached: Option<Subscription>,
}

/// The editor adapter for a [`TextField`]. A trigger menu input builds it;
/// use it for a field you render yourself.
pub struct TextFieldAdapter<F: TextField> {
    field: Rc<F>,
    state: Rc<RefCell<AdapterState>>,
}

impl<F: TextField> Clone for TextFieldAdapter<F> {
    fn clone(&self) -> Self {
        Self {
            field: self.field.clone(),
            state: self.state.clone(),
        }
    }
}

/// The editor adapter for a text field (`textFieldAdapter`).
pub fn text_field_adapter<F: TextField>(field: F, cx: &App) -> TextFieldAdapter<F> {
    let previous = field.value(cx);
    TextFieldAdapter {
        field: Rc::new(field),
        state: Rc::new(RefCell::new(AdapterState {
            listeners: Vec::new(),
            next_listener: 0,
            previous,
            composing: false,
            programmatic: false,
            last_selection: None,
            attached: None,
        })),
    }
}

impl<F: TextField> TextFieldAdapter<F> {
    /// The field it adapts.
    pub fn field(&self) -> &F {
        &self.field
    }

    fn focused(&self, window: &Window, cx: &App) -> bool {
        self.field.focus_handle(cx).is_focused(window)
    }

    fn emit(state: &RefCell<AdapterState>, change: EditorChange, window: &mut Window, cx: &mut App) {
        let listeners: Vec<EditorListener> =
            state.borrow().listeners.iter().map(|(_, listener)| listener.clone()).collect();
        for listener in listeners {
            listener(&change, window, cx);
        }
    }

    fn note_selection(field: &F, state: &RefCell<AdapterState>, window: &mut Window, cx: &mut App) {
        let selection = Some(field.selection_range(cx));
        {
            let mut state = state.borrow_mut();
            if state.last_selection == selection {
                return;
            }
            state.last_selection = selection;
        }
        Self::emit(state, EditorChange::Selection, window, cx);
    }

    fn handle(field: &F, state: &RefCell<AdapterState>, event: &TextFieldEvent, window: &mut Window, cx: &mut App) {
        match *event {
            // During a composition the text from before it stays, so the
            // composition's end reports the whole commit.
            TextFieldEvent::BeforeInput => {
                let composing = state.borrow().composing;
                if !composing {
                    state.borrow_mut().previous = field.value(cx);
                }
            }
            TextFieldEvent::Input { composing, user } => {
                // An input outside a composition means any composition is
                // over, even when the end of it never came.
                if !composing {
                    state.borrow_mut().composing = false;
                }
                if state.borrow().composing {
                    return;
                }
                let value = field.value(cx);
                let (diff, programmatic) = {
                    let mut state = state.borrow_mut();
                    let diff = text_diff(&state.previous, &value);
                    state.previous = value;
                    (diff, state.programmatic)
                };
                Self::emit(
                    state,
                    EditorChange::Input {
                        range: diff.range,
                        inserted: diff.inserted,
                        user: !programmatic && user,
                    },
                    window,
                    cx,
                );
                Self::note_selection(field, state, window, cx);
            }
            TextFieldEvent::CompositionStart => {
                {
                    let mut state = state.borrow_mut();
                    state.composing = true;
                    state.previous = field.value(cx);
                }
                Self::emit(state, EditorChange::CompositionStart, window, cx);
            }
            TextFieldEvent::CompositionEnd => {
                let value = field.value(cx);
                let diff = {
                    let mut state = state.borrow_mut();
                    state.composing = false;
                    let diff = text_diff(&state.previous, &value);
                    state.previous = value;
                    diff
                };
                Self::emit(state, EditorChange::CompositionEnd, window, cx);
                if !diff.range.is_empty() || !diff.inserted.is_empty() {
                    Self::emit(
                        state,
                        EditorChange::Input {
                            range: diff.range,
                            inserted: diff.inserted,
                            user: true,
                        },
                        window,
                        cx,
                    );
                }
            }
            TextFieldEvent::Blur => {
                {
                    let mut state = state.borrow_mut();
                    state.composing = false;
                    state.last_selection = None;
                }
                Self::emit(state, EditorChange::Selection, window, cx);
            }
            TextFieldEvent::Selection => Self::note_selection(field, state, window, cx),
        }
    }
}

impl<F: TextField> TextEditorAdapter for TextFieldAdapter<F> {
    fn focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        Some(self.field.focus_handle(cx))
    }

    fn text_before(&self, length: usize, window: &Window, cx: &App) -> Option<TextBefore> {
        if !self.focused(window, cx) {
            return None;
        }
        let value = self.field.value(cx);
        let caret = char_boundary(&value, self.field.selection_range(cx).end);
        let offset = value[..caret]
            .char_indices()
            .rev()
            .take(length)
            .last()
            .map_or(caret, |(index, _)| index);
        Some(TextBefore {
            text: value[offset..caret].to_string(),
            offset,
        })
    }

    fn selection(&self, window: &Window, cx: &App) -> Option<TextRange> {
        self.focused(window, cx).then(|| self.field.selection_range(cx))
    }

    fn rect_at(&self, offset: usize, window: &Window, cx: &App) -> Option<TextRect> {
        self.field.caret_rect(offset, window, cx)
    }

    fn replace(
        &self,
        range: TextRange,
        content: EditorContent,
        _options: EditorReplaceOptions,
        window: &mut Window,
        cx: &mut App,
    ) {
        let text = content.text().clone();
        self.state.borrow_mut().programmatic = true;
        insert_text(self.field.as_ref(), range, &text, window, cx);
        let mut state = self.state.borrow_mut();
        state.programmatic = false;
        state.previous = self.field.value(cx);
    }

    fn subscribe(&self, listener: EditorListener, cx: &mut App) -> Subscription {
        let id = {
            let mut state = self.state.borrow_mut();
            let id = state.next_listener;
            state.next_listener += 1;
            state.listeners.push((id, listener));
            id
        };
        let attach = self.state.borrow().attached.is_none();
        if attach {
            let field = Rc::downgrade(&self.field);
            let state = Rc::downgrade(&self.state);
            let attached = self.field.subscribe(
                Rc::new(move |event, window, cx| {
                    if let (Some(field), Some(state)) = (field.upgrade(), state.upgrade()) {
                        Self::handle(&field, &state, event, window, cx);
                    }
                }),
                cx,
            );
            self.state.borrow_mut().attached = Some(attached);
        }
        let state = Rc::downgrade(&self.state);
        Subscription::new(move || {
            let Some(state) = state.upgrade() else {
                return;
            };
            let detached = {
                let mut state = state.borrow_mut();
                state.listeners.retain(|(listener, _)| *listener != id);
                if state.listeners.is_empty() {
                    state.attached.take()
                } else {
                    None
                }
            };
            drop(detached);
        })
    }

    fn composing(&self, _cx: &App) -> bool {
        self.state.borrow().composing
    }

    fn can_submit(&self) -> bool {
        true
    }

    fn submit(&self, window: &mut Window, cx: &mut App) {
        self.field.submit(window, cx);
    }
}
