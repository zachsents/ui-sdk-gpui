//! Port of `components/resizable/resizable.ts`: panels a person resizes,
//! collapses and keeps the layout of, by dragging the handle between them
//! or with the keyboard.
//!
//! The group's state and what it does are `resizable_api.rs`'s; finding its
//! panels and handles and pressing them, `resizable_dom.rs`'s and
//! `resizable_handles.rs`'s; the layout rules, `resizable_layout.rs`'s. Here
//! are the options, the parts and what each part's state holds.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, FocusHandle, InteractiveElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, px, relative,
};
use ui_sdk_drag_and_drop::{DragSession, create_drag_session};
use ui_sdk_forms::{Node, watch_fieldsets, watch_form_disabled};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentArgs, AgentError, AgentState, AgentValue, Aria, ChangeCallback, ChangeDetails,
    ChangeOptions, CommitCallback, Controllable, Derived, InputEvent, Instance, Part, Reason,
};
use ui_sdk_platform::Region;

use crate::resizable_api::{Drag, HandleValue, Keys, Settings, ShownInputs, Waiting, settings};
use crate::resizable_dom::Member;
use crate::resizable_handles;
use crate::resizable_layout::{PanelConstraints, PanelSizes, is_collapsed, same_size, to_length};
use crate::resizable_storage::ResizableStorage;

/// The direction panels are laid out in. Horizontal panels sit side by
/// side, vertical ones stack.
pub type ResizableOrientation = gpui::Orientation;

/// Why a group's layout wants to change: a handle was dragged (`Drag`), an
/// arrow, Home or End moved one (`StepKey`), Enter or a double-click on a
/// handle collapsed or restored the panel beside it (`Collapse`), the group
/// changed size, so panels sized in pixels moved to stay within their bounds
/// (`Resize`), or a call (`Imperative`).
pub type ResizableLayoutChangeReason = Reason;

/// What `on_layout_change` receives after the layout.
pub type ResizableLayoutChangeDetails = ChangeDetails;

/// What `on_layout_commit` receives after the layout.
pub type ResizableLayoutCommitDetails = ChangeDetails;

/// Options for every write method.
pub type ResizableChangeOptions = ChangeOptions;

/// The props the `panel` part takes beside its id (`ResizablePanelProps`):
/// its sizes and whether it collapses.
pub type ResizablePanelProps = PanelSizes;

/// A panel, by its id or its index, for the methods that take one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResizablePanelRef {
    Id(SharedString),
    Index(usize),
}

impl From<usize> for ResizablePanelRef {
    fn from(index: usize) -> Self {
        Self::Index(index)
    }
}

impl From<&str> for ResizablePanelRef {
    fn from(id: &str) -> Self {
        Self::Id(SharedString::from(id.to_string()))
    }
}

impl From<SharedString> for ResizablePanelRef {
    fn from(id: SharedString) -> Self {
        Self::Id(id)
    }
}

impl From<&SharedString> for ResizablePanelRef {
    fn from(id: &SharedString) -> Self {
        Self::Id(id.clone())
    }
}

/// The props the `handle` part takes beside its key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResizableHandleProps {
    /// Stops this handle: no drags and no keys, and it leaves the tab order.
    pub disabled: bool,
}

impl ResizableHandleProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The options `Resizable` takes.
#[derive(Clone)]
pub struct ResizableOptions {
    /// Each panel's size in percent, in panel order, like `vec![25.0, 75.0]`.
    /// Pass it to control the group.
    pub layout: Option<Vec<f64>>,
    /// The layout at first, when uncontrolled.
    pub default_layout: Option<Vec<f64>>,
    /// Runs while the layout changes: on every frame of a drag and every key.
    /// `details.reason` says why. It runs once the group's update is over, so
    /// it may update any entity, the group too.
    pub on_layout_change: Option<ChangeCallback<Vec<f64>>>,
    /// Runs as the layout is asked to change, before it does, inside the
    /// group's update: `details.cancel()` keeps the layout. It may update any
    /// entity but the group.
    pub on_before_layout_change: Option<ChangeCallback<Vec<f64>>>,
    /// Runs once an interaction that changed the layout ends: the pointer
    /// lifts, the key comes up, or a panel collapses. For saving. It runs once
    /// the group's update is over, so it may update any entity, the group
    /// too.
    pub on_layout_commit: Option<CommitCallback<Vec<f64>>>,
    /// The direction panels are laid out in. Defaults to horizontal.
    pub orientation: ResizableOrientation,
    /// Stops every handle: no drags, no keys, and handles leave the tab
    /// order.
    pub disabled: bool,
    /// How far an arrow key moves a handle, in percent. Defaults to 1.
    pub step: f64,
    /// How far Shift with an arrow key moves a handle, in percent. Defaults
    /// to 10.
    pub large_step: f64,
    /// Saves the layout when an interaction completes and restores it the
    /// next time the group's panels join, under this key. Give panels ids
    /// that last, so it survives changes elsewhere.
    pub auto_save_id: Option<SharedString>,
    /// Where `auto_save_id` saves: anything with `get_item` and `set_item`,
    /// like session storage. Defaults to local storage, which the app
    /// names with `ui_sdk_storage::init`.
    pub storage: Option<Rc<dyn ResizableStorage>>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for ResizableOptions {
    fn default() -> Self {
        Self {
            layout: None,
            default_layout: None,
            on_layout_change: None,
            on_before_layout_change: None,
            on_layout_commit: None,
            orientation: ResizableOrientation::Horizontal,
            disabled: false,
            step: 1.0,
            large_step: 10.0,
            auto_save_id: None,
            storage: None,
            debug: false,
        }
    }
}

impl ResizableOptions {
    pub fn layout(mut self, layout: Vec<f64>) -> Self {
        self.layout = Some(layout);
        self
    }

    pub fn default_layout(mut self, layout: Vec<f64>) -> Self {
        self.default_layout = Some(layout);
        self
    }

    pub fn on_layout_change(
        mut self,
        callback: impl Fn(&Vec<f64>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_layout_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_layout_change(
        mut self,
        callback: impl Fn(&Vec<f64>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_layout_change = Some(Rc::new(callback));
        self
    }

    pub fn on_layout_commit(
        mut self,
        callback: impl Fn(&Vec<f64>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_layout_commit = Some(Rc::new(callback));
        self
    }

    pub fn orientation(mut self, orientation: ResizableOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    pub fn large_step(mut self, large_step: f64) -> Self {
        self.large_step = large_step;
        self
    }

    pub fn auto_save_id(mut self, id: impl Into<SharedString>) -> Self {
        self.auto_save_id = Some(id.into());
        self
    }

    pub fn storage(mut self, storage: impl ResizableStorage + 'static) -> Self {
        self.storage = Some(Rc::new(storage));
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Debug, PartialEq)]
pub struct ResizableState {
    /// The direction panels are laid out in.
    pub orientation: ResizableOrientation,
    /// Whether the group is disabled, by itself or by a fieldset.
    pub disabled: bool,
    /// Whether a handle is being dragged.
    pub dragging: bool,
    /// Each panel's size in percent.
    pub layout: Vec<f64>,
}

/// What a panel's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizablePanelState {
    /// The direction panels are laid out in.
    pub orientation: ResizableOrientation,
    /// The panel's size in percent (`--resizable-panel-size`).
    pub size: f64,
    /// Whether the panel is down at its `collapsed_size`, below its minimum.
    pub collapsed: bool,
    /// Whether the panel can collapse.
    pub collapsible: bool,
    /// Whether it's collapsed to nothing, so nothing in it should take focus
    /// or be read (`inert`). It's hidden from assistive technology; gpui
    /// can't make its content unfocusable, so leave focusable content out
    /// while it's set.
    pub inert: bool,
}

/// What a handle's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizableHandleState {
    /// The direction panels are laid out in. The separator itself runs
    /// across it.
    pub orientation: ResizableOrientation,
    /// Whether the handle can't move.
    pub disabled: bool,
    /// Whether the handle is being dragged.
    pub dragging: bool,
    /// Whether the pointer is over the handle or within its hit area.
    pub hovering: bool,
}

/// The keys a handle handles, along the group's axis.
const HORIZONTAL_KEYS: &[&str] = &[
    "ArrowLeft",
    "ArrowRight",
    "shift+ArrowLeft",
    "shift+ArrowRight",
    "Home",
    "End",
    "Enter",
];

const VERTICAL_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "shift+ArrowUp",
    "shift+ArrowDown",
    "Home",
    "End",
    "Enter",
];

/// A group of resizable panels (`resizable`). Its entity is its
/// controller.
pub struct Resizable {
    pub(crate) instance: Instance,
    // Each panel's size in percent, in panel order. A controlled group shows
    // the layout it was passed once it fits the panels there are.
    pub(crate) layout: Controllable<Vec<f64>>,
    // The `layout` option, applied once it fits the panels there are.
    pub(crate) controlled_layout: Option<Vec<f64>>,
    pub(crate) default_layout: Option<Vec<f64>>,
    pub(crate) auto_save_id: Option<SharedString>,
    pub(crate) storage: Option<Rc<dyn ResizableStorage>>,
    pub(crate) settings: Settings,
    // The panels' ids, in order, and what each allows.
    pub(crate) ids: Vec<SharedString>,
    pub(crate) constraints: Vec<PanelConstraints>,
    // Which panel each handle follows, by the handle's key; -1 for none.
    pub(crate) handles: HashMap<SharedString, isize>,
    // The handle under the pointer, or within its hit area.
    pub(crate) hovering: Option<SharedString>,
    // The handle being dragged.
    pub(crate) dragging: Option<SharedString>,
    // The pixels panels share, as last measured above zero.
    pub(crate) space: f64,
    // The size props panels passed, in order.
    pub(crate) sizes: Vec<PanelSizes>,
    pub(crate) drag: Option<Drag>,
    pub(crate) keys: Option<Keys>,
    // The size each panel had before it last collapsed, by id, for
    // expanding.
    pub(crate) expanded: HashMap<SharedString, f64>,
    // The last layout a change asked for. A controlled group gets it back
    // when its owner passes it, which can be after the interaction completes.
    pub(crate) requested: Option<Vec<f64>>,
    pub(crate) waiting: Option<Waiting>,
    // What handles show, worked out once per change to what it's worked out
    // from.
    pub(crate) shown: RefCell<Derived<ShownInputs, HashMap<SharedString, HandleValue>>>,
    // The pixels panels share, as last measured, zero included.
    pub(crate) measured: f64,
    // The panels and handles as they rendered, in page order, and as the
    // group last reconciled them.
    pub(crate) members: RefCell<Vec<Member>>,
    pub(crate) reconciled: Vec<Member>,
    pub(crate) drags: DragSession<Self>,
    // A drag being stopped some other way than by its pointer.
    pub(crate) stopping: bool,
    // The group's element, which a press must land in.
    pub(crate) root_region: Region,
    // The fieldsets around the group, which disable it.
    pub(crate) form_node: Node,
    pub(crate) handle_focus: RefCell<HashMap<SharedString, FocusHandle>>,
    _subscriptions: Vec<Subscription>,
}

impl Resizable {
    pub fn new(options: ResizableOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = settings(&options);
        let mut layout = Controllable::new("layout", Vec::new());
        if options.layout.is_some() {
            layout.set_controlled(Some(Vec::new()));
        }
        layout.set_on_change(options.on_layout_change.clone());
        layout.set_on_before_change(options.on_before_layout_change.clone());
        layout.set_on_commit(options.on_layout_commit.clone());
        let mut instance = Instance::new("resizable").with_agent(cx);
        instance.debug = options.debug;
        let form_node = watch_form_disabled(cx);
        let fieldsets = {
            let entity = cx.weak_entity();
            watch_fieldsets(&form_node, move |_disabled, window, cx| {
                entity
                    .update(cx, |group, cx| {
                        group.end_if_disabled(window, cx);
                        cx.notify();
                    })
                    .ok();
            })
        };
        Self {
            instance,
            layout,
            controlled_layout: options.layout,
            default_layout: options.default_layout,
            auto_save_id: options.auto_save_id,
            storage: options.storage,
            settings,
            ids: Vec::new(),
            constraints: Vec::new(),
            handles: HashMap::new(),
            hovering: None,
            dragging: None,
            space: 0.0,
            sizes: Vec::new(),
            drag: None,
            keys: None,
            expanded: HashMap::new(),
            requested: None,
            waiting: None,
            shown: RefCell::new(Derived::new()),
            measured: 0.0,
            members: RefCell::new(Vec::new()),
            reconciled: Vec::new(),
            drags: create_drag_session(cx),
            stopping: false,
            root_region: Region::new(),
            form_node,
            handle_focus: RefCell::new(HashMap::new()),
            _subscriptions: vec![fieldsets],
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A controlled layout applies once it fits the panels there
    /// are.
    pub fn set_options(&mut self, options: ResizableOptions, window: &mut Window, cx: &mut Context<Self>) {
        let settings = settings(&options);
        let changed = self.settings != settings;
        self.settings = settings;
        self.layout.set_on_change(options.on_layout_change);
        self.layout.set_on_before_change(options.on_before_layout_change);
        self.layout.set_on_commit(options.on_layout_commit);
        self.default_layout = options.default_layout;
        self.auto_save_id = options.auto_save_id;
        self.storage = options.storage;
        self.instance.debug = options.debug;
        self.set_controlled_layout(options.layout, window, cx);
        self.end_if_disabled(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled layout, or `None` to let the group keep its
    /// own. It applies once it fits the panels there are.
    pub fn set_controlled_layout(
        &mut self,
        layout: Option<Vec<f64>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = self.layout.get().clone();
        let before = shown.clone();
        self.layout.set_controlled(layout.as_ref().map(|_| shown));
        self.controlled_layout = layout;
        self.apply_controlled();
        if &before != self.layout.get() {
            cx.notify();
        }
        self.heard_layout(window, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// What the root part's state holds.
    pub fn state(&self) -> ResizableState {
        ResizableState {
            orientation: self.settings.orientation,
            disabled: self.is_disabled(),
            dragging: self.dragging.is_some(),
            layout: self.layout.get().clone(),
        }
    }

    /// The panels' ids, in order, as the group last found them.
    pub fn panel_ids(&self) -> &[SharedString] {
        &self.ids
    }

    // The panel's place in the layout, while it has one (`panelIndex`).
    fn panel_index(&self, id: &SharedString) -> Option<usize> {
        self.ids
            .iter()
            .position(|each| each == id)
            .filter(|index| *index < self.layout.get().len())
    }

    /// The state the panel `id` renders with.
    pub fn panel_state(&self, id: &SharedString, props: &ResizablePanelProps) -> ResizablePanelState {
        let index = self.panel_index(id);
        let size = index.map_or(0.0, |index| self.layout.get()[index]);
        let collapsed = index
            .and_then(|index| self.constraints.get(index))
            .is_some_and(|constraints| is_collapsed(constraints, size));
        ResizablePanelState {
            orientation: self.settings.orientation,
            size,
            collapsed,
            collapsible: props.collapsible,
            // Nothing shows, so nothing in it should take focus or be read.
            inert: index.is_some() && collapsed && same_size(size, 0.0),
        }
    }

    /// The state the handle `key` renders with.
    pub fn handle_state(&self, key: &SharedString, props: &ResizableHandleProps) -> ResizableHandleState {
        ResizableHandleState {
            orientation: self.settings.orientation,
            disabled: self.is_disabled() || props.disabled,
            dragging: self.dragging.as_ref() == Some(key),
            hovering: self.hovering.as_ref() == Some(key),
        }
    }

    fn handle_focus_handle(&self, key: &SharedString, cx: &App) -> FocusHandle {
        self.handle_focus
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// The `root` part, around the panels and handles: it lays the panels out
    /// in a row or a column, so give it a size. Build it before them: it
    /// starts their render, so the order they render in is page order. It
    /// follows the pointer for every group in its window, so render it on
    /// every frame.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<ResizableState> {
        let group = this.read(cx);
        group.members.borrow_mut().clear();
        let state = group.state();
        let vertical = group.vertical();
        let entity = this.downgrade();
        let part = Part::new(&group.instance, "root", None, state)
            .map_el(|el| group.form_node.track(el))
            .map_el(|el| group.root_region.measure(el))
            .child(resizable_handles::listener(this))
            .child(group.drags.listener())
            .flex()
            .on_props(move |window, cx| {
                entity.update(cx, |group, cx| group.settle(window, cx)).ok();
            });
        if vertical {
            part.flex_col()
        } else {
            part.flex_row()
        }
    }

    /// The `panel` part with `id`, which identifies it for the methods that
    /// take one, `aria-controls` and saved layouts. Before the group lays its
    /// panels out, it takes its `default_size` as its basis, and one without
    /// grows into what's left; after, its share of the layout.
    pub fn panel(
        this: &Entity<Self>,
        id: impl Into<SharedString>,
        props: ResizablePanelProps,
        cx: &App,
    ) -> Part<ResizablePanelState> {
        let id = id.into();
        let group = this.read(cx);
        group.members.borrow_mut().push(Member::Panel(id.clone(), props));
        let state = group.panel_state(&id, &props);
        let sized = props.default_size.is_some();
        // Shares add up to 100%, so the handles' size comes off every panel in
        // proportion, and a panel's share of what panels have is exactly its
        // size.
        let basis = match group.panel_index(&id) {
            Some(_) => relative((state.size / 100.0) as f32).into(),
            None => props.default_size.map_or_else(|| px(0.).into(), to_length),
        };
        Part::new(&group.instance, "panel", Some(&id), state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(state.inert))
            .flex_grow(if sized { 0. } else { 1. })
            .flex_shrink(1.)
            .flex_basis(basis)
            .min_w_0()
            .min_h_0()
    }

    /// The `handle` part with `key`, between two panels: a focusable
    /// separator once the group lays its panels out, whose value is the size
    /// of the panel before it. Name it with `aria_label`, like "Resize
    /// files". It can stay a thin line: its hit area is at least 10 pixels
    /// across, centered on it, and 24 for touch.
    pub fn handle(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: ResizableHandleProps,
        cx: &App,
    ) -> Part<ResizableHandleState> {
        let key = key.into();
        let group = this.read(cx);
        group
            .members
            .borrow_mut()
            .push(Member::Handle(key.clone(), props));
        let state = group.handle_state(&key, &props);
        let value = group.handle_value(&key);
        // A focusable separator needs a value, so it joins the tab order only
        // once the group lays its panels out.
        let focusable = value.as_ref().is_some_and(|value| !value.disabled) && !group.is_disabled();
        let focus = group.handle_focus_handle(&key, cx);
        // It runs across the group, so a horizontal group's separator is
        // vertical.
        let across = if group.vertical() {
            ResizableOrientation::Horizontal
        } else {
            ResizableOrientation::Vertical
        };
        let keys = if group.vertical() {
            VERTICAL_KEYS
        } else {
            HORIZONTAL_KEYS
        };
        let mut part = Part::new(&group.instance, "handle", Some(&key), state)
            .aria_role(Role::Separator)
            .aria_orientation(across)
            .aria(Aria::new().disabled(state.disabled))
            .flex_none();
        if let Some(value) = &value {
            part = part
                .focus_mode(FocusMode::TabStop, focusable.then_some(&focus), false)
                .aria_numeric_value(value.now.round())
                .aria_min_numeric_value(value.min.round())
                .aria_max_numeric_value(value.max.round())
                .aria(Aria::new().controls(group.instance.element_id("panel", Some(&value.panel))));
        }
        let downs = this.downgrade();
        let ups = this.downgrade();
        let clicks = this.downgrade();
        let pressed = key.clone();
        let clicked = key.clone();
        // A press never focuses the handle: focus stays where the user was
        // working, and the window's listener drags it.
        part.on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_keys(keys, move |event, window, cx| {
                downs
                    .update(cx, |group, cx| group.keydown(event, &pressed, window, cx))
                    .ok();
            })
            .on_key_up(move |event, window, cx| {
                ups.update(cx, |group, cx| {
                    group.commit_keys(Some(InputEvent::from(event)), window, cx)
                })
                .ok();
            })
            .on_click(move |event, window, cx| {
                if event.click_count() != 2 {
                    return;
                }
                clicks
                    .update(cx, |group, cx| {
                        group.toggle(&clicked, Some(InputEvent::from(event)), window, cx)
                    })
                    .ok();
            })
    }
}

// A list of percentages, or one, from an agent's argument.
fn layout_arg(args: &AgentArgs) -> Result<Vec<f64>, AgentError> {
    let takes = || AgentError::Takes {
        action: args.action().to_string().into(),
        expected: "a list of numbers",
        given: args.value(0).clone(),
    };
    args.list(0)
        .map_err(|_| takes())?
        .iter()
        .map(|size| size.trim().parse::<f64>().ok().filter(|size| !size.is_nan()))
        .collect::<Option<Vec<f64>>>()
        .ok_or_else(takes)
}

// A panel by its id or its index, from an agent's argument.
fn panel_arg(args: &AgentArgs) -> Result<ResizablePanelRef, AgentError> {
    match args.value(0) {
        AgentValue::Number(number) => number
            .as_u64()
            .map(|index| ResizablePanelRef::Index(index as usize))
            .ok_or_else(|| AgentError::refused("takes a panel's id or index")),
        _ => Ok(ResizablePanelRef::Id(args.string(0)?)),
    }
}

/// `agent: { actions: ["setLayout", "collapse", "expand"] }`, with
/// `controlled` and `commits` both `["layout"]`.
impl Agent for Resizable {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setLayout", |group, args, window, cx| {
            let layout = layout_arg(args)?;
            Ok(group
                .set_layout(layout, ChangeOptions::default(), window, cx)
                .into())
        }),
        AgentAction::new("collapse", |group, args, window, cx| {
            let panel = panel_arg(args)?;
            Ok(group.collapse(panel, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("expand", |group, args, window, cx| {
            let panel = panel_arg(args)?;
            Ok(group.expand(panel, ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["layout"]);

    const COMMITS: &'static [&'static str] = &["layout"];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("layout", self.get_layout())
            .with("disabled", self.is_disabled())
            .with("step", self.settings.step)
    }
}
