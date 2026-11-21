//! What a page's document gives the forms code, which gpui has no equivalent
//! of: a tree of elements in page order, whether each one is in the page, and
//! events that travel through the tree.
//!
//! A [`Node`] stands for one element the forms code reads: a form, a
//! fieldset, a legend, a label, a control, or any element whose surroundings
//! matter, like a field's root. The part it stands for puts it in its element
//! with [`Node::track`], which records each frame where the element sits: the
//! path of element ids that leads to it, which says what holds it, and the
//! order it drew in, which is page order. An element that stops drawing leaves
//! the page. Once a frame has drawn, what follows from the tree, like the form
//! that owns a control or the fieldsets that disable it, is worked out again,
//! and the entities that read it are notified when it changes.
//!
//! Elements inside a cached view (`AnyView::cached`) keep the place and order
//! they had when they last drew.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::fmt;
use std::rc::{Rc, Weak};

use gpui::{
    AnyWindowHandle, App, Bounds, Edges, Element, ElementId, EntityId, GlobalElementId,
    InspectorElementId, LayoutId, ParentElement, Pixels, Position, SharedString, Size, Style,
    StatefulInteractiveElement, Subscription, Window, px, relative,
};
use ui_sdk_kernel::InputEvent;

use crate::elements::{Form, FormFacts};
use crate::internals::ControlData;

/// What a node stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// A `<form>`. The controls inside it, or naming it in their `form`
    /// option, belong to it.
    Form,
    /// A `<fieldset>`. Its `disabled` disables every control inside it,
    /// except those in its first legend.
    Fieldset,
    /// A `<legend>`, which names its fieldset.
    Legend,
    /// A `<label>`, which names its control and activates it when pressed.
    Label,
    /// A control that takes part in forms, through its `ElementInternals`.
    Control,
    /// Any other element the forms code reads, like a field's root.
    Element,
}

/// One element of the page, as the forms code sees it. Clones are the same
/// node. It leaves the page when its last handle drops.
#[derive(Clone)]
pub struct Node(pub(crate) Rc<NodeInner>);

/// A node that doesn't keep its element alive.
#[derive(Clone)]
pub struct WeakNode(Weak<NodeInner>);

pub(crate) struct NodeInner {
    key: u64,
    kind: NodeKind,
    state: RefCell<NodeState>,
    pub(crate) control: Option<RefCell<ControlData>>,
    pub(crate) form: Option<RefCell<FormFacts>>,
}

pub(crate) type DisabledCallback = Rc<dyn Fn(bool, &mut Window, &mut App)>;

#[derive(Default)]
struct NodeState {
    id: Option<SharedString>,
    tag: Option<&'static str>,
    place: Option<Place>,
    connected: bool,
    // The element state that last held the node, which says it's still drawn.
    token: u64,
    observer: Option<EntityId>,
    listeners: Vec<Listener>,
    watchers: Vec<(u64, DisabledCallback)>,
    // A fieldset's own `disabled`.
    disabled: bool,
    // What a label or legend reads as.
    text: SharedString,
    // A label's `for`.
    for_id: Option<SharedString>,
    derived: Derived,
    validity_changed: bool,
}

struct Place {
    path: Rc<[ElementId]>,
    order: u64,
    window: AnyWindowHandle,
    bounds: Bounds<Pixels>,
}

// What follows from the tree, worked out once a frame has drawn.
#[derive(Clone, Default)]
struct Derived {
    form_disabled: bool,
    owner: Option<WeakNode>,
    labels: Vec<WeakNode>,
    name: Option<SharedString>,
    control: Option<WeakNode>,
}

impl PartialEq for Derived {
    fn eq(&self, other: &Self) -> bool {
        self.form_disabled == other.form_disabled
            && same_weak(&self.owner, &other.owner)
            && self.labels.len() == other.labels.len()
            && self.labels.iter().zip(&other.labels).all(|(a, b)| a.ptr_eq(b))
            && self.name == other.name
            && same_weak(&self.control, &other.control)
    }
}

fn same_weak(a: &Option<WeakNode>, b: &Option<WeakNode>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.ptr_eq(b),
        (None, None) => true,
        _ => false,
    }
}

thread_local! {
    static DOCUMENT: RefCell<Document> = RefCell::new(Document::default());
    // Writes from code, which fire no `input` or `change` (`value.rs`).
    pub(crate) static SCRIPT_WRITES: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

#[derive(Default)]
struct Document {
    nodes: Vec<Weak<NodeInner>>,
    next_key: u64,
    next_order: u64,
    next_token: u64,
    next_listener: u64,
    // Something changed that what follows from the tree depends on.
    dirty: bool,
    // A check is queued for the end of the current effect cycle.
    queued: bool,
    listeners: Vec<Listener>,
    validity_listeners: Vec<(u64, ValidityListener)>,
}

pub(crate) type ValidityListener = Rc<dyn Fn(&Node, &mut Window, &mut App)>;

fn next_id(pick: impl FnOnce(&mut Document) -> &mut u64) -> u64 {
    DOCUMENT.with(|document| {
        let mut document = document.borrow_mut();
        let counter = pick(&mut document);
        *counter += 1;
        *counter
    })
}

pub(crate) fn mark_dirty() {
    DOCUMENT
        .try_with(|document| {
            if let Ok(mut document) = document.try_borrow_mut() {
                document.dirty = true;
            }
        })
        .ok();
}

/// Marks what follows from the tree as out of date, and works it out again at
/// the end of the current effect cycle.
pub(crate) fn invalidate(cx: &mut App) {
    mark_dirty();
    queue_check(cx);
}

fn queue_check(cx: &mut App) {
    let queue = DOCUMENT.with(|document| {
        let mut document = document.borrow_mut();
        !std::mem::replace(&mut document.queued, true)
    });
    if queue {
        cx.defer(|cx| {
            DOCUMENT.with(|document| document.borrow_mut().queued = false);
            settle(cx);
        });
    }
}

impl Node {
    pub(crate) fn with_kind(kind: NodeKind) -> Self {
        let node = Node(Rc::new(NodeInner {
            key: next_id(|document| &mut document.next_key),
            kind,
            state: RefCell::new(NodeState::default()),
            control: (kind == NodeKind::Control).then(|| RefCell::new(ControlData::default())),
            form: (kind == NodeKind::Form).then(|| RefCell::new(FormFacts::default())),
        }));
        DOCUMENT.with(|document| document.borrow_mut().nodes.push(Rc::downgrade(&node.0)));
        node
    }

    /// A node for an element that is none of the others, so what's around it
    /// and inside it can be read, like whether a fieldset disables it.
    pub fn element() -> Self {
        Self::with_kind(NodeKind::Element)
    }

    /// An element node with a tag that [`Node::closest_tagged`] finds, like
    /// `"field"` for a field's root, standing in for its `data-slot`.
    pub fn tagged(tag: &'static str) -> Self {
        let node = Self::element();
        node.0.state.borrow_mut().tag = Some(tag);
        node
    }

    /// What it stands for.
    pub fn kind(&self) -> NodeKind {
        self.0.kind
    }

    /// A number unique to the node while the app runs.
    pub fn key(&self) -> u64 {
        self.0.key
    }

    /// The tag [`Node::tagged`] gave it.
    pub fn tag(&self) -> Option<&'static str> {
        self.0.state.borrow().tag
    }

    /// Its `id`, which labels' `for` and controls' `form` point at.
    pub fn id(&self) -> Option<SharedString> {
        self.0.state.borrow().id.clone()
    }

    /// Sets its `id`.
    pub fn set_id(&self, id: Option<SharedString>, cx: &mut App) {
        let mut state = self.0.state.borrow_mut();
        if state.id == id {
            return;
        }
        state.id = id;
        drop(state);
        invalidate(cx);
    }

    // Sets its `id` with no update to queue the check in; the next frame's
    // check hears it.
    pub(crate) fn set_id_quiet(&self, id: Option<SharedString>) {
        let mut state = self.0.state.borrow_mut();
        if state.id == id {
            return;
        }
        state.id = id;
        drop(state);
        mark_dirty();
    }

    // The id labels match: its own, or the one its field gave it.
    pub(crate) fn effective_id(&self) -> Option<SharedString> {
        self.id().or_else(|| {
            self.0
                .control
                .as_ref()
                .and_then(|control| control.borrow().attrs.as_ref()?.id.clone())
        })
    }

    /// The entity to notify when what follows from the tree changes for this
    /// node, like the fieldsets that disable it or the labels that name it.
    pub fn set_observer(&self, entity: Option<EntityId>) {
        self.0.state.borrow_mut().observer = entity;
    }

    pub(crate) fn observer(&self) -> Option<EntityId> {
        self.0.state.borrow().observer
    }

    /// Puts the node in `el`, the element it stands for, so it knows each
    /// frame where the element sits. Call it on the part's element before
    /// adding children; it adds an empty child that fills the element. The
    /// element has an id, as a part does, since ids are what say which
    /// element holds which.
    pub fn track<E: ParentElement + StatefulInteractiveElement>(&self, el: E) -> E {
        el.child(Tracker { node: self.clone() })
    }

    /// Whether its element drew in the last frame.
    pub fn is_connected(&self) -> bool {
        self.0.state.borrow().connected
    }

    /// Its element's bounds when it last drew.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.0.state.borrow().place.as_ref().map(|place| place.bounds)
    }

    /// The id of the element it stands for, as it last drew, which ARIA
    /// relations name, like a label's for `labelled_by`.
    pub fn element_id(&self) -> Option<ElementId> {
        let state = self.0.state.borrow();
        state
            .place
            .as_ref()
            .filter(|_| state.connected)
            .and_then(|place| place.path.last().cloned())
    }

    pub(crate) fn window(&self) -> Option<AnyWindowHandle> {
        self.0.state.borrow().place.as_ref().map(|place| place.window)
    }

    fn path(&self) -> Option<Rc<[ElementId]>> {
        let state = self.0.state.borrow();
        state
            .connected
            .then(|| state.place.as_ref().map(|place| place.path.clone()))
            .flatten()
    }

    fn order(&self) -> u64 {
        self.0.state.borrow().place.as_ref().map_or(0, |place| place.order)
    }

    /// A handle that doesn't keep it alive.
    pub fn downgrade(&self) -> WeakNode {
        WeakNode(Rc::downgrade(&self.0))
    }

    /// Whether `other` is this node's element or inside it, like
    /// `Node.contains`. False when either isn't in the page.
    pub fn contains(&self, other: &Node) -> bool {
        match (self.path(), other.path()) {
            (Some(outer), Some(inner)) => inner.starts_with(&outer),
            _ => false,
        }
    }

    /// The nodes whose elements hold this one, nearest first.
    pub fn ancestors(&self) -> Vec<Node> {
        let Some(path) = self.path() else {
            return Vec::new();
        };
        let mut ancestors: Vec<Placed> = placed()
            .into_iter()
            .filter(|other| other.path.len() < path.len() && path.starts_with(&other.path))
            .collect();
        ancestors.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.order.cmp(&b.order)));
        ancestors.into_iter().map(|placed| placed.node).collect()
    }

    /// The nearest node, this one or one around it, that `matches`, like
    /// `Element.closest`.
    pub fn closest(&self, matches: impl Fn(&Node) -> bool) -> Option<Node> {
        if matches(self) {
            return Some(self.clone());
        }
        self.ancestors().into_iter().find(|node| matches(node))
    }

    /// The nearest element node with `tag`, this one or one around it.
    pub fn closest_tagged(&self, tag: &str) -> Option<Node> {
        self.closest(|node| node.tag() == Some(tag))
    }

    /// The nodes inside this one's element, in page order, not counting
    /// itself.
    pub fn descendants(&self) -> Vec<Node> {
        let Some(path) = self.path() else {
            return Vec::new();
        };
        placed()
            .into_iter()
            .filter(|other| other.node.0.key != self.0.key && other.path.starts_with(&path))
            .map(|placed| placed.node)
            .collect()
    }

    /// Whether a disabled fieldset around it disables it, as of the last
    /// frame. A control in a fieldset's first legend stays enabled.
    pub fn form_disabled(&self) -> bool {
        self.0.state.borrow().derived.form_disabled
    }

    /// Whether a person can't use it: disabled by itself or a fieldset, or
    /// marked `aria-disabled` (`isDisabledElement`).
    pub fn is_disabled(&self) -> bool {
        if self.form_disabled() {
            return true;
        }
        match self.0.kind {
            NodeKind::Fieldset => self.0.state.borrow().disabled,
            NodeKind::Control => self
                .0
                .control
                .as_ref()
                .is_some_and(|control| control.borrow().is_disabled()),
            _ => false,
        }
    }

    /// The form a control belongs to, or the form around any other element.
    pub fn form(&self) -> Option<Form> {
        let owner = self.0.state.borrow().derived.owner.clone()?;
        owner.upgrade().map(Form::from_node)
    }

    /// Calls `listener` for each event of `kind` that reaches this node: one
    /// aimed at it, or one passing through on its way to or from a node
    /// inside it. `capture` hears it on the way in, before the target, like
    /// `addEventListener`'s `capture`. Dropping the subscription stops it.
    pub fn listen(
        &self,
        kind: FormEventType,
        capture: bool,
        listener: impl Fn(&FormEvent, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        let id = next_id(|document| &mut document.next_listener);
        self.0.state.borrow_mut().listeners.push(Listener {
            id,
            kind,
            capture,
            callback: Rc::new(listener),
        });
        let node = self.downgrade();
        Subscription::new(move || {
            if let Some(node) = node.upgrade() {
                node.0.state.borrow_mut().listeners.retain(|listener| listener.id != id);
            }
        })
    }

    pub(crate) fn watch_disabled(&self, callback: DisabledCallback) -> Subscription {
        let id = next_id(|document| &mut document.next_listener);
        self.0.state.borrow_mut().watchers.push((id, callback));
        let node = self.downgrade();
        Subscription::new(move || {
            if let Some(node) = node.upgrade() {
                node.0.state.borrow_mut().watchers.retain(|(each, _)| *each != id);
            }
        })
    }

    pub(crate) fn fieldset_disabled_flag(&self) -> bool {
        self.0.state.borrow().disabled
    }

    pub(crate) fn set_fieldset_disabled_flag(&self, disabled: bool) -> bool {
        let mut state = self.0.state.borrow_mut();
        std::mem::replace(&mut state.disabled, disabled) != disabled
    }

    pub(crate) fn text(&self) -> SharedString {
        self.0.state.borrow().text.clone()
    }

    pub(crate) fn set_text(&self, text: SharedString) -> bool {
        let mut state = self.0.state.borrow_mut();
        if state.text == text {
            return false;
        }
        state.text = text;
        true
    }

    pub(crate) fn for_id(&self) -> Option<SharedString> {
        self.0.state.borrow().for_id.clone()
    }

    pub(crate) fn set_for_id(&self, for_id: Option<SharedString>) -> bool {
        let mut state = self.0.state.borrow_mut();
        if state.for_id == for_id {
            return false;
        }
        state.for_id = for_id;
        true
    }

    // The labels naming a control, in page order, as of the last frame.
    pub(crate) fn label_nodes(&self) -> Vec<Node> {
        let state = self.0.state.borrow();
        state.derived.labels.iter().filter_map(WeakNode::upgrade).collect()
    }

    // The text of those labels, which names the control.
    pub(crate) fn label_name(&self) -> Option<SharedString> {
        self.0.state.borrow().derived.name.clone()
    }

    // A label's control, as of the last frame.
    pub(crate) fn labeled_control(&self) -> Option<Node> {
        let state = self.0.state.borrow();
        state.derived.control.as_ref().and_then(WeakNode::upgrade)
    }

    // Tells validity listeners, once the frame has drawn, that the control's
    // validity changed (`validityChanged`).
    pub(crate) fn mark_validity_changed(&self) {
        self.0.state.borrow_mut().validity_changed = true;
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Node {}

impl std::hash::Hash for Node {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.key.hash(state);
    }
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.0.state.borrow();
        f.debug_struct("Node")
            .field("key", &self.0.key)
            .field("kind", &self.0.kind)
            .field("id", &state.id)
            .field("connected", &state.connected)
            .finish()
    }
}

impl WeakNode {
    /// The node, while something keeps it.
    pub fn upgrade(&self) -> Option<Node> {
        self.0.upgrade().map(Node)
    }

    /// Whether both are handles to one node.
    pub fn ptr_eq(&self, other: &WeakNode) -> bool {
        self.0.ptr_eq(&other.0)
    }
}

impl Drop for NodeInner {
    fn drop(&mut self) {
        // What follows from the tree changes when an element leaves.
        mark_dirty();
    }
}

/// Orders two nodes the way their elements sit in the page
/// (`documentOrder`). Nodes not in the page sort last.
pub fn document_order(a: &Node, b: &Node) -> Ordering {
    match (a.is_connected(), b.is_connected()) {
        (true, true) => a.order().cmp(&b.order()),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => Ordering::Equal,
    }
}

// A node in the page, with where it sits.
pub(crate) struct Placed {
    pub(crate) node: Node,
    path: Rc<[ElementId]>,
    order: u64,
}

// Every node in the page, in page order.
pub(crate) fn placed() -> Vec<Placed> {
    let nodes: Vec<Node> = DOCUMENT.with(|document| {
        let mut document = document.borrow_mut();
        document.nodes.retain(|node| node.strong_count() > 0);
        document.nodes.iter().filter_map(|node| node.upgrade().map(Node)).collect()
    });
    let mut placed: Vec<Placed> = nodes
        .into_iter()
        .filter_map(|node| {
            let (path, order) = {
                let state = node.0.state.borrow();
                let place = state.place.as_ref().filter(|_| state.connected)?;
                (place.path.clone(), place.order)
            };
            Some(Placed { node, path, order })
        })
        .collect();
    placed.sort_by_key(|placed| placed.order);
    placed
}

// The connected nodes of a kind, in page order.
pub(crate) fn placed_of(kind: NodeKind) -> Vec<Node> {
    placed()
        .into_iter()
        .filter(|placed| placed.node.0.kind == kind)
        .map(|placed| placed.node)
        .collect()
}

// The element that puts a node in the tree: empty, filling its parent, and
// out of the layout flow.
struct Tracker {
    node: Node,
}

// Held in the tracker's element state, which gpui keeps while the element
// draws, and drops once a frame draws without it.
struct Token {
    node: Weak<NodeInner>,
    id: u64,
}

impl Drop for Token {
    fn drop(&mut self) {
        let Some(node) = self.node.upgrade() else {
            return;
        };
        let Ok(mut state) = node.state.try_borrow_mut() else {
            return;
        };
        // A node that moved holds another token now.
        if state.token == self.id && state.connected {
            state.connected = false;
            drop(state);
            mark_dirty();
        }
    }
}

impl Element for Tracker {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::NamedInteger("ui-sdk-forms".into(), self.node.0.key))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style {
            position: Position::Absolute,
            inset: Edges {
                top: px(0.).into(),
                right: px(0.).into(),
                bottom: px(0.).into(),
                left: px(0.).into(),
            },
            size: Size {
                width: relative(1.).into(),
                height: relative(1.).into(),
            },
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let Some(id) = id else {
            return;
        };
        let node = &self.node;
        let token = window.with_element_state(id, |held: Option<Token>, _| {
            let token = held.unwrap_or_else(|| Token {
                node: Rc::downgrade(&node.0),
                id: next_id(|document| &mut document.next_token),
            });
            (token.id, token)
        });
        // The path to the tracker's parent, the element the node stands for.
        let path = &id[..id.len().saturating_sub(1)];
        record(node, path, bounds, token, window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

fn record(
    node: &Node,
    path: &[ElementId],
    bounds: Bounds<Pixels>,
    token: u64,
    window: &mut Window,
    cx: &mut App,
) {
    let order = next_id(|document| &mut document.next_order);
    let moved = {
        let mut state = node.0.state.borrow_mut();
        let moved = !state.connected
            || state.place.as_ref().is_none_or(|place| &*place.path != path);
        let path = match state.place.take() {
            Some(place) if !moved => place.path,
            _ => Rc::from(path),
        };
        state.place = Some(Place {
            path,
            order,
            window: window.window_handle(),
            bounds,
        });
        state.connected = true;
        state.token = token;
        moved
    };
    if moved {
        mark_dirty();
    }
    // An element that stopped drawing drops its token at the end of the
    // frame, so the check after it hears that too.
    queue_check(cx);
}

// Works out again what follows from the tree, and tells whoever reads it.
fn settle(cx: &mut App) {
    let dirty = DOCUMENT.with(|document| std::mem::take(&mut document.borrow_mut().dirty));
    let nodes: Vec<Node> = DOCUMENT.with(|document| {
        let mut document = document.borrow_mut();
        document.nodes.retain(|node| node.strong_count() > 0);
        document.nodes.iter().filter_map(|node| node.upgrade().map(Node)).collect()
    });

    let mut disabled_changes: Vec<(Node, bool)> = Vec::new();
    let mut notify: Vec<EntityId> = Vec::new();
    if dirty {
        let placed = placed();
        let mut derived: std::collections::HashMap<u64, Derived> = derive(&placed)
            .into_iter()
            .map(|(node, derived)| (node.0.key, derived))
            .collect();
        for node in &nodes {
            // A node not in the page has nothing around it.
            let next = derived.remove(&node.0.key).unwrap_or_default();
            let mut state = node.0.state.borrow_mut();
            if state.derived == next {
                continue;
            }
            if state.derived.form_disabled != next.form_disabled {
                disabled_changes.push((node.clone(), next.form_disabled));
            }
            state.derived = next;
            if let Some(observer) = state.observer {
                notify.push(observer);
            }
        }
    }

    for (node, disabled) in disabled_changes {
        let mut callbacks: Vec<DisabledCallback> = node
            .0
            .state
            .borrow()
            .watchers
            .iter()
            .map(|(_, callback)| callback.clone())
            .collect();
        if let Some(control) = &node.0.control {
            callbacks.extend(control.borrow().disabled_callbacks.iter().cloned());
        }
        if callbacks.is_empty() {
            continue;
        }
        let Some(window) = node.window() else {
            continue;
        };
        window
            .update(cx, |_, window, cx| {
                for callback in callbacks {
                    callback(disabled, window, cx);
                }
            })
            .ok();
    }

    let changed: Vec<Node> = nodes
        .iter()
        .filter(|node| std::mem::take(&mut node.0.state.borrow_mut().validity_changed))
        .cloned()
        .collect();
    if !changed.is_empty() {
        let listeners: Vec<ValidityListener> = DOCUMENT.with(|document| {
            document
                .borrow()
                .validity_listeners
                .iter()
                .map(|(_, listener)| listener.clone())
                .collect()
        });
        for node in changed {
            let Some(window) = node.window() else {
                continue;
            };
            let listeners = listeners.clone();
            window
                .update(cx, |_, window, cx| {
                    for listener in &listeners {
                        listener(&node, window, cx);
                    }
                })
                .ok();
        }
    }

    notify.sort_unstable();
    notify.dedup();
    for entity in notify {
        cx.notify(entity);
    }
}

// What follows from the tree for every node in the page.
fn derive(placed: &[Placed]) -> Vec<(Node, Derived)> {
    let of = |kind: NodeKind| -> Vec<&Placed> {
        placed.iter().filter(|each| each.node.0.kind == kind).collect()
    };
    let fieldsets = of(NodeKind::Fieldset);
    let legends = of(NodeKind::Legend);
    let forms = of(NodeKind::Form);
    let labels = of(NodeKind::Label);
    let controls: Vec<&Placed> = placed.iter().filter(|each| is_control(&each.node)).collect();

    // A fieldset's first legend keeps the controls in it enabled. A legend
    // belongs to the nearest fieldset around it.
    let disabling: Vec<(&Placed, Option<&Placed>)> = fieldsets
        .iter()
        .copied()
        .filter(|fieldset| fieldset.node.fieldset_disabled_flag())
        .map(|fieldset| {
            let legend = legends.iter().copied().find(|legend| {
                nearest(&fieldsets, legend).is_some_and(|nearest| nearest.node == fieldset.node)
            });
            (fieldset, legend)
        })
        .collect();

    // A label's control: the element its `for` names, or the label itself,
    // when it's a control, else the first control inside it.
    let label_controls: Vec<(&Placed, Option<&Placed>)> = labels
        .iter()
        .copied()
        .map(|label| {
            let target = match label.node.for_id() {
                Some(id) => placed
                    .iter()
                    .find(|each| each.node.effective_id().as_ref() == Some(&id)),
                None => Some(label),
            };
            let control = target.and_then(|target| {
                if is_control(&target.node) {
                    Some(target)
                } else {
                    controls.iter().copied().find(|control| inside(target, control))
                }
            });
            (label, control)
        })
        .collect();

    placed
        .iter()
        .map(|each| {
            let form_disabled = disabling.iter().any(|&(fieldset, legend)| {
                inside(fieldset, each) && !legend.is_some_and(|legend| within(legend, each))
            });
            let mut derived = Derived {
                form_disabled,
                owner: nearest(&forms, each).map(|form| form.node.downgrade()),
                ..Derived::default()
            };
            match each.node.0.kind {
                NodeKind::Control => {
                    let attribute = each
                        .node
                        .0
                        .control
                        .as_ref()
                        .and_then(|control| control.borrow().form_attribute.clone());
                    // A control that names a form belongs to it, or to none
                    // when no form has that id.
                    if let Some(id) = attribute {
                        derived.owner = forms
                            .iter()
                            .find(|form| form.node.id().as_ref() == Some(&id))
                            .map(|form| form.node.downgrade());
                    }
                    let naming: Vec<&Placed> = label_controls
                        .iter()
                        .filter(|(_, control)| control.is_some_and(|control| control.node == each.node))
                        .map(|&(label, _)| label)
                        .collect();
                    derived.labels = naming.iter().map(|label| label.node.downgrade()).collect();
                    let text: Vec<SharedString> = naming
                        .iter()
                        .map(|label| label.node.text())
                        .filter(|text| !text.trim().is_empty())
                        .collect();
                    derived.name = (!text.is_empty()).then(|| text.join(" ").into());
                }
                NodeKind::Label => {
                    derived.control = label_controls
                        .iter()
                        .find(|(label, _)| label.node == each.node)
                        .and_then(|(_, control)| control.map(|control| control.node.downgrade()));
                }
                _ => {}
            }
            (each.node.clone(), derived)
        })
        .collect()
}

// Whether `inner`'s element is inside `outer`'s.
fn inside(outer: &Placed, inner: &Placed) -> bool {
    inner.path.len() > outer.path.len() && inner.path.starts_with(&outer.path)
}

// Whether `inner`'s element is `outer`'s or inside it.
fn within(outer: &Placed, inner: &Placed) -> bool {
    inner.path.starts_with(&outer.path)
}

// The nearest of `candidates` around `inner`.
fn nearest<'a>(candidates: &[&'a Placed], inner: &Placed) -> Option<&'a Placed> {
    candidates
        .iter()
        .copied()
        .filter(|candidate| inside(candidate, inner))
        .max_by_key(|candidate| candidate.path.len())
}

// Whether a label looking for its control stops at this node.
pub(crate) fn is_control(node: &Node) -> bool {
    node.0
        .control
        .as_ref()
        .is_some_and(|control| control.borrow().control)
}

/// Calls `listener` with a control whenever its validity changes in a way no
/// event reports, once the frame has drawn (`onValidityChange`). Dropping the
/// subscription stops it.
pub fn on_validity_change(listener: impl Fn(&Node, &mut Window, &mut App) + 'static) -> Subscription {
    let id = next_id(|document| &mut document.next_listener);
    DOCUMENT.with(|document| {
        document
            .borrow_mut()
            .validity_listeners
            .push((id, Rc::new(listener)))
    });
    Subscription::new(move || {
        DOCUMENT
            .try_with(|document| {
                if let Ok(mut document) = document.try_borrow_mut() {
                    document.validity_listeners.retain(|(each, _)| *each != id);
                }
            })
            .ok();
    })
}

/// The events form code sends between elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FormEventType {
    /// A control's value changed as the user changed it.
    Input,
    /// A user's change to a control's value is done.
    Change,
    /// A submit or a validity check found the control invalid. It doesn't
    /// travel back out from its target.
    Invalid,
    /// A form is about to submit.
    Submit,
    /// A form is about to reset.
    Reset,
}

impl FormEventType {
    fn bubbles(self) -> bool {
        !matches!(self, FormEventType::Invalid)
    }
}

/// One event on its way through the tree, with whatever canceled it.
pub struct FormEvent {
    kind: FormEventType,
    target: Node,
    submitter: Option<Node>,
    input: Option<InputEvent>,
    canceled: Cell<bool>,
    stopped: Cell<bool>,
    pub(crate) after: RefCell<Vec<AfterReset>>,
}

// What `after_reset` queued on a `Reset`.
pub(crate) type AfterReset = Box<dyn FnOnce(&mut Window, &mut App)>;

impl FormEvent {
    pub(crate) fn new(kind: FormEventType, target: Node) -> Self {
        Self {
            kind,
            target,
            submitter: None,
            input: None,
            canceled: Cell::new(false),
            stopped: Cell::new(false),
            after: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn with_submitter(mut self, submitter: Option<Node>) -> Self {
        self.submitter = submitter;
        self
    }

    pub(crate) fn with_input(mut self, input: Option<InputEvent>) -> Self {
        self.input = input;
        self
    }

    /// What kind of event it is.
    pub fn kind(&self) -> FormEventType {
        self.kind
    }

    /// The node it's aimed at: the control for `Input`, `Change` and
    /// `Invalid`, the form for `Submit` and `Reset`.
    pub fn target(&self) -> &Node {
        &self.target
    }

    /// The button a submit came from, if one did.
    pub fn submitter(&self) -> Option<&Node> {
        self.submitter.as_ref()
    }

    /// The gpui event behind it, when one is.
    pub fn input(&self) -> Option<&InputEvent> {
        self.input.as_ref()
    }

    /// Cancels what the event would do: an `Invalid` keeps the control from
    /// being reported, a `Submit` or `Reset` stops the form.
    pub fn prevent_default(&self) {
        self.canceled.set(true);
    }

    /// Whether a listener canceled it.
    pub fn default_prevented(&self) -> bool {
        self.canceled.get()
    }

    /// Stops the event from reaching listeners after this one.
    pub fn stop_propagation(&self) {
        self.stopped.set(true);
    }
}

/// What hears a form event.
pub type FormListener = Rc<dyn Fn(&FormEvent, &mut Window, &mut App)>;

#[derive(Clone)]
struct Listener {
    id: u64,
    kind: FormEventType,
    capture: bool,
    callback: FormListener,
}

/// Calls `listener` for each event of `kind` anywhere in the page, like a
/// listener on the document. Dropping the subscription stops it.
pub fn listen_document(
    kind: FormEventType,
    capture: bool,
    listener: impl Fn(&FormEvent, &mut Window, &mut App) + 'static,
) -> Subscription {
    let id = next_id(|document| &mut document.next_listener);
    DOCUMENT.with(|document| {
        document.borrow_mut().listeners.push(Listener {
            id,
            kind,
            capture,
            callback: Rc::new(listener),
        })
    });
    Subscription::new(move || {
        DOCUMENT
            .try_with(|document| {
                if let Ok(mut document) = document.try_borrow_mut() {
                    document.listeners.retain(|listener| listener.id != id);
                }
            })
            .ok();
    })
}

// Sends an event the way the DOM does: in from the document to the target
// for listeners that capture, to the target, then back out for the rest.
pub(crate) fn dispatch(event: &FormEvent, window: &mut Window, cx: &mut App) {
    let kind = event.kind;
    let of = |listeners: &[Listener], capture: Option<bool>| -> Vec<FormListener> {
        listeners
            .iter()
            .filter(|listener| listener.kind == kind && capture.is_none_or(|capture| listener.capture == capture))
            .map(|listener| listener.callback.clone())
            .collect()
    };
    let document: Vec<Listener> = DOCUMENT.with(|document| document.borrow().listeners.clone());
    let ancestors = event.target.ancestors();
    let mut chain: Vec<FormListener> = of(&document, Some(true));
    for node in ancestors.iter().rev() {
        chain.extend(of(&node.0.state.borrow().listeners, Some(true)));
    }
    chain.extend(of(&event.target.0.state.borrow().listeners, None));
    if kind.bubbles() {
        for node in &ancestors {
            chain.extend(of(&node.0.state.borrow().listeners, Some(false)));
        }
        chain.extend(of(&document, Some(false)));
    }
    for listener in chain {
        if event.stopped.get() {
            break;
        }
        listener(event, window, cx);
    }
}
