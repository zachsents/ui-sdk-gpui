//! Port of `components/token-input/token-input-api.ts`: TokenInput's state
//! and api: the value and its changes, undo history, the caret,
//! announcements, and the editor adapter a trigger menu writes through.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, Context, FocusHandle, SharedString, Subscription, WeakEntity, Window};
use ui_sdk_forms::{ValueInputOptions, ValueMissingKind, bind_value_input, keeps_value_on_reset};
use ui_sdk_kernel::{
    ChangeCallback, ChangeDetails, ChangeOptions, ChangeSource, InputEvent, Reason, resolve_source,
    with_source,
};
use ui_sdk_text::{
    EditorChange, EditorContent, EditorListener, EditorReplaceOptions, PendingToken, TextBefore,
    TextEditorAdapter, TextRange, TextRect, Token, TokenStatus, TokenUpdate, text_diff,
};

use crate::token_input::{FocusAt, TokenInput, TokenInputValueChange, TokenInputValueChangeDetails, TokenInputSubmitDetails};
use crate::token_input_model::{
    Segment, TokenCodec, TokenInputValue, floor_char, normalize_value, offset_text, removed_tokens,
    replace_range, same_value, snap_offset, snap_range, to_label, to_text, token_at, tokens_of,
    value_length,
};
use crate::token_input_view::Geometry;

/// One step of the undo history: the value, and the selection it left.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub value: TokenInputValue,
    pub selection: Option<TextRange>,
}

const BLINK_INTERVAL: Duration = Duration::from_millis(500);

// Reasons a person caused, which a trigger menu follows.
fn user_reason(reason: Reason) -> bool {
    matches!(reason, Reason::InputChange | Reason::Paste | Reason::Drop | Reason::Cut)
}

// The ids of the tokens a selection covers.
pub(crate) fn selected_ids(value: &[Segment], range: Option<TextRange>) -> Vec<SharedString> {
    let Some(range) = range.filter(|range| range.start != range.end) else {
        return Vec::new();
    };
    let mut at = 0;
    let mut ids = Vec::new();
    for segment in value {
        if let Segment::Token(token) = segment
            && at >= range.start
            && at < range.end
            && let Some(id) = &token.id
        {
            ids.push(id.clone());
        }
        at += segment.len();
    }
    ids
}

// What a token says to screen readers besides its label.
fn default_description(token: &Token) -> SharedString {
    let status = match token.status {
        Some(TokenStatus::Pending) => ", loading",
        Some(TokenStatus::Unresolved) => ", unresolved",
        None => "",
    };
    format!("{}{status}", token.kind).into()
}

/// What `insert` puts in: text, a token followed by a space unless one
/// follows, or a value.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenInputContent {
    Text(SharedString),
    Token(Token),
    Value(TokenInputValue),
}

impl From<&str> for TokenInputContent {
    fn from(text: &str) -> Self {
        TokenInputContent::Text(SharedString::new(text))
    }
}

impl From<String> for TokenInputContent {
    fn from(text: String) -> Self {
        TokenInputContent::Text(text.into())
    }
}

impl From<SharedString> for TokenInputContent {
    fn from(text: SharedString) -> Self {
        TokenInputContent::Text(text)
    }
}

impl From<Token> for TokenInputContent {
    fn from(token: Token) -> Self {
        TokenInputContent::Token(token)
    }
}

impl From<TokenInputValue> for TokenInputContent {
    fn from(value: TokenInputValue) -> Self {
        TokenInputContent::Value(value)
    }
}

impl From<EditorContent> for TokenInputContent {
    fn from(content: EditorContent) -> Self {
        match content {
            EditorContent::Text(text) => TokenInputContent::Text(text),
            EditorContent::Token(token) => TokenInputContent::Token(token),
        }
    }
}

/// Where `insert` puts its content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InsertAt {
    /// Over the selection, or at the end before the editor ever had one.
    #[default]
    Caret,
    End,
    Range(TextRange),
}

/// Options for `insert`.
#[derive(Default)]
pub struct InsertOptions {
    /// Where it goes. The caret by default.
    pub at: InsertAt,
    /// Keeps a token pending until this settles: updated with what it
    /// resolves to, removed if it fails.
    pub resolve: Option<PendingToken>,
    /// The reason and event `on_value_change` hears. `Imperative` by default.
    pub change: ChangeOptions,
}

impl InsertOptions {
    pub fn at(mut self, at: InsertAt) -> Self {
        self.at = at;
        self
    }

    pub fn resolve(
        mut self,
        resolve: impl std::future::Future<Output = Result<Option<TokenUpdate>, SharedString>> + 'static,
    ) -> Self {
        self.resolve = Some(Box::pin(resolve));
        self
    }

    pub fn change(mut self, change: ChangeOptions) -> Self {
        self.change = change;
        self
    }
}

/// How one commit reports itself (`Edit`).
#[derive(Clone, Debug)]
pub(crate) struct Edit {
    pub reason: Reason,
    pub source: Option<ChangeSource>,
    pub event: Option<InputEvent>,
    pub input_type: Option<&'static str>,
    // The caret or selection after the edit.
    pub selection: Option<TextRange>,
    // Edits of one group join one undo step, like typing.
    pub group: Option<&'static str>,
    // Undo and redo move through history instead of adding to it.
    pub record: bool,
}

impl Edit {
    pub(crate) fn new(reason: Reason) -> Self {
        Self {
            reason,
            source: None,
            event: None,
            input_type: None,
            selection: None,
            group: None,
            record: true,
        }
    }

    pub(crate) fn from_options(options: ChangeOptions) -> Self {
        Self {
            source: options.source,
            event: options.event,
            ..Self::new(options.reason.unwrap_or(Reason::Imperative))
        }
    }

    pub(crate) fn event(mut self, event: Option<InputEvent>) -> Self {
        self.event = event;
        self
    }

    pub(crate) fn input_type(mut self, input_type: Option<&'static str>) -> Self {
        self.input_type = input_type;
        self
    }

    pub(crate) fn selection(mut self, selection: Option<TextRange>) -> Self {
        self.selection = selection;
        self
    }

    pub(crate) fn group(mut self, group: Option<&'static str>) -> Self {
        self.group = group;
        self
    }

    pub(crate) fn record(mut self, record: bool) -> Self {
        self.record = record;
        self
    }
}

// `on_value_change` with what the edit did.
fn change_callback(
    callback: Option<TokenInputValueChange>,
    input_type: Option<&'static str>,
    removed: Vec<Token>,
) -> Option<ChangeCallback<TokenInputValue>> {
    let callback = callback?;
    Some(Rc::new(move |value: &TokenInputValue, details, window: &mut Window, cx: &mut App| {
        callback(
            value,
            &TokenInputValueChangeDetails::new(details, input_type, &removed),
            window,
            cx,
        )
    }))
}

/// What the editor adapter reads, as the input last left it. Its listeners
/// hear what happens inside the input's update, so the adapter reads a copy,
/// and an edit one of them asks for waits until the update is over.
pub(crate) struct EditorMirror {
    pub focus_handle: FocusHandle,
    // The latest value's offset text, each token as U+FFFC.
    pub text: String,
    pub selection: TextRange,
    pub focused: bool,
    pub composing: bool,
    pub geometry: Option<Geometry>,
    listeners: Vec<(usize, EditorListener)>,
    next_listener: usize,
    telling: usize,
}

impl EditorMirror {
    pub(crate) fn new(focus_handle: FocusHandle, value: &[Segment]) -> Self {
        let text = offset_text(value);
        let end = text.len();
        Self {
            focus_handle,
            text,
            selection: TextRange::caret(end),
            focused: false,
            composing: false,
            geometry: None,
            listeners: Vec::new(),
            next_listener: 0,
            telling: 0,
        }
    }
}

/// The editor a trigger menu outside the token input reads and writes
/// (`editor`), from [`TokenInput::text_editor`]. It doesn't keep the input
/// alive. Offsets are into the value's text with each token as U+FFFC.
#[derive(Clone)]
pub struct TokenInputEditorAdapter {
    input: WeakEntity<TokenInput>,
    mirror: Rc<RefCell<EditorMirror>>,
}

impl TokenInputEditorAdapter {
    /// The input, while it's alive.
    pub fn entity(&self) -> Option<gpui::Entity<TokenInput>> {
        self.input.upgrade()
    }

    // Runs an edit on the input now, or once its update is over when one of
    // its listeners asked for it.
    fn edit(
        &self,
        window: &mut Window,
        cx: &mut App,
        edit: impl FnOnce(&mut TokenInput, &mut Window, &mut Context<TokenInput>) + 'static,
    ) {
        let input = self.input.clone();
        if self.mirror.borrow().telling > 0 {
            window.defer(cx, move |window, cx| {
                input.update(cx, |input, cx| edit(input, window, cx)).ok();
            });
        } else {
            input.update(cx, |input, cx| edit(input, window, cx)).ok();
        }
    }
}

impl TextEditorAdapter for TokenInputEditorAdapter {
    fn focus_handle(&self, _cx: &App) -> Option<FocusHandle> {
        let mirror = self.mirror.borrow();
        mirror.geometry.is_some().then(|| mirror.focus_handle.clone())
    }

    fn text_before(&self, length: usize, _window: &Window, _cx: &App) -> Option<TextBefore> {
        let mirror = self.mirror.borrow();
        if !mirror.focused {
            return None;
        }
        let caret = floor_char(&mirror.text, mirror.selection.end);
        let offset = mirror.text[..caret]
            .char_indices()
            .rev()
            .take(length)
            .last()
            .map_or(caret, |(index, _)| index);
        Some(TextBefore {
            text: mirror.text[offset..caret].to_string(),
            offset,
        })
    }

    fn selection(&self, _window: &Window, _cx: &App) -> Option<TextRange> {
        let mirror = self.mirror.borrow();
        mirror.focused.then_some(mirror.selection)
    }

    fn rect_at(&self, offset: usize, _window: &Window, _cx: &App) -> Option<TextRect> {
        let mirror = self.mirror.borrow();
        let geometry = mirror.geometry.as_ref()?;
        Some(geometry.caret_bounds(geometry.shown.to_shown(offset)))
    }

    fn replace(
        &self,
        range: TextRange,
        content: EditorContent,
        options: EditorReplaceOptions,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pending = options.pending;
        self.edit(window, cx, move |input, window, cx| {
            input.insert_at(range, content.into(), ChangeOptions::reason(Reason::ItemPress), pending, window, cx);
        });
    }

    fn subscribe(&self, listener: EditorListener, _cx: &mut App) -> Subscription {
        let id = {
            let mut mirror = self.mirror.borrow_mut();
            let id = mirror.next_listener;
            mirror.next_listener += 1;
            mirror.listeners.push((id, listener));
            id
        };
        let mirror = Rc::downgrade(&self.mirror);
        Subscription::new(move || {
            if let Some(mirror) = mirror.upgrade() {
                mirror.borrow_mut().listeners.retain(|(listener, _)| *listener != id);
            }
        })
    }

    fn composing(&self, _cx: &App) -> bool {
        self.mirror.borrow().composing
    }

    fn tokens(&self) -> bool {
        true
    }

    fn can_submit(&self) -> bool {
        true
    }

    fn submit(&self, window: &mut Window, cx: &mut App) {
        self.edit(window, cx, |input, window, cx| input.submit(None, window, cx));
    }
}

impl TokenInput {
    pub(crate) fn codec(&self) -> &TokenCodec {
        &self.settings.codec
    }

    /// What a token says to screen readers besides its label: the
    /// `get_token_description` option's, or its type and status.
    pub fn describe(&self, token: &Token) -> SharedString {
        match &self.settings.describe {
            Some(describe) => describe(token),
            None => default_description(token),
        }
    }

    /// The selection, as offsets into the value's text, each token U+FFFC.
    pub fn selection(&self) -> TextRange {
        TextRange::new(self.start, self.end)
    }

    /// The latest value, which a controlled owner may not have passed back
    /// yet.
    pub fn latest(&self) -> &TokenInputValue {
        &self.committed
    }

    /// The editor a trigger menu outside the token input reads and writes.
    pub fn text_editor(&self) -> TokenInputEditorAdapter {
        TokenInputEditorAdapter {
            input: self.this.clone(),
            mirror: self.mirror.clone(),
        }
    }

    // Tells the editor's listeners what happened.
    pub(crate) fn emit(&mut self, change: EditorChange, window: &mut Window, cx: &mut Context<Self>) {
        let listeners: Vec<EditorListener> = {
            let mut mirror = self.mirror.borrow_mut();
            if mirror.listeners.is_empty() {
                return;
            }
            mirror.telling += 1;
            mirror.listeners.iter().map(|(_, listener)| listener.clone()).collect()
        };
        self.sync_mirror();
        for listener in listeners {
            listener(&change, window, cx);
        }
        self.mirror.borrow_mut().telling -= 1;
    }

    // Keeps what the editor adapter reads current.
    pub(crate) fn sync_mirror(&self) {
        let mut mirror = self.mirror.borrow_mut();
        mirror.text = offset_text(&self.committed);
        mirror.selection = self.selection();
        mirror.focused = self.focused;
        mirror.composing = self.composition.is_some();
        mirror.geometry = self.geometry.clone();
    }

    // Reports the value to the form: the codec text under its name, and a
    // missing value while required and empty.
    pub(crate) fn sync_form(&self) {
        bind_value_input(
            &self.internals,
            ValueInputOptions {
                value: to_text(self.value.get(), &self.settings.codec).into(),
                name: self.settings.name.clone(),
                required: self.settings.required,
                disabled: self.settings.disabled,
                read_only: self.settings.read_only,
                form: self.settings.form.clone(),
                missing: Some(ValueMissingKind::Text),
            },
        );
    }

    // Says what happened to tokens, joined, so several changes at once are
    // one message.
    pub(crate) fn say(&mut self, message: String, window: &mut Window, cx: &mut Context<Self>) {
        self.messages.push(message);
        ui_sdk_platform::announce(&self.messages.join(". "), window, cx);
        if !self.clearing_messages {
            self.clearing_messages = true;
            cx.defer_in(window, |input, _window, _cx| {
                input.messages.clear();
                input.clearing_messages = false;
            });
        }
    }

    // Puts the selection at a range, on whole tokens and characters within
    // the latest value, with no report.
    pub(crate) fn place_range(&mut self, range: TextRange) {
        let range = snap_range(&self.committed, range);
        self.start = range.start;
        self.end = range.end;
        self.direction = ui_sdk_native::SelectionDirection::None;
        self.upstream = false;
        self.goal_x = None;
    }

    /// Selects a range, or puts the caret at an empty one (`setSelection`).
    pub(crate) fn select_range(&mut self, range: TextRange, window: &mut Window, cx: &mut Context<Self>) {
        self.place_range(range);
        self.changed(window, cx);
    }

    /// Reports what an operation changed: a new frame with the caret shown
    /// and in view, and the selection, if it moved, once the update is over,
    /// as `selectionchange` comes after the call that moved it.
    pub(crate) fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focused && (self.start..self.end) != self.reported && !self.selection_pending {
            self.selection_pending = true;
            cx.defer_in(window, |input, window, cx| input.report_selection(window, cx));
        }
        self.autoscroll = true;
        if self.focused {
            self.restart_blink(window, cx);
            window.invalidate_character_coordinates();
        }
        self.sync_mirror();
        cx.notify();
    }

    pub(crate) fn restart_blink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.caret_visible = true;
        self.blink_epoch = self.blink_epoch.wrapping_add(1);
        let epoch = self.blink_epoch;
        self.blink_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                let blinking = this.update(cx, |input, cx| {
                    if input.blink_epoch != epoch || !input.focused {
                        return false;
                    }
                    input.caret_visible = !input.caret_visible;
                    cx.notify();
                    true
                });
                if !matches!(blinking, Ok(true)) {
                    break;
                }
            }
        }));
    }

    // Focus came into the editor (`onFocus`): listeners hear where the
    // selection is.
    pub(crate) fn focused_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focused = true;
        // An editor that just took focus isn't composing yet.
        ui_sdk_kernel::set_composing(window, self.composition.is_some(), cx);
        self.reported = self.start..self.end;
        self.sync_mirror();
        self.selection_reported(window, cx);
        self.changed(window, cx);
    }

    // Focus left the editor (`onBlur`): what an input method composed
    // commits, so a dropped `compositionend` can't leave it stuck
    // composing, and listeners hear the selection is gone.
    pub(crate) fn blurred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_composition(window, cx);
        self.focused = false;
        self.caret_visible = false;
        self.blink_task = None;
        self.drag = None;
        self.sync_mirror();
        self.emit(EditorChange::Selection, window, cx);
        cx.notify();
    }

    fn report_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selection_pending = false;
        let range = self.start..self.end;
        if range == self.reported {
            return;
        }
        self.reported = range;
        self.selection_reported(window, cx);
    }

    // `onSelectionChange`: a token armed for removal stays armed only while
    // it's what's selected; the tokens the selection covers are selected.
    pub(crate) fn selection_reported(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let range = self.selection();
        if let Some(armed) = &self.armed {
            let still = range.end - range.start == crate::TOKEN_LEN
                && token_at(&self.committed, range.start).and_then(|token| token.id.as_ref()) == Some(armed);
            if !still {
                self.armed = None;
            }
        }
        self.set_selected(range, window, cx);
    }

    // `internal.setSelection`.
    pub(crate) fn set_selected(&mut self, range: TextRange, window: &mut Window, cx: &mut Context<Self>) {
        let ids = selected_ids(&self.committed, Some(range));
        if ids != self.selected {
            self.selected = ids;
            cx.notify();
        }
        self.sync_mirror();
        self.emit(EditorChange::Selection, window, cx);
    }

    /// Asks for a new value (`commit`): it's normalized, and unless it's the
    /// same, `on_value_change` hears it with what explains it. Returns
    /// whether it went through.
    pub(crate) fn commit(&mut self, next: &[Segment], edit: Edit, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let before = self.committed.clone();
        let value = normalize_value(next);
        // A controlled owner passes this value back as it is.
        self.passed.hold(&value);
        if same_value(&before, &value) {
            if let Some(after) = edit.selection {
                self.select_range(after, window, cx);
            }
            return false;
        }
        let removed = removed_tokens(&before, &value);
        self.pending_selection = edit.selection;
        self.committed = value.clone();
        if let Some(after) = edit.selection {
            self.place_range(after);
        }
        let accepted = if self.value.is_controlled() && self.value.get() == &value {
            // The owner still holds this value, but heard another asked for
            // since, so it hears this one too.
            let source = edit
                .source
                .unwrap_or_else(|| resolve_source(edit.reason, edit.event.as_ref()));
            let details = ChangeDetails::new(edit.reason, source, edit.event.clone());
            if let Some(callback) = self.on_value_change.clone() {
                let extra = TokenInputValueChangeDetails::new(&details, edit.input_type, &removed);
                with_source(source, || callback(&value, &extra, window, cx));
            }
            !details.canceled()
        } else {
            self.value
                .set_on_change(change_callback(self.on_value_change.clone(), edit.input_type, removed.clone()));
            self.value.change(
                &self.instance,
                value.clone(),
                ChangeOptions {
                    reason: Some(edit.reason),
                    source: edit.source,
                    event: edit.event.clone(),
                },
                window,
                cx,
            )
        };
        if !accepted {
            // A canceled edit comes back out of the editor.
            self.committed = before;
            self.pending_selection = None;
            self.render(window, cx);
            return false;
        }
        self.value_changed(window, cx);
        if edit.record {
            self.history.record(
                HistoryEntry {
                    value: value.clone(),
                    selection: edit.selection,
                },
                edit.group,
            );
        }
        let diff = text_diff(&offset_text(&before), &offset_text(&value));
        self.emit(
            EditorChange::Input {
                inserted: diff.inserted,
                range: diff.range,
                user: user_reason(edit.reason),
            },
            window,
            cx,
        );
        for token in &removed {
            let message = format!("Removed {} {}", self.describe(token), token.label);
            self.say(message, window, cx);
        }
        // A controlled input whose owner didn't take the value shows what it
        // holds, once the owner has had its turn to render.
        if self.value.is_controlled() {
            self.await_owner(value, window);
        }
        self.sync_form();
        self.sync_mirror();
        cx.notify();
        true
    }

    // The frame after the next one has drawn, the owner has rendered.
    fn await_owner(&mut self, value: TokenInputValue, window: &mut Window) {
        let this = self.this.clone();
        window.on_next_frame(move |window, _cx| {
            window.on_next_frame(move |window, cx| {
                this.update(cx, |input, cx| input.settle_owner(&value, window, cx)).ok();
            });
        });
    }

    fn settle_owner(&mut self, value: &TokenInputValue, window: &mut Window, cx: &mut Context<Self>) {
        if self.committed != *value || self.value.get() == value {
            return;
        }
        self.committed = self.value.get().clone();
        self.render(window, cx);
    }

    // A value from outside, like a controlled reset after sending, is its
    // own undo step. The value this instance sent, passed back, isn't.
    // Returns whether what shows changed.
    pub(crate) fn value_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let value = self.value.get().clone();
        if value == self.shown {
            return false;
        }
        self.shown = value.clone();
        if value != self.committed {
            self.history.record(
                HistoryEntry {
                    value: value.clone(),
                    selection: None,
                },
                None,
            );
            self.committed = value;
        }
        self.render(window, cx);
        true
    }

    // Shows the latest value, and the selection an edit left.
    pub(crate) fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let length = value_length(&self.committed);
        let target = self
            .pending_selection
            .take()
            .or_else(|| self.focused.then(|| self.selection()));
        match target {
            Some(target) if self.focused => {
                let target = TextRange::new(target.start.min(length), target.end.min(length));
                self.select_range(target, window, cx);
            }
            _ => {
                let start = snap_offset(&self.committed, self.start.min(length), false);
                let end = snap_offset(&self.committed, self.end.min(length), true);
                self.start = start.min(end);
                self.end = end;
            }
        }
        self.sync_form();
        self.sync_mirror();
        cx.notify();
    }

    // Inserts at a range: text, a token followed by a space unless one
    // follows, or a value.
    pub(crate) fn insert_at(
        &mut self,
        range: TextRange,
        content: TokenInputContent,
        options: ChangeOptions,
        pending: Option<PendingToken>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let value = self.committed.clone();
        let mut inserted: TokenInputValue = match content {
            TokenInputContent::Value(content) => normalize_value(&content),
            TokenInputContent::Text(text) => vec![Segment::Text(text)],
            TokenInputContent::Token(mut token) => {
                if pending.is_some() {
                    token.status = Some(TokenStatus::Pending);
                }
                normalize_value(&[Segment::Token(token)])
            }
        };
        let token = match inserted.as_slice() {
            [Segment::Token(token)] => Some(token.clone()),
            _ => None,
        };
        let range = snap_range(&value, range);
        let following = offset_text(&value)[range.end..].chars().next();
        if token.is_some() && following.is_none_or(|c| !ui_sdk_text::is_js_space(c)) {
            inserted.push(Segment::Text(" ".into()));
        }
        let end = range.start + value_length(&inserted);
        let next = replace_range(&value, range, &inserted);
        let edit = Edit::from_options(options).selection(Some(TextRange::caret(end)));
        if !self.commit(&next, edit, window, cx) {
            return false;
        }
        let Some(token) = token else {
            return true;
        };
        let message = format!("Inserted {} {}", self.describe(&token), token.label);
        self.say(message, window, cx);
        let Some(pending) = pending else {
            return true;
        };
        // An id the value already had is given to its first token, so read
        // the one it ended up with.
        let Some(token_id) = token_at(&next, range.start).and_then(|token| token.id.clone()) else {
            return true;
        };
        let label = token.label.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = pending.await;
            this.update_in(cx, |input, window, cx| match result {
                Ok(patch) => {
                    let mut patch = patch.unwrap_or_default();
                    patch.status = Some(patch.status.flatten());
                    input.update_token(&token_id, patch, window, cx);
                }
                Err(_) => {
                    input.say(format!("Couldn't add {label}"), window, cx);
                    input.remove_token_with(&token_id, Edit::new(Reason::Imperative).record(false), window, cx);
                }
            })
            .ok();
        })
        .detach();
        true
    }

    /// Changes a token's status, data or label without an undo step or a
    /// caret move, everywhere in the history too, so a token removed while it
    /// loads comes back settled on undo. Returns false when there's no such
    /// token or nothing changed. Its id stays.
    pub fn update_token(&mut self, id: &str, patch: TokenUpdate, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut patch = patch;
        patch.id = None;
        let update = |value: &TokenInputValue| -> TokenInputValue {
            value
                .iter()
                .map(|segment| match segment {
                    Segment::Token(token) if token.id.as_deref() == Some(id) => {
                        let mut token = token.clone();
                        token.update(patch.clone());
                        Segment::Token(token)
                    }
                    other => other.clone(),
                })
                .collect()
        };
        self.history.map(|entry| HistoryEntry {
            value: update(&entry.value),
            ..entry
        });
        let Some(before) = tokens_of(&self.committed)
            .into_iter()
            .find(|token| token.id.as_deref() == Some(id))
            .cloned()
        else {
            return false;
        };
        let next = update(&self.committed);
        let changed = self.commit(&next, Edit::new(Reason::Imperative).record(false), window, cx);
        if before.status == Some(TokenStatus::Pending) && !matches!(patch.status, Some(Some(_))) {
            let label = patch.label.clone().unwrap_or(before.label);
            self.say(format!("{label} ready"), window, cx);
        }
        changed
    }

    pub(crate) fn remove_token_with(&mut self, id: &str, edit: Edit, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let remove = |value: &TokenInputValue| -> TokenInputValue {
            value
                .iter()
                .filter(|segment| !matches!(segment, Segment::Token(token) if token.id.as_deref() == Some(id)))
                .cloned()
                .collect()
        };
        if !edit.record {
            self.history.map(|entry| HistoryEntry {
                value: remove(&entry.value),
                ..entry
            });
        }
        let next = remove(&self.committed);
        self.commit(&next, edit, window, cx)
    }

    pub(crate) fn undo_with(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(entry) = self.history.undo().cloned() else {
            return false;
        };
        let edit = Edit::new(Reason::Undo)
            .event(event)
            .selection(entry.selection)
            .record(false);
        self.commit(&entry.value, edit, window, cx)
    }

    pub(crate) fn redo_with(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(entry) = self.history.redo().cloned() else {
            return false;
        };
        let edit = Edit::new(Reason::Redo)
            .event(event)
            .selection(entry.selection)
            .record(false);
        self.commit(&entry.value, edit, window, cx)
    }

    // The submit shortcut: `on_submit`, else its form, which `form` can name
    // from outside it, like a native control's. The form submits once the
    // update is over, since a submit reads and focuses controls.
    pub(crate) fn submit(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(on_submit) = self.on_submit.clone() {
            let value = self.value.get().clone();
            on_submit(&value, &TokenInputSubmitDetails { event }, window, cx);
            return;
        }
        if let Some(form) = self.internals.form() {
            window.defer(cx, move |window, cx| form.request_submit(None, event, window, cx));
        }
    }

    pub(crate) fn can_submit(&self) -> bool {
        self.on_submit.is_some() || self.internals.form().is_some()
    }

    // A form reset returns to the default value, unless the app controls the
    // value with no default.
    pub(crate) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if keeps_value_on_reset(self.value.is_controlled().then_some(&()), self.has_default.then_some(&())) {
            return;
        }
        let initial = self.initial.clone();
        let edit = Edit::new(Reason::Reset).selection(Some(TextRange::caret(0)));
        self.commit(&initial, edit, window, cx);
    }

    /// The value: text and tokens (`getValue`).
    pub fn value(&self) -> &TokenInputValue {
        self.value.get()
    }

    /// Replaces the value as one undoable step, with the caret at its end.
    /// Returns false when the change was canceled or nothing changed.
    pub fn set_value(
        &mut self,
        value: TokenInputValue,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let caret = value_length(&normalize_value(&value));
        let edit = Edit::from_options(options).selection(Some(TextRange::caret(caret)));
        self.commit(&value, edit, window, cx)
    }

    /// The value as codec text, like `Ask [@Ada](mention:ada)`, in its own
    /// codec or another (`getText`).
    pub fn text(&self, codec: Option<&TokenCodec>) -> String {
        to_text(self.value.get(), codec.unwrap_or(&self.settings.codec))
    }

    /// The value as people read it, with each token's label (`getLabel`).
    pub fn label(&self) -> String {
        to_label(self.value.get())
    }

    /// Inserts text, a token or a value at the caret, the end or a range, as
    /// one undoable step. With `resolve`, a token is pending until it
    /// settles: updated with what it resolves to, removed if it fails.
    /// Returns false when the change was canceled.
    pub fn insert(
        &mut self,
        content: impl Into<TokenInputContent>,
        options: InsertOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let length = value_length(&self.committed);
        let range = match options.at {
            InsertAt::Range(range) => range,
            InsertAt::End => TextRange::caret(length),
            InsertAt::Caret => self.selection(),
        };
        self.insert_at(range, content.into(), options.change, options.resolve, window, cx)
    }

    /// Removes a token as one undoable step. Returns false when the change
    /// was canceled or nothing changed.
    pub fn remove_token(
        &mut self,
        id: &str,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.remove_token_with(id, Edit::from_options(options), window, cx)
    }

    /// Undoes the last step. Returns false when there was none or the change
    /// was canceled.
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.undo_with(None, window, cx)
    }

    /// Redoes the last undone step. Returns false when there was none or the
    /// change was canceled.
    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.redo_with(None, window, cx)
    }

    /// Whether there's a step to undo.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Whether there's a step to redo.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Empties the input as one undoable step. Returns false when the change
    /// was canceled or nothing changed.
    pub fn clear(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let edit = Edit::from_options(options).selection(Some(TextRange::caret(0)));
        self.commit(&[], edit, window, cx)
    }

    /// Focuses the editor, with the caret at its start or end. A disabled
    /// input can't take focus.
    pub fn focus(&mut self, at: FocusAt, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_disabled() {
            return;
        }
        window.focus(&self.focus_handle, cx);
        let offset = match at {
            FocusAt::Start => 0,
            FocusAt::End => value_length(self.value.get()),
        };
        self.select_range(TextRange::caret(offset), window, cx);
    }
}
