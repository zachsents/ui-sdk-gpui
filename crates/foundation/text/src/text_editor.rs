//! Port of `foundation/text/text-editor.ts`.
//!
//! What a suggestion menu needs from the field it suggests into: a trigger
//! menu reads and writes text through this, so one engine works for a text
//! field, a text area, a token input and any editor an app wraps. Types only:
//! every adapter brings its own code.
//!
//! Offsets are byte offsets into the editor's text as [`TextEditorAdapter::text_before`]
//! reads it, where a token reads as U+FFFC, so one token is one character.

use std::any::Any;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use gpui::{App, Bounds, FocusHandle, Pixels, SharedString, Subscription, Window};

/// Whether a token is waiting on something.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenStatus {
    /// It's loading.
    Pending,
    /// It needs the user, like an ambiguous mention.
    Unresolved,
}

/// An atomic token in a token input: a mention, a file, a command.
#[derive(Clone)]
pub struct Token {
    /// Kind, like `"mention"`, `"file"` or `"skill"` (`type` in the original).
    pub kind: SharedString,
    /// The id it stands for.
    pub value: SharedString,
    /// The text it shows, reads as and falls back to.
    pub label: SharedString,
    /// Stable identity, generated when missing.
    pub id: Option<SharedString>,
    /// Pending while it loads, unresolved when it needs the user.
    pub status: Option<TokenStatus>,
    /// What the chip renders from, like an icon or a run state. Never serialized.
    pub data: Option<Rc<dyn Any>>,
}

impl Token {
    pub fn new(kind: impl Into<SharedString>, value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            kind: kind.into(),
            value: value.into(),
            label: label.into(),
            id: None,
            status: None,
            data: None,
        }
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn status(mut self, status: TokenStatus) -> Self {
        self.status = Some(status);
        self
    }

    pub fn data(mut self, data: impl Any) -> Self {
        self.data = Some(Rc::new(data));
        self
    }

    /// Applies what a pending token resolved to.
    pub fn update(&mut self, update: TokenUpdate) {
        if let Some(kind) = update.kind {
            self.kind = kind;
        }
        if let Some(value) = update.value {
            self.value = value;
        }
        if let Some(label) = update.label {
            self.label = label;
        }
        if let Some(id) = update.id {
            self.id = Some(id);
        }
        if let Some(status) = update.status {
            self.status = status;
        }
        if let Some(data) = update.data {
            self.data = data;
        }
    }

    /// The trigger it was typed with, when its data is a [`TokenTrigger`],
    /// as a trigger menu's tokens carry (`data.trigger`).
    pub fn trigger(&self) -> Option<&TokenTrigger> {
        self.data.as_ref()?.downcast_ref::<TokenTrigger>()
    }
}

/// What a token a trigger menu inserts carries in its `data`
/// (`{ trigger, arguments }`): the trigger it was typed with, which its label
/// leaves out, so an editor that turns the token back into text, like a token
/// input's `revert`, gets it back; and the arguments given for it, by name,
/// in the order they were given, none when it took none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TokenTrigger {
    pub trigger: SharedString,
    pub arguments: Vec<(SharedString, SharedString)>,
}

impl TokenTrigger {
    pub fn new(trigger: impl Into<SharedString>) -> Self {
        Self {
            trigger: trigger.into(),
            arguments: Vec::new(),
        }
    }

    /// The value given for the argument `name`.
    pub fn argument(&self, name: &str) -> Option<&SharedString> {
        self.arguments
            .iter()
            .find(|(own, _)| own.as_ref() == name)
            .map(|(_, value)| value)
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Token")
            .field("kind", &self.kind)
            .field("value", &self.value)
            .field("label", &self.label)
            .field("id", &self.id)
            .field("status", &self.status)
            .field("data", &self.data.as_ref().map(|_| ".."))
            .finish()
    }
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.value == other.value
            && self.label == other.label
            && self.id == other.id
            && self.status == other.status
            && match (&self.data, &other.data) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

/// A token to insert. The same shape as a token; its `id` is generated when missing.
pub type TokenInsert = Token;

/// What a pending token resolves to, any field of a token
/// (`Partial<TokenInsert>`). `status: Some(None)` clears the status.
#[derive(Clone, Default)]
pub struct TokenUpdate {
    pub kind: Option<SharedString>,
    pub value: Option<SharedString>,
    pub label: Option<SharedString>,
    pub id: Option<SharedString>,
    pub status: Option<Option<TokenStatus>>,
    pub data: Option<Option<Rc<dyn Any>>>,
}

impl fmt::Debug for TokenUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenUpdate")
            .field("kind", &self.kind)
            .field("value", &self.value)
            .field("label", &self.label)
            .field("id", &self.id)
            .field("status", &self.status)
            .field("data", &self.data.as_ref().map(|data| data.as_ref().map(|_| "..")))
            .finish()
    }
}

/// A range of text offsets, start inclusive and end exclusive. A token counts
/// as one character, U+FFFC.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

impl TextRange {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// A range with nothing in it, where the caret is.
    pub fn caret(offset: usize) -> Self {
        Self::new(offset, offset)
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn range(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

/// A rectangle in window coordinates.
pub type TextRect = Bounds<Pixels>;

/// Something that happened in an editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorChange {
    Input {
        /// The text the edit put in, where `range` was.
        inserted: String,
        /// The range the edit replaced, in offsets from before the edit.
        range: TextRange,
        /// Whether a person typed, pasted or dictated it. A collaborator's or
        /// a program's edit is `false`, and never opens a menu.
        user: bool,
    },
    /// The caret or the selection moved.
    Selection,
    /// An input method started composing.
    CompositionStart,
    /// An input method committed.
    CompositionEnd,
}

/// What `replace` puts in: text or a token.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorContent {
    Text(SharedString),
    Token(TokenInsert),
}

impl EditorContent {
    /// The text it reads as: the text, or the token's label.
    pub fn text(&self) -> &SharedString {
        match self {
            EditorContent::Text(text) => text,
            EditorContent::Token(token) => &token.label,
        }
    }
}

impl From<&str> for EditorContent {
    fn from(text: &str) -> Self {
        EditorContent::Text(SharedString::new(text))
    }
}

impl From<SharedString> for EditorContent {
    fn from(text: SharedString) -> Self {
        EditorContent::Text(text)
    }
}

impl From<Token> for EditorContent {
    fn from(token: Token) -> Self {
        EditorContent::Token(token)
    }
}

/// What a pending token settles to: what it resolved to, if anything, or an
/// error that removes it.
pub type PendingToken = Pin<Box<dyn Future<Output = Result<Option<TokenUpdate>, SharedString>>>>;

/// Options for `replace`.
#[derive(Default)]
pub struct EditorReplaceOptions {
    /// Inserts a token as pending until this settles: updated with what it
    /// resolves to, removed if it fails.
    pub pending: Option<PendingToken>,
}

impl fmt::Debug for EditorReplaceOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EditorReplaceOptions")
            .field("pending", &self.pending.is_some())
            .finish()
    }
}

/// Up to some characters before the caret, and the offset they start at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextBefore {
    pub text: String,
    pub offset: usize,
}

/// What [`TextEditorAdapter::subscribe`] calls.
pub type EditorListener = Rc<dyn Fn(&EditorChange, &mut Window, &mut App)>;

/// The editor a trigger menu reads and writes. A text field adapter
/// ([`text_field_adapter`]) and a token input provide one. Write one for any
/// other editor.
///
/// [`text_field_adapter`]: crate::text_field_adapter
pub trait TextEditorAdapter {
    /// The focus handle of the element that gets keys and the menu's ARIA.
    /// `None` until it's rendered.
    fn focus_handle(&self, cx: &App) -> Option<FocusHandle>;

    /// Up to `length` characters before the caret, and the offset they start
    /// at. A token reads as U+FFFC. `None` while unfocused.
    fn text_before(&self, length: usize, window: &Window, cx: &App) -> Option<TextBefore>;

    /// The selection as text offsets, or `None` while unfocused.
    fn selection(&self, window: &Window, cx: &App) -> Option<TextRange>;

    /// The rectangle at a text offset, for anchoring.
    fn rect_at(&self, offset: usize, window: &Window, cx: &App) -> Option<TextRect>;

    /// Replaces a range with text or a token as one undoable step, and puts
    /// the caret after it.
    fn replace(
        &self,
        range: TextRange,
        content: EditorContent,
        options: EditorReplaceOptions,
        window: &mut Window,
        cx: &mut App,
    );

    /// Calls `listener` for text edits, caret moves and composition, until
    /// the subscription drops.
    fn subscribe(&self, listener: EditorListener, cx: &mut App) -> Subscription;

    /// Whether an input method is composing.
    fn composing(&self, cx: &App) -> bool;

    /// Whether `replace` accepts tokens.
    fn tokens(&self) -> bool {
        false
    }

    /// Whether it can send what's typed, through [`TextEditorAdapter::submit`].
    fn can_submit(&self) -> bool {
        false
    }

    /// Sends what's typed, like a form submit, for items that end with a submit.
    fn submit(&self, _window: &mut Window, _cx: &mut App) {}
}
