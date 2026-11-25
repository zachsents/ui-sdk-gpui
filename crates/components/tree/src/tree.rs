//! Port of `components/tree/tree.ts`: a hierarchy of files, pages or choices
//! with branches that expand, as a WAI-ARIA tree. Its options, the state its
//! parts expose, and the parts: `root`, `item`, `label`, `indicator`,
//! `checkbox` and `group`. `tree_api.rs` holds the state and the api.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, ClickEvent, Entity, InteractiveElement, Modifiers, MouseButton, Role as AccessRole,
    StatefulInteractiveElement,
    ScrollHandle, SharedString, Styled, Window,
};
use ui_sdk_kernel::part::{FocusMode, with_role};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    ActionOutcome, Agent, AgentAction, AgentItem, AgentState, AgentSurface, AgentValue, Aria,
    ChangeCallback, ChangeOptions, Current, InputEvent, Need, Part, Reason,
};
use ui_sdk_motion::{PanelState, PresenceTiming, hidden_attrs};

use ui_sdk_lists::{ListHighlightDetails, TreeCheckStrategy, TreeCheckedState, TreeItemData};
pub use crate::tree_api::Tree;

/// How many items can be selected: none, like a tree of links, one, or many.
/// Checkboxes are separate, with `checkable`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TreeMode {
    None,
    #[default]
    Single,
    Multiple,
}

impl TreeMode {
    /// The original's name for it, like `"multiple"`.
    pub fn as_str(self) -> &'static str {
        match self {
            TreeMode::None => "none",
            TreeMode::Single => "single",
            TreeMode::Multiple => "multiple",
        }
    }
}

/// The selected value: one item's value or none, or a list with
/// `TreeMode::Multiple`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TreeValue {
    /// One item's value, or `None` for nothing (`string | null`).
    Single(Option<SharedString>),
    /// The selected items' values (`string[]`).
    Multiple(Vec<SharedString>),
}

impl Default for TreeValue {
    fn default() -> Self {
        TreeValue::Single(None)
    }
}

impl TreeValue {
    /// One item's value.
    pub fn one(value: impl Into<SharedString>) -> Self {
        TreeValue::Single(Some(value.into()))
    }

    /// Several items' values.
    pub fn many(values: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        TreeValue::Multiple(values.into_iter().map(Into::into).collect())
    }

    /// Nothing selected, in the shape `mode` takes: an empty list with
    /// `Multiple`, else `None`.
    pub fn empty(mode: TreeMode) -> Self {
        match mode {
            TreeMode::Multiple => TreeValue::Multiple(Vec::new()),
            _ => TreeValue::Single(None),
        }
    }

    /// The selected keys, whatever the shape (`toKeys`).
    pub fn keys(&self) -> Vec<SharedString> {
        match self {
            TreeValue::Single(value) => value.iter().cloned().collect(),
            TreeValue::Multiple(values) => values.clone(),
        }
    }

    /// Keys in the shape `mode` takes: the list with `Multiple`, else the
    /// first.
    pub fn from_keys(mode: TreeMode, keys: Vec<SharedString>) -> Self {
        match mode {
            TreeMode::Multiple => TreeValue::Multiple(keys),
            _ => TreeValue::Single(keys.into_iter().next()),
        }
    }
}

/// Whether moving focus with the keyboard selects, with one item selectable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TreeActivation {
    /// Selects the focused item.
    Automatic,
    /// Waits for Enter, Space or a press.
    #[default]
    Manual,
}

/// Why branches expanded or collapsed: a branch's row was pressed
/// (`ItemPress`), its indicator was pressed (`TriggerPress`), an arrow key
/// or `*` opened it (`ListNavigation`), or a controller or api call changed
/// it (`Imperative`). `FindInPage` stays in the vocabulary; gpui has no find
/// in page.
pub type TreeExpandedChangeReason = Reason;

/// Why the selection changed: an item was pressed (`ItemPress`), the
/// keyboard moved it (`ListNavigation`), or a controller or api call changed
/// it (`Imperative`).
pub type TreeValueChangeReason = Reason;

/// Why the checked items changed: a checkbox, or a row with Space
/// (`ItemPress`), or a controller or api call (`Imperative`).
pub type TreeCheckedChangeReason = Reason;

/// Options for every write method.
pub type TreeChangeOptions = ChangeOptions;

/// What `on_action` receives besides the item's value.
#[derive(Clone, Debug, Default)]
pub struct TreeActionDetails {
    /// The click or key press that ran it.
    pub event: Option<InputEvent>,
}

/// What happened, for `announcement` to put into words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TreeAnnouncementType {
    /// A branch expanded.
    Expanded,
    /// A branch collapsed.
    Collapsed,
    /// A branch's children started loading.
    Loading,
    /// Loading a branch's children failed.
    LoadFailed,
}

impl TreeAnnouncementType {
    /// The original's name for it, like `"load-failed"`.
    pub fn as_str(self) -> &'static str {
        match self {
            TreeAnnouncementType::Expanded => "expanded",
            TreeAnnouncementType::Collapsed => "collapsed",
            TreeAnnouncementType::Loading => "loading",
            TreeAnnouncementType::LoadFailed => "load-failed",
        }
    }
}

/// What `announcement` is asked to put into words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeAnnouncement {
    /// What happened: a branch expanded or collapsed, its children started
    /// loading, or loading them failed.
    pub r#type: TreeAnnouncementType,
    /// The branch's value.
    pub value: SharedString,
    /// The branch's text.
    pub label: SharedString,
}

/// Whether a load of a branch's children was given up (`AbortSignal`):
/// expanding the branch again while it loads, collapsing it, or the tree
/// going away aborts it, and the tree drops the load's work then too.
#[derive(Clone, Debug, Default)]
pub struct TreeLoadSignal(Rc<Cell<bool>>);

impl TreeLoadSignal {
    /// Whether the load was aborted.
    pub fn aborted(&self) -> bool {
        self.0.get()
    }

    pub(crate) fn abort(&self) {
        self.0.set(true);
    }
}

/// What `load_children` receives besides the branch's value.
#[derive(Clone, Debug, Default)]
pub struct TreeLoadDetails {
    /// Aborted when the load is given up.
    pub signal: TreeLoadSignal,
}

/// `on_action`: the item's value and what ran it. Return
/// `ActionOutcome::Pending` with the work to mark the item pending until it
/// settles. It runs inside the tree's update, since the tree reads what it
/// returns, so it may update any entity but the tree.
pub type TreeActionCallback =
    Rc<dyn Fn(&SharedString, &TreeActionDetails, &mut Window, &mut App) -> ActionOutcome>;

/// `load_children`: loads a branch's children. `ActionOutcome::Done` when
/// they're there now, `Pending` with the work that loads them, `Failed` when
/// it couldn't start. It runs inside the tree's update, since the tree reads
/// what it returns, so it may update any entity but the tree.
pub type TreeLoadChildren =
    Rc<dyn Fn(&SharedString, &TreeLoadDetails, &mut Window, &mut App) -> ActionOutcome>;

/// `on_highlight`: the item the keyboard moved focus to, and its row among
/// the items that show. It runs once the tree's update is over, so it may
/// update any entity, the tree too.
pub type TreeHighlightCallback = Rc<dyn Fn(&SharedString, &ListHighlightDetails, &mut Window, &mut App)>;

/// `announcement`: what screen readers hear for a change, or `None` for
/// nothing.
pub type TreeAnnouncementFn = Rc<dyn Fn(&TreeAnnouncement) -> Option<String>>;

/// The options `Tree` takes.
#[derive(Clone)]
pub struct TreeOptions {
    /// The items as data: each a `value`, a `label` and, for a branch,
    /// `children`. Gives the tree its whole shape even where rows aren't
    /// rendered; render the parts from [`Tree::items`].
    pub items: Option<Rc<[TreeItemData]>>,
    /// Items from file paths, like `["src/index.ts", "README.md"]`: each
    /// folder becomes a branch, folders first. A path ending in `/` is an
    /// empty folder.
    pub paths: Option<Rc<[SharedString]>>,
    /// How many items can be selected. `Single` by default.
    pub mode: TreeMode,
    /// The selected value. Pass it to control the selection.
    pub value: Option<TreeValue>,
    /// The value selected at first, when uncontrolled. Nothing by default.
    pub default_value: Option<TreeValue>,
    /// Runs when the selection wants to change. `details.reason` is
    /// `ItemPress`, `ListNavigation` or `Imperative`. It runs once the tree's
    /// update is over, so it may update any entity, the tree too.
    pub on_value_change: Option<ChangeCallback<TreeValue>>,
    /// Runs as the selection is asked to change, before it does, inside the
    /// tree's update: `details.cancel()` stops it. It may update any entity
    /// but the tree.
    pub on_before_value_change: Option<ChangeCallback<TreeValue>>,
    /// Whether moving focus with the keyboard selects, with one item
    /// selectable. `Manual` by default.
    pub activation: TreeActivation,
    /// The expanded branches' values. Pass it to control which branches are
    /// open.
    pub expanded: Option<Vec<SharedString>>,
    /// The branches expanded at first, when uncontrolled. The ancestors of
    /// `current` are added.
    pub default_expanded: Vec<SharedString>,
    /// Runs when branches expand or collapse. `details.reason` is
    /// `ItemPress`, `TriggerPress`, `ListNavigation` or `Imperative`. It runs
    /// once the tree's update is over, so it may update any entity, the tree
    /// too.
    pub on_expanded_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Runs as branches are asked to expand or collapse, before they do,
    /// inside the tree's update: `details.cancel()` stops it. It may update
    /// any entity but the tree.
    pub on_before_expanded_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Whether pressing a branch's row expands or collapses it. Its
    /// indicator always does. True by default.
    pub expand_on_press: bool,
    /// Gives every item a tri-state checkbox, kept apart from the selection.
    /// Checking a branch checks everything in it, and a branch shows mixed
    /// while only some of it is checked. Space checks the focused item.
    pub checkable: bool,
    /// The checked items' values, as `check_strategy` reports them. Pass it
    /// to control the checkboxes.
    pub checked: Option<Vec<SharedString>>,
    /// The items checked at first, when uncontrolled. A branch stands for
    /// everything in it.
    pub default_checked: Vec<SharedString>,
    /// Runs when checkboxes change. `details.reason` is `ItemPress` or
    /// `Imperative`. It runs once the tree's update is over, so it may update
    /// any entity, the tree too.
    pub on_checked_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Runs as checkboxes are asked to change, before they do, inside the
    /// tree's update: `details.cancel()` stops it. It may update any entity
    /// but the tree.
    pub on_before_checked_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Which items the checked value lists: the checked leaves (`Leaf`, the
    /// default), the outermost fully checked items (`Trunk`), every fully
    /// checked item (`Branch`), or each item on its own with no cascading
    /// (`Independent`).
    pub check_strategy: TreeCheckStrategy,
    /// Runs when an item is pressed or Enter is pressed on it, apart from
    /// selecting it, like opening a file. Pending work marks the item
    /// pending until it settles. It runs inside the tree's update, since the
    /// tree reads what it returns.
    pub on_action: Option<TreeActionCallback>,
    /// What screen readers hear once an item's action has been pending for a
    /// second, so a slow action doesn't pass in silence. Set it in the app's
    /// language; an empty string says nothing. "Loading" by default.
    pub pending_announcement: Option<SharedString>,
    /// Loads a branch's children the first time it expands while it has
    /// none. The branch is `loading` and expands once the work settles; a
    /// failure marks it `failed` and the next expand tries again. Expanding
    /// it again while loading aborts it.
    pub load_children: Option<TreeLoadChildren>,
    /// The value of the item for the current page. It's
    /// `aria-current="page"`, takes the tab stop first, and its ancestors
    /// expand, at first and whenever it changes.
    pub current: Option<SharedString>,
    /// Runs when the keyboard moves focus to an item, with its row among the
    /// items that show, from 0. A virtualized list scrolls that row into view
    /// here, so an item it hasn't rendered can take focus once it does.
    pub on_highlight: Option<TreeHighlightCallback>,
    /// Disables every item. Items stay focusable unless
    /// `focusable_when_disabled` is false.
    pub disabled: bool,
    /// Whether disabled items take focus, so a screen reader still finds
    /// them. True by default.
    pub focusable_when_disabled: bool,
    /// Whether collapsed branches keep their groups in the page, hidden.
    pub keep_mounted: bool,
    /// Keeps collapsed groups in the page, hidden, where the original keeps
    /// them as `hidden="until-found"` for the browser's find in page. gpui
    /// has none, so it keeps them as `keep_mounted` does.
    pub hidden_until_found: bool,
    /// What screen readers hear when a branch expands, collapses, starts
    /// loading or fails to load; `None` from it says nothing. Expanding and
    /// collapsing are announced on Apple platforms only, where VoiceOver
    /// doesn't read the change. "Expanded", "Collapsed", "Loading" or
    /// "Couldn't load src." by default.
    pub announcement: Option<TreeAnnouncementFn>,
    /// How groups animate open and closed, as their CSS transition would
    /// declare. The default has none, so they show and hide at once.
    pub group_timing: PresenceTiming,
    /// The scroll handle of the element the items scroll in, which `reveal`
    /// scrolls an item into view in (`scrollIntoView` in the original).
    pub scroller: Option<ScrollHandle>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for TreeOptions {
    fn default() -> Self {
        Self {
            items: None,
            paths: None,
            mode: TreeMode::Single,
            value: None,
            default_value: None,
            on_value_change: None,
            on_before_value_change: None,
            activation: TreeActivation::Manual,
            expanded: None,
            default_expanded: Vec::new(),
            on_expanded_change: None,
            on_before_expanded_change: None,
            expand_on_press: true,
            checkable: false,
            checked: None,
            default_checked: Vec::new(),
            on_checked_change: None,
            on_before_checked_change: None,
            check_strategy: TreeCheckStrategy::Leaf,
            on_action: None,
            pending_announcement: None,
            load_children: None,
            current: None,
            on_highlight: None,
            disabled: false,
            focusable_when_disabled: true,
            keep_mounted: false,
            hidden_until_found: false,
            announcement: None,
            group_timing: PresenceTiming::default(),
            scroller: None,
            debug: false,
        }
    }
}

fn strings(values: impl IntoIterator<Item = impl Into<SharedString>>) -> Vec<SharedString> {
    values.into_iter().map(Into::into).collect()
}

impl TreeOptions {
    pub fn items(mut self, items: impl Into<Rc<[TreeItemData]>>) -> Self {
        self.items = Some(items.into());
        self
    }

    pub fn paths(mut self, paths: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.paths = Some(strings(paths).into());
        self
    }

    pub fn mode(mut self, mode: TreeMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn value(mut self, value: TreeValue) -> Self {
        self.value = Some(value);
        self
    }

    pub fn default_value(mut self, value: TreeValue) -> Self {
        self.default_value = Some(value);
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&TreeValue, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&TreeValue, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn activation(mut self, activation: TreeActivation) -> Self {
        self.activation = activation;
        self
    }

    pub fn expanded(mut self, expanded: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.expanded = Some(strings(expanded));
        self
    }

    pub fn default_expanded(mut self, expanded: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.default_expanded = strings(expanded);
        self
    }

    pub fn on_expanded_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_expanded_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_expanded_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_expanded_change = Some(Rc::new(callback));
        self
    }

    pub fn expand_on_press(mut self, expand_on_press: bool) -> Self {
        self.expand_on_press = expand_on_press;
        self
    }

    pub fn checkable(mut self, checkable: bool) -> Self {
        self.checkable = checkable;
        self
    }

    pub fn checked(mut self, checked: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.checked = Some(strings(checked));
        self
    }

    pub fn default_checked(mut self, checked: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.default_checked = strings(checked);
        self
    }

    pub fn on_checked_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_checked_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_checked_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_checked_change = Some(Rc::new(callback));
        self
    }

    pub fn check_strategy(mut self, strategy: TreeCheckStrategy) -> Self {
        self.check_strategy = strategy;
        self
    }

    pub fn on_action(
        mut self,
        callback: impl Fn(&SharedString, &TreeActionDetails, &mut Window, &mut App) -> ActionOutcome + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(callback));
        self
    }

    pub fn pending_announcement(mut self, text: impl Into<SharedString>) -> Self {
        self.pending_announcement = Some(text.into());
        self
    }

    pub fn load_children(
        mut self,
        load: impl Fn(&SharedString, &TreeLoadDetails, &mut Window, &mut App) -> ActionOutcome + 'static,
    ) -> Self {
        self.load_children = Some(Rc::new(load));
        self
    }

    pub fn current(mut self, current: Option<impl Into<SharedString>>) -> Self {
        self.current = current.map(Into::into);
        self
    }

    pub fn on_highlight(
        mut self,
        callback: impl Fn(&SharedString, &ListHighlightDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_highlight = Some(Rc::new(callback));
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn focusable_when_disabled(mut self, focusable: bool) -> Self {
        self.focusable_when_disabled = focusable;
        self
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn hidden_until_found(mut self, hidden_until_found: bool) -> Self {
        self.hidden_until_found = hidden_until_found;
        self
    }

    pub fn announcement(mut self, announcement: impl Fn(&TreeAnnouncement) -> Option<String> + 'static) -> Self {
        self.announcement = Some(Rc::new(announcement));
        self
    }

    pub fn group_timing(mut self, timing: PresenceTiming) -> Self {
        self.group_timing = timing;
        self
    }

    pub fn scroller(mut self, scroller: ScrollHandle) -> Self {
        self.scroller = Some(scroller);
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `item` part takes, beside its value, which identifies it in
/// the tree: `on_action`, the value, `expanded` and `checked` list it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeItemProps {
    /// The value of the item it's nested in, or `None` at the top level. In
    /// the original the item around it passes it; pass it here for an item
    /// in another's group. With `items` or `paths`, the data says it.
    pub parent: Option<SharedString>,
    /// Whether it's a branch. With data, the data says it; set it for an
    /// item with a group, or a branch whose children load later.
    pub branch: Option<bool>,
    /// Stops it from being selected, checked, expanded or run. It stays
    /// focusable unless `focusable_when_disabled` is false.
    pub disabled: bool,
    /// Its text: what names it, which typeahead matches, and what
    /// announcements call it. Data items have theirs.
    pub label: Option<SharedString>,
}

impl TreeItemProps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parent(mut self, parent: Option<impl Into<SharedString>>) -> Self {
        self.parent = parent.map(Into::into);
        self
    }

    pub fn branch(mut self, branch: bool) -> Self {
        self.branch = Some(branch);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// The props the `group` part takes, beside its branch's value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TreeGroupProps {
    /// Whether the group stays in the page, hidden, while collapsed.
    /// Overrides the tree's.
    pub keep_mounted: Option<bool>,
    /// Whether the group stays in the page, hidden, as for find in page.
    /// Overrides the tree's.
    pub hidden_until_found: Option<bool>,
}

impl TreeGroupProps {
    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = Some(keep_mounted);
        self
    }

    pub fn hidden_until_found(mut self, hidden_until_found: bool) -> Self {
        self.hidden_until_found = Some(hidden_until_found);
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeState {
    /// How many items can be selected.
    pub mode: TreeMode,
    /// Whether items have checkboxes.
    pub checkable: bool,
    /// Whether the whole tree is disabled, by its `disabled` option or a
    /// disabled fieldset around it.
    pub disabled: bool,
}

/// What an item's `with` reads. Its label, indicator and checkbox get the
/// same, the original's `data-*` attributes on each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeItemState {
    /// Whether it's a branch that's expanded.
    pub expanded: bool,
    /// Whether its value is in the tree's value. Separate from `checked`,
    /// which its checkbox sets.
    pub selected: bool,
    /// Its checkbox: checked, unchecked or indeterminate. Unchecked without
    /// `checkable`.
    pub checked: TreeCheckedState,
    /// Whether it can't be selected, checked, expanded or run.
    pub disabled: bool,
    /// Whether it has children, or can load them.
    pub branch: bool,
    /// Its depth, 1 at the top level (`--tree-level` in the original).
    pub level: usize,
    /// Whether it's the current page's item.
    pub current: bool,
    /// Whether its children are loading.
    pub loading: bool,
    /// Whether loading its children failed.
    pub failed: bool,
    /// Whether its `on_action` is still running.
    pub pending: bool,
}

/// What the label's `with` reads: its item's.
pub type TreeLabelState = TreeItemState;
/// What the indicator's `with` reads: its item's.
pub type TreeIndicatorState = TreeItemState;
/// What the checkbox's `with` reads: its item's.
pub type TreeCheckboxState = TreeItemState;

/// What a group's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeGroupState {
    /// Whether its branch is expanded.
    pub expanded: bool,
    /// The depth of the items in it.
    pub level: usize,
    /// The group's presence, and the height to give it as it animates
    /// (`data-starting-style`, `data-ending-style` and `--tree-group-height`
    /// in the original). Clip its overflow while it moves.
    pub panel: PanelState,
}

// The keys an item handles: Space with checkboxes or a selection, and
// `mod+a` with several selectable.
const ITEM_KEYS: &[&str] = &[
    "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "*", "Enter", "typeahead",
];
const ITEM_KEYS_SPACE: &[&str] = &[
    "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "*", "Enter", " ", "typeahead",
];
const ITEM_KEYS_MULTIPLE: &[&str] = &[
    "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "*", "Enter", " ", "typeahead",
    "mod+a",
];

/// The modifiers held with the event behind a press.
pub(crate) fn modifiers_of(event: Option<&InputEvent>) -> Modifiers {
    match event {
        Some(InputEvent::Click(click)) => click.modifiers(),
        Some(InputEvent::KeyDown(key)) => key.keystroke.modifiers,
        Some(InputEvent::KeyUp(key)) => key.keystroke.modifiers,
        Some(InputEvent::MouseDown(down)) => down.modifiers,
        Some(InputEvent::MouseUp(up)) => up.modifiers,
        _ => Modifiers::default(),
    }
}

impl Tree {
    /// The state every part of an item shares (`itemState`), with the props
    /// its item rendered with, when the part is the item itself.
    pub(crate) fn item_state(&self, key: &SharedString, props: Option<&TreeItemProps>) -> TreeItemState {
        let props = props.cloned().or_else(|| self.item_props(key));
        let branch = self.is_branch(key, props.as_ref());
        TreeItemState {
            expanded: branch && self.expanded.get().contains(key),
            selected: self.selected.contains(key),
            checked: self.checks.get(key).copied().unwrap_or_default(),
            disabled: self.is_disabled_key(key, props.as_ref()),
            branch,
            level: self.level(key, props.as_ref()),
            current: self.current.as_ref() == Some(key),
            loading: self.loading.contains(key),
            failed: self.failed.contains(key),
            pending: self.pending.contains(key),
        }
    }

    /// What the root part's state holds.
    pub fn state(&self) -> TreeState {
        TreeState {
            mode: self.mode,
            checkable: self.checkable,
            disabled: self.is_disabled(),
        }
    }

    /// The `root` part, around the items: a tree. Name it with
    /// `aria_label`. Build it before its items: it starts their render, so
    /// the order they render in is page order, and once the frame is drawn
    /// it learns which items joined, left or moved.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<TreeState> {
        let tree = this.read(cx);
        tree.begin(cx);
        let state = tree.state();
        let form = tree.form.clone();
        let entity = this.downgrade();
        Part::new(&tree.instance, "root", None, state)
            .aria_role(Role::Tree)
            .unneeded(Need::Keys, "Focus stays on an item, whose keydown handles them.")
            .aria(
                Aria::new()
                    .multiselectable(state.mode == TreeMode::Multiple)
                    .disabled(state.disabled),
            )
            .map_el(|el| form.track(el))
            .on_props(move |window, cx| {
                entity.update(cx, |tree, cx| tree.rendered(window, cx)).ok();
            })
    }

    /// An `item` part, by its `value`. Put its row in a [`Tree::label`] and
    /// its children in a [`Tree::group`], both built after it. It holds the
    /// tree's one tab stop while it's the current page's item, the first
    /// selected one, or the one focused last, and handles the keys while it
    /// holds focus itself, so a control inside it, like a rename field,
    /// keeps its own.
    pub fn item(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: TreeItemProps,
        cx: &App,
    ) -> Part<TreeItemState> {
        let key: SharedString = value.into();
        let tree = this.read(cx);
        tree.item_parts.register(key.clone(), props.clone());
        let state = tree.item_state(&key, Some(&props));
        let handle = tree.roving.focus_handle(&key, cx);
        let member = tree.member(&key, &props, &state);
        let holds = tree.roving.bind(key.clone(), &handle, member, cx);
        tree.recovery.bind(key.clone(), props.clone(), &handle, cx);
        // An item that doesn't take focus while disabled is never the stop.
        let tab_stop = holds && (tree.focusable_when_disabled || !state.disabled);
        let label = tree.known_label(&key, Some(&props));
        let id = &tree.instance;
        let mut aria = Aria::new()
            .labelled_by(id.element_id("label", Some(&key)))
            .disabled(state.disabled)
            .current(state.current.then_some(Current::Page))
            .busy(state.loading || state.pending);
        if tree.checkable {
            aria = aria.checked(state.checked);
        }
        if let Some(label) = label {
            aria = aria.fallback_label(label);
        }
        let keys = match (tree.mode, tree.checkable) {
            (TreeMode::Multiple, _) => ITEM_KEYS_MULTIPLE,
            (TreeMode::Single, _) | (_, true) => ITEM_KEYS_SPACE,
            (TreeMode::None, false) => ITEM_KEYS,
        };
        let position = tree.position(&key, &props);
        let mode = tree.mode;
        let pressed = this.downgrade();
        let typed = this.downgrade();
        let press_key = key.clone();
        let key_key = key.clone();
        let own = handle.clone();
        let mut part = Part::new(&tree.instance, "item", Some(&key), state)
            .aria_role(Role::TreeItem)
            .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
            .aria_level(state.level)
            .aria(aria)
            .on_keys(keys, move |event, window, cx| {
                // Only its own: a key on an item nested in it, or on a control
                // inside it, isn't.
                if !own.is_focused(window) {
                    return;
                }
                typed
                    .update(cx, |tree, cx| tree.keydown(event, &key_key, window, cx))
                    .ok();
            })
            .on_press(move |event, window, cx| {
                // Items nest, so a press on a nested item reaches this one
                // too; the innermost takes it. Enter and Space reach the
                // item's keydown, which handles them as the original's does.
                cx.stop_propagation();
                if matches!(event, InputEvent::Click(ClickEvent::Keyboard(_))) {
                    return;
                }
                pressed
                    .update(cx, |tree, cx| tree.press(&press_key, Some(event), window, cx))
                    .ok();
            });
        if state.branch {
            part = part.aria_expanded(state.expanded);
        }
        if mode != TreeMode::None {
            part = part.aria_selected(state.selected);
        }
        if let Some(position) = position {
            part = part
                .aria_position_in_set(position.index)
                .aria_size_of_set(position.size);
        }
        part
    }

    /// The `label` part of the item with `value`: its row's text, which
    /// names it. It's named by the item's `label` prop or data label; give it
    /// an `aria_label` of your own when the text differs. Style a row from
    /// it, apart from the items nested in its item, and a focused row from
    /// its item's focus.
    pub fn label(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<TreeLabelState> {
        let key: SharedString = value.into();
        let tree = this.read(cx);
        let state = tree.item_state(&key, None);
        let part = Part::new(&tree.instance, "label", Some(&key), state).map_el(|el| with_role(el, AccessRole::Label));
        match tree.known_label(&key, None) {
            Some(label) => part.aria_label(label),
            None => part,
        }
    }

    /// The `indicator` part of the item with `value`: the arrow that expands
    /// and collapses a branch. Alt or Option expands everything in it.
    /// Hidden from screen readers, which hear the item's expanded state.
    pub fn indicator(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<TreeIndicatorState> {
        let key: SharedString = value.into();
        let tree = this.read(cx);
        let state = tree.item_state(&key, None);
        let entity = this.downgrade();
        Part::new(&tree.instance, "indicator", Some(&key), state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true))
            .on_press(move |event, window, cx| {
                // The item's press would toggle it again.
                cx.stop_propagation();
                entity
                    .update(cx, |tree, cx| tree.press_indicator(&key, Some(event), window, cx))
                    .ok();
            })
    }

    /// The `checkbox` part of the item with `value`, with `checkable`.
    /// Hidden from screen readers, which hear the item's checked state.
    pub fn checkbox(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<TreeCheckboxState> {
        let key: SharedString = value.into();
        let tree = this.read(cx);
        let state = tree.item_state(&key, None);
        let entity = this.downgrade();
        Part::new(&tree.instance, "checkbox", Some(&key), state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true))
            .on_press(move |event, window, cx| {
                // The checkbox doesn't select or run the item.
                cx.stop_propagation();
                entity
                    .update(cx, |tree, cx| tree.press_checkbox(&key, Some(event), window, cx))
                    .ok();
            })
    }

    /// The `group` part of the branch with `value`: its children, or `None`
    /// while it's out of the page. It stays in the page while it animates
    /// closed, and while collapsed with `keep_mounted`, hidden. Style it from
    /// its state: give it `panel.height` and clip its overflow while it
    /// animates. Groups that render open with the tree don't animate.
    pub fn group(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: TreeGroupProps,
        cx: &App,
    ) -> Option<Part<TreeGroupState>> {
        let key: SharedString = value.into();
        let tree = this.read(cx);
        // A render with new props, like `keep_mounted`, checks the group
        // again (`stateAndRenders`).
        let stale = tree.group_props.borrow_mut().insert(key.clone(), props) != Some(props);
        if !tree.group_present(&key, &props) {
            return None;
        }
        let open = tree.expanded.get().contains(&key);
        let shown = open || tree.exiting.contains(&key);
        let hidden = hidden_attrs(shown, tree.findable(&props));
        let panel = tree.panels.get(&key).cloned();
        let state = TreeGroupState {
            expanded: open,
            level: tree.level(&key, None) + 1,
            panel: panel.as_ref().map(|panel| panel.state()).unwrap_or_default(),
        };
        let mut part = Part::new(&tree.instance, "group", Some(&key), state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(hidden));
        if hidden {
            part = part.hidden();
        }
        if let Some(panel) = &panel {
            part = panel.track(part);
        }
        // The group tracks focus only to know when focus is inside it: a
        // press on the group itself, not on an item in it, which would have
        // taken focus first, focuses its branch, the element around it that
        // takes focus.
        let branch = tree.roving.focus_handle(&key, cx);
        part = part.on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if !window.default_prevented() {
                window.focus(&branch, cx);
                window.prevent_default();
            }
        });
        if stale || panel.is_none() {
            let entity = this.downgrade();
            part = part.on_props(move |window, cx| {
                entity.update(cx, |tree, cx| tree.sync_group(&key, window, cx)).ok();
            });
        }
        Some(part)
    }
}

/// `agent: { actions: ["setExpanded", "expand", "collapse", "expandAll",
/// "collapseAll", "setValue", "setChecked"], closed: true }` and
/// `controlled: ["value", "expanded", "checked"]`, with `toggle`, which
/// `expand` and `collapse` cover, unlisted. Its items are the data's, every
/// level flattened.
impl Agent for Tree {
    const SURFACE: AgentSurface = AgentSurface::new().closed(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setExpanded", |tree, args, window, cx| {
            let expanded = args.list(0)?;
            Ok(tree.set_expanded(expanded, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("expand", |tree, args, window, cx| {
            let value = args.string(0)?;
            Ok(tree.expand(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("collapse", |tree, args, window, cx| {
            let value = args.string(0)?;
            Ok(tree.collapse(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("expandAll", |tree, _args, window, cx| {
            Ok(tree.expand_all(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("collapseAll", |tree, _args, window, cx| {
            Ok(tree.collapse_all(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setValue", |tree, args, window, cx| {
            let value = match tree.mode {
                TreeMode::Multiple => TreeValue::Multiple(args.list(0)?),
                _ if args.is_missing(0) => TreeValue::Single(None),
                _ => TreeValue::Single(Some(args.string(0)?)),
            };
            Ok(tree.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setChecked", |tree, args, window, cx| {
            let checked = args.list(0)?;
            Ok(tree.set_checked(checked, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |tree, args, window, cx| {
            let value = args.string(0)?;
            Ok(tree.toggle(value, ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["value", "expanded", "checked"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        let list = |keys: &[SharedString]| AgentValue::from(keys.iter().map(ToString::to_string).collect::<Vec<_>>());
        let value = match self.get_value() {
            TreeValue::Single(Some(value)) => AgentValue::from(value.to_string()),
            TreeValue::Single(None) => AgentValue::Null,
            TreeValue::Multiple(values) => list(values),
        };
        AgentState::new()
            .with("value", value)
            .with("expanded", list(self.get_expanded()))
            .with("checked", list(self.get_checked()))
            .with("disabled", self.is_disabled())
            .with("pending", list(&self.pending))
    }

    fn agent_items(&self, _cx: &App) -> Option<Vec<AgentItem>> {
        fn flatten(items: &[TreeItemData], disabled: bool, into: &mut Vec<AgentItem>) {
            for item in items {
                into.push(AgentItem::new(item.value.clone(), item.label.clone()).disabled(disabled || item.disabled));
                if let Some(children) = &item.children {
                    flatten(children, disabled, into);
                }
            }
        }
        let items = self.items()?;
        let mut into = Vec::new();
        flatten(&items, self.is_disabled(), &mut into);
        Some(into)
    }
}
