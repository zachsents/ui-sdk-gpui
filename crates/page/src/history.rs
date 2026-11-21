//! Port of `page/history/history.ts`: hears changes through `observe` and
//! puts values back through the agent bridge, so it wires to no control. A
//! change made after seeking back drops the entries after it, like undo in
//! an editor.
//!
//! Changes reach the history as they go through, inside the component's
//! update and without the app, so its listeners run a moment later, once the
//! update is over. It records until `destroy`, or until every handle to it
//! drops.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use futures::channel::mpsc::{UnboundedSender, unbounded};
use futures::channel::oneshot;
use futures::future::{self, LocalBoxFuture, Shared};
use futures::{FutureExt, StreamExt};
use gpui::{App, AsyncApp, SharedString, Subscription, Task};
use indexmap::IndexMap;
use ui_sdk_kernel::{AgentValue, ChangeSource, Reason};

use crate::agent::{AgentBridge, AgentStep, js_text, same_value};
use crate::guard::{Guard, guarded_act};
use crate::observe::{ObserveOptions, ObservedChange, Within, observe};
use crate::registry::{self, setter_of};

pub use crate::stage::{AppliedChange, StagedChange, StagedChanges, stage_changes};

/// One change in the history.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub id: SharedString,
    pub r#ref: SharedString,
    pub label: SharedString,
    pub key: SharedString,
    pub value: AgentValue,
    /// The value before the change, which stepping back over it restores.
    pub previous: AgentValue,
    /// Who made it: `Agent`, or how a person did, like `Mouse` or `Keyboard`.
    pub source: ChangeSource,
    pub reason: Reason,
    /// Reverted by `undo_source`, and left out of the page's state.
    pub reverted: bool,
    /// A password or one-time-code field's change, or one inside a secret
    /// mark, whose `value` and `previous` read `"[redacted]"`. Seeking and
    /// undoing never put it back, and report it as not put back.
    pub redacted: bool,
}

/// What `create_history` records.
#[derive(Clone)]
pub struct HistoryOptions {
    /// Hears only components inside this element, like one panel. Defaults
    /// to the whole page.
    pub within: Option<Within>,
    /// Keys worth an entry. `value`, `checked` and `pressed` by default.
    pub keys: Option<Vec<SharedString>>,
    /// Folds a drag, or a run of key presses, on one control into one entry.
    /// False records every step, even of a drag that commits once. True by
    /// default.
    pub coalesce: bool,
}

impl Default for HistoryOptions {
    fn default() -> Self {
        Self {
            within: None,
            keys: None,
            coalesce: true,
        }
    }
}

impl HistoryOptions {
    pub fn within(mut self, within: impl Into<Within>) -> Self {
        self.within = Some(within.into());
        self
    }

    pub fn keys(mut self, keys: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.keys = Some(keys.into_iter().map(Into::into).collect());
        self
    }

    pub fn coalesce(mut self, coalesce: bool) -> Self {
        self.coalesce = coalesce;
        self
    }
}

/// A value to put back on a control: its ref, the key, and the value, which
/// a key no entry knows a value for has none of.
#[derive(Clone, Debug, PartialEq)]
pub struct RestoreStep {
    pub r#ref: SharedString,
    pub key: SharedString,
    pub value: Option<AgentValue>,
}

fn id_of(r#ref: &str, key: &str) -> String {
    format!("{ref}:{key}")
}

// A component's value for a key now, like a checkbox's `checked`.
fn current(r#ref: &str, key: &str, cx: &App) -> Option<AgentValue> {
    let entry = registry::entry(r#ref)?;
    let agent = entry.borrow().instance.agent()?;
    agent.state(cx)?.get(key).cloned()
}

fn round(value: &AgentValue) -> AgentValue {
    match value {
        AgentValue::Number(number) => {
            let rounded = (number.as_f64().unwrap_or(0.) * 100.).round() / 100.;
            if rounded.fract() == 0.0 && rounded.abs() < 9e15 {
                AgentValue::from(rounded as i64)
            } else {
                serde_json::Number::from_f64(rounded).map_or(AgentValue::Null, AgentValue::Number)
            }
        }
        AgentValue::Object(object) => AgentValue::Object(
            object
                .iter()
                .map(|(key, inner)| (key.clone(), round(inner)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn show(value: &AgentValue) -> String {
    match value {
        AgentValue::Array(values) => {
            let joined = values
                .iter()
                .map(|value| match value {
                    AgentValue::Null => String::new(),
                    value => js_text(value),
                })
                .collect::<Vec<_>>()
                .join(", ");
            if joined.is_empty() { "none".into() } else { joined }
        }
        AgentValue::Object(_) => round(value).to_string().replace('"', ""),
        other => js_text(&round(other)),
    }
}

/// A change in words, like "Agent set Region to Frankfurt".
pub fn describe_entry(entry: &HistoryEntry) -> String {
    let who = if entry.source == ChangeSource::Agent { "Agent" } else { "You" };
    format!("{who} set {} to {}", entry.label, show(&entry.value))
}

// A key's value after the first `position` entries, skipping reverted ones.
fn value_at(entries: &[HistoryEntry], position: usize, id: &str) -> Option<AgentValue> {
    let mut value = None;
    let mut seen = false;
    for (index, entry) in entries.iter().enumerate() {
        if id_of(&entry.r#ref, &entry.key) != id {
            continue;
        }
        if !seen {
            value = Some(entry.previous.clone());
        }
        seen = true;
        if index >= position {
            break;
        }
        if !entry.reverted {
            value = Some(entry.value.clone());
        }
    }
    value
}

/// The history `create_history` returns. Clones are the same history.
#[derive(Clone)]
pub struct History {
    inner: Rc<Inner>,
}

struct Inner {
    agent: AgentBridge,
    keys: Vec<SharedString>,
    coalesce: bool,
    within: Option<Within>,
    state: RefCell<State>,
    listeners: RefCell<Vec<(u64, Rc<dyn Fn(&mut App)>)>>,
    next_listener: Cell<u64>,
    notify: UnboundedSender<()>,
    stop: RefCell<Option<Subscription>>,
    // One seek or undo at a time, each planned from where the last left the
    // page.
    turn: RefCell<Shared<LocalBoxFuture<'static, ()>>>,
    deliver: RefCell<Option<Task<()>>>,
}

struct State {
    entries: Rc<[HistoryEntry]>,
    position: usize,
    count: u64,
    // The keys the running seek or undo is putting back, less any someone
    // else changed meanwhile.
    restoring: HashSet<String>,
}

/// Starts recording changes, and returns the history. `entries` and
/// `position` read it, `subscribe` hears it change, `seek(n)` puts the page
/// back to how it was after `n` entries, and `destroy` stops recording.
/// Restores go through the agent's `act`, so its `allow`, disabled state and
/// change callbacks apply.
///
/// ```ignore
/// let history = create_history(&agent, HistoryOptions::default().within("panel"), cx);
/// history.subscribe(move |cx| view.update(cx, |_, cx| cx.notify())).detach();
/// history.seek(0, cx).detach(); // the page as it was when recording started
/// history.undo_source(ChangeSource::Agent, cx).detach(); // only what the agent did
/// ```
pub fn create_history(agent: &AgentBridge, options: HistoryOptions, cx: &mut App) -> History {
    let (notify, mut notified) = unbounded::<()>();
    let inner = Rc::new(Inner {
        agent: agent.clone(),
        keys: options
            .keys
            .clone()
            .unwrap_or_else(|| vec!["value".into(), "checked".into(), "pressed".into()]),
        coalesce: options.coalesce,
        within: options.within.clone(),
        state: RefCell::new(State {
            entries: Rc::from(Vec::new()),
            position: 0,
            count: 0,
            restoring: HashSet::new(),
        }),
        listeners: RefCell::new(Vec::new()),
        next_listener: Cell::new(0),
        notify,
        stop: RefCell::new(None),
        turn: RefCell::new(future::ready(()).boxed_local().shared()),
        deliver: RefCell::new(None),
    });
    let weak = Rc::downgrade(&inner);
    let deliver = cx.spawn(async move |cx| {
        while notified.next().await.is_some() {
            while notified.try_recv().is_ok() {}
            let Some(inner) = weak.upgrade() else {
                break;
            };
            let listeners: Vec<Rc<dyn Fn(&mut App)>> = inner
                .listeners
                .borrow()
                .iter()
                .map(|(_, listener)| listener.clone())
                .collect();
            drop(inner);
            cx.update(|cx| {
                for listener in listeners {
                    listener(cx);
                }
            });
        }
    });
    *inner.deliver.borrow_mut() = Some(deliver);
    let weak = Rc::downgrade(&inner);
    // A drag that commits arrives settled, as one change, unless every step
    // is wanted.
    let stop = observe(
        move |change| {
            if let Some(inner) = weak.upgrade() {
                History { inner }.heard(change);
            }
        },
        ObserveOptions {
            within: options.within,
            live: !options.coalesce,
        },
        cx,
    );
    *inner.stop.borrow_mut() = Some(stop);
    History { inner }
}

impl History {
    /// Every entry, oldest first.
    pub fn entries(&self) -> Rc<[HistoryEntry]> {
        self.inner.state.borrow().entries.clone()
    }

    /// How many entries are applied: `entries().len()` is now, 0 is the
    /// start.
    pub fn position(&self) -> usize {
        self.inner.state.borrow().position
    }

    /// Calls `listener` whenever the entries or the position change, until
    /// the returned subscription drops.
    pub fn subscribe(&self, listener: impl Fn(&mut App) + 'static) -> Subscription {
        let id = self.inner.next_listener.get();
        self.inner.next_listener.set(id + 1);
        self.inner.listeners.borrow_mut().push((id, Rc::new(listener)));
        let inner = Rc::downgrade(&self.inner);
        Subscription::new(move || {
            if let Some(inner) = inner.upgrade() {
                inner.listeners.borrow_mut().retain(|(other, _)| *other != id);
            }
        })
    }

    /// Puts the page back to how it was after `to` entries, or forward
    /// again, and resolves with the changes it couldn't put back. When none
    /// went back, the position stays where it was.
    pub fn seek(&self, to: usize, cx: &mut App) -> Task<Vec<RestoreStep>> {
        let this = self.clone();
        self.in_turn(cx, async move |cx| {
            let (target, from, before) = {
                let state = this.inner.state.borrow();
                let target = to.min(state.entries.len());
                if target == state.position {
                    return Vec::new();
                }
                (target, state.position, state.entries.clone())
            };
            // Every key the skipped stretch touched goes to its value at the
            // target.
            let mut touched: IndexMap<String, (SharedString, SharedString)> = IndexMap::new();
            for entry in &before[target.min(from)..target.max(from)] {
                touched.insert(id_of(&entry.r#ref, &entry.key), (entry.r#ref.clone(), entry.key.clone()));
            }
            // At the target while it writes, so a change made meanwhile drops
            // the future it replaces.
            this.update(before.clone(), target);
            let steps = touched
                .iter()
                .map(|(id, (r#ref, key))| RestoreStep {
                    r#ref: r#ref.clone(),
                    key: key.clone(),
                    value: value_at(&before, target, id),
                })
                .collect();
            let failed = this.restore(steps, cx).await;
            // Nothing went back and nothing new was recorded, so the page is
            // still where it was.
            let unchanged = Rc::ptr_eq(&this.inner.state.borrow().entries, &before);
            if failed.len() == touched.len() && unchanged {
                this.update(before, from);
            }
            failed
        })
    }

    /// Reverts every applied change one source made, like `Agent`, keeps
    /// the rest, and resolves with the changes it couldn't put back, whose
    /// entries stay applied.
    pub fn undo_source(&self, source: ChangeSource, cx: &mut App) -> Task<Vec<RestoreStep>> {
        let this = self.clone();
        self.in_turn(cx, async move |cx| {
            let mut undone: HashSet<SharedString> = HashSet::new();
            let mut values: HashMap<String, AgentValue> = HashMap::new();
            let mut touched: IndexMap<String, (SharedString, SharedString)> = IndexMap::new();
            {
                let state = this.inner.state.borrow();
                for entry in &state.entries[..state.position] {
                    let id = id_of(&entry.r#ref, &entry.key);
                    values.entry(id.clone()).or_insert_with(|| entry.previous.clone());
                    if entry.reverted {
                        continue;
                    }
                    if entry.source == source {
                        undone.insert(entry.id.clone());
                        touched.insert(id, (entry.r#ref.clone(), entry.key.clone()));
                    } else {
                        values.insert(id, entry.value.clone());
                    }
                }
            }
            if touched.is_empty() {
                return Vec::new();
            }
            let steps = touched
                .iter()
                .map(|(id, (r#ref, key))| RestoreStep {
                    r#ref: r#ref.clone(),
                    key: key.clone(),
                    value: values.get(id).cloned(),
                })
                .collect();
            let failed = this.restore(steps, cx).await;
            let stayed: HashSet<String> = failed.iter().map(|step| id_of(&step.r#ref, &step.key)).collect();
            let (entries, position) = {
                let state = this.inner.state.borrow();
                let entries: Vec<HistoryEntry> = state
                    .entries
                    .iter()
                    .map(|entry| {
                        let mut entry = entry.clone();
                        if undone.contains(&entry.id) && !stayed.contains(&id_of(&entry.r#ref, &entry.key)) {
                            entry.reverted = true;
                        }
                        entry
                    })
                    .collect();
                (entries, state.position)
            };
            this.update(Rc::from(entries), position);
            failed
        })
    }

    /// Forgets every entry, keeping the page as it is.
    pub fn clear(&self) {
        self.update(Rc::from(Vec::new()), 0);
    }

    /// Stops recording.
    pub fn destroy(&self) {
        self.inner.stop.borrow_mut().take();
        self.inner.listeners.borrow_mut().clear();
    }

    fn update(&self, entries: Rc<[HistoryEntry]>, position: usize) {
        {
            let mut state = self.inner.state.borrow_mut();
            state.entries = entries;
            state.position = position;
        }
        self.inner.notify.unbounded_send(()).ok();
    }

    fn heard(&self, observed: &ObservedChange) {
        if !self.inner.keys.contains(&observed.key) {
            return;
        }
        let id = id_of(&observed.r#ref, &observed.key);
        let redacted = registry::entry(&observed.r#ref).is_some_and(|entry| entry.borrow().secret);
        let (next, at) = {
            let mut state = self.inner.state.borrow_mut();
            // The agent changing a key being put back is the history's own
            // write.
            if observed.source == ChangeSource::Agent && state.restoring.contains(&id) {
                return;
            }
            // Anyone else's change is later than the restore, so it stays and
            // the restore leaves that key.
            state.restoring.remove(&id);
            state.count += 1;
            let entry = HistoryEntry {
                id: format!("e{}", state.count).into(),
                r#ref: observed.r#ref.clone(),
                label: observed.label.clone().unwrap_or_else(|| observed.component.clone()),
                key: observed.key.clone(),
                value: observed.value.clone(),
                previous: observed.previous.clone(),
                source: observed.source,
                reason: observed.reason,
                reverted: false,
                redacted,
            };
            // A change made from the past drops the future it replaces.
            let mut kept: Vec<HistoryEntry> = state.entries[..state.position].to_vec();
            let joins = self.inner.coalesce
                && kept.last().is_some_and(|last| {
                    !last.reverted
                        && last.r#ref == entry.r#ref
                        && last.key == entry.key
                        && last.source == entry.source
                        && last.reason == entry.reason
                        && entry.reason != Reason::ItemPress
                        && entry.reason != Reason::Imperative
                });
            if joins {
                if let Some(last) = kept.last_mut() {
                    last.value = entry.value;
                }
                let at = kept.len();
                (kept, at)
            } else {
                kept.push(entry);
                let at = kept.len();
                (kept, at)
            }
        };
        self.update(Rc::from(next), at);
    }

    fn in_turn<R: 'static>(&self, cx: &mut App, run: impl AsyncFnOnce(&mut AsyncApp) -> R + 'static) -> Task<R> {
        let previous = self.inner.turn.borrow().clone();
        let (done, finished) = oneshot::channel::<()>();
        *self.inner.turn.borrow_mut() = finished.map(|_| ()).boxed_local().shared();
        cx.spawn(async move |cx| {
            previous.await;
            let result = run(cx).await;
            drop(done);
            result
        })
    }

    // One act per key, so one that's refused or left the page doesn't stop
    // the rest. Resolves with the ones that didn't go back. A redacted key
    // never does: its values read "[redacted]", which would land in the
    // field.
    async fn restore(&self, steps: Vec<RestoreStep>, cx: &mut AsyncApp) -> Vec<RestoreStep> {
        struct Done(Weak<Inner>);
        impl Drop for Done {
            fn drop(&mut self) {
                if let Some(inner) = self.0.upgrade() {
                    inner.state.borrow_mut().restoring.clear();
                }
            }
        }
        self.inner.state.borrow_mut().restoring = steps
            .iter()
            .map(|step| id_of(&step.r#ref, &step.key))
            .collect();
        let _done = Done(Rc::downgrade(&self.inner));
        // An edit in progress supersedes the restore before its coalesced
        // entry commits.
        let weak = Rc::downgrade(&self.inner);
        let within = self.inner.within.clone();
        let _guard = cx.update(|cx| {
            observe(
                move |change| {
                    if change.source != ChangeSource::Agent
                        && let Some(inner) = weak.upgrade()
                    {
                        inner
                            .state
                            .borrow_mut()
                            .restoring
                            .remove(&id_of(&change.r#ref, &change.key));
                    }
                },
                ObserveOptions { within, live: true },
                cx,
            )
        });
        let redacted: HashSet<String> = self
            .inner
            .state
            .borrow()
            .entries
            .iter()
            .filter(|entry| entry.redacted)
            .map(|entry| id_of(&entry.r#ref, &entry.key))
            .collect();
        let mut failed = Vec::new();
        for step in steps {
            let id = id_of(&step.r#ref, &step.key);
            let restoring = self.inner.state.borrow().restoring.contains(&id);
            let mut done = false;
            if let Some(value) = &step.value
                && !redacted.contains(&id)
                && restoring
            {
                let weak = Rc::downgrade(&self.inner);
                let guarded = id.clone();
                let guard: Guard = Rc::new(move || {
                    weak.upgrade()
                        .is_some_and(|inner| inner.state.borrow().restoring.contains(&guarded))
                });
                let steps = vec![AgentStep::new(step.r#ref.to_string(), setter_of(&step.key)).args([value.clone()])];
                let agent = self.inner.agent.clone();
                let result = cx.update(|cx| guarded_act(&agent, steps, guard, cx)).await;
                // A change callback or a controlled owner can refuse a write
                // the agent still calls done.
                done = result.ok
                    && cx.update(|cx| {
                        current(&step.r#ref, &step.key, cx).is_some_and(|now| same_value(&now, value))
                    });
            }
            if !done {
                failed.push(step);
            }
        }
        failed
    }
}
