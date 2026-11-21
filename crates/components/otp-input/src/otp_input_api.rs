//! Port of `components/otp-input/otp-input-api.ts`: OTP Input's state and
//! behavior. One native field holds the whole code, so the field's own
//! typing, deleting, undo, paste and input methods all work on it. It sits
//! invisible over the slots, which render from the value and the field's
//! selection.

use std::rc::Rc;

use gpui::{
    App, Context, KeyDownEvent, Pixels, Point, SharedString, TextInputConfiguration, UnderlineStyle,
    WeakEntity, Window, px, transparent_black,
};
use regex::Regex;
use ui_sdk_forms::{
    FormEventType, FormValue, InternalsOptions, Problem, ValidityFlag, ValidityState, ValueMissingKind,
    ValueSource, bind_internals, dispatch_change, keeps_value_on_reset, notify_value, value_missing_message,
};
use ui_sdk_kernel::{
    ChangeCallback, ChangeDetails, ChangeOptions, CommitCallback, InputEvent, Key, Reason, is_composing, with_source,
};
use ui_sdk_native::{
    BeforeEdit, Edit, FieldHook, FieldNotice, InputType, SelectionDirection, TextField, TextFieldEvent,
    TextFieldKind, TextFieldOptions as FieldOptions, TextFieldStyle,
};
use ui_sdk_platform::is_rtl;

use crate::otp_input::OtpInput;
use crate::otp_input_model::{
    OtpInputPattern, Selection, fits_pattern, focus_selection, insert_code, is_valid_code, key_selection,
    pasted_code, pattern_attribute, press_selection, slot_selection,
};

/// Why the code changed: typing or deleting (`InputChange`), a paste
/// (`Paste`), the field's undo and redo (`Undo`, `Redo`), a form reset
/// (`Reset`) or a call (`Imperative`). A code from an input method or the
/// keyboard's suggestions is `InputChange`.
pub type OtpInputValueChangeReason = Reason;

/// Options for every write method.
pub type OtpInputChangeOptions = ChangeOptions;

/// Where the invisible field keeps its text and caret, which decides where
/// the platform puts its own popups, like an input method's candidates.
/// `Start` and `End` follow the reading direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OtpInputTextAlign {
    #[default]
    Start,
    Center,
    End,
}

/// The options `OtpInput` takes.
#[derive(Clone, Default)]
pub struct OtpInputOptions {
    /// The code, for a controlled input.
    pub value: Option<SharedString>,
    /// The code it starts with, when uncontrolled. A form reset returns
    /// here.
    pub default_value: Option<SharedString>,
    /// Runs when the code changes: typing or deleting (`InputChange`), a
    /// paste (`Paste`), the field's undo and redo (`Undo`, `Redo`), a form
    /// reset (`Reset`) or a call (`Imperative`). `details.cancel()` keeps
    /// the code as it was. It runs inside the input's update, so it may
    /// update any entity but the input.
    pub on_value_change: Option<ChangeCallback<SharedString>>,
    /// Runs when a change fills every slot, with the whole code, like to
    /// verify it. A call filling it counts, so does a code that changes
    /// while full. It runs inside the input's update.
    pub on_value_commit: Option<CommitCallback<SharedString>>,
    /// How many characters the code has, one slot each. 6 by default.
    pub max_length: Option<usize>,
    /// Which characters it accepts: digits, the default, letters,
    /// alphanumerics or a regular expression each character must match.
    /// Anything else typed or pasted is ignored.
    pub pattern: OtpInputPattern,
    /// Where the invisible field keeps its text and caret.
    pub text_align: OtpInputTextAlign,
    /// Disables it like a native input: it can't be focused, and isn't
    /// submitted.
    pub disabled: bool,
    /// Makes it read-only: focusable and submitted, but the code can't
    /// change.
    pub read_only: bool,
    /// Requires a code before the form submits. A partial code is too short
    /// either way.
    pub required: bool,
    /// The name it submits the code under.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
    /// The field's `id`, which a label's `for` names.
    pub id: Option<SharedString>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl OtpInputOptions {
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&SharedString, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_value_commit(
        mut self,
        callback: impl Fn(&SharedString, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_commit = Some(Rc::new(callback));
        self
    }

    pub fn max_length(mut self, max_length: usize) -> Self {
        self.max_length = Some(max_length);
        self
    }

    pub fn pattern(mut self, pattern: impl Into<OtpInputPattern>) -> Self {
        self.pattern = pattern.into();
        self
    }

    pub fn text_align(mut self, text_align: OtpInputTextAlign) -> Self {
        self.text_align = text_align;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn form(mut self, form: impl Into<SharedString>) -> Self {
        self.form = Some(form.into());
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

// The options that aren't the code's (`settings`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub max_length: usize,
    pub pattern: OtpInputPattern,
    pub text_align: OtpInputTextAlign,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub name: Option<SharedString>,
    pub form: Option<SharedString>,
    pub id: Option<SharedString>,
}

pub(crate) fn finite_length(max_length: Option<usize>) -> usize {
    max_length.filter(|length| *length > 0).unwrap_or(6)
}

pub(crate) fn settings(options: &OtpInputOptions) -> Settings {
    Settings {
        max_length: finite_length(options.max_length),
        pattern: options.pattern.clone(),
        text_align: options.text_align,
        disabled: options.disabled,
        read_only: options.read_only,
        required: options.required,
        name: options.name.clone(),
        form: options.form.clone(),
        id: options.id.clone(),
    }
}

/// A value from outside, kept to what the code can hold.
pub fn clean_code(value: &str, pattern: &OtpInputPattern, max_length: usize) -> SharedString {
    value
        .chars()
        .filter(|c| fits_pattern(c.encode_utf8(&mut [0; 4]), pattern))
        .take(max_length)
        .collect::<String>()
        .into()
}

// The character index of a byte offset into `value`.
pub(crate) fn char_index(value: &str, offset: usize) -> usize {
    value.char_indices().take_while(|(index, _)| *index < offset).count()
}

// The byte offset of a character index into `value`.
pub(crate) fn byte_offset(value: &str, index: usize) -> usize {
    value.char_indices().nth(index).map_or(value.len(), |(offset, _)| offset)
}

fn length_of(value: &str) -> usize {
    value.chars().count()
}

// The field paints nothing of its own: its text, caret, selection and an
// input method's underline are transparent over the slots.
fn hidden_style() -> TextFieldStyle {
    let clear = transparent_black();
    TextFieldStyle::default()
        .text_color(clear)
        .placeholder_color(clear)
        .caret_color(clear)
        .selection_color(clear)
        .selected_text_color(clear)
        .composition_underline(UnderlineStyle {
            thickness: px(1.),
            color: Some(clear),
            wavy: false,
        })
}

// Typing that fits is left to the field, so the undo history stays its own.
// Anything else, like several characters at once or a full-width digit,
// goes through `insert_many` once the field's update is over, which cleans
// it up or drops it. A paste goes there whole.
fn before_input(pattern: OtpInputPattern, this: WeakEntity<OtpInput>) -> FieldHook<BeforeEdit> {
    Rc::new(move |_field: &mut TextField, edit: &BeforeEdit, window: &mut Window, cx: &mut Context<TextField>| {
        let input_type = edit.input_type;
        // An input method's text, as it composes and as it commits, goes
        // back once composition ends if it doesn't fit.
        if !input_type.is_insert()
            || matches!(input_type, InputType::InsertCompositionText | InputType::InsertFromComposition)
        {
            return;
        }
        let paste = input_type == InputType::InsertFromPaste;
        // One fitting character is typing, which the field does itself.
        if !paste && length_of(&edit.data) <= 1 && fits_pattern(&edit.data, &pattern) {
            return;
        }
        edit.prevent_default();
        if edit.data.is_empty() {
            return;
        }
        let text = edit.data.clone();
        let reason = if paste { Reason::Paste } else { Reason::InputChange };
        let source = edit.source;
        let this = this.clone();
        window.defer(cx, move |window, cx| {
            this.update(cx, |otp, cx| with_source(source, || otp.insert_many(&text, reason, window, cx)))
                .ok();
        });
    })
}

// `form.requestSubmit()` from the field, once its update is over.
fn submit_hook(internals: ui_sdk_forms::ElementInternals) -> FieldNotice {
    Rc::new(move |_field, window, cx| {
        let internals = internals.clone();
        window.defer(cx, move |window, cx| {
            if let Some(form) = internals.form() {
                form.request_submit(None, None, window, cx);
            }
        });
    })
}

// The field's own history moves are reported as undo and redo.
fn reason_of(edit: Option<&Edit>) -> Reason {
    match edit.map(|edit| edit.input_type) {
        Some(InputType::HistoryUndo) => Reason::Undo,
        Some(InputType::HistoryRedo) => Reason::Redo,
        _ => Reason::InputChange,
    }
}

impl OtpInput {
    /// The code.
    pub fn get_value(&self) -> &SharedString {
        self.value.get()
    }

    /// Sets the code, as typing it would. Characters that don't fit the
    /// pattern are dropped. Returns false when the change was canceled or
    /// nothing changed.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        options: OtpInputChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.write(&value.into(), reason, options.event, window, cx)
    }

    /// Empties the code. Returns false when the change was canceled or
    /// nothing changed.
    pub fn clear(&mut self, options: OtpInputChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.write("", reason, options.event, window, cx)
    }

    /// Moves focus to the input.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.field.update(cx, |field, cx| field.focus(window, cx));
    }

    /// Whether it's disabled, by itself or by a fieldset (`isDisabled`).
    pub fn is_disabled(&self) -> bool {
        self.settings.disabled || self.internals.form_disabled()
    }

    // A reset puts a code back; nobody just entered it. The code given is
    // what's committed, which a controlled input commits before its owner
    // passes it back.
    fn complete(&self, value: &SharedString, reason: Reason, event: Option<InputEvent>, window: &mut Window, cx: &mut App) {
        if length_of(value) != self.settings.max_length || matches!(reason, Reason::Reset | Reason::Restore) {
            return;
        }
        self.value.commit_value(&self.instance, value, reason, event, window, cx);
    }

    // A change asked for from outside the field: a call, a reset. The field
    // follows unless it was canceled.
    pub(crate) fn write(
        &mut self,
        value: &str,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let code = clean_code(value, &self.settings.pattern, self.settings.max_length);
        let previous = self.value.get().clone();
        let options = ChangeOptions {
            reason: Some(reason),
            event: event.clone(),
            source: None,
        };
        if !self.value.change(&self.instance, code.clone(), options, window, cx) {
            return false;
        }
        self.sync_element(window, cx);
        // A reset may leave the field's value as it was, so the flag is read
        // either way.
        self.check_length(cx);
        if code != previous {
            self.complete(&code, reason, event, window, cx);
        }
        self.sync_form();
        cx.notify();
        true
    }

    // Follows whether the code is flagged as too short: a partial code the
    // user typed is, and nothing else.
    fn check_length(&mut self, cx: &App) {
        let length = length_of(self.field.read(cx).value());
        self.too_short = self.user_edited && length > 0 && length < self.settings.max_length;
    }

    // Puts the state's value in the field, and the selection where it
    // belongs.
    pub(crate) fn sync_element(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.value.get().clone();
        if self.awaiting_owner || *self.field.read(cx).value() == value {
            return;
        }
        self.field.update(cx, |field, cx| field.set_value(value.clone(), window, cx));
        notify_value(self.internals.node(), ValueSource::Write, window, cx);
        // A value written from code is never flagged as too short.
        self.too_short = false;
        self.user_edited = false;
        if self.field.read(cx).is_focused(window) {
            let selection = focus_selection(length_of(&value), self.settings.max_length);
            self.select(selection, SelectionDirection::None, window, cx);
        }
    }

    fn select(&mut self, selection: Selection, direction: SelectionDirection, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.field.read(cx).value().clone();
        let (start, end) = (byte_offset(&value, selection.start), byte_offset(&value, selection.end));
        self.field
            .update(cx, |field, cx| field.set_selection_range(start, end, direction, window, cx));
        self.selection = selection;
        cx.notify();
    }

    // Mirrors the field's selection into state, first moving a collapsed
    // caret onto the character under it, so that slot is the one typing
    // replaces. After a deletion the caret stays where the character was, so
    // typing puts one back there and the characters after it keep their
    // slots.
    fn sync_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.field.read(cx);
        if !field.is_focused(window) {
            return;
        }
        let value = field.value().clone();
        let current = Selection::new(
            char_index(&value, field.selection_start()),
            char_index(&value, field.selection_end()),
        );
        let deleted = self.after_delete && current.start == current.end;
        let next = if deleted {
            None
        } else {
            slot_selection(current, Some(self.selection), length_of(&value), self.settings.max_length)
        };
        match next {
            Some((selection, direction)) => self.select(selection, direction, window, cx),
            None => {
                if self.selection != current {
                    self.selection = current;
                    cx.notify();
                }
            }
        }
    }

    // The field changed: typing, deleting, undo, a paste or an input method.
    // Anything that isn't a code goes back, and so does a canceled change.
    fn typed(&mut self, edit: Option<&Edit>, composing: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.after_delete = edit.is_some_and(|edit| edit.input_type.is_delete());
        // An insert of ours reports the reason it was made with.
        let inserting = if edit.is_some_and(|edit| !edit.user) {
            self.inserting.take()
        } else {
            None
        };
        if edit.is_some() {
            self.user_edited = true;
            let node = self.internals.node().clone();
            notify_value(&node, ValueSource::Input, window, cx);
            dispatch_change(Some(&node), &[FormEventType::Input], window, cx);
        }
        let previous = self.value.get().clone();
        let mut next = self.field.read(cx).value().clone();
        if next != previous && !is_valid_code(&next, &self.settings.pattern, self.settings.max_length) {
            // An input method owns its text until it commits; it goes back then.
            if composing {
                return;
            }
            next = match pasted_code(&next, &self.settings.pattern) {
                Some(code) if length_of(&code) <= self.settings.max_length => code.into(),
                _ => previous.clone(),
            };
            self.restore(&next, window, cx);
        }
        if next != previous {
            let (reason, event) = inserting.unwrap_or_else(|| (reason_of(edit), None));
            let options = ChangeOptions {
                reason: Some(reason),
                event: event.clone(),
                source: None,
            };
            if self.value.change(&self.instance, next.clone(), options, window, cx) {
                // A controlled input shows what its owner passes back, once
                // the owner has rendered. Until then it keeps the typed code
                // and its caret, rather than the value the owner held before.
                if self.value.is_controlled() {
                    self.await_owner(window, cx);
                }
                self.complete(&next, reason, event, window, cx);
            } else {
                self.restore(&previous, window, cx);
            }
        }
        self.check_length(cx);
        self.sync_selection(window, cx);
        self.sync_form();
        cx.notify();
    }

    // The owner renders on the next frame; the field follows it after that
    // frame.
    fn await_owner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.awaiting_owner = true;
        self.owner_wait = self.owner_wait.wrapping_add(1);
        let wait = self.owner_wait;
        cx.on_next_frame(window, move |_, window, cx| {
            cx.on_next_frame(window, move |otp, window, cx| {
                if otp.owner_wait == wait {
                    otp.awaiting_owner = false;
                    otp.sync_element(window, cx);
                }
            });
        });
    }

    // Puts a value back, keeping the caret where the rejected text began.
    fn restore(&mut self, value: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.field.read(cx);
        let current = field.value().clone();
        let start = char_index(&current, field.selection_start()) as isize;
        let inserted = length_of(&current) as isize - length_of(value) as isize;
        let caret = byte_offset(value, (start - inserted.max(0)).max(0) as usize);
        self.field.update(cx, |field, cx| {
            field.set_value(value.clone(), window, cx);
            field.set_selection_range(caret, caret, SelectionDirection::None, window, cx);
        });
        self.user_edited = false;
    }

    // Replaces the whole value the way typing would, as one step the field
    // can undo.
    fn insert(&mut self, value: SharedString, reason: Reason, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        self.inserting = Some((reason, event));
        let inserted = self.field.update(cx, |field, cx| {
            let length = field.len();
            field.set_selection_range(0, length, SelectionDirection::None, window, cx);
            field.replace_range(0..length, &value, window, cx)
        });
        if !inserted {
            self.inserting = None;
        }
    }

    // Several characters at once, from a paste, an input method or a code
    // the keyboard suggests: spaces and dashes go, and a whole code replaces
    // the value.
    fn insert_many(&mut self, text: &str, reason: Reason, window: &mut Window, cx: &mut Context<Self>) {
        let Some(code) = pasted_code(text, &self.settings.pattern) else {
            return;
        };
        let value = self.value.get().clone();
        let field = self.field.read(cx);
        let shown = field.value().clone();
        let selection = Selection::new(
            char_index(&shown, field.selection_start()),
            char_index(&shown, field.selection_end()),
        );
        let next = insert_code(&value, selection, &code, self.settings.max_length);
        if next.as_str() != value.as_ref() {
            self.insert(next.into(), reason, None, window, cx);
        }
    }

    // The slot a press landed on. The field covers the slots, so the slot is
    // found under the point.
    fn slot_at(&self, position: Point<Pixels>) -> Option<usize> {
        self.instance
            .bound_parts()
            .into_iter()
            .find(|part| part.slot == "slot" && part.bounds.is_some_and(|bounds| bounds.contains(&position)))
            .and_then(|part| part.key.and_then(|key| key.parse().ok()))
    }

    // A mouse press selects the slot under it. The field would put the caret
    // where its invisible text is, so the press is taken over: focus, then
    // the slot. A tap keeps the field's handling, which touch screens need
    // for their paste menu, and selects on the click.
    pub(crate) fn press_slot(&mut self, position: Point<Pixels>, mouse: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.after_delete = false;
        let Some(index) = self.slot_at(position) else {
            return;
        };
        if self.is_disabled() {
            return;
        }
        if mouse {
            window.prevent_default();
            if !self.field.read(cx).is_focused(window) {
                // The focus that follows keeps the slot the press selects.
                self.pressed = true;
                self.field.update(cx, |field, cx| field.focus(window, cx));
            }
        }
        let length = length_of(self.field.read(cx).value());
        let selection = press_selection(index, length, self.settings.max_length);
        self.select(selection, SelectionDirection::None, window, cx);
    }

    // Arrow keys, Home and End move a slot at a time, here rather than after
    // the field moves the caret, whose selection change comes too late for
    // a key typed right after. With a modifier, they extend the selection as
    // the field does.
    pub(crate) fn keydown(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = Key::of(&event.keystroke);
        if key != Key::Backspace && key != Key::Delete {
            self.after_delete = false;
        }
        // An input method owns the keys while it composes, like arrows
        // choosing a candidate.
        if is_composing(window, cx) {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.shift || modifiers.alt || modifiers.control || modifiers.platform {
            return;
        }
        let length = length_of(self.field.read(cx).value());
        let Some(next) = key_selection(key.name(), self.selection, length, self.settings.max_length, is_rtl(cx))
        else {
            return;
        };
        window.prevent_default();
        self.select(next, SelectionDirection::None, window, cx);
    }

    // What the field tells once its update is over, which the original hears
    // with listeners bound on the input.
    pub(crate) fn field_event(&mut self, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Input(edit) => {
                let composing = self.field.read(cx).is_composing();
                with_source(edit.source, || self.typed(Some(edit), composing, window, cx));
            }
            // Input methods differ on whether an edit follows the end of a
            // composition.
            TextFieldEvent::CompositionEnd(_) => self.typed(None, false, window, cx),
            TextFieldEvent::Change(_) => {
                dispatch_change(Some(self.internals.node()), &[FormEventType::Change], window, cx);
            }
            TextFieldEvent::Focus => {
                self.focused = true;
                if !std::mem::take(&mut self.pressed) {
                    let length = length_of(self.field.read(cx).value());
                    let selection = focus_selection(length, self.settings.max_length);
                    self.select(selection, SelectionDirection::None, window, cx);
                }
                cx.notify();
            }
            TextFieldEvent::Blur => {
                self.focused = false;
                self.pressed = false;
                cx.notify();
            }
            TextFieldEvent::SelectionChange { .. } => self.sync_selection(window, cx),
            _ => {}
        }
    }

    // A form reset returns the code to its default, unless it's controlled
    // with none. The form puts the field's own value back first, which
    // nobody typed.
    pub(crate) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.user_edited = false;
        self.too_short = false;
        let controlled = self.value.is_controlled().then_some(());
        let default = self.has_default.then_some(());
        if !keeps_value_on_reset(controlled.as_ref(), default.as_ref()) {
            let initial = self.initial.clone();
            self.write(&initial, Reason::Reset, None, window, cx);
        }
        notify_value(self.internals.node(), ValueSource::Reset, window, cx);
        self.sync_form();
        cx.notify();
    }

    // The field's options: a text field as long as the code (`maxlength`),
    // with nothing of its own painted, nothing corrected or capitalized, and
    // the component deciding what goes in. A Field writing its `disabled`,
    // or a fieldset, disables it.
    pub(crate) fn field_options(&self) -> FieldOptions {
        FieldOptions {
            kind: TextFieldKind::Text,
            max_length: Some(self.settings.max_length),
            disabled: self.is_disabled() || self.internals.control_attrs().disabled,
            read_only: self.settings.read_only,
            required: self.settings.required,
            configuration: TextInputConfiguration::default(),
            style: hidden_style(),
            on_before_input: Some(before_input(self.settings.pattern.clone(), self.this.clone())),
            on_submit: Some(submit_hook(self.internals.clone())),
            ..FieldOptions::default()
        }
    }

    pub(crate) fn sync_field(&self, cx: &mut App) {
        let options = self.field_options();
        self.field.update(cx, |field, cx| field.set_options(options, cx));
    }

    // Reports the code and its constraints, as the native input does by
    // itself: `required`, the pattern, and `minlength`, the code's length.
    pub(crate) fn sync_form(&mut self) {
        let value = self.value.get().clone();
        let invalid = self.problem(&value);
        let settings = &self.settings;
        bind_internals(
            &self.internals,
            InternalsOptions {
                value: FormValue::Text(value.clone()),
                name: settings.name.clone(),
                value_missing: settings.required && value.is_empty(),
                message: value_missing_message(ValueMissingKind::Text),
                invalid,
                anchor: None,
                disabled: settings.disabled,
                read_only: settings.read_only,
                required: settings.required,
                // A read-only input is barred from validation.
                barred: settings.read_only,
                form: settings.form.clone(),
            },
        );
    }

    // What the code's pattern and length find, with Chromium's English
    // messages, a mismatched format first.
    fn problem(&mut self, value: &str) -> Option<Problem> {
        if value.is_empty() {
            return None;
        }
        let mut flags = ValidityState::default();
        let mut message = None;
        let max_length = self.settings.max_length;
        if let Some(attribute) = pattern_attribute(&self.settings.pattern, max_length, self.too_short) {
            let fits = self.pattern_regex(&attribute).is_some_and(|regex| regex.is_match(value));
            if !fits {
                flags.set(ValidityFlag::PatternMismatch, true);
                message.get_or_insert_with(|| "Please match the requested format.".to_string());
            }
        }
        if self.too_short {
            let length = length_of(value);
            flags.set(ValidityFlag::TooShort, true);
            message.get_or_insert_with(|| {
                format!(
                    "Please lengthen this text to {max_length} {} or more (you are currently using {length} {}).",
                    characters(max_length),
                    characters(length)
                )
            });
        }
        let message = message?;
        Some(Problem {
            flags,
            message: message.into(),
            anchor: None,
        })
    }

    // The pattern attribute, compiled once for each text it takes.
    fn pattern_regex(&mut self, attribute: &str) -> Option<Regex> {
        match &self.pattern_cache {
            Some((text, regex)) if text == attribute => regex.clone(),
            _ => {
                let regex = Regex::new(&format!("^(?:{attribute})$")).ok();
                self.pattern_cache = Some((attribute.to_string(), regex.clone()));
                regex
            }
        }
    }
}

fn characters(count: usize) -> &'static str {
    if count == 1 { "character" } else { "characters" }
}
