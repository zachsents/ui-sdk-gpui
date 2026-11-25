//! Port of `components/tree/tree-api.ts`: what makes a tree a tree, on the
//! same public blocks a component of your own uses: the tab stop and arrow
//! keys from `RovingFocus`, the selection rules from `create_selection`,
//! focus that survives a collapse from `FocusRecovery`, group animations
//! from the motion crate's `Panel`, and the shape from a tree collection,
//! built from `items` or from the items in the page.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    App, Context, FocusHandle, KeyDownEvent, Pixels, ScrollHandle, SharedString, Task, Window,
    point,
};
use ui_sdk_focus::{
    FocusRecovery, LostFocus, RovingFocus, RovingFocusOptions, RovingMember, scroll_to_show,
};
use ui_sdk_forms::{Node, is_disabled, watch_form_disabled};
use ui_sdk_kernel::dev::{self, Origin};
use ui_sdk_kernel::{
    ActionOutcome, AnnouncePending, ChangeOptions, Checked, Controllable, InputEvent, Instance, Key,
    Parts, Reason, held, while_destroying,
};
use ui_sdk_lists::{
    ListHighlightDetails, ToggleTreeCheckedOptions, TreeCheckStrategy, TreeCheckedState,
    TreeCollection, TreeCollectionNode, TreeItemData, TreePosition, create_tree_collection,
    toggle_tree_checked, tree_checked_states, tree_data_nodes, tree_from_paths,
};
use ui_sdk_motion::{Opening, Panel, PanelBinding, PresenceTiming};
use ui_sdk_platform::{announce, is_apple, is_rtl};
use ui_sdk_selection::{ExtendOptions, Selection, SelectionOptions, create_selection};
use ui_sdk_text::{Typeahead, is_typeahead_key};

use crate::tree::{
    TreeActionCallback, TreeActionDetails, TreeActivation, TreeAnnouncement, TreeAnnouncementFn,
    TreeAnnouncementType, TreeGroupProps, TreeHighlightCallback, TreeItemProps, TreeItemState,
    TreeLoadChildren, TreeLoadDetails, TreeLoadSignal, TreeMode, TreeOptions, TreeValue,
    modifiers_of,
};

// What an item of the data says about itself, by its value.
struct ItemInfo {
    label: SharedString,
    disabled: bool,
}

// Where the data came from, kept to tell new data from the same.
enum Source {
    Items(Rc<[TreeItemData]>),
    Paths(Rc<[SharedString]>),
}

// The shape `items` or `paths` give, kept until they change.
struct Built {
    source: Source,
    items: Rc<[TreeItemData]>,
    tree: TreeCollection,
    by_key: HashMap<SharedString, ItemInfo>,
}

fn build(items: Option<&Rc<[TreeItemData]>>, paths: Option<&Rc<[SharedString]>>) -> Option<Built> {
    let (source, list): (Source, Rc<[TreeItemData]>) = match (items, paths) {
        (Some(items), _) => (Source::Items(items.clone()), items.clone()),
        (None, Some(paths)) => (Source::Paths(paths.clone()), tree_from_paths(paths).into()),
        (None, None) => return None,
    };
    let nodes = tree_data_nodes(&list);
    let by_key = nodes
        .iter()
        .map(|node| {
            (
                node.key.clone(),
                ItemInfo {
                    label: node.item.label.clone(),
                    disabled: node.item.disabled,
                },
            )
        })
        .collect();
    let tree = create_tree_collection(nodes.iter());
    drop(nodes);
    Some(Built {
        source,
        items: list,
        tree,
        by_key,
    })
}

fn same<T: ?Sized>(a: Option<&Rc<T>>, b: Option<&Rc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

// A list with each key once, in its first place (`[...new Set(list)]`).
fn dedupe(list: impl IntoIterator<Item = SharedString>) -> Vec<SharedString> {
    let mut seen = HashSet::new();
    list.into_iter().filter(|key| seen.insert(key.clone())).collect()
}

fn with(list: &[SharedString], more: impl IntoIterator<Item = SharedString>) -> Vec<SharedString> {
    list.iter().cloned().chain(more).collect()
}

// Labels that aren't plain text name the item with `label`; a branch's own
// text never includes its children's, which sit outside its label.
fn default_announcement(change: &TreeAnnouncement) -> String {
    match change.r#type {
        TreeAnnouncementType::Expanded => "Expanded".into(),
        TreeAnnouncementType::Collapsed => "Collapsed".into(),
        TreeAnnouncementType::Loading => "Loading".into(),
        TreeAnnouncementType::LoadFailed => format!("Couldn't load {}.", change.label),
    }
}

// Where Up, Down, Home and End go. With data, that's any row that shows,
// rendered or not, so a virtualized list can follow.
fn move_target(event: &KeyDownEvent, from: &SharedString, keys: &[SharedString]) -> Option<SharedString> {
    let index = keys.iter().position(|key| key == from);
    match Key::of(&event.keystroke) {
        Key::ArrowDown => keys.get(index.map_or(0, |index| index + 1)).cloned(),
        Key::ArrowUp => index.filter(|index| *index > 0).map(|index| keys[index - 1].clone()),
        Key::Home => keys.first().cloned(),
        Key::End => keys.last().cloned(),
        _ => None,
    }
}

// A branch's children loading, and what gives the load up.
struct Load {
    signal: TreeLoadSignal,
    task: Option<Task<()>>,
}

// An item `reveal` waits to show, with the expanded branches it asked from.
struct Revealing {
    value: SharedString,
    before: Vec<SharedString>,
}

// An item's place among its siblings in the page, for a tree without data.
#[derive(Default, PartialEq)]
struct SiblingOrder {
    index: HashMap<SharedString, usize>,
    size: usize,
}

// What the follow-up saw last, to tell what changed.
#[derive(Clone)]
struct Seen {
    value: TreeValue,
    checked: Vec<SharedString>,
    checkable: bool,
    strategy: TreeCheckStrategy,
    items: Option<Rc<[TreeItemData]>>,
    paths: Option<Rc<[SharedString]>>,
    version: u64,
    expanded: Vec<SharedString>,
    current: Option<SharedString>,
}

/// A tree. Its entity is its controller: "Expand all" and "Collapse all"
/// buttons, a link to a file, or a test call its methods.
pub struct Tree {
    pub(crate) instance: Instance,
    // The selected value in its public shape, and the same as keys for the
    // selection rules.
    pub(crate) value: Controllable<TreeValue>,
    pub(crate) selected: Vec<SharedString>,
    pub(crate) expanded: Controllable<Vec<SharedString>>,
    // The checked value as reported, and every item's checkbox that isn't
    // unchecked.
    pub(crate) checked: Controllable<Vec<SharedString>>,
    pub(crate) checks: HashMap<SharedString, TreeCheckedState>,
    pub(crate) mode: TreeMode,
    pub(crate) checkable: bool,
    pub(crate) check_strategy: TreeCheckStrategy,
    activation: TreeActivation,
    expand_on_press: bool,
    disabled: bool,
    // The root's place in the page, which says whether a fieldset disables it.
    pub(crate) form: Node,
    pub(crate) focusable_when_disabled: bool,
    keep_mounted: bool,
    hidden_until_found: bool,
    pub(crate) current: Option<SharedString>,
    items: Option<Rc<[TreeItemData]>>,
    paths: Option<Rc<[SharedString]>>,
    // Collapsed branches whose group is still animating out.
    pub(crate) exiting: Vec<SharedString>,
    // Branches loading their children, and the ones whose last load failed.
    pub(crate) loading: Vec<SharedString>,
    pub(crate) failed: Vec<SharedString>,
    // Items whose `on_action` work hasn't settled.
    pub(crate) pending: Vec<SharedString>,
    // Bumped after items join, leave or move.
    version: u64,
    on_action: Option<TreeActionCallback>,
    pending_announcement: Option<SharedString>,
    load_children: Option<TreeLoadChildren>,
    on_highlight: Option<TreeHighlightCallback>,
    announcement: Option<TreeAnnouncementFn>,
    group_timing: PresenceTiming,
    scroller: Option<ScrollHandle>,
    pub(crate) roving: RovingFocus<SharedString>,
    selection: Selection<Tree, SharedString>,
    pub(crate) recovery: FocusRecovery<SharedString, TreeItemProps>,
    typeahead: Typeahead,
    announcer: AnnouncePending,
    data: Option<Built>,
    // The shape the items in the page give, rebuilt after they join, leave or
    // move, from these nodes.
    composed: TreeCollection,
    composed_nodes: Vec<TreeCollectionNode>,
    siblings: HashMap<Option<SharedString>, SiblingOrder>,
    // The items in the page, in order, with the props each rendered with.
    pub(crate) item_parts: Parts<SharedString, TreeItemProps>,
    // Branches expanded with Alt or Option, whose branches rendered later
    // expand too.
    deep: HashSet<SharedString>,
    deep_all: bool,
    deep_why: ChangeOptions,
    // Children loading, and the branches that have loaded.
    loaded: HashSet<SharedString>,
    loads: HashMap<SharedString, Load>,
    // A branch whose children are loading because a key asked to step into
    // it.
    step_into: Option<SharedString>,
    step_into_event: Option<InputEvent>,
    // Each group's panel, and the props it rendered with last.
    pub(crate) panels: HashMap<SharedString, Panel>,
    pub(crate) group_props: RefCell<HashMap<SharedString, TreeGroupProps>>,
    // Groups that rendered open with the tree, which don't animate in.
    initial: HashSet<SharedString>,
    revealing: Option<Revealing>,
    seen: Seen,
}

impl Tree {
    pub fn new(options: TreeOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let empty = TreeValue::empty(options.mode);
        let mut value = Controllable::new(
            "value",
            options
                .value
                .clone()
                .unwrap_or_else(|| options.default_value.clone().unwrap_or(empty)),
        ).after_update();
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());
        let mut expanded = Controllable::new(
            "expanded",
            options.expanded.clone().unwrap_or_else(|| options.default_expanded.clone()),
        );
        expanded.set_controlled(options.expanded.clone());
        expanded.set_on_change(options.on_expanded_change.clone());
        let mut checked = Controllable::new(
            "checked",
            options.checked.clone().unwrap_or_else(|| options.default_checked.clone()),
        );
        checked.set_controlled(options.checked.clone());
        checked.set_on_change(options.on_checked_change.clone());
        let mut instance = Instance::new("tree").with_agent(cx);
        instance.debug = options.debug;

        let landed = cx.weak_entity();
        let roving = RovingFocus::new(
            RovingFocusOptions::default().on_items_change(move |window, cx| {
                landed
                    .update(cx, |tree, cx| tree.focus_landed_child(window, cx))
                    .ok();
            }),
            window,
            cx,
        );
        let selection = create_selection(
            SelectionOptions::new(
                |this: &Tree| this.selected.clone(),
                |this: &mut Tree, keys, why, window, cx| {
                    let value = TreeValue::from_keys(this.mode, keys);
                    this.change_value(value, why, window, cx)
                },
                |this: &Tree| this.visible_keys(),
            )
            .multiple(|this: &Tree| this.mode == TreeMode::Multiple)
            .selectable(|this: &Tree, key| this.mode != TreeMode::None && !this.is_disabled_key(key, None)),
        );
        let recovery = FocusRecovery::new(
            |this: &mut Tree, lost: &LostFocus<SharedString, TreeItemProps>, window, cx| this.recover(lost, window, cx),
            window,
            cx,
        );
        cx.on_release(|this: &mut Tree, _cx| {
            while_destroying(|| {
                for load in this.loads.values() {
                    load.signal.abort();
                }
            })
        })
        .detach();

        let data = build(options.items.as_ref(), options.paths.as_ref());
        let seen = Seen {
            value: value.get().clone(),
            checked: checked.get().clone(),
            checkable: options.checkable,
            strategy: options.check_strategy,
            items: options.items.clone(),
            paths: options.paths.clone(),
            version: 0,
            expanded: expanded.get().clone(),
            current: options.current.clone(),
        };
        let mut this = Self {
            instance,
            value,
            selected: Vec::new(),
            expanded,
            checked,
            checks: HashMap::new(),
            mode: options.mode,
            checkable: options.checkable,
            check_strategy: options.check_strategy,
            activation: options.activation,
            expand_on_press: options.expand_on_press,
            disabled: options.disabled,
            form: watch_form_disabled(cx),
            focusable_when_disabled: options.focusable_when_disabled,
            keep_mounted: options.keep_mounted,
            hidden_until_found: options.hidden_until_found,
            current: options.current,
            items: options.items,
            paths: options.paths,
            exiting: Vec::new(),
            loading: Vec::new(),
            failed: Vec::new(),
            pending: Vec::new(),
            version: 0,
            on_action: options.on_action,
            pending_announcement: options.pending_announcement,
            load_children: options.load_children,
            on_highlight: options.on_highlight,
            announcement: options.announcement,
            group_timing: options.group_timing,
            scroller: options.scroller,
            roving,
            selection,
            recovery,
            typeahead: Typeahead::default(),
            announcer: AnnouncePending::new(),
            data,
            composed: create_tree_collection(Vec::<TreeCollectionNode>::new()),
            composed_nodes: Vec::new(),
            siblings: HashMap::new(),
            item_parts: Parts::new(),
            deep: HashSet::new(),
            deep_all: false,
            deep_why: ChangeOptions::reason(Reason::Imperative),
            loaded: HashSet::new(),
            loads: HashMap::new(),
            step_into: None,
            step_into_event: None,
            panels: HashMap::new(),
            group_props: RefCell::new(HashMap::new()),
            initial: HashSet::new(),
            revealing: None,
            seen,
        };

        // The first state: the selection's keys, the checkboxes, and with
        // data, the current item's ancestors expanded.
        this.selected = this.value.get().keys();
        if let (Some(data), Some(current)) = (&this.data, &this.current)
            && options.expanded.is_none()
            && data.tree.has(current)
        {
            let next = dedupe(with(this.expanded.get(), data.tree.ancestors(current)));
            this.expanded.set_held(next);
        }
        this.checks = this.checks_for();
        this.seen = this.snapshot();
        if dev::DEV && this.checkable && this.mode != TreeMode::None {
            dev::warn(
                "has checkboxes and a selection, so items are both checked and selected. Pass `mode: TreeMode::None` unless both mean something.",
                Some(Origin::new(this.instance.label(), this.instance.id())),
            );
        }
        this.sync_groups(window, cx);
        this
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). What they change follows in the same pass, like the
    /// checkboxes new data gives, or a new `current`'s ancestors expanding.
    pub fn set_options(&mut self, options: TreeOptions, window: &mut Window, cx: &mut Context<Self>) {
        // A tree that starts or stops selecting several keeps what was
        // chosen, in its new shape, unless the value is controlled.
        let several = options.mode == TreeMode::Multiple;
        if several != (self.mode == TreeMode::Multiple) && options.value.is_none() {
            let keys = self.value.get().keys();
            self.value.set_held(TreeValue::from_keys(options.mode, keys));
        }
        let mut changed = self.value.set_controlled(options.value);
        self.value.set_on_change(options.on_value_change);
        self.value.set_on_before_change(options.on_before_value_change);
        changed |= self.expanded.set_controlled(options.expanded);
        self.expanded.set_on_change(options.on_expanded_change);
        changed |= self.checked.set_controlled(options.checked);
        self.checked.set_on_change(options.on_checked_change);
        if !same(self.items.as_ref(), options.items.as_ref()) || !same(self.paths.as_ref(), options.paths.as_ref()) {
            let reuse = match (&self.data, &options.items, &options.paths) {
                (Some(Built { source: Source::Items(old), .. }), Some(new), _) => Rc::ptr_eq(old, new),
                (Some(Built { source: Source::Paths(old), .. }), None, Some(new)) => Rc::ptr_eq(old, new),
                _ => false,
            };
            if !reuse {
                self.data = build(options.items.as_ref(), options.paths.as_ref());
            }
            changed = true;
        }
        changed |= self.mode != options.mode
            || self.checkable != options.checkable
            || self.check_strategy != options.check_strategy
            || self.activation != options.activation
            || self.expand_on_press != options.expand_on_press
            || self.disabled != options.disabled
            || self.focusable_when_disabled != options.focusable_when_disabled
            || self.keep_mounted != options.keep_mounted
            || self.hidden_until_found != options.hidden_until_found
            || self.current != options.current;
        self.items = options.items;
        self.paths = options.paths;
        self.mode = options.mode;
        self.checkable = options.checkable;
        self.check_strategy = options.check_strategy;
        self.activation = options.activation;
        self.expand_on_press = options.expand_on_press;
        self.disabled = options.disabled;
        self.focusable_when_disabled = options.focusable_when_disabled;
        self.keep_mounted = options.keep_mounted;
        self.hidden_until_found = options.hidden_until_found;
        self.current = options.current;
        self.on_action = options.on_action;
        self.pending_announcement = options.pending_announcement;
        self.load_children = options.load_children;
        self.on_highlight = options.on_highlight;
        self.announcement = options.announcement;
        self.scroller = options.scroller;
        for panel in self.panels.values() {
            panel.set_timing(options.group_timing.clone());
        }
        self.group_timing = options.group_timing;
        self.instance.debug = options.debug;
        self.selection.sync(self);
        self.derive(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the tree keep its own.
    pub fn set_controlled_value(&mut self, value: Option<TreeValue>, window: &mut Window, cx: &mut Context<Self>) {
        if self.value.set_controlled(value) {
            cx.notify();
        }
        self.selection.sync(self);
        self.derive(window, cx);
    }

    /// Passes the controlled expanded branches, or `None` to let the tree
    /// keep its own.
    pub fn set_controlled_expanded(
        &mut self,
        expanded: Option<Vec<SharedString>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.expanded.set_controlled(expanded) {
            cx.notify();
        }
        self.derive(window, cx);
    }

    /// Passes the controlled checked value, or `None` to let the tree keep
    /// its own.
    pub fn set_controlled_checked(
        &mut self,
        checked: Option<Vec<SharedString>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.checked.set_controlled(checked) {
            cx.notify();
        }
        self.derive(window, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The items `items` or `paths` give, to render them, or `None` for a
    /// tree without data.
    pub fn items(&self) -> Option<Rc<[TreeItemData]>> {
        self.data.as_ref().map(|data| data.items.clone())
    }

    // The shape the items in the page give (`fromParts`), or with data, the
    // data's (`shape`).
    fn shape(&self) -> &TreeCollection {
        match &self.data {
            Some(data) => &data.tree,
            None => &self.composed,
        }
    }

    /// The props the item with `key` rendered with last.
    pub(crate) fn item_props(&self, key: &SharedString) -> Option<TreeItemProps> {
        self.item_parts.info(key)
    }

    fn parent_of(&self, key: &SharedString) -> Option<SharedString> {
        match &self.data {
            Some(data) => data.tree.parent(key),
            None => self.item_props(key).and_then(|props| props.parent),
        }
    }

    // Outermost first, from the parent `first`. A parent chain that loops
    // stops where it repeats.
    fn ancestors_from(&self, key: &SharedString, first: Option<SharedString>) -> Vec<SharedString> {
        let mut found = Vec::new();
        let mut seen = HashSet::from([key.clone()]);
        let mut up = first;
        while let Some(parent) = up {
            if !seen.insert(parent.clone()) {
                break;
            }
            up = self.parent_of(&parent);
            found.insert(0, parent);
        }
        found
    }

    // Outermost first.
    fn ancestors(&self, key: &SharedString) -> Vec<SharedString> {
        self.ancestors_from(key, self.parent_of(key))
    }

    pub(crate) fn is_branch(&self, key: &SharedString, props: Option<&TreeItemProps>) -> bool {
        let own = match props {
            Some(props) => props.branch,
            None => self.item_props(key).and_then(|props| props.branch),
        };
        own.or_else(|| self.data.as_ref().map(|data| data.tree.is_branch(key)))
            .unwrap_or(false)
    }

    /// Whether the whole tree is disabled: by its `disabled` option, or a
    /// disabled fieldset around it.
    pub fn is_disabled(&self) -> bool {
        is_disabled(self.disabled, self.form.form_disabled())
    }

    pub(crate) fn is_disabled_key(&self, key: &SharedString, props: Option<&TreeItemProps>) -> bool {
        let own = match props {
            Some(props) => props.disabled,
            None => self.item_props(key).is_some_and(|props| props.disabled),
        };
        self.is_disabled()
            || own
            || self
                .data
                .as_ref()
                .and_then(|data| data.by_key.get(key))
                .is_some_and(|item| item.disabled)
    }

    // An item's depth, from data, or from the parents its props name.
    pub(crate) fn level(&self, key: &SharedString, props: Option<&TreeItemProps>) -> usize {
        if let Some(data) = &self.data
            && data.tree.has(key)
        {
            return data.tree.level(key);
        }
        let first = match props {
            Some(props) => props.parent.clone(),
            None => self.item_props(key).and_then(|props| props.parent),
        };
        let mut level = 1;
        let mut seen = HashSet::from([key.clone()]);
        let mut up = first;
        while let Some(parent) = up {
            if !seen.insert(parent.clone()) {
                break;
            }
            level += 1;
            up = self.item_props(&parent).and_then(|props| props.parent);
        }
        level
    }

    // Whether every ancestor is expanded, so the item shows.
    fn shows(&self, key: &SharedString) -> bool {
        let expanded = self.expanded.get();
        self.ancestors(key).iter().all(|ancestor| expanded.contains(ancestor))
    }

    fn focusable(&self, key: &SharedString, props: Option<&TreeItemProps>) -> bool {
        self.shows(key) && (self.focusable_when_disabled || !self.is_disabled_key(key, props))
    }

    // The item that takes the tab stop first: the current page's, else the
    // first selected.
    fn preferred_key(&self) -> Option<&SharedString> {
        self.current.as_ref().or(self.selected.first())
    }

    // What an item tells the roving focus as it renders. Before the stop is
    // settled, the preferred item holds it when it shows, else the first
    // item.
    pub(crate) fn member(&self, key: &SharedString, props: &TreeItemProps, state: &TreeItemState) -> RovingMember {
        let focusable = self.focusable(key, Some(props));
        let claims = focusable && !state.disabled && {
            match self.preferred_key() {
                None => true,
                Some(preferred) => match &self.data {
                    Some(data) if !(data.tree.has(preferred) && self.shows(preferred)) => true,
                    _ => key == preferred,
                },
            }
        };
        RovingMember::new()
            .disabled(state.disabled)
            .focusable(focusable)
            .preferred(state.current || state.selected)
            .claims(claims)
    }

    // With data, the keys of the rows that show and can take focus, rendered
    // or not.
    fn data_keys(&self) -> Option<Vec<SharedString>> {
        let data = self.data.as_ref()?;
        Some(
            data.tree
                .visible(self.expanded.get())
                .iter()
                .filter(|key| self.focusable_when_disabled || !self.is_disabled_key(key, None))
                .cloned()
                .collect(),
        )
    }

    // The keys of the items that show, in order: from data, or the items in
    // the page.
    fn visible_keys(&self) -> Vec<SharedString> {
        self.data_keys()
            .unwrap_or_else(|| self.roving.items().into_iter().map(|item| item.key).collect())
    }

    // What names an item, when the tree knows: its `label` prop, else the
    // data's.
    pub(crate) fn known_label(&self, key: &SharedString, props: Option<&TreeItemProps>) -> Option<SharedString> {
        let own = match props {
            Some(props) => props.label.clone(),
            None => self.item_props(key).and_then(|props| props.label),
        };
        own.or_else(|| {
            self.data
                .as_ref()
                .and_then(|data| data.by_key.get(key))
                .map(|item| item.label.clone())
        })
    }

    // The text typeahead and announcements read for an item.
    fn label_text(&self, key: &SharedString) -> SharedString {
        self.known_label(key, None).unwrap_or_else(|| key.clone())
    }

    // With data, every item's place among its siblings; without, its place
    // among the items in the page, as of the last frame.
    pub(crate) fn position(&self, key: &SharedString, props: &TreeItemProps) -> Option<TreePosition> {
        if let Some(data) = &self.data
            && data.tree.has(key)
        {
            return Some(data.tree.position(key));
        }
        let order = self.siblings.get(&props.parent)?;
        let index = *order.index.get(key)?;
        Some(TreePosition {
            index,
            size: order.size,
        })
    }

    fn snapshot(&self) -> Seen {
        Seen {
            value: self.value.get().clone(),
            checked: self.checked.get().clone(),
            checkable: self.checkable,
            strategy: self.check_strategy,
            items: self.items.clone(),
            paths: self.paths.clone(),
            version: self.version,
            expanded: self.expanded.get().clone(),
            current: self.current.clone(),
        }
    }

    // Every item's checkbox that isn't unchecked, from the checked value and
    // the shape.
    fn checks_for(&self) -> HashMap<SharedString, TreeCheckedState> {
        if !self.checkable {
            return HashMap::new();
        }
        let tree = self.shape();
        let read = tree_checked_states(tree, self.checked.get(), self.check_strategy);
        let mut checks = HashMap::new();
        for key in tree.keys() {
            let checked = read.get(&key);
            if checked != Checked::False {
                checks.insert(key, checked);
            }
        }
        // Checked items not rendered yet show as checked.
        for key in self.checked.get() {
            if !tree.has(key) {
                checks.insert(key.clone(), Checked::True);
            }
        }
        checks
    }

    // Derived state, kept in step with what it's derived from: the
    // selection's keys, the checkboxes, and the groups still animating out.
    // A new current page, or data arriving for one, reveals it. The
    // original's store subscription: every change calls it.
    fn derive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = self.snapshot();
        let before = std::mem::replace(&mut self.seen, now.clone());
        if now.value != before.value {
            self.selected = now.value.keys();
        }
        if (now.checkable || before.checkable)
            && (now.checked != before.checked
                || now.checkable != before.checkable
                || now.strategy != before.strategy
                || !same(now.items.as_ref(), before.items.as_ref())
                || !same(now.paths.as_ref(), before.paths.as_ref())
                || now.version != before.version)
        {
            let checks = self.checks_for();
            if checks != self.checks {
                self.checks = checks;
                cx.notify();
            }
        }
        if now.expanded != before.expanded {
            let closed: Vec<SharedString> = before
                .expanded
                .iter()
                .filter(|key| !now.expanded.contains(key) && held(&self.instance, "group", Some(key)))
                .cloned()
                .collect();
            let staying = self.exiting.iter().filter(|key| !now.expanded.contains(key)).cloned();
            self.exiting = dedupe(staying.chain(closed));
            // A controlled tree expands once its `expanded` comes back, and
            // its groups show a moment later.
            if let Some(revealing) = self.revealing.take_if(|revealing| revealing.before != now.expanded)
                && self.shows(&revealing.value)
            {
                cx.defer_in(window, move |tree, window, cx| tree.land(&revealing.value, window, cx));
            }
        }
        let had_data = before.items.is_some() || before.paths.is_some();
        let has_data = now.items.is_some() || now.paths.is_some();
        if let Some(current) = now.current
            && (Some(&current) != before.current.as_ref() || (!had_data && has_data))
        {
            // Once the update that brought it is done, so `on_expanded_change`
            // never runs inside the app's own render, which handed it over.
            cx.defer_in(window, move |tree, window, cx| tree.reveal_current(&current, window, cx));
        }
        self.sync_groups(window, cx);
    }

    fn reveal_current(&mut self, current: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let path = match &self.data {
            Some(data) if data.tree.has(current) => data.tree.ancestors(current),
            Some(_) => Vec::new(),
            None => self.ancestors(current),
        };
        let expanded = self.expanded.get();
        let hidden: Vec<SharedString> = path.into_iter().filter(|ancestor| !expanded.contains(ancestor)).collect();
        if !hidden.is_empty() {
            let next = with(expanded, hidden);
            self.change_expanded(next, ChangeOptions::reason(Reason::Imperative), window, cx);
        }
    }

    // Starts a render of the items, from the root.
    pub(crate) fn begin(&self, cx: &App) {
        self.item_parts.begin();
        self.roving.begin(cx);
        self.recovery.begin(cx);
    }

    // After a frame that rendered the tree: items that joined, left or moved
    // bump the version once, which rebuilds the shape the items in the page
    // give and their places among their siblings, and branches that render
    // inside one expanded deeply expand too, once per batch.
    pub(crate) fn rendered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.item_parts.entries();
        let nodes: Vec<TreeCollectionNode> = entries
            .iter()
            .map(|(key, props)| TreeCollectionNode {
                key: key.clone(),
                parent: props.parent.clone(),
                branch: props.branch.unwrap_or(false),
            })
            .collect();
        if nodes == self.composed_nodes {
            return;
        }
        let joined: Vec<SharedString> = {
            let before: HashSet<&SharedString> = self.composed_nodes.iter().map(|node| &node.key).collect();
            nodes
                .iter()
                .filter(|node| !before.contains(&node.key))
                .map(|node| node.key.clone())
                .collect()
        };
        let mut siblings: HashMap<Option<SharedString>, SiblingOrder> = HashMap::new();
        for node in &nodes {
            let order = siblings.entry(node.parent.clone()).or_default();
            order.size += 1;
            order.index.insert(node.key.clone(), order.size);
        }
        self.siblings = siblings;
        self.composed = create_tree_collection(nodes.clone());
        self.composed_nodes = nodes;
        self.version += 1;
        let deep: Vec<SharedString> = joined
            .into_iter()
            .filter(|key| self.is_branch(key, None) && self.in_deep(key))
            .filter(|key| !self.expanded.get().contains(key) && !self.needs_load(key))
            .collect();
        if !deep.is_empty() {
            let next = with(self.expanded.get(), deep);
            let why = self.deep_why.clone();
            self.change_expanded(next, why, window, cx);
        }
        self.derive(window, cx);
        let bound: HashSet<&SharedString> = entries.iter().map(|(key, _)| key).collect();
        self.panels.retain(|key, panel| panel.is_bound() || bound.contains(key));
        cx.notify();
    }

    fn keeps_mounted(&self, props: &TreeGroupProps) -> bool {
        props.keep_mounted.unwrap_or(self.keep_mounted)
    }

    pub(crate) fn findable(&self, props: &TreeGroupProps) -> bool {
        props.hidden_until_found.unwrap_or(self.hidden_until_found)
    }

    // Whether a group is in the page: expanded, animating out, or kept.
    pub(crate) fn group_present(&self, key: &SharedString, props: &TreeGroupProps) -> bool {
        self.expanded.get().contains(key)
            || self.exiting.contains(key)
            || self.keeps_mounted(props)
            || self.findable(props)
    }

    // The groups that may need binding: expanded, leaving, kept or bound.
    fn group_keys(&self) -> Vec<SharedString> {
        let kept: Vec<SharedString> = self
            .group_props
            .borrow()
            .iter()
            .filter(|(_, props)| self.keeps_mounted(props) || self.findable(props))
            .map(|(key, _)| key.clone())
            .collect();
        let bound = self
            .panels
            .iter()
            .filter(|(_, panel)| panel.is_bound())
            .map(|(key, _)| key.clone());
        dedupe(
            self.expanded
                .get()
                .iter()
                .cloned()
                .chain(self.exiting.iter().cloned())
                .chain(kept)
                .chain(bound),
        )
    }

    fn sync_groups(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for key in self.group_keys() {
            self.sync_group(&key, window, cx);
        }
    }

    // Binds, checks or unbinds a group's panel for the state now
    // (`bindGroup`): the group's enter and exit animations.
    pub(crate) fn sync_group(&mut self, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let props = self.group_props.borrow().get(key).copied().unwrap_or_default();
        let present = self.group_present(key, &props);
        let open = self.expanded.get().contains(key);
        let panel = match self.panels.get(key) {
            Some(panel) => panel.clone(),
            None if present => {
                // Open before any item settled means it rendered open with
                // the page.
                if self.roving.stop().is_none() && open {
                    self.initial.insert(key.clone());
                }
                let panel = Panel::new(cx).with_timing(self.group_timing.clone());
                self.panels.insert(key.clone(), panel.clone());
                panel
            }
            None => return,
        };
        if !open {
            self.initial.remove(key);
        }
        let opening = if self.initial.contains(key) {
            Opening::Mount
        } else {
            Opening::Animate
        };
        let settled = key.clone();
        let exited = key.clone();
        let binding = PanelBinding::default()
            .open(open)
            .exiting(self.exiting.contains(key))
            .opening(opening)
            .trigger(Some(self.roving.focus_handle(key, cx)))
            .settled(move |this: &mut Self, _open, _window, _cx| {
                this.initial.remove(&settled);
            })
            .exited(move |this: &mut Self, window, cx| this.finish_exit(&exited, window, cx));
        panel.sync(self, present, &binding, window, cx);
    }

    fn finish_exit(&mut self, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.exiting.retain(|other| other != key);
        cx.notify();
        self.sync_group(key, window, cx);
    }

    fn say(&self, r#type: TreeAnnouncementType, key: &SharedString, window: &mut Window, cx: &mut App) {
        if matches!(r#type, TreeAnnouncementType::Expanded | TreeAnnouncementType::Collapsed) && !is_apple(cx) {
            return;
        }
        let change = TreeAnnouncement {
            r#type,
            value: key.clone(),
            label: self.label_text(key),
        };
        let text = match &self.announcement {
            Some(announcement) => announcement(&change),
            None => Some(default_announcement(&change)),
        };
        if let Some(text) = text {
            announce(&text, window, cx);
        }
    }

    fn in_deep(&self, key: &SharedString) -> bool {
        self.deep_all || self.ancestors(key).iter().any(|ancestor| self.deep.contains(ancestor))
    }

    fn needs_load(&self, key: &SharedString) -> bool {
        self.load_children.is_some()
            && !self.loaded.contains(key)
            && self.is_branch(key, None)
            && self.shape().children(Some(key)).is_empty()
    }

    fn set_loading(&mut self, key: &SharedString, on: bool, failed: Option<bool>) {
        let failed = failed.unwrap_or_else(|| self.failed.contains(key));
        if on {
            self.loading.push(key.clone());
        } else {
            self.loading.retain(|other| other != key);
        }
        if failed {
            self.failed = dedupe(with(&self.failed, [key.clone()]));
        } else {
            self.failed.retain(|other| other != key);
        }
    }

    fn load(&mut self, key: &SharedString, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(running) = self.loads.remove(key) {
            running.signal.abort();
            self.set_loading(key, false, None);
            cx.notify();
            return false;
        }
        let Some(load_children) = self.load_children.clone() else {
            return false;
        };
        let signal = TreeLoadSignal::default();
        let details = TreeLoadDetails { signal: signal.clone() };
        match load_children(key, &details, window, cx) {
            ActionOutcome::Done => {
                self.loaded.insert(key.clone());
                let next = with(self.expanded.get(), [key.clone()]);
                self.change_expanded(next, why, window, cx)
            }
            ActionOutcome::Canceled => false,
            ActionOutcome::Failed(_) => {
                self.set_loading(key, false, Some(true));
                self.say(TreeAnnouncementType::LoadFailed, key, window, cx);
                cx.notify();
                false
            }
            ActionOutcome::Pending(work) => {
                self.set_loading(key, true, Some(false));
                self.say(TreeAnnouncementType::Loading, key, window, cx);
                let loaded = key.clone();
                let given = signal.clone();
                let task = cx.spawn_in(window, async move |this, cx| {
                    let ok = work.await.is_ok();
                    this.update_in(cx, |tree, window, cx| tree.load_done(&loaded, &given, ok, why, window, cx))
                        .ok();
                });
                self.loads.insert(
                    key.clone(),
                    Load {
                        signal,
                        task: Some(task),
                    },
                );
                cx.notify();
                false
            }
        }
    }

    fn load_done(
        &mut self,
        key: &SharedString,
        signal: &TreeLoadSignal,
        ok: bool,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if signal.aborted() {
            return;
        }
        // The task running this finishes on its own.
        if let Some(task) = self.loads.remove(key).and_then(|mut load| load.task.take()) {
            task.detach();
        }
        self.set_loading(key, false, Some(!ok));
        cx.notify();
        if !ok {
            return self.say(TreeAnnouncementType::LoadFailed, key, window, cx);
        }
        self.loaded.insert(key.clone());
        let next = with(self.expanded.get(), [key.clone()]);
        self.change_expanded(next, why, window, cx);
    }

    fn change_value(&mut self, value: TreeValue, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let changed = self.value.change(&self.instance, value, why, window, cx);
        self.derive(window, cx);
        if changed {
            cx.notify();
        }
        changed
    }

    fn change_expanded(
        &mut self,
        expanded: Vec<SharedString>,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let changed = self.expanded.change(&self.instance, dedupe(expanded), why, window, cx);
        self.derive(window, cx);
        if changed {
            cx.notify();
        }
        changed
    }

    fn change_checked(
        &mut self,
        checked: Vec<SharedString>,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let changed = self.checked.change(&self.instance, checked, why, window, cx);
        self.derive(window, cx);
        if changed {
            cx.notify();
        }
        changed
    }

    fn set_open(
        &mut self,
        key: &SharedString,
        opening: bool,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !opening {
            self.deep.remove(key);
            if let Some(load) = self.loads.remove(key) {
                load.signal.abort();
            }
            // A step into a branch ends with it, so a later expand doesn't
            // take focus or selection.
            if self.step_into.as_ref() == Some(key) {
                self.step_into = None;
                self.step_into_event = None;
            }
        }
        if self.expanded.get().contains(key) == opening && !self.loads.contains_key(key) {
            return false;
        }
        if opening && self.needs_load(key) {
            return self.load(key, why, window, cx);
        }
        let next = if opening {
            with(self.expanded.get(), [key.clone()])
        } else {
            self.expanded.get().iter().filter(|other| *other != key).cloned().collect()
        };
        let changed = self.change_expanded(next, why, window, cx);
        if changed && self.roving.focus_handle(key, cx).is_focused(window) {
            let r#type = if opening {
                TreeAnnouncementType::Expanded
            } else {
                TreeAnnouncementType::Collapsed
            };
            self.say(r#type, key, window, cx);
        }
        changed
    }

    // Expands a branch and everything in it that's known, and anything in it
    // that renders later.
    fn expand_deep(&mut self, key: &SharedString, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.deep.insert(key.clone());
        self.deep_why = why.clone();
        let below: Vec<SharedString> = self
            .shape()
            .descendants(key)
            .into_iter()
            .filter(|child| self.is_branch(child, None) && !self.needs_load(child))
            .collect();
        let loads = self.needs_load(key);
        let own = (!loads).then(|| key.clone());
        let next = with(self.expanded.get(), own.into_iter().chain(below));
        let changed = self.change_expanded(next, why.clone(), window, cx);
        if loads {
            self.load(key, why, window, cx);
        }
        changed
    }

    fn collapse_deep(&mut self, key: &SharedString, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut below: HashSet<SharedString> = self.shape().descendants(key).into_iter().collect();
        below.insert(key.clone());
        for branch in &below {
            self.deep.remove(branch);
        }
        let next = self
            .expanded
            .get()
            .iter()
            .filter(|other| !below.contains(*other))
            .cloned()
            .collect();
        self.change_expanded(next, why, window, cx)
    }

    fn highlight(
        &self,
        key: &SharedString,
        index: Option<usize>,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(on_highlight) = &self.on_highlight {
            on_highlight(key, &ListHighlightDetails { index, event }, window, cx);
        }
    }

    // The place of `key` among the items that show.
    fn row_of(&self, key: &SharedString) -> Option<usize> {
        self.visible_keys().iter().position(|other| other == key)
    }

    // With one item selectable, automatic activation selects wherever a key
    // moves focus.
    fn follow(&mut self, key: &SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == TreeMode::Single && self.activation == TreeActivation::Automatic {
            let why = ChangeOptions {
                reason: Some(Reason::ListNavigation),
                event,
                source: None,
            };
            let selection = self.selection.clone();
            selection.choose(self, key, why, window, cx);
        }
    }

    // Focus waiting for a loading branch's first child, which takes it once
    // it's in the page.
    fn focus_landed_child(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.step_into.clone() else {
            return;
        };
        if self.loads.contains_key(&parent) {
            return;
        }
        let Some(child) = self
            .roving
            .items()
            .into_iter()
            .find(|item| self.parent_of(&item.key).as_ref() == Some(&parent))
        else {
            return;
        };
        // Selects only while focus still waits on the branch, not once the
        // user moved on or left.
        let stayed = self.roving.focus_handle(&parent, cx).is_focused(window);
        self.step_into = None;
        self.roving.focus(Some(&child.key), window, cx);
        let event = self.step_into_event.take();
        if stayed {
            self.follow(&child.key, event, window, cx);
        }
    }

    // Focus inside a branch that collapses goes to the outermost collapsed
    // ancestor, else the parent, else the first item that shows.
    fn recover(
        &mut self,
        lost: &LostFocus<SharedString, TreeItemProps>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<FocusHandle> {
        let first = match &self.data {
            Some(data) => data.tree.parent(&lost.key),
            None => lost.props.parent.clone(),
        };
        let path = self.ancestors_from(&lost.key, first);
        let expanded = self.expanded.get();
        let target = path
            .iter()
            .find(|ancestor| !expanded.contains(ancestor))
            .or(path.last())
            .filter(|target| self.item_parts.contains(target));
        match target {
            Some(target) => Some(self.roving.focus_handle(target, cx)),
            None => self
                .roving
                .items()
                .into_iter()
                .find(|item| item.key != lost.key)
                .map(|item| item.handle),
        }
    }

    fn toggle_checked(&mut self, key: &SharedString, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.checkable || self.is_disabled_key(key, None) {
            return false;
        }
        let checkable = |other: &str| !self.is_disabled_key(&SharedString::from(other.to_string()), None);
        let options = ToggleTreeCheckedOptions::default()
            .strategy(self.check_strategy)
            .checkable(&checkable);
        let next = toggle_tree_checked(self.shape(), self.checked.get(), key, &options);
        self.change_checked(next, why, window, cx)
    }

    fn pending_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = !self.pending.is_empty();
        let text = self.pending_announcement.clone();
        self.announcer.update(pending, text, announce, window, cx);
    }

    // An `on_action` that goes on marks its item pending until it settles.
    // Presses meanwhile are ignored.
    fn act(&mut self, key: &SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.on_action.clone() else {
            return;
        };
        if self.pending.contains(key) {
            return;
        }
        let ActionOutcome::Pending(work) = action(key, &TreeActionDetails { event }, window, cx) else {
            return;
        };
        self.pending.push(key.clone());
        self.pending_changed(window, cx);
        cx.notify();
        let settled = key.clone();
        // It runs to its end even if the tree goes, as the original's
        // Promise does.
        cx.spawn_in(window, async move |this, cx| {
            let _ = work.await;
            this.update_in(cx, |tree, window, cx| {
                tree.pending.retain(|other| *other != settled);
                tree.pending_changed(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // Whether focus is on a control inside the item, like a rename field,
    // rather than on the item or another part of the tree.
    fn focus_in_control(&self, key: &SharedString, window: &Window, cx: &App) -> bool {
        let Some(focused) = window.focused(cx) else {
            return false;
        };
        let own = self.roving.focus_handle(key, cx);
        if focused == own || !own.contains(&focused, window) {
            return false;
        }
        let item = self
            .item_parts
            .keys()
            .iter()
            .any(|other| other != key && self.roving.focus_handle(other, cx) == focused);
        let group = self.panels.values().any(|panel| panel.focus_handle() == focused);
        !item && !group
    }

    // A press on an item, or Enter: focus, selection, expanding a branch,
    // then `on_action`.
    pub(crate) fn press(&mut self, key: &SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.roving.focus_handle(key, cx);
        // A control inside the item, like a rename field, keeps focus.
        if !handle.is_focused(window) && !self.focus_in_control(key, window, cx) {
            window.focus(&handle, cx);
        }
        let active = self.roving.stop();
        if active.as_ref() != Some(key) {
            self.roving.remember(key, cx);
        }
        if self.is_disabled_key(key, None) || self.pending.contains(key) {
            return;
        }
        let modifiers = modifiers_of(event.as_ref());
        let why = ChangeOptions {
            reason: Some(Reason::ItemPress),
            event: event.clone(),
            source: None,
        };
        let expanded = self.expanded.get().contains(key);
        let selection = self.selection.clone();
        if self.mode == TreeMode::Multiple && modifiers.shift {
            selection.extend(self, key, ExtendOptions::new().from(active), why.clone(), window, cx);
        } else if self.mode == TreeMode::Multiple || modifiers.control || modifiers.platform {
            selection.toggle(self, key, why.clone(), window, cx);
        } else {
            selection.choose(self, key, why.clone(), window, cx);
        }
        if self.is_branch(key, None) && self.expand_on_press {
            self.set_open(key, !expanded, why, window, cx);
        }
        self.act(key, event, window, cx);
    }

    pub(crate) fn keydown(&mut self, event: &KeyDownEvent, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if window.default_prevented() {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        let r#mod = modifiers.control || modifiers.platform;
        let pressed = Key::of(&event.keystroke);
        let input = InputEvent::from(event);
        let keys = self.data_keys();
        let mut wanted: Option<SharedString> = None;
        let roving = self.roving.clone();
        let moved = roving.keydown_with(
            event,
            key,
            |event, context, _cx| {
                let list: Vec<SharedString> = match &keys {
                    Some(keys) => keys.clone(),
                    None => context.items.iter().map(|item| item.key.clone()).collect(),
                };
                wanted = move_target(event, &context.from.key, &list);
                wanted.clone()
            },
            window,
            cx,
        );
        // A row that shows but isn't rendered, like in a virtualized list,
        // takes focus once it is.
        let target = moved
            .as_ref()
            .map(|moved| moved.key.clone())
            .or_else(|| wanted.clone().filter(|wanted| wanted != key));
        if let Some(target) = target {
            if moved.is_none() {
                window.prevent_default();
                self.roving.focus(Some(&target), window, cx);
            }
            self.highlight(&target, self.row_of(&target), Some(input.clone()), window, cx);
            let home_or_end = matches!(pressed, Key::Home | Key::End);
            let why = ChangeOptions::event_of(Reason::ListNavigation, input.clone());
            if self.mode == TreeMode::Multiple && modifiers.shift && (!home_or_end || r#mod) {
                let selection = self.selection.clone();
                selection.extend(self, &target, ExtendOptions::new().from(Some(key.clone())), why, window, cx);
            } else if !modifiers.shift {
                self.follow(&target, Some(input), window, cx);
            }
            return;
        }
        if wanted.is_some() {
            return;
        }
        let rtl = is_rtl(cx);
        let (forward, backward) = if rtl {
            (Key::ArrowLeft, Key::ArrowRight)
        } else {
            (Key::ArrowRight, Key::ArrowLeft)
        };
        let branch = self.is_branch(key, None);
        let expanded = branch && self.expanded.get().contains(key);
        let disabled = self.is_disabled_key(key, None);
        let nav = || ChangeOptions::event_of(Reason::ListNavigation, input.clone());
        if pressed == forward && !r#mod {
            if !branch {
                return;
            }
            if modifiers.alt {
                if !disabled {
                    self.expand_deep(key, nav(), window, cx);
                }
            } else if !expanded {
                if disabled {
                    return;
                }
                if self.loads.contains_key(key) {
                    self.step_into = Some(key.clone());
                    self.step_into_event = Some(input.clone());
                } else {
                    self.set_open(key, true, nav(), window, cx);
                }
            } else {
                let child = self
                    .roving
                    .items()
                    .into_iter()
                    .find(|item| self.parent_of(&item.key).as_ref() == Some(key));
                let first = child.as_ref().map(|child| child.key.clone()).or_else(|| {
                    self.data
                        .as_ref()
                        .and_then(|data| data.tree.children(Some(key)).first().cloned())
                });
                if let Some(child) = &child {
                    self.roving.focus(Some(&child.key), window, cx);
                    self.highlight(&child.key, self.row_of(&child.key), Some(input.clone()), window, cx);
                } else if let Some(first) = &first {
                    self.roving.focus(Some(first), window, cx);
                }
                if let Some(first) = first {
                    self.follow(&first, Some(input.clone()), window, cx);
                }
            }
        } else if pressed == backward && !r#mod {
            if branch && modifiers.alt && !disabled {
                self.collapse_deep(key, nav(), window, cx);
            } else if expanded && !disabled {
                self.set_open(key, false, nav(), window, cx);
            } else {
                let Some(parent) = self.parent_of(key) else {
                    return;
                };
                self.roving.focus(Some(&parent), window, cx);
                self.highlight(&parent, self.row_of(&parent), Some(input.clone()), window, cx);
                self.follow(&parent, Some(input.clone()), window, cx);
            }
        } else if pressed == Key::Character("*".into()) {
            let parent = self.parent_of(key);
            let siblings: Vec<SharedString> = self
                .shape()
                .children(parent.as_deref())
                .iter()
                .filter(|sibling| {
                    self.is_branch(sibling, None) && !self.is_disabled_key(sibling, None) && !self.needs_load(sibling)
                })
                .cloned()
                .collect();
            let next = with(self.expanded.get(), siblings);
            self.change_expanded(next, nav(), window, cx);
        } else if r#mod
            && !modifiers.shift
            && !modifiers.alt
            && matches!(&pressed, Key::Character(typed) if typed.to_lowercase() == "a")
        {
            if self.mode != TreeMode::Multiple {
                return;
            }
            let selection = self.selection.clone();
            selection.toggle_all(self, nav(), window, cx);
        } else if pressed == Key::Enter && !r#mod && !modifiers.alt {
            self.press(key, Some(input.clone()), window, cx);
        } else if pressed == Key::Space && !self.typeahead.active() {
            let why = ChangeOptions::event_of(Reason::ItemPress, input.clone());
            let selection = self.selection.clone();
            if self.checkable {
                self.toggle_checked(key, why, window, cx);
            } else if self.mode == TreeMode::Multiple && modifiers.shift {
                selection.extend(self, key, ExtendOptions::new().from(Some(key.clone())), why, window, cx);
            } else if self.mode != TreeMode::None {
                selection.toggle(self, key, why, window, cx);
            } else {
                return;
            }
        } else if is_typeahead_key(&event.keystroke) {
            let keys = if self.data.is_some() {
                self.visible_keys()
            } else {
                self.roving.items().into_iter().map(|item| item.key).collect()
            };
            let labels: Vec<SharedString> = keys.iter().map(|other| self.label_text(other)).collect();
            let current = keys.iter().position(|other| other == key);
            let Some(found) = self.typeahead.search(&pressed, &labels, current) else {
                return;
            };
            let to = keys[found].clone();
            self.roving.focus(Some(&to), window, cx);
            self.highlight(&to, Some(found), Some(input.clone()), window, cx);
            self.follow(&to, Some(input), window, cx);
        } else {
            return;
        }
        window.prevent_default();
    }

    pub(crate) fn press_indicator(
        &mut self,
        key: &SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_disabled_key(key, None) {
            return;
        }
        if self.roving.stop().as_ref() != Some(key) {
            self.roving.remember(key, cx);
        }
        let alt = modifiers_of(event.as_ref()).alt;
        let why = ChangeOptions {
            reason: Some(Reason::TriggerPress),
            event,
            source: None,
        };
        let expanded = self.expanded.get().contains(key);
        if alt {
            if expanded {
                self.collapse_deep(key, why, window, cx);
            } else {
                self.expand_deep(key, why, window, cx);
            }
        } else {
            self.set_open(key, !expanded, why, window, cx);
        }
    }

    pub(crate) fn press_checkbox(
        &mut self,
        key: &SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = self.roving.focus_handle(key, cx);
        if !handle.is_focused(window) {
            window.focus(&handle, cx);
        }
        let why = ChangeOptions {
            reason: Some(Reason::ItemPress),
            event,
            source: None,
        };
        self.toggle_checked(key, why, window, cx);
    }

    // Brings an item into view in `scroller` once it's laid out, the
    // original's `scrollIntoView({ block: "nearest" })`.
    fn scroll_into_view(&self, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scroller) = self.scroller.clone() else {
            return;
        };
        let key = key.clone();
        let entity = cx.weak_entity();
        // The item may only show from the next frame, which lays it out; its
        // bounds are read once that frame is drawn.
        window.on_next_frame(move |window, _cx| window.on_next_frame(move |_window, cx| {
            let Some(bounds) = entity.upgrade().and_then(|tree| {
                tree.read(cx)
                    .instance
                    .bound_parts()
                    .into_iter()
                    .find(|part| part.slot == "item" && part.key.as_ref() == Some(&key))
                    .and_then(|part| part.bounds)
            }) else {
                return;
            };
            let viewport = scroller.bounds();
            let offset = scroller.offset();
            let axis = |scroll: Pixels, size: Pixels, start: Pixels, length: Pixels| {
                -scroll_to_show(-scroll, size, start - scroll, length, Pixels::ZERO, Pixels::ZERO)
            };
            let x = axis(offset.x, viewport.size.width, bounds.left() - viewport.left(), bounds.size.width);
            let y = axis(offset.y, viewport.size.height, bounds.top() - viewport.top(), bounds.size.height);
            if x != offset.x || y != offset.y {
                scroller.set_offset(point(x, y));
            }
        }));
    }

    // Reveals an item that shows now: reports it, focuses it and scrolls it
    // into view.
    fn land(&mut self, value: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.row_of(value) {
            self.highlight(value, Some(index), None, window, cx);
        }
        self.roving.focus(Some(value), window, cx);
        self.scroll_into_view(value, window, cx);
    }

    /// The values of the expanded branches.
    pub fn get_expanded(&self) -> &Vec<SharedString> {
        self.expanded.get()
    }

    /// Expands exactly these branches. Returns false when canceled.
    pub fn set_expanded(
        &mut self,
        expanded: Vec<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.change_expanded(expanded, options, window, cx)
    }

    /// Whether a branch is expanded.
    pub fn is_expanded(&self, value: &str) -> bool {
        self.expanded.get().iter().any(|key| key.as_ref() == value)
    }

    /// Expands a branch, loading its children first with `load_children`
    /// when it has none.
    pub fn expand(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.set_open(&value.into(), true, options, window, cx)
    }

    /// Collapses a branch. Focus inside it moves to the branch.
    pub fn collapse(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.set_open(&value.into(), false, options, window, cx)
    }

    /// Expands a collapsed branch, or collapses an expanded one.
    pub fn toggle(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let value = value.into();
        let opening = !self.expanded.get().contains(&value);
        self.set_open(&value, opening, options, window, cx)
    }

    /// Expands every branch, and every branch that renders inside one later.
    pub fn expand_all(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.deep_all = true;
        self.deep_why = options.clone();
        let branches: Vec<SharedString> = self
            .shape()
            .keys()
            .into_iter()
            .filter(|key| self.is_branch(key, None) && !self.needs_load(key))
            .collect();
        let next = with(self.expanded.get(), branches);
        self.change_expanded(next, options, window, cx)
    }

    /// Collapses every branch.
    pub fn collapse_all(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.deep_all = false;
        self.deep.clear();
        self.change_expanded(Vec::new(), options, window, cx)
    }

    /// The selected value: one value or none, or a list with
    /// `TreeMode::Multiple`.
    pub fn get_value(&self) -> &TreeValue {
        self.value.get()
    }

    /// Selects these items. Returns false when canceled.
    pub fn set_value(&mut self, value: TreeValue, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.change_value(value, options, window, cx)
    }

    /// The checked value, as `check_strategy` reports it.
    pub fn get_checked(&self) -> &Vec<SharedString> {
        self.checked.get()
    }

    /// Sets the checked value. Returns false when canceled.
    pub fn set_checked(
        &mut self,
        checked: Vec<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.change_checked(checked, options, window, cx)
    }

    /// Whether an item's checkbox is checked, unchecked or indeterminate.
    pub fn checked_state(&self, value: &str) -> TreeCheckedState {
        self.checks.get(value).copied().unwrap_or_default()
    }

    /// Expands an item's ancestors, scrolls it into view in `scroller` and
    /// focuses it, through `on_highlight` when it isn't rendered. A
    /// controlled tree does it once its `expanded` comes back.
    pub fn reveal(&mut self, value: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let value = value.into();
        let before = self.expanded.get().clone();
        let hidden: Vec<SharedString> = self
            .ancestors(&value)
            .into_iter()
            .filter(|ancestor| !before.contains(ancestor))
            .collect();
        if !hidden.is_empty()
            && !self.change_expanded(with(&before, hidden), ChangeOptions::reason(Reason::Imperative), window, cx)
        {
            return;
        }
        if self.shows(&value) {
            return self.land(&value, window, cx);
        }
        self.revealing = Some(Revealing { value, before });
    }

    /// The item that holds focus, if one does, to style its row: the
    /// original styles a focused row with `:focus-visible` on the item
    /// around its label, which gpui's styles can't reach.
    pub fn focused_value(&self, window: &Window) -> Option<SharedString> {
        self.roving.focused(window)
    }

    /// Focuses an item, or with `None` the item with the tab stop. An item
    /// that isn't rendered yet takes the stop now and focus once it renders.
    pub fn focus(&self, value: Option<&str>, window: &mut Window, cx: &mut App) {
        let key = value.map(|value| SharedString::from(value.to_string()));
        self.roving.focus(key.as_ref(), window, cx);
    }
}
