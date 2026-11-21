//! Port of `foundation/kernel/undo-stack.ts`: an undo stack whose entries
//! group, so typing runs into one entry until a pause, a new word, or an edit
//! of another kind. Framework-free and pure apart from the clock, which
//! callers can pass in.

use std::time::{Duration, Instant};

use gpui::SharedString;

/// How an undo stack keeps entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UndoStackOptions {
    /// How many entries are kept. 100 by default.
    pub limit: usize,
    /// The pause after which typing starts a new entry. 500 milliseconds by default.
    pub group_delay: Duration,
}

impl Default for UndoStackOptions {
    fn default() -> Self {
        Self {
            limit: 100,
            group_delay: Duration::from_millis(500),
        }
    }
}

impl UndoStackOptions {
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    pub fn group_delay(mut self, delay: Duration) -> Self {
        self.group_delay = delay;
        self
    }
}

/// An undo stack for any state: a board's columns, a drawing, a form. Record
/// the state after each edit; `undo` and `redo` return the state to show.
/// Edits of one group within `group_delay` join one entry, like a run of
/// typing or a drag's steps.
///
/// ```ignore
/// let mut stack = create_undo_stack(lists.clone(), UndoStackOptions::default());
/// stack.record(next.clone(), None); // after each change
/// if let Some(state) = stack.undo() { show(state.clone()) }
/// ```
#[derive(Clone, Debug)]
pub struct UndoStack<S> {
    entries: Vec<S>,
    index: usize,
    // The kind and time of the last edit that can take more into its entry.
    open: Option<(SharedString, Instant)>,
    options: UndoStackOptions,
}

/// An undo stack that starts at `initial` (`createUndoStack`).
pub fn create_undo_stack<S>(initial: S, options: UndoStackOptions) -> UndoStack<S> {
    UndoStack {
        entries: vec![initial],
        index: 0,
        open: None,
        options,
    }
}

impl<S> UndoStack<S> {
    /// Options set after creation, which apply from the next edit.
    pub fn set_options(&mut self, options: UndoStackOptions) {
        self.options = options;
    }

    /// Records the state after an edit, now. Edits of the same `group`, like
    /// "insert" or "delete", join the last entry while they come within
    /// `group_delay` of each other. Without a group, the edit is its own
    /// entry.
    pub fn record(&mut self, state: S, group: Option<&str>) {
        self.record_at(state, group, Instant::now());
    }

    /// `record` at `time`, for callers that pass the clock in.
    pub fn record_at(&mut self, state: S, group: Option<&str>, time: Instant) {
        let UndoStackOptions { limit, group_delay } = self.options;
        self.entries.truncate(self.index + 1);
        let joins = match (&self.open, group) {
            (Some((open, at)), Some(group)) => {
                open.as_ref() == group && time.saturating_duration_since(*at) < group_delay
            }
            _ => false,
        };
        if joins {
            self.entries[self.index] = state;
        } else {
            self.entries.push(state);
            if self.entries.len() > limit + 1 {
                self.entries.remove(0);
            }
            self.index = self.entries.len() - 1;
        }
        self.open = group.map(|group| (SharedString::from(group.to_string()), time));
    }

    /// Ends the open group, so the next edit starts an entry, like after a caret jump.
    pub fn seal(&mut self) {
        self.open = None;
    }

    /// Steps back, and returns the state to show, or `None` at the first entry.
    pub fn undo(&mut self) -> Option<&S> {
        self.open = None;
        if self.index == 0 {
            return None;
        }
        self.index -= 1;
        self.entries.get(self.index)
    }

    /// Steps forward again, and returns the state to show, or `None` at the last entry.
    pub fn redo(&mut self) -> Option<&S> {
        self.open = None;
        if self.index + 1 >= self.entries.len() {
            return None;
        }
        self.index += 1;
        self.entries.get(self.index)
    }

    /// Whether there's an entry to step back to.
    pub fn can_undo(&self) -> bool {
        self.index > 0
    }

    /// Whether there's an entry to step forward to.
    pub fn can_redo(&self) -> bool {
        self.index + 1 < self.entries.len()
    }

    /// Rewrites every entry, like a token's status changing everywhere
    /// without an entry of its own.
    pub fn map(&mut self, update: impl FnMut(S) -> S) {
        self.entries = std::mem::take(&mut self.entries).into_iter().map(update).collect();
    }

    /// Starts over from `state`, like after a controlled value from outside.
    pub fn reset(&mut self, state: S) {
        self.entries = vec![state];
        self.index = 0;
        self.open = None;
    }

    /// The state shown now.
    pub fn current(&self) -> &S {
        &self.entries[self.index]
    }
}
