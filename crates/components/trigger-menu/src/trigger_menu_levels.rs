//! Port of `components/trigger-menu/trigger-menu-levels.ts`: the trigger
//! menu's match and its levels: the trigger's own, the arguments of a chosen
//! item, and a level that continues after an inserted item. It follows the
//! editor's edits and caret, remembers dismissed matches, and decides when a
//! match starts, changes or ends. Opening, items and announcements belong to
//! the state module, which it calls back (`apply_level`, `close_level`).
//!
//! The dismissed matches live beside the editor's listener, which moves them
//! along with every edit, the menu's own too: those come while the menu
//! writes, inside its update.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, SharedString, Window};
use ui_sdk_kernel::{InputEvent, Reason};
use ui_sdk_text::{EditorChange, TextRange};

use crate::trigger_match::{
    DismissedMatch, FoundMatch, TriggerMatch, find_trigger_match, is_valid_query, look_behind, map_dismissed,
};
use crate::trigger_menu::{
    TriggerArgument, TriggerArguments, TriggerMenuOpenChangeReason, TriggerMenuQueryChangeReason,
};
use crate::trigger_menu_api::TriggerMenu;
use crate::trigger_menu_select::ChosenItem;

/// One level of a match.
#[derive(Clone)]
pub(crate) struct TriggerLevel {
    // Which trigger matched.
    pub rule: usize,
    pub r#match: TriggerMatch,
    // Where the current level's query starts.
    pub query_start: usize,
    // The item whose arguments are being typed, and where each one started.
    pub item: Option<Rc<ChosenItem>>,
    pub starts: Vec<usize>,
    pub values: TriggerArguments,
    // A level after `TriggerAfter::Continue`, which has no trigger text of
    // its own.
    pub continued: bool,
}

impl TriggerLevel {
    // The level with the caret at `end`.
    pub fn ending_at(&self, end: usize) -> Self {
        let mut level = self.clone();
        level.r#match.end = end;
        level
    }
}

/// The argument a level asks for, if it's an argument level (`argumentOf`).
pub(crate) fn argument_of(level: Option<&TriggerLevel>) -> Option<&TriggerArgument> {
    let level = level?;
    let index = level.starts.len().checked_sub(1)?;
    level.item.as_ref()?.item.extra.arguments.get(index)
}

/// The level for a match the trigger itself makes (`triggerLevel`).
pub(crate) fn trigger_level(found: FoundMatch) -> TriggerLevel {
    TriggerLevel {
        rule: found.rule,
        query_start: found.r#match.query_start(),
        r#match: found.r#match,
        item: None,
        starts: Vec::new(),
        values: TriggerArguments::default(),
        continued: false,
    }
}

/// The match levels' state: the current level and the dismissed matches.
pub(crate) struct TriggerLevels {
    pub current: Option<TriggerLevel>,
    pub dismissed: Rc<RefCell<Vec<DismissedMatch>>>,
}

impl Default for TriggerLevels {
    fn default() -> Self {
        Self {
            current: None,
            dismissed: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

/// Moves the dismissed matches along with an edit: what the editor's
/// listener does first, for every edit.
pub(crate) fn move_dismissed(dismissed: &RefCell<Vec<DismissedMatch>>, range: TextRange, inserted: usize) {
    let mut dismissed = dismissed.borrow_mut();
    *dismissed = dismissed
        .iter()
        .filter_map(|entry| map_dismissed(entry, range, inserted))
        .collect();
}

impl TriggerMenu {
    /// The caret, or `None` while the editor is unfocused or has a selection.
    pub(crate) fn caret(&self, window: &Window, cx: &gpui::App) -> Option<usize> {
        let selection = self.editor()?.selection(window, cx)?;
        (selection.start == selection.end).then_some(selection.end)
    }

    /// The text between `start` and the caret.
    pub(crate) fn text_from(&self, start: usize, window: &Window, cx: &gpui::App) -> Option<String> {
        let at = self.caret(window, cx)?;
        if at < start {
            return None;
        }
        // At least as many characters as the bytes hold.
        let before = self.editor()?.text_before(at - start, window, cx)?;
        let skip = start.checked_sub(before.offset)?;
        before.text.get(skip..).map(str::to_string)
    }

    /// The match that ends at the caret, if any.
    pub(crate) fn match_at_caret(&self, window: &Window, cx: &gpui::App) -> Option<FoundMatch> {
        self.caret(window, cx)?;
        let rules = self.rules();
        let before = self.editor()?.text_before(look_behind(&rules), window, cx)?;
        find_trigger_match(&before.text, before.offset, &rules)
    }

    pub(crate) fn enter(
        &mut self,
        level: TriggerLevel,
        query: SharedString,
        reason: TriggerMenuQueryChangeReason,
        open: TriggerMenuOpenChangeReason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.levels.current = Some(level.clone());
        self.apply_level(level, query, reason, open, event, window, cx);
    }

    /// Ends the match. Escape also remembers it as dismissed, so it stays
    /// closed until its trigger is deleted or retyped.
    pub(crate) fn end(
        &mut self,
        reason: TriggerMenuOpenChangeReason,
        event: Option<InputEvent>,
        dismiss: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if dismiss && let Some(level) = self.levels.current.as_ref().filter(|level| !level.continued) {
            let at = self.caret(window, cx).unwrap_or(level.r#match.end);
            let entry = DismissedMatch {
                trigger: level.r#match.trigger.clone(),
                start: level.r#match.start,
                end: at.max(level.r#match.end),
            };
            self.levels.dismissed.borrow_mut().push(entry);
        }
        self.levels.current = None;
        self.close_level(reason, event, window, cx);
    }

    /// Opens the match at the caret, even a dismissed one, like for a
    /// shortcut or a controller.
    pub(crate) fn reopen(
        &mut self,
        reason: TriggerMenuOpenChangeReason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(found) = self.match_at_caret(window, cx) else {
            return false;
        };
        self.levels
            .dismissed
            .borrow_mut()
            .retain(|entry| entry.start != found.r#match.start);
        let query = found.r#match.query.clone();
        let query_reason = if reason == Reason::Shortcut {
            Reason::InputChange
        } else {
            Reason::Imperative
        };
        self.enter(trigger_level(found), query, query_reason, reason, event, window, cx);
        true
    }

    // An edit finds the match at the caret; the listener moved the dismissed
    // matches already. A match opens only when a person's edit touched it:
    // never on focus, a caret move, a program's edit or a collaborator's.
    fn on_input(&mut self, range: TextRange, inserted: &str, user: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_writing() || self.editor().is_some_and(|editor| editor.composing(cx)) {
            return;
        }
        if self
            .levels
            .current
            .as_ref()
            .is_some_and(|level| level.item.is_some() || level.continued)
        {
            return self.follow_level(user, window, cx);
        }
        let found = self.match_at_caret(window, cx);
        let dismissed = found.as_ref().is_some_and(|found| {
            self.levels
                .dismissed
                .borrow()
                .iter()
                .any(|entry| entry.start == found.r#match.start && entry.trigger == found.r#match.trigger)
        });
        let edit_end = range.start + inserted.len();
        let touched = found
            .as_ref()
            .is_some_and(|found| range.start <= found.r#match.end && edit_end >= found.r#match.start);
        let same = found.as_ref().is_some_and(|found| {
            self.levels.current.as_ref().is_some_and(|current| {
                current.r#match.start == found.r#match.start && current.r#match.trigger == found.r#match.trigger
            })
        });
        let found = match found {
            Some(found) if !dismissed && (same || (user && touched)) => found,
            _ => {
                if self.levels.current.is_some() {
                    self.end(Reason::InputChange, None, false, window, cx);
                }
                return;
            }
        };
        let query = found.r#match.query.clone();
        self.enter(
            trigger_level(found),
            query,
            Reason::InputChange,
            Reason::InputChange,
            None,
            window,
            cx,
        );
    }

    // An argument level or a continued level follows the caret from where it
    // started.
    fn follow_level(&mut self, user: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(level) = self.levels.current.clone() else {
            return;
        };
        let Some(at) = self.caret(window, cx) else {
            return self.end(Reason::InputChange, None, false, window, cx);
        };
        if at < level.query_start {
            return if user {
                self.step_back(window, cx)
            } else {
                self.end(Reason::InputChange, None, false, window, cx)
            };
        }
        let query = self.text_from(level.query_start, window, cx).unwrap_or_default();
        let invalid = level.continued
            && self
                .rules()
                .get(level.rule)
                .is_none_or(|rule| !is_valid_query(&query, rule));
        if query.contains('\n') || invalid {
            return self.end(Reason::InputChange, None, false, window, cx);
        }
        self.enter(
            level.ending_at(at),
            query.into(),
            Reason::InputChange,
            Reason::InputChange,
            None,
            window,
            cx,
        );
    }

    // Backspace past an argument's start goes back to the previous argument,
    // or to the item.
    fn step_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(level) = self.levels.current.clone() else {
            return;
        };
        if level.continued {
            return self.end(Reason::InputChange, None, false, window, cx);
        }
        if level.starts.len() > 1 {
            let mut back = level.clone();
            back.starts.pop();
            if let Some(argument) = back
                .item
                .as_ref()
                .and_then(|item| item.item.extra.arguments.get(back.starts.len() - 1))
            {
                back.values.remove(&argument.name);
            }
            back.query_start = *back.starts.last().expect("an argument level has a start");
            let query = self.text_from(back.query_start, window, cx).unwrap_or_default();
            return self.enter(
                back,
                query.into(),
                Reason::InputChange,
                Reason::InputChange,
                None,
                window,
                cx,
            );
        }
        self.levels.current = None;
        let Some(found) = self.match_at_caret(window, cx) else {
            return self.end(Reason::InputChange, None, false, window, cx);
        };
        let query = found.r#match.query.clone();
        self.enter(
            trigger_level(found),
            query,
            Reason::InputChange,
            Reason::InputChange,
            None,
            window,
            cx,
        );
    }

    // The caret left the match: closes with `CaretMove`. A caret still in
    // the same match stays, which also covers a caret reported before the
    // text.
    fn on_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.levels.current.as_ref() else {
            return;
        };
        if self.is_writing() {
            return;
        }
        let (query_start, start, follows) = (
            current.query_start,
            current.r#match.start,
            current.item.is_some() || current.continued,
        );
        match self.caret(window, cx) {
            Some(at) if at >= query_start => {}
            _ => return self.end(Reason::CaretMove, None, false, window, cx),
        }
        if follows {
            return;
        }
        if self
            .match_at_caret(window, cx)
            .is_none_or(|found| found.r#match.start != start)
        {
            self.end(Reason::CaretMove, None, false, window, cx);
        }
    }

    /// What the editor reported: an edit or a caret move.
    pub(crate) fn on_editor_change(&mut self, change: &EditorChange, window: &mut Window, cx: &mut Context<Self>) {
        match change {
            EditorChange::Input { inserted, range, user } => self.on_input(*range, inserted, *user, window, cx),
            EditorChange::Selection => self.on_selection(window, cx),
            EditorChange::CompositionStart | EditorChange::CompositionEnd => {}
        }
    }
}
