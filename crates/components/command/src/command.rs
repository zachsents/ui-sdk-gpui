//! Port of `components/command/command.ts`: Command, a search field over an
//! always-open list of commands, like a command palette. Typing filters and
//! ranks the commands, arrow keys move a highlight while focus stays in the
//! field, Enter runs the highlighted one, and commands can open pages of
//! more commands. It's built on the listbox engine Combobox uses.
//!
//! This module holds the public types and the entity, with its options and
//! agent surface; the api is in `command_api.rs`, and parts in
//! `command_parts.rs`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, ScrollHandle, SharedString, Subscription,
    Window,
};
use ui_sdk_kernel::{
    ActionOutcome, Agent, AgentAction, AgentState, AgentValue, AnnouncePending, ChangeCallback, ChangeDetails,
    ChangeOptions, Controllable, InputEvent, Instance, Reason, SelectEvent, flag_while_bound,
};
use ui_sdk_lists::{
    AsyncOptionsStatus, AutoHighlight, AutocompleteCore, AutocompleteCoreConfig, AutocompleteCoreState,
    HighlightCause, ItemTexts, ListAnnouncementFn, ListScroller, ListSettled, LoadMoreFn, SearchFilter,
    create_autocomplete_core,
};
use ui_sdk_native::{TextField, TextFieldEvent, TextFieldOptions, TextFieldStyle};

use crate::command_data::{CommandData, CommandDataFields, CommandDataItem, page_items};

/// Why the highlighted command changed, from the shared vocabulary:
/// `ListNavigation`, `ItemHover`, `InputChange` (typing highlights the best
/// match), `ItemPress` and `Back` (moving to another page highlights its best
/// match), `Shortcut`, `EscapeKey`, `ItemRemoved` and `Imperative`.
pub type CommandValueChangeReason = Reason;

/// Why the search text changed: `InputChange`, `ItemPress` and `Shortcut`
/// (opening a page empties it), `Back` and `EscapeKey` (going back puts that
/// page's text back, and Escape clears it), and `Imperative`.
pub type CommandQueryChangeReason = Reason;

/// Why the stack of pages changed: a command opened one (`ItemPress`,
/// `Shortcut`), Backspace or Escape went back (`Back`, `EscapeKey`), or a
/// call (`Imperative`).
pub type CommandPagesChangeReason = Reason;

/// Options for every write method: the reason and event the change's
/// callback receives.
pub type CommandChangeOptions = ChangeOptions;

/// What `on_value_change` receives besides the value: the reason, source and
/// event, with `cancel()`, as every change's details have, and where the
/// highlighted command is.
pub struct CommandValueChangeDetails<'a> {
    details: &'a ChangeDetails,
    /// The command's index among the visible commands, for a virtualized
    /// list to scroll to. `None` for none (`-1`).
    pub index: Option<usize>,
}

impl<'a> CommandValueChangeDetails<'a> {
    pub(crate) fn new(details: &'a ChangeDetails, index: Option<usize>) -> Self {
        Self { details, index }
    }

    /// The change's own details, with its reason, source and event.
    pub fn details(&self) -> &ChangeDetails {
        self.details
    }
}

impl Deref for CommandValueChangeDetails<'_> {
    type Target = ChangeDetails;

    fn deref(&self) -> &ChangeDetails {
        self.details
    }
}

impl fmt::Debug for CommandValueChangeDetails<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandValueChangeDetails")
            .field("details", self.details)
            .field("index", &self.index)
            .finish()
    }
}

/// What `on_action` receives besides the command's value.
#[derive(Clone, Debug, Default)]
pub struct CommandActionDetails {
    /// The click or key press that ran it. `None` for a call's `run()`.
    pub event: Option<InputEvent>,
}

/// `on_value_change`: the highlighted command and where it is.
pub type CommandValueChange =
    Rc<dyn Fn(&Option<SharedString>, &CommandValueChangeDetails<'_>, &mut Window, &mut App)>;

/// `on_action`: the command that ran. Returning `ActionOutcome::Pending`
/// marks the command pending until its work settles.
pub type CommandAction = Rc<dyn Fn(&SharedString, &CommandActionDetails, &mut Window, &mut App) -> ActionOutcome>;

/// A command's `on_select`: the event that ran it, whose `prevent_default()`
/// skips `on_action` and the page. Returning `ActionOutcome::Pending` marks
/// the command pending until its work settles.
pub type CommandSelect = Rc<dyn Fn(&SelectEvent, &mut Window, &mut App) -> ActionOutcome>;

/// The options `Command` takes.
#[derive(Clone)]
pub struct CommandOptions {
    /// The highlighted command's value. Pass `Some` to control the highlight;
    /// `Some(None)` highlights nothing.
    pub value: Option<Option<SharedString>>,
    /// The command highlighted at first, when uncontrolled. Without it, the
    /// first command is, with `auto_highlight`.
    pub default_value: Option<SharedString>,
    /// Runs when the highlight moves: arrow keys, the pointer, typing, a page
    /// change or a call. `details.index` says where the command is, for
    /// virtualized lists. It runs once the command's update is over, so it
    /// may update any entity, the command too.
    pub on_value_change: Option<CommandValueChange>,
    /// Runs as the highlight is asked to move, before it does, inside the
    /// command's update: `details.cancel()` keeps it where it was. It may
    /// update any entity but the command.
    pub on_before_value_change: Option<CommandValueChange>,
    /// The search text, which filters the commands. Pass it to control it.
    pub query: Option<SharedString>,
    /// The search text at first, when uncontrolled.
    pub default_query: SharedString,
    /// Runs when the search text wants to change: typing, opening a page,
    /// going back, or a call. It runs inside the command's update.
    pub on_query_change: Option<ChangeCallback<SharedString>>,
    /// The stack of open pages, from the first page opened to the current
    /// one. Pass it to control it.
    pub pages: Option<Vec<SharedString>>,
    /// The pages open at first, when uncontrolled. None by default.
    pub default_pages: Vec<SharedString>,
    /// Runs when a page opens or Backspace or Escape goes back. It runs
    /// inside the command's update.
    pub on_pages_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Runs when a command runs, with its value: Enter, a click, or its
    /// shortcut. Return `ActionOutcome::Pending` to mark the command pending
    /// until it settles: it's pending, busy and disabled, and running it
    /// again waits. A failure puts the reason in its `error` state until the
    /// next run. It runs inside the command's update.
    pub on_action: Option<CommandAction>,
    /// What screen readers hear once a command has been pending for a
    /// second, so a slow action doesn't pass in silence. Set it in the app's
    /// language. Empty says nothing. "Loading" by default.
    pub pending_announcement: Option<SharedString>,
    /// The commands as data, ranked as the user types, with pages for
    /// commands that have `items`. Render them from the list's
    /// `state.items`.
    pub items: Option<CommandData>,
    /// Commands shown instead of `items` on the first page while the search
    /// is empty, like recent or suggested ones. Needs `items`.
    pub empty_items: Option<CommandData>,
    /// How typing narrows the list: `Fuzzy` keeps commands with the text's
    /// letters in order, best matches first; `Contains` and `StartsWith`
    /// match the text itself. A function returns a score (0 hides) or a
    /// boolean. `Off` keeps every command, for results a server already
    /// filtered. Composed commands keep their order, and data commands are
    /// ranked by score. `Fuzzy` by default.
    pub filter: SearchFilter<CommandDataFields>,
    /// Whether the first command is highlighted while nothing else is, and
    /// the best match as the user types, so Enter runs it. Defaults to true.
    pub auto_highlight: bool,
    /// Whether arrow keys wrap from the last command to the first. Defaults
    /// to true.
    pub r#loop: bool,
    /// Whether results are loading or failed, for the empty and status parts.
    /// The list is busy while loading.
    pub status: AsyncOptionsStatus,
    /// Runs when the highlight or scrolling reaches the last command, so the
    /// next page can load. Not again for the same commands, unless the last
    /// load failed.
    pub on_load_more: Option<LoadMoreFn>,
    /// What screen readers hear after typing pauses, when no status part is
    /// rendered. `None` from it says nothing. "3 results", "No results" or
    /// "Loading" by default.
    pub announcement: Option<ListAnnouncementFn>,
    /// The search field's placeholder.
    pub placeholder: Option<SharedString>,
    /// The colors the search field's text, caret, selection and placeholder
    /// paint with.
    pub field_style: TextFieldStyle,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for CommandOptions {
    fn default() -> Self {
        Self {
            value: None,
            default_value: None,
            on_value_change: None,
            on_before_value_change: None,
            query: None,
            default_query: SharedString::default(),
            on_query_change: None,
            pages: None,
            default_pages: Vec::new(),
            on_pages_change: None,
            on_action: None,
            pending_announcement: None,
            items: None,
            empty_items: None,
            filter: SearchFilter::Fuzzy,
            auto_highlight: true,
            r#loop: true,
            status: AsyncOptionsStatus::Idle,
            on_load_more: None,
            announcement: None,
            placeholder: None,
            field_style: TextFieldStyle::default(),
            debug: false,
        }
    }
}

impl CommandOptions {
    pub fn value(mut self, value: Option<SharedString>) -> Self {
        self.value = Some(value);
        self
    }

    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&Option<SharedString>, &CommandValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&Option<SharedString>, &CommandValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn query(mut self, query: impl Into<SharedString>) -> Self {
        self.query = Some(query.into());
        self
    }

    pub fn default_query(mut self, query: impl Into<SharedString>) -> Self {
        self.default_query = query.into();
        self
    }

    pub fn on_query_change(
        mut self,
        callback: impl Fn(&SharedString, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_query_change = Some(Rc::new(callback));
        self
    }

    pub fn pages(mut self, pages: Vec<SharedString>) -> Self {
        self.pages = Some(pages);
        self
    }

    pub fn default_pages(mut self, pages: Vec<SharedString>) -> Self {
        self.default_pages = pages;
        self
    }

    pub fn on_pages_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_pages_change = Some(Rc::new(callback));
        self
    }

    pub fn on_action(
        mut self,
        action: impl Fn(&SharedString, &CommandActionDetails, &mut Window, &mut App) -> ActionOutcome + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(action));
        self
    }

    pub fn pending_announcement(mut self, text: impl Into<SharedString>) -> Self {
        self.pending_announcement = Some(text.into());
        self
    }

    pub fn items(mut self, items: CommandData) -> Self {
        self.items = Some(items);
        self
    }

    pub fn empty_items(mut self, items: CommandData) -> Self {
        self.empty_items = Some(items);
        self
    }

    pub fn filter(mut self, filter: SearchFilter<CommandDataFields>) -> Self {
        self.filter = filter;
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

    pub fn status(mut self, status: AsyncOptionsStatus) -> Self {
        self.status = status;
        self
    }

    pub fn on_load_more(mut self, load: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_load_more = Some(Rc::new(load));
        self
    }

    pub fn announcement(
        mut self,
        announcement: impl Fn(&ui_sdk_lists::ListAnnouncement) -> Option<SharedString> + 'static,
    ) -> Self {
        self.announcement = Some(Rc::new(announcement));
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn field_style(mut self, style: TextFieldStyle) -> Self {
        self.field_style = style;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props of a command a part renders (`CommandItemProps`).
#[derive(Clone, Default)]
pub struct CommandItemProps {
    /// Passed to `on_action` and `on_value_change`, and identifies the
    /// command.
    pub value: SharedString,
    /// The text the search matches, and the command's name. Without it, the
    /// value.
    pub label: Option<SharedString>,
    /// More words the search matches.
    pub keywords: Vec<SharedString>,
    /// Keeps the command from running. Arrow keys and the pointer skip it.
    pub disabled: bool,
    /// A keyboard shortcut that runs the command while it's in the page,
    /// like `"mod+p"`. It's the command's `aria-keyshortcuts`, and a
    /// shortcut part inside shows it.
    pub shortcut: Option<SharedString>,
    /// The page choosing this command opens, by a page part's value.
    pub page: Option<SharedString>,
    /// The command's position among all visible commands, for virtualized
    /// lists that render only some. Its `aria-posinset` and `aria-setsize`.
    pub index: Option<usize>,
    /// Runs when the command runs, before `on_action`, with the click or key
    /// press that ran it, or a new select event for a shortcut or a call's
    /// `run()`. `prevent_default()` skips `on_action` and the page. Returning
    /// `ActionOutcome::Pending` marks the command pending, like `on_action`.
    pub on_select: Option<CommandSelect>,
    /// The group it's in, by the group part's key.
    pub group: Option<SharedString>,
    /// Rendered from the data, whose shortcuts the root binds.
    pub data: bool,
    /// It's a link, which the app follows in its own press on the part
    /// (`on_press`): Enter clicks it, so it follows itself and the press
    /// runs it. Pressed while disabled or pending, or canceled by
    /// `on_select`, it calls `window.prevent_default()`, so the app's press
    /// doesn't follow it.
    pub link: bool,
}

impl CommandItemProps {
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            ..Self::default()
        }
    }

    /// The props of a command from the data, with its shortcut, which the
    /// root binds.
    pub fn from_data(item: &CommandDataItem) -> Self {
        Self {
            value: item.value.clone(),
            label: Some(item.label.clone()),
            keywords: item.keywords.clone(),
            disabled: item.disabled,
            shortcut: item.extra.shortcut.clone(),
            data: true,
            ..Self::default()
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn keywords<K: Into<SharedString>>(mut self, keywords: impl IntoIterator<Item = K>) -> Self {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn page(mut self, page: impl Into<SharedString>) -> Self {
        self.page = Some(page.into());
        self
    }

    pub fn index(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }

    pub fn on_select(mut self, select: impl Fn(&SelectEvent, &mut Window, &mut App) -> ActionOutcome + 'static) -> Self {
        self.on_select = Some(Rc::new(select));
        self
    }

    pub fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn data(mut self, data: bool) -> Self {
        self.data = data;
        self
    }

    pub fn link(mut self, link: bool) -> Self {
        self.link = link;
        self
    }
}

/// What the `root` part's `with` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandState {
    /// The current page, or `None` on the first page.
    pub page: Option<SharedString>,
    /// Whether results are loading.
    pub loading: bool,
}

/// What the `input` part's `with` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandInputState {
    /// The current page, or `None` on the first page.
    pub page: Option<SharedString>,
}

/// What the `list` part's `with` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandListState {
    /// Whether results are loading or failed.
    pub status: AsyncOptionsStatus,
    /// The visible data commands, ranked, for rendering them, or a
    /// virtualized list, yourself.
    pub items: CommandData,
}

/// What the `item` part's `with` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandItemState {
    /// Whether the command is highlighted, by the keyboard, the pointer or
    /// typing.
    pub highlighted: bool,
    /// Whether the command can't run.
    pub disabled: bool,
    /// Whether its action is running.
    pub pending: bool,
    /// Why its last action failed, until it runs again.
    pub error: Option<SharedString>,
}

/// What the `shortcut` part's `with` reads (`CommandShortcutState`).
pub type CommandShortcutState = ui_sdk_shortcuts::ShortcutState;

/// What a shortcut part takes (`CommandShortcutProps`).
pub type CommandShortcutProps = ui_sdk_shortcuts::ShortcutProps;

/// A command palette. Its entity is its controller.
pub struct Command {
    pub(crate) instance: Instance,
    pub(crate) core: AutocompleteCore<Self, CommandDataFields>,
    pub(crate) field: Entity<TextField>,
    pub(crate) scroll: ScrollHandle,
    pub(crate) settled: ListSettled,

    // State.
    // The highlighted command's value, which is its key. The highlight
    // follows it.
    pub(crate) value: Controllable<Option<SharedString>>,
    pub(crate) query: Controllable<SharedString>,
    pub(crate) pages: Controllable<Vec<SharedString>>,
    // The search text of each page below the current one, put back on the
    // way back.
    pub(crate) queries: Vec<SharedString>,
    pub(crate) items: Option<CommandData>,
    pub(crate) status: AsyncOptionsStatus,
    // The commands whose action is running, and the last one that failed.
    pub(crate) pending: Vec<SharedString>,
    pub(crate) error: Option<(SharedString, SharedString)>,

    // Options.
    pub(crate) on_value_change: Option<CommandValueChange>,
    pub(crate) on_before_value_change: Option<CommandValueChange>,
    pub(crate) on_action: Option<CommandAction>,
    pub(crate) pending_announcement: Option<SharedString>,
    pub(crate) empty_items: Option<CommandData>,
    pub(crate) filter: SearchFilter<CommandDataFields>,
    pub(crate) auto_highlight: bool,
    pub(crate) r#loop: bool,
    pub(crate) on_load_more: Option<LoadMoreFn>,
    pub(crate) announcement: Option<ListAnnouncementFn>,
    pub(crate) placeholder: Option<SharedString>,
    pub(crate) field_style: TextFieldStyle,

    // The cause of the next page change, for the highlight that follows it.
    pub(crate) page_cause: Option<HighlightCause>,
    // The cause of a composed page change, until its commands render.
    pub(crate) awaiting: Option<HighlightCause>,
    pub(crate) previous_value: Option<SharedString>,
    pub(crate) previous_pages: Vec<SharedString>,
    // How many times commands had joined or left the list, as last seen.
    pub(crate) previous_mounts: u64,
    // What each command part rendered with last.
    pub(crate) item_props: RefCell<HashMap<SharedString, CommandItemProps>>,
    pub(crate) texts: ItemTexts,
    // The text last written into the search field, so only new text is.
    pub(crate) written: SharedString,
    // Whether the search field rendered, which starts the check that
    // something names it.
    pub(crate) input_bound: bool,
    pub(crate) announce_pending: AnnouncePending,
    _subscriptions: Vec<Subscription>,
}

// The search field: a text input whose completion, capitals and spelling
// marks are off, as the field part has them.
fn field_options(placeholder: Option<SharedString>, style: TextFieldStyle, value: SharedString) -> TextFieldOptions {
    TextFieldOptions {
        default_value: value,
        placeholder,
        style,
        ..TextFieldOptions::default()
    }
}

impl Command {
    pub fn new(options: CommandOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("command").with_agent(cx);
        instance.debug = options.debug;
        let current_value = match &options.value {
            Some(value) => value.clone(),
            None => options.default_value.clone(),
        };
        let mut value = Controllable::new("value", current_value).after_update();
        value.set_controlled(options.value.clone());
        let mut query = Controllable::new("query", options.query.clone().unwrap_or(options.default_query.clone()));
        query.set_controlled(options.query.clone());
        query.set_on_change(options.on_query_change.clone());
        let mut pages = Controllable::new("pages", options.pages.clone().unwrap_or(options.default_pages.clone()));
        pages.set_controlled(options.pages.clone());
        pages.set_on_change(options.on_pages_change.clone());
        let written = query.get().clone();
        let field = cx.new(|cx| {
            TextField::new(
                field_options(options.placeholder.clone(), options.field_style, written.clone()),
                window,
                cx,
            )
        });
        let core = create_autocomplete_core(
            AutocompleteCoreConfig {
                id: instance.id().clone(),
                state: Rc::new(|this: &Command| AutocompleteCoreState {
                    // Inline, so always open.
                    open: true,
                    status: this.status,
                    settled: this.settled.get(),
                    // Inline, so focus never has to move into the list.
                    focus_in_list: false,
                }),
                query: Rc::new(|this: &Command| this.query.get().clone()),
                items: Rc::new(|this: &Command| this.page_items()),
                // Commands shown on the first page while the search is empty.
                empty_items: Some(Rc::new(|this: &Command| {
                    if this.pages.get().is_empty() {
                        this.empty_items.clone()
                    } else {
                        None
                    }
                })),
                filter: Rc::new(|this: &Command| this.filter.clone()),
                auto_highlight: Rc::new(|this: &Command| {
                    if this.auto_highlight {
                        AutoHighlight::Always
                    } else {
                        AutoHighlight::Off
                    }
                }),
                highlight: Rc::new(|this: &mut Command, key, why, window, cx| {
                    this.highlight(key, why, window, cx);
                }),
                preferred: None,
                open: None,
                on_highlight: None,
                container: None,
                scroll_root: Some(Rc::new(|this: &Command| Some(ListScroller::new(this.scroll.clone())))),
                // A composed page that just changed highlights its best match
                // once its commands render, for the page change's reason, not
                // for the commands that left.
                settles_mount: Some(Rc::new(|this: &Command| this.awaiting.is_some())),
                r#loop: Some(Rc::new(|this: &Command| this.r#loop)),
                on_load_more: Some(Rc::new(|this: &Command| this.on_load_more.clone())),
                announcement: Some(Rc::new(|this: &Command| this.announcement.clone())),
                silent: Some(Rc::new(|this: &Command| {
                    flag_while_bound(&this.instance, "status").unwrap_or(false)
                })),
            },
            window,
            cx,
        );
        let this = cx.entity();
        let subscriptions = vec![
            cx.subscribe_in(&field, window, |command: &mut Self, _field, event, window, cx| {
                command.field_event(event, window, cx);
            }),
            // Items rendering and leaving notify the command, which follows
            // what changed, like a composed page's commands arriving.
            cx.observe_in(&this, window, |command: &mut Self, _, window, cx| {
                command.follow(window, cx);
            }),
        ];
        let previous_pages = pages.get().clone();
        let previous_value = value.get().clone();
        let command = Self {
            instance,
            core,
            field,
            scroll: ScrollHandle::new(),
            settled: ListSettled::new(),
            value,
            query,
            pages,
            queries: Vec::new(),
            items: options.items.clone(),
            status: options.status,
            pending: Vec::new(),
            error: None,
            on_value_change: options.on_value_change.clone(),
            on_before_value_change: options.on_before_value_change.clone(),
            on_action: options.on_action.clone(),
            pending_announcement: options.pending_announcement.clone(),
            empty_items: options.empty_items.clone(),
            filter: options.filter.clone(),
            auto_highlight: options.auto_highlight,
            r#loop: options.r#loop,
            on_load_more: options.on_load_more.clone(),
            announcement: options.announcement.clone(),
            placeholder: options.placeholder.clone(),
            field_style: options.field_style,
            page_cause: None,
            awaiting: None,
            previous_value,
            previous_pages,
            previous_mounts: 0,
            item_props: RefCell::new(HashMap::new()),
            texts: ItemTexts::new(),
            written,
            input_bound: false,
            announce_pending: AnnouncePending::new(),
            _subscriptions: subscriptions,
        };
        // The value it starts with is highlighted, and data commands are
        // known at once, so without one the first is, before anything renders.
        cx.defer_in(window, |command, window, cx| command.start(window, cx));
        command
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A controlled value is highlighted in the same pass.
    pub fn set_options(&mut self, options: CommandOptions, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = self.value.set_controlled(options.value.clone());
        changed |= self.query.set_controlled(options.query.clone());
        self.query.set_on_change(options.on_query_change.clone());
        changed |= self.pages.set_controlled(options.pages.clone());
        self.pages.set_on_change(options.on_pages_change.clone());
        let items_changed = match (&self.items, &options.items) {
            (Some(a), Some(b)) => !a.ptr_eq(b),
            (None, None) => false,
            _ => true,
        };
        changed |= items_changed
            || self.status != options.status
            || self.filter != options.filter
            || self.auto_highlight != options.auto_highlight
            || self.placeholder != options.placeholder
            || self.field_style != options.field_style;
        self.items = options.items.clone();
        self.status = options.status;
        self.on_value_change = options.on_value_change.clone();
        self.on_before_value_change = options.on_before_value_change.clone();
        self.on_action = options.on_action.clone();
        self.pending_announcement = options.pending_announcement.clone();
        self.empty_items = options.empty_items.clone();
        self.filter = options.filter.clone();
        self.auto_highlight = options.auto_highlight;
        self.r#loop = options.r#loop;
        self.on_load_more = options.on_load_more.clone();
        self.announcement = options.announcement.clone();
        self.instance.debug = options.debug;
        if self.placeholder != options.placeholder || self.field_style != options.field_style {
            self.placeholder = options.placeholder.clone();
            self.field_style = options.field_style;
            let field_options = field_options(self.placeholder.clone(), self.field_style, self.written.clone());
            self.field.update(cx, |field, cx| field.set_options(field_options, cx));
        }
        let core = self.core.clone();
        core.options_changed(self, window, cx);
        self.observe(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled highlight, `Some(None)` for nothing, or `None`
    /// to let the command keep its own.
    pub fn set_controlled_value(
        &mut self,
        value: Option<Option<SharedString>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.value.set_controlled(value) {
            self.observe(window, cx);
            cx.notify();
        }
    }

    /// Passes the controlled search text, or `None` to let the command keep
    /// its own.
    pub fn set_controlled_query(&mut self, query: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        if self.query.set_controlled(query) {
            self.observe(window, cx);
            cx.notify();
        }
    }

    /// Passes the controlled stack of pages, or `None` to let the command
    /// keep its own.
    pub fn set_controlled_pages(
        &mut self,
        pages: Option<Vec<SharedString>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pages.set_controlled(pages) {
            self.observe(window, cx);
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The search field it renders, a native text field.
    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// The scroller the list part tracks, which revealing a command scrolls.
    /// Make the list scroll with `overflow_y_scroll()` and a height limit.
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    // The current page's data commands, or `None` for a page only parts
    // compose.
    pub(crate) fn page_items(&self) -> Option<CommandData> {
        page_items(self.items.as_ref(), self.pages.get())
    }

    /// The page on top of the stack, or `None` on the first page.
    pub fn current_page(&self) -> Option<SharedString> {
        self.pages.get().last().cloned()
    }

    // Typing in the search field searches.
    fn field_event(&mut self, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let TextFieldEvent::Input(edit) = event {
            let text = edit.value.clone();
            self.written = text.clone();
            self.search(text, ChangeOptions::reason(Reason::InputChange).source(edit.source), window, cx);
        }
    }
}

impl Focusable for Command {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.read(cx).focus_handle(cx)
    }
}

/// `agent: { actions: ["run", "setValue", "setQuery", "setPages"] }`, with
/// `value`, `query` and `pages` controlled.
impl Agent for Command {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("run", |command, args, window, cx| {
            let value = if args.is_missing(0) { None } else { Some(args.string(0)?) };
            command.run(value, window, cx);
            Ok(AgentValue::Null)
        }),
        AgentAction::new("setValue", |command, args, window, cx| {
            let value = if args.is_missing(0) { None } else { Some(args.string(0)?) };
            Ok(command.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setQuery", |command, args, window, cx| {
            let query = args.string(0)?;
            Ok(command.set_query(query, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setPages", |command, args, window, cx| {
            let pages = args.list(0)?;
            Ok(command.set_pages(pages, ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["value", "query", "pages"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        let pages: Vec<AgentValue> = self
            .pages
            .get()
            .iter()
            .map(|page| AgentValue::String(page.to_string()))
            .collect();
        let pending: Vec<AgentValue> = self
            .pending
            .iter()
            .map(|key| AgentValue::String(key.to_string()))
            .collect();
        AgentState::new()
            .with(
                "value",
                self.value
                    .get()
                    .as_ref()
                    .map_or(AgentValue::Null, |value| AgentValue::String(value.to_string())),
            )
            .with("query", self.query.get().to_string())
            .with("pages", AgentValue::Array(pages))
            .with("pending", AgentValue::Array(pending))
            .with(
                "error",
                self.error
                    .as_ref()
                    .map_or(AgentValue::Null, |(_, reason)| AgentValue::String(reason.to_string())),
            )
    }
}
