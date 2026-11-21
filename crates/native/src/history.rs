//! A field's undo history, as the platform keeps one: typing runs together
//! into one step until the caret moves or another kind of edit comes, and so
//! do runs of Backspace or Delete. Paste, cut, an input method's commit and
//! a program's edit are each their own step. Writing the value from code
//! clears it, as writing `value` does.

use crate::events::SelectionDirection;

/// A selection to put back: its start, end and direction.
pub(crate) type Selection = (usize, usize, SelectionDirection);

/// Which edits run together into one step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {
    Typing,
    DeleteBackward,
    DeleteForward,
    /// Its own step.
    Single,
}

// One replacement: at `at`, `removed` gave way to `inserted`.
struct Delta {
    at: usize,
    removed: String,
    inserted: String,
}

struct Step {
    deltas: Vec<Delta>,
    before: Selection,
    after: Selection,
    group: Group,
}

const LIMIT: usize = 500;

#[derive(Default)]
pub(crate) struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    // Whether the last step may take in the next edit.
    open: bool,
}

impl History {
    /// Records an edit: at `at`, `removed` replaced by `inserted`.
    pub fn record(
        &mut self,
        at: usize,
        removed: String,
        inserted: String,
        before: Selection,
        after: Selection,
        group: Group,
    ) {
        self.redo.clear();
        if self.open
            && let Some(step) = self.undo.last_mut()
            && step.group == group
            && let Some(last) = step.deltas.last()
            && continues(last, at, &removed, &inserted, group)
        {
            step.deltas.push(Delta {
                at,
                removed,
                inserted,
            });
            step.after = after;
            return;
        }
        self.undo.push(Step {
            deltas: vec![Delta {
                at,
                removed,
                inserted,
            }],
            before,
            after,
            group,
        });
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.open = group != Group::Single;
    }

    /// Ends the step typing or deleting adds to, like when the caret moves.
    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The value and selection undoing the last step leaves, without taking it.
    pub fn peek_undo(&self, value: &str) -> Option<(String, Selection)> {
        let step = self.undo.last()?;
        let mut text = value.to_string();
        for delta in step.deltas.iter().rev() {
            let end = delta.at + delta.inserted.len();
            if end > text.len() || !text.is_char_boundary(delta.at) || !text.is_char_boundary(end)
            {
                return None;
            }
            text.replace_range(delta.at..end, &delta.removed);
        }
        Some((text, step.before))
    }

    /// The value and selection redoing the last undone step leaves.
    pub fn peek_redo(&self, value: &str) -> Option<(String, Selection)> {
        let step = self.redo.last()?;
        let mut text = value.to_string();
        for delta in &step.deltas {
            let end = delta.at + delta.removed.len();
            if end > text.len() || !text.is_char_boundary(delta.at) || !text.is_char_boundary(end)
            {
                return None;
            }
            text.replace_range(delta.at..end, &delta.inserted);
        }
        Some((text, step.after))
    }

    /// Moves the last step to the redo stack, once its undo applied.
    pub fn undone(&mut self) {
        if let Some(step) = self.undo.pop() {
            self.redo.push(step);
        }
        self.open = false;
    }

    /// Moves the last undone step back, once its redo applied.
    pub fn redone(&mut self) {
        if let Some(step) = self.redo.pop() {
            self.undo.push(step);
        }
        self.open = false;
    }
}

// Whether an edit continues the last one of its step: typing on from where
// it left the caret, or deleting on from where the last deletion was.
fn continues(last: &Delta, at: usize, removed: &str, inserted: &str, group: Group) -> bool {
    match group {
        Group::Typing => removed.is_empty() && at == last.at + last.inserted.len(),
        Group::DeleteBackward => inserted.is_empty() && at + removed.len() == last.at,
        Group::DeleteForward => inserted.is_empty() && at == last.at,
        Group::Single => false,
    }
}
