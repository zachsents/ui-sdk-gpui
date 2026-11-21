//! What a text field reports: the `beforeinput` and `input` events, with
//! the kind of edit the browser names in `inputType`, and the rest of what
//! an `<input>` dispatches.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{Context, SharedString, Window};
use ui_sdk_kernel::ChangeSource;

use crate::text_field::TextField;

/// The kind of an edit, `InputEvent.inputType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputType {
    /// Typing, or a program's insertion.
    InsertText,
    /// Text replacing a range the platform chose, like an accent from a
    /// press and hold, or a whole value set by assistive technology.
    InsertReplacementText,
    /// Enter. A single-line field reports it before it commits and inserts
    /// nothing.
    InsertLineBreak,
    /// An input method's text while it composes.
    InsertCompositionText,
    /// An input method committing what it composed.
    InsertFromComposition,
    InsertFromPaste,
    /// Control-Y on a Mac, which puts back what Control-K cut.
    InsertFromYank,
    /// Control-T on a Mac, which swaps the characters around the caret.
    InsertTranspose,
    /// A program's deletion, with no direction.
    DeleteContent,
    DeleteContentBackward,
    DeleteContentForward,
    DeleteWordBackward,
    DeleteWordForward,
    /// To the start of the visual line.
    DeleteSoftLineBackward,
    /// To the end of the visual line.
    DeleteSoftLineForward,
    /// To the start of the paragraph.
    DeleteHardLineBackward,
    /// To the end of the paragraph, Control-K on a Mac.
    DeleteHardLineForward,
    DeleteByCut,
    HistoryUndo,
    HistoryRedo,
}

impl InputType {
    /// The browser's name for it, like `"insertText"`.
    pub fn as_str(self) -> &'static str {
        use InputType::*;
        match self {
            InsertText => "insertText",
            InsertReplacementText => "insertReplacementText",
            InsertLineBreak => "insertLineBreak",
            InsertCompositionText => "insertCompositionText",
            InsertFromComposition => "insertFromComposition",
            InsertFromPaste => "insertFromPaste",
            InsertFromYank => "insertFromYank",
            InsertTranspose => "insertTranspose",
            DeleteContent => "deleteContent",
            DeleteContentBackward => "deleteContentBackward",
            DeleteContentForward => "deleteContentForward",
            DeleteWordBackward => "deleteWordBackward",
            DeleteWordForward => "deleteWordForward",
            DeleteSoftLineBackward => "deleteSoftLineBackward",
            DeleteSoftLineForward => "deleteSoftLineForward",
            DeleteHardLineBackward => "deleteHardLineBackward",
            DeleteHardLineForward => "deleteHardLineForward",
            DeleteByCut => "deleteByCut",
            HistoryUndo => "historyUndo",
            HistoryRedo => "historyRedo",
        }
    }

    /// Whether it puts text in, `inputType.startsWith("insert")`.
    pub fn is_insert(self) -> bool {
        self.as_str().starts_with("insert")
    }

    /// Whether it takes text out, `inputType.startsWith("delete")`.
    pub fn is_delete(self) -> bool {
        self.as_str().starts_with("delete")
    }

    /// Whether it deletes toward the start, `inputType.includes("Backward")`.
    pub fn is_backward(self) -> bool {
        self.as_str().contains("Backward")
    }
}

/// Which end of a selection moves, `selectionDirection`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SelectionDirection {
    Forward,
    Backward,
    #[default]
    None,
}

impl SelectionDirection {
    /// The browser's name for it, like `"forward"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Backward => "backward",
            Self::None => "none",
        }
    }
}

/// An edit about to apply, the `beforeinput` event.
#[derive(Debug)]
pub struct BeforeEdit {
    pub input_type: InputType,
    /// The range of the value it replaces, `getTargetRanges()`.
    pub range: Range<usize>,
    /// The text it puts there, `data`: what's typed, pasted or composed,
    /// with what the field never takes gone (line breaks in a single-line
    /// field, control characters, and what a number field or the filter
    /// refuses), but before the maximum length cuts it, as in the browser.
    /// Empty for deletions.
    pub data: String,
    /// What of `data` the field puts there once the maximum length cuts it:
    /// all of it while there's room. With room for none of it, typing or
    /// pasting changes nothing, and an input method's commit takes its text
    /// away.
    pub inserted: String,
    /// Whether an input method is composing, `isComposing`.
    pub composing: bool,
    /// Whether `prevent_default` stops it. An input method's text while it
    /// composes can't be stopped.
    pub cancelable: bool,
    pub source: ChangeSource,
    // There's no room for any of `data`, so the edit changes nothing.
    no_room: bool,
    prevented: Cell<bool>,
}

impl BeforeEdit {
    // An edit that puts in all of its text.
    pub(crate) fn new(
        input_type: InputType,
        range: Range<usize>,
        data: String,
        composing: bool,
        cancelable: bool,
        source: ChangeSource,
    ) -> Self {
        Self {
            input_type,
            range,
            inserted: data.clone(),
            data,
            composing,
            cancelable,
            source,
            no_room: false,
            prevented: Cell::new(false),
        }
    }

    // What the maximum length leaves of `data`. A commit's text goes even
    // when none of it is left, as the browser clears what was composed.
    pub(crate) fn cut(mut self, inserted: String) -> Self {
        self.no_room = !self.data.is_empty()
            && inserted.is_empty()
            && self.input_type != InputType::InsertFromComposition;
        self.inserted = inserted;
        self
    }

    // Whether the edit changes nothing, with no room for any of its text.
    pub(crate) fn has_no_room(&self) -> bool {
        self.no_room
    }

    /// Refuses the edit, when it's cancelable.
    pub fn prevent_default(&self) {
        self.prevented.set(true);
    }

    pub fn default_prevented(&self) -> bool {
        self.prevented.get()
    }

    /// The value the edit would leave, from the value now: `inserted` in
    /// place of `range`.
    pub fn next_value(&self, value: &str) -> String {
        if self.no_room {
            return value.to_string();
        }
        let mut next = String::with_capacity(value.len() + self.inserted.len());
        next.push_str(&value[..self.range.start]);
        next.push_str(&self.inserted);
        next.push_str(&value[self.range.end..]);
        next
    }
}

/// An edit that applied, the `input` event.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub input_type: InputType,
    /// The range of the value before the edit that it replaced.
    pub range: Range<usize>,
    /// The text that replaced it, `data`.
    pub inserted: String,
    /// The value after it.
    pub value: SharedString,
    /// Whether an input method is still composing, `isComposing`.
    pub composing: bool,
    /// Whether a person made it: typing, pasting, an input method,
    /// assistive technology. A program's `replace_range` is false.
    pub user: bool,
    pub source: ChangeSource,
}

/// What a text field tells its subscribers, after the update that caused it.
#[derive(Clone, Debug, PartialEq)]
pub enum TextFieldEvent {
    /// The value changed through an edit, `input`.
    Input(Edit),
    /// The value was written from code, which fires no `input`: `set_value`,
    /// `reset`, or a default value it follows.
    Write(SharedString),
    /// What the user typed was committed: on blur, or Enter in a single-line
    /// field, `change`.
    Change(SharedString),
    /// The selection or the caret moved, `selectionchange`.
    SelectionChange {
        range: Range<usize>,
        direction: SelectionDirection,
    },
    Focus,
    Blur,
    /// An input method started composing, `compositionstart`.
    CompositionStart,
    /// It committed this text, empty when it gave up, `compositionend`.
    CompositionEnd(String),
}

/// A listener given the field itself, the way a DOM listener reaches its
/// element through `event.target`. It runs inside the field's update: it may
/// change the field through the reference it gets, and update any other
/// entity but one already being updated, like a component whose own call
/// made the edit.
pub type FieldHook<E> = Rc<dyn Fn(&mut TextField, &E, &mut Window, &mut Context<TextField>)>;

/// A listener for something that carries no data, like focus.
pub type FieldNotice = Rc<dyn Fn(&mut TextField, &mut Window, &mut Context<TextField>)>;

/// Which characters typing, pasting and input methods may put in.
pub type TextFilter = Rc<dyn Fn(char) -> bool>;
