//! Port of `components/token-input/token-input.ts`: TokenInput, a text field
//! with atomic tokens, like mentions, files and commands in an AI prompt. The
//! value is text and tokens in memory, and one codec turns it into text at
//! every boundary: the clipboard, forms and drafts.
//!
//! This module holds the public types and the entity, with its options and
//! agent surface. The model is in `token_input_model.rs`, what the editor
//! shows in `token_input_view.rs`, editing in `token_input_editing.rs`, the
//! api in `token_input_api.rs`, and parts in `token_input_parts.rs`.

use std::cell::RefCell;
use std::fmt;
use std::ops::{Deref, Range};
use std::rc::Rc;

use gpui::{
    AnyElement, App, Autocapitalize, Context, Div, Entity, FocusHandle, Focusable, Pixels, Point,
    SharedString, Stateful, Subscription, Task, TextInputAction, TextInputConfiguration, WeakEntity,
    Window,
};
use ui_sdk_forms::{ElementInternals, focus_with};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentArgs, AgentError, AgentState, AgentValue, ChangeDetails, ChangeOptions,
    Controllable, InputEvent, Instance, Reason, UndoStack, UndoStackOptions, create_undo_stack,
    focus_events, json,
};
use ui_sdk_native::{SelectionDirection, TextFieldStyle};
use ui_sdk_text::{TextRange, Token, TokenStatus};

use crate::token_input_api::{EditorMirror, HistoryEntry};
use crate::token_input_editing::{Composition, Drag};
use crate::token_input_model::{
    Segment, TokenCodec, TokenInputValue, normalize_value, status_name, value_length,
};
use crate::token_input_view::{A11ySnapshot, Geometry};

/// Why a token input's value wants to change, from the shared vocabulary:
/// `InputChange` (typing, deleting, a new line), `Paste`, `Drop`, `Cut`,
/// `Undo`, `Redo`, `RemovePress` (a token's remove part), `ItemPress` (a
/// trigger menu's choice), `Reset` (a form reset), `Restore` and
/// `Imperative` (a call).
pub type TokenInputValueChangeReason = Reason;

/// Options for every write method: the reason and event `on_value_change`
/// receives.
pub type TokenInputChangeOptions = ChangeOptions;

/// What `on_submit` receives besides the value.
#[derive(Clone, Debug, Default)]
pub struct TokenInputSubmitDetails {
    /// The key press, or the trigger menu item's choice, that submitted.
    pub event: Option<InputEvent>,
}

/// What `on_value_change` receives besides the value: the reason, source
/// and event, with `cancel()`, as every change's details have, and what the
/// edit did.
pub struct TokenInputValueChangeDetails<'a> {
    details: &'a ChangeDetails,
    /// The browser's name for the kind of edit, like `"insertText"` or
    /// `"deleteContentBackward"`, for typing and deleting.
    pub input_type: Option<&'static str>,
    /// The tokens the old value held and the new one doesn't, matched by id.
    pub removed: &'a [Token],
}

impl<'a> TokenInputValueChangeDetails<'a> {
    pub(crate) fn new(details: &'a ChangeDetails, input_type: Option<&'static str>, removed: &'a [Token]) -> Self {
        Self {
            details,
            input_type,
            removed,
        }
    }

    /// The change's own details, with its reason, source and event.
    pub fn details(&self) -> &ChangeDetails {
        self.details
    }
}

impl Deref for TokenInputValueChangeDetails<'_> {
    type Target = ChangeDetails;

    fn deref(&self) -> &ChangeDetails {
        self.details
    }
}

impl fmt::Debug for TokenInputValueChangeDetails<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenInputValueChangeDetails")
            .field("details", self.details)
            .field("input_type", &self.input_type)
            .field("removed", &self.removed)
            .finish()
    }
}

/// `on_value_change`: the requested value and what explains it.
pub type TokenInputValueChange =
    Rc<dyn Fn(&TokenInputValue, &TokenInputValueChangeDetails<'_>, &mut Window, &mut App)>;

/// `on_submit`: the value, and what submitted it.
pub type TokenInputSubmit = Rc<dyn Fn(&TokenInputValue, &TokenInputSubmitDetails, &mut Window, &mut App)>;

/// `get_token_description`: what a token says to screen readers besides its
/// label.
pub type TokenDescription = Rc<dyn Fn(&Token) -> SharedString>;

/// What the editor renders inside a token's host: usually a
/// [`TokenInput::token`] part with a [`TokenInput::token_remove`] inside.
pub type RenderToken = Rc<dyn Fn(&Entity<TokenInput>, &Token, &mut Window, &mut App) -> AnyElement>;

/// How a token's host is styled, like a chip around its label when nothing
/// renders its content, from the token's state.
pub type HostStyle = Rc<dyn Fn(Stateful<Div>, &TokenInputTokenState) -> Stateful<Div>>;

/// How Backspace and Delete treat a token next to the caret.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TokenDeletion {
    /// `"whole"`: removes it in one press.
    #[default]
    Whole,
    /// `"select"`: selects it first, and removes it on the second press.
    Select,
    /// `"revert"`: turns it back into its text, so a trigger menu opens on
    /// it again.
    Revert,
}

/// Which keys submit (`submitShortcut`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TokenInputSubmitShortcut {
    /// `"enter"`: Enter submits, and Shift+Enter starts a new line.
    #[default]
    Enter,
    /// `"mod+enter"`: Command+Enter on Apple platforms and Control+Enter
    /// elsewhere submits, and Enter starts a new line, which many Japanese
    /// and Chinese writers prefer.
    ModEnter,
}

/// What copying writes as plain text (`copy`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TokenInputCopy {
    /// `"codec"`: keeps tokens through any app that pastes text.
    #[default]
    Codec,
    /// `"label"`: what people read.
    Label,
}

/// Where the caret goes as the editor takes focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FocusAt {
    Start,
    #[default]
    End,
}

/// The options `TokenInput` takes.
#[derive(Clone, Default)]
pub struct TokenInputOptions {
    /// The value. Pass it to control the input.
    pub value: Option<TokenInputValue>,
    /// The value at first, when uncontrolled. A form reset returns to it.
    pub default_value: Option<TokenInputValue>,
    /// Runs when typing, pasting, a menu, undo or a call wants to change the
    /// value. `details.reason` says why, `details.removed` lists removed
    /// tokens. It runs once the input's update is over, so it may update any
    /// entity, the input too.
    pub on_value_change: Option<TokenInputValueChange>,
    /// Runs as the value is asked to change, before it does, inside the
    /// input's update, with what explains it in the details:
    /// `details.cancel()` stops it. It may update any entity but the input.
    pub on_before_value_change: Option<TokenInputValueChange>,
    /// Runs on the submit shortcut, unless an input method is composing or a
    /// trigger menu took the key. Without it, the shortcut submits the form
    /// the input is in. With neither, the key is left alone, so Enter starts
    /// a new line. It runs once the input's update is over, so it may update
    /// any entity, the input too, like clearing it once it's sent.
    pub on_submit: Option<TokenInputSubmit>,
    /// Which keys submit. Enter by default.
    pub submit_shortcut: TokenInputSubmitShortcut,
    /// How Backspace and Delete treat a token next to the caret. `Whole` by
    /// default.
    pub token_deletion: TokenDeletion,
    /// What copying writes as plain text. The codec's text by default.
    pub copy: TokenInputCopy,
    /// How tokens become text and back, for the clipboard and forms.
    /// `[@label](type:value)` by default.
    pub codec: Option<TokenCodec>,
    /// What a token says to screen readers besides its label, like its kind
    /// and status. By default its type, and "loading" while pending.
    pub get_token_description: Option<TokenDescription>,
    /// Undo history: how many steps it keeps, and how long a typing pause
    /// starts a new step.
    pub history: Option<UndoStackOptions>,
    /// Disables the input: it can't be focused or changed, and isn't
    /// submitted.
    pub disabled: bool,
    /// Keeps the input focusable and submitted, but stops changes.
    pub read_only: bool,
    /// Blocks form submission while the input is empty.
    pub required: bool,
    /// The name the codec text submits under. Without one, the input
    /// submits nothing.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when the input isn't inside it.
    pub form: Option<SharedString>,
    /// The colors its text, caret, selection and placeholder paint with.
    pub field_style: TextFieldStyle,
    /// What the platform's text services offer: corrections, capitals and
    /// suggestions. By default corrections and suggestions, as an editable
    /// element has its browser's spelling and autocorrect.
    pub configuration: Option<TextInputConfiguration>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl TokenInputOptions {
    pub fn value(mut self, value: TokenInputValue) -> Self {
        self.value = Some(value);
        self
    }

    pub fn default_value(mut self, value: TokenInputValue) -> Self {
        self.default_value = Some(value);
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&TokenInputValue, &TokenInputValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&TokenInputValue, &TokenInputValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_submit(
        mut self,
        callback: impl Fn(&TokenInputValue, &TokenInputSubmitDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_submit = Some(Rc::new(callback));
        self
    }

    pub fn submit_shortcut(mut self, shortcut: TokenInputSubmitShortcut) -> Self {
        self.submit_shortcut = shortcut;
        self
    }

    pub fn token_deletion(mut self, deletion: TokenDeletion) -> Self {
        self.token_deletion = deletion;
        self
    }

    pub fn copy(mut self, copy: TokenInputCopy) -> Self {
        self.copy = copy;
        self
    }

    pub fn codec(mut self, codec: TokenCodec) -> Self {
        self.codec = Some(codec);
        self
    }

    pub fn get_token_description(mut self, describe: impl Fn(&Token) -> SharedString + 'static) -> Self {
        self.get_token_description = Some(Rc::new(describe));
        self
    }

    pub fn history(mut self, history: UndoStackOptions) -> Self {
        self.history = Some(history);
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

    pub fn field_style(mut self, style: TextFieldStyle) -> Self {
        self.field_style = style;
        self
    }

    pub fn configuration(mut self, configuration: TextInputConfiguration) -> Self {
        self.configuration = Some(configuration);
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the `editor` part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenInputEditorState {
    /// Whether the input holds nothing.
    pub empty: bool,
    /// Whether the input is disabled, by itself or by a fieldset.
    pub disabled: bool,
    /// Whether the input is read-only: it stays focusable, but typing,
    /// pastes and drops don't change it.
    pub read_only: bool,
}

/// What the `token` part's `with` reads, and a token's host when nothing
/// renders its content.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenInputTokenState {
    /// The token this part shows.
    pub token: Token,
    /// Whether the token is in the selection, or selected for removal.
    pub selected: bool,
    /// Whether the token is loading.
    pub pending: bool,
    /// Whether the token needs the user, like an ambiguous mention.
    pub unresolved: bool,
    /// Whether the input is disabled, by itself or by a fieldset.
    pub disabled: bool,
    /// Whether the input is read-only, so the token's remove part does
    /// nothing.
    pub read_only: bool,
}

/// What the `token-remove` part's `with` reads: disabled and read-only
/// inputs mark it, so it can hide or dim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenInputTokenRemoveState {
    pub disabled: bool,
    pub read_only: bool,
}

// What `sync` copies from options, beside the value: what renders and how
// the input edits.
#[derive(Clone)]
pub(crate) struct Settings {
    pub submit_shortcut: TokenInputSubmitShortcut,
    pub token_deletion: TokenDeletion,
    pub copy: TokenInputCopy,
    pub codec: TokenCodec,
    pub describe: Option<TokenDescription>,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub name: Option<SharedString>,
    pub form: Option<SharedString>,
    pub style: TextFieldStyle,
    pub configuration: TextInputConfiguration,
}

impl Settings {
    fn of(options: &TokenInputOptions) -> Self {
        Self {
            submit_shortcut: options.submit_shortcut,
            token_deletion: options.token_deletion,
            copy: options.copy,
            codec: options.codec.clone().unwrap_or_default(),
            describe: options.get_token_description.clone(),
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            name: options.name.clone(),
            form: options.form.clone(),
            style: options.field_style,
            configuration: options.configuration.clone().unwrap_or_else(default_configuration),
        }
    }
}

impl PartialEq for Settings {
    fn eq(&self, other: &Self) -> bool {
        self.submit_shortcut == other.submit_shortcut
            && self.token_deletion == other.token_deletion
            && self.copy == other.copy
            && self.codec == other.codec
            && match (&self.describe, &other.describe) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && self.disabled == other.disabled
            && self.read_only == other.read_only
            && self.required == other.required
            && self.name == other.name
            && self.form == other.form
            && self.style == other.style
            && self.configuration == other.configuration
    }
}

// An editable element keeps its browser's spelling and autocorrect.
fn default_configuration() -> TextInputConfiguration {
    TextInputConfiguration {
        autocorrect: true,
        autocapitalize: Autocapitalize::Sentences,
        suggestions: true,
        input_action: TextInputAction::Unspecified,
    }
}

// A controlled value passed back as the same value keeps its normalized
// form, so tokens keep their ids and hosts: the raw value last passed, and
// what normalizing it gave.
#[derive(Default)]
pub(crate) struct Normalized {
    raw: Option<TokenInputValue>,
    normalized: TokenInputValue,
}

impl Normalized {
    pub(crate) fn get(&mut self, raw: &TokenInputValue) -> TokenInputValue {
        if self.raw.as_ref() != Some(raw) {
            self.normalized = normalize_value(raw);
            self.raw = Some(raw.clone());
        }
        self.normalized.clone()
    }

    // A value this instance committed, which an owner passes back as it is.
    pub(crate) fn hold(&mut self, value: &TokenInputValue) {
        self.raw = Some(value.clone());
        self.normalized = value.clone();
    }
}

/// A token input. Its entity is its controller.
pub struct TokenInput {
    pub(crate) instance: Instance,
    pub(crate) this: WeakEntity<Self>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) settings: Settings,
    pub(crate) on_value_change: Option<TokenInputValueChange>,
    pub(crate) on_before_value_change: Option<TokenInputValueChange>,
    pub(crate) on_submit: Option<TokenInputSubmit>,

    // State: the value, the one a reset returns to, and what else parts read.
    pub(crate) value: Controllable<TokenInputValue>,
    pub(crate) initial: TokenInputValue,
    pub(crate) has_default: bool,
    pub(crate) passed: Normalized,
    pub(crate) passed_default: Normalized,
    pub(crate) form_disabled: bool,
    // Ids of the tokens in the selection, or selected for removal.
    pub(crate) selected: Vec<SharedString>,

    // The value this instance committed last. Edits build on it, since a
    // controlled value comes back only once the owner renders. It also tells
    // this instance's own changes from a value set from outside.
    pub(crate) committed: TokenInputValue,
    // The value parts last showed, `shown`.
    pub(crate) shown: TokenInputValue,
    // The selection, as value offsets, and which end moves.
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) direction: SelectionDirection,
    pub(crate) focused: bool,
    // The selection to restore once the editor shows the next value.
    pub(crate) pending_selection: Option<TextRange>,
    pub(crate) history: UndoStack<HistoryEntry>,
    pub(crate) messages: Vec<String>,
    pub(crate) clearing_messages: bool,
    // What the editor adapter reads, which the input keeps current.
    pub(crate) mirror: Rc<RefCell<EditorMirror>>,

    // Editing: where the last typing or deletion left the caret, whether the
    // last text typed ended in a space, a token the first Backspace selected,
    // and an input method's composition.
    pub(crate) last_end: Option<usize>,
    pub(crate) after_space: bool,
    pub(crate) armed: Option<SharedString>,
    pub(crate) composition: Option<Composition>,
    // Text a refused key would have typed, which the platform still sends.
    pub(crate) refused_text: Option<String>,

    // The view: the caret, scrolling, and the last frame's layout.
    pub(crate) upstream: bool,
    pub(crate) goal_x: Option<Pixels>,
    pub(crate) caret_visible: bool,
    pub(crate) blink_epoch: usize,
    pub(crate) blink_task: Option<Task<()>>,
    pub(crate) scroll: Point<Pixels>,
    pub(crate) autoscroll: bool,
    pub(crate) geometry: Option<Geometry>,
    pub(crate) drag: Option<Drag>,
    pub(crate) reported: Range<usize>,
    pub(crate) selection_pending: bool,
    pub(crate) a11y: Rc<RefCell<A11ySnapshot>>,

    pub(crate) internals: ElementInternals,
    _subscriptions: Vec<Subscription>,
}

impl TokenInput {
    pub fn new(options: TokenInputOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let mut passed = Normalized::default();
        let mut passed_default = Normalized::default();
        let initial = options
            .default_value
            .as_ref()
            .map_or_else(Vec::new, |value| passed_default.get(value));
        let current = options
            .value
            .as_ref()
            .map_or_else(|| initial.clone(), |value| passed.get(value));
        let mut value = Controllable::new("value", current.clone());
        value.set_controlled(options.value.is_some().then(|| current.clone()));
        let mut instance = Instance::new("token-input").with_agent(cx);
        Segment::register_data();
        instance.debug = options.debug;
        let internals = ElementInternals::new(cx);
        let history = create_undo_stack(
            HistoryEntry {
                value: current.clone(),
                selection: None,
            },
            options.history.unwrap_or_default(),
        );
        let this = cx.weak_entity();
        let focus_target = this.clone();
        let subscriptions = vec![
            focus_events::on_focus(&focus_handle, window, cx, Self::focused_in),
            focus_events::on_blur(&focus_handle, window, cx, Self::blurred),
            cx.observe_keystrokes(Self::keystroke),
            internals.on_reset(cx, |input, window, cx| input.reset(window, cx)),
            internals.on_disabled(cx, |input, disabled, _window, cx| {
                input.form_disabled = disabled;
                input.sync_form();
                cx.notify();
            }),
            // A submit that finds the input invalid focuses it with the caret
            // at its end, as `focus()` does.
            focus_with(&internals, move |window, cx| {
                focus_target
                    .update(cx, |input, cx| input.focus(FocusAt::End, window, cx))
                    .ok();
            }),
            // An input that leaves mid-composition leaves no composition behind.
            cx.on_release_in(window, |input, window, cx| {
                if input.composition.is_some() {
                    ui_sdk_kernel::set_composing(window, false, cx);
                }
            }),
        ];
        let end = value_length(&current);
        let mirror = Rc::new(RefCell::new(EditorMirror::new(focus_handle.clone(), &current)));
        let input = Self {
            instance,
            this,
            focus_handle,
            settings: Settings::of(&options),
            on_value_change: options.on_value_change.clone(),
            on_before_value_change: options.on_before_value_change.clone(),
            on_submit: options.on_submit.clone(),
            value,
            initial,
            has_default: options.default_value.is_some(),
            passed,
            passed_default,
            form_disabled: false,
            selected: Vec::new(),
            committed: current.clone(),
            shown: current,
            start: end,
            end,
            direction: SelectionDirection::None,
            focused: false,
            pending_selection: None,
            history,
            messages: Vec::new(),
            clearing_messages: false,
            mirror,
            last_end: None,
            after_space: false,
            armed: None,
            composition: None,
            refused_text: None,
            upstream: false,
            goal_x: None,
            caret_visible: false,
            blink_epoch: 0,
            blink_task: None,
            scroll: Point::default(),
            autoscroll: true,
            geometry: None,
            drag: None,
            reported: end..end,
            selection_pending: false,
            a11y: Rc::default(),
            internals,
            _subscriptions: subscriptions,
        };
        input.sync_form();
        input
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A new controlled value shows at once.
    pub fn set_options(&mut self, options: TokenInputOptions, window: &mut Window, cx: &mut Context<Self>) {
        let settings = Settings::of(&options);
        let mut changed = settings != self.settings;
        self.settings = settings;
        self.on_value_change = options.on_value_change.clone();
        self.on_before_value_change = options.on_before_value_change.clone();
        self.on_submit = options.on_submit.clone();
        self.history.set_options(options.history.unwrap_or_default());
        self.instance.debug = options.debug;
        if let Some(default) = &options.default_value {
            self.initial = self.passed_default.get(default);
        }
        self.has_default = options.default_value.is_some();
        let value = options.value.as_ref().map(|value| self.passed.get(value));
        changed |= self.value.set_controlled(value);
        changed |= self.value_changed(window, cx);
        self.sync_form();
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the input keep its own.
    pub fn set_controlled_value(
        &mut self,
        value: Option<TokenInputValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.as_ref().map(|value| self.passed.get(value));
        let mut changed = self.value.set_controlled(value);
        changed |= self.value_changed(window, cx);
        if changed {
            self.sync_form();
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// Whether the input is disabled, by itself, its field or a fieldset.
    pub fn is_disabled(&self) -> bool {
        ui_sdk_forms::is_disabled(self.settings.disabled, self.form_disabled)
            || self.internals.control_attrs().disabled
    }

    pub fn is_read_only(&self) -> bool {
        self.settings.read_only
    }

    pub fn is_required(&self) -> bool {
        self.settings.required
    }

    /// Whether typing, pasting and calls may change it: neither disabled nor
    /// read-only.
    pub fn is_editable(&self) -> bool {
        !self.is_disabled() && !self.settings.read_only
    }

    /// Whether an input method is composing in the editor.
    pub fn is_composing(&self) -> bool {
        self.composition.is_some()
    }

    /// Whether the editor has focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Its part in forms.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    pub(crate) fn token_state(&self, token: &Token) -> TokenInputTokenState {
        let selected = token
            .id
            .as_ref()
            .is_some_and(|id| self.selected.contains(id));
        TokenInputTokenState {
            token: token.clone(),
            selected,
            pending: token.status == Some(TokenStatus::Pending),
            unresolved: token.status == Some(TokenStatus::Unresolved),
            disabled: self.is_disabled(),
            read_only: self.settings.read_only,
        }
    }
}

impl Focusable for TokenInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// A segment as data: text, or a token's fields.
fn segment_json(segment: &Segment) -> AgentValue {
    match segment {
        Segment::Text(text) => AgentValue::String(text.to_string()),
        Segment::Token(token) => {
            let mut object = json!({
                "type": token.kind.to_string(),
                "value": token.value.to_string(),
                "label": token.label.to_string(),
            });
            if let Some(id) = &token.id {
                object["id"] = json!(id.to_string());
            }
            if let Some(status) = token.status {
                object["status"] = json!(status_name(status));
            }
            object
        }
    }
}

// A token from data: `{ type, value, label }`, with its id and status.
fn token_of(value: &AgentValue) -> Option<Token> {
    let object = value.as_object()?;
    let text = |key: &str| object.get(key).and_then(AgentValue::as_str).map(str::to_string);
    let kind = text("type")?;
    let token_value = text("value").unwrap_or_default();
    let label = text("label").unwrap_or_else(|| token_value.clone());
    let mut token = Token::new(kind, token_value, label);
    token.id = text("id").map(SharedString::from);
    token.status = match text("status").as_deref() {
        Some("pending") => Some(TokenStatus::Pending),
        Some("unresolved") => Some(TokenStatus::Unresolved),
        _ => None,
    };
    Some(token)
}

// A value from an agent's argument: codec text, or text and tokens as data.
fn value_of(args: &AgentArgs, index: usize, codec: &TokenCodec) -> Result<TokenInputValue, AgentError> {
    match args.value(index) {
        AgentValue::Array(items) => items
            .iter()
            .map(|item| match item {
                AgentValue::String(text) => Ok(Segment::Text(text.clone().into())),
                other => token_of(other).map(Segment::Token).ok_or_else(|| {
                    AgentError::refused("Each part is text or a token with a type, a value and a label.")
                }),
            })
            .collect(),
        AgentValue::Null => Ok(Vec::new()),
        _ => Ok(crate::token_input_model::from_text(&args.string(index)?, codec)),
    }
}

/// `agent: { actions: ["setValue", "insert", "removeToken", "clear"] }`, with
/// `value` controlled. A value comes as codec text, like
/// `Ask [@Ada](mention:ada)`, or as a list of texts and tokens.
impl Agent for TokenInput {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |input, args, window, cx| {
            let value = value_of(args, 0, &input.settings.codec)?;
            Ok(input.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("insert", |input, args, window, cx| {
            let content = match args.value(0) {
                AgentValue::Array(_) => {
                    crate::TokenInputContent::Value(value_of(args, 0, &input.settings.codec)?)
                }
                AgentValue::Object(_) => crate::TokenInputContent::Token(token_of(args.value(0)).ok_or_else(|| {
                    AgentError::refused("A token has a type, a value and a label.")
                })?),
                _ => crate::TokenInputContent::Text(args.string(0)?),
            };
            Ok(input
                .insert(content, crate::InsertOptions::default(), window, cx)
                .into())
        }),
        AgentAction::new("removeToken", |input, args, window, cx| {
            let id = args.string(0)?;
            Ok(input.remove_token(&id, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |input, _args, window, cx| {
            Ok(input.clear(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with(
                "value",
                AgentValue::Array(self.value.get().iter().map(segment_json).collect()),
            )
            .with("disabled", self.is_disabled())
            .with("readOnly", self.settings.read_only)
            .with("required", self.settings.required)
    }
}
