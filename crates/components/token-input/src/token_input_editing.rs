//! Port of `components/token-input/token-input-editing.ts`: what the
//! platform hands the editor, turned into edits of the value.
//!
//! The original lets the browser type at a caret, so spelling, autocorrect
//! and input methods keep working, and reads it back; everything that would
//! touch a token, every deletion, new lines, paste, drop, cut and undo, it
//! cancels and applies to the value, so one keystroke removes one token and
//! the history is its own. gpui has no editable element to read back, so
//! every edit applies to the value here: typed text and an input method's
//! composition come through the platform's text input, which the entity
//! handles ([`EntityInputHandler`]), and keys run once every listener has
//! had them, the way a browser's default action follows `keydown`. A
//! listener keeps the editor from acting on a key with
//! `window.prevent_default()`, as `preventDefault()` does, or with
//! `cx.stop_propagation()`. The keys, the moves and selecting with the mouse
//! are the native crate's text field's, with a token one character wide.

use std::cell::RefCell;
use std::ops::Range;

use gpui::{
    Bounds, ClipboardEntry, ClipboardItem, Context, EntityInputHandler,
    KeyDownEvent, Keystroke, KeystrokeEvent, MouseDownEvent, OperatingSystem, Pixels, Point,
    ScrollEvent, SharedString, TextInputConfiguration, UTF16Selection, Window, point, px,
};
use ui_sdk_kernel::{AgentValue, InputEvent, Reason, is_composing, json};
use ui_sdk_native::SelectionDirection;
use ui_sdk_platform::is_apple;
use ui_sdk_text::{EditorChange, TextRange, Token, is_js_space};
use unicode_segmentation::UnicodeSegmentation;

use crate::token_input::{TokenDeletion, TokenInput, TokenInputCopy, TokenInputSubmitShortcut};
use crate::token_input_api::Edit;
use crate::token_input_model::{
    Segment, TOKEN_CHAR, TOKEN_LEN, TokenInputValue, floor_char, from_text, line_boundary,
    offset_text, replace_range, slice_value, snap_offset, to_label, to_text, token_at,
    token_before, value_length, word_boundary,
};

/// An input method composing: the marked text, what it composes over, and
/// the selection inside it. Nothing it composes is in the value until it
/// commits, as the original reads it back only at `compositionend`.
#[derive(Clone, Debug)]
pub(crate) struct Composition {
    /// The range of the latest value it composes over.
    pub range: TextRange,
    /// The marked text.
    pub text: String,
    /// The selection inside the marked text.
    pub selected: Range<usize>,
}

/// The unit a drag selects by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Granularity {
    Character,
    Word,
    Line,
}

/// A drag selecting text.
#[derive(Clone, Debug)]
pub(crate) struct Drag {
    granularity: Granularity,
    // What the press selected, which the drag keeps selected.
    origin: Range<usize>,
}

thread_local! {
    // What Control-K last cut on a Mac, for Control-Y. The platform keeps one
    // for every field, so this does too.
    static KILL_BUFFER: RefCell<TokenInputValue> = const { RefCell::new(Vec::new()) };
}

// The browser's names for the edits it reports, `inputType`.
const INSERT_TEXT: &str = "insertText";
const INSERT_REPLACEMENT_TEXT: &str = "insertReplacementText";
const INSERT_PARAGRAPH: &str = "insertParagraph";
const INSERT_LINE_BREAK: &str = "insertLineBreak";
const INSERT_FROM_YANK: &str = "insertFromYank";
const DELETE_BY_CUT: &str = "deleteByCut";

/// Where a move takes the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Movement {
    Left,
    Right,
    WordLeft,
    /// To the end of the next word: a Mac, and Linux.
    WordRight,
    /// To the start of the next word: Windows.
    NextWordStart,
    /// The start of the visual line.
    LineStart,
    LineEnd,
    ParagraphStart,
    ParagraphEnd,
    /// The start of this paragraph, or of the one before when already there.
    ParagraphBackward,
    /// The end of this paragraph, or of the one after when already there.
    ParagraphForward,
    Up,
    Down,
    PageUp,
    PageDown,
    Start,
    End,
}

/// What a deletion removes, when nothing is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Deletion {
    CharBackward,
    CharForward,
    WordBackward,
    /// To the end of the next word.
    WordForward,
    /// To the start of the next word: Windows.
    WordForwardToStart,
    LineBackward,
    LineForward,
    /// To the end of the paragraph, and the Mac's kill buffer.
    ParagraphForward,
}

impl Deletion {
    // The browser's `inputType` for it.
    fn input_type(self) -> &'static str {
        match self {
            Deletion::CharBackward => "deleteContentBackward",
            Deletion::CharForward => "deleteContentForward",
            Deletion::WordBackward => "deleteWordBackward",
            Deletion::WordForward | Deletion::WordForwardToStart => "deleteWordForward",
            Deletion::LineBackward => "deleteSoftLineBackward",
            Deletion::LineForward => "deleteSoftLineForward",
            Deletion::ParagraphForward => "deleteHardLineForward",
        }
    }
}

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Move(Movement, bool),
    Delete(Deletion),
    LineBreak,
    /// Control-O on a Mac: a line break after the caret.
    OpenLine,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Yank,
    Transpose,
    CharacterPalette,
}

/// The command a keystroke runs in the editor, if any: each platform's
/// multi-line text field's.
pub(crate) fn command_for(keystroke: &Keystroke, os: OperatingSystem) -> Option<Command> {
    if matches!(os, OperatingSystem::Mac | OperatingSystem::Ios) {
        mac(keystroke)
    } else {
        other(keystroke, os)
    }
}

fn mac(keystroke: &Keystroke) -> Option<Command> {
    use Command::*;
    use Movement::*;
    let modifiers = keystroke.modifiers;
    let (ctrl, alt, shift, cmd) = (modifiers.control, modifiers.alt, modifiers.shift, modifiers.platform);
    let key = keystroke.key.as_str();
    if ctrl && !cmd && !alt {
        return match key {
            "a" => Some(Move(ParagraphStart, shift)),
            "e" => Some(Move(ParagraphEnd, shift)),
            "b" => Some(Move(Left, shift)),
            "f" => Some(Move(Right, shift)),
            "p" => Some(Move(Up, shift)),
            "n" => Some(Move(Down, shift)),
            "d" if !shift => Some(Delete(Deletion::CharForward)),
            "h" if !shift => Some(Delete(Deletion::CharBackward)),
            "k" if !shift => Some(Delete(Deletion::ParagraphForward)),
            "y" if !shift => Some(Yank),
            "t" if !shift => Some(Transpose),
            "o" if !shift => Some(OpenLine),
            "backspace" => Some(Delete(Deletion::CharBackward)),
            _ => None,
        };
    }
    if ctrl && cmd && !alt && !shift && key == "space" {
        return Some(CharacterPalette);
    }
    if ctrl {
        return None;
    }
    match key {
        "left" => Some(Move(
            if cmd {
                LineStart
            } else if alt {
                WordLeft
            } else {
                Left
            },
            shift,
        )),
        "right" => Some(Move(
            if cmd {
                LineEnd
            } else if alt {
                WordRight
            } else {
                Right
            },
            shift,
        )),
        "up" => Some(Move(
            if cmd {
                Start
            } else if alt {
                ParagraphBackward
            } else {
                Up
            },
            shift,
        )),
        "down" => Some(Move(
            if cmd {
                End
            } else if alt {
                ParagraphForward
            } else {
                Down
            },
            shift,
        )),
        "home" if !alt => Some(Move(if cmd { Start } else { LineStart }, shift)),
        "end" if !alt => Some(Move(if cmd { End } else { LineEnd }, shift)),
        "pageup" if !cmd && !alt => Some(Move(PageUp, shift)),
        "pagedown" if !cmd && !alt => Some(Move(PageDown, shift)),
        "backspace" => Some(Delete(if cmd {
            Deletion::LineBackward
        } else if alt {
            Deletion::WordBackward
        } else {
            Deletion::CharBackward
        })),
        "delete" => Some(Delete(if cmd {
            Deletion::LineForward
        } else if alt {
            Deletion::WordForward
        } else {
            Deletion::CharForward
        })),
        "enter" if !cmd => Some(LineBreak),
        "a" if cmd && !alt && !shift => Some(SelectAll),
        "c" if cmd && !alt && !shift => Some(Copy),
        "x" if cmd && !alt && !shift => Some(Cut),
        "v" if cmd => Some(Paste),
        "z" if cmd && !alt => Some(if shift { Redo } else { Undo }),
        _ => None,
    }
}

fn other(keystroke: &Keystroke, os: OperatingSystem) -> Option<Command> {
    use Command::*;
    use Movement::*;
    let modifiers = keystroke.modifiers;
    let (ctrl, alt, shift) = (modifiers.control, modifiers.alt, modifiers.shift);
    let windows = os == OperatingSystem::Windows;
    let linux = !windows;
    let key = keystroke.key.as_str();
    if modifiers.platform {
        return None;
    }
    if alt {
        // Alt-Backspace undoes on Windows. Alt and an arrow go back and forward.
        return (windows && key == "backspace" && !ctrl && !shift).then_some(Undo);
    }
    match key {
        "left" => Some(Move(if ctrl { WordLeft } else { Left }, shift)),
        "right" => Some(Move(
            match (ctrl, windows) {
                (false, _) => Right,
                (true, true) => NextWordStart,
                (true, false) => WordRight,
            },
            shift,
        )),
        "up" => Some(Move(if ctrl { ParagraphBackward } else { Up }, shift)),
        "down" => Some(Move(if ctrl { ParagraphForward } else { Down }, shift)),
        "home" => Some(Move(if ctrl { Start } else { LineStart }, shift)),
        "end" => Some(Move(if ctrl { End } else { LineEnd }, shift)),
        "pageup" if !ctrl => Some(Move(PageUp, shift)),
        "pagedown" if !ctrl => Some(Move(PageDown, shift)),
        "backspace" => Some(Delete(if ctrl && shift && linux {
            Deletion::LineBackward
        } else if ctrl {
            Deletion::WordBackward
        } else {
            Deletion::CharBackward
        })),
        "delete" if shift && !ctrl => Some(Cut),
        "delete" => Some(Delete(match (ctrl, shift, windows) {
            (false, _, _) => Deletion::CharForward,
            (true, true, false) => Deletion::LineForward,
            (true, _, true) => Deletion::WordForwardToStart,
            (true, _, false) => Deletion::WordForward,
        })),
        "insert" if ctrl && !shift => Some(Copy),
        "insert" if shift && !ctrl => Some(Paste),
        "enter" if !ctrl => Some(LineBreak),
        "a" if ctrl && !shift => Some(SelectAll),
        "c" if ctrl && !shift => Some(Copy),
        "x" if ctrl && !shift => Some(Cut),
        "v" if ctrl => Some(Paste),
        "z" if ctrl => Some(if shift { Redo } else { Undo }),
        "y" if ctrl && !shift => Some(Redo),
        _ => None,
    }
}

// The start of the character before `at`: a grapheme of text, or a token,
// which is one character to the caret.
pub(crate) fn previous_stop(text: &str, at: usize) -> usize {
    let at = floor_char(text, at);
    if at == 0 {
        return 0;
    }
    if text[..at].ends_with(TOKEN_CHAR) {
        return at - TOKEN_LEN;
    }
    let piece = text[..at].rfind(TOKEN_CHAR).map_or(0, |index| index + TOKEN_LEN);
    text[piece..at]
        .grapheme_indices(true)
        .next_back()
        .map_or(piece, |(index, _)| piece + index)
}

// The end of the character after `at`.
pub(crate) fn next_stop(text: &str, at: usize) -> usize {
    let at = floor_char(text, at);
    if at >= text.len() {
        return text.len();
    }
    if text[at..].starts_with(TOKEN_CHAR) {
        return at + TOKEN_LEN;
    }
    let piece = text[at..].find(TOKEN_CHAR).map_or(text.len(), |index| at + index);
    text[at..piece]
        .graphemes(true)
        .next()
        .map_or(piece, |grapheme| at + grapheme.len())
}

// A segment with a letter or a digit in it is a word, and so is a token:
// word moves stop at it, as word deletions do.
fn is_word(segment: &str) -> bool {
    segment.chars().any(|c| c.is_alphanumeric() || c == TOKEN_CHAR)
}

fn is_space(segment: &str) -> bool {
    segment.chars().all(char::is_whitespace)
}

// The start of the word before `offset`.
fn previous_word_start(text: &str, offset: usize) -> usize {
    text[..offset]
        .split_word_bound_indices()
        .rev()
        .find(|(_, segment)| is_word(segment))
        .map_or(0, |(start, _)| start)
}

// The end of the word after `offset`.
fn next_word_end(text: &str, offset: usize) -> usize {
    text[offset..]
        .split_word_bound_indices()
        .find(|(_, segment)| is_word(segment))
        .map_or(text.len(), |(start, segment)| offset + start + segment.len())
}

// The start of the word after the one at `offset`: Control-Right on Windows.
fn next_word_start(text: &str, offset: usize) -> usize {
    let mut first = true;
    for (start, segment) in text[offset..].split_word_bound_indices() {
        let space = is_space(segment);
        if first {
            first = false;
            if !space {
                continue;
            }
        }
        if !space {
            return offset + start;
        }
    }
    text.len()
}

// The word, run of spaces or punctuation, or token, that the character
// starting at `offset` belongs to: what a double click selects.
fn word_at(text: &str, offset: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let offset = offset.min(text.len());
    let mut last = 0..0;
    for (start, segment) in text.split_word_bound_indices() {
        let end = start + segment.len();
        if offset < end {
            if segment == "\n" && start > 0 {
                return last;
            }
            return start..end;
        }
        last = start..end;
    }
    last
}

fn paragraph_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |index| index + 1)
}

fn paragraph_end(text: &str, offset: usize) -> usize {
    text[offset..].find('\n').map_or(text.len(), |index| offset + index)
}

// The UTF-8 offset of a UTF-16 one, which the platform's text input counts in.
pub(crate) fn utf16_to_offset(text: &str, utf16: usize) -> usize {
    let mut offset = 0;
    let mut count = 0;
    for c in text.chars() {
        if count >= utf16 {
            break;
        }
        count += c.len_utf16();
        offset += c.len_utf8();
    }
    offset
}

pub(crate) fn offset_to_utf16(text: &str, offset: usize) -> usize {
    text[..floor_char(text, offset)].encode_utf16().count()
}

fn utf16_range(text: &str, range: &Range<usize>) -> Range<usize> {
    utf16_to_offset(text, range.start)..utf16_to_offset(text, range.end)
}

fn to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    offset_to_utf16(text, range.start)..offset_to_utf16(text, range.end)
}

// Enter, Tab and Backspace arrive as keys. A platform that also sends their
// control character as text has it dropped here.
fn without_lone_control(text: &str) -> &str {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_control() => "",
        _ => text,
    }
}

// What typing may put in: line breaks as `\n`, and no control characters
// but tabs.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

// Where a deletion from `at` ends: a character, a word or a line away.
fn deletion_target(input_type: &str, value: &[Segment], at: usize, direction: isize) -> usize {
    if input_type.contains("Word") {
        return word_boundary(value, at, direction);
    }
    if input_type.contains("Line") {
        return line_boundary(value, at, direction);
    }
    let text = offset_text(value);
    if direction < 0 {
        text[..at].chars().next_back().map_or(at, |c| at - c.len_utf8())
    } else {
        text[at..].chars().next().map_or(at, |c| at + c.len_utf8())
    }
}

// The trigger a token was typed with, which `revert` puts back before its
// label: its data's `trigger`, as a trigger menu's tokens carry it
// (`TokenTrigger`), or from data of the app's own, JSON with a `trigger`
// key or the trigger itself.
fn trigger_of(token: &Token) -> String {
    if let Some(trigger) = token.trigger() {
        return trigger.trigger.to_string();
    }
    let Some(data) = &token.data else {
        return String::new();
    };
    if let Some(value) = data.downcast_ref::<AgentValue>() {
        return value
            .get("trigger")
            .and_then(AgentValue::as_str)
            .unwrap_or_default()
            .to_string();
    }
    if let Some(trigger) = data.downcast_ref::<SharedString>() {
        return trigger.to_string();
    }
    data.downcast_ref::<String>().cloned().unwrap_or_default()
}

// How one edit through the editor reports itself.
pub(crate) struct EditDetails {
    pub reason: Reason,
    pub event: Option<InputEvent>,
    pub input_type: Option<&'static str>,
    pub group: Option<&'static str>,
    pub at: Option<usize>,
}

impl EditDetails {
    fn new(reason: Reason, event: Option<InputEvent>) -> Self {
        Self {
            reason,
            event,
            input_type: None,
            group: None,
            at: None,
        }
    }
}

impl TokenInput {
    // An edit through the editor: an edit anywhere but where the last typing
    // or deletion left the caret starts a new undo step.
    pub(crate) fn edit(
        &mut self,
        next: TokenInputValue,
        selection: TextRange,
        details: EditDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if details.group.is_some()
            && let Some(at) = details.at
            && Some(at) != self.last_end
        {
            self.history.seal();
        }
        let grouped = details.group.is_some();
        let edit = Edit::new(details.reason)
            .event(details.event)
            .input_type(details.input_type)
            .selection(Some(selection))
            .group(details.group);
        self.commit(&next, edit, window, cx);
        self.last_end = grouped.then_some(selection.end);
    }

    // Backspace, Delete and their word and line forms, applied to the value.
    pub(crate) fn remove(
        &mut self,
        input_type: &'static str,
        range: TextRange,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.committed.clone();
        if range.start != range.end {
            self.armed = None;
            let reason = if input_type == DELETE_BY_CUT {
                Reason::Cut
            } else {
                Reason::InputChange
            };
            let mut details = EditDetails::new(reason, event);
            details.input_type = Some(input_type);
            self.edit(replace_range(&current, range, &[]), TextRange::caret(range.start), details, window, cx);
            return;
        }
        let backward = input_type.contains("Backward");
        let direction = if backward { -1 } else { 1 };
        let at = range.start;
        let neighbour = if backward {
            token_before(&current, at)
        } else {
            token_at(&current, at)
        };
        if let Some(token) = neighbour.cloned()
            && (input_type.starts_with("deleteContent") || input_type.starts_with("deleteWord"))
        {
            let offset = if backward { at - TOKEN_LEN } else { at };
            self.remove_token_key(token, offset, event, input_type, window, cx);
            return;
        }
        self.armed = None;
        let target = deletion_target(input_type, &current, at, direction).min(value_length(&current));
        if target == at {
            return;
        }
        let start = at.min(target);
        if input_type == Deletion::ParagraphForward.input_type() {
            let killed = slice_value(&current, start, at.max(target));
            KILL_BUFFER.with(|buffer| *buffer.borrow_mut() = killed);
        }
        let mut details = EditDetails::new(Reason::InputChange, event);
        details.input_type = Some(input_type);
        details.group = Some("delete");
        details.at = Some(at);
        self.edit(
            replace_range(&current, TextRange::new(start, at.max(target)), &[]),
            TextRange::caret(start),
            details,
            window,
            cx,
        );
    }

    // One keystroke on a token: removes it, selects it first, or turns it
    // back into its text.
    fn remove_token_key(
        &mut self,
        token: Token,
        offset: usize,
        event: Option<InputEvent>,
        input_type: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.committed.clone();
        let id = token.id.clone().unwrap_or_default();
        let policy = self.settings.token_deletion;
        let range = TextRange::new(offset, offset + TOKEN_LEN);
        if policy == TokenDeletion::Select && self.armed.as_ref() != Some(&id) {
            self.armed = Some(id);
            self.select_range(range, window, cx);
            let message = format!(
                "{}, {}, selected. Press Backspace again to remove it",
                token.label,
                self.describe(&token)
            );
            self.say(message, window, cx);
            return;
        }
        self.armed = None;
        // Reverted, a token is its trigger and label again, so a trigger menu
        // opens on it.
        let text = if policy == TokenDeletion::Revert {
            format!("{}{}", trigger_of(&token), token.label)
        } else {
            String::new()
        };
        let end = offset + text.len();
        let replacement: TokenInputValue = if text.is_empty() {
            Vec::new()
        } else {
            vec![Segment::Text(text.into())]
        };
        let mut details = EditDetails::new(Reason::InputChange, event);
        details.input_type = Some(input_type);
        self.edit(replace_range(&current, range, &replacement), TextRange::caret(end), details, window, cx);
    }

    // Typing, or a new line, over a range: a new word starts a new undo step.
    pub(crate) fn insert_text(
        &mut self,
        text: &str,
        range: TextRange,
        input_type: &'static str,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.after_space && text.chars().any(|c| !is_js_space(c)) {
            self.history.seal();
        }
        self.after_space = text.chars().next_back().is_some_and(is_js_space);
        let range = crate::token_input_model::snap_range(&self.committed, range);
        let next = replace_range(&self.committed, range, &[Segment::Text(SharedString::new(text))]);
        let mut details = EditDetails::new(Reason::InputChange, event);
        details.input_type = Some(input_type);
        details.group = Some("insert");
        details.at = Some(range.start);
        self.edit(next, TextRange::caret(range.start + text.len()), details, window, cx);
    }

    // Runs once every listener has had the key, unless one stopped it.
    pub(crate) fn keystroke(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.refused_text = None;
        if event.action.is_some()
            || self.is_disabled()
            || self.composition.is_some()
            || !self.focus_handle.is_focused(window)
        {
            return;
        }
        if window.default_prevented() {
            // The platform still types the key's text after this; it goes too.
            self.refused_text = event.keystroke.key_char.clone();
            return;
        }
        if is_composing(window, cx) {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let apple = is_apple(cx);
        let command = if apple { modifiers.platform } else { modifiers.control };
        let key = keystroke.key.to_lowercase();
        let input = InputEvent::KeyDown(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        if command && !modifiers.alt && key == "z" {
            if self.is_editable() {
                if modifiers.shift {
                    self.redo_with(Some(input), window, cx);
                } else {
                    self.undo_with(Some(input), window, cx);
                }
            }
            return;
        }
        if !apple && modifiers.control && key == "y" {
            if self.is_editable() {
                self.redo_with(Some(input), window, cx);
            }
            return;
        }
        // Formatting has no place in plain text.
        if command && matches!(key.as_str(), "b" | "i" | "u") {
            return;
        }
        if key == "enter" {
            let submits = match self.settings.submit_shortcut {
                TokenInputSubmitShortcut::Enter => !modifiers.shift && !command && !modifiers.alt,
                TokenInputSubmitShortcut::ModEnter => command && !modifiers.shift,
            };
            // With nothing to submit, Enter is left to start a new line.
            if submits && self.can_submit() {
                self.submit(Some(input), window, cx);
                return;
            }
        }
        let os = cx.platform_info().os;
        if let Some(found) = command_for(keystroke, os) {
            self.run(found, input, window, cx);
        }
    }

    // Runs what a key does.
    fn run(&mut self, command: Command, event: InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::Move(movement, extend) => self.move_caret(movement, extend, window, cx),
            Command::Delete(deletion) => {
                if self.is_editable() {
                    let range = self.selection();
                    self.remove(deletion.input_type(), range, Some(event), window, cx);
                }
            }
            Command::LineBreak | Command::OpenLine => {
                if self.is_editable() {
                    let shift = matches!(&event, InputEvent::KeyDown(key) if key.keystroke.modifiers.shift);
                    let input_type = if shift { INSERT_LINE_BREAK } else { INSERT_PARAGRAPH };
                    let range = self.selection();
                    self.insert_text("\n", range, input_type, Some(event), window, cx);
                }
            }
            Command::SelectAll => self.select_all(window, cx),
            Command::Copy => self.copy(cx),
            Command::Cut => self.cut(Some(event), window, cx),
            Command::Paste => self.paste(Some(event), window, cx),
            Command::Undo => {
                if self.is_editable() {
                    self.undo_with(Some(event), window, cx);
                }
            }
            Command::Redo => {
                if self.is_editable() {
                    self.redo_with(Some(event), window, cx);
                }
            }
            Command::Yank => {
                let killed = KILL_BUFFER.with(|buffer| buffer.borrow().clone());
                if self.is_editable() && !killed.is_empty() {
                    let range = self.selection();
                    let next = replace_range(&self.committed, range, &killed);
                    let end = range.start + value_length(&killed);
                    let mut details = EditDetails::new(Reason::InputChange, Some(event));
                    details.input_type = Some(INSERT_FROM_YANK);
                    self.edit(next, TextRange::caret(end), details, window, cx);
                }
            }
            // An edit the original refuses, like formatting.
            Command::Transpose => {}
            Command::CharacterPalette => window.show_character_palette(),
        }
    }

    /// Selects everything, as Command-A or Control-A does.
    pub fn select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let length = value_length(&self.committed);
        self.select_range(TextRange::new(0, length), window, cx);
    }

    // The end of the selection that moves: the caret.
    pub(crate) fn head(&self) -> usize {
        if self.direction == SelectionDirection::Backward {
            self.start
        } else {
            self.end
        }
    }

    // The end of the selection that stays.
    pub(crate) fn anchor(&self) -> usize {
        if self.direction == SelectionDirection::Backward {
            self.end
        } else {
            self.start
        }
    }

    pub(crate) fn place(&mut self, offset: usize, upstream: bool) {
        let offset = snap_offset(&self.committed, offset.min(value_length(&self.committed)), false);
        self.start = offset;
        self.end = offset;
        self.direction = SelectionDirection::None;
        self.upstream = upstream;
    }

    pub(crate) fn select_to(&mut self, anchor: usize, head: usize, upstream: bool) {
        let length = value_length(&self.committed);
        let anchor = anchor.min(length);
        let head = head.min(length);
        if head < anchor {
            self.start = snap_offset(&self.committed, head, false);
            self.end = snap_offset(&self.committed, anchor, true);
            self.direction = SelectionDirection::Backward;
        } else if head > anchor {
            self.start = snap_offset(&self.committed, anchor, false);
            self.end = snap_offset(&self.committed, head, true);
            self.direction = SelectionDirection::Forward;
        } else {
            self.place(head, upstream);
        }
        self.upstream = upstream;
    }

    fn move_caret(&mut self, movement: Movement, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        use Movement::*;
        let vertical = matches!(movement, Up | Down | PageUp | PageDown);
        if !vertical {
            self.goal_x = None;
        }
        let collapsed = self.start == self.end;
        // An arrow collapses a selection to the end it points at.
        if !extend && !collapsed && matches!(movement, Left | Right) {
            let at = if movement == Left { self.start } else { self.end };
            self.place(at, false);
            self.changed(window, cx);
            return;
        }
        let backward = matches!(
            movement,
            Left | WordLeft | LineStart | ParagraphStart | ParagraphBackward | Up | PageUp | Start
        );
        let from = if extend || collapsed {
            self.head()
        } else if backward {
            self.start
        } else {
            self.end
        };
        let upstream = self.upstream && from == self.head();
        let (target, target_upstream) = self.target(movement, from, upstream);
        if extend {
            let anchor = self.anchor();
            self.select_to(anchor, target, target_upstream);
        } else {
            self.place(target, target_upstream);
        }
        self.changed(window, cx);
    }

    // Where a move from `from` lands, and whether at the end of a wrapped row.
    fn target(&mut self, movement: Movement, from: usize, upstream: bool) -> (usize, bool) {
        use Movement::*;
        let text = offset_text(&self.committed);
        let len = text.len();
        match movement {
            Left => (previous_stop(&text, from), false),
            Right => (next_stop(&text, from), false),
            WordLeft => (previous_word_start(&text, from), false),
            WordRight => (next_word_end(&text, from), false),
            NextWordStart => (next_word_start(&text, from), false),
            ParagraphStart => (paragraph_start(&text, from), false),
            ParagraphEnd => (paragraph_end(&text, from), false),
            ParagraphBackward => {
                let start = paragraph_start(&text, from);
                if start == from && from > 0 {
                    (paragraph_start(&text, from - 1), false)
                } else {
                    (start, false)
                }
            }
            ParagraphForward => {
                let end = paragraph_end(&text, from);
                if end == from && from < len {
                    (paragraph_end(&text, from + 1), false)
                } else {
                    (end, false)
                }
            }
            Start => (0, false),
            End => (len, false),
            LineStart => (self.line_range(&text, from, upstream).start, false),
            LineEnd => {
                let range = self.line_range(&text, from, upstream);
                (range.end, self.row_wraps(from, upstream))
            }
            Up | Down | PageUp | PageDown => self.vertical(movement, from, upstream),
        }
    }

    // The visual line an offset is on: its row in the last layout, or its
    // paragraph before there is one.
    fn line_range(&self, text: &str, offset: usize, upstream: bool) -> Range<usize> {
        let Some(geometry) = &self.geometry else {
            return paragraph_start(text, offset)..paragraph_end(text, offset);
        };
        let shown = geometry.shown.to_shown(offset);
        let row = geometry.layout.row_for(shown, upstream);
        let range = geometry.layout.row_range(row);
        let start = geometry.shown.to_value(range.start);
        let end = geometry.shown.to_value(range.end);
        start..end.max(start)
    }

    fn row_wraps(&self, offset: usize, upstream: bool) -> bool {
        let Some(geometry) = &self.geometry else {
            return false;
        };
        let row = geometry.layout.row_for(geometry.shown.to_shown(offset), upstream);
        geometry.layout.rows[row].wraps
    }

    // Up and Down: a row at a time, or a page, toward where the caret aimed.
    // Past the first row the caret goes to the start, past the last to the end.
    fn vertical(&mut self, movement: Movement, from: usize, upstream: bool) -> (usize, bool) {
        let up = matches!(movement, Movement::Up | Movement::PageUp);
        let edge = if up { 0 } else { value_length(&self.committed) };
        let Some(geometry) = self.geometry.clone() else {
            return (edge, false);
        };
        let layout = &geometry.layout;
        let shown = geometry.shown.to_shown(from);
        let row = layout.row_for(shown, upstream);
        let x = *self.goal_x.get_or_insert_with(|| layout.x_for(row, shown));
        let count = match movement {
            Movement::PageUp | Movement::PageDown => {
                let visible = (geometry.bounds.size.height / layout.line_height).floor() as usize;
                visible.saturating_sub(1).max(1)
            }
            _ => 1,
        };
        let last = layout.rows.len() - 1;
        let target = if up {
            if row == 0 {
                return (edge, false);
            }
            row.saturating_sub(count)
        } else {
            if row == last {
                return (edge, false);
            }
            (row + count).min(last)
        };
        let offset = layout.offset_on_row(target, x);
        let wraps = layout.rows[target].wraps && offset == layout.rows[target].end;
        (geometry.shown.to_value(offset), wraps)
    }

    // What copying a range writes: the codec text or the labels as text, and
    // the codec text beside it, so a paste into a token input gets the
    // tokens back.
    fn write_clipboard(&self, range: TextRange, cx: &mut Context<Self>) {
        let part = slice_value(&self.committed, range.start, range.end);
        let text = to_text(&part, self.codec());
        let plain = match self.settings.copy {
            TokenInputCopy::Label => to_label(&part),
            TokenInputCopy::Codec => text.clone(),
        };
        cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
            plain,
            json!({ "uiTokens": text }),
        ));
    }

    // What a paste puts in: the tokens another token input copied, or the
    // text read through the codec.
    fn read_clipboard(&self, item: &ClipboardItem) -> TokenInputValue {
        let tokens = item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::String(string) => string
                .metadata_json::<AgentValue>()
                .and_then(|metadata| metadata.get("uiTokens").and_then(AgentValue::as_str).map(str::to_string)),
            _ => None,
        });
        match tokens {
            Some(tokens) => from_text(&tokens, self.codec()),
            None => item.text().map_or_else(Vec::new, |text| from_text(&clean(&text), self.codec())),
        }
    }

    /// Copies the selection.
    pub fn copy(&mut self, cx: &mut Context<Self>) {
        let range = self.selection();
        if range.is_empty() {
            return;
        }
        self.write_clipboard(range, cx);
    }

    /// Cuts the selection, as a person would.
    pub fn cut(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let range = self.selection();
        if range.is_empty() || !self.is_editable() {
            return;
        }
        self.write_clipboard(range, cx);
        let next = replace_range(&self.committed, range, &[]);
        self.edit(next, TextRange::caret(range.start), EditDetails::new(Reason::Cut, event), window, cx);
    }

    /// Pastes the clipboard over the selection, as a person would.
    pub fn paste(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        self.paste_item(&item, event, window, cx);
    }

    fn paste_item(&mut self, item: &ClipboardItem, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_composition(window, cx);
        if !self.is_editable() {
            return;
        }
        let range = self.selection();
        let content = self.read_clipboard(item);
        // An image or a file has no text, and leaves the selection alone.
        if content.is_empty() {
            return;
        }
        let next = replace_range(&self.committed, range, &content);
        let end = range.start + value_length(&content);
        self.edit(next, TextRange::caret(end), EditDetails::new(Reason::Paste, event), window, cx);
    }

    // Typed text, or an input method's commit.
    fn platform_insert(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = clean(without_lone_control(text));
        if let Some(composition) = self.composition.take() {
            self.finish_composition(composition, Some(text), window, cx);
            return;
        }
        if !self.is_editable() {
            return;
        }
        // A key a listener refused still sends its text.
        if let Some(refused) = self.refused_text.take()
            && refused == text
        {
            return;
        }
        if text.is_empty() {
            return;
        }
        let selection = self.selection();
        let (range, input_type) = match range_utf16 {
            Some(range) => {
                let shown = offset_text(&self.committed);
                let range = utf16_range(&shown, &range);
                let range = crate::token_input_model::snap_range(
                    &self.committed,
                    TextRange::new(range.start, range.end),
                );
                let input_type = if range == selection {
                    INSERT_TEXT
                } else {
                    INSERT_REPLACEMENT_TEXT
                };
                (range, input_type)
            }
            None => (selection, INSERT_TEXT),
        };
        self.insert_text(&text, range, input_type, None, window, cx);
    }

    // An input method's text while it composes, which replaces the marked
    // text, or the selection when it starts. It never changes the value
    // until it commits.
    fn platform_compose(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        selected_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_editable() {
            return;
        }
        if self.composition.is_none() {
            self.composition = Some(Composition {
                range: self.selection(),
                text: String::new(),
                selected: 0..0,
            });
            ui_sdk_kernel::set_composing(window, true, cx);
            self.sync_mirror();
            self.emit(EditorChange::CompositionStart, window, cx);
        }
        let Some(mut composition) = self.composition.take() else {
            return;
        };
        let shown = crate::token_input_view::Shown::of(&self.committed, Some(&composition));
        let marked = shown.marked.clone().unwrap_or(0..0);
        let range = match range_utf16 {
            Some(range) => {
                let range = utf16_range(&shown.text, &range);
                // What the input method replaces stays within the text around
                // the marked text, never a token.
                let lower = shown.text[..marked.start].rfind(TOKEN_CHAR).map_or(0, |index| index + TOKEN_LEN);
                let upper = shown.text[marked.end..]
                    .find(TOKEN_CHAR)
                    .map_or(shown.text.len(), |index| marked.end + index);
                range.start.clamp(lower, upper)..range.end.clamp(lower, upper)
            }
            None => marked.clone(),
        };
        let start = range.start.min(marked.start);
        let end = range.end.max(marked.end);
        let mut next = String::new();
        next.push_str(&shown.text[start..range.start.max(start)]);
        next.push_str(text);
        next.push_str(&shown.text[range.end.min(end)..end]);
        let lead = range.start.max(start) - start;
        // What it now composes over: its range, widened by what the input
        // method replaced around the marked text.
        let from = if start < marked.start {
            shown.to_value(start)
        } else {
            composition.range.start
        };
        let to = if end > marked.end {
            shown.to_value(end)
        } else {
            composition.range.end
        };
        composition.range = TextRange::new(from, to);
        composition.selected = match selected_utf16 {
            Some(selected) => {
                let from = lead + utf16_to_offset(text, selected.start);
                let to = lead + utf16_to_offset(text, selected.end);
                from.min(next.len())..to.min(next.len())
            }
            None => (lead + text.len())..(lead + text.len()),
        };
        composition.text = next;
        if text.is_empty() && composition.text.is_empty() {
            // The input method gave up, leaving what it composed over.
            self.finish_composition(composition, None, window, cx);
            return;
        }
        self.composition = Some(composition);
        self.sync_mirror();
        self.changed(window, cx);
    }

    // The committed text is read back as one step of its own (`compositionend`).
    pub(crate) fn finish_composition(
        &mut self,
        composition: Composition,
        text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        ui_sdk_kernel::set_composing(window, false, cx);
        self.composition = None;
        self.sync_mirror();
        self.emit(EditorChange::CompositionEnd, window, cx);
        self.history.seal();
        let text = text.unwrap_or(composition.text);
        // What an input method committed into a read-only input comes back out.
        if !self.is_editable() || (text.is_empty() && composition.range.is_empty()) {
            self.changed(window, cx);
            return;
        }
        let range = crate::token_input_model::snap_range(&self.committed, composition.range);
        let next = replace_range(&self.committed, range, &[Segment::Text(text.clone().into())]);
        let mut details = EditDetails::new(Reason::InputChange, None);
        details.at = Some(range.start);
        self.edit(next, TextRange::caret(range.start + text.len()), details, window, cx);
        self.history.seal();
        self.changed(window, cx);
    }

    /// Commits what an input method is composing, as it stands: what a
    /// press elsewhere, losing focus or a paste does.
    pub(crate) fn commit_composition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(composition) = self.composition.take() {
            self.finish_composition(composition, None, window, cx);
        }
    }

    // The window rectangle of a range's start, as wide as the range on its row.
    fn platform_bounds(&self, range: Range<usize>) -> Option<Bounds<Pixels>> {
        let geometry = self.geometry.as_ref()?;
        let layout = &geometry.layout;
        let row = layout.row_for(range.start, false);
        let x0 = layout.x_for(row, range.start);
        let x1 = if layout.row_for(range.end, true) == row {
            layout.x_for(row, range.end)
        } else {
            x0
        };
        let top = layout.text_top(row);
        Some(Bounds::from_corners(
            geometry.window(point(x0, top)),
            geometry.window(point(x1.max(x0), top + layout.line_height)),
        ))
    }

    // What the platform's text input reads: the shown text, with what an
    // input method composes.
    fn shown_text(&self) -> SharedString {
        crate::token_input_view::Shown::of(&self.committed, self.composition.as_ref()).text
    }

    // The selection the platform's text input reads, in shown offsets.
    fn shown_selection(&self) -> Range<usize> {
        match &self.composition {
            Some(composition) => {
                let start = composition.range.start;
                (start + composition.selected.start)..(start + composition.selected.end)
            }
            None => self.start..self.end,
        }
    }

    // A press puts the caret, Shift extends, a double press selects a word
    // and a triple press a line, and dragging extends by the same unit. A
    // press on a token selects all of it, as `user-select: all` does.
    pub(crate) fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if window.default_prevented() {
            return;
        }
        window.prevent_default();
        if self.is_disabled() {
            return;
        }
        cx.stop_propagation();
        window.request_virtual_keyboard();
        self.commit_composition(window, cx);
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }
        self.goal_x = None;
        self.history.seal();
        let Some(geometry) = self.geometry.clone() else {
            return;
        };
        if let Some(start) = geometry.token_under(event.position) {
            let range = TextRange::new(start, start + TOKEN_LEN);
            if event.modifiers.shift {
                let anchor = self.anchor();
                let head = if range.start < anchor { range.start } else { range.end };
                self.select_to(anchor, head, false);
            } else {
                self.place_range(range);
                self.direction = SelectionDirection::Forward;
            }
            self.drag = Some(Drag {
                granularity: Granularity::Word,
                origin: self.start..self.end,
            });
            self.changed(window, cx);
            return;
        }
        let Some((offset, upstream)) = geometry.hit(event.position) else {
            return;
        };
        match event.click_count {
            0 | 1 => {
                if event.modifiers.shift {
                    let anchor = self.anchor();
                    self.select_to(anchor, offset, upstream);
                } else {
                    self.place(offset, upstream);
                }
                let anchor = self.anchor();
                self.drag = Some(Drag {
                    granularity: Granularity::Character,
                    origin: anchor..anchor,
                });
            }
            2 => {
                let word = self.word_under(event.position);
                self.place_range(TextRange::new(word.start, word.end));
                self.drag = Some(Drag {
                    granularity: Granularity::Word,
                    origin: word,
                });
            }
            _ => {
                let line = self.line_under(event.position);
                self.place_range(TextRange::new(line.start, line.end));
                self.drag = Some(Drag {
                    granularity: Granularity::Line,
                    origin: line,
                });
            }
        }
        self.changed(window, cx);
    }

    /// Extends the selection a drag makes to a window position.
    pub(crate) fn drag_to(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.clone() else {
            return;
        };
        let Some(geometry) = self.geometry.clone() else {
            return;
        };
        let Some((offset, upstream)) = geometry.hit(position) else {
            return;
        };
        match drag.granularity {
            Granularity::Character => self.select_to(drag.origin.start, offset, upstream),
            Granularity::Word | Granularity::Line => {
                let unit = if drag.granularity == Granularity::Word {
                    self.word_under(position)
                } else {
                    self.line_under(position)
                };
                if unit.start < drag.origin.start {
                    self.select_to(drag.origin.end, unit.start, false);
                } else {
                    self.select_to(drag.origin.start, unit.end.max(drag.origin.end), false);
                }
            }
        }
        self.changed(window, cx);
    }

    pub(crate) fn end_drag(&mut self) {
        self.drag = None;
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    // The word, spaces, punctuation or token under a position.
    fn word_under(&self, position: Point<Pixels>) -> Range<usize> {
        let Some(geometry) = &self.geometry else {
            return 0..0;
        };
        let text = offset_text(&self.committed);
        let offset = geometry.char_under(position);
        word_at(&text, offset)
    }

    // The paragraph under a position.
    fn line_under(&self, position: Point<Pixels>) -> Range<usize> {
        let text = offset_text(&self.committed);
        let Some((offset, _)) = self.geometry.as_ref().and_then(|geometry| geometry.hit(position)) else {
            return 0..text.len();
        };
        paragraph_start(&text, offset)..paragraph_end(&text, offset)
    }

    // The wheel scrolls text taller than the editor.
    pub(crate) fn scroll_wheel(&mut self, event: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(geometry) = &self.geometry else {
            return;
        };
        let layout = &geometry.layout;
        let delta = event.delta.pixel_delta(layout.line_height);
        let max = (layout.height - geometry.bounds.size.height).max(px(0.));
        let mut scroll = self.scroll;
        scroll.y = (scroll.y - delta.y).clamp(px(0.), max);
        if scroll != self.scroll {
            self.scroll = scroll;
            cx.stop_propagation();
            cx.notify();
        }
    }

    // Assistive technology typing over the selection.
    pub(crate) fn a11y_replace(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_composition(window, cx);
        if !self.is_editable() {
            return;
        }
        let range = self.selection();
        self.insert_text(&clean(&text), range, INSERT_TEXT, Some(InputEvent::Accessibility), window, cx);
    }

    // Assistive technology setting the whole text.
    pub(crate) fn a11y_set_value(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_composition(window, cx);
        if !self.is_editable() {
            return;
        }
        let range = TextRange::new(0, value_length(&self.committed));
        self.insert_text(&clean(&text), range, INSERT_REPLACEMENT_TEXT, Some(InputEvent::Accessibility), window, cx);
    }

    // Assistive technology moving the selection.
    pub(crate) fn a11y_select(&mut self, data: Option<&gpui::accesskit::ActionData>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gpui::accesskit::ActionData::SetTextSelection(selection)) = data else {
            return;
        };
        let (anchor, focus) = {
            let snapshot = self.a11y.borrow();
            match (snapshot.offset_of(&selection.anchor), snapshot.offset_of(&selection.focus)) {
                (Some(anchor), Some(focus)) => (anchor, focus),
                _ => return,
            }
        };
        self.select_to(anchor, focus, false);
        self.changed(window, cx);
    }
}

impl EntityInputHandler for TokenInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.shown_text();
        let range = utf16_range(&text, &range_utf16);
        let range = floor_char(&text, range.start)..floor_char(&text, range.end).max(floor_char(&text, range.start));
        adjusted_range.replace(to_utf16(&text, &range));
        Some(text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let text = self.shown_text();
        Some(UTF16Selection {
            range: to_utf16(&text, &self.shown_selection()),
            reversed: self.composition.is_none() && self.direction == SelectionDirection::Backward,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        let composition = self.composition.as_ref()?;
        let shown = crate::token_input_view::Shown::of(&self.committed, Some(composition));
        let marked = shown.marked.clone()?;
        Some(to_utf16(&shown.text, &marked))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_composition(window, cx);
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        self.paste_item(&item, None, window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.platform_insert(range_utf16, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.platform_compose(range_utf16, new_text, new_selected_range, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let text = self.shown_text();
        let range = utf16_range(&text, &range_utf16);
        self.platform_bounds(range)
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        let text = self.shown_text();
        let range = utf16_range(&text, &range_utf16);
        let Some(geometry) = &self.geometry else {
            return Vec::new();
        };
        geometry
            .layout
            .range_rects(range)
            .into_iter()
            .map(|rect| Bounds::new(geometry.window(rect.origin), rect.size))
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let geometry = self.geometry.as_ref()?;
        let (offset, _) = geometry.hit_shown(point)?;
        Some(offset_to_utf16(&geometry.shown.text, offset))
    }

    fn set_selected_text_range(&mut self, range_utf16: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.composition.is_some() {
            return;
        }
        let text = offset_text(&self.committed);
        let range = utf16_range(&text, &range_utf16);
        self.select_range(TextRange::new(range.start, range.end), window, cx);
    }

    fn text_length_utf16(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        let text = self.shown_text();
        Some(offset_to_utf16(&text, text.len()))
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        self.is_editable()
    }

    fn text_input_configuration(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> TextInputConfiguration {
        self.settings.configuration.clone()
    }
}
