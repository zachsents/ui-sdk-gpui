//! How a field edits: every change to its text goes through `apply`, which
//! cleans what's inserted, asks `beforeinput`, writes, records the undo
//! step and fires `input`. The commands keys run, moves, deletions, the
//! clipboard and the history, build on it.

use std::cell::RefCell;
use std::ops::Range;

use gpui::{ClipboardItem, Context, SharedString, Window};
use ui_sdk_kernel::ChangeSource;
use unicode_segmentation::UnicodeSegmentation;

use crate::boundaries::{
    char_count, floor_char, next_grapheme, next_word_end, next_word_start, paragraph_end,
    paragraph_start, previous_grapheme, previous_word_start, text_diff,
};
use crate::events::{BeforeEdit, Edit, InputType, SelectionDirection};
use crate::history::{Group, Selection};
use crate::keys::{Command, Deletion, Movement};
use crate::style::TextFieldKind;
use crate::text_field::{TextField, parse_number};

thread_local! {
    // What Control-K last cut on a Mac, for Control-Y. The platform keeps
    // one for every field, so this does too.
    static KILL_BUFFER: RefCell<String> = const { RefCell::new(String::new()) };
}

/// One edit to apply.
pub(crate) struct Change {
    pub input_type: InputType,
    pub range: Range<usize>,
    pub text: String,
    pub source: ChangeSource,
    /// A person's edit: refused by a read-only field, cleaned by the filter
    /// and the maximum length, and asked about with `beforeinput`.
    pub user: bool,
    pub group: Group,
    /// The selection after it. Without one, a caret after the text.
    pub selection: Option<Selection>,
}

impl TextField {
    /// Applies an edit. Returns whether it changed the text.
    pub(crate) fn apply(
        &mut self,
        change: Change,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if change.user && !self.is_editable() {
            return false;
        }
        let range = self.clamp_range(change.range.clone());
        let data = self.clean(&change.text, change.user);
        // Typing nothing the field takes, like a letter in a number field,
        // leaves the selection where it was.
        if change.input_type.is_insert() && !change.text.is_empty() && data.is_empty() {
            return false;
        }
        if data.is_empty() && range.is_empty() {
            return false;
        }
        let text = if change.user {
            self.fit(&data, &range)
        } else {
            data.clone()
        };
        if change.user {
            // `beforeinput` hears all of it, as the browser's does: the
            // maximum length cuts it after.
            let before = BeforeEdit::new(change.input_type, range.clone(), data, false, true, change.source)
                .cut(text.clone());
            let revision = self.revision;
            if !self.before_input(&before, window, cx) || self.revision != revision {
                return false;
            }
            // With no room for any of it, the selection stays as it was.
            if before.has_no_room() {
                return false;
            }
        }
        self.write(range, text, change, window, cx);
        true
    }

    fn write(
        &mut self,
        range: Range<usize>,
        text: String,
        change: Change,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.selection_tuple();
        let removed = self.value[range.clone()].to_string();
        let mut next = String::with_capacity(self.value.len() - removed.len() + text.len());
        next.push_str(&self.value[..range.start]);
        next.push_str(&text);
        next.push_str(&self.value[range.end..]);
        self.replace_value(next.into());
        let caret = range.start + text.len();
        let (start, end, direction) =
            change
                .selection
                .unwrap_or((caret, caret, SelectionDirection::None));
        self.set_selection_quiet(start, end, direction);
        self.upstream = false;
        self.goal_x = None;
        self.history.record(
            range.start,
            removed,
            text.clone(),
            before,
            self.selection_tuple(),
            change.group,
        );
        self.dirty = true;
        if change.user {
            self.user_edited = true;
        }
        let edit = Edit {
            input_type: change.input_type,
            range,
            inserted: text,
            value: self.value.clone(),
            composing: false,
            user: change.user,
            source: change.source,
        };
        self.fire_input(edit, window, cx);
        self.changed(window, cx);
    }

    pub(crate) fn clamp_range(&self, range: Range<usize>) -> Range<usize> {
        let len = self.value.len();
        let end = floor_char(&self.value, range.end.min(len));
        let start = floor_char(&self.value, range.start.min(len)).min(end);
        start..end
    }

    // What an edit may put in: line breaks as the field holds them, no
    // control characters, and for a person's edit, what a number field and
    // the filter take. The maximum length is `fit`'s.
    pub(crate) fn clean(&self, text: &str, user: bool) -> String {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    if self.multi_line {
                        out.push('\n');
                    }
                }
                '\n' => {
                    if self.multi_line {
                        out.push('\n');
                    }
                }
                '\t' => out.push(c),
                c if c.is_control() => {}
                c => out.push(c),
            }
        }
        if !user {
            return out;
        }
        if self.kind == TextFieldKind::Number && !self.multi_line {
            out.retain(is_number_char);
        }
        if let Some(filter) = &self.filter {
            out.retain(|c| filter(c));
        }
        out
    }

    // What of a person's text fits the maximum length in place of `range`:
    // whole graphemes, as many characters as the rest of the value leaves
    // room for.
    pub(crate) fn fit(&self, text: &str, range: &Range<usize>) -> String {
        let mut out = text.to_string();
        if let Some(max) = self.max_length {
            let kept = char_count(&self.value) - char_count(&self.value[range.clone()]);
            let room = max.saturating_sub(kept);
            if char_count(&out) > room {
                let mut count = 0;
                let mut end = 0;
                for grapheme in out.graphemes(true) {
                    let chars = char_count(grapheme);
                    if count + chars > room {
                        break;
                    }
                    count += chars;
                    end += grapheme.len();
                }
                out.truncate(end);
            }
        }
        out
    }

    /// Runs what a key does.
    pub(crate) fn run(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let source = ChangeSource::Keyboard;
        match command {
            Command::Move(movement, extend) => self.move_caret(movement, extend, window, cx),
            Command::Delete(deletion) => self.delete(deletion, window, cx),
            Command::LineBreak => {
                self.insert(InputType::InsertLineBreak, "\n", Group::Typing, None, source, window, cx);
            }
            Command::OpenLine => {
                let selection = Some((self.start, self.start, SelectionDirection::None));
                self.insert(InputType::InsertLineBreak, "\n", Group::Single, selection, source, window, cx);
            }
            Command::Commit => self.enter(window, cx),
            Command::SelectAll => self.select_all(window, cx),
            Command::Copy => self.copy(cx),
            Command::Cut => self.cut(window, cx),
            Command::Paste => self.paste(window, cx),
            Command::Undo => {
                self.undo(window, cx);
            }
            Command::Redo => {
                self.redo(window, cx);
            }
            Command::Yank => {
                let text = KILL_BUFFER.with(|buffer| buffer.borrow().clone());
                if !text.is_empty() {
                    self.insert(InputType::InsertFromYank, &text, Group::Single, None, source, window, cx);
                }
            }
            Command::Transpose => self.transpose(window, cx),
            Command::Step(by) => self.step_number(by, window, cx),
            Command::CharacterPalette => window.show_character_palette(),
        }
    }

    // A person's insertion over the selection.
    #[allow(clippy::too_many_arguments)]
    fn insert(
        &mut self,
        input_type: InputType,
        text: &str,
        group: Group,
        selection: Option<Selection>,
        source: ChangeSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.apply(
            Change {
                input_type,
                range: self.start..self.end,
                text: text.to_string(),
                source,
                user: true,
                group,
                selection,
            },
            window,
            cx,
        )
    }

    // Enter in a single-line field inserts nothing, but tells `beforeinput`,
    // then commits what was typed.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_editable() {
            let before = BeforeEdit::new(
                InputType::InsertLineBreak,
                self.start..self.end,
                String::new(),
                false,
                true,
                ChangeSource::Keyboard,
            );
            self.before_input(&before, window, cx);
        }
        self.commit(window, cx);
    }

    /// Selects all the text, as Command-A or Control-A does.
    pub fn select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.value.len();
        self.set_selection_quiet(0, len, SelectionDirection::None);
        self.upstream = false;
        self.goal_x = None;
        self.history.close();
        self.changed(window, cx);
    }

    /// Copies the selection. A password never copies.
    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if self.start == self.end || self.kind == TextFieldKind::Password {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text().to_string()));
    }

    /// Cuts the selection, as a person would. A password never cuts.
    pub fn cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.start == self.end || self.kind == TextFieldKind::Password || !self.is_editable() {
            return;
        }
        self.copy(cx);
        self.apply(
            Change {
                input_type: InputType::DeleteByCut,
                range: self.start..self.end,
                text: String::new(),
                source: ChangeSource::Keyboard,
                user: true,
                group: Group::Single,
                selection: None,
            },
            window,
            cx,
        );
    }

    /// Pastes the clipboard's text over the selection, as a person would.
    pub fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.paste_text(&text, ChangeSource::Keyboard, window, cx);
    }

    pub(crate) fn paste_text(
        &mut self,
        text: &str,
        source: ChangeSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_composition(window, cx);
        self.insert(InputType::InsertFromPaste, text, Group::Single, None, source, window, cx);
    }

    /// Undoes the last step, as a person would. Returns whether it did.
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.history_step(false, window, cx)
    }

    /// Redoes the last undone step. Returns whether it did.
    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.history_step(true, window, cx)
    }

    fn history_step(&mut self, redo: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.is_editable() {
            return false;
        }
        self.commit_composition(window, cx);
        let step = if redo {
            self.history.peek_redo(&self.value)
        } else {
            self.history.peek_undo(&self.value)
        };
        let Some((next, (start, end, direction))) = step else {
            return false;
        };
        let input_type = if redo {
            InputType::HistoryRedo
        } else {
            InputType::HistoryUndo
        };
        let (range, inserted) = text_diff(&self.value, &next);
        let before = BeforeEdit::new(
            input_type,
            range.clone(),
            inserted.clone(),
            false,
            true,
            ChangeSource::Keyboard,
        );
        let revision = self.revision;
        if !self.before_input(&before, window, cx) || self.revision != revision {
            return false;
        }
        if redo {
            self.history.redone();
        } else {
            self.history.undone();
        }
        self.replace_value(SharedString::from(next));
        self.set_selection_quiet(start, end, direction);
        self.upstream = false;
        self.goal_x = None;
        self.dirty = true;
        self.user_edited = true;
        let edit = Edit {
            input_type,
            range,
            inserted,
            value: self.value.clone(),
            composing: false,
            user: true,
            source: ChangeSource::Keyboard,
        };
        self.fire_input(edit, window, cx);
        self.changed(window, cx);
        true
    }

    fn move_caret(
        &mut self,
        movement: Movement,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            self.history.close();
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
        self.history.close();
        self.changed(window, cx);
    }

    pub(crate) fn place(&mut self, offset: usize, upstream: bool) {
        self.set_selection_quiet(offset, offset, SelectionDirection::None);
        self.upstream = upstream;
    }

    pub(crate) fn select_to(&mut self, anchor: usize, head: usize, upstream: bool) {
        if head < anchor {
            self.set_selection_quiet(head, anchor, SelectionDirection::Backward);
        } else if head > anchor {
            self.set_selection_quiet(anchor, head, SelectionDirection::Forward);
        } else {
            self.set_selection_quiet(head, head, SelectionDirection::None);
        }
        self.upstream = upstream;
    }

    // Where a move from `from` lands, and whether at the end of a wrapped row.
    fn target(&mut self, movement: Movement, from: usize, upstream: bool) -> (usize, bool) {
        use Movement::*;
        let value = self.value.clone();
        let len = value.len();
        match movement {
            Left => (previous_grapheme(&value, from), false),
            Right => (next_grapheme(&value, from), false),
            WordLeft => (previous_word_start(&value, from), false),
            WordRight => (next_word_end(&value, from), false),
            NextWordStart => (next_word_start(&value, from), false),
            ParagraphStart => (paragraph_start(&value, from), false),
            ParagraphEnd => (paragraph_end(&value, from), false),
            ParagraphBackward => {
                let start = paragraph_start(&value, from);
                if start == from && from > 0 {
                    (paragraph_start(&value, from - 1), false)
                } else {
                    (start, false)
                }
            }
            ParagraphForward => {
                let end = paragraph_end(&value, from);
                if end == from && from < len {
                    (paragraph_end(&value, from + 1), false)
                } else {
                    (end, false)
                }
            }
            Start => (0, false),
            End => (len, false),
            LineStart => (self.line_range(from, upstream).start, false),
            LineEnd => {
                let range = self.line_range(from, upstream);
                let wraps = self.row_wraps(from, upstream);
                (range.end, wraps)
            }
            Up | Down | PageUp | PageDown => self.vertical(movement, from, upstream),
        }
    }

    // The visual line an offset is on, in value offsets: its row in the
    // last layout, or its paragraph before there is one.
    fn line_range(&self, offset: usize, upstream: bool) -> Range<usize> {
        let Some(geometry) = &self.geometry else {
            return paragraph_start(&self.value, offset)..paragraph_end(&self.value, offset);
        };
        let shown = geometry.display.to_display(offset);
        let row = geometry.layout.row_for(shown, upstream);
        let range = geometry.layout.row_range(row);
        let start = floor_char(&self.value, geometry.display.to_value(range.start));
        let end = floor_char(&self.value, geometry.display.to_value(range.end));
        start..end.max(start)
    }

    fn row_wraps(&self, offset: usize, upstream: bool) -> bool {
        let Some(geometry) = &self.geometry else {
            return false;
        };
        let shown = geometry.display.to_display(offset);
        let row = geometry.layout.row_for(shown, upstream);
        geometry.layout.rows[row].wraps
    }

    // Up and Down: a row at a time, or a page, toward where the caret aimed.
    // Past the first row the caret goes to the start, past the last to the
    // end, which in a single-line field is all they do.
    fn vertical(&mut self, movement: Movement, from: usize, upstream: bool) -> (usize, bool) {
        let up = matches!(movement, Movement::Up | Movement::PageUp);
        let edge = if up { 0 } else { self.value.len() };
        if !self.multi_line {
            return (edge, false);
        }
        let Some(geometry) = self.geometry.clone() else {
            return (edge, false);
        };
        let layout = &geometry.layout;
        let shown = geometry.display.to_display(from);
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
        (floor_char(&self.value, geometry.display.to_value(offset)), wraps)
    }

    fn delete(&mut self, deletion: Deletion, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_editable() {
            return;
        }
        let input_type = match deletion {
            Deletion::CharBackward => InputType::DeleteContentBackward,
            Deletion::CharForward => InputType::DeleteContentForward,
            Deletion::WordBackward => InputType::DeleteWordBackward,
            Deletion::WordForward | Deletion::WordForwardToStart => InputType::DeleteWordForward,
            Deletion::LineBackward => InputType::DeleteSoftLineBackward,
            Deletion::LineForward => InputType::DeleteSoftLineForward,
            Deletion::ParagraphForward => InputType::DeleteHardLineForward,
        };
        let group = if input_type.is_backward() {
            Group::DeleteBackward
        } else {
            Group::DeleteForward
        };
        let range = if self.start != self.end {
            self.start..self.end
        } else {
            let at = self.end;
            let value = self.value.clone();
            match deletion {
                Deletion::CharBackward => previous_grapheme(&value, at)..at,
                Deletion::CharForward => at..next_grapheme(&value, at),
                Deletion::WordBackward => previous_word_start(&value, at)..at,
                Deletion::WordForward => at..next_word_end(&value, at),
                Deletion::WordForwardToStart => at..next_word_start(&value, at),
                Deletion::LineBackward => self.line_range(at, self.upstream).start.min(at)..at,
                Deletion::LineForward => at..self.line_range(at, self.upstream).end.max(at),
                Deletion::ParagraphForward => {
                    let end = paragraph_end(&value, at);
                    if end == at && at < value.len() {
                        at..at + 1
                    } else {
                        at..end
                    }
                }
            }
        };
        if range.is_empty() {
            return;
        }
        if deletion == Deletion::ParagraphForward {
            let killed = self.value[range.clone()].to_string();
            KILL_BUFFER.with(|buffer| *buffer.borrow_mut() = killed);
        }
        self.apply(
            Change {
                input_type,
                range,
                text: String::new(),
                source: ChangeSource::Keyboard,
                user: true,
                group,
                selection: None,
            },
            window,
            cx,
        );
    }

    // Control-T: swaps the characters around the caret, or the two before
    // it at the end of a line, and moves past them.
    fn transpose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.start != self.end || !self.is_editable() {
            return;
        }
        let value = self.value.clone();
        let at = self.end;
        if at == 0 {
            return;
        }
        let at_line_end = at == value.len() || value[at..].starts_with('\n');
        let (first, middle, last) = if at_line_end {
            let middle = previous_grapheme(&value, at);
            if middle == 0 {
                return;
            }
            (previous_grapheme(&value, middle), middle, at)
        } else {
            (previous_grapheme(&value, at), at, next_grapheme(&value, at))
        };
        if value[first..middle].contains('\n') || value[middle..last].contains('\n') {
            return;
        }
        let swapped = format!("{}{}", &value[middle..last], &value[first..middle]);
        self.apply(
            Change {
                input_type: InputType::InsertTranspose,
                range: first..last,
                text: swapped,
                source: ChangeSource::Keyboard,
                user: true,
                group: Group::Single,
                selection: Some((last, last, SelectionDirection::None)),
            },
            window,
            cx,
        );
    }

    // A number field's arrows: a step up or down from the value, on the
    // step's grid, within the bounds. Like the browser, it fires `input`
    // and `change` together.
    fn step_number(&mut self, by: i8, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_editable() {
            return;
        }
        let step = self.step.filter(|step| *step > 0. && step.is_finite()).unwrap_or(1.);
        let min = self.min.filter(|min| min.is_finite());
        let max = self.max.filter(|max| max.is_finite() && min.is_none_or(|min| *max >= min));
        let clamp = |value: f64| {
            let value = min.map_or(value, |min| value.max(min));
            max.map_or(value, |max| value.min(max))
        };
        let base = min.unwrap_or(0.);
        let next = match parse_number(&self.value) {
            Some(current) => {
                let steps = ((current - base) / step).round();
                let on_grid = base + steps * step;
                let next = if (on_grid - current).abs() > step * 1e-9 {
                    // Off the grid, a step goes to the grid point in its direction.
                    if by > 0 {
                        base + ((current - base) / step).ceil() * step
                    } else {
                        base + ((current - base) / step).floor() * step
                    }
                } else {
                    on_grid + step * by as f64
                };
                clamp(next)
            }
            None => clamp(0.),
        };
        let decimals = decimals_of(step).max(decimals_of(base));
        let mut text = format!("{next:.decimals$}");
        if text.starts_with('-') && text[1..].chars().all(|c| c == '0' || c == '.') {
            text.remove(0);
        }
        if text == self.value.as_ref() {
            return;
        }
        let len = self.value.len();
        let applied = self.apply(
            Change {
                input_type: InputType::InsertReplacementText,
                range: 0..len,
                text,
                source: ChangeSource::Keyboard,
                user: true,
                group: Group::Single,
                selection: None,
            },
            window,
            cx,
        );
        if applied {
            self.commit(window, cx);
        }
    }
}

// The characters a number field takes, as `type="number"` does.
fn is_number_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')
}

// How many decimals a number needs to be written exactly, as a step's
// precision decides a number field's.
fn decimals_of(value: f64) -> usize {
    let text = format!("{value}");
    match text.split_once('.') {
        Some((_, decimals)) => decimals.len().min(12),
        None => 0,
    }
}
