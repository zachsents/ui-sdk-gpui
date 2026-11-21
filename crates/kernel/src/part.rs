//! Port of `foundation/kernel/part.ts` and the slot half of `primitive.ts`.
//!
//! A slot becomes a method on the component that returns a [`Part`]: a
//! `Stateful<Div>` with the slot's role, ARIA, focus behavior and handlers
//! already on it, and the slot's state beside it. The app styles it and adds
//! children like any div; [`Part::with`] styles it from state, where the
//! original's CSS targets `data-slot` and state attributes.
//!
//! A part says what its slot definition said: its role
//! ([`Part::aria_role`]), how it takes focus ([`Part::focus_mode`]), its press
//! ([`Part::on_press`]), the keys it handles ([`Part::on_keys`]), the needs
//! another element meets ([`Part::unneeded`]), and the ARIA gpui's setters
//! leave out ([`Part::aria`]). Development builds check them against the
//! role, once per slot (`checkRoles`, `checkRole`, `checkFocus`). What a
//! slot decides from state found only as the part lays out, its presence,
//! ARIA and style, it decides then ([`Part::on_layout`]).
//!
//! The original reads a bound element back: its `aria-label` and
//! `aria-labelledby` ([`Part::keep_name`], [`name_of`]), and `el.click()`
//! presses it from code ([`press_of`], [`PartPress::click`]).
//!
//! A part renders as a [`PartElement`], which delegates to the div and, as
//! gpui builds the accessibility tree, writes the kernel's ARIA to the part's
//! accesskit node. Relations name other parts by element id. Each part
//! records its accesskit node id, which gpui derives from its
//! `GlobalElementId`, in a registry per window while it's in the page, and a
//! relation resolves through it. That's the original's id following
//! (`followId`, `takeOwnId`, aliases): a part with an app's own element id
//! still answers to the one its slot makes. A part in the page is one gpui
//! drew this frame or keeps from the last; it leaves when gpui drops its
//! element state, which is the original's `bind` and its cleanup.
//!
//! A relation is written only to a node in this frame's tree, since
//! assistive technology reads its target and accesskit panics on one that
//! isn't there. Relations are written once the part's own subtree has
//! prepainted, as gpui asks for its synthetic children, so a target is
//! either in the tree already (a label before the part, or anything inside
//! it) or laid out this frame and still to prepaint, like a trigger's popup
//! in a deferred layer. gpui prepaints every element it laid out except
//! those inside one that doesn't display, which lay out empty, so a target
//! laid out this frame with a size is one gpui adds to the tree before the
//! frame ends. A node from an earlier frame is never written. A target that
//! first appears after the part pointing at it draws one more frame so the
//! relation resolves.
//!
//! accesskit computes no description from described-by, so the kernel does,
//! as browsers do from the targets' text, for a part with no description of
//! its own: each target in this frame's tree gives its name as accesskit
//! reads a labelled-by target's, a `Label`'s value or another role's label.
//! Text inside a target that isn't its own name, like `text!` children,
//! isn't read, so a description part takes its text as its `aria_label`. A
//! target gpui writes after the part gives the name it had the frame
//! before, and the part draws again when that changed.
//!
//! For the agent bridge, a part keeps its press, which the bridge's `press`
//! runs as a click with no pointer behind it, and the agent marks an app puts
//! on it ([`Part::agent_marks`], [`with_agent_marks`]); while a watcher is
//! installed, every part and every element given ARIA or marks records
//! itself in [`page`] as it lays out.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::{Rc, Weak};

use accesskit::{Action as AccessAction, Node, NodeId, Role as AccessRole};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    AnyElement, App, Bounds, ClickEvent, Display, Div, Element, ElementId, EntityId, FocusHandle,
    Global, GlobalElementId, InspectorElementId, InteractiveElement, Interactivity, IntoElement,
    KeyDownEvent, LayoutId, ParentElement, Pixels, Role, SharedString, Stateful,
    StatefulInteractiveElement, Style, StyleRefinement, Styled, Window, WindowId, div,
};

use super::attrs::{AgentMarks, Aria};
use super::details::{InputEvent, current_source, with_optional_source};
use super::dev::{self, Declared, Origin};
use super::events;
use super::focus_events;
use super::page::{self, Recorded};
use super::primitive::{Instance, Shared};
use super::roles::{self, HandledKey, Need};
use super::watchers;

// What runs after a frame renders a part (`onProps`).
type PropsListener = Box<dyn FnOnce(&mut Window, &mut App)>;

type PressHandler = Rc<dyn Fn(InputEvent, &mut Window, &mut App)>;

// What decides, as a part lays out, what only its layout knows.
type LayoutDecide = Box<dyn FnMut(&mut PartLayout<'_>, &mut Window, &mut App)>;

// The same, on the element a part renders as: its style, ARIA and presence.
type LayoutHook<E> = Box<dyn FnMut(&mut E, &mut Option<Aria>, &mut bool, &mut Window, &mut App)>;

/// A part's press (`press` on a slot), kept so the agent bridge, or the
/// component itself, can press the part as `el.click()` does in the
/// original: a click with no pointer or key behind it.
#[derive(Clone)]
pub struct PartPress(PressHandler);

impl PartPress {
    /// Runs the press as a click with no pointer or key behind it, the way
    /// assistive technology presses a part (`InputEvent::Accessibility`).
    /// Call it outside the component's own update; wrap it in
    /// `with_source(ChangeSource::Agent, ..)` for an agent's press.
    pub fn press(&self, window: &mut Window, cx: &mut App) {
        (self.0)(InputEvent::Accessibility, window, cx)
    }

    /// Clicks the part from code, as `el.click()` does, like Enter following
    /// a link item: the press runs with `event`, what asked for the click,
    /// once the update running now is over, so a component can click its
    /// own part. It reports the source that event and the change running
    /// now give it (`resolveSource`).
    pub fn click(&self, event: InputEvent, window: &mut Window, cx: &mut App) {
        let press = self.0.clone();
        let cause = current_source();
        window.defer(cx, move |window, cx| {
            with_optional_source(cause, || press(event, window, cx))
        });
    }
}

impl fmt::Debug for PartPress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PartPress")
    }
}

/// One rendered part: its element and the state its slot exposes.
pub struct Part<S = ()> {
    el: Stateful<Div>,
    state: S,
    slot: String,
    record: Box<PartRecord>,
    aria: Option<Aria>,
    marks: Option<AgentMarks>,
    on_props: Vec<PropsListener>,
    on_layout: Vec<LayoutDecide>,
}

// What a part keeps beside its element: who it belongs to and what its slot
// declares.
struct PartRecord {
    instance: Rc<Shared>,
    component: SharedString,
    slot: &'static str,
    key: Option<SharedString>,
    made_id: ElementId,
    role: Option<roles::Role>,
    declared: Declared,
    active_descendant: bool,
    // The focus handle its element tracks, when it takes focus itself.
    focus: Option<FocusHandle>,
    keys: &'static [HandledKey],
    press: Option<PressHandler>,
    input_type: Option<SharedString>,
    autocomplete: Option<SharedString>,
    // Whether it keeps what names it for its component (`keep_name`).
    keep_name: bool,
}

impl<S> Part<S> {
    /// A part for `slot` of `instance`, with `key` for parts there are many of.
    pub fn new(instance: &Instance, slot: &'static str, key: Option<&str>, state: S) -> Self {
        let made_id = instance.element_id(slot, key);
        Self {
            el: div().id(made_id.clone()),
            state,
            slot: instance.slot_name(slot),
            record: Box::new(PartRecord {
                instance: instance.shared().clone(),
                component: instance.label().clone(),
                slot,
                key: key.map(|key| SharedString::from(key.to_string())),
                made_id,
                role: None,
                declared: Declared::default(),
                active_descendant: false,
                focus: None,
                keys: &[],
                press: None,
                input_type: None,
                autocomplete: None,
                keep_name: false,
            }),
            aria: None,
            marks: None,
            on_props: Vec::new(),
            on_layout: Vec::new(),
        }
    }

    /// A part with an id the app wrote, which wins over the one its slot
    /// makes. Relations that name the made one still point at it.
    pub fn with_element_id(mut self, id: impl Into<ElementId>) -> Self {
        let interactivity = self.el.interactivity();
        interactivity.element_id = Some(id.into());
        self
    }

    /// The slot's state, what `render` functions read in the original (`partState`).
    pub fn state(&self) -> &S {
        &self.state
    }

    /// The slot's name, the original's `data-slot`, like `"tabs-trigger"`.
    pub fn slot(&self) -> &str {
        &self.slot
    }

    /// The part's key among its slot's, its `id` or `value` in the original.
    pub fn key(&self) -> Option<&SharedString> {
        self.record.key.as_ref()
    }

    /// The element id its slot makes, which relations name.
    pub fn element_id(&self) -> &ElementId {
        &self.record.made_id
    }

    /// The WAI-ARIA role it declared with `aria_role`.
    pub fn declared_role(&self) -> Option<roles::Role> {
        self.record.role
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &S) -> Stateful<Div>) -> Self {
        self.el = style(self.el, &self.state);
        self
    }

    /// Maps the element, like adding behavior a component composes in.
    pub fn map_el(mut self, f: impl FnOnce(Stateful<Div>) -> Stateful<Div>) -> Self {
        self.el = f(self.el);
        self
    }

    /// The element and the state. The element leaves the part's kernel ARIA
    /// and registration behind; render the part itself to keep them.
    pub fn into_element_parts(self) -> (Stateful<Div>, S) {
        (self.el, self.state)
    }

    /// The part with another state, made from its own, and everything else
    /// kept, like a component's item state over the option a behavior
    /// builds.
    pub fn map_state<S2>(self, map: impl FnOnce(S) -> S2) -> Part<S2> {
        Part {
            el: self.el,
            state: map(self.state),
            slot: self.slot,
            record: self.record,
            aria: self.aria,
            marks: self.marks,
            on_props: self.on_props,
            on_layout: self.on_layout,
        }
    }

    /// The part's WAI-ARIA role (`role` on a slot). Sets the accesskit role
    /// gpui reports, and development builds check what the role needs.
    pub fn aria_role(mut self, role: roles::Role) -> Self {
        self.record.role = Some(role);
        if let Some(access) = role.accesskit() {
            self.el = self.el.role(access);
        }
        self
    }

    /// How the part takes focus (`focus` on a slot), with the handle its
    /// element tracks. `disabled_unfocusable` takes a `Native` part out of
    /// the tab order, as a disabled native control is.
    pub fn focus_mode(mut self, mode: FocusMode, handle: Option<&FocusHandle>, disabled_unfocusable: bool) -> Self {
        self.el = apply_focus(self.el, handle, mode, disabled_unfocusable);
        self.record.declared.focus = true;
        self.record.active_descendant = mode == FocusMode::ActiveDescendant;
        self.record.focus = handle.filter(|_| mode != FocusMode::ActiveDescendant).cloned();
        self
    }

    /// The part's activation (`press` on a slot): a click, a tap, Enter or
    /// Space on the focused part, or assistive technology's click. The agent
    /// bridge's `press` runs it too.
    pub fn on_press(mut self, handler: impl Fn(InputEvent, &mut Window, &mut App) + 'static) -> Self {
        let handler: PressHandler = Rc::new(handler);
        self.record.press = Some(match self.record.press.take() {
            // A second press runs after the first, as its click listener does.
            Some(first) => {
                let second = handler.clone();
                Rc::new(move |event: InputEvent, window: &mut Window, cx: &mut App| {
                    first(event.clone(), window, cx);
                    second(event, window, cx);
                })
            }
            None => handler.clone(),
        });
        self.el = press_with(self.el, handler);
        self.record.declared.press = true;
        self
    }

    /// The keys the part handles (`keys` and `on.keydown` on a slot), by the
    /// original's names, with the handler. Keys an input method owns while
    /// it composes text never reach it. An app that handles a key first
    /// listens with `capture_key_down` and stops propagation.
    pub fn on_keys(
        mut self,
        keys: &'static [HandledKey],
        handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.el = self.el.on_key_down(move |event, window, cx| {
            if events::is_composing(window, cx) {
                return;
            }
            handler(event, window, cx)
        });
        self.record.declared.keys = true;
        self.record.keys = keys;
        self
    }

    /// The keys `on_keys` declared.
    pub fn handled_keys(&self) -> &'static [HandledKey] {
        self.record.keys
    }

    /// A need the role has that another element meets, with why
    /// (`{ none: "why" }`), like `Need::Keys` on a combobox's list, whose
    /// input handles them.
    pub fn unneeded(mut self, need: Need, _why: &'static str) -> Self {
        match need {
            Need::Focus => self.record.declared.focus_unneeded = true,
            Need::Press => self.record.declared.press_unneeded = true,
            Need::Keys | Need::KeyDown => self.record.declared.keys_unneeded = true,
        }
        self
    }

    /// Adds ARIA beyond gpui's setters. A later call wins where it sets
    /// something, and its labelled-by replaces an earlier one, as an
    /// author's own label does.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.aria = Some(match self.aria.take() {
            Some(own) => own.merge(aria),
            None => aria,
        });
        self
    }

    /// The ARIA `aria` put on it.
    pub fn aria_extras(&self) -> Option<&Aria> {
        self.aria.as_ref()
    }

    /// Marks the part, and everything inside it, for the agent bridge: a
    /// change a person must make, a region agents stay out of, or values that
    /// never reach the model. A later call adds to an earlier one.
    pub fn agent_marks(mut self, marks: AgentMarks) -> Self {
        self.marks = Some(match self.marks.take() {
            Some(own) => own.merge(marks),
            None => marks,
        });
        self
    }

    /// The `type` and `autocomplete` of the native control the part is, as an
    /// `<input>` carries them, like `"password"` or `"one-time-code"`. The
    /// agent bridge reads them, as the original reads the element's
    /// attributes: a password, a card number or a one-time code never
    /// reaches the model.
    pub fn input_attrs(mut self, input_type: Option<&str>, autocomplete: Option<&str>) -> Self {
        self.record.input_type = input_type.map(|value| SharedString::from(value.to_string()));
        self.record.autocomplete = autocomplete.map(|value| SharedString::from(value.to_string()));
        self
    }

    /// Keeps what names the part as it renders, its `aria_label` as gpui
    /// writes it and its labelled-by, for its component to read back with
    /// [`name_of`], as the original reads the element's `aria-label` and
    /// `aria-labelledby`. It costs a node written each frame the part lays
    /// out, so a component asks for the parts it reads.
    pub fn keep_name(mut self) -> Self {
        self.record.keep_name = true;
        self
    }

    /// Runs `listener` after each frame that renders the part, with what it
    /// rendered in place (`onProps`). Work that depends on what a part
    /// renders with, like registering a shortcut, runs only while it asks.
    pub fn on_props(mut self, listener: impl FnOnce(&mut Window, &mut App) + 'static) -> Self {
        self.on_props.push(Box::new(listener));
        self
    }

    /// Decides, as the part lays out, what only its layout knows, like
    /// whether an image another part loads has loaded: the original's
    /// `present` and `attrs` for state found then. `decide` runs before
    /// anything reads the part's ARIA, and may restyle it, add ARIA or leave
    /// it out of the page ([`PartLayout`]). A later call runs after an
    /// earlier one. It's kept with the element until the frame is done.
    pub fn on_layout(mut self, decide: impl FnMut(&mut PartLayout<'_>, &mut Window, &mut App) + 'static) -> Self {
        self.on_layout.push(Box::new(decide));
        self
    }
}

/// What a part decides as it lays out ([`Part::on_layout`]): its style and
/// ARIA, and whether it's in the page.
pub struct PartLayout<'a> {
    style: &'a mut StyleRefinement,
    aria: &'a mut Option<Aria>,
    present: &'a mut bool,
}

impl PartLayout<'_> {
    /// The part's style, which goes on over the app's, as an inline style
    /// wins over classes.
    pub fn style(&mut self) -> &mut StyleRefinement {
        self.style
    }

    /// Adds ARIA, as `Part::aria` does.
    pub fn aria(&mut self, aria: Aria) {
        *self.aria = Some(match self.aria.take() {
            Some(own) => own.merge(aria),
            None => aria,
        });
    }

    /// Whether the part is in the page this frame (`present`). One that
    /// isn't lays out as nothing, isn't bound and has no accessibility node,
    /// as the original doesn't render it.
    pub fn present(&mut self, present: bool) {
        *self.present = present;
    }
}

impl<S> Styled for Part<S> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.el.style()
    }
}

impl<S> InteractiveElement for Part<S> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.el.interactivity()
    }
}

impl<S> StatefulInteractiveElement for Part<S> {}

impl<S> ParentElement for Part<S> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.el.extend(elements)
    }
}

impl<S: 'static> IntoElement for Part<S> {
    type Element = PartElement;

    fn into_element(self) -> Self::Element {
        let layout = self
            .on_layout
            .into_iter()
            .map(|mut decide| -> LayoutHook<Stateful<Div>> {
                Box::new(move |el, aria, present, window, cx| {
                    let mut layout = PartLayout {
                        style: el.style(),
                        aria,
                        present,
                    };
                    decide(&mut layout, window, cx)
                })
            })
            .collect();
        PartElement {
            element: self.el,
            record: Some(self.record),
            aria: self.aria,
            marks: self.marks,
            on_props: self.on_props,
            layout,
            present: true,
            parts: None,
            bound: None,
            registered: None,
            relations: None,
            description: None,
        }
    }
}

/// How a part with a focusable role takes focus (`focus` on a slot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusMode {
    /// One member of a group holds the tab stop; arrow keys move it.
    Roving { tab_stop: bool },
    /// Always in the tab order.
    TabStop,
    /// Never focused itself; the container that keeps focus points at it,
    /// with `Aria::active_descendant` or gpui's `aria_active_descendant()`
    /// on the item.
    ActiveDescendant,
    /// Focusable as a native control is: in the tab order unless disabled.
    Native,
}

/// Puts a slot's focus behavior on an element.
pub fn apply_focus(
    el: Stateful<Div>,
    handle: Option<&FocusHandle>,
    mode: FocusMode,
    disabled_unfocusable: bool,
) -> Stateful<Div> {
    let Some(handle) = handle else {
        return el;
    };
    match mode {
        FocusMode::Roving { tab_stop } => el.track_focus(&handle.clone().tab_stop(tab_stop)),
        FocusMode::TabStop => el.track_focus(&handle.clone().tab_stop(true)),
        FocusMode::Native => el.track_focus(&handle.clone().tab_stop(!disabled_unfocusable)),
        FocusMode::ActiveDescendant => el,
    }
}

/// `press` on a slot: a click, a tap, or Enter or Space on the focused part,
/// which gpui turns into a keyboard click, or assistive technology's click,
/// which reports `ChangeSource::Virtual`. A tap, which gpui hands over as a
/// mouse click, comes as the touch click it was (`InputEvent::click`).
pub fn on_press(
    el: Stateful<Div>,
    handler: impl Fn(InputEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    press_with(el, Rc::new(handler))
}

fn press_with(el: Stateful<Div>, handler: PressHandler) -> Stateful<Div> {
    let clicked = handler.clone();
    el.on_click(move |event: &ClickEvent, window, cx| clicked(InputEvent::click(event, window), window, cx))
        .on_a11y_action(AccessAction::Click, move |_, window, cx| {
            handler(InputEvent::Accessibility, window, cx)
        })
}

// A `Label` node names what it labels through its value, not its label, so
// a label part named with `aria_label` gives it as both.
fn name_label(node: &mut Node) {
    if node.role() == AccessRole::Label
        && node.value().is_none()
        && let Some(label) = node.label().map(str::to_owned)
    {
        node.set_value(label);
    }
}

/// Sets a part's role, which every part with ARIA needs to be reported.
pub fn with_role(el: Stateful<Div>, role: Role) -> Stateful<Div> {
    // gpui leaves generic containers out of the accessibility tree, and
    // asserts against setting one.
    if role == Role::GenericContainer {
        return el;
    }
    el.role(role)
}

/// Whether a label reference points only at ids ui-sdk made, like a
/// component's own label, rather than an app's (`madeLabel`).
pub fn made_label(ids: &[ElementId]) -> bool {
    ids.iter().all(|id| match id {
        ElementId::Name(name) => name.starts_with("ui-sdk-"),
        _ => false,
    })
}

/// Whether a part of `slot`, with `key`, is in the page (`held`).
pub fn held(instance: &Instance, slot: &str, key: Option<&str>) -> bool {
    instance
        .shared()
        .bound
        .borrow()
        .values()
        .any(|part| part.slot == slot && part.key.as_deref() == key)
}

/// What names a part as it rendered: its `aria_label`, as gpui writes it,
/// and the elements its labelled-by names, by element id
/// (`el.getAttribute("aria-label")` and `"aria-labelledby"`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartName {
    pub aria_label: Option<SharedString>,
    pub labelled_by: Vec<ElementId>,
}

impl PartName {
    /// Whether nothing names it, neither a label nor a labelled-by.
    pub fn is_empty(&self) -> bool {
        self.aria_label.is_none() && self.labelled_by.is_empty()
    }
}

/// What named the part of `slot`, with `key`, as it rendered last, for a
/// part that keeps it ([`Part::keep_name`]). `None` while it's not in the
/// page.
pub fn name_of(instance: &Instance, slot: &str, key: Option<&str>) -> Option<PartName> {
    instance
        .shared()
        .bound
        .borrow()
        .values()
        .find(|part| part.slot == slot && part.key.as_deref() == key)
        .and_then(|part| part.name.clone())
}

/// The press of the part of `slot`, with `key`, in the page, to click it
/// from code with [`PartPress::click`], like Enter on a link item
/// (`el.click()`).
pub fn press_of(instance: &Instance, slot: &str, key: Option<&str>) -> Option<PartPress> {
    instance
        .shared()
        .bound
        .borrow()
        .values()
        .find(|part| part.slot == slot && part.key.as_deref() == key)
        .and_then(|part| part.press.clone())
}

/// An element that isn't a part, with ARIA and relations as a part's, like
/// the ARIA a trigger menu writes on the app's editor (`aliasReferences`).
/// It joins the registry under its own element id, so parts can point at it.
pub fn with_aria<E: Element>(element: E, aria: Aria) -> PartElement<E> {
    PartElement::plain(element, Some(aria), None)
}

/// An element that isn't a part, marked for the agent bridge with
/// everything inside it, like a section agents stay out of
/// (`data-agent-hidden` on the app's own element). The element needs an id,
/// which places it in the page.
pub fn with_agent_marks<E: Element>(element: E, marks: AgentMarks) -> PartElement<E> {
    PartElement::plain(element, None, Some(marks))
}

/// The accesskit node id of the part or element with `id` in `window`, for a
/// relation a custom element writes by hand: only while it's in this
/// frame's tree, or laid out this frame to join it, as a part's own
/// relations resolve. Ask as the element prepaints, after its children
/// have, and write the relation in `a11y_synthetic_children`.
pub fn node_id_of(id: &ElementId, window: &mut Window, cx: &App) -> Option<NodeId> {
    let parts = cx
        .try_global::<Registries>()?
        .0
        .get(&window.window_handle().window_id())?
        .clone();
    let node = parts.borrow_mut().resolve(id, window);
    node
}

/// The accesskit node id gpui gives an element with `global_id`
/// (`GlobalElementId::accesskit_node_id`): the path's hash, which its `Hash`
/// writes.
pub fn node_id(global_id: &GlobalElementId) -> NodeId {
    struct Capture(u64);
    impl Hasher for Capture {
        fn write(&mut self, bytes: &[u8]) {
            for byte in bytes {
                self.0 = self.0.rotate_left(8) ^ u64::from(*byte);
            }
        }
        fn write_u64(&mut self, n: u64) {
            self.0 = n;
        }
        fn finish(&self) -> u64 {
            self.0
        }
    }
    let mut capture = Capture(0);
    global_id.hash(&mut capture);
    NodeId(capture.0)
}

/// A part in the page, as its instance, the watchers and the agent bridge
/// read it.
#[derive(Clone, Debug)]
pub struct BoundPart {
    pub window: WindowId,
    pub slot: &'static str,
    pub key: Option<SharedString>,
    /// The element id its slot makes, which relations name.
    pub element_id: ElementId,
    pub node: NodeId,
    /// Its element and every ancestor's with an id, from the window down.
    pub path: GlobalElementId,
    /// Its bounds as of its last layout.
    pub bounds: Option<Bounds<Pixels>>,
    /// Its accessibility as gpui and the kernel write it, its role, label,
    /// states and actions, kept while a watcher is installed.
    pub accessible: Option<Node>,
    /// Its press (`Part::on_press`), which the agent bridge runs.
    pub press: Option<PartPress>,
    /// The native `type` of the control it is, like `"password"`
    /// (`Part::input_attrs`).
    pub input_type: Option<SharedString>,
    /// The native `autocomplete` of the control it is, like
    /// `"one-time-code"`.
    pub autocomplete: Option<SharedString>,
    /// Whether gpui placed it after it last rendered. A part inside an
    /// element that doesn't display, like one with `hidden()`, renders but
    /// isn't placed (`checkVisibility`).
    pub shown: bool,
    /// What named it as it rendered, when it keeps it (`Part::keep_name`).
    pub name: Option<PartName>,
}

// The registries of every window, by window.
#[derive(Default)]
struct Registries(HashMap<WindowId, Rc<RefCell<WindowParts>>>);

impl Global for Registries {}

// Components whose watched parts left the page. A part leaves while gpui
// draws, where a notify schedules no frame, and possibly in a frame that draws
// no part at all, so a task tells them once the draw is over.
struct Departures(UnboundedSender<EntityId>);

impl Global for Departures {}

fn departures(cx: &mut App) -> UnboundedSender<EntityId> {
    if let Some(departures) = cx.try_global::<Departures>() {
        return departures.0.clone();
    }
    let (sender, mut receiver) = unbounded::<EntityId>();
    cx.spawn(async move |cx| {
        while let Some(entity) = receiver.next().await {
            cx.update(|cx| cx.notify(entity));
        }
    })
    .detach();
    cx.set_global(Departures(sender.clone()));
    sender
}

// One window's parts in the page, by element id, and the frame gpui draws.
struct WindowParts {
    frame: u64,
    drawing: bool,
    serial: u64,
    by_id: HashMap<ElementId, Entry>,
    // The nodes gpui added to this frame's accessibility tree so far.
    pushed: HashSet<NodeId>,
    // Ids a relation named this frame that nothing in the page had, with the
    // views of the parts that named them.
    missing: HashMap<ElementId, Vec<EntityId>>,
    // Ids whose name a description read this frame before gpui wrote their
    // node, with the views of the parts it describes.
    early: HashMap<ElementId, Vec<EntityId>>,
    // Views that draw again once the frame is done: a name they were
    // described with changed after they read it.
    redraw: Vec<EntityId>,
    departures: UnboundedSender<EntityId>,
}

struct Entry {
    serial: u64,
    node: NodeId,
    // The frame it last laid out in, and its layout then.
    frame: u64,
    layout: Option<LayoutId>,
    has_role: bool,
    // Its name as gpui last wrote its node, which a description reads: a
    // label's value, or another role's label.
    name: Option<String>,
}

impl WindowParts {
    fn new(departures: UnboundedSender<EntityId>) -> Self {
        Self {
            frame: 0,
            drawing: false,
            serial: 0,
            by_id: HashMap::default(),
            pushed: HashSet::default(),
            missing: HashMap::default(),
            early: HashMap::default(),
            redraw: Vec::new(),
            departures,
        }
    }

    // Notes the name gpui wrote for the element with `ids`. A part described
    // by it earlier this frame read the old one, and draws again if it
    // changed.
    fn named(&mut self, ids: &[ElementId], serial: u64, name: Option<&str>) {
        for id in ids {
            let early = self.early.remove(id);
            let Some(entry) = self.by_id.get_mut(id).filter(|entry| entry.serial == serial) else {
                continue;
            };
            if entry.name.as_deref() == name {
                continue;
            }
            entry.name = name.map(str::to_owned);
            for view in early.into_iter().flatten() {
                if !self.redraw.contains(&view) {
                    self.redraw.push(view);
                }
            }
        }
    }

    // What the targets in this frame's tree among `ids` describe a part
    // with: each one's name, as accesskit names a part by its labelled-by
    // targets, between spaces. A target gpui writes later this frame gives
    // the name it had last.
    fn description(
        &mut self,
        ids: &[ElementId],
        resolved: &HashMap<ElementId, NodeId>,
        view: EntityId,
    ) -> Option<String> {
        let mut description = String::new();
        for id in ids.iter().filter(|id| resolved.contains_key(*id)) {
            let Some(entry) = self.by_id.get(id) else {
                continue;
            };
            let early = !self.pushed.contains(&entry.node);
            if let Some(name) = entry.name.as_deref().filter(|name| !name.is_empty()) {
                if !description.is_empty() {
                    description.push(' ');
                }
                description.push_str(name);
            }
            if early {
                let views = self.early.entry(id.clone()).or_default();
                if !views.contains(&view) {
                    views.push(view);
                }
            }
        }
        (!description.is_empty()).then_some(description)
    }

    // The node of the element with `id` if this frame's tree has it, or
    // will by the end of the frame: one gpui added already, or one that laid
    // out this frame with a size, which gpui prepaints. An element inside one
    // that doesn't display lays out empty and never joins the tree.
    fn resolve(&mut self, id: &ElementId, window: &mut Window) -> Option<NodeId> {
        let Some(entry) = self.by_id.get(id) else {
            let view = window.current_view();
            let views = self.missing.entry(id.clone()).or_default();
            if !views.contains(&view) {
                views.push(view);
            }
            return None;
        };
        if !entry.has_role {
            return None;
        }
        if self.pushed.contains(&entry.node) {
            return Some(entry.node);
        }
        if entry.frame != self.frame {
            return None;
        }
        let node = entry.node;
        let size = window.layout_bounds(entry.layout?).size;
        (size.width > Pixels::ZERO || size.height > Pixels::ZERO).then_some(node)
    }
}

// The window's registry, and the frame it's drawing: the first part laid out
// in a draw starts the frame, and the end of the draw's update closes it.
fn window_parts(window: &mut Window, cx: &mut App) -> Rc<RefCell<WindowParts>> {
    let id = window.window_handle().window_id();
    let existing = cx
        .try_global::<Registries>()
        .and_then(|registries| registries.0.get(&id).cloned());
    let parts = match existing {
        Some(parts) => parts,
        None => {
            let parts = Rc::new(RefCell::new(WindowParts::new(departures(cx))));
            cx.default_global::<Registries>().0.insert(id, parts.clone());
            parts
        }
    };
    let opening = {
        let mut state = parts.borrow_mut();
        if state.drawing {
            false
        } else {
            state.drawing = true;
            state.frame += 1;
            state.pushed.clear();
            state.missing.clear();
            state.early.clear();
            true
        }
    };
    if opening {
        let closing = parts.clone();
        window.defer(cx, move |window, cx| {
            let redraw = {
                let mut state = closing.borrow_mut();
                state.drawing = false;
                std::mem::take(&mut state.redraw)
            };
            for view in redraw {
                cx.notify(view);
            }
            focus_events::frame_drawn(window, cx);
        });
    }
    parts
}

// A part's place in the page, kept as its element state: gpui drops it once
// a frame doesn't draw the part, which takes the part out of the registry.
struct Token {
    parts: Rc<RefCell<WindowParts>>,
    departures: UnboundedSender<EntityId>,
    instance: Option<Weak<Shared>>,
    ids: Vec<ElementId>,
    serial: u64,
    slot: &'static str,
    window: WindowId,
    path: GlobalElementId,
}

impl Drop for Token {
    fn drop(&mut self) {
        page::forget(self.window, &self.path, self.serial);
        let mut notify = None;
        if let Some(instance) = self.instance.as_ref().and_then(Weak::upgrade) {
            if let Ok(mut bound) = instance.bound.try_borrow_mut() {
                bound.remove(&self.serial);
            }
            if instance.watches(self.slot) {
                notify = instance.entity.get();
            }
        }
        if let Ok(mut parts) = self.parts.try_borrow_mut() {
            for id in &self.ids {
                if parts.by_id.get(id).is_some_and(|entry| entry.serial == self.serial) {
                    parts.by_id.remove(id);
                }
            }
        }
        if let Some(entity) = notify {
            self.departures.unbounded_send(entity).ok();
        }
    }
}

/// What a [`Part`] renders as: its div, which it delegates to, with the
/// kernel's ARIA written to its accesskit node and its place in the page
/// recorded. [`with_aria`] makes one of any element.
pub struct PartElement<E = Stateful<Div>> {
    element: E,
    record: Option<Box<PartRecord>>,
    aria: Option<Aria>,
    marks: Option<AgentMarks>,
    on_props: Vec<PropsListener>,
    // What it decides as it lays out (`Part::on_layout`), and whether that
    // left it in the page.
    layout: Vec<LayoutHook<E>>,
    present: bool,
    parts: Option<Rc<RefCell<WindowParts>>>,
    bound: Option<(Weak<Shared>, u64)>,
    // Its ids in the registry, the serial they hold and its node.
    registered: Option<(Vec<ElementId>, u64, NodeId)>,
    // Its relations' targets this frame, resolved once its subtree
    // prepainted, and the description its described-by targets give it.
    relations: Option<HashMap<ElementId, NodeId>>,
    description: Option<String>,
}

impl<E: Element> PartElement<E> {
    // An element that isn't a part, with ARIA or agent marks.
    fn plain(element: E, aria: Option<Aria>, marks: Option<AgentMarks>) -> Self {
        Self {
            element,
            record: None,
            aria,
            marks,
            on_props: Vec::new(),
            layout: Vec::new(),
            present: true,
            parts: None,
            bound: None,
            registered: None,
            relations: None,
            description: None,
        }
    }

    /// The wrapped element.
    pub fn inner(&mut self) -> &mut E {
        &mut self.element
    }

    /// Marks the element, and everything inside it, for the agent bridge,
    /// like `with_agent_marks`. A later call adds to an earlier one.
    pub fn agent_marks(mut self, marks: AgentMarks) -> Self {
        self.marks = Some(match self.marks.take() {
            Some(own) => own.merge(marks),
            None => marks,
        });
        self
    }

    fn role_of(&self) -> Option<AccessRole> {
        match self.record.as_ref().and_then(|record| record.role) {
            Some(role) => role.accesskit(),
            None => self.element.a11y_role(),
        }
    }

    // The node gpui would build for the part, with the kernel's ARIA and no
    // relations, for development checks and the watchers.
    fn accessible(&self) -> Node {
        let mut node = Node::new(self.role_of().unwrap_or(AccessRole::GenericContainer));
        self.element.write_a11y_info(&mut node);
        if let Some(aria) = &self.aria {
            aria.write(&mut node, &|_, _| None);
        }
        node
    }

    fn check(&self) {
        let Some(record) = &self.record else {
            return;
        };
        let Some(role) = record.role else {
            return;
        };
        let node = self.accessible();
        let (attributes, labelled) = match &self.aria {
            Some(aria) => (aria.role_attributes(), !aria.labelled_by.is_empty()),
            None => (Vec::new(), false),
        };
        let written = dev::written_of(role, &node, &attributes, labelled);
        let from = Origin::new(&record.component, &record.instance.id);
        dev::check_role(record.slot, role, &node, record.declared, &written, from);
        dev::check_focus(record.slot, record.declared.focus, record.active_descendant, &node, from);
    }

    // What names the part, for one that keeps it: the label gpui writes from
    // its element, and its labelled-by.
    fn name(&self) -> Option<PartName> {
        if !self.record.as_ref().is_some_and(|record| record.keep_name) {
            return None;
        }
        let mut node = Node::new(AccessRole::GenericContainer);
        self.element.write_a11y_info(&mut node);
        Some(PartName {
            aria_label: node.label().map(|label| SharedString::from(label.to_string())),
            labelled_by: self
                .aria
                .as_ref()
                .map(|aria| aria.labelled_by.clone())
                .unwrap_or_default(),
        })
    }

    fn bind(&mut self, global_id: &GlobalElementId, window: &mut Window, cx: &mut App) {
        let parts = window_parts(window, cx);
        let node = node_id(global_id);
        let has_role = self.role_of().is_some();
        let mut ids = Vec::new();
        if let Some(record) = &self.record {
            ids.push(record.made_id.clone());
        }
        if let Some(own) = self.element.id()
            && !ids.contains(&own)
        {
            ids.push(own);
        }
        let accessible = watchers::installed().then(|| self.accessible());
        let window_id = window.window_handle().window_id();
        let instance = self.record.as_ref().map(|record| Rc::downgrade(&record.instance));
        let slot = self.record.as_ref().map_or("", |record| record.slot);
        let key = self.record.as_ref().and_then(|record| record.key.clone());
        let made_id = self.record.as_ref().map(|record| record.made_id.clone());
        let press = self
            .record
            .as_ref()
            .and_then(|record| record.press.clone())
            .map(PartPress);
        let input_type = self.record.as_ref().and_then(|record| record.input_type.clone());
        let autocomplete = self.record.as_ref().and_then(|record| record.autocomplete.clone());
        let name = self.name();
        let frame = parts.borrow().frame;

        let (serial, fresh) = window.with_element_state::<Token, _>(global_id, |token, _window| {
            let (token, fresh) = match token {
                Some(token) if token.ids == ids => (token, false),
                stale => {
                    drop(stale);
                    let serial = {
                        let mut state = parts.borrow_mut();
                        state.serial += 1;
                        state.serial
                    };
                    let departures = parts.borrow().departures.clone();
                    let token = Token {
                        parts: parts.clone(),
                        departures,
                        instance: instance.clone(),
                        ids: ids.clone(),
                        serial,
                        slot,
                        window: window_id,
                        path: global_id.clone(),
                    };
                    (token, true)
                }
            };
            {
                let mut state = parts.borrow_mut();
                for id in &ids {
                    // The name gpui last wrote for it stays, for a part it
                    // describes that reads it before gpui writes it again.
                    let name = state
                        .by_id
                        .remove(id)
                        .filter(|entry| entry.serial == token.serial)
                        .and_then(|entry| entry.name);
                    state.by_id.insert(
                        id.clone(),
                        Entry {
                            serial: token.serial,
                            node,
                            frame,
                            layout: None,
                            has_role,
                            name,
                        },
                    );
                }
            }
            if let (Some(instance), Some(made_id)) =
                (instance.as_ref().and_then(Weak::upgrade), made_id.clone())
            {
                let mut bound = instance.bound.borrow_mut();
                let part = BoundPart {
                    window: window_id,
                    slot,
                    key: key.clone(),
                    element_id: made_id,
                    node,
                    path: global_id.clone(),
                    bounds: None,
                    accessible: accessible.clone(),
                    press: press.clone(),
                    input_type: input_type.clone(),
                    autocomplete: autocomplete.clone(),
                    shown: false,
                    name: name.clone(),
                };
                match bound.get_mut(&token.serial) {
                    Some(existing) => {
                        let bounds = existing.bounds;
                        *existing = part;
                        existing.bounds = bounds;
                    }
                    None => {
                        bound.insert(token.serial, part);
                    }
                }
            }
            ((token.serial, fresh), token)
        });

        if let Some(accessible) = accessible {
            page::record(Recorded {
                window: window_id,
                path: global_id.clone(),
                element_id: made_id.or_else(|| self.element.id()),
                role: self.role_of(),
                accessible,
                labelled_by: self
                    .aria
                    .as_ref()
                    .map(|aria| aria.labelled_by.clone())
                    .unwrap_or_default(),
                marks: self.marks.clone().unwrap_or_default(),
                serial,
            });
        }

        if fresh {
            // The parts that named it while it was missing draw again, now
            // that it's here.
            let views: Vec<EntityId> = {
                let mut state = parts.borrow_mut();
                ids.iter()
                    .filter_map(|id| state.missing.remove(id))
                    .flatten()
                    .collect()
            };
            if !views.is_empty() {
                window.on_next_frame(move |_window, cx| {
                    for view in views {
                        cx.notify(view);
                    }
                });
            }
            if let Some(instance) = instance.as_ref().and_then(Weak::upgrade) {
                let first = !instance.drawn.replace(true);
                let watched = instance.watches(slot) || (first && !instance.watched.borrow().is_empty());
                if watched && let Some(entity) = instance.entity.get() {
                    window.defer(cx, move |_window, cx| cx.notify(entity));
                }
            }
        }
        self.bound = instance.map(|instance| (instance, serial));
        self.registered = Some((ids, serial, node));
        self.parts = Some(parts);
    }

    // Its layout this frame, which tells whether gpui will add it to the
    // accessibility tree.
    fn laid_out(&self, layout: LayoutId) {
        let (Some(parts), Some((ids, serial, _))) = (&self.parts, &self.registered) else {
            return;
        };
        let mut parts = parts.borrow_mut();
        for id in ids {
            if let Some(entry) = parts.by_id.get_mut(id)
                && entry.serial == *serial
            {
                entry.layout = Some(layout);
            }
        }
    }

    // The node of each relation's target that's in this frame's tree, or
    // will be, once the part's own subtree prepainted, and the description
    // the described-by ones give.
    fn resolve_relations(&mut self, window: &mut Window) {
        self.relations = None;
        self.description = None;
        let (Some(aria), Some(parts)) = (&self.aria, &self.parts) else {
            return;
        };
        if !window.is_a11y_active() || !aria.has_relations() {
            return;
        }
        let targets = aria
            .labelled_by
            .iter()
            .chain(&aria.described_by)
            .chain(&aria.controls)
            .chain(&aria.owns)
            .chain(&aria.details)
            .chain(&aria.flow_to)
            .chain(&aria.active_descendant)
            .chain(&aria.error_message);
        let mut resolved = HashMap::default();
        let mut parts = parts.borrow_mut();
        for id in targets {
            if let Some(node) = parts.resolve(id, window) {
                resolved.insert(id.clone(), node);
            }
        }
        self.description = parts.description(&aria.described_by, &resolved, window.current_view());
        self.relations = Some(resolved);
    }
}

// The name a node gives the parts it labels or describes: a label's value,
// or another role's label, as accesskit reads a labelled-by target.
fn node_name(node: &Node) -> Option<&str> {
    if node.role() == AccessRole::Label {
        node.value()
    } else {
        node.label()
    }
}

impl<E: Element> Element for PartElement<E> {
    type RequestLayoutState = Option<E::RequestLayoutState>;
    type PrepaintState = Option<E::PrepaintState>;

    fn id(&self) -> Option<ElementId> {
        self.element.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.element.source_location()
    }

    fn a11y_role(&self) -> Option<AccessRole> {
        if !self.present {
            return None;
        }
        self.role_of()
    }

    fn write_a11y_info(&self, node: &mut Node) {
        self.element.write_a11y_info(node);
        if let Some(aria) = &self.aria {
            aria.write_states(node);
        }
        name_label(node);
        // gpui adds the node to this frame's tree as it asks for it.
        if let (Some(parts), Some((ids, serial, id))) = (&self.parts, &self.registered) {
            let mut parts = parts.borrow_mut();
            parts.pushed.insert(*id);
            parts.named(ids, *serial, node_name(node));
        }
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut gpui::A11ySubtreeBuilder,
    ) {
        let Some(prepaint) = prepaint else {
            return;
        };
        self.element.a11y_synthetic_children(prepaint, builder);
        // The relations, now that the part's subtree is in the tree.
        if let Some(aria) = &self.aria {
            let resolved = self.relations.take().unwrap_or_default();
            let node = builder.parent_node();
            aria.write_relations(node, &|id, _| resolved.get(id).copied());
            // accesskit computes no description from described-by, so the
            // targets' names describe it, unless it has a description of
            // its own.
            if let Some(description) = self.description.take()
                && node.description().is_none()
            {
                node.set_description(description);
            }
            name_label(node);
        }
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        for decide in &mut self.layout {
            decide(&mut self.element, &mut self.aria, &mut self.present, window, cx);
        }
        if !self.present {
            // Out of the page this frame: nothing lays out, binds or renders.
            self.on_props.clear();
            let none = Style {
                display: Display::None,
                ..Style::default()
            };
            return (window.request_layout(none, None, cx), None);
        }
        if let Some(global_id) = id {
            self.bind(global_id, window, cx);
        }
        if dev::DEV {
            self.check();
        }
        for listener in self.on_props.drain(..) {
            window.defer(cx, listener);
        }
        let (layout, state) = self.element.request_layout(id, inspector_id, window, cx);
        self.laid_out(layout);
        (layout, Some(state))
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let request_layout = request_layout.as_mut()?;
        if let Some((instance, serial)) = &self.bound
            && let Some(instance) = instance.upgrade()
            && let Some(part) = instance.bound.borrow_mut().get_mut(serial)
        {
            part.bounds = Some(bounds);
            part.shown = true;
        }
        // The view it draws in, which the component notifies as it changes.
        if let Some(record) = &self.record {
            record.instance.record_view(window.current_view());
        }
        let prepaint = self
            .element
            .prepaint(id, inspector_id, bounds, request_layout, window, cx);
        self.resolve_relations(window);
        Some(prepaint)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (Some(request_layout), Some(prepaint)) = (request_layout, prepaint) else {
            return;
        };
        // A key press goes to the focused element, which notes whether it
        // repeats (`key_held`).
        if let Some(record) = &self.record
            && record.focus.as_ref().is_some_and(|focus| focus.is_focused(window))
        {
            events::hear_key_presses(window, cx);
        }
        self.element
            .paint(id, inspector_id, bounds, request_layout, prepaint, window, cx)
    }
}

impl<E: Element + Styled> Styled for PartElement<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl<E: Element + InteractiveElement> InteractiveElement for PartElement<E> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl<E: Element + StatefulInteractiveElement> StatefulInteractiveElement for PartElement<E> {}

impl<E: Element + ParentElement> ParentElement for PartElement<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements)
    }
}
