//! Port of `components/navigation-menu/navigation-menu.ts`: a site's main
//! links, grouped into panels that open from a row of triggers.
//!
//! The menu follows the disclosure navigation pattern: a landmark with a
//! list of buttons that show and hide groups of ordinary links. Each panel
//! shows on a popup surface: the shared viewport when there is one, or the
//! content itself, positioned against its trigger, when there isn't.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Div, ElementId, Entity, InteractiveElement, Interactivity, IntoElement,
    ParentElement, Pixels, SharedString, Size, Stateful, StatefulInteractiveElement, StyleRefinement,
    Styled, Window,
};
use ui_sdk_kernel::dev;
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::{HandledKey, Role};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentSurface, Aria, ChangeCallback, ChangeDetails, ChangeOptions,
    Current, Key, Part, Reason, key_while_bound,
};
use ui_sdk_lists::{IconPlacement, ItemIconProps, ItemIconState, check_icon_item, icon_part};
use ui_sdk_motion::{PresenceState, PresenceTiming};
use ui_sdk_platform::{LayoutBox, Placement, PositionProps};
use ui_sdk_popups::{HoverEvent, owns_event};

use crate::navigation_menu_api::ListEntry;
pub use crate::navigation_menu_api::NavigationMenu;
use crate::navigation_menu_dom::IndicatorProbe;
pub use crate::navigation_menu_keyboard::NavigationMenuMotion;
use crate::navigation_menu_keyboard::motion_of;

/// Why the open item changed: the pointer rested on or left a trigger
/// (`TriggerHover`), a trigger was pressed or an arrow key opened it
/// (`TriggerPress`), Escape (`EscapeKey`), a press outside (`OutsidePress`),
/// focus left the navigation (`FocusOut`), a link was pressed (`LinkPress`),
/// the menu around a nested one moved on (`ParentClose`), or a call
/// (`Imperative`). `ClosePress` and `FindInPage` stay for calls that pass
/// them.
pub type NavigationMenuValueChangeReason = Reason;

/// Options every write method takes.
pub type NavigationMenuChangeOptions = ChangeOptions;

/// Which arrow keys move between the top-level items, and where panels open.
pub type NavigationMenuOrientation = gpui::Orientation;

/// What `on_open_change_complete` runs, with whether the menu is open.
pub type OpenChangeCompleteCallback = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// The props the `link-icon` part takes: `mirror` flips it in
/// right-to-left text, and `content` is the icon itself.
pub type NavigationMenuLinkIconProps = ItemIconProps;

/// What the `link-icon` part's `with` reads: whether it mirrors, and
/// whether to flip it now.
pub type NavigationMenuLinkIconState = ItemIconState;

// Where a link's icon sits: in a link, in a content or the list.
const ICON: IconPlacement = IconPlacement {
    component: "navigation-menu",
    item: Role::Link,
    list: None,
};

/// The menu a nested one sits in, and the item whose content holds it.
#[derive(Clone)]
pub struct NavigationMenuParent {
    pub menu: Entity<NavigationMenu>,
    pub item: SharedString,
}

/// The options `NavigationMenu` takes.
#[derive(Clone)]
pub struct NavigationMenuOptions {
    /// The open item's value. Pass it to control the menu. `Some(None)`
    /// closes every panel.
    pub value: Option<Option<SharedString>>,
    /// The item open at first, when uncontrolled.
    pub default_value: Option<SharedString>,
    /// Runs when an item wants to open or the open one to close.
    /// `details.reason` says why, and `details.cancel()` stops it. It runs
    /// inside the menu's update, so it may update any entity but the menu.
    pub on_value_change: Option<ChangeCallback<Option<SharedString>>>,
    /// Runs once the menu has finished opening or closing, after any
    /// animation. Switching items doesn't count.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// How long the pointer rests on a trigger before its panel opens.
    /// Moving to another trigger while one is open switches at once. 50 ms
    /// by default.
    pub delay: Duration,
    /// How long before the open panel closes once the pointer leaves its
    /// trigger and the panel. 50 ms by default.
    pub close_delay: Duration,
    /// Whether resting the pointer on a trigger opens its panel. Pressing a
    /// trigger always does, so `false` makes a click-only menu. True by
    /// default.
    pub open_on_hover: bool,
    /// Whether pressing a link closes the menu, like after a route change. A
    /// link's own `close_on_select` wins. A press with a modifier key, which
    /// opens a new tab, never closes it. True by default.
    pub close_on_select: bool,
    /// Which arrow keys move between the top-level items, and the side
    /// panels open on: below a horizontal menu, beside a vertical one.
    /// Horizontal when unset.
    pub orientation: Option<NavigationMenuOrientation>,
    /// The menu this one is nested in, and the item whose content holds it
    /// (`parent`). A nested menu is no landmark, and closes once its parent
    /// moves on to another item.
    pub parent: Option<NavigationMenuParent>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for NavigationMenuOptions {
    fn default() -> Self {
        Self {
            value: None,
            default_value: None,
            on_value_change: None,
            on_open_change_complete: None,
            delay: Duration::from_millis(50),
            close_delay: Duration::from_millis(50),
            open_on_hover: true,
            close_on_select: true,
            orientation: None,
            parent: None,
            debug: false,
        }
    }
}

impl NavigationMenuOptions {
    pub fn value(mut self, value: Option<impl Into<SharedString>>) -> Self {
        self.value = Some(value.map(Into::into));
        self
    }

    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&Option<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn close_delay(mut self, delay: Duration) -> Self {
        self.close_delay = delay;
        self
    }

    pub fn open_on_hover(mut self, open_on_hover: bool) -> Self {
        self.open_on_hover = open_on_hover;
        self
    }

    pub fn close_on_select(mut self, close_on_select: bool) -> Self {
        self.close_on_select = close_on_select;
        self
    }

    pub fn orientation(mut self, orientation: NavigationMenuOrientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    pub fn parent(mut self, menu: Entity<NavigationMenu>, item: impl Into<SharedString>) -> Self {
        self.parent = Some(NavigationMenuParent {
            menu,
            item: item.into(),
        });
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `trigger` part takes beside the value of its item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavigationMenuTriggerProps {
    /// Stops the trigger from opening its panel. Like a native disabled
    /// button, it can't be focused or pressed.
    pub disabled: bool,
}

impl NavigationMenuTriggerProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Where a surface goes, the content's or the viewport's: against its
/// `anchor`, or the open trigger, on the side the orientation suggests.
/// The side defaults to the bottom of a horizontal menu and the inline end
/// of a vertical one, the alignment to the start, and the side offset to 4.
pub type NavigationMenuPlacementProps = PositionProps;

/// The props the `content` part takes beside the value of its item.
#[derive(Clone, Default)]
pub struct NavigationMenuContentProps {
    /// Where it goes without a viewport, as its own popup.
    pub position: NavigationMenuPlacementProps,
    /// Whether the content stays in the page, hidden, while its item is
    /// closed. Closed content stays in its item; it moves into the viewport
    /// when it opens.
    pub keep_mounted: bool,
    /// Keeps closed content in the page, as `keep_mounted` does. The
    /// original makes it findable with `hidden="until-found"`, which gpui
    /// has no find in page for.
    pub hidden_until_found: bool,
    /// How it animates in and out, the transitions its CSS would declare:
    /// in the viewport as items switch, and as its own popup.
    pub timing: Option<PresenceTiming>,
}

impl NavigationMenuContentProps {
    pub fn position(mut self, position: NavigationMenuPlacementProps) -> Self {
        self.position = position;
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

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = Some(timing);
        self
    }
}

/// The props the `link` part takes beside its key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavigationMenuLinkProps {
    /// Whether the link is the current page: `aria-current="page"`.
    pub current: bool,
    /// Whether pressing the link closes the menu. Overrides the menu's
    /// `close_on_select`.
    pub close_on_select: Option<bool>,
}

impl NavigationMenuLinkProps {
    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    pub fn close_on_select(mut self, close_on_select: bool) -> Self {
        self.close_on_select = Some(close_on_select);
        self
    }
}

/// The props the `viewport` part takes.
#[derive(Clone, Default)]
pub struct NavigationMenuViewportProps {
    /// Where it goes: below the open trigger, lined up with its start, or
    /// beside it in a vertical menu, unless these say otherwise. It flips and
    /// shifts to stay in the window. An `anchor` of the whole bar gives one
    /// dropdown that stays put while its contents change.
    pub position: NavigationMenuPlacementProps,
    /// How it animates in and out as the menu opens and closes.
    pub timing: Option<PresenceTiming>,
}

impl NavigationMenuViewportProps {
    pub fn position(mut self, position: NavigationMenuPlacementProps) -> Self {
        self.position = position;
        self
    }

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = Some(timing);
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationMenuState {
    /// The open item's value.
    pub value: Option<SharedString>,
    /// Whether a panel is open.
    pub open: bool,
    /// The menu's `orientation` option, or horizontal when it's unset.
    pub orientation: NavigationMenuOrientation,
    /// Whether the menu sits inside another one's content (`data-nested`).
    pub nested: bool,
}

/// What the list's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationMenuListState {
    /// Whether a panel is open.
    pub open: bool,
    /// The menu's orientation.
    pub orientation: NavigationMenuOrientation,
    /// Whether a top-level link in it has a `link-icon` part
    /// (`data-has-icons`), as of the last frame, so links without one can
    /// leave its space.
    pub has_icons: bool,
}

/// What an item's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationMenuItemState {
    /// Whether this item's panel is open.
    pub open: bool,
}

/// What a trigger's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationMenuTriggerState {
    /// Whether this trigger's panel is open.
    pub open: bool,
    /// Set by the trigger's `disabled` prop, which stops it opening its
    /// panel.
    pub disabled: bool,
}

/// What a content's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavigationMenuContentState {
    /// Whether this content's item is open. False while it animates out.
    pub open: bool,
    /// Which way it moves when another item takes over or gives way, or
    /// `None` when the menu opened or closed as a whole (`data-motion`).
    pub motion: Option<NavigationMenuMotion>,
    /// The menu's orientation.
    pub orientation: NavigationMenuOrientation,
    /// Kept in the page while closed, it renders hidden.
    pub hidden: bool,
    /// Its enter and exit (`data-starting-style`, `data-ending-style`): in
    /// the viewport as items switch, or as its own popup.
    pub presence: PresenceState,
    /// Where it's placed as its own popup, as of the last frame: its side,
    /// alignment and room (`data-side`, `--available-height`, ..).
    pub placement: Option<Placement>,
    /// Whether a link in it has a `link-icon` part (`data-has-icons`), as
    /// of the last frame.
    pub has_icons: bool,
}

/// What a link's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationMenuLinkState {
    /// Whether the link is the current page (`data-current`).
    pub current: bool,
    /// Whether a `link-icon` part is in it (`data-has-icon`).
    pub has_icon: bool,
}

/// What the viewport's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavigationMenuViewportState {
    /// Whether a panel is open. False while it animates out.
    pub open: bool,
    /// The menu's orientation.
    pub orientation: NavigationMenuOrientation,
    /// The open content's size (`--viewport-width`, `--viewport-height`),
    /// kept while the viewport closes, or `None` before it laid out. Give the
    /// viewport this size, plus its own border, and clip its overflow.
    pub size: Option<Size<Pixels>>,
    /// Its enter and exit as the menu opens and closes.
    pub presence: PresenceState,
    /// Where it's placed, as of the last frame (`--available-height`, ..).
    pub placement: Option<Placement>,
    /// Out of the page while closed, it renders hidden.
    pub hidden: bool,
}

/// What the indicator's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavigationMenuIndicatorState {
    /// Whether a panel is open.
    pub open: bool,
    /// The menu's orientation.
    pub orientation: NavigationMenuOrientation,
    /// Whether it has something to cover (`data-visible`).
    pub visible: bool,
    /// The box it covers inside the root (`--active-item-left`, `-right`,
    /// `-top`, `-bottom`, `-width`, `-height`): the open trigger's, or the
    /// current page's top-level link while every panel is closed.
    pub item_box: Option<LayoutBox>,
}

// Where a content renders this frame.
pub(crate) enum ContentPlacement {
    // Into the viewport, which renders it.
    Viewport(Rc<crate::navigation_menu_api::Stash>),
    // In its item, hidden: kept while closed.
    InPlace,
    // As its own popup, above the window.
    Popup(crate::navigation_menu_api::SurfaceView),
}

/// A content part (`content`), which renders into the viewport while its
/// item is open, as its own popup without a viewport, and in place while
/// closed and kept. Style it and add its links like any part.
pub struct NavigationMenuContent {
    pub(crate) key: SharedString,
    pub(crate) part: Part<NavigationMenuContentState>,
    pub(crate) placement: ContentPlacement,
}

impl NavigationMenuContent {
    /// The content's state.
    pub fn state(&self) -> &NavigationMenuContentState {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &NavigationMenuContentState) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Changes the part underneath, like adding ARIA.
    pub fn map_part(mut self, map: impl FnOnce(Part<NavigationMenuContentState>) -> Part<NavigationMenuContentState>) -> Self {
        self.part = map(self.part);
        self
    }
}

impl Styled for NavigationMenuContent {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl InteractiveElement for NavigationMenuContent {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl StatefulInteractiveElement for NavigationMenuContent {}

impl ParentElement for NavigationMenuContent {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl IntoElement for NavigationMenuContent {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        match self.placement {
            ContentPlacement::Viewport(stash) => {
                stash.push(self.key, self.part.into_any_element());
                gpui::Empty.into_any_element()
            }
            ContentPlacement::InPlace => self.part.hidden().into_any_element(),
            ContentPlacement::Popup(surface) => surface.render(self.part),
        }
    }
}

/// The viewport part (`viewport`): the one popup every content renders
/// into, sized to the open one and placed against the open trigger. Style
/// it and add to it like any part.
pub struct NavigationMenuViewport {
    pub(crate) part: Part<NavigationMenuViewportState>,
    pub(crate) surface: crate::navigation_menu_api::SurfaceView,
}

impl NavigationMenuViewport {
    /// The viewport's state.
    pub fn state(&self) -> &NavigationMenuViewportState {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &NavigationMenuViewportState) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Changes the part underneath, like adding ARIA.
    pub fn map_part(mut self, map: impl FnOnce(Part<NavigationMenuViewportState>) -> Part<NavigationMenuViewportState>) -> Self {
        self.part = map(self.part);
        self
    }
}

impl Styled for NavigationMenuViewport {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl InteractiveElement for NavigationMenuViewport {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl StatefulInteractiveElement for NavigationMenuViewport {}

impl ParentElement for NavigationMenuViewport {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl IntoElement for NavigationMenuViewport {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        if self.part.state().hidden {
            return self.part.hidden().into_any_element();
        }
        self.surface.render_viewport(self.part)
    }
}

// The keys the list hands to its top-level items.
const LIST_KEYS: &[HandledKey] = &["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"];
// The keys a content hands to its links, and Tab, which flows out of it.
const PANEL_KEYS: &[HandledKey] = &["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "Tab"];

impl NavigationMenu {
    fn root_state(&self) -> NavigationMenuState {
        NavigationMenuState {
            value: self.value.get().clone(),
            open: self.value.get().is_some(),
            orientation: self.orientation,
            nested: self.nested,
        }
    }

    /// The `root` part: a navigation landmark, or a plain element when the
    /// menu sits in another one's content. Name it with `aria_label`. Build
    /// it first: it starts the render of the menu's parts, so the order they
    /// render in is page order. Presses and focus anywhere in it count as
    /// inside the open panel.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<NavigationMenuState> {
        let menu = this.read(cx);
        menu.begin_render();
        let state = menu.root_state();
        let entity = this.downgrade();
        let settled = this.downgrade();
        let focus = menu.shared.root.focus_handle().expect("the root's region has a focus handle");
        let mut part = Part::new(&menu.instance, "root", None, state.clone())
            .on_keys(&["Tab"], move |event, window, cx| {
                entity.update(cx, |menu, cx| menu.root_keydown(event, window, cx)).ok();
            })
            .on_props(move |window, cx| {
                settled.update(cx, |menu, cx| menu.settle_top_level(window, cx)).ok();
            })
            .map_el(|el| el.track_focus(&focus.tab_stop(false)));
        if !state.nested {
            part = part.aria_role(Role::Navigation);
        }
        menu.shared.root.measure(part)
    }

    /// The `list` part: the top-level items, which Left and Right move
    /// between, or Up and Down in a vertical menu, with Home and End to the
    /// first and last. Nothing wraps.
    pub fn list(this: &Entity<Self>, cx: &App) -> Part<NavigationMenuListState> {
        let menu = this.read(cx);
        let state = NavigationMenuListState {
            open: menu.value.get().is_some(),
            orientation: menu.orientation,
            has_icons: menu.icons_in(&menu.instance.element_id("list", None)),
        };
        let entity = this.downgrade();
        Part::new(&menu.instance, "list", None, state)
            .aria_role(Role::List)
            .on_keys(LIST_KEYS, move |event, window, cx| {
                entity.update(cx, |menu, cx| menu.list_keydown(event, window, cx)).ok();
            })
    }

    /// An `item` part for `value`: a link, or a trigger and the content it
    /// opens. Values identify the items, so each is unique.
    pub fn item(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<NavigationMenuItemState> {
        let menu = this.read(cx);
        let key: SharedString = value.into();
        let state = NavigationMenuItemState {
            open: menu.value.get().as_ref() == Some(&key),
        };
        let part = Part::new(&menu.instance, "item", Some(&key), state).aria_role(Role::ListItem);
        menu.ensuring(this, key, part)
    }

    // A part of an item makes the item's surface once it has rendered.
    fn ensuring<S: 'static>(&self, this: &Entity<Self>, key: SharedString, part: Part<S>) -> Part<S> {
        if self.surfaces.contains_key(&Some(key.clone())) {
            return part;
        }
        let entity = this.downgrade();
        part.on_props(move |window, cx| {
            entity.update(cx, |menu, cx| menu.ensure_surface(key, window, cx)).ok();
        })
    }

    /// The `trigger` part of the item with `value`: a button that opens and
    /// closes its panel, like a disclosure, and opens it when the pointer
    /// rests on it. Tab from the open trigger moves into its panel,
    /// wherever it renders. It takes the element's `on_hover`, which gpui
    /// allows once per element; style hovering with `hover`.
    pub fn trigger(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: NavigationMenuTriggerProps,
        cx: &App,
    ) -> Part<NavigationMenuTriggerState> {
        let menu = this.read(cx);
        let key: SharedString = value.into();
        let region = menu.trigger_region(&key, cx);
        let handle = region.focus_handle().expect("a trigger's region has a focus handle");
        menu.triggers.register(key.clone(), props);
        menu.list_items.register(ListEntry::Trigger(key.clone()), ());
        let open = menu.value.get().as_ref() == Some(&key);
        let state = NavigationMenuTriggerState {
            open,
            disabled: props.disabled,
        };
        let mut aria = Aria::new().disabled(props.disabled);
        if menu.is_shown(&key) {
            aria = aria.controls(menu.instance.element_id("content", Some(&key)));
        }
        let entity = this.downgrade();
        let pressed = key.clone();
        let hovered = key.clone();
        let moved = key.clone();
        let keyed = key.clone();
        let hover_entity = entity.clone();
        let move_entity = entity.clone();
        let key_entity = entity.clone();
        let part = Part::new(&menu.instance, "trigger", Some(&key), state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&handle), props.disabled)
            .aria_expanded(open)
            .aria(aria)
            .on_press(move |event, window, cx| {
                if props.disabled {
                    return;
                }
                entity
                    .update(cx, |menu, cx| menu.trigger_press(pressed.clone(), event, window, cx))
                    .ok();
            })
            .on_keys(&["Tab"], move |event, window, cx| {
                let Some(target) = key_entity.upgrade() else {
                    return;
                };
                let took = target.update(cx, |menu, cx| {
                    if menu.value.get().as_ref() != Some(&keyed) {
                        return false;
                    }
                    let layer = menu.surface().layer.clone();
                    layer.trigger_keydown(menu, event, window, cx)
                });
                if took {
                    cx.stop_propagation();
                }
            })
            .on_hover(move |entered, window, cx| {
                let event = HoverEvent::from_window(window);
                let entered = *entered;
                hover_entity
                    .update(cx, |menu, cx| {
                        if entered {
                            menu.hover_enter(&event, hovered.clone(), props.disabled, window, cx);
                        } else {
                            menu.leave_trigger(&event, &hovered, window, cx);
                        }
                    })
                    .ok();
            })
            .on_mouse_move(move |event, window, cx| {
                let event = HoverEvent::from_move(event, window);
                move_entity
                    .update(cx, |menu, cx| menu.hover_move(&event, moved.clone(), props.disabled, window, cx))
                    .ok();
            });
        let part = menu.ensuring(this, key, part);
        region.measure(part)
    }

    /// The `content` part of the item with `value`, or `None` while it's out
    /// of the page: the panel of links its trigger opens. It renders into
    /// the viewport while its item is open or animating out, as its own
    /// popup placed against its trigger without a viewport, and in place,
    /// hidden, while closed and kept. In a panel, arrow keys move between
    /// its links, and without a viewport, Tab flows out of it back to the
    /// window after its trigger. As its own popup it takes the element's
    /// `on_hover`, as the viewport does.
    pub fn content(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: NavigationMenuContentProps,
        cx: &App,
    ) -> Option<NavigationMenuContent> {
        let menu = this.read(cx);
        let key: SharedString = value.into();
        let keep = props.keep_mounted || props.hidden_until_found;
        if let Some(timing) = &props.timing
            && let Some(surface) = menu.surfaces.get(&Some(key.clone()))
        {
            surface.layer.presence().set_timing(timing.clone());
        }
        menu.shared.content_props.borrow_mut().insert(key.clone(), props);
        let shown = menu.is_shown(&key);
        if !shown && !keep {
            return None;
        }
        let open = menu.value.get().as_ref() == Some(&key);
        let placement = if menu.in_viewport(&key) {
            Some(ContentPlacement::Viewport(menu.stash.clone()))
        } else if menu.viewport || !shown {
            Some(ContentPlacement::InPlace)
        } else {
            menu.surface_view(Some(&key), this.downgrade(), cx).map(ContentPlacement::Popup)
        }?;
        let own_popup = matches!(placement, ContentPlacement::Popup(_));
        let in_viewport = matches!(placement, ContentPlacement::Viewport(_));
        let state = NavigationMenuContentState {
            open,
            motion: motion_of(
                &key,
                menu.value.get().as_deref(),
                menu.previous.as_deref(),
                menu.forward,
            ),
            orientation: menu.orientation,
            hidden: !shown,
            presence: menu.content_presence(&key),
            placement: own_popup
                .then(|| menu.surfaces.get(&Some(key.clone())))
                .flatten()
                .and_then(|surface| surface.layer.positioner())
                .and_then(|positioner| positioner.placement()),
            has_icons: menu.icons_in(&menu.instance.element_id("content", Some(&key))),
        };
        let region = menu.content_region(&key, cx);
        let focus = region.focus_handle().expect("a content's region has a focus handle");
        let entity = this.downgrade();
        let keyed = key.clone();
        let mut part = Part::new(&menu.instance, "content", Some(&key), state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(state.hidden))
            .map_el(|el| el.track_focus(&focus.tab_stop(false)));
        // Animating out, it's inert: it hears nothing.
        if !state.presence.inert {
            part = part.on_keys(PANEL_KEYS, move |event, window, cx| {
                entity
                    .update(cx, |menu, cx| {
                        menu.panel_keydown(&keyed, event, window, cx);
                        // Without a viewport, Tab flows out of the content
                        // back to the window after its trigger.
                        if menu.viewport {
                            return;
                        }
                        let Some(region) = menu.content_regions.borrow().get(&keyed).map(|(region, _)| region.clone())
                        else {
                            return;
                        };
                        if !owns_event(&region, window, cx) {
                            return;
                        }
                        if let Some(layer) = menu.surfaces.get(&Some(keyed.clone())).map(|surface| surface.layer.clone()) {
                            layer.keydown(menu, event, window, cx);
                        }
                    })
                    .ok();
            });
            if own_popup {
                let hover = this.downgrade();
                part = part.on_hover(move |entered, window, cx| {
                    let event = HoverEvent::from_window(window);
                    let entered = *entered;
                    hover
                        .update(cx, |menu, cx| {
                            if entered {
                                menu.pointer_enter_panel(&event, window, cx);
                            } else {
                                menu.pointer_leave_panel(&event, window, cx);
                            }
                        })
                        .ok();
                });
            }
        }
        if in_viewport {
            part = menu.viewport_size.observe(key.clone(), part);
        }
        if shown && menu.pending.as_ref() == Some(&key) {
            let entity = this.downgrade();
            let bound = key.clone();
            part = part.on_props(move |window, cx| {
                entity.update(cx, |menu, cx| menu.content_bound(&bound, window, cx)).ok();
            });
        }
        let part = menu.ensuring(this, key.clone(), part);
        // As its own popup, the surface measures it; elsewhere it measures
        // itself, for containment.
        let part = if own_popup { part } else { region.measure(part) };
        Some(NavigationMenuContent { key, part, placement })
    }

    /// A `link` part, by a key of the app's own: a link the app's own click
    /// follows, like a router's. Pressing it closes the menu, and every menu
    /// it's nested in, unless `close_on_select` says otherwise; a press that
    /// opens it elsewhere, with a modifier key or the middle button, leaves
    /// the menu open. `current` marks the current page.
    pub fn link(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: NavigationMenuLinkProps,
        cx: &App,
    ) -> Part<NavigationMenuLinkState> {
        let menu = this.read(cx);
        let key: SharedString = key.into();
        let region = menu.link_region(&key, cx);
        let handle = region.focus_handle().expect("a link's region has a focus handle");
        menu.links.register(key.clone(), props);
        menu.list_items.register(ListEntry::Link(key.clone()), ());
        let state = NavigationMenuLinkState {
            current: props.current,
            has_icon: key_while_bound(&menu.instance, "link-icon").contains(&key),
        };
        let entity = this.downgrade();
        let mut part = Part::new(&menu.instance, "link", Some(&key), state)
            .aria_role(Role::Link)
            .focus_mode(FocusMode::TabStop, Some(&handle), false)
            .aria(Aria::new().current(props.current.then_some(Current::Page)));
        // A link that shows only an icon names nothing.
        if dev::DEV && state.has_icon {
            let entity = entity.clone();
            let key = key.clone();
            part = part.keep_name().on_props(move |window, cx| {
                if let Some(menu) = entity.upgrade() {
                    check_icon_item(&menu.read(cx).instance, "link", Some(&key), ICON, window);
                }
            });
        }
        let part = part
            // A link answers Enter alone, as a native one does: Space makes
            // no press of it.
            .capture_key_down(|event, window, _cx| {
                if Key::of(&event.keystroke) == Key::Space && !event.keystroke.modifiers.modified() {
                    window.prevent_default();
                }
            })
            // The app's own click, like a router's, runs first: closing
            // unmounts the panel, so the menu closes once the press is over.
            .on_press(move |event, window, cx| {
                let entity = entity.clone();
                window.defer(cx, move |window, cx| {
                    entity
                        .update(cx, |menu, cx| menu.link_press(&event, props.close_on_select, window, cx))
                        .ok();
                });
            });
        region.measure(part)
    }

    /// The `link-icon` part of the link with `key`: an icon before its text,
    /// which screen readers skip, so it never joins the link's name. A link
    /// with only an icon needs an `aria_label`. The link knows it has one,
    /// and so does the content or list around it. `mirror` flips it in
    /// right-to-left text, where its state's `flipped` says to.
    pub fn link_icon(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: NavigationMenuLinkIconProps,
        cx: &App,
    ) -> Part<NavigationMenuLinkIconState> {
        let key: SharedString = key.into();
        icon_part(&this.read(cx).instance, "link-icon", Some(&key), &props, cx)
    }

    // Whether a link icon's nearest content or list is the part with `id`,
    // as of the last frame: the content around it, or the list for a
    // top-level link.
    fn icons_in(&self, id: &ElementId) -> bool {
        // Asking makes the menu render again as icons come and go.
        key_while_bound(&self.instance, "link-icon");
        let list = self.instance.element_id("list", None);
        let contents: Vec<ElementId> = self
            .content_regions
            .borrow()
            .keys()
            .map(|key| self.instance.element_id("content", Some(key)))
            .collect();
        self.instance.bound_parts().iter().any(|part| {
            part.slot == "link-icon"
                && part
                    .path
                    .iter()
                    .rev()
                    .find(|ancestor| **ancestor == list || contents.contains(ancestor))
                    == Some(id)
        })
    }

    /// The `viewport` part: the one popup every content renders into, sized
    /// to the open one and placed against the open trigger. Render it after
    /// the list, always, closed or not, so contents know to render into it
    /// before any item opens. Give it its state's `size` and clip its
    /// overflow; place the contents in it at its top left corner.
    pub fn viewport(this: &Entity<Self>, props: NavigationMenuViewportProps, cx: &App) -> NavigationMenuViewport {
        let menu = this.read(cx);
        let viewport = menu.surfaces.get(&None).expect("the viewport's surface is made with the menu");
        if let Some(timing) = &props.timing {
            viewport.layer.presence().set_timing(timing.clone());
        }
        *menu.shared.viewport_props.borrow_mut() = props;
        let mounted = viewport.layer.is_mounted();
        let state = NavigationMenuViewportState {
            open: menu.value.get().is_some(),
            orientation: menu.orientation,
            size: menu.viewport_size.write(menu.value.get().as_ref()),
            presence: viewport.layer.presence().state(),
            placement: viewport.layer.positioner().and_then(|positioner| positioner.placement()),
            hidden: !mounted,
        };
        let focus = menu
            .viewport_region
            .focus_handle()
            .expect("the viewport's region has a focus handle");
        let entity = this.downgrade();
        let hover = this.downgrade();
        let viewport_region = menu.viewport_region.clone();
        let mut part = Part::new(&menu.instance, "viewport", None, state)
            .map_el(|el| el.track_focus(&focus.tab_stop(false)));
        if !state.presence.inert {
            part = part
                // Keys from a nested menu's viewport are its own.
                .on_keys(&["Tab"], move |event, window, cx| {
                    if window.default_prevented() || !owns_event(&viewport_region, window, cx) {
                        return;
                    }
                    entity
                        .update(cx, |menu, cx| {
                            let layer = menu.surfaces[&None].layer.clone();
                            layer.keydown(menu, event, window, cx);
                        })
                        .ok();
                })
                .on_hover(move |entered, window, cx| {
                    let event = HoverEvent::from_window(window);
                    let entered = *entered;
                    hover
                        .update(cx, |menu, cx| {
                            if entered {
                                menu.pointer_enter_panel(&event, window, cx);
                            } else {
                                menu.pointer_leave_panel(&event, window, cx);
                            }
                        })
                        .ok();
                });
        }
        let surface = menu
            .surface_view(None, this.downgrade(), cx)
            .expect("the viewport's surface is made with the menu");
        NavigationMenuViewport { part, surface }
    }

    /// The `indicator` part: it covers the open trigger, or the current
    /// page's top-level link while every panel is closed, and screen readers
    /// skip it. Render it in the root, and place it from its state's
    /// `item_box`, which is relative to the root.
    pub fn indicator(this: &Entity<Self>, cx: &App) -> Part<NavigationMenuIndicatorState> {
        let menu = this.read(cx);
        let target = menu.shared.trigger_of(menu.value.get().as_ref()).or_else(|| {
            let links = menu.top_links.borrow();
            links
                .iter()
                .find(|key| menu.links.info(key).is_some_and(|props| props.current))
                .and_then(|key| menu.link_regions.borrow().get(key).cloned())
        });
        let item_box = IndicatorProbe::layout_box(target.as_ref(), &menu.shared.root);
        let state = NavigationMenuIndicatorState {
            open: menu.value.get().is_some(),
            orientation: menu.orientation,
            visible: item_box.is_some(),
            item_box,
        };
        let part = Part::new(&menu.instance, "indicator", None, state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true));
        menu.indicator.watch(part, target, menu.shared.root.clone(), item_box)
    }
}

/// `agent: { actions: ["setValue", "close"], reveals: true }`.
impl Agent for NavigationMenu {
    const SURFACE: AgentSurface = AgentSurface::new().reveals(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |menu, args, window, cx| {
            let value = if args.is_missing(0) { None } else { Some(args.string(0)?) };
            Ok(menu.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("close", |menu, _args, window, cx| {
            Ok(menu.close(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new().with("value", self.value.get().as_ref().map(ToString::to_string))
    }
}
