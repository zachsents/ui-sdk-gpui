//! Port of `page/observe/registry.ts`: the registry the agent bridge and
//! `observe` share: every component on the page with a ref that stays stable
//! while it's there, its accessible name and row text read from its
//! elements, and every change that goes through, settled or step by step,
//! once either entry installs it on the kernel's `watchers`.
//!
//! The original reads names and rows from the DOM. Here they come from what
//! the kernel records: each part's accessible node (its `aria_label`, or the
//! text of the parts that label it), and the elements around it, by their
//! paths of element ids. Watchers hear changes without the app, so the
//! registry lives in the thread, and what it reads of the page needs no app
//! either.
//!
//! A component's entity often outlives its parts here, like a closed tab's
//! field the app keeps: one whose parts all left the page is off the page,
//! and its state reads as it is now. A component whose entity dropped
//! leaves what it last showed, like the original's.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::LazyLock;

use gpui::{App, ElementId, SharedString, Subscription, WindowId};
use indexmap::IndexMap;
use regex::Regex;
use ui_sdk_kernel::accesskit::Role as AccessRole;
use ui_sdk_kernel::watchers::{self, WatchedChange, Watcher};
use ui_sdk_kernel::{
    AgentState, AgentValue, BoundPart, ChangeSource, Instance, InstanceHandle, Reason, json, page_ancestors,
    page_element, page_element_by_id, page_elements_in,
};
use ui_sdk_text::collapse;

use crate::agent::AgentComponent;
use crate::marks::{Placed, contains, is_off_limits, is_secret, text_of};

/// A change a person or an agent made to a component, as `observe` and
/// `AgentBridge::subscribe` report it.
#[derive(Clone, Debug, PartialEq)]
pub struct ObservedChange {
    /// The component's ref, the same as in the agent's `snapshot`, like `"c4"`.
    pub r#ref: SharedString,
    /// The component, like `"select"`.
    pub component: SharedString,
    /// The instance's id.
    pub id: SharedString,
    /// Its accessible name, from its label or its element.
    pub label: Option<SharedString>,
    /// Other text on its row, like the name beside a member's menu.
    pub row: Option<SharedString>,
    /// What changed, like `"value"` or `"open"`.
    pub key: SharedString,
    /// The new value. `"[redacted]"` for a password or one-time-code field,
    /// or one inside a secret mark.
    pub value: AgentValue,
    /// The value before the change, redacted the same way, so a log can undo
    /// it. For a change a commit settles, like a slider's drag, the value
    /// before the interaction began.
    pub previous: AgentValue,
    /// Why it changed, like `ItemPress` or `EscapeKey`. A settled change has
    /// its commit's.
    pub reason: Reason,
    /// Who changed it: a person's input, `Programmatic` or `Agent`.
    pub source: ChangeSource,
}

impl ObservedChange {
    /// The change as data, with the original's keys.
    pub fn to_value(&self) -> AgentValue {
        let mut change = json!({
            "ref": self.r#ref.to_string(),
            "component": self.component.to_string(),
            "id": self.id.to_string(),
            "key": self.key.to_string(),
            "value": self.value,
            "previous": self.previous,
            "reason": self.reason.as_str(),
            "source": self.source.as_str(),
        });
        if let Some(object) = change.as_object_mut() {
            if let Some(label) = &self.label {
                object.insert("label".into(), json!(label.to_string()));
            }
            if let Some(row) = &self.row {
                object.insert("row".into(), json!(row.to_string()));
            }
        }
        change
    }
}

pub(crate) struct Entry {
    pub r#ref: SharedString,
    // The instance's id, which the kernel's changes carry.
    pub id: SharedString,
    // What named it on screen when last seen, `component|label|row`, so a
    // component that comes back keeps its ref.
    pub key: Option<String>,
    pub label: Option<SharedString>,
    pub row: Option<SharedString>,
    // A password or one-time code, or a field inside a secret mark, whose
    // value is redacted and isn't kept once it leaves the page.
    pub secret: bool,
    // Inside a hidden mark: the agent never sees it, and it isn't kept once
    // it leaves the page. The app's own `observe` still hears it.
    pub off_limits: bool,
    pub component: SharedString,
    // The keys a user controls, from the component's definition, kept for
    // when it's gone.
    pub controlled: Option<&'static [&'static str]>,
    // The keys an interaction changes in steps and then commits, like a
    // slider's `value`: each one's value before the steps not yet committed.
    pub started: HashMap<SharedString, AgentValue>,
    // Weak, so the registry never keeps a component alive.
    pub instance: InstanceHandle,
    // The window its parts were last in.
    pub window: Option<WindowId>,
    // Its state as last read or changed, which stands in for it once its
    // entity is gone.
    pub last: Option<AgentState>,
}

pub(crate) type EntryRef = Rc<RefCell<Entry>>;

/// Where a component that left the page is listed from.
#[derive(Clone)]
pub(crate) enum OffPage {
    /// Its entity lives on, like a closed tab's field: its state reads as it
    /// is now.
    Away(EntryRef),
    /// Its entity is gone: what it showed when it left.
    Left(AgentComponent),
}

struct Retired {
    at: u64,
    refs: VecDeque<SharedString>,
}

type ListenerFn = Rc<dyn Fn(&ObservedChange, &[BoundPart])>;

struct Listener {
    id: u64,
    // Whether it hears each step (`live`) rather than settled changes.
    live: bool,
    run: ListenerFn,
}

#[derive(Default)]
struct Registry {
    entries: IndexMap<SharedString, EntryRef>,
    // The same entries by instance id, for the changes the kernel reports.
    by_id: HashMap<SharedString, EntryRef>,
    // Refs of components that left, by key, for the same components when
    // they come back, like a tab's fields. Components that leave together,
    // with none made between them, like a tab's twins with the same label,
    // queue in the order they left, so they take back their own refs; a
    // later departure replaces an earlier one. `at` is the instance count
    // when they left. The most recent `RETIRED_LIMIT` keys, for a page that
    // swaps many components in and out.
    retired: IndexMap<String, Retired>,
    // What left the page, so an agent can check a closed tab without
    // opening it. The most recent `OFF_PAGE_LIMIT`.
    off_page: IndexMap<SharedString, OffPage>,
    count: u64,
    listeners: Vec<Listener>,
    next_listener: u64,
    installed: bool,
}

const RETIRED_LIMIT: usize = 500;
const OFF_PAGE_LIMIT: usize = 50;

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

fn with<R>(run: impl FnOnce(&mut Registry) -> R) -> R {
    REGISTRY.with(|registry| run(&mut registry.borrow_mut()))
}

fn watch(instance: &InstanceHandle) {
    let (Some(id), Some(name)) = (instance.id(), instance.name()) else {
        return;
    };
    with(|registry| {
        registry.count += 1;
        let r#ref: SharedString = format!("c{}", registry.count).into();
        let entry = Rc::new(RefCell::new(Entry {
            r#ref: r#ref.clone(),
            id: id.clone(),
            key: None,
            label: None,
            row: None,
            secret: false,
            off_limits: false,
            component: name.into(),
            controlled: None,
            started: HashMap::new(),
            instance: instance.clone(),
            window: None,
            last: None,
        }));
        registry.entries.insert(r#ref, entry.clone());
        registry.by_id.insert(id, entry);
    });
}

/// Every entry, in the order they joined.
pub(crate) fn entries() -> Vec<EntryRef> {
    with(|registry| registry.entries.values().cloned().collect())
}

/// The entry a ref names.
pub(crate) fn entry(r#ref: &str) -> Option<EntryRef> {
    with(|registry| registry.entries.get(r#ref).cloned())
}

fn entry_by_id(id: &str) -> Option<EntryRef> {
    with(|registry| registry.by_id.get(id).cloned())
}

/// What left the page, oldest first.
pub(crate) fn off_page() -> Vec<OffPage> {
    with(|registry| registry.off_page.values().cloned().collect())
}

/// What a ref that left the page showed, or shows now that it's away.
pub(crate) fn off_page_component(r#ref: &str, cx: &App) -> Option<AgentComponent> {
    let listed = with(|registry| registry.off_page.get(r#ref).cloned())?;
    match listed {
        OffPage::Left(component) => Some(component),
        OffPage::Away(entry) => away_component(&entry, cx),
    }
}

/// An off-page entry as the snapshot lists it.
pub(crate) fn away_component(entry: &EntryRef, cx: &App) -> Option<AgentComponent> {
    let agent = entry.borrow().instance.agent()?;
    let state = agent.state(cx)?;
    entry.borrow_mut().last = Some(state.clone());
    left_component(&entry.borrow(), &state)
}

// What a component that left shows: its values and checks, which are worth
// keeping. A closed menu's `open: false` isn't.
fn left_component(entry: &Entry, state: &AgentState) -> Option<AgentComponent> {
    let keys: Vec<&str> = state_keys(entry.controlled, state)
        .into_iter()
        .filter(|key| *key != "open")
        .collect();
    if keys.is_empty() {
        return None;
    }
    let mut last = AgentComponent::off_page(&entry.r#ref, &entry.component);
    if let Some(label) = &entry.label {
        last.set("label", json!(label.to_string()));
    }
    if let Some(row) = &entry.row {
        last.set("row", json!(row.to_string()));
    }
    for key in keys {
        let value = state.get(key).cloned().unwrap_or(AgentValue::Null);
        if key == "value" || value != json!("") {
            last.set(key, value);
        }
    }
    Some(last)
}

/// Lists an entry whose parts all left the page, newest last.
pub(crate) fn mark_away(entry: &EntryRef) {
    let (r#ref, listed) = {
        let entry = entry.borrow();
        (
            entry.r#ref.clone(),
            entry.key.is_some() && !entry.secret && !entry.off_limits,
        )
    };
    with(|registry| {
        if !listed {
            if matches!(registry.off_page.get(&r#ref), Some(OffPage::Away(_))) {
                registry.off_page.shift_remove(&r#ref);
            }
            return;
        }
        if matches!(registry.off_page.get(&r#ref), Some(OffPage::Away(away)) if Rc::ptr_eq(away, entry)) {
            return;
        }
        registry.off_page.shift_remove(&r#ref);
        registry.off_page.insert(r#ref, OffPage::Away(entry.clone()));
        if registry.off_page.len() > OFF_PAGE_LIMIT {
            registry.off_page.shift_remove_index(0);
        }
    });
}

/// Unlists an entry that's back on the page.
pub(crate) fn mark_back(entry: &EntryRef) {
    let r#ref = entry.borrow().r#ref.clone();
    with(|registry| {
        if matches!(registry.off_page.get(&r#ref), Some(OffPage::Away(away)) if Rc::ptr_eq(away, entry)) {
            registry.off_page.shift_remove(&r#ref);
        }
    });
}

/// Takes an entry whose instance is gone out of the registry, and keeps its
/// ref for a component with the same key that comes back, and what it last
/// showed.
pub(crate) fn retire(entry: &EntryRef) {
    let entry_data = entry.borrow();
    with(|registry| {
        if !registry
            .entries
            .get(&entry_data.r#ref)
            .is_some_and(|listed| Rc::ptr_eq(listed, entry))
        {
            return;
        }
        registry.entries.shift_remove(&entry_data.r#ref);
        if registry
            .by_id
            .get(&entry_data.id)
            .is_some_and(|listed| Rc::ptr_eq(listed, entry))
        {
            registry.by_id.remove(&entry_data.id);
        }
        let Some(key) = entry_data.key.clone() else {
            registry.off_page.shift_remove(&entry_data.r#ref);
            return;
        };
        if entry_data.secret || entry_data.off_limits {
            registry.off_page.shift_remove(&entry_data.r#ref);
            return;
        }
        let count = registry.count;
        match registry.retired.get_mut(&key) {
            Some(queued) if queued.at == count => queued.refs.push_back(entry_data.r#ref.clone()),
            _ => {
                // Removed first, so the key moves to the newest end.
                registry.retired.shift_remove(&key);
                registry.retired.insert(
                    key,
                    Retired {
                        at: count,
                        refs: VecDeque::from([entry_data.r#ref.clone()]),
                    },
                );
                if registry.retired.len() > RETIRED_LIMIT {
                    registry.retired.shift_remove_index(0);
                }
            }
        }
        let last = entry_data
            .last
            .as_ref()
            .and_then(|state| left_component(&entry_data, state));
        registry.off_page.shift_remove(&entry_data.r#ref);
        if let Some(last) = last {
            registry.off_page.insert(entry_data.r#ref.clone(), OffPage::Left(last));
            if registry.off_page.len() > OFF_PAGE_LIMIT {
                registry.off_page.shift_remove_index(0);
            }
        }
    });
}

// Fields a browser tool stops seeing when they close, so the bridge doesn't
// keep them either: a password, a card or a one-time code, by the native
// `type` and `autocomplete` a part carries (`Part::input_attrs`), or a
// password field's role.
static SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(password|current-password|new-password|one-time-code|cc-[a-z-]+)\b")
        .expect("the secret field pattern compiles")
});

fn is_secret_field(part: &BoundPart) -> bool {
    role_of(part) == Some(AccessRole::PasswordInput)
        || SECRET.is_match(&format!(
            "{} {}",
            part.input_type.as_deref().unwrap_or("null"),
            part.autocomplete.as_deref().unwrap_or("null")
        ))
}

/// The state keys the bridge reports: the controlled ones, or whichever of
/// `open`, `value` and `checked` the state has.
pub(crate) fn state_keys(controlled: Option<&'static [&'static str]>, state: &AgentState) -> Vec<&'static str> {
    match controlled {
        Some(keys) => keys.to_vec(),
        None => ["open", "value", "checked"]
            .into_iter()
            .filter(|key| state.contains(key))
            .collect(),
    }
}

/// The api method that sets a state key, like `setOpen` for `open`.
pub(crate) fn setter_of(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => format!("set{}{}", first.to_uppercase(), chars.as_str()),
        None => "set".into(),
    }
}

/// Keeps the component's key current, and the first time it's seen gives it
/// the ref it had before it left the page, when one with its key did.
pub(crate) fn remember(entry: &EntryRef, parts: &[BoundPart]) {
    let agent = entry.borrow().instance.agent();
    let first = entry.borrow().key.is_none();
    let label = component_label(parts);
    let row = row_of(parts);
    let secret = is_secret(parts)
        || parts.iter().any(is_secret_field)
        || agent.as_ref().is_some_and(|agent| agent.surface().secret);
    // A region can turn off-limits and back, like a panel an app locks while
    // it's busy.
    let off_limits = is_off_limits(parts);
    let key = {
        let mut entry = entry.borrow_mut();
        entry.secret |= secret;
        entry.off_limits = off_limits;
        entry.window = parts.first().map(|part| part.window).or(entry.window);
        if let Some(agent) = &agent {
            entry.controlled = agent.controlled();
        }
        let key = format!(
            "{}|{}|{}",
            entry.component,
            label.as_deref().unwrap_or("undefined"),
            row.as_deref().unwrap_or("undefined")
        );
        entry.label = label;
        entry.row = row;
        entry.key = Some(key.clone());
        key
    };
    if !first {
        return;
    }
    with(|registry| {
        let Some(queue) = registry.retired.get_mut(&key) else {
            return;
        };
        let Some(old) = queue.refs.pop_front() else {
            return;
        };
        if queue.refs.is_empty() {
            registry.retired.shift_remove(&key);
        }
        registry.off_page.shift_remove(&old);
        let current = entry.borrow().r#ref.clone();
        registry.entries.shift_remove(&current);
        entry.borrow_mut().r#ref = old.clone();
        registry.entries.insert(old, entry.clone());
    });
}

/// The parts of an instance in the page, in the order they bound.
pub(crate) fn bound(instance: &InstanceHandle) -> Vec<BoundPart> {
    instance.bound_parts()
}

/// A part's accesskit role.
pub(crate) fn role_of(part: &BoundPart) -> Option<AccessRole> {
    part.accessible.as_ref().map(|node| node.role())
}

/// A part's accessible name: its own, from `aria_label`, else the text of
/// the elements that name it (`accessibleName`).
pub(crate) fn name_of(part: &BoundPart) -> String {
    let own = text_of(part.accessible.as_ref());
    if !own.is_empty() {
        return own;
    }
    let Some(element) = page_element(part.window, &part.path) else {
        return String::new();
    };
    let names: Vec<String> = element
        .labelled_by
        .iter()
        .filter_map(|id| page_element_by_id(part.window, id))
        .map(|label| text_of(Some(&label.accessible)))
        .filter(|text| !text.is_empty())
        .collect();
    collapse(&names.join(" "))
}

/// A part's own text, with its spaces collapsed (`textOf`).
pub(crate) fn part_text(part: &BoundPart) -> String {
    text_of(part.accessible.as_ref())
}

// The first `count` characters.
pub(crate) fn cut(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

fn is_text_field(part: &BoundPart) -> bool {
    matches!(
        role_of(part),
        Some(
            AccessRole::TextInput
                | AccessRole::MultilineTextInput
                | AccessRole::SearchInput
                | AccessRole::EmailInput
                | AccessRole::NumberInput
                | AccessRole::PasswordInput
                | AccessRole::PhoneNumberInput
                | AccessRole::UrlInput
        )
    )
}

/// The component's name: its label part's text, its root's or group's name,
/// its field's, or what names it on screen.
pub(crate) fn component_label(parts: &[BoundPart]) -> Option<SharedString> {
    let keyless = |slot: &str| parts.iter().find(|part| part.slot == slot && part.key.is_none());
    // Not when the slot has many keys: an accordion's first trigger names
    // its item, not the accordion. A dialog's one trigger does name it.
    let first = |slot: &str| -> Option<&BoundPart> {
        let found: Vec<&BoundPart> = parts.iter().filter(|part| part.slot == slot).collect();
        let keys: HashSet<Option<&SharedString>> = found.iter().map(|part| part.key.as_ref()).collect();
        if keys.len() == 1 {
            found.first().copied()
        } else {
            found.into_iter().find(|part| part.key.is_none())
        }
    };
    let text = |el: Option<&BoundPart>| -> Option<String> {
        el.map(|el| cut(&part_text(el), 80)).filter(|text| !text.is_empty())
    };
    if let Some(label) = text(first("label")) {
        return Some(label.into());
    }
    // A group's own element names it before its items do.
    let group = keyless("group").or_else(|| keyless("root"));
    if let Some(name) = group.map(name_of).filter(|name| !name.is_empty()) {
        return Some(name.into());
    }
    // Then its field's name, like a date picker's input labelled "Due date",
    // before a heading that changes with what it shows, like "November
    // 2026", and a step button before it. Not a search field in its popup,
    // which comes and goes.
    let input = parts
        .iter()
        .find(|part| part.slot == "input" || (part.slot == "control" && is_text_field(part)));
    let popup = parts.iter().find(|part| part.slot == "content");
    if let Some(input) = input
        && !popup.is_some_and(|popup| contains(popup, input))
    {
        let name = name_of(input);
        if !name.is_empty() {
            return Some(name.into());
        }
    }
    // Then what names it on screen: a title, a heading, a trigger or the
    // button it is.
    let root = keyless("root");
    let button = root.filter(|root| role_of(root) == Some(AccessRole::Button));
    let named = |el: Option<&BoundPart>| -> Option<String> {
        let el = el?;
        let name = name_of(el);
        if name.is_empty() { text(Some(el)) } else { Some(name) }
    };
    let shown = named(first("title"))
        .or_else(|| named(first("heading")))
        .or_else(|| named(first("trigger")))
        .or_else(|| named(button));
    if let Some(shown) = shown {
        return Some(shown.into());
    }
    // Then any part's own name, keyless first, so an accordion isn't named
    // by its first item, but a slider is by its one thumb. Next, a shown
    // part alone in its slot with a key of its own.
    let keys: HashSet<&SharedString> = parts.iter().filter_map(|part| part.key.as_ref()).collect();
    let keyed: Vec<&BoundPart> = if keys.len() == 1 {
        parts.iter().filter(|part| part.key.is_some()).collect()
    } else {
        Vec::new()
    };
    let sole = parts.iter().filter(|part| {
        part.key.is_some()
            && !part.slot.starts_with("group")
            && part.shown
            && parts.iter().all(|other| {
                std::ptr::eq(other, *part) || (other.slot != part.slot && other.key != part.key)
            })
    });
    for part in parts
        .iter()
        .filter(|part| part.key.is_none())
        .chain(sole)
        .chain(keyed)
    {
        let name = name_of(part);
        if !name.is_empty() {
            return Some(name.into());
        }
    }
    None
}

/// Text in the component's row that isn't the component's own, its parts'
/// or its label's, like the name beside a member's menu, so an agent knows
/// which row it is. A row is the nearest element around it with a row or
/// list item role, a part or an element given ARIA with `with_aria`; its
/// text is its own name and the names of the elements in it.
pub(crate) fn row_of(parts: &[BoundPart]) -> Option<SharedString> {
    let el = parts
        .iter()
        .find(|part| part.slot == "trigger" || part.slot == "root")
        .or_else(|| parts.first())?;
    let row = page_ancestors(el.window, &el.path)
        .into_iter()
        .find(|element| matches!(element.role, Some(AccessRole::Row | AccessRole::ListItem)))?;
    if parts.iter().any(|part| contains(part, row.as_ref())) {
        return None;
    }
    // The parts' own elements, and the labels that name them.
    let mut own: Vec<(WindowId, Vec<ElementId>)> = parts
        .iter()
        .map(|part| (part.window, part.path.to_vec()))
        .collect();
    let mut own_texts: HashSet<String> = HashSet::new();
    for part in parts {
        let text = name_of(part);
        if !text.is_empty() {
            own_texts.insert(text);
        }
        if let Some(element) = page_element(part.window, &part.path) {
            for id in &element.labelled_by {
                if let Some(label) = page_element_by_id(part.window, id) {
                    own.push((label.window, label.path.to_vec()));
                }
            }
        }
    }
    let inside_own = |element: &dyn Placed| {
        own.iter()
            .any(|(window, path)| *window == element.window() && element.path().starts_with(path))
    };
    let mut words: Vec<String> = Vec::new();
    for element in page_elements_in(row.window, &row.path) {
        if inside_own(element.as_ref()) {
            continue;
        }
        if is_off_limits([element.as_ref()]) {
            continue;
        }
        let text = text_of(Some(&element.accessible));
        let text = text.trim();
        if !text.is_empty() && !own_texts.contains(text) && !words.iter().any(|word| word == text) {
            words.push(text.to_string());
        }
    }
    let row = cut(&words.join(" "), 80);
    (!row.is_empty()).then(|| row.into())
}

// A change the kernel reports. Live listeners hear every step. Settled ones
// hear a person's steps on a key the component commits at its commit
// instead. A change no person made, like a value set from code or by an
// agent, has no interaction to commit it, so they hear it now.
fn report(
    id: &str,
    key: &'static str,
    value: AgentValue,
    reason: Reason,
    source: ChangeSource,
    previous: AgentValue,
    controlled: bool,
) {
    let Some(entry) = entry_by_id(id) else {
        return;
    };
    // What it shows now, for when it's gone. A controlled value waits for its
    // owner, who may never pass it.
    if !controlled && let Some(last) = &mut entry.borrow_mut().last {
        last.set(key, value.clone());
    }
    if with(|registry| registry.listeners.is_empty()) {
        return;
    }
    let commits = entry
        .borrow()
        .instance
        .agent()
        .map_or(&[][..], |agent| agent.commits());
    let held = commits.contains(&key) && source != ChangeSource::Programmatic && source != ChangeSource::Agent;
    {
        // A person's first step keeps the value before it. A change no person
        // made between steps is reported now, so the commit starts after it.
        let mut entry = entry.borrow_mut();
        let key: SharedString = key.into();
        if entry.started.contains_key(&key) != held {
            let start = if held { previous.clone() } else { value.clone() };
            entry.started.insert(key, start);
        }
    }
    send(&entry, key, value, reason, source, previous, held.then_some(true));
}

// A commit the kernel reports: one settled change for the steps before it,
// from the value before the first. A commit with no steps held, like one a
// call from code makes, was reported as it changed.
fn settle(id: &str, key: &'static str, value: AgentValue, reason: Reason, source: ChangeSource) {
    let Some(entry) = entry_by_id(id) else {
        return;
    };
    if with(|registry| registry.listeners.is_empty()) {
        return;
    }
    let Some(previous) = entry.borrow_mut().started.remove(key) else {
        return;
    };
    send(&entry, key, value, reason, source, previous, Some(false));
}

// A change as a record for the listeners it's for, named the way the
// agent's snapshot names the component: live ones for `live` true, settled
// ones for false, and every one when it's `None`.
fn send(
    entry: &EntryRef,
    key: &'static str,
    value: AgentValue,
    reason: Reason,
    source: ChangeSource,
    previous: AgentValue,
    live: Option<bool>,
) {
    let instance = entry.borrow().instance.clone();
    if !instance.is_alive() {
        return;
    }
    let parts = bound(&instance);
    if !parts.is_empty() {
        remember(entry, &parts);
    }
    let change = {
        let entry = entry.borrow();
        let redacted = || json!("[redacted]");
        ObservedChange {
            r#ref: entry.r#ref.clone(),
            component: entry.component.clone(),
            id: entry.id.clone(),
            label: entry.label.clone(),
            row: entry.row.clone(),
            key: key.into(),
            value: if entry.secret { redacted() } else { value },
            previous: if entry.secret { redacted() } else { previous },
            reason,
            source,
        }
    };
    let listeners: Vec<(bool, ListenerFn)> = with(|registry| {
        registry
            .listeners
            .iter()
            .map(|listener| (listener.live, listener.run.clone()))
            .collect()
    });
    for (hears_steps, listener) in listeners {
        if live.unwrap_or(hears_steps) == hears_steps {
            listener(&change, &parts);
        }
    }
}

// What the registry hears from the kernel.
struct RegistryWatcher;

impl Watcher for RegistryWatcher {
    fn add(&self, instance: &InstanceHandle) {
        watch(instance);
    }

    fn destroy(&self, instance: &Instance) {
        if let Some(entry) = entry_by_id(instance.id()) {
            retire(&entry);
        }
    }

    fn change(&self, change: &WatchedChange) {
        if change.canceled {
            return;
        }
        let id = change.instance.id();
        let value = change
            .known_value()
            .unwrap_or_else(|| decoded(&format!("{:?}", change.value)));
        let reason = change.details.reason;
        let source = change.details.source;
        if change.commit {
            settle(id, change.key, value, reason, source);
            return;
        }
        let previous = change
            .known_previous()
            .or_else(|| change.previous().map(|previous| decoded(&format!("{previous:?}"))))
            .unwrap_or(AgentValue::Null);
        report(id, change.key, value, reason, source, previous, change.controlled);
    }
}

/// Starts hearing components as they're made, and those made before. The
/// agent and `observe` both call it. Parts laid out before it record no names
/// or marks, so every window draws again.
pub(crate) fn install(cx: &mut App) {
    let fresh = with(|registry| !std::mem::replace(&mut registry.installed, true));
    if !fresh {
        return;
    }
    let watcher = Rc::new(RegistryWatcher);
    // Never removed, like the original's hooks.
    let _remove = watchers::install(watcher.clone());
    watchers::watch_earlier(watcher.as_ref());
    cx.refresh_windows();
}

/// Calls `listener` with every settled change, or with every step when
/// `live` is true, until the returned subscription drops.
pub(crate) fn listen(
    listener: impl Fn(&ObservedChange, &[BoundPart]) + 'static,
    live: bool,
    cx: &mut App,
) -> Subscription {
    install(cx);
    let id = with(|registry| {
        let id = registry.next_listener;
        registry.next_listener += 1;
        registry.listeners.push(Listener {
            id,
            live,
            run: Rc::new(listener),
        });
        id
    });
    Subscription::new(move || {
        REGISTRY.with(|registry| {
            if let Ok(mut registry) = registry.try_borrow_mut() {
                registry.listeners.retain(|listener| listener.id != id);
            }
        });
    })
}

/// The bound parts of the component a ref names, in the order they bound.
pub(crate) fn elements_of(r#ref: &str) -> Vec<BoundPart> {
    entry(r#ref).map_or_else(Vec::new, |entry| bound(&entry.borrow().instance))
}

/// A value the kernel couldn't read as data, read back from its debug text:
/// `Some(x)` and a variant around one value are the value, `None` is null,
/// lists, tuples and structs are arrays and objects, maps are objects, and a
/// unit variant is its name. Text that doesn't read stays text.
pub(crate) fn decoded(text: &str) -> AgentValue {
    let mut reader = DebugText { text, at: 0 };
    match reader.value() {
        Some(value) if reader.done() => value,
        _ => AgentValue::String(text.to_string()),
    }
}

struct DebugText<'a> {
    text: &'a str,
    at: usize,
}

impl DebugText<'_> {
    fn rest(&self) -> &str {
        &self.text[self.at..]
    }

    fn skip_spaces(&mut self) {
        let rest = self.rest();
        self.at += rest.len() - rest.trim_start().len();
    }

    fn done(&mut self) -> bool {
        self.skip_spaces();
        self.at == self.text.len()
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn eat(&mut self, expected: char) -> bool {
        self.skip_spaces();
        if self.peek() == Some(expected) {
            self.at += expected.len_utf8();
            true
        } else {
            false
        }
    }

    // Values separated by commas up to `end`, which a trailing comma may
    // come before.
    fn values(&mut self, end: char) -> Option<Vec<AgentValue>> {
        let mut values = Vec::new();
        loop {
            if self.eat(end) {
                return Some(values);
            }
            values.push(self.value()?);
            if !self.eat(',') {
                return self.eat(end).then_some(values);
            }
        }
    }

    fn value(&mut self) -> Option<AgentValue> {
        self.skip_spaces();
        let next = self.peek()?;
        match next {
            '"' => self.string('"').map(AgentValue::String),
            '\'' => self.string('\'').map(AgentValue::String),
            '[' => {
                self.at += 1;
                self.values(']').map(AgentValue::Array)
            }
            '(' => {
                self.at += 1;
                let values = self.values(')')?;
                Some(if values.is_empty() { AgentValue::Null } else { AgentValue::Array(values) })
            }
            '{' => {
                self.at += 1;
                self.map()
            }
            '-' | '0'..='9' => self.number(),
            _ if next.is_alphabetic() || next == '_' => self.named(),
            _ => None,
        }
    }

    fn string(&mut self, quote: char) -> Option<String> {
        self.at += 1;
        let mut out = String::new();
        let mut chars = self.rest().char_indices();
        while let Some((index, char)) = chars.next() {
            match char {
                _ if char == quote => {
                    self.at += index + 1;
                    return Some(out);
                }
                '\\' => {
                    let (_, escaped) = chars.next()?;
                    match escaped {
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        '0' => out.push('\0'),
                        'u' => {
                            let mut code = String::new();
                            for (_, char) in chars.by_ref() {
                                match char {
                                    '{' => {}
                                    '}' => break,
                                    _ => code.push(char),
                                }
                            }
                            out.push(char::from_u32(u32::from_str_radix(&code, 16).ok()?)?);
                        }
                        other => out.push(other),
                    }
                }
                _ => out.push(char),
            }
        }
        None
    }

    fn number(&mut self) -> Option<AgentValue> {
        let rest = self.rest();
        let length = rest
            .char_indices()
            .find(|(index, char)| {
                !(char.is_ascii_digit()
                    || *char == '.'
                    || *char == 'e'
                    || *char == 'E'
                    || *char == '_'
                    || ((*char == '-' || *char == '+') && (*index == 0 || rest[..*index].ends_with(['e', 'E']))))
            })
            .map_or(rest.len(), |(index, _)| index);
        let digits = rest[..length].replace('_', "");
        self.at += length;
        if let Ok(integer) = digits.parse::<i64>() {
            return Some(AgentValue::from(integer));
        }
        let number: f64 = digits.parse().ok()?;
        Some(serde_json::Number::from_f64(number).map_or(AgentValue::Null, AgentValue::Number))
    }

    fn identifier(&mut self) -> &str {
        let start = self.at;
        let rest = self.rest();
        let length = rest
            .char_indices()
            .find(|(index, char)| {
                !(char.is_alphanumeric() || *char == '_' || (*char == ':' && rest[*index..].starts_with("::")))
                    && !(*char == ':' && rest[..*index].ends_with(':'))
            })
            .map_or(rest.len(), |(index, _)| index);
        self.at += length;
        &self.text[start..start + length]
    }

    fn named(&mut self) -> Option<AgentValue> {
        let name = self.identifier().to_string();
        match name.as_str() {
            "true" => return Some(AgentValue::Bool(true)),
            "false" => return Some(AgentValue::Bool(false)),
            "None" | "inf" | "NaN" => return Some(AgentValue::Null),
            _ => {}
        }
        self.skip_spaces();
        match self.peek() {
            Some('(') => {
                self.at += 1;
                let mut values = self.values(')')?;
                Some(if values.len() == 1 {
                    values.remove(0)
                } else {
                    AgentValue::Array(values)
                })
            }
            Some('{') => {
                self.at += 1;
                self.fields()
            }
            _ => Some(AgentValue::String(
                name.rsplit("::").next().unwrap_or(&name).to_string(),
            )),
        }
    }

    // A struct's fields, `name: value`, up to `}`, with `..` for the ones
    // left out.
    fn fields(&mut self) -> Option<AgentValue> {
        let mut object = serde_json::Map::new();
        loop {
            if self.eat('}') {
                return Some(AgentValue::Object(object));
            }
            self.skip_spaces();
            if self.rest().starts_with("..") {
                self.at += 2;
                continue;
            }
            let name = self.identifier().to_string();
            if name.is_empty() || !self.eat(':') {
                return None;
            }
            let value = self.value()?;
            object.insert(name, value);
            if !self.eat(',') {
                return self.eat('}').then_some(AgentValue::Object(object));
            }
        }
    }

    // A map's entries, `key: value`, up to `}`.
    fn map(&mut self) -> Option<AgentValue> {
        let mut object = serde_json::Map::new();
        loop {
            if self.eat('}') {
                return Some(AgentValue::Object(object));
            }
            let key = match self.value()? {
                AgentValue::String(key) => key,
                other => other.to_string(),
            };
            if !self.eat(':') {
                return None;
            }
            let value = self.value()?;
            object.insert(key, value);
            if !self.eat(',') {
                return self.eat('}').then_some(AgentValue::Object(object));
            }
        }
    }
}
