//! Port of `foundation/kernel/primitive.ts`'s per-instance identity.
//!
//! `definePrimitive` builds an instance from options, state, an api and
//! slots. In gpui a component is an entity: its struct holds the state, its
//! methods are the api, and its part methods build the slots' elements. What
//! every instance still shares lives in [`Instance`]: its name, its unique id,
//! its devtools label, its parts in the page, and its place in the registry
//! `observe` and the agent bridge read, through an [`InstanceHandle`].
//!
//! A component hands its entity to its instance with [`Instance::with_agent`]
//! as it's made, which is how the bridge reads its state and calls its
//! actions, how parts whose presence it asks about re-render it, and how the
//! views its parts render in redraw when it notifies.
//!
//! gpui redraws a cached view (`AnyView::cached`) only when the view, or a
//! view inside it, notifies: an entity that isn't a view marks no view
//! dirty. So each part records the view it prepaints in, and when the
//! component notifies, it notifies those views, which render it again. The
//! views are taken as they're notified: a part still in one records it
//! again as the view redraws.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::fmt::{self, Debug};
use std::rc::{Rc, Weak};

use gpui::{App, Context, ElementId, EntityId, SharedString, WeakEntity, Window};

use super::details::{ChangeDetails, ChangeSource, with_source};
use super::part::BoundPart;
use super::primitive_types::{Agent, AgentArgs, AgentError, AgentItem, AgentResult, AgentState, AgentSurface, AgentValue};
use super::watchers;

pub use super::attrs::id_part;

thread_local! {
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

// What the instance shares with its parts, the watchers and the bridge.
pub(crate) struct Shared {
    pub(crate) name: &'static str,
    pub(crate) id: SharedString,
    pub(crate) label: RefCell<SharedString>,
    pub(crate) agent: RefCell<Option<Rc<dyn ErasedAgent>>>,
    pub(crate) entity: Cell<Option<EntityId>>,
    // The parts in the page now, by the serial they bound with, which is
    // the order they bound.
    pub(crate) bound: RefCell<BTreeMap<u64, BoundPart>>,
    // Slots whose parts coming or going re-render the component, because it
    // asked whether they're in the page.
    pub(crate) watched: RefCell<HashSet<&'static str>>,
    // Whether a frame has drawn any of its parts yet.
    pub(crate) drawn: Cell<bool>,
    // The views its parts prepainted in since it last notified them.
    pub(crate) views: RefCell<Vec<EntityId>>,
}

impl Shared {
    pub(crate) fn watches(&self, slot: &str) -> bool {
        self.watched.borrow().contains(slot)
    }

    pub(crate) fn record_view(&self, view: EntityId) {
        let mut views = self.views.borrow_mut();
        if !views.contains(&view) {
            views.push(view);
        }
    }
}

// Notifies the views the instance's parts rendered in whenever its entity
// notifies, so a cached view that renders them draws them again.
fn redraw_views<T: 'static>(shared: &Rc<Shared>, cx: &mut Context<T>) {
    let shared = Rc::downgrade(shared);
    let entity = cx.entity();
    App::observe(cx, &entity, move |_, cx| {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        let views = std::mem::take(&mut *shared.views.borrow_mut());
        for view in views {
            cx.notify(view);
        }
    })
    .detach();
}

/// One live component: its name, id and label.
pub struct Instance {
    name: &'static str,
    id: SharedString,
    label: SharedString,
    /// Logs each change with `log::debug!`, the `debug` option.
    pub debug: bool,
    shared: Rc<Shared>,
}

impl Debug for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Instance")
            .field("name", &self.name)
            .field("id", &self.id)
            .field("label", &self.label)
            .field("debug", &self.debug)
            .finish()
    }
}

impl Instance {
    /// A new instance of the primitive `name`, like `"tabs"`. Registers it
    /// with the watchers until it drops.
    pub fn new(name: &'static str) -> Self {
        let n = NEXT_ID.with(|next| {
            let n = next.get();
            next.set(n + 1);
            n
        });
        Self::made(name, format!("ui-sdk-{n}").into())
    }

    /// An instance whose id the app wrote, like an `id` prop.
    pub fn with_id(name: &'static str, id: impl Into<SharedString>) -> Self {
        Self::made(name, id.into())
    }

    fn made(name: &'static str, id: SharedString) -> Self {
        let shared = Rc::new(Shared {
            name,
            id: id.clone(),
            label: RefCell::new(name.into()),
            agent: RefCell::new(None),
            entity: Cell::new(None),
            bound: RefCell::new(BTreeMap::new()),
            watched: RefCell::new(HashSet::new()),
            drawn: Cell::new(false),
            views: RefCell::new(Vec::new()),
        });
        let instance = Self {
            name,
            id,
            label: name.into(),
            debug: false,
            shared,
        };
        watchers::emit_create(&instance);
        instance
    }

    /// Hands the component's entity to the instance, as the component is
    /// made, so the agent bridge reads its state and calls its actions,
    /// parts whose presence it asks about re-render it, and the views its
    /// parts render in redraw when it notifies.
    pub fn with_agent<T: Agent>(self, cx: &mut Context<T>) -> Self {
        *self.shared.agent.borrow_mut() = Some(Rc::new(Erased {
            entity: cx.weak_entity(),
        }));
        self.with_entity(cx)
    }

    /// Hands an entity that isn't an agent to its instance as it's made,
    /// like the native text field's, so the views its parts render in redraw
    /// when it notifies, and parts whose presence it asks about re-render it.
    pub fn with_entity<T: 'static>(self, cx: &mut Context<T>) -> Self {
        self.shared.entity.set(Some(cx.entity_id()));
        redraw_views(&self.shared, cx);
        self
    }

    /// Records the view drawing now as one that shows the entity, for an
    /// element that draws its state outside its parts, like the native text
    /// field's text. Call it as the element prepaints.
    pub fn record_view(&self, window: &Window) {
        self.shared.record_view(window.current_view());
    }

    /// The primitive's name, which prefixes each part's slot name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The instance's unique id, `part.id` in the original.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// Sets the name devtools and warnings show, the definition's `label`.
    pub fn set_label(&mut self, label: impl Into<SharedString>) {
        self.label = label.into();
        *self.shared.label.borrow_mut() = self.label.clone();
    }

    /// A slot's name, like `tabs-list` for the tabs' `list` slot, and the
    /// name alone for `root` (`data-slot`).
    pub fn slot_name(&self, slot: &str) -> String {
        if slot == "root" {
            self.name.to_string()
        } else {
            format!("{}-{}", self.name, slot)
        }
    }

    /// The element id of a part: the instance id, the slot, and a key for
    /// parts there are many of. Relations between parts name it.
    pub fn element_id(&self, slot: &'static str, key: Option<&str>) -> ElementId {
        match key {
            Some(key) => ElementId::Name(format!("{}-{}-{}", self.id, slot, id_part(key)).into()),
            None => ElementId::Name(format!("{}-{}", self.id, slot).into()),
        }
    }

    /// The handle watchers and the agent bridge keep.
    pub fn handle(&self) -> InstanceHandle {
        InstanceHandle {
            shared: Rc::downgrade(&self.shared),
        }
    }

    /// The component's entity, once `with_agent` handed it over.
    pub fn entity_id(&self) -> Option<EntityId> {
        self.shared.entity.get()
    }

    /// The instance's parts in the page now, in the order they bound.
    pub fn bound_parts(&self) -> Vec<BoundPart> {
        self.shared.bound.borrow().values().cloned().collect()
    }

    pub(crate) fn shared(&self) -> &Rc<Shared> {
        &self.shared
    }

    // `debug` logs in development builds only, as the original's does.
    #[doc(hidden)]
    pub fn debug_change(&self, key: &str, value: &dyn Debug, details: &ChangeDetails) {
        if super::dev::DEV && self.debug {
            log::debug!(
                "[ui-sdk] {}#{} {}: {:?} ({}, {})",
                self.label,
                self.id,
                key,
                value,
                details.reason.as_str(),
                details.source.as_str()
            );
        }
    }

    #[doc(hidden)]
    pub fn debug_log(&self, key: &str, value: &dyn Debug, reason: &str, source: ChangeSource) {
        if super::dev::DEV && self.debug {
            log::debug!(
                "[ui-sdk] {}#{} {}: {:?} ({}, {})",
                self.label,
                self.id,
                key,
                value,
                reason,
                source.as_str()
            );
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        watchers::emit_destroy(self);
    }
}

/// A live instance as the watchers and the agent bridge hold it, weakly, so
/// it never keeps a component alive.
#[derive(Clone)]
pub struct InstanceHandle {
    shared: Weak<Shared>,
}

impl Debug for InstanceHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.shared.upgrade() {
            Some(shared) => write!(f, "InstanceHandle({}#{})", shared.name, shared.id),
            None => write!(f, "InstanceHandle(gone)"),
        }
    }
}

impl InstanceHandle {
    /// Whether the instance is still alive.
    pub fn is_alive(&self) -> bool {
        self.shared.strong_count() > 0
    }

    /// The primitive's name, like `"select"`.
    pub fn name(&self) -> Option<&'static str> {
        self.shared.upgrade().map(|shared| shared.name)
    }

    pub fn id(&self) -> Option<SharedString> {
        self.shared.upgrade().map(|shared| shared.id.clone())
    }

    /// What warnings and the devtools call it.
    pub fn label(&self) -> Option<SharedString> {
        self.shared.upgrade().map(|shared| shared.label.borrow().clone())
    }

    pub fn entity_id(&self) -> Option<EntityId> {
        self.shared.upgrade().and_then(|shared| shared.entity.get())
    }

    /// Its parts in the page now, in the order they bound.
    pub fn bound_parts(&self) -> Vec<BoundPart> {
        self.shared
            .upgrade()
            .map(|shared| shared.bound.borrow().values().cloned().collect())
            .unwrap_or_default()
    }

    /// What the agent bridge reads and calls, once the component handed its
    /// entity over.
    pub fn agent(&self) -> Option<AgentHandle> {
        let shared = self.shared.upgrade()?;
        let agent = shared.agent.borrow().clone()?;
        Some(AgentHandle(agent))
    }

    pub fn ptr_eq(&self, other: &InstanceHandle) -> bool {
        Weak::ptr_eq(&self.shared, &other.shared)
    }
}

/// An action as the bridge lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentActionInfo {
    pub name: &'static str,
    pub listed: bool,
}

pub(crate) trait ErasedAgent {
    fn surface(&self) -> AgentSurface;
    fn controlled(&self) -> Option<&'static [&'static str]>;
    fn commits(&self) -> &'static [&'static str];
    fn actions(&self) -> Vec<AgentActionInfo>;
    fn state(&self, cx: &App) -> Option<AgentState>;
    fn items(&self, cx: &App) -> Option<Vec<AgentItem>>;
    fn shown(&self, cx: &App) -> bool;
    fn custom_value(&self, cx: &App) -> bool;
    fn act(&self, action: &str, args: &[AgentValue], window: &mut Window, cx: &mut App) -> AgentResult;
}

struct Erased<T> {
    entity: WeakEntity<T>,
}

impl<T: Agent> ErasedAgent for Erased<T> {
    fn surface(&self) -> AgentSurface {
        T::SURFACE
    }

    fn controlled(&self) -> Option<&'static [&'static str]> {
        T::CONTROLLED
    }

    fn commits(&self) -> &'static [&'static str] {
        T::COMMITS
    }

    fn actions(&self) -> Vec<AgentActionInfo> {
        T::ACTIONS
            .iter()
            .map(|action| AgentActionInfo {
                name: action.name,
                listed: action.listed,
            })
            .collect()
    }

    fn state(&self, cx: &App) -> Option<AgentState> {
        let entity = self.entity.upgrade()?;
        Some(entity.read(cx).agent_state(cx))
    }

    fn items(&self, cx: &App) -> Option<Vec<AgentItem>> {
        let entity = self.entity.upgrade()?;
        entity.read(cx).agent_items(cx)
    }

    fn shown(&self, cx: &App) -> bool {
        self.entity
            .upgrade()
            .is_some_and(|entity| entity.read(cx).agent_shown(cx))
    }

    fn custom_value(&self, cx: &App) -> bool {
        self.entity
            .upgrade()
            .is_some_and(|entity| entity.read(cx).agent_custom_value(cx))
    }

    fn act(&self, action: &str, args: &[AgentValue], window: &mut Window, cx: &mut App) -> AgentResult {
        let Some(found) = T::ACTIONS.iter().find(|candidate| candidate.name == action) else {
            return Err(AgentError::Unknown(action.to_string().into()));
        };
        let entity = self.entity.upgrade().ok_or(AgentError::Gone)?;
        let run = found.run;
        let args = AgentArgs::new(action, args);
        with_source(ChangeSource::Agent, || {
            entity.update(cx, |this, cx| run(this, &args, window, cx))
        })
    }
}

/// What the agent bridge reads from a live component and calls on it.
#[derive(Clone)]
pub struct AgentHandle(Rc<dyn ErasedAgent>);

impl Debug for AgentHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AgentHandle").field(&self.0.surface()).finish()
    }
}

impl AgentHandle {
    /// Its definition's `agent` flags.
    pub fn surface(&self) -> AgentSurface {
        self.0.surface()
    }

    /// The state keys a user controls, `None` for the default.
    pub fn controlled(&self) -> Option<&'static [&'static str]> {
        self.0.controlled()
    }

    /// The state keys an interaction commits.
    pub fn commits(&self) -> &'static [&'static str] {
        self.0.commits()
    }

    /// Every action it takes, listed or not.
    pub fn actions(&self) -> Vec<AgentActionInfo> {
        self.0.actions()
    }

    /// Whether it takes `action`.
    pub fn takes(&self, action: &str) -> bool {
        self.0.actions().iter().any(|info| info.name == action)
    }

    /// Its state now, or `None` once it's gone.
    pub fn state(&self, cx: &App) -> Option<AgentState> {
        self.0.state(cx)
    }

    pub fn items(&self, cx: &App) -> Option<Vec<AgentItem>> {
        self.0.items(cx)
    }

    pub fn shown(&self, cx: &App) -> bool {
        self.0.shown(cx)
    }

    pub fn custom_value(&self, cx: &App) -> bool {
        self.0.custom_value(cx)
    }

    /// Runs one of its actions as the agent: every change it makes, and
    /// every change those cause, reports `ChangeSource::Agent`. Call it
    /// outside the component's own update.
    pub fn act(&self, action: &str, args: &[AgentValue], window: &mut Window, cx: &mut App) -> AgentResult {
        self.0.act(action, args, window, cx)
    }
}
