//! Port of `components/command/command-api.ts`: Command is Autocomplete's
//! core rendered inline: always open, with no popup, where the search text is
//! the `query` and the highlight is the `value`. On top it runs commands,
//! with a pending state and shortcuts, and keeps a stack of pages, each with
//! the search text typed on it.

use std::rc::Rc;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use gpui::{App, Context, KeyDownEvent, SharedString, Window};
use ui_sdk_kernel::{
    ActionOutcome, ChangeCallback, ChangeOptions, InputEvent, Key, Reason, SelectEvent, is_composing, log, press_of,
};
use ui_sdk_lists::{HighlightCause, HighlightOptions, SearchFilter};

use crate::command::{Command, CommandActionDetails, CommandItemProps, CommandValueChange, CommandValueChangeDetails};
use crate::command_data::find_command;

// `on_value_change` with where the command is.
fn value_callback(callback: Option<CommandValueChange>, index: Option<usize>) -> Option<ChangeCallback<Option<SharedString>>> {
    let callback = callback?;
    Some(Rc::new(
        move |value: &Option<SharedString>, details, window: &mut Window, cx: &mut App| {
            callback(value, &CommandValueChangeDetails::new(details, index), window, cx)
        },
    ))
}

impl Command {
    // The command's index among the visible commands, `None` for none.
    fn index_of(&self, key: Option<&SharedString>) -> Option<usize> {
        let key = key?;
        self.core
            .listbox
            .entries(self)
            .iter()
            .position(|entry| &entry.key == key)
    }

    /// What a command part rendered with last, while it's in the page
    /// (`itemProps`).
    pub(crate) fn props_of(&self, key: &str) -> Option<CommandItemProps> {
        self.core.listbox.item_meta(key)?;
        self.item_props.borrow().get(key).cloned()
    }

    // The highlight is the value, through `on_value_change`, so a controlled
    // one waits for it to come back. A highlight the component picks with
    // nothing before it is a default, not a change.
    pub(crate) fn highlight(
        &mut self,
        key: Option<SharedString>,
        why: Option<HighlightCause>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if &key == self.value.get() {
            return false;
        }
        let Some(why) = why else {
            if !self.value.is_controlled() && self.value.set_held(key.clone()) {
                log(&self.instance, "value", &key, Reason::Imperative);
                self.observe(window, cx);
                cx.notify();
            }
            return false;
        };
        let index = self.index_of(key.as_ref());
        self.value
            .set_on_change(value_callback(self.on_value_change.clone(), index));
        let options = ChangeOptions {
            reason: Some(why.reason),
            source: None,
            event: why.event,
        };
        let changed = self.value.change(&self.instance, key, options, window, cx);
        if changed {
            self.observe(window, cx);
            cx.notify();
        }
        changed
    }

    /// Looks at what changed: the core first, as its subscription comes
    /// first in the original, then the command's own.
    pub(crate) fn observe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.core.clone();
        core.observe(self, window, cx);
        self.follow(window, cx);
        self.sync_field(window, cx);
    }

    // The highlight follows the value, from a choice, a call or a controlled
    // value. A new page highlights its best match: at once for data, and
    // once its commands render for a composed page.
    pub(crate) fn follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.value.get().clone();
        if value != self.core.listbox.highlighted() || value != self.previous_value {
            self.previous_value = value.clone();
            let listbox = self.core.listbox.clone();
            listbox.highlight(self, value, HighlightOptions::default(), window, cx);
        }
        let pages = self.pages.get().clone();
        let mounts = self.core.mounts();
        if pages != self.previous_pages {
            self.previous_pages = pages;
            let why = self
                .page_cause
                .take()
                .unwrap_or_else(|| HighlightCause::new(Reason::Imperative, None));
            if self.page_items().is_some() {
                let core = self.core.clone();
                core.settle(self, true, Some(why), window, cx);
            } else {
                self.awaiting = Some(why);
            }
        } else if self.awaiting.is_some() && mounts != self.previous_mounts {
            let why = self.awaiting.take();
            let core = self.core.clone();
            core.settle(self, true, why, window, cx);
        }
        self.previous_mounts = mounts;
    }

    // Only new text is written into the search field: typing stores the
    // query before the text, and a write in between would put the old text
    // back with the caret at the end. A controlled query the app didn't take
    // leaves what was typed.
    pub(crate) fn sync_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.get().clone();
        if query == self.written {
            return;
        }
        self.written = query.clone();
        self.field.update(cx, |field, cx| {
            if *field.value() != query {
                field.set_value(query, window, cx);
            }
        });
    }

    fn change_query(&mut self, query: SharedString, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.core
            .cause(options.reason.unwrap_or(Reason::Imperative), options.event.clone());
        let changed = self.query.change(&self.instance, query, options, window, cx);
        if changed {
            self.observe(window, cx);
            cx.notify();
        }
        changed
    }

    fn change_pages(
        &mut self,
        pages: Vec<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let changed = self.pages.change(&self.instance, pages, options, window, cx);
        if changed {
            self.observe(window, cx);
            cx.notify();
        }
        changed
    }

    // The text changing on its own, not with a page. An app that holds the
    // text and the value hears both in one event, so it renders them
    // together. With `filter: Off` the app's next items decide the
    // highlight, so it waits.
    pub(crate) fn search(&mut self, query: SharedString, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let changed = self.change_query(query.clone(), options, window, cx);
        if changed
            && self.query.is_controlled()
            && self.value.is_controlled()
            && self.filter != SearchFilter::Off
        {
            let core = self.core.clone();
            core.settle_for(self, query, window, cx);
        }
        changed
    }

    fn push_page(&mut self, page: SharedString, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let pages = self.pages.get().clone();
        let query = self.query.get().clone();
        self.page_cause = Some(HighlightCause::new(
            options.reason.unwrap_or(Reason::Imperative),
            options.event.clone(),
        ));
        let mut next = pages.clone();
        next.push(page);
        if !self.change_pages(next, options.clone(), window, cx) {
            return false;
        }
        self.queries.truncate(pages.len());
        self.queries.push(query);
        self.change_query(SharedString::default(), options, window, cx);
        true
    }

    fn pop_page(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let pages = self.pages.get().clone();
        if pages.is_empty() {
            return false;
        }
        self.page_cause = Some(HighlightCause::new(
            options.reason.unwrap_or(Reason::Imperative),
            options.event.clone(),
        ));
        if !self.change_pages(pages[..pages.len() - 1].to_vec(), options.clone(), window, cx) {
            return false;
        }
        let restored = self.queries.get(pages.len() - 1).cloned().unwrap_or_default();
        self.queries.truncate(pages.len() - 1);
        self.change_query(restored, options, window, cx);
        true
    }

    // Says the pending announcement once a command has been pending a while,
    // and nothing for an empty one.
    fn announce_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = !self.pending.is_empty();
        let text = self.pending_announcement.clone();
        let mut announcer = std::mem::take(&mut self.announce_pending);
        announcer.update(
            pending,
            text,
            |text, window, cx| {
                if !text.is_empty() {
                    ui_sdk_platform::announce(text, window, cx);
                }
            },
            window,
            cx,
        );
        self.announce_pending = announcer;
    }

    /// Runs a command, as Enter, a click or its shortcut would: its
    /// `on_select`, then its page or `on_action`. A pending outcome marks it
    /// pending. `fresh` gives `on_select` a new event to cancel, as a
    /// shortcut's key press is already handled.
    pub(crate) fn run_item(
        &mut self,
        key: SharedString,
        event: Option<InputEvent>,
        reason: Reason,
        fresh: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let props = self.props_of(&key);
        // A command from the data, whether the list renders it or the app does.
        let data = find_command(self.items.as_ref(), &key).or_else(|| find_command(self.empty_items.as_ref(), &key));
        if props.is_none() && data.is_none() {
            return;
        }
        if props.as_ref().is_some_and(|props| props.disabled)
            || data.as_ref().is_some_and(|data| data.disabled)
            || self.pending.contains(&key)
        {
            // A command rendered as a link would still follow itself.
            if props.as_ref().is_some_and(|props| props.link) {
                window.prevent_default();
            }
            return;
        }
        if self.error.as_ref().is_some_and(|(failed, _)| *failed == key) {
            self.error = None;
            cx.notify();
        }
        let selection = SelectEvent::new(if fresh { None } else { event.clone() });
        let mut results = Vec::new();
        if let Some(on_select) = props.as_ref().and_then(|props| props.on_select.clone()) {
            let outcome = on_select(&selection, window, cx);
            if matches!(outcome, ActionOutcome::Canceled) {
                selection.prevent_default();
            }
            results.push(outcome);
        }
        // Canceling a link's press keeps it from following itself, as its
        // click is the event the original cancels.
        if selection.default_prevented() && props.as_ref().is_some_and(|props| props.link) {
            window.prevent_default();
        }
        let page = props
            .as_ref()
            .and_then(|props| props.page.clone())
            .or_else(|| data.as_ref().filter(|data| data.extra.items.is_some()).map(|data| data.value.clone()));
        if !selection.default_prevented() {
            match page {
                Some(page) => {
                    let options = ChangeOptions {
                        reason: Some(reason),
                        source: None,
                        event: event.clone(),
                    };
                    self.push_page(page, options, window, cx);
                }
                None => {
                    if let Some(on_action) = self.on_action.clone() {
                        let details = CommandActionDetails { event: event.clone() };
                        results.push(on_action(&key, &details, window, cx));
                    }
                }
            }
        }
        let mut pending = Vec::new();
        let mut failed = None;
        for outcome in results {
            match outcome {
                ActionOutcome::Pending(work) => pending.push(work),
                ActionOutcome::Failed(error) => {
                    failed.get_or_insert(error);
                }
                ActionOutcome::Done | ActionOutcome::Canceled => {}
            }
        }
        if let Some(error) = failed {
            self.error = Some((key.clone(), error));
            cx.notify();
        }
        if pending.is_empty() {
            return;
        }
        // Each command is pending on its own, so others that started
        // meanwhile stay pending.
        self.pending.push(key.clone());
        log(&self.instance, "pending", &self.pending, reason);
        self.announce_pending(window, cx);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let mut work: FuturesUnordered<_> = pending.into_iter().collect();
            let mut settled = false;
            while let Some(result) = work.next().await {
                if let Err(error) = result
                    && !settled
                {
                    // The first failure settles it, as `Promise.all` does; the
                    // rest run on.
                    settled = true;
                    let key = key.clone();
                    this.update_in(cx, |command, window, cx| command.settle_pending(&key, Some(error), window, cx))
                        .ok();
                }
            }
            if !settled {
                this.update_in(cx, |command, window, cx| command.settle_pending(&key, None, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn settle_pending(&mut self, key: &SharedString, error: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        self.pending.retain(|other| other != key);
        log(&self.instance, "pending", &self.pending, Reason::Action);
        if let Some(error) = error {
            self.error = Some((key.clone(), error));
            log(&self.instance, "error", &self.error, Reason::Action);
        }
        self.announce_pending(window, cx);
        cx.notify();
    }

    /// The search field's keys past the core's steps: Enter runs the
    /// highlighted command, Escape clears the search text and then goes back
    /// a page, and Backspace in an empty field goes back a page.
    pub(crate) fn on_input_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.core.clone();
        if core.navigate(self, event, window, cx) {
            return;
        }
        match Key::of(&event.keystroke) {
            Key::Enter => {
                let Some(value) = self.value.get().clone() else {
                    // The search isn't form data, so Enter never submits a
                    // form around it.
                    if !is_composing(window, cx) {
                        window.prevent_default();
                    }
                    return;
                };
                // Links follow themselves, so routers and modifier keys work.
                match press_of(&self.instance, "item", Some(&value))
                    .filter(|_| self.core.listbox.element(&value).is_some_and(|element| element.link))
                {
                    Some(press) => press.click(InputEvent::from(event), window, cx),
                    None => self.run_item(value, Some(InputEvent::from(event)), Reason::ItemPress, false, window, cx),
                }
                window.prevent_default();
            }
            // Escape steps back one thing at a time: the search text, then a
            // page. With neither, it's left to a dialog around the list.
            Key::Escape => {
                let query_empty = self.query.get().is_empty();
                if query_empty && self.pages.get().is_empty() {
                    return;
                }
                window.prevent_default();
                let options = ChangeOptions::event(Reason::EscapeKey, event);
                if !query_empty {
                    self.search(SharedString::default(), options, window, cx);
                } else {
                    self.pop_page(options, window, cx);
                }
            }
            // Backspace in an empty field goes back a page.
            Key::Backspace => {
                if self.field.read(cx).value().is_empty() && !self.pages.get().is_empty() {
                    window.prevent_default();
                    self.pop_page(ChangeOptions::event(Reason::Back, event), window, cx);
                }
            }
            _ => {}
        }
    }

    // The value it starts with is highlighted, and data commands are known
    // at once, so without one the first is.
    pub(crate) fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(value) = self.value.get().clone() {
            let listbox = self.core.listbox.clone();
            listbox.highlight(self, Some(value), HighlightOptions::default(), window, cx);
        }
        let core = self.core.clone();
        core.settle(self, false, None, window, cx);
    }

    /// The highlighted command's value, or `None` (`getValue`).
    pub fn value(&self) -> Option<&SharedString> {
        self.value.get().as_ref()
    }

    /// Highlights a command, and scrolls it into view. Returns false when
    /// the change was canceled or nothing changed.
    pub fn set_value(
        &mut self,
        value: Option<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let why = HighlightCause::new(options.reason.unwrap_or(Reason::Imperative), options.event);
        self.highlight(value, Some(why), window, cx)
    }

    /// The search text (`getQuery`).
    pub fn query(&self) -> &SharedString {
        self.query.get()
    }

    /// Changes the search text, which filters the commands. Returns false
    /// when the change was canceled or nothing changed.
    pub fn set_query(
        &mut self,
        query: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.search(query.into(), options, window, cx)
    }

    /// The stack of open pages, from the first page opened to the current
    /// one. Empty on the first page (`getPages`).
    pub fn pages(&self) -> &[SharedString] {
        self.pages.get()
    }

    /// Replaces the stack of pages. Returns false when the change was
    /// canceled or nothing changed.
    pub fn set_pages(
        &mut self,
        pages: Vec<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.page_cause = Some(HighlightCause::new(
            options.reason.unwrap_or(Reason::Imperative),
            options.event.clone(),
        ));
        self.change_pages(pages, options, window, cx)
    }

    /// Opens a page on top of the current one, by the value of a page part
    /// or a data command. Returns false when the change was canceled or
    /// nothing changed.
    pub fn push(
        &mut self,
        page: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.push_page(page.into(), options, window, cx)
    }

    /// Goes back to the previous page, and puts back the search text typed
    /// there. Returns false when the change was canceled or nothing changed.
    pub fn pop(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.pop_page(options, window, cx)
    }

    /// Runs a command, as Enter would: the highlighted one, or the one with
    /// this value (`run`).
    pub fn run(&mut self, value: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = value.or_else(|| self.value.get().clone()) {
            self.run_item(key, None, Reason::ItemPress, true, window, cx);
        }
    }

    /// The commands whose action is running.
    pub fn pending(&self) -> &[SharedString] {
        &self.pending
    }

    /// The command whose last action failed, with why, until it runs again.
    pub fn error(&self) -> Option<&(SharedString, SharedString)> {
        self.error.as_ref()
    }

    /// Moves focus to the search field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let field = self.field.clone();
        field.update(cx, |field, cx| field.focus(window, cx));
    }
}
