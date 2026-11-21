//! Port of `components/trigger-menu/trigger-menu.ts`: the trigger menu's
//! public types. Suggestions anchored in text, opened by a trigger like "@",
//! "/" or "#". It owns a range inside someone else's text, so it reads and
//! writes through an editor adapter: a native field, a token input or any
//! editor an app wraps. State and the api live in `trigger_menu_api.rs`, keys
//! in `trigger_menu_keyboard.rs`, and parts in `trigger_menu_parts.rs`, on the
//! listbox engine and the popup layer Combobox uses.

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use gpui::{App, Pixels, SharedString, Window};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentValue, ChangeDetails, ChangeOptions, ChangeSource, InputEvent, PENDING, Reason,
};
use ui_sdk_lists::{AsyncOptionsContext, AsyncOptionsStatus, ListData, ListFilter, ListGroup, ListItem};
use ui_sdk_platform::{Align, Anchor, Side};
use ui_sdk_popups::{DismissAttemptCallback, PopupContentProps, PopupContentState, PopupInitialFocus};
use ui_sdk_text::{EditorContent, PendingToken, TextEditorAdapter, TextRange};

use crate::trigger_match::{TriggerBoundary, TriggerMatch, TriggerRule, TriggerSpaces};
use crate::trigger_menu_api::{TriggerMenu, TriggerMenuOpenOptions};
use crate::trigger_menu_select::Writer;

/// What an item does once it, and any arguments it asks for, are chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TriggerAfter {
    /// Closes the menu.
    #[default]
    Compose,
    /// Keeps it open and lists the same trigger's items for the text typed
    /// after it.
    Continue,
    /// Asks the editor to send. Tab never submits.
    Submit,
}

impl TriggerAfter {
    /// The original's name for it, like `"compose"`.
    pub fn as_str(self) -> &'static str {
        match self {
            TriggerAfter::Compose => "compose",
            TriggerAfter::Continue => "continue",
            TriggerAfter::Submit => "submit",
        }
    }
}

/// What a suggestion holds beside a list item's value, label, keywords and
/// disabled state (`TriggerMenuDataItem`'s own fields).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriggerItemExtra {
    /// After choosing it: `Compose` closes the menu, `Continue` keeps it open
    /// and lists the same trigger's items for the text typed after it, and
    /// `Submit` asks the editor to send. Tab never submits. Defaults to
    /// `Compose`.
    pub after: TriggerAfter,
    /// Typed arguments asked for in order once it's chosen, like
    /// `/model <name>`.
    pub arguments: Vec<TriggerArgument>,
}

impl TriggerItemExtra {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn after(mut self, after: TriggerAfter) -> Self {
        self.after = after;
        self
    }

    pub fn arguments(mut self, arguments: impl IntoIterator<Item = TriggerArgument>) -> Self {
        self.arguments = arguments.into_iter().collect();
        self
    }
}

/// A suggestion, as data: a list item with what it does once chosen and the
/// arguments it asks for in its `extra`.
pub type TriggerMenuDataItem = ListItem<SharedString, TriggerItemExtra>;

/// A labelled group of suggestions, as data.
pub type TriggerMenuDataGroup = ListGroup<SharedString, TriggerItemExtra>;

/// Suggestions as data: items, plain strings, groups and separators.
pub type TriggerMenuData = ListData<SharedString, TriggerItemExtra>;

/// How typing narrows data items. Loaded items aren't filtered.
pub type TriggerMenuFilter = ListFilter<SharedString, TriggerItemExtra>;

/// The arguments typed or chosen for an item, by name, in the order they
/// were given (`Record<string, string>`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TriggerArguments(Vec<(SharedString, SharedString)>);

impl TriggerArguments {
    /// The value given for the argument `name`.
    pub fn get(&self, name: &str) -> Option<&SharedString> {
        self.0
            .iter()
            .find(|(own, _)| own.as_ref() == name)
            .map(|(_, value)| value)
    }

    /// Each argument's name and value, in the order they were given.
    pub fn iter(&self) -> impl Iterator<Item = (&SharedString, &SharedString)> {
        self.0.iter().map(|(name, value)| (name, value))
    }

    /// The values, in the order they were given (`Object.values`).
    pub fn values(&self) -> impl Iterator<Item = &SharedString> {
        self.0.iter().map(|(_, value)| value)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    // An argument's value, in its place if it had one, else last.
    pub(crate) fn set(&mut self, name: SharedString, value: SharedString) {
        match self.0.iter_mut().find(|(own, _)| *own == name) {
            Some(entry) => entry.1 = value,
            None => self.0.push((name, value)),
        }
    }

    pub(crate) fn remove(&mut self, name: &str) {
        self.0.retain(|(own, _)| own.as_ref() != name);
    }
}

/// An argument an item asks for once chosen, like Discord's options.
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerArgument {
    /// The name it's reported under in `details.arguments`.
    pub name: SharedString,
    /// Ghost text shown at the caret until it's typed. Defaults to `<name>`.
    pub hint: Option<SharedString>,
    /// Whether Enter can finish the command without it. Defaults to false.
    pub required: bool,
    /// Suggestions for it, as data or a loader. Without them, it's free text
    /// and no list shows.
    pub items: Option<TriggerItems>,
    /// Whether text that matches no suggestion is accepted, like a
    /// combobox's custom value. Defaults to true.
    pub allow_custom_value: bool,
}

impl TriggerArgument {
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self {
            name: name.into(),
            hint: None,
            required: false,
            items: None,
            allow_custom_value: true,
        }
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// Suggestions for it, as data or a loader.
    pub fn items(mut self, items: impl Into<TriggerItems>) -> Self {
        self.items = Some(items.into());
        self
    }

    pub fn allow_custom_value(mut self, allow: bool) -> Self {
        self.allow_custom_value = allow;
        self
    }

    // The ghost text for it.
    pub(crate) fn hint_text(&self) -> SharedString {
        self.hint.clone().unwrap_or_else(|| format!("<{}>", self.name).into())
    }
}

/// Loads suggestions for a query, like a search on a server: given the
/// query and a signal a newer query aborts, the work that answers with the
/// items, or why it failed. It runs once typing pauses.
pub type TriggerItemsLoader = Rc<
    dyn Fn(
        SharedString,
        AsyncOptionsContext,
        &mut App,
    ) -> LocalBoxFuture<'static, Result<TriggerMenuData, SharedString>>,
>;

/// What a trigger or an argument lists: data, or a loader given the query.
#[derive(Clone)]
pub enum TriggerItems {
    Data(TriggerMenuData),
    /// Loaded once typing pauses. A newer query aborts the older load, so
    /// only its results land, and the last results stay while the next ones
    /// load. Loaded items aren't filtered.
    Loader(TriggerItemsLoader),
}

impl TriggerItems {
    /// Suggestions loaded for the query once typing pauses.
    pub fn loader(
        load: impl Fn(
            SharedString,
            AsyncOptionsContext,
            &mut App,
        ) -> LocalBoxFuture<'static, Result<TriggerMenuData, SharedString>>
        + 'static,
    ) -> Self {
        TriggerItems::Loader(Rc::new(load))
    }

    pub(crate) fn is_loader(&self) -> bool {
        matches!(self, TriggerItems::Loader(_))
    }
}

impl fmt::Debug for TriggerItems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TriggerItems::Data(data) => f.debug_tuple("Data").field(data).finish(),
            TriggerItems::Loader(_) => write!(f, "Loader(..)"),
        }
    }
}

impl PartialEq for TriggerItems {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TriggerItems::Data(a), TriggerItems::Data(b)) => a == b,
            (TriggerItems::Loader(a), TriggerItems::Loader(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl From<TriggerMenuData> for TriggerItems {
    fn from(data: TriggerMenuData) -> Self {
        TriggerItems::Data(data)
    }
}

impl<N: Into<ui_sdk_lists::ListNode<SharedString, TriggerItemExtra>>> From<Vec<N>> for TriggerItems {
    fn from(nodes: Vec<N>) -> Self {
        TriggerItems::Data(TriggerMenuData::new(nodes))
    }
}

impl<N: Into<ui_sdk_lists::ListNode<SharedString, TriggerItemExtra>>, const LEN: usize> From<[N; LEN]>
    for TriggerItems
{
    fn from(nodes: [N; LEN]) -> Self {
        TriggerItems::Data(TriggerMenuData::new(nodes))
    }
}

/// What replaces the trigger and query when an item is chosen: given the
/// item, the match and the arguments, the text or token to insert.
pub type TriggerInsert = Rc<dyn Fn(&TriggerMenuDataItem, &TriggerMatch, &TriggerArguments) -> EditorContent>;

/// A trigger: where it matches, what it lists and what choosing inserts.
#[derive(Clone)]
pub struct TriggerDefinition {
    /// How it matches: its text, where it may start, and what its query may
    /// hold.
    pub rule: TriggerRule,
    /// The kind of token choosing an item inserts, where the editor takes
    /// tokens (`type`). Defaults to `"mention"`.
    pub kind: Option<SharedString>,
    /// Suggestions as data, or a loader given the query. Without them, the
    /// composed items show.
    pub items: Option<TriggerItems>,
    /// How typing narrows data items. Loaded items aren't filtered. Defaults
    /// to `Contains`.
    pub filter: Option<TriggerMenuFilter>,
    /// What replaces the trigger and query. Defaults to a token where the
    /// editor takes tokens, else the trigger, the label and a space.
    pub insert: Option<TriggerInsert>,
}

impl TriggerDefinition {
    /// A trigger for the text that starts a match, like `"@"`.
    pub fn new(char: impl Into<SharedString>) -> Self {
        Self {
            rule: TriggerRule::new(char),
            kind: None,
            items: None,
            filter: None,
            insert: None,
        }
    }

    pub fn boundary(mut self, boundary: TriggerBoundary) -> Self {
        self.rule.boundary = boundary;
        self
    }

    pub fn spaces(mut self, spaces: impl Into<TriggerSpaces>) -> Self {
        self.rule.spaces = spaces.into();
        self
    }

    pub fn pattern(mut self, pattern: impl Fn(char) -> bool + 'static) -> Self {
        self.rule.pattern = Some(Rc::new(pattern));
        self
    }

    pub fn max_length(mut self, max_length: usize) -> Self {
        self.rule.max_length = Some(max_length);
        self
    }

    /// The kind of token choosing an item inserts (`type`).
    pub fn kind(mut self, kind: impl Into<SharedString>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Suggestions as data or a loader.
    pub fn items(mut self, items: impl Into<TriggerItems>) -> Self {
        self.items = Some(items.into());
        self
    }

    pub fn filter(mut self, filter: TriggerMenuFilter) -> Self {
        self.filter = Some(filter);
        self
    }

    pub fn insert(
        mut self,
        insert: impl Fn(&TriggerMenuDataItem, &TriggerMatch, &TriggerArguments) -> EditorContent + 'static,
    ) -> Self {
        self.insert = Some(Rc::new(insert));
        self
    }
}

impl fmt::Debug for TriggerDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TriggerDefinition")
            .field("rule", &self.rule)
            .field("kind", &self.kind)
            .field("items", &self.items)
            .field("filter", &self.filter)
            .field("insert", &self.insert.as_ref().map(|_| ".."))
            .finish()
    }
}

/// Why a trigger menu opened or closed: `InputChange`, `CaretMove`,
/// `ItemPress`, `EscapeKey`, `ClosePress`, `OutsidePress`, `FocusOut`,
/// `WindowBlur`, `Imperative` or `Shortcut`.
pub type TriggerMenuOpenChangeReason = Reason;

/// The dismissals `dismissible` refuses that `on_dismiss_attempt` hears
/// about: `EscapeKey` and `OutsidePress`.
pub type TriggerMenuDismissReason = ui_sdk_kernel::DismissAttemptReason;

/// Why a trigger menu's query changed: `InputChange`, `ItemPress` or
/// `Imperative`.
pub type TriggerMenuQueryChangeReason = Reason;

/// Options for every write method.
pub type TriggerMenuChangeOptions = ChangeOptions;

/// What `on_select` receives besides the value.
pub struct TriggerMenuSelectDetails {
    /// Why it was chosen: `ItemPress`.
    pub reason: Reason,
    /// The trigger that opened the menu, like `"@"`.
    pub trigger: SharedString,
    /// The trigger, the query and their range in the text, which the
    /// insertion replaces.
    pub r#match: TriggerMatch,
    /// The chosen item.
    pub item: TriggerMenuDataItem,
    /// The arguments typed or chosen for it, by name.
    pub arguments: TriggerArguments,
    /// The key or click that chose it.
    pub event: Option<InputEvent>,
    /// How the user chose it.
    pub source: ChangeSource,
    pub(crate) writer: Writer,
    pub(crate) range: TextRange,
    pub(crate) prevented: Cell<bool>,
    pub(crate) inserted: Cell<bool>,
}

impl TriggerMenuSelectDetails {
    /// Replaces the match with your own text or token now, instead of the
    /// default insertion.
    pub fn insert(&self, content: impl Into<EditorContent>, window: &mut Window, cx: &mut App) {
        self.inserted.set(true);
        self.writer.write(self.range, content.into(), None, window, cx);
    }

    /// Skips the default insertion.
    pub fn prevent_default(&self) {
        self.prevented.set(true);
    }

    /// Whether `prevent_default()` was called, or `insert()` replaced the
    /// match. Read live, after both callbacks.
    pub fn default_prevented(&self) -> bool {
        self.prevented.get() || self.inserted.get()
    }
}

impl fmt::Debug for TriggerMenuSelectDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TriggerMenuSelectDetails")
            .field("reason", &self.reason)
            .field("trigger", &self.trigger)
            .field("match", &self.r#match)
            .field("item", &self.item)
            .field("arguments", &self.arguments)
            .field("event", &self.event)
            .field("source", &self.source)
            .field("default_prevented", &self.default_prevented())
            .finish()
    }
}

/// Runs when an item, with every required argument, is chosen, with its
/// value and the details. Return a future and a token is inserted at once as
/// pending, updated with what it resolves to and removed if it fails; the
/// menu closes either way. It runs inside the menu's update, so it may
/// update any entity but the menu.
pub type TriggerMenuSelectCallback =
    Rc<dyn Fn(&SharedString, &TriggerMenuSelectDetails, &mut Window, &mut App) -> Option<PendingToken>>;

/// Runs when the menu wants to open or close, with the trigger of the match,
/// like `"@"`. `details.reason` says why, and `details.cancel()` stops it.
pub type TriggerMenuOpenChangeCallback =
    Rc<dyn Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App)>;

/// Runs when the text after the trigger changes, with the trigger and the
/// argument being typed.
pub type TriggerMenuQueryChangeCallback =
    Rc<dyn Fn(&SharedString, &ChangeDetails, &SharedString, Option<&SharedString>, &mut Window, &mut App)>;

/// Runs once the menu has finished opening or closing, after any animation.
pub type TriggerMenuOpenChangeCompleteCallback = Rc<dyn Fn(bool, &mut Window, &mut App)>;

// `on_select` for a callback with nothing to wait for.
fn select_callback(
    callback: impl Fn(&SharedString, &TriggerMenuSelectDetails, &mut Window, &mut App) + 'static,
) -> TriggerMenuSelectCallback {
    Rc::new(move |value, details, window, cx| {
        callback(value, details, window, cx);
        None
    })
}

/// The options `TriggerMenu` takes.
pub struct TriggerMenuOptions {
    /// The texts that open the menu, and what each one lists.
    pub triggers: Vec<TriggerDefinition>,
    /// The editor the menu reads and writes, for editors without a part of
    /// their own. The input part sets its own, which wins.
    pub editor: Option<Rc<dyn TextEditorAdapter>>,
    /// Whether the menu is open. Pass it to control it; it shows only while
    /// there's a match.
    pub open: Option<bool>,
    /// Runs when the menu wants to open or close, with the trigger.
    /// `details.reason` is one of [`TriggerMenuOpenChangeReason`]'s, and
    /// `details.cancel()` stops it. It runs inside the menu's update, so it
    /// may update any entity but the menu.
    pub on_open_change: Option<TriggerMenuOpenChangeCallback>,
    /// Runs once the menu has finished opening or closing, after any
    /// animation.
    pub on_open_change_complete: Option<TriggerMenuOpenChangeCompleteCallback>,
    /// Runs when the text after the trigger changes, with the trigger and the
    /// argument being typed. Read-only: it's the editor's text.
    /// `details.reason` is `InputChange`, `ItemPress` or `Imperative`.
    pub on_query_change: Option<TriggerMenuQueryChangeCallback>,
    /// Runs when an item, with every required argument, is chosen. The
    /// default insertion follows unless `details.prevent_default()`. Return
    /// a future and a token is inserted at once as pending, updated with
    /// what it resolves to and removed if it fails; the menu closes either
    /// way.
    pub on_select: Option<TriggerMenuSelectCallback>,
    /// What screen readers hear once an `on_select` future has been pending
    /// for a second, so a slow action doesn't pass in silence. Set it in the
    /// app's language. Empty says nothing. Defaults to "Loading".
    pub pending_announcement: Option<SharedString>,
    /// Whether items that come from outside are loading or failed, for the
    /// status and empty parts. Loaders report their own. Defaults to `Idle`.
    pub status: Option<AsyncOptionsStatus>,
    /// Whether the first match is highlighted so Enter and Tab choose it.
    /// Visual only: screen readers hear about it once an arrow key moves the
    /// highlight, so typed characters keep being echoed. Defaults to true.
    pub auto_highlight: bool,
    /// Whether arrow keys wrap from the last item to the first. Defaults to
    /// true.
    pub r#loop: bool,
    /// Reopens a match dismissed with Escape, with the caret in it. Defaults
    /// to `"ctrl+space"`; empty turns it off.
    pub shortcut: Option<SharedString>,
    /// Whether Escape and an outside press can close it. Defaults to true.
    pub dismissible: bool,
    /// Runs when `dismissible` refused a dismissal made on purpose, with
    /// [`TriggerMenuDismissReason`]'s reasons.
    pub on_dismiss_attempt: Option<DismissAttemptCallback>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for TriggerMenuOptions {
    fn default() -> Self {
        Self {
            triggers: Vec::new(),
            editor: None,
            open: None,
            on_open_change: None,
            on_open_change_complete: None,
            on_query_change: None,
            on_select: None,
            pending_announcement: None,
            status: None,
            auto_highlight: true,
            r#loop: true,
            shortcut: None,
            dismissible: true,
            on_dismiss_attempt: None,
            debug: false,
        }
    }
}

impl Clone for TriggerMenuOptions {
    fn clone(&self) -> Self {
        Self {
            triggers: self.triggers.clone(),
            editor: self.editor.clone(),
            open: self.open,
            on_open_change: self.on_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_query_change: self.on_query_change.clone(),
            on_select: self.on_select.clone(),
            pending_announcement: self.pending_announcement.clone(),
            status: self.status,
            auto_highlight: self.auto_highlight,
            r#loop: self.r#loop,
            shortcut: self.shortcut.clone(),
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            debug: self.debug,
        }
    }
}

impl TriggerMenuOptions {
    pub fn triggers(mut self, triggers: impl IntoIterator<Item = TriggerDefinition>) -> Self {
        self.triggers = triggers.into_iter().collect();
        self
    }

    pub fn editor(mut self, editor: Rc<dyn TextEditorAdapter>) -> Self {
        self.editor = Some(editor);
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn on_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn on_query_change(
        mut self,
        callback: impl Fn(&SharedString, &ChangeDetails, &SharedString, Option<&SharedString>, &mut Window, &mut App)
        + 'static,
    ) -> Self {
        self.on_query_change = Some(Rc::new(callback));
        self
    }

    /// `on_select`, for a callback with nothing to wait for.
    pub fn on_select(
        mut self,
        callback: impl Fn(&SharedString, &TriggerMenuSelectDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(select_callback(callback));
        self
    }

    /// `on_select`, for a callback that returns work the token it inserts
    /// waits for.
    pub fn on_select_pending(
        mut self,
        callback: impl Fn(&SharedString, &TriggerMenuSelectDetails, &mut Window, &mut App) -> Option<PendingToken> + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(callback));
        self
    }

    pub fn pending_announcement(mut self, text: impl Into<SharedString>) -> Self {
        self.pending_announcement = Some(text.into());
        self
    }

    pub fn status(mut self, status: AsyncOptionsStatus) -> Self {
        self.status = Some(status);
        self
    }

    pub fn auto_highlight(mut self, auto_highlight: bool) -> Self {
        self.auto_highlight = auto_highlight;
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(
        mut self,
        callback: impl Fn(&ui_sdk_kernel::DismissAttemptDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// Props of the content.
#[derive(Clone, Default)]
pub struct TriggerMenuContentProps {
    /// Which side of the trigger the list opens on. Defaults to `Bottom`.
    pub side: Option<Side>,
    /// How the list lines up with the trigger along that side. Defaults to
    /// `Start`.
    pub align: Option<Align>,
    /// Gap between the line of text and the list. Defaults to 4.
    pub side_offset: Option<Pixels>,
    /// Shift along the text. Defaults to 0.
    pub align_offset: Option<Pixels>,
    /// Space to keep from the window's edges. Defaults to 8.
    pub collision_padding: Option<Pixels>,
    /// Space to keep between an arrow and the list's corners. Defaults to 6.
    pub arrow_padding: Option<Pixels>,
    /// Positions the list against this instead of the trigger in the text,
    /// like the whole field.
    pub anchor: Option<Anchor>,
    /// Whether the list stays in the page, hidden, while closed. Defaults to
    /// false.
    pub keep_mounted: bool,
}

impl TriggerMenuContentProps {
    pub fn side(mut self, side: Side) -> Self {
        self.side = Some(side);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = Some(align);
        self
    }

    pub fn side_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.side_offset = Some(offset.into());
        self
    }

    pub fn align_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.align_offset = Some(offset.into());
        self
    }

    pub fn collision_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.collision_padding = Some(padding.into());
        self
    }

    pub fn arrow_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.arrow_padding = Some(padding.into());
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.anchor = Some(anchor.into());
        self
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    // The popup's content props: placed at the text, and focus stays in the
    // editor.
    pub(crate) fn popup(&self) -> PopupContentProps {
        let mut props = PopupContentProps::default()
            .keep_mounted(self.keep_mounted)
            .initial_focus(PopupInitialFocus::None);
        props.position.side = self.side;
        props.position.align = self.align;
        props.position.side_offset = self.side_offset;
        props.position.align_offset = self.align_offset;
        props.position.collision_padding = self.collision_padding;
        props.position.arrow_padding = self.arrow_padding;
        props.position.anchor = self.anchor.clone();
        props
    }
}

/// Props of an item.
#[derive(Clone)]
pub struct TriggerMenuItemProps {
    /// What `on_select` receives when the item is chosen, and its key.
    pub value: SharedString,
    /// The text inserted, matched and named by. gpui can't read the text an
    /// item shows, so without it the value is.
    pub label: Option<SharedString>,
    /// More words filtering matches.
    pub keywords: Vec<SharedString>,
    /// Keeps the item from being chosen. It can still be highlighted, as the
    /// listbox pattern asks.
    pub disabled: bool,
    /// Runs before the menu's `on_select`. `details.prevent_default()` skips
    /// the default insertion.
    pub on_select: Option<TriggerMenuSelectCallback>,
    /// The key of the group it's in, which the app passes from the group
    /// around it.
    pub group: Option<SharedString>,
}

impl TriggerMenuItemProps {
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: None,
            keywords: Vec::new(),
            disabled: false,
            on_select: None,
            group: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn keywords(mut self, keywords: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// `on_select`, for a callback with nothing to wait for.
    pub fn on_select(
        mut self,
        callback: impl Fn(&SharedString, &TriggerMenuSelectDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(select_callback(callback));
        self
    }

    /// `on_select`, for a callback that returns work the token it inserts
    /// waits for.
    pub fn on_select_pending(
        mut self,
        callback: impl Fn(&SharedString, &TriggerMenuSelectDetails, &mut Window, &mut App) -> Option<PendingToken> + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(callback));
        self
    }

    pub fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }
}

/// What the `input` part styles from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerMenuInputState {
    /// Whether the menu is open (`data-open`). It opens on a typed trigger,
    /// and closes when nothing matches unless it's loading or renders an
    /// empty part.
    pub open: bool,
    /// The trigger of the current match, like `"@"`, or none
    /// (`data-trigger`).
    pub trigger: Option<SharedString>,
}

/// What the `content` and `list` parts style from.
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerMenuContentState {
    /// Whether the menu is open. False while it animates out.
    pub open: bool,
    /// The trigger of the current match, like `"@"`, or none
    /// (`data-trigger`).
    pub trigger: Option<SharedString>,
    /// The text after the trigger, or after the current argument's start.
    pub query: SharedString,
    /// The argument being typed, by name, or none at the first level.
    pub argument: Option<SharedString>,
    /// Whether items are loading or failed (`aria-busy` while loading).
    pub status: AsyncOptionsStatus,
    /// The visible data items, for rendering them yourself.
    pub items: TriggerMenuData,
    /// Whether some item holds an icon (`data-has-icons`), so items without
    /// one can keep its space.
    pub has_icons: bool,
    /// The popup's own state on the content: its presence, where it's
    /// placed, and whether it's kept hidden. `None` on the list.
    pub popup: Option<PopupContentState<()>>,
}

/// What the `item` part styles from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriggerMenuItemState {
    /// Whether the item is highlighted, by the keyboard or the pointer
    /// (`data-highlighted`).
    pub highlighted: bool,
    /// Whether the item can't be chosen (`data-disabled`).
    pub disabled: bool,
    /// Whether it holds an icon part (`data-has-icon`).
    pub has_icon: bool,
}

/// What the `empty` part styles from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerMenuEmptyState {
    /// Whether items are loading (`data-loading`).
    pub loading: bool,
    /// Whether no item matches (`data-empty`).
    pub empty: bool,
    /// The text after the trigger.
    pub query: SharedString,
}

/// What the `status` part styles from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerMenuStatusState {
    /// Whether the menu is open. It opens on a typed trigger, and closes
    /// when nothing matches unless it's loading or renders an empty part.
    pub open: bool,
    /// Whether items are loading or failed.
    pub status: AsyncOptionsStatus,
    /// Whether items are loading.
    pub loading: bool,
    /// How many items match.
    pub count: usize,
    /// The argument being typed, by name, or none.
    pub argument: Option<SharedString>,
}

/// What the `hint` part styles from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerMenuHintState {
    /// The argument being typed, by name, or none.
    pub argument: Option<SharedString>,
    /// The ghost text: the hints of the arguments still to type.
    pub hint: SharedString,
}

/// `agent: { actions: ["setOpen", "insert"] }` and `controlled: ["open"]`,
/// with `open` and `close`, which `setOpen` covers, unlisted.
impl Agent for TriggerMenu {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", |menu, args, window, cx| {
            let open = args.boolean(0)?;
            Ok(menu.set_open(open, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("insert", |menu, args, window, cx| {
            let text = args.string(0)?;
            menu.insert(EditorContent::Text(text), window, cx);
            Ok(AgentValue::Null)
        }),
        AgentAction::unlisted("open", |menu, args, window, cx| {
            let mut options = TriggerMenuOpenOptions::default();
            if !args.is_missing(0) {
                options.trigger = Some(args.string(0)?);
            }
            menu.open(options, window, cx);
            Ok(PENDING.into())
        }),
        AgentAction::unlisted("close", |menu, _args, window, cx| {
            Ok(menu.close(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["open"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("open", self.is_open())
            .with("query", self.get_query().to_string())
            .with("pending", self.is_pending())
    }
}
