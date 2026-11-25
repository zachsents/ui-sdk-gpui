//! Port of `components/navigation-menu/navigation-menu-api.ts`: how a
//! navigation menu opens, closes and moves focus.
//!
//! Every panel shows on a surface from the popup layer: the shared viewport
//! when there is one, or the content itself, positioned against its trigger,
//! when there isn't. Each surface is a stepped popup over one view of the
//! menu's value, so dismissal, Escape, focus return and Tab flow come from
//! the layer for both.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, Subscription, WeakEntity, Window, deferred,
};
use ui_sdk_disclosure::focus_pressed_trigger;
use ui_sdk_focus::{Orientation, orientation_of};
use ui_sdk_kernel::focus_events::on_focus_out;
use ui_sdk_kernel::modality::{restore_focus, window_source};
use ui_sdk_kernel::{
    ChangeOptions, Controllable, InputEvent, Instance, Part, Parts, Reason, flag_while_bound, held,
    key_while_bound,
};
use ui_sdk_motion::{Presence, PresenceState, PresenceTiming};
use ui_sdk_platform::{
    Align, Anchor, ElementBounds, LAYER_PRIORITY, PositionOptions, PositionProps, Region, Side,
    Target, listener, positioned,
};
use ui_sdk_popups::{
    AttachConfig, CloseDetails, HoverEvent, HoverIntent, HoverIntentOptions, HideRequest, Modal,
    OpenRules, OpenSteps, OpenStepsOptions, PlacementDefaults, PopupBehaviorName, PopupElement,
    PopupLayer, PopupLayerOptions, PopupOpener, ReturnMode, SafeArea, SafeAreaAxis, ShowRequest,
    content_anchor, create_hover_intent, create_safe_area, create_stepped_popup, modal_triple,
    opener_of, placement_options, register_popup_element, window_box,
};

use crate::navigation_menu::{
    NavigationMenuContentProps, NavigationMenuLinkProps, NavigationMenuOptions,
    NavigationMenuOrientation, NavigationMenuParent, NavigationMenuTriggerProps,
    NavigationMenuViewportProps, OpenChangeCompleteCallback,
};
use crate::navigation_menu_dom::{IndicatorProbe, ViewportSize};
use crate::navigation_menu_keyboard::opens_elsewhere;

// Opening again within this long of closing skips the delay, so the pointer
// can leave and come back without waiting.
const WARM_FOR: Duration = Duration::from_millis(300);

// What the surfaces' layers read without the menu, which may be mid-update
// when they ask: the open item, the triggers, the root and the placement
// props, as of the menu's last change or render.
pub(crate) struct Shared {
    pub(crate) value: RefCell<Option<SharedString>>,
    pub(crate) active: RefCell<Option<SharedString>>,
    pub(crate) orientation: Cell<Orientation>,
    pub(crate) triggers: RefCell<HashMap<SharedString, Region>>,
    pub(crate) root: Region,
    pub(crate) viewport_props: RefCell<NavigationMenuViewportProps>,
    pub(crate) content_props: RefCell<HashMap<SharedString, NavigationMenuContentProps>>,
}

impl Shared {
    // The item the menu shows now, or the one animating out.
    fn shown_item(&self) -> Option<SharedString> {
        self.value.borrow().clone().or_else(|| self.active.borrow().clone())
    }

    pub(crate) fn trigger_of(&self, key: Option<&SharedString>) -> Option<Region> {
        self.triggers.borrow().get(key?).cloned()
    }

    // Where a surface goes: against its anchor prop, or the trigger, on the
    // side the orientation suggests unless the props say otherwise.
    fn position(&self, props: &PositionProps) -> PositionOptions {
        let side = match self.orientation.get() {
            Orientation::Vertical => Side::End,
            Orientation::Horizontal => Side::Bottom,
        };
        placement_options(props, PlacementDefaults::new(Align::Start).side(Some(side)), None)
    }

    fn anchor(&self, props: &PositionProps, key: Option<&SharedString>) -> Option<Anchor> {
        content_anchor(props.anchor.clone(), || {
            let key = key.cloned().or_else(|| self.shown_item());
            self.trigger_of(key.as_ref()).map(Anchor::element)
        })
    }
}

// A surface: the popup layer and its open and close steps over one view of
// the value. The viewport's is open while any item is, and a content's
// while its item is.
#[derive(Clone)]
pub(crate) struct Surface {
    pub(crate) layer: PopupLayer<NavigationMenu>,
    pub(crate) steps: OpenSteps<NavigationMenu, ()>,
}

// The contents rendered into the viewport this frame, which it renders.
#[derive(Default)]
pub(crate) struct Stash(RefCell<Vec<(SharedString, AnyElement)>>);

impl Stash {
    pub(crate) fn push(&self, key: SharedString, element: AnyElement) {
        self.0.borrow_mut().push((key, element));
    }

    fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    fn take(&self) -> Vec<(SharedString, AnyElement)> {
        std::mem::take(&mut *self.0.borrow_mut())
    }
}

// What a surface's part renders through: above the window, placed by its
// layer once it shows, and out of sight until then, so it lays out for its
// first placement.
pub(crate) struct SurfaceView {
    layer: PopupLayer<NavigationMenu>,
    region: Region,
    intent: HoverIntent<NavigationMenu>,
    entity: WeakEntity<NavigationMenu>,
    // The viewport's: the contents rendered into it, and whether it has
    // joined the popup layer.
    stash: Option<Rc<Stash>>,
    attached: bool,
    // A content's: whether it's bound to its own popup.
    key: Option<SharedString>,
    bound: bool,
}

impl SurfaceView {
    // The viewport with the contents rendered into it this frame, after
    // the app's own children. The first of them shows it.
    pub(crate) fn render_viewport<S: 'static>(&self, part: Part<S>) -> AnyElement {
        let contents: Vec<AnyElement> = self
            .stash
            .as_ref()
            .map(|stash| stash.take().into_iter().map(|(_, element)| element).collect())
            .unwrap_or_default();
        let arrived = !contents.is_empty();
        self.render_with(part.children(contents), arrived)
    }

    // A content as its own popup.
    pub(crate) fn render<S: 'static>(&self, part: Part<S>) -> AnyElement {
        self.render_with(part, false)
    }

    fn render_with<S: 'static>(&self, part: Part<S>, contents: bool) -> AnyElement {
        let layer = self.layer.clone();
        // Focus moves in once the content has rendered, since tab order is
        // the frame's: the popup itself for touch, which keeps an on-screen
        // keyboard away, and nowhere otherwise, as the surfaces'
        // `initialFocus` says. The layer hears presses and the back gesture.
        let mut part = self
            .region
            .measure(part)
            .child(listener())
            .child(layer.listener())
            .child(self.intent.listener());
        if !layer.is_mounted() {
            return part.hidden().into_any_element();
        }
        // The viewport joins the layer once a content renders into it, so it
        // shows for the first time with that content's size, in its final
        // place. A content without one joins its own as it first renders.
        let entity = self.entity.clone();
        if self.stash.is_some() && !self.attached && contents {
            part = part.on_props(move |window, cx| {
                entity.update(cx, |menu, cx| menu.show_viewport(window, cx)).ok();
            });
        } else if let Some(key) = self.key.clone().filter(|_| !self.bound) {
            part = part.on_props(move |window, cx| {
                entity.update(cx, |menu, cx| menu.bind_content(key, window, cx)).ok();
            });
        }
        if !layer.is_shown() {
            // Mounted before it shows, it waits for its first placement out
            // of sight.
            part = part.opacity(0.);
        }
        let part = part.occlude();
        match layer.positioner() {
            Some(positioner) => {
                positioner.set_anchor(layer.anchor());
                if let Some(options) = layer.position_options() {
                    positioner.set_options(options);
                }
                deferred(positioned(&positioner, part))
                    .priority(LAYER_PRIORITY)
                    .into_any_element()
            }
            None => deferred(window_box(part)).priority(LAYER_PRIORITY).into_any_element(),
        }
    }
}

// A content in the viewport: its enter and exit as items switch, and
// whether it's leaving.
struct ViewportContent {
    presence: Presence,
    leaving: bool,
}

// A trigger or a link in the list, in page order.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ListEntry {
    Trigger(SharedString),
    Link(SharedString),
}

/// A navigation menu (`navigationMenu`). Its entity is its controller.
pub struct NavigationMenu {
    pub(crate) instance: Instance,
    pub(crate) value: Controllable<Option<SharedString>>,
    pub(crate) orientation: NavigationMenuOrientation,
    // The item that opened last. Kept after it closes, so focus goes back to
    // its trigger and its content stays in the viewport while that animates
    // out.
    pub(crate) active: Option<SharedString>,
    // The item open before the current one, and whether the current one
    // comes after it. `None` when the menu opened from closed.
    pub(crate) previous: Option<SharedString>,
    pub(crate) forward: Option<bool>,
    // Contents animating out of the viewport after another item took over.
    pub(crate) exiting: Vec<SharedString>,
    // Whether a viewport is in the page. Contents render into it.
    pub(crate) viewport: bool,
    // Whether this menu sits inside another one's content.
    pub(crate) nested: bool,
    pub(crate) on_open_change_complete: Option<OpenChangeCompleteCallback>,
    pub(crate) delay: Duration,
    pub(crate) close_delay: Duration,
    pub(crate) open_on_hover: bool,
    pub(crate) close_on_select: bool,
    pub(crate) parent: Option<NavigationMenuParent>,
    pub(crate) shared: Rc<Shared>,
    pub(crate) surfaces: HashMap<Option<SharedString>, Surface>,
    pub(crate) intent: HoverIntent<Self>,
    // Crossing other triggers on the way to the open panel doesn't switch
    // to them.
    pub(crate) aim: SafeArea,
    pub(crate) pointer_on: Option<SharedString>,
    closed_at: Option<Instant>,
    // The item the last open asked for, which the viewport opens for, and
    // whether the last open or close went through.
    asked: Option<SharedString>,
    changed: bool,
    // Whether `on_open_change_complete` last said the menu is open, and
    // whether the menu opened since it last said it closed.
    reported_open: bool,
    opened_since_close: bool,
    last_value: Option<SharedString>,
    // The triggers and links in the page, in order, with their props.
    pub(crate) triggers: Parts<SharedString, NavigationMenuTriggerProps>,
    pub(crate) links: Parts<SharedString, NavigationMenuLinkProps>,
    pub(crate) list_items: Parts<ListEntry, ()>,
    pub(crate) link_regions: RefCell<HashMap<SharedString, Region>>,
    pub(crate) content_regions: RefCell<HashMap<SharedString, (Region, PopupElement)>>,
    pub(crate) viewport_region: Region,
    _viewport_element: PopupElement,
    pub(crate) stash: Rc<Stash>,
    viewport_contents: HashMap<SharedString, ViewportContent>,
    pub(crate) viewport_size: Rc<ViewportSize>,
    pub(crate) indicator: Rc<IndicatorProbe>,
    // The top-level links in the page, which the indicator covers when
    // they're the current page, as of the last frame.
    pub(crate) top_links: RefCell<Vec<SharedString>>,
    viewport_attached: bool,
    bound_contents: HashSet<SharedString>,
    // An arrow key opened this item, and focus moves into its panel once it
    // renders.
    pub(crate) pending: Option<SharedString>,
    parent_seen: Option<SharedString>,
    parent_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl NavigationMenu {
    pub fn new(options: NavigationMenuOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut value = Controllable::new("value", options.default_value.clone()).after_update();
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());
        let mut instance = Instance::new("navigation-menu").with_agent(cx);
        instance.debug = options.debug;
        // The viewport and the contents coming and going render the menu
        // again, which binds or unbinds their surfaces.
        flag_while_bound(&instance, "viewport");
        key_while_bound(&instance, "content");
        let orientation = orientation_of(options.orientation, Orientation::Horizontal);
        let initial = value.get().clone();
        let root_focus = cx.focus_handle();
        let shared = Rc::new(Shared {
            value: RefCell::new(initial.clone()),
            active: RefCell::new(initial.clone()),
            orientation: Cell::new(orientation),
            triggers: RefCell::new(HashMap::new()),
            root: Region::with_focus_handle(root_focus.clone()),
            viewport_props: RefCell::new(NavigationMenuViewportProps::default()),
            content_props: RefCell::new(HashMap::new()),
        });
        let viewport_region = Region::with_focus_handle(cx.focus_handle());
        let viewport_element = register_popup_element(&viewport_region, instance.slot_name("viewport"));
        let intent = create_hover_intent(
            HoverIntentOptions {
                trigger: Rc::new(|menu: &Self| {
                    menu.shared
                        .trigger_of(menu.value.get().as_ref())
                        .and_then(|trigger| trigger.bounds())
                }),
                popup: Rc::new(|menu: &Self| menu.surface_element().and_then(|panel| panel.bounds())),
                closes_on_leave: Rc::new(|menu: &Self| {
                    menu.value.get().is_some() && menu.surface().layer.opener() == PopupOpener::Hover
                }),
                close_delay: Rc::new(|menu: &Self| menu.close_delay),
                close: Rc::new(|menu: &mut Self, event, window, cx| {
                    // Someone working inside the panel keeps it open.
                    if menu.surface_element().is_some_and(|panel| panel.contains_focused(window, cx)) {
                        return;
                    }
                    menu.close_with(Reason::TriggerHover, Some(event.clone()), ReturnMode::Leave, window, cx);
                }),
            },
            cx,
        );
        let this = cx.entity();
        let subscriptions = vec![cx.observe_in(&this, window, |menu: &mut Self, _, window, cx| {
            menu.check_bound(window, cx)
        }),
        // Focus leaving the navigation closes it, wherever it leaves from.
        on_focus_out(&root_focus, window, cx, |menu: &mut Self, _event, window, cx| {
            menu.focus_left(window, cx)
        })];
        // A nested menu closes once its parent moves on to another item.
        let parent_seen = options.parent.as_ref().and_then(|parent| parent.menu.read(cx).value.get().clone());
        let parent_subscription = options.parent.as_ref().map(|parent| Self::watch_parent(parent, window, cx));
        let owner = cx.entity_id();
        let open = initial.is_some();
        let mut menu = Self {
            instance,
            value,
            orientation,
            active: initial.clone(),
            previous: None,
            forward: None,
            exiting: Vec::new(),
            viewport: false,
            nested: options.parent.is_some(),
            on_open_change_complete: options.on_open_change_complete,
            delay: options.delay,
            close_delay: options.close_delay,
            open_on_hover: options.open_on_hover,
            close_on_select: options.close_on_select,
            parent: options.parent,
            shared,
            surfaces: HashMap::new(),
            intent,
            aim: create_safe_area(),
            pointer_on: None,
            closed_at: None,
            asked: None,
            changed: false,
            reported_open: open,
            opened_since_close: open,
            last_value: initial.clone(),
            triggers: Parts::new(),
            links: Parts::new(),
            list_items: Parts::new(),
            link_regions: RefCell::new(HashMap::new()),
            content_regions: RefCell::new(HashMap::new()),
            viewport_region,
            _viewport_element: viewport_element,
            stash: Rc::new(Stash::default()),
            viewport_contents: HashMap::new(),
            viewport_size: ViewportSize::new(owner),
            indicator: IndicatorProbe::new(owner),
            top_links: RefCell::new(Vec::new()),
            viewport_attached: false,
            bound_contents: HashSet::new(),
            pending: None,
            parent_seen,
            parent_subscription,
            _subscriptions: subscriptions,
        };
        let viewport = menu.create_surface(None, window, cx);
        menu.surfaces.insert(None, viewport);
        if let Some(key) = initial {
            menu.ensure_surface(key, window, cx);
        }
        menu
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A controlled `value` brings what it derives, and mounts its
    /// surface, in the same pass.
    pub fn set_options(&mut self, options: NavigationMenuOptions, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = self.value.set_controlled(options.value);
        self.value.set_on_change(options.on_value_change);
        self.value.set_on_before_change(options.on_before_value_change);
        let orientation = orientation_of(options.orientation, Orientation::Horizontal);
        changed |= self.orientation != orientation || self.open_on_hover != options.open_on_hover;
        self.orientation = orientation;
        self.shared.orientation.set(orientation);
        self.on_open_change_complete = options.on_open_change_complete;
        self.delay = options.delay;
        self.close_delay = options.close_delay;
        self.open_on_hover = options.open_on_hover;
        self.close_on_select = options.close_on_select;
        let parent_changed = self.parent.as_ref().map(|parent| (parent.menu.entity_id(), parent.item.clone()))
            != options.parent.as_ref().map(|parent| (parent.menu.entity_id(), parent.item.clone()));
        if parent_changed {
            self.parent_seen = options.parent.as_ref().and_then(|parent| parent.menu.read(cx).value.get().clone());
            self.parent_subscription = options.parent.as_ref().map(|parent| Self::watch_parent(parent, window, cx));
            self.nested = options.parent.is_some();
            changed = true;
        }
        self.parent = options.parent;
        self.instance.debug = options.debug;
        self.after_change(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, `Some(None)` to close every panel, or
    /// `None` to let the menu keep its own.
    pub fn set_controlled_value(
        &mut self,
        value: Option<Option<SharedString>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.value.set_controlled(value) {
            cx.notify();
        }
        self.after_change(window, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The open item's value, or `None` when every panel is closed.
    pub fn get_value(&self) -> Option<&SharedString> {
        self.value.get().as_ref()
    }

    /// Whether a panel is open.
    pub fn is_open(&self) -> bool {
        self.value.get().is_some()
    }

    /// Which way the top-level items run.
    pub fn orientation(&self) -> NavigationMenuOrientation {
        self.orientation
    }

    /// Opens an item's panel, or closes the open one with `None`. Returns
    /// false when canceled.
    pub fn set_value(
        &mut self,
        value: Option<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        match value {
            None => self.close_with(reason, options.event, ReturnMode::Return, window, cx),
            Some(value) => self.open(value, reason, options.event, PopupOpener::Imperative, window, cx),
        }
    }

    /// Closes the open panel. Returns false when canceled or already closed.
    pub fn close(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.close_with(reason, options.event, ReturnMode::Return, window, cx)
    }

    /// Moves focus to an item's trigger: the open one, or the first one,
    /// without a value.
    pub fn focus(&self, value: Option<&str>, window: &mut Window, cx: &mut App) {
        let key = value
            .map(|value| SharedString::from(value.to_string()))
            .or_else(|| self.value.get().clone());
        let trigger = match key {
            None => self.triggers.keys().first().and_then(|key| self.trigger_handle(key)),
            Some(key) => self.trigger_handle(&key),
        };
        if let Some(trigger) = trigger {
            window.focus(&trigger, cx);
        }
    }

    pub(crate) fn trigger_handle(&self, key: &SharedString) -> Option<FocusHandle> {
        self.shared.trigger_of(Some(key)).and_then(|trigger| trigger.focus_handle())
    }

    // Whether `key`'s view of the value is open: the viewport's while any
    // item is, a content's while its item is.
    fn view_open(&self, key: Option<&SharedString>) -> bool {
        match key {
            None => self.value.get().is_some(),
            Some(key) => self.value.get().as_ref() == Some(key),
        }
    }

    // Whether a surface is in the page: open, or animating out.
    pub(crate) fn is_mounted(&self, key: Option<&SharedString>) -> bool {
        self.surfaces
            .get(&key.cloned())
            .is_some_and(|surface| surface.layer.is_mounted())
    }

    // Whether any content without a viewport is in the page.
    fn contents_mounted(&self) -> bool {
        self.surfaces
            .iter()
            .any(|(key, surface)| key.is_some() && surface.layer.is_mounted())
    }

    // The item the menu shows now, or the one animating out.
    pub(crate) fn shown_item(&self) -> Option<SharedString> {
        self.value.get().clone().or_else(|| self.active.clone())
    }

    /// Whether a content shows in the viewport: open, the last open while
    /// the viewport animates out, or animating out after a switch.
    pub fn is_shown_in_viewport(&self, key: &SharedString) -> bool {
        let value = self.value.get();
        value.as_ref() == Some(key) || (value.is_none() && self.active.as_ref() == Some(key)) || self.exiting.contains(key)
    }

    /// Whether a content shows at all, in the viewport or as its own popup
    /// (`isShown`).
    pub fn is_shown(&self, key: &SharedString) -> bool {
        if self.viewport {
            return self.is_mounted(None) && self.is_shown_in_viewport(key);
        }
        self.value.get().as_ref() == Some(key) || self.is_mounted(Some(key))
    }

    /// Whether an item's content renders inside the viewport: while its item
    /// is open or animating out. Closed contents kept in the page stay with
    /// their item (`inViewport`).
    pub fn in_viewport(&self, key: &SharedString) -> bool {
        self.viewport && self.is_mounted(None) && self.is_shown_in_viewport(key)
    }

    // The key of the surface showing the menu now: the viewport, or the
    // open content.
    pub(crate) fn surface_key(&self) -> Option<SharedString> {
        if self.viewport {
            return None;
        }
        self.shown_item()
            .filter(|key| self.surfaces.contains_key(&Some(key.clone())))
    }

    pub(crate) fn surface(&self) -> &Surface {
        self.surfaces
            .get(&self.surface_key())
            .or_else(|| self.surfaces.get(&None))
            .expect("the viewport's surface is made with the menu")
    }

    // The element showing the open panel: the viewport, or the content.
    pub(crate) fn surface_element(&self) -> Option<Region> {
        self.surface().layer.element()
    }

    // A surface for `key`, the viewport's with `None`.
    fn create_surface(&self, key: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Surface {
        let trigger = {
            let shared = self.shared.clone();
            let key = key.clone();
            move || {
                let key = key.clone().or_else(|| shared.shown_item());
                shared.trigger_of(key.as_ref())
            }
        };
        let mut layer = PopupLayerOptions::new(
            PopupBehaviorName::NavigationMenu,
            trigger,
            |menu: &mut Self, reason, details: CloseDetails, window, cx| {
                menu.close_with(reason, details.event, details.focus, window, cx);
            },
            |_menu: &Self| modal_triple(Modal::False),
        );
        layer.owner = Some((self.instance.label().clone(), self.instance.id().clone()));
        layer.position = true;
        // Presses and focus anywhere in the navigation count as inside, so
        // moving between its items never dismisses the open one.
        let shared = self.shared.clone();
        layer.contains = Some(Rc::new(
            move |target: &Target, _event: Option<&InputEvent>, window: &Window, _cx: &App| {
                shared.root.contains(target, window)
            },
        ));
        layer.on_open_change = Some(Rc::new(|menu: &mut Self, open, _closed, _window, _cx| {
            if !open {
                menu.stop_hover();
            }
        }));
        let settled = key.clone();
        layer.on_open_change_complete = Some(Rc::new(move |menu: &mut Self, open, _closed, window, cx| {
            menu.surface_settled(settled.clone(), open, window, cx);
        }));
        let open = self.view_open(key.as_ref());
        let view = key.clone();
        let changing = key.clone();
        let (layer, steps) = create_stepped_popup(
            layer,
            open,
            move |layer| OpenStepsOptions {
                layer,
                is_open: Rc::new(move |menu: &Self| menu.view_open(view.as_ref())),
                change: Rc::new(move |menu: &mut Self, open, change, window, cx| {
                    let value = if open { changing.clone().or_else(|| menu.asked.clone()) } else { None };
                    menu.changed = menu.value.change(&menu.instance, value, change.options(), window, cx);
                    menu.after_change(window, cx);
                    menu.changed
                }),
                payload: None,
                dismissal: None,
                return_focus: None,
                rules: OpenRules {
                    open: None,
                    // The viewport stays open while another item takes it
                    // over, which opens it anew.
                    reopen: Some(Rc::new(|menu: &mut Self, _request, step, window, cx| {
                        if *menu.value.get() != menu.asked {
                            step.open(menu, None, window, cx);
                        }
                    })),
                    close: None,
                },
            },
            window,
            cx,
        );
        Surface { layer, steps }
    }

    /// Contents need their surface before their item first opens, so it
    /// hears the opening. Every part of an item makes it as it first renders
    /// (`ensureSurface`).
    pub(crate) fn ensure_surface(&mut self, key: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.surfaces.contains_key(&Some(key.clone())) {
            return;
        }
        let surface = self.create_surface(Some(key.clone()), window, cx);
        // How its content animates, if it rendered before.
        surface.layer.presence().set_timing(self.content_timing(&key));
        self.surfaces.insert(Some(key), surface);
    }

    // Derived state, written in the same change as the value, before any
    // surface hears about it, so a surface opening finds its trigger.
    fn follow_up(&mut self, from: Option<&SharedString>) {
        let Some(value) = self.value.get().clone() else {
            self.previous = None;
            self.forward = None;
            return;
        };
        let order = self.triggers.keys();
        let index = |key: &SharedString| order.iter().position(|other| other == key).map_or(-1, |index| index as isize);
        let switching = from.is_some();
        self.active = Some(value.clone());
        self.previous = from.cloned();
        self.forward = from.map(|from| index(&value) > index(from));
        let mut exiting: Vec<SharedString> = self
            .exiting
            .iter()
            .filter(|key| **key != value && Some(*key) != from)
            .cloned()
            .collect();
        if switching
            && self.viewport
            && let Some(from) = from
        {
            exiting.push(from.clone());
        }
        self.exiting = exiting;
    }

    // What the original's store subscriptions do: hears the value change
    // from any source, derives what comes with it, and has every surface
    // hear it.
    pub(crate) fn after_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.value.get().clone();
        if value != self.last_value {
            let from = std::mem::replace(&mut self.last_value, value.clone());
            self.follow_up(from.as_ref());
            *self.shared.value.borrow_mut() = value.clone();
            *self.shared.active.borrow_mut() = self.active.clone();
            match &value {
                None => self.closed_at = Some(Instant::now()),
                Some(key) => {
                    if from.is_none() {
                        self.opened_since_close = true;
                    }
                    self.ensure_surface(key.clone(), window, cx);
                }
            }
            cx.notify();
        }
        let surfaces: Vec<(Option<SharedString>, PopupLayer<Self>)> = self
            .surfaces
            .iter()
            .map(|(key, surface)| (key.clone(), surface.layer.clone()))
            .collect();
        for (key, layer) in surfaces {
            let open = self.view_open(key.as_ref());
            layer.open_changed(self, open, window, cx);
        }
        self.sync_viewport_contents(window, cx);
    }

    pub(crate) fn stop_hover(&mut self) {
        self.intent.clear();
        self.aim.stop();
    }

    /// Opens an item. Opening the open item again keeps it open, and a press
    /// on a panel the pointer opened keeps it open from now on.
    pub(crate) fn open(
        &mut self,
        key: SharedString,
        reason: Reason,
        event: Option<InputEvent>,
        by: PopupOpener,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.stop_hover();
        self.ensure_surface(key.clone(), window, cx);
        self.asked = Some(key.clone());
        self.changed = self.value.get().as_ref() == Some(&key);
        let surface_key = if self.viewport { None } else { Some(key) };
        let Some(steps) = self.surfaces.get(&surface_key).map(|surface| surface.steps.clone()) else {
            return false;
        };
        let _ = steps.show(self, ShowRequest::new().reason(reason).event(event).by(by), None, window, cx);
        self.changed
    }

    /// Closes the open item. Controlled menus close when the new value comes
    /// back, and focus returns then.
    pub(crate) fn close_with(
        &mut self,
        reason: Reason,
        event: Option<InputEvent>,
        focus: ReturnMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.value.get().is_none() {
            return false;
        }
        self.changed = false;
        let steps = self.surface().steps.clone();
        steps.hide(
            self,
            HideRequest::new().reason(reason).event(event).focus(focus),
            window,
            cx,
        );
        self.changed
    }

    // Focus left the navigation, from any of its items: the open panel
    // closes. Focus on nothing belongs to no one.
    fn focus_left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.value.get().is_none() {
            return;
        }
        let Some(focused) = window.focused(cx) else {
            return;
        };
        if self.shared.root.contains(&Target::Focus(focused), window) {
            return;
        }
        let event = InputEvent::Focus(window_source(window));
        self.close_with(Reason::FocusOut, Some(event), ReturnMode::Leave, window, cx);
    }

    // Listens for the parent's open item to change (`watch`).
    fn watch_parent(parent: &NavigationMenuParent, window: &mut Window, cx: &mut Context<Self>) -> Subscription {
        cx.observe_in(&parent.menu, window, |menu: &mut Self, parent, window, cx| {
            menu.parent_changed(&parent, window, cx)
        })
    }

    fn parent_changed(&mut self, parent: &Entity<Self>, window: &mut Window, cx: &mut Context<Self>) {
        let value = parent.read(cx).value.get().clone();
        if value == self.parent_seen {
            return;
        }
        self.parent_seen = value.clone();
        let Some(item) = self.parent.as_ref().map(|parent| parent.item.clone()) else {
            return;
        };
        if value.as_ref() != Some(&item) && self.value.get().is_some() {
            self.close_with(Reason::ParentClose, None, ReturnMode::Leave, window, cx);
        }
    }

    /// The pointer left the open panel. Onto its own trigger, it's still
    /// hovering.
    pub(crate) fn pointer_leave_panel(&mut self, event: &HoverEvent, window: &mut Window, cx: &mut Context<Self>) {
        let on_trigger = self
            .shared
            .trigger_of(self.value.get().as_ref())
            .is_some_and(|trigger| trigger.contains_point(event.position, window));
        if on_trigger {
            return;
        }
        let intent = self.intent.clone();
        intent.leave(self, event, window, cx);
    }

    pub(crate) fn pointer_enter_panel(&mut self, event: &HoverEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.aim.stop();
        let intent = self.intent.clone();
        intent.enter(self, event, None, Duration::ZERO, window, cx);
    }

    pub(crate) fn leave_trigger(&mut self, event: &HoverEvent, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.pointer_on.as_ref() == Some(key) {
            self.pointer_on = None;
        }
        let panel = self.surface_element();
        if self.value.get().as_ref() == Some(key)
            && let Some(panel) = panel
            && !event.touch
        {
            let along = match self.orientation {
                Orientation::Horizontal => SafeAreaAxis::Y,
                Orientation::Vertical => SafeAreaAxis::X,
            };
            self.aim.start(event.position, ElementBounds::from(panel), along);
        }
        let intent = self.intent.clone();
        intent.leave(self, event, window, cx);
    }

    pub(crate) fn hover_enter(
        &mut self,
        event: &HoverEvent,
        key: SharedString,
        disabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.open_on_hover || disabled {
            return;
        }
        self.pointer_on = Some(key.clone());
        if self.value.get().is_some() {
            let intent = self.intent.clone();
            intent.enter(self, event, None, Duration::ZERO, window, cx);
            if self.value.get().as_ref() == Some(&key) {
                return;
            }
            // Another trigger takes the open menu over at once, unless the
            // pointer is only crossing it on its way to the open panel.
            let aim = self.aim.clone();
            let resting = key.clone();
            let rest_event = event.event.clone();
            let crossing = aim.contains(
                event.position,
                move |menu: &mut Self, window, cx| {
                    if menu.pointer_on.as_ref() == Some(&resting) {
                        menu.open(resting, Reason::TriggerHover, Some(rest_event), PopupOpener::Hover, window, cx);
                    }
                },
                window,
                cx,
            );
            if !crossing {
                self.open(key, Reason::TriggerHover, Some(event.event.clone()), PopupOpener::Hover, window, cx);
            }
            return;
        }
        // Still animating out, or just closed: the delay is skipped.
        let warm = self.is_mounted(None)
            || self.contents_mounted()
            || self.closed_at.is_some_and(|at| at.elapsed() < WARM_FOR);
        let delay = if warm { Duration::ZERO } else { self.delay };
        let open_event = event.event.clone();
        let intent = self.intent.clone();
        intent.enter(
            self,
            event,
            Some(Box::new(move |menu: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                let disabled = menu.triggers.info(&key).is_some_and(|props| props.disabled);
                if !disabled && menu.open_on_hover {
                    menu.open(key, Reason::TriggerHover, Some(open_event), PopupOpener::Hover, window, cx);
                }
            })),
            delay,
            window,
            cx,
        );
    }

    pub(crate) fn hover_move(
        &mut self,
        event: &HoverEvent,
        key: SharedString,
        disabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.aim.active() && self.value.get().as_ref() != Some(&key) {
            self.hover_enter(event, key, disabled, window, cx);
        }
    }

    pub(crate) fn trigger_press(&mut self, key: SharedString, event: InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        let trigger = self.trigger_handle(&key);
        focus_pressed_trigger(trigger.as_ref(), window, cx);
        let by = opener_of(Some(&event));
        if self.value.get().as_ref() != Some(&key) {
            self.open(key, Reason::TriggerPress, Some(event), by, window, cx);
            return;
        }
        // A click on a trigger the pointer opened keeps it open: the user
        // meant to open it.
        if self.surface().layer.opener() == PopupOpener::Hover && matches!(by, PopupOpener::Pointer | PopupOpener::Touch) {
            self.open(key, Reason::TriggerPress, Some(event), by, window, cx);
            return;
        }
        self.close_with(Reason::TriggerPress, Some(event), ReturnMode::Return, window, cx);
    }

    /// A link was pressed: the menu closes, and so does every menu it's
    /// nested in (`linkPress`).
    pub(crate) fn link_press(
        &mut self,
        event: &InputEvent,
        close_on_select: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if opens_elsewhere(event) {
            return;
        }
        if !close_on_select.unwrap_or(self.close_on_select) {
            return;
        }
        self.close_with(Reason::LinkPress, Some(event.clone()), ReturnMode::Return, window, cx);
        if let Some(parent) = self.parent.clone() {
            parent
                .menu
                .update(cx, |menu, cx| menu.link_press(event, Some(true), window, cx));
        }
    }

    // The viewport joins the popup layer, placed against its anchor prop or
    // the open trigger. Returns whether it joined just now.
    fn attach_viewport(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.viewport_attached || !self.viewport {
            return false;
        }
        self.viewport_attached = true;
        let position = {
            let shared = self.shared.clone();
            Rc::new(move || shared.position(&shared.viewport_props.borrow().position)) as Rc<dyn Fn() -> PositionOptions>
        };
        let anchor = {
            let shared = self.shared.clone();
            Rc::new(move || shared.anchor(&shared.viewport_props.borrow().position, None)) as Rc<dyn Fn() -> Option<Anchor>>
        };
        let config = AttachConfig {
            position: Some(position),
            anchor: Some(anchor),
            initial_focus: Some(Rc::new(|_window: &mut Window, _cx: &mut App| None)),
            ..AttachConfig::default()
        };
        let layer = self.surfaces[&None].layer.clone();
        let region = self.viewport_region.clone();
        layer.bind(self, region, config, window, cx);
        true
    }

    /// The first content to render into the viewport shows it. Laid out out
    /// of sight first, it's placed for its real size before it shows
    /// (`showViewport`).
    pub(crate) fn show_viewport(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.attach_viewport(window, cx) {
            return;
        }
        self.surfaces[&None].layer.place();
        cx.notify();
    }

    // The viewport leaves the layer once it has closed, so the next open
    // waits for its content too.
    fn detach_viewport(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::replace(&mut self.viewport_attached, false) {
            return;
        }
        let layer = self.surfaces[&None].layer.clone();
        let region = self.viewport_region.clone();
        layer.unbind(self, &region, window, cx);
    }

    /// A content without a viewport joins its own popup as it first renders,
    /// placed against its anchor prop or its trigger. A viewport later in
    /// the page binds after its contents, so this waits until the frame is
    /// drawn, and a content never shows as its own popup first.
    pub(crate) fn bind_content(&mut self, key: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.viewport || held(&self.instance, "viewport", None) || self.bound_contents.contains(&key) {
            return;
        }
        let Some(layer) = self.surfaces.get(&Some(key.clone())).map(|surface| surface.layer.clone()) else {
            return;
        };
        let region = self.content_region(&key, cx);
        let position = {
            let shared = self.shared.clone();
            let key = key.clone();
            Rc::new(move || {
                let props = shared.content_props.borrow();
                let position = props.get(&key).map(|props| props.position.clone()).unwrap_or_default();
                shared.position(&position)
            }) as Rc<dyn Fn() -> PositionOptions>
        };
        let anchor = {
            let shared = self.shared.clone();
            let key = key.clone();
            Rc::new(move || {
                let position = shared
                    .content_props
                    .borrow()
                    .get(&key)
                    .map(|props| props.position.clone())
                    .unwrap_or_default();
                shared.anchor(&position, Some(&key))
            }) as Rc<dyn Fn() -> Option<Anchor>>
        };
        let config = AttachConfig {
            position: Some(position),
            anchor: Some(anchor),
            initial_focus: Some(Rc::new(|_window: &mut Window, _cx: &mut App| None)),
            ..AttachConfig::default()
        };
        self.bound_contents.insert(key.clone());
        layer.bind(self, region, config, window, cx);
        cx.notify();
    }

    fn unbind_content(&mut self, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if !self.bound_contents.remove(key) {
            return;
        }
        let Some(layer) = self.surfaces.get(&Some(key.clone())).map(|surface| surface.layer.clone()) else {
            return;
        };
        let region = self.content_region(key, cx);
        layer.unbind(self, &region, window, cx);
    }

    // The parts the surfaces depend on came or went: a viewport that
    // appeared takes the contents, one that left lets go of its layer, and
    // contents that left the page let go of theirs.
    fn check_bound(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = held(&self.instance, "viewport", None);
        if viewport != self.viewport {
            self.viewport = viewport;
            if viewport {
                let bound: Vec<SharedString> = self.bound_contents.iter().cloned().collect();
                for key in bound {
                    self.unbind_content(&key, window, cx);
                }
            } else {
                self.detach_viewport(window, cx);
            }
            self.sync_viewport_contents(window, cx);
            cx.notify();
        }
        let gone: Vec<SharedString> = self
            .bound_contents
            .iter()
            .filter(|key| !held(&self.instance, "content", Some(key)))
            .cloned()
            .collect();
        for key in gone {
            self.unbind_content(&key, window, cx);
        }
    }

    // A content in the viewport animates in when its item opens, and out
    // when another item takes over, while the viewport stays
    // (`bindInViewport`).
    fn sync_viewport_contents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut keys: Vec<SharedString> = self.viewport_contents.keys().cloned().collect();
        for key in self.value.get().iter().chain(self.active.iter()).chain(self.exiting.iter()) {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        for key in keys {
            let in_viewport = self.in_viewport(&key);
            let exiting = self.exiting.contains(&key);
            let Some(entry) = self.viewport_contents.get_mut(&key) else {
                if in_viewport {
                    let presence = Presence::new(false).with_timing(self.content_timing(&key));
                    presence.animate_in(window, cx);
                    self.viewport_contents.insert(key, ViewportContent { presence, leaving: false });
                }
                continue;
            };
            if !in_viewport {
                let entry = self.viewport_contents.remove(&key).expect("just found");
                entry.presence.stop(cx);
                self.viewport_size.unobserve(&key);
                continue;
            }
            if exiting && !entry.leaving {
                entry.leaving = true;
                let presence = entry.presence.clone();
                presence.stop(cx);
                // Content animating out is inert. Focus inside it goes back
                // to its trigger.
                let inside = self
                    .content_regions
                    .borrow()
                    .get(&key)
                    .is_some_and(|(region, _)| region.contains_focused(window, cx));
                if inside && let Some(trigger) = self.trigger_handle(&key) {
                    restore_focus(&trigger, window, cx);
                }
                let left = key.clone();
                presence.animate_out(
                    self,
                    move |menu, window, cx| {
                        menu.exiting.retain(|other| *other != left);
                        menu.sync_viewport_contents(window, cx);
                        cx.notify();
                    },
                    window,
                    cx,
                );
            } else if !exiting && entry.leaving && self.value.get().as_ref() == Some(&key) {
                entry.leaving = false;
                let presence = entry.presence.clone();
                presence.stop(cx);
                presence.animate_in(window, cx);
            }
        }
    }

    // How a content animates, from its props.
    fn content_timing(&self, key: &SharedString) -> PresenceTiming {
        self.shared
            .content_props
            .borrow()
            .get(key)
            .and_then(|props| props.timing.clone())
            .unwrap_or_default()
    }

    // A content's enter and exit as it shows now: in the viewport as items
    // switch, or as its own popup.
    pub(crate) fn content_presence(&self, key: &SharedString) -> PresenceState {
        if let Some(entry) = self.viewport_contents.get(key) {
            return entry.presence.state();
        }
        self.surfaces
            .get(&Some(key.clone()))
            .map(|surface| surface.layer.presence().state())
            .unwrap_or_default()
    }

    // What `on_open_change_complete` hears from a surface. Only the surface
    // in use reports, and only when the menu as a whole opened or closed:
    // without a viewport, a switch opens another surface. A close reports
    // even when its open never completed, like Escape during the intro.
    fn surface_settled(&mut self, key: Option<SharedString>, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if key.is_none() && !open {
            // Contents left over from a viewport that closed mid-switch go
            // with it.
            self.exiting.clear();
            self.detach_viewport(window, cx);
            self.sync_viewport_contents(window, cx);
        }
        if self.surface_key() != key || open != self.value.get().is_some() {
            return;
        }
        let reported = if open { self.reported_open } else { !self.opened_since_close };
        if reported {
            return;
        }
        self.reported_open = open;
        if !open {
            self.opened_since_close = false;
        }
        if let Some(callback) = self.on_open_change_complete.clone() {
            callback(open, window, cx);
        }
    }

    // A content's region, with the focus handle its element tracks, made the
    // first time it's asked for and marked as a popup's content.
    pub(crate) fn content_region(&self, key: &SharedString, cx: &App) -> Region {
        let mut regions = self.content_regions.borrow_mut();
        if let Some((region, _)) = regions.get(key) {
            return region.clone();
        }
        let region = Region::with_focus_handle(cx.focus_handle());
        let element = register_popup_element(&region, self.instance.slot_name("content"));
        regions.insert(key.clone(), (region.clone(), element));
        region
    }

    // A trigger's region, made the first time it's asked for.
    pub(crate) fn trigger_region(&self, key: &SharedString, cx: &App) -> Region {
        let mut triggers = self.shared.triggers.borrow_mut();
        triggers
            .entry(key.clone())
            .or_insert_with(|| Region::with_focus_handle(cx.focus_handle()))
            .clone()
    }

    // A link's region, made the first time it's asked for.
    pub(crate) fn link_region(&self, key: &SharedString, cx: &App) -> Region {
        self.link_regions
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| Region::with_focus_handle(cx.focus_handle()))
            .clone()
    }

    // What a surface's part renders through.
    pub(crate) fn surface_view(&self, key: Option<&SharedString>, entity: WeakEntity<Self>, cx: &App) -> Option<SurfaceView> {
        let surface = self.surfaces.get(&key.cloned())?;
        let (region, stash) = match key {
            None => (self.viewport_region.clone(), Some(self.stash.clone())),
            Some(key) => (self.content_region(key, cx), None),
        };
        Some(SurfaceView {
            layer: surface.layer.clone(),
            region,
            intent: self.intent.clone(),
            entity,
            stash,
            attached: self.viewport_attached,
            key: key.cloned(),
            bound: key.is_some_and(|key| self.bound_contents.contains(key)),
        })
    }

    // Starts a render of the menu's parts, from its root.
    pub(crate) fn begin_render(&self) {
        self.triggers.begin();
        self.links.begin();
        self.list_items.begin();
        self.stash.clear();
    }
}
