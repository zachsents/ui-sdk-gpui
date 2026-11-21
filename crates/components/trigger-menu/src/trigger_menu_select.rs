//! Port of `components/trigger-menu/trigger-menu-select.ts`: the trigger
//! menu's choosing: an item, its arguments one by one, then `on_select`, the
//! insertion and what the item does after.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use futures::FutureExt;
use futures::future::Shared;
use gpui::{App, Context, SharedString, Window};
use ui_sdk_kernel::{InputEvent, Reason, change_source, with_source};
use ui_sdk_lists::ListNode;
use ui_sdk_text::{
    EditorContent, EditorReplaceOptions, PendingToken, TextEditorAdapter, TextRange, Token, TokenTrigger,
    TokenUpdate,
};

use crate::trigger_match::TriggerMatch;
use crate::trigger_menu::{
    TriggerAfter, TriggerArguments, TriggerDefinition, TriggerItemExtra, TriggerMenuData, TriggerMenuDataItem,
    TriggerMenuSelectCallback, TriggerMenuSelectDetails,
};
use crate::trigger_menu_api::TriggerMenu;
use crate::trigger_menu_levels::{TriggerLevel, argument_of};

/// Writes to the editor as one step, without following it as a person's
/// edit: the editor's listener skips what it reports while `writing` is set.
#[derive(Clone)]
pub(crate) struct Writer {
    pub editor: Option<Rc<dyn TextEditorAdapter>>,
    pub writing: Rc<Cell<bool>>,
}

impl Writer {
    pub fn write(
        &self,
        range: TextRange,
        content: EditorContent,
        pending: Option<PendingToken>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(target) = &self.editor else {
            return;
        };
        struct Restore<'a>(&'a Cell<bool>, bool);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let _restore = Restore(&self.writing, self.writing.replace(true));
        target.replace(range, content, EditorReplaceOptions { pending }, window, cx);
    }
}

/// What a token a trigger menu inserts carries in its `data`: the trigger,
/// which its label leaves out, so a token turned back into text gets it
/// back, and the arguments typed for it. It's the text crate's
/// [`TokenTrigger`], so a token input reads it without knowing the menu.
pub type TriggerTokenData = TokenTrigger;

/// An item chosen: from the data, or a composed item's props, with the
/// `on_select` only a composed item has.
pub(crate) struct ChosenItem {
    pub item: TriggerMenuDataItem,
    pub on_select: Option<TriggerMenuSelectCallback>,
}

// An `on_select` future, shared between the pending count and the token it
// makes pending.
type SharedPending = Shared<PendingToken>;

/// Every data item by key, with what only the data knows, like its
/// arguments (`itemsByKey`).
pub(crate) fn items_by_key(data: Option<&TriggerMenuData>) -> HashMap<SharedString, TriggerMenuDataItem> {
    let mut map = HashMap::new();
    let Some(data) = data else {
        return map;
    };
    for node in data.nodes() {
        match node {
            ListNode::Item(item) => {
                map.insert(item.value.clone(), item.clone());
            }
            ListNode::Group(group) => {
                for item in &group.items {
                    map.insert(item.value.clone(), item.clone());
                }
            }
            ListNode::Separator(_) => {}
        }
    }
    map
}

/// A token where the editor takes them, else the trigger, the label and a
/// space (`defaultInsert`). A token's label is the item's alone; the trigger
/// rides in its data, so a token turned back into text gets it back.
pub(crate) fn default_insert(
    item: &TriggerMenuDataItem,
    level: &TriggerLevel,
    rule: Option<&TriggerDefinition>,
    tokens: bool,
) -> EditorContent {
    let values: Vec<&SharedString> = level.values.values().collect();
    let trigger: SharedString = if level.continued {
        SharedString::default()
    } else {
        level.r#match.trigger.clone()
    };
    if !tokens {
        let mut text = format!("{trigger}{}", item.label);
        for value in &values {
            text.push(' ');
            text.push_str(value);
        }
        text.push(' ');
        return EditorContent::Text(text.into());
    }
    let mut label = item.label.to_string();
    for value in &values {
        label.push(' ');
        label.push_str(value);
    }
    let kind = rule
        .and_then(|rule| rule.kind.clone())
        .unwrap_or_else(|| "mention".into());
    let data = TriggerTokenData {
        trigger,
        arguments: level
            .values
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    };
    EditorContent::Token(Token::new(kind, item.value.clone(), label).data(data))
}

impl TriggerMenu {
    /// Whether the menu is writing to the editor, so the edits it reports
    /// are its own.
    pub(crate) fn is_writing(&self) -> bool {
        self.writing.get()
    }

    pub(crate) fn writer(&self) -> Writer {
        Writer {
            editor: self.editor(),
            writing: self.writing.clone(),
        }
    }

    /// Writes to the editor as one step, without following it as a person's
    /// edit.
    pub(crate) fn write(
        &self,
        range: TextRange,
        content: impl Into<EditorContent>,
        pending: Option<PendingToken>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.writer().write(range, content.into(), pending, window, cx);
    }

    /// The item a key names: from the data, or a composed item's props.
    pub(crate) fn item_for(&self, key: &SharedString) -> Option<ChosenItem> {
        if let Some(item) = self.data_item(key) {
            return Some(ChosenItem { item, on_select: None });
        }
        let entry = self.listbox.find(self, Some(key))?;
        let on_select = self
            .item_props
            .borrow()
            .get(key)
            .and_then(|props| props.on_select.clone());
        let item = TriggerMenuDataItem {
            value: entry.value.unwrap_or_else(|| key.clone()),
            label: entry.label,
            keywords: entry.keywords,
            disabled: entry.disabled,
            extra: TriggerItemExtra::default(),
        };
        Some(ChosenItem { item, on_select })
    }

    // A data item of the current level by key, from a map kept while the
    // data stays the same.
    fn data_item(&self, key: &SharedString) -> Option<TriggerMenuDataItem> {
        let mut memo = self.by_key.borrow_mut();
        let stale = match (&*memo, &self.items) {
            (Some((data, _)), Some(items)) => !data.ptr_eq(items),
            (None, None) => false,
            _ => true,
        };
        if stale {
            *memo = self
                .items
                .as_ref()
                .map(|items| (items.clone(), items_by_key(Some(items))));
        }
        memo.as_ref().and_then(|(_, map)| map.get(key).cloned())
    }

    /// Choosing a highlighted item: an item with arguments asks for them
    /// first (`choose`).
    pub(crate) fn choose(
        &mut self,
        key: &SharedString,
        event: Option<InputEvent>,
        tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(level) = self.levels.current.clone() else {
            return;
        };
        if level.item.is_some() {
            return self.accept(event, tab, window, cx);
        }
        let Some(chosen) = self.item_for(key) else {
            return;
        };
        let Some(at) = self.caret(window, cx) else {
            return;
        };
        if chosen.item.disabled {
            return;
        }
        if chosen.item.extra.arguments.is_empty() {
            return self.complete(Rc::new(chosen), level, at, event, tab, window, cx);
        }
        let text = format!("{}{} ", level.r#match.trigger, chosen.item.label);
        self.write(
            TextRange::new(level.r#match.start, at),
            SharedString::from(text.clone()),
            None,
            window,
            cx,
        );
        let start = level.r#match.start + text.len();
        let mut next = level.ending_at(start);
        next.item = Some(Rc::new(chosen));
        next.starts = vec![start];
        next.values = TriggerArguments::default();
        next.query_start = start;
        self.enter(
            next,
            SharedString::default(),
            Reason::ItemPress,
            Reason::ItemPress,
            event,
            window,
            cx,
        );
    }

    /// Accepts the current argument: the highlighted suggestion, else the
    /// typed text (`accept`).
    pub(crate) fn accept(&mut self, event: Option<InputEvent>, tab: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(level) = self.levels.current.clone() else {
            return;
        };
        let Some(argument) = argument_of(Some(&level)).cloned() else {
            return;
        };
        let Some(at) = self.caret(window, cx) else {
            return;
        };
        let Some(item) = level.item.clone() else {
            return;
        };
        let typed = self.text_from(level.query_start, window, cx).unwrap_or_default();
        let query = SharedString::from(ui_sdk_text::js_trim(&typed).to_string());
        let chosen = self.highlighted_while_open().and_then(|key| self.item_for(&key));
        let value = match &chosen {
            Some(chosen) => chosen.item.value.clone(),
            None if argument.allow_custom_value => query.clone(),
            None => SharedString::default(),
        };
        if value.is_empty() && argument.required {
            return;
        }
        let mut values = level.values.clone();
        if !value.is_empty() {
            values.set(argument.name.clone(), value);
        }
        let label = match &chosen {
            Some(chosen) => chosen.item.label.clone(),
            None => query,
        };
        let index = level.starts.len() - 1;
        let rest = &item.item.extra.arguments[index + 1..];
        // Enter finishes once only optional arguments are left. Tab moves to
        // the next one.
        let finish = rest.is_empty() || (!tab && rest.iter().all(|next| !next.required));
        let text = if label.is_empty() {
            String::new()
        } else {
            format!("{label} ")
        };
        if !text.is_empty() {
            self.write(
                TextRange::new(level.query_start, at),
                SharedString::from(text.clone()),
                None,
                window,
                cx,
            );
        }
        let next = level.query_start + text.len();
        if finish {
            let mut level = level;
            level.values = values;
            return self.complete(item, level, next, event, tab, window, cx);
        }
        let mut level = level.ending_at(next);
        level.values = values;
        level.starts.push(next);
        level.query_start = next;
        self.enter(
            level,
            SharedString::default(),
            Reason::ItemPress,
            Reason::ItemPress,
            event,
            window,
            cx,
        );
    }

    /// Runs the item's and the menu's `on_select`, inserts, then does what
    /// `after` says (`complete`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn complete(
        &mut self,
        chosen: Rc<ChosenItem>,
        level: TriggerLevel,
        at: usize,
        event: Option<InputEvent>,
        tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rule = self.triggers.get(level.rule).cloned();
        let range = TextRange::new(level.r#match.start, at);
        let query = if level.continued {
            self.text_from(level.query_start, window, cx).unwrap_or_default().into()
        } else {
            level.r#match.query.clone()
        };
        let r#match = TriggerMatch {
            end: at,
            query,
            ..level.r#match.clone()
        };
        let source = change_source(Reason::ItemPress, event.as_ref());
        let details = TriggerMenuSelectDetails {
            reason: Reason::ItemPress,
            trigger: level.r#match.trigger.clone(),
            r#match: r#match.clone(),
            item: chosen.item.clone(),
            arguments: level.values.clone(),
            event: event.clone(),
            source,
            writer: self.writer(),
            range,
            prevented: Cell::new(false),
            inserted: Cell::new(false),
        };
        let value = chosen.item.value.clone();
        // The item hears first, then the menu. Either can prevent the
        // insertion or return a future.
        let callbacks = [chosen.on_select.clone(), self.on_select.clone()];
        let mut pending: Option<SharedPending> = None;
        let mut others = Vec::new();
        with_source(source, || {
            for callback in callbacks.into_iter().flatten() {
                if let Some(work) = callback(&value, &details, window, cx) {
                    match pending {
                        None => pending = Some(work.shared()),
                        Some(_) => others.push(work),
                    }
                }
            }
        });
        // A second future runs to its end too, as a promise does, though
        // only the first makes the token pending.
        for work in others {
            cx.spawn(async move |_, _| {
                work.await.ok();
            })
            .detach();
        }
        if let Some(pending) = &pending {
            self.waiting(pending.clone(), window, cx);
        }
        if !details.default_prevented() {
            let content = match rule.as_ref().and_then(|rule| rule.insert.clone()) {
                Some(insert) => insert(&chosen.item, &r#match, &level.values),
                None => default_insert(
                    &chosen.item,
                    &level,
                    rule.as_ref(),
                    self.editor().is_some_and(|editor| editor.tokens()),
                ),
            };
            let token_pending = match (&content, &pending) {
                (EditorContent::Token(_), Some(pending)) => Some(Box::pin(pending.clone()) as PendingToken),
                _ => None,
            };
            self.write(range, content, token_pending, window, cx);
        }
        if chosen.item.extra.after == TriggerAfter::Continue {
            let start = self.caret(window, cx).unwrap_or(at);
            let next = TriggerLevel {
                rule: level.rule,
                r#match: TriggerMatch {
                    trigger: level.r#match.trigger.clone(),
                    start,
                    end: start,
                    query: SharedString::default(),
                },
                query_start: start,
                item: None,
                starts: Vec::new(),
                values: TriggerArguments::default(),
                continued: true,
            };
            return self.enter(
                next,
                SharedString::default(),
                Reason::ItemPress,
                Reason::ItemPress,
                event,
                window,
                cx,
            );
        }
        self.end(Reason::ItemPress, event, false, window, cx);
        if chosen.item.extra.after == TriggerAfter::Submit && !tab {
            // Once the menu's update is over, since a submit reads and
            // writes fields.
            if let Some(editor) = self.editor() {
                window.defer(cx, move |window, cx| editor.submit(window, cx));
            }
        }
    }

    // An `on_select` future that hasn't settled: the menu is pending until it
    // does (`waiting`).
    fn waiting(&mut self, pending: SharedPending, window: &mut Window, cx: &mut Context<Self>) {
        self.pending += 1;
        self.pending_changed(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let _settled: Result<Option<TokenUpdate>, SharedString> = pending.await;
            this.update_in(cx, |menu, window, cx| {
                menu.pending = menu.pending.saturating_sub(1);
                menu.pending_changed(window, cx);
            })
            .ok();
        })
        .detach();
    }
}

// Keeps a memo of data by key beside the data it came from.
pub(crate) type ByKey = RefCell<Option<(TriggerMenuData, HashMap<SharedString, TriggerMenuDataItem>)>>;
