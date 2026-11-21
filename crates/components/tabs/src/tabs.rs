//! Port of `components/tabs/tabs.ts`: panels in the same space, one shown at
//! a time, switched with a list of tabs.
//!
//! The list is one tab stop, on the selected tab. Arrow keys along its
//! orientation, Home and End move between tabs and select them, or only
//! move focus with manual activation, where Enter or Space selects. Disabled
//! tabs stay focusable but never select. Each panel is named by its tab and
//! shows while its tab is selected, animating in and out with its presence.
//! The indicator measures the selected tab inside the list.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, Entity, KeyDownEvent, Pixels, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, point, px,
};
use ui_sdk_disclosure::hidden_attrs;
use ui_sdk_focus::{
    Direction, Orientation, RovingFocus, RovingFocusOptions, RovingMember, TabStop, bind_tab_stop,
    orientation_of, scroll_to_show,
};
use ui_sdk_kernel::dev;
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::{HandledKey, Role};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentSurface, Aria, ChangeCallback, ChangeDetails, ChangeOptions,
    Controllable, InputEvent, Instance, Part, Reason, flag_while_bound, key_while_bound, press_of,
};
use ui_sdk_lists::{IconPlacement, ItemIconProps, ItemIconState, check_icon_item, icon_part};
use ui_sdk_motion::{Presence, PresenceState, PresenceTiming};
use ui_sdk_platform::{LayoutBox, LinkTarget, layout_box, opens_elsewhere};
use ui_sdk_selection::{Indicator, IndicatorOptions, bind_indicator, direction_between};

/// Why the selection changed: a tab was pressed (`ItemPress`), arrow keys
/// moved it (`ListNavigation`), the selected tab left the page
/// (`ItemRemoved`), or a call changed it (`Imperative`). `FindInPage` stays
/// for calls that pass it; gpui has no find in page.
pub type TabsValueChangeReason = Reason;

/// Options for every write method.
pub type TabsChangeOptions = ChangeOptions;

/// Which way the tabs run, which picks the arrow keys that move between them.
pub type TabsOrientation = Orientation;

/// Which way the selection moved, on screen.
pub type TabsDirection = Direction;

/// The props the `tab-icon` part takes: `mirror` flips it in right-to-left
/// text, and `content` is the icon itself.
pub type TabsTabIconProps = ItemIconProps;

/// What the `tab-icon` part's `with` reads: whether it mirrors, and whether
/// to flip it now.
pub type TabsTabIconState = ItemIconState;

// Where a tab's icon sits: in a tab, in the tab list.
const ICON: IconPlacement = IconPlacement {
    component: "tabs",
    item: Role::Tab,
    list: Some(Role::TabList),
};

/// When a tab that arrow keys focus is selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabsActivation {
    /// As soon as arrow keys focus it.
    #[default]
    Automatic,
    /// Once Enter or Space presses it.
    Manual,
}

impl TabsActivation {
    /// The original's name for it, like `"manual"`.
    pub fn as_str(self) -> &'static str {
        match self {
            TabsActivation::Automatic => "automatic",
            TabsActivation::Manual => "manual",
        }
    }
}

/// The options `Tabs` takes.
#[derive(Clone)]
pub struct TabsOptions {
    /// The selected tab's value. Pass it to control the tabs. `Some(None)`
    /// selects nothing.
    pub value: Option<Option<SharedString>>,
    /// The tab selected at first, when uncontrolled. Defaults to the first
    /// enabled tab, which never calls `on_value_change`.
    pub default_value: Option<SharedString>,
    /// Runs when the selection wants to change. `details.reason` is
    /// `ItemPress`, `ListNavigation`, `ItemRemoved` or `Imperative`, and
    /// `details.cancel()` stops it. It runs inside the tabs' update, so it
    /// may update any entity but the tabs.
    pub on_value_change: Option<ChangeCallback<Option<SharedString>>>,
    /// Which arrow keys move between tabs: Left and Right, or Up and Down.
    /// Horizontal when unset.
    pub orientation: Option<TabsOrientation>,
    /// `Automatic` selects a tab as soon as arrow keys focus it. `Manual`
    /// waits for Enter or Space.
    pub activation: TabsActivation,
    /// Whether arrow keys wrap from the last tab to the first. True by
    /// default.
    pub r#loop: bool,
    /// Keeps hidden panels in the page, hidden, as `keep_mounted` does. The
    /// original makes them findable with `hidden="until-found"`, which gpui
    /// has no find in page for.
    pub hidden_until_found: bool,
    /// How panels animate in and out, the transitions the original's CSS
    /// declares on them.
    pub panel_timing: PresenceTiming,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for TabsOptions {
    fn default() -> Self {
        Self {
            value: None,
            default_value: None,
            on_value_change: None,
            orientation: None,
            activation: TabsActivation::Automatic,
            r#loop: true,
            hidden_until_found: false,
            panel_timing: PresenceTiming::default(),
            debug: false,
        }
    }
}

impl TabsOptions {
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

    pub fn orientation(mut self, orientation: TabsOrientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    pub fn activation(mut self, activation: TabsActivation) -> Self {
        self.activation = activation;
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
        self
    }

    pub fn hidden_until_found(mut self, hidden_until_found: bool) -> Self {
        self.hidden_until_found = hidden_until_found;
        self
    }

    pub fn panel_timing(mut self, timing: PresenceTiming) -> Self {
        self.panel_timing = timing;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `tab` part takes beside its value, which pairs it with the
/// panel of the same value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabsTabProps {
    /// Keeps the tab focusable but stops it from being selected, as the tabs
    /// pattern asks.
    pub disabled: bool,
    /// Renders the tab as a link, which the app follows in its own press on
    /// the part (`on_press`), like a router's, and an arrow key that selects
    /// it clicks. It keeps its tab role. A press that opens it elsewhere,
    /// with a modifier key or another button, leaves the selection alone,
    /// and a disabled one calls `window.prevent_default()`, so the app's
    /// press doesn't follow it.
    pub link: Option<LinkTarget>,
}

impl TabsTabProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn link(mut self, link: LinkTarget) -> Self {
        self.link = Some(link);
        self
    }
}

/// The props the `panel` part takes beside its value, the value of the tab
/// it belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabsPanelProps {
    /// Whether the panel stays in the page, hidden, while its tab isn't
    /// selected.
    pub keep_mounted: bool,
    /// The tabs' `hidden_until_found` for this panel, which overrides it.
    pub hidden_until_found: Option<bool>,
}

impl TabsPanelProps {
    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn hidden_until_found(mut self, hidden_until_found: bool) -> Self {
        self.hidden_until_found = Some(hidden_until_found);
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabsState {
    /// The selected tab's value.
    pub value: Option<SharedString>,
    /// The tabs' `orientation` option, or horizontal when it's unset.
    pub orientation: TabsOrientation,
    /// Which way the last selection moved (`data-activation-direction`).
    pub direction: TabsDirection,
}

/// What the list's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabsListState {
    /// The tabs' `orientation` option, or horizontal when it's unset.
    pub orientation: TabsOrientation,
    /// Which way the last selection moved.
    pub direction: TabsDirection,
    /// Whether any tab has a `tab-icon` part (`data-has-icons`), which
    /// helps line up text and icon tabs.
    pub has_icons: bool,
}

/// What a tab's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabsTabState {
    /// Whether this tab's value is the tabs' value.
    pub selected: bool,
    /// Whether this tab can't be selected.
    pub disabled: bool,
    /// The tabs' orientation.
    pub orientation: TabsOrientation,
    /// Which way the last selection moved.
    pub direction: TabsDirection,
    /// Whether a `tab-icon` part is in it (`data-has-icon`).
    pub has_icon: bool,
}

/// What a panel's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabsPanelState {
    /// Whether this panel's tab is selected. False while it animates out.
    pub selected: bool,
    /// Kept in the page while its tab isn't selected, it renders hidden.
    pub hidden: bool,
    /// The tabs' orientation.
    pub orientation: TabsOrientation,
    /// Which way the last selection moved, for a slide in that direction.
    pub direction: TabsDirection,
    /// Its enter and exit (`data-starting-style`, `data-ending-style`).
    pub presence: PresenceState,
}

/// What the indicator's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabsIndicatorState {
    /// The tabs' orientation.
    pub orientation: TabsOrientation,
    /// Which way the last selection moved. Skip the indicator's animation
    /// while it's `None`, so its first placement doesn't slide in.
    pub direction: TabsDirection,
    /// The selected tab's box inside the list (`--active-tab-left`, `-right`,
    /// `-top`, `-bottom`, `-width`, `-height`), or `None` while nothing is
    /// selected or before the tabs lay out. Place the indicator from it.
    pub active_tab: Option<LayoutBox>,
}

// The keys the list handles, by orientation.
const HORIZONTAL_KEYS: &[HandledKey] = &["ArrowLeft", "ArrowRight", "Home", "End", "Enter", " "];
const VERTICAL_KEYS: &[HandledKey] = &["ArrowUp", "ArrowDown", "Home", "End", "Enter", " "];

// A panel in the page: its enter and exit, and whether it was showing when
// it last looked.
struct PanelEntry {
    presence: Presence,
    selected: bool,
}

/// Tabs (`tabs`). Its entity is its controller.
pub struct Tabs {
    instance: Instance,
    value: Controllable<Option<SharedString>>,
    // Kept in state, not read from options, so changing it renders the parts
    // again.
    orientation: TabsOrientation,
    activation: TabsActivation,
    r#loop: bool,
    hidden_until_found: bool,
    direction: TabsDirection,
    // Panels animating out. They stay present until their animation ends.
    exiting: Vec<SharedString>,
    // The tab that takes focus from Tab: the selected one, or the first
    // enabled one when the selected one is missing or disabled.
    roving: RovingFocus<SharedString>,
    // The props each tab rendered with last, which selection reads before
    // its tab has settled in the list.
    tab_props: RefCell<HashMap<SharedString, TabsTabProps>>,
    // The props each panel rendered with last, which decide whether it's in
    // the page.
    panel_props: RefCell<HashMap<SharedString, TabsPanelProps>>,
    panels: HashMap<SharedString, PanelEntry>,
    // Each panel's own tab stop, for while nothing inside it takes focus.
    panel_stops: RefCell<HashMap<SharedString, TabStop>>,
    panel_timing: PresenceTiming,
    // Measures the list and its tabs, for the indicator over the selected
    // one.
    indicator: Indicator<SharedString>,
    // The list's scroll, so a tab selected out of view comes into view.
    list_scroll: ScrollHandle,
    // Selections made after mount animate. The first paint never does.
    changed: bool,
    // The first check picks a default, which is not a change.
    checked: bool,
    // The default's tab can render after the others, so the first tab
    // stands in until it does, unless someone picks another.
    awaited: Option<SharedString>,
    stand_in: Option<SharedString>,
    // The value the last change saw.
    previous: Option<SharedString>,
}

impl Tabs {
    pub fn new(options: TabsOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut value = Controllable::new("value", options.default_value.clone());
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        let mut instance = Instance::new("tabs").with_agent(cx);
        instance.debug = options.debug;
        let orientation = orientation_of(options.orientation, Orientation::Horizontal);
        // Disabled tabs stay focusable, as the tabs pattern asks. Tab lands
        // on the selected one; before the tabs settle, the selected tab holds
        // the stop, or with nothing selected the first enabled one claims it.
        // Once they've settled, or come and gone, the selection is checked.
        let this = cx.weak_entity();
        let roving = RovingFocus::new(
            RovingFocusOptions::default()
                .orientation(orientation)
                .r#loop(options.r#loop)
                .remember(false)
                .on_items_change(move |window, cx| {
                    this.update(cx, |tabs, cx| tabs.check(window, cx)).ok();
                }),
            window,
            cx,
        );
        let list_scroll = ScrollHandle::new();
        // The indicator inside the list scrolls with the tabs.
        let indicator = bind_indicator(IndicatorOptions::new().scroll(&list_scroll), cx);
        let previous = value.get().clone();
        Self {
            instance,
            value,
            orientation,
            activation: options.activation,
            r#loop: options.r#loop,
            hidden_until_found: options.hidden_until_found,
            direction: Direction::None,
            exiting: Vec::new(),
            roving,
            tab_props: RefCell::new(HashMap::new()),
            panel_props: RefCell::new(HashMap::new()),
            panels: HashMap::new(),
            panel_stops: RefCell::new(HashMap::new()),
            panel_timing: options.panel_timing,
            indicator,
            list_scroll,
            changed: false,
            checked: false,
            awaited: options.default_value,
            stand_in: None,
            previous,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A controlled `value` brings which way it moved and the
    /// panel it leaves animating out in the same pass.
    pub fn set_options(&mut self, options: TabsOptions, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = self.value.set_controlled(options.value);
        self.value.set_on_change(options.on_value_change);
        let orientation = orientation_of(options.orientation, Orientation::Horizontal);
        changed |= self.orientation != orientation
            || self.activation != options.activation
            || self.r#loop != options.r#loop
            || self.hidden_until_found != options.hidden_until_found;
        self.orientation = orientation;
        self.activation = options.activation;
        self.r#loop = options.r#loop;
        self.hidden_until_found = options.hidden_until_found;
        self.roving.set_orientation(orientation);
        self.roving.set_loop(options.r#loop);
        for entry in self.panels.values() {
            entry.presence.set_timing(options.panel_timing.clone());
        }
        self.panel_timing = options.panel_timing;
        self.instance.debug = options.debug;
        self.after_change(window, cx);
        self.sync_panels(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, `Some(None)` to select nothing, or `None`
    /// to let the tabs keep their own.
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

    pub fn state(&self) -> TabsState {
        TabsState {
            value: self.value.get().clone(),
            orientation: self.orientation,
            direction: self.direction,
        }
    }

    /// The selected tab's value, or `None` when nothing is selected.
    pub fn get_value(&self) -> Option<&SharedString> {
        self.value.get().as_ref()
    }

    /// Which way the tabs run.
    pub fn orientation(&self) -> TabsOrientation {
        self.orientation
    }

    /// Which way the last selection moved.
    pub fn direction(&self) -> TabsDirection {
        self.direction
    }

    /// Whether a selection was made since the tabs were made, which animates
    /// the panels.
    pub fn is_changed(&self) -> bool {
        self.changed
    }

    /// The list's scroll handle, which the list part tracks.
    pub fn list_scroll_handle(&self) -> &ScrollHandle {
        &self.list_scroll
    }

    fn tab_props_of(&self, key: &SharedString) -> Option<TabsTabProps> {
        self.tab_props.borrow().get(key).cloned()
    }

    // What a tab tells the roving group as it renders: disabled, preferred
    // while selected, and, before the stop is decided, whether it may hold
    // it: the selected tab, or with nothing selected the first enabled one.
    fn member(&self, key: &SharedString, props: &TabsTabProps) -> RovingMember {
        let value = self.value.get();
        RovingMember::new()
            .disabled(props.disabled)
            .preferred(value.as_ref() == Some(key))
            .claims(match value {
                None => !props.disabled,
                Some(value) => value == key,
            })
    }

    /// Whether the tab `key` shows as selected. Until the tabs settle,
    /// uncontrolled tabs with nothing selected show the tab that claims the
    /// stop as selected, which is the one the first check selects, so the
    /// first frame already shows its panel (`shows`).
    pub fn shows(&self, key: &SharedString) -> bool {
        let value = self.value.get();
        if self.checked || value.is_some() || self.value.is_controlled() {
            return value.as_ref() == Some(key);
        }
        let Some(props) = self.tab_props_of(key) else {
            return false;
        };
        self.roving.tab_index(key, &self.member(key, &props))
    }

    // A tab's bounds as of its last layout.
    fn tab_bounds(&self, key: &SharedString) -> Option<Bounds<Pixels>> {
        self.roving
            .items()
            .into_iter()
            .find(|item| &item.key == key)
            .and_then(|item| item.bounds)
    }

    // A tab's box on screen, for which way the selection moved.
    fn tab_rect(&self, key: Option<&SharedString>) -> Option<LayoutBox> {
        let bounds = self.tab_bounds(key?)?;
        Some(layout_box(bounds, Bounds::default(), None))
    }

    // Runs once the tabs have settled, or came or went. Uncontrolled tabs
    // always select something, so the list never loses its tab stop.
    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = self.roving.items();
        if list.is_empty() {
            return;
        }
        let initial = !self.checked;
        self.checked = true;
        let has = |key: &Option<SharedString>| key.as_ref().is_some_and(|key| list.iter().any(|tab| &tab.key == key));
        let controlled = self.value.is_controlled();
        if controlled {
            self.awaited = None;
        }
        if let Some(awaited) = self.awaited.clone()
            && has(&Some(awaited.clone()))
        {
            self.awaited = None;
            self.stand_in = None;
            if self.value.set_held(Some(awaited)) {
                self.after_change(window, cx);
            }
        }
        let value = self.value.get().clone();
        if !controlled && !has(&value) {
            let Some(first) = list.iter().find(|tab| !tab.props.disabled).map(|tab| tab.key.clone()) else {
                cx.notify();
                return;
            };
            // The first pick is the default, so it calls nothing. A selected
            // tab that leaves the page later is a change.
            if initial {
                self.stand_in = self.awaited.as_ref().map(|_| first.clone());
                if self.value.set_held(Some(first)) {
                    self.after_change(window, cx);
                }
            } else {
                self.change(Some(first), ChangeOptions::reason(Reason::ItemRemoved), window, cx);
            }
        }
        // The stop and the panels render from the settled tabs.
        cx.notify();
    }

    // What the original's store subscription does: hears the selection
    // change from any source, a press, a key, a call or a controlled value,
    // before anything renders it.
    fn after_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.value.get().clone();
        if value == self.previous {
            return;
        }
        let from = std::mem::replace(&mut self.previous, value.clone());
        if value != self.stand_in {
            self.awaited = None;
        }
        self.changed = true;
        let (direction, exiting) = self.selecting(from.as_ref(), value.as_ref());
        self.direction = direction;
        self.exiting = exiting;
        // The tab stop moves to the new selection at once.
        self.roving.check_with(|tab| value.as_ref() == Some(&tab.key), cx);
        if let Some(value) = &value {
            self.reveal(value);
        }
        self.sync_panels(window, cx);
        cx.notify();
    }

    // What selecting `value` brings with it: the direction the selection
    // moves, and the panel it leaves animating out.
    fn selecting(&self, from: Option<&SharedString>, value: Option<&SharedString>) -> (Direction, Vec<SharedString>) {
        let direction = direction_between(
            self.tab_rect(from).as_ref(),
            self.tab_rect(value).as_ref(),
            self.orientation,
        );
        let mut exiting: Vec<SharedString> = self
            .exiting
            .iter()
            .filter(|key| Some(*key) != value)
            .cloned()
            .collect();
        if let Some(from) = from
            && self.panels.contains_key(from)
            && !self.exiting.contains(from)
        {
            exiting.push(from.clone());
        }
        (direction, exiting)
    }

    // Brings a tab into view inside a scrolling list. The window itself
    // never scrolls. gpui doesn't scroll what takes focus, so this runs for
    // every selection and for each tab arrow keys focus.
    fn reveal(&self, key: &SharedString) {
        let scroll = &self.list_scroll;
        let max = scroll.max_offset();
        if max.x <= px(0.) && max.y <= px(0.) {
            return;
        }
        let Some(tab) = self.tab_bounds(key) else {
            return;
        };
        let list = scroll.bounds();
        let rect = layout_box(tab, list, Some(scroll));
        let offset = scroll.offset();
        let left = scroll_to_show(-offset.x, list.size.width, rect.left, rect.width, px(0.), px(0.));
        let top = scroll_to_show(-offset.y, list.size.height, rect.top, rect.height, px(0.), px(0.));
        scroll.set_offset(point(-left.min(max.x), -top.min(max.y)));
    }

    fn change(
        &mut self,
        value: Option<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        let changed = self.value.change(&self.instance, value, options, window, cx);
        if changed {
            self.after_change(window, cx);
            cx.notify();
        }
        changed
    }

    /// Selects a tab, or nothing with `None`. Disabled tabs never select.
    /// Returns false when canceled.
    pub fn set_value(
        &mut self,
        value: Option<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(key) = &value
            && self.tab_props_of(key).is_some_and(|props| props.disabled)
        {
            return false;
        }
        self.change(value, options, window, cx)
    }

    /// Selects a tab, the same as `set_value`.
    pub fn select(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.set_value(Some(value.into()), options, window, cx)
    }

    /// Moves focus to the tab that holds the tab stop, the selected one
    /// unless it left.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.roving.focus(None, window, cx);
    }

    // Keys on the list, from the tab that has focus.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // A handler of the app's that prevented the key keeps it.
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented() || modifiers.alt || modifiers.control || modifiers.platform || modifiers.shift {
            return;
        }
        let Some(current) = self.roving.focused(window) else {
            return;
        };
        // gpui clicks the focused tab on Enter and Space, which presses it.
        let key = ui_sdk_kernel::Key::of(&event.keystroke);
        if key == ui_sdk_kernel::Key::Enter || key == ui_sdk_kernel::Key::Space {
            return;
        }
        let roving = self.roving.clone();
        let Some(moved) = roving.keydown(event, &current, window, cx) else {
            return;
        };
        if moved.key == moved.from {
            return;
        }
        self.reveal(&moved.key);
        let Some(next) = roving.items().into_iter().find(|tab| tab.key == moved.key) else {
            return;
        };
        if self.activation == TabsActivation::Manual || next.props.disabled {
            return;
        }
        // Links follow themselves, so routers and history see the
        // navigation: the key clicks a link tab, whose press still reports
        // the key's navigation.
        let link = self.tab_props_of(&next.key).is_some_and(|props| props.link.is_some());
        if link && let Some(press) = press_of(&self.instance, "tab", Some(&next.key)) {
            press.click(InputEvent::from(event), window, cx);
        } else {
            self.set_value(Some(next.key), ChangeOptions::event(Reason::ListNavigation, event), window, cx);
        }
    }

    // A press on a tab: a disabled one refuses it, and a link's press that
    // opens it elsewhere leaves the selection alone.
    fn press(&mut self, key: &SharedString, event: InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        let props = self.tab_props_of(key).unwrap_or_default();
        if props.disabled {
            // A disabled link must not navigate either.
            window.prevent_default();
            return;
        }
        if opens_elsewhere(Some(&event), props.link.as_ref()) {
            return;
        }
        // A press that moved no focus, like a screen reader's, would leave
        // the arrow keys nowhere.
        let handle = self.roving.focus_handle(key, cx);
        if !handle.is_focused(window) {
            window.focus(&handle, cx);
        }
        // How a press selects: as a press, or as the arrow key that clicked a
        // link tab (`pressDetails`).
        let reason = match event {
            InputEvent::KeyDown(_) => Reason::ListNavigation,
            _ => Reason::ItemPress,
        };
        self.set_value(Some(key.clone()), ChangeOptions::event_of(reason, event), window, cx);
    }

    fn findable(&self, props: &TabsPanelProps) -> bool {
        props.hidden_until_found.unwrap_or(self.hidden_until_found)
    }

    // Whether a panel is in the page: kept, showing, or animating out.
    fn panel_present(&self, key: &SharedString, props: &TabsPanelProps) -> bool {
        props.keep_mounted || self.findable(props) || self.shows(key) || self.exiting.contains(key)
    }

    // The panels that may need binding: rendered, selected, leaving or bound.
    fn panel_keys(&self) -> Vec<SharedString> {
        let mut keys: Vec<SharedString> = self.panel_props.borrow().keys().cloned().collect();
        for key in self
            .value
            .get()
            .iter()
            .chain(self.exiting.iter())
            .chain(self.panels.keys())
        {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        keys
    }

    fn sync_panels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for key in self.panel_keys() {
            self.sync_panel(&key, window, cx);
        }
    }

    // Binds, checks or unbinds one panel for the state now: enter and exit
    // animations, never on the first paint.
    fn sync_panel(&mut self, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(props) = self.panel_props.borrow().get(key).copied() else {
            return;
        };
        let present = self.panel_present(key, &props);
        let selected = self.shows(key);
        let Some(entry) = self.panels.get_mut(key) else {
            if !present {
                return;
            }
            let animate = selected && self.changed;
            let presence = Presence::new(selected && !animate).with_timing(self.panel_timing.clone());
            if animate {
                presence.animate_in(window, cx);
            }
            self.panels.insert(key.clone(), PanelEntry { presence, selected });
            return;
        };
        if entry.selected != selected {
            entry.selected = selected;
            let presence = entry.presence.clone();
            presence.stop(cx);
            if selected {
                presence.animate_in(window, cx);
            } else if self.exiting.contains(key) {
                let exited = key.clone();
                presence.animate_out(self, move |tabs, window, cx| tabs.finish_exit(&exited, window, cx), window, cx);
                return;
            } else {
                presence.set_open(false, cx);
            }
        }
        if !self.panel_present(key, &props) {
            self.unbind_panel(key, cx);
        }
    }

    // A panel left the page: what runs stops, and it no longer animates out.
    fn unbind_panel(&mut self, key: &SharedString, cx: &mut Context<Self>) {
        if let Some(entry) = self.panels.remove(key) {
            entry.presence.stop(cx);
        }
        self.panel_stops.borrow_mut().remove(key);
        self.exiting.retain(|value| value != key);
    }

    fn finish_exit(&mut self, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.exiting.retain(|value| value != key);
        cx.notify();
        self.sync_panel(key, window, cx);
    }

    /// The `root` part, around the list and the panels.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<TabsState> {
        let tabs = this.read(cx);
        Part::new(&tabs.instance, "root", None, tabs.state())
    }

    /// The `list` part, around the tabs: a tablist, which Left and Right
    /// move through, or Up and Down when it's vertical, with Home and End to
    /// the first and last tab. Name it with `aria_label`. Build it before
    /// the tabs: it starts their render, so the order they render in is page
    /// order. It measures the box the indicator covers, and a list made to
    /// scroll brings a tab selected or focused out of view into view.
    pub fn list(this: &Entity<Self>, cx: &App) -> Part<TabsListState> {
        let tabs = this.read(cx);
        tabs.roving.begin(cx);
        let state = TabsListState {
            orientation: tabs.orientation,
            direction: tabs.direction,
            has_icons: flag_while_bound(&tabs.instance, "tab-icon").unwrap_or(false),
        };
        let keys = match tabs.orientation {
            Orientation::Horizontal => HORIZONTAL_KEYS,
            Orientation::Vertical => VERTICAL_KEYS,
        };
        let entity = this.downgrade();
        let mut part = Part::new(&tabs.instance, "list", None, state)
            .aria_role(Role::TabList)
            .on_keys(keys, move |event, window, cx| {
                entity.update(cx, |tabs, cx| tabs.on_key_down(event, window, cx)).ok();
            })
            .track_scroll(&tabs.list_scroll);
        if tabs.orientation == Orientation::Vertical {
            part = part.aria_orientation(Orientation::Vertical);
        }
        tabs.indicator.list(part)
    }

    /// A `tab` part for `value`, which pairs it with the panel of the same
    /// value: one tab stop for the list, on the selected tab, and it
    /// controls its panel while that's in the page. Values identify the
    /// tabs, so each is unique. Name it with `aria_label`.
    pub fn tab(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: TabsTabProps,
        cx: &App,
    ) -> Part<TabsTabState> {
        let tabs = this.read(cx);
        let key: SharedString = value.into();
        tabs.tab_props.borrow_mut().insert(key.clone(), props.clone());
        let selected = tabs.shows(&key);
        let handle = tabs.roving.focus_handle(&key, cx);
        let tab_stop = tabs.roving.bind(key.clone(), &handle, tabs.member(&key, &props), cx);
        let state = TabsTabState {
            selected,
            disabled: props.disabled,
            orientation: tabs.orientation,
            direction: tabs.direction,
            has_icon: key_while_bound(&tabs.instance, "tab-icon").contains(&key),
        };
        let entity = this.downgrade();
        let pressed = key.clone();
        let mut part = Part::new(&tabs.instance, "tab", Some(&key), state)
            .aria_role(Role::Tab)
            .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
            .aria_selected(selected)
            .aria(
                Aria::new()
                    .disabled(props.disabled)
                    .controls(tabs.instance.element_id("panel", Some(&key))),
            );
        // A tab that shows only an icon names nothing.
        if dev::DEV && state.has_icon {
            let entity = entity.clone();
            let key = key.clone();
            part = part.keep_name().on_props(move |window, cx| {
                if let Some(tabs) = entity.upgrade() {
                    check_icon_item(&tabs.read(cx).instance, "tab", Some(&key), ICON, window);
                }
            });
        }
        let part = part.on_press(move |event, window, cx| {
            entity.update(cx, |tabs, cx| tabs.press(&pressed, event, window, cx)).ok();
        });
        let part = tabs.roving.measure(key.clone(), part);
        tabs.indicator.item(key, part)
    }

    /// The `tab-icon` part of the tab with `value`: an icon inside the tab,
    /// which screen readers skip, so the tab's name stays its label. An
    /// icon-only tab still needs an `aria_label`. The tab and the list know
    /// it has one. `mirror` flips it in right-to-left text, where its
    /// state's `flipped` says to.
    pub fn tab_icon(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: TabsTabIconProps,
        cx: &App,
    ) -> Part<TabsTabIconState> {
        let key: SharedString = value.into();
        icon_part(&this.read(cx).instance, "tab-icon", Some(&key), &props, cx)
    }

    /// The `indicator` part: a marker over the selected tab, which screen
    /// readers skip. Render it inside the list and place it from its state's
    /// `active_tab`. It measures again when a tab or the list changes size.
    pub fn indicator(this: &Entity<Self>, cx: &App) -> Part<TabsIndicatorState> {
        let tabs = this.read(cx);
        let state = TabsIndicatorState {
            orientation: tabs.orientation,
            direction: tabs.direction,
            active_tab: tabs.indicator.layout_box(tabs.value.get().as_ref()),
        };
        Part::new(&tabs.instance, "indicator", None, state).aria_role(Role::Presentation)
    }

    /// The `panel` part for `value`, or `None` while it's out of the page: a
    /// tabpanel named by its tab, shown while its tab is selected and while
    /// it animates out, and kept in the page, hidden, with `keep_mounted`.
    /// It's a tab stop only while nothing inside it can take focus, so a
    /// panel that opens with a field doesn't add a stop before it.
    pub fn panel(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: TabsPanelProps,
        cx: &App,
    ) -> Option<Part<TabsPanelState>> {
        let tabs = this.read(cx);
        let key: SharedString = value.into();
        // A render with new props, like `keep_mounted`, checks the panel
        // again.
        let stale = tabs.panel_props.borrow_mut().insert(key.clone(), props) != Some(props);
        if !tabs.panel_present(&key, &props) {
            return None;
        }
        let selected = tabs.shows(&key);
        let shown = selected || tabs.exiting.contains(&key);
        let hidden = hidden_attrs(shown, tabs.findable(&props));
        let entry = tabs.panels.get(&key);
        let presence = entry.map(|entry| entry.presence.state()).unwrap_or_default();
        let state = TabsPanelState {
            selected,
            hidden,
            orientation: tabs.orientation,
            direction: tabs.direction,
            presence,
        };
        let stop = tabs
            .panel_stops
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| bind_tab_stop(cx.focus_handle()))
            .clone();
        let mut part = Part::new(&tabs.instance, "panel", Some(&key), state)
            .aria_role(Role::TabPanel)
            .aria(
                Aria::new()
                    .labelled_by(tabs.instance.element_id("tab", Some(&key)))
                    .hidden(hidden),
            );
        // Animating out, it takes no focus.
        part = stop.bind(part, !presence.inert);
        if hidden {
            part = part.hidden();
        }
        if stale || entry.is_none() {
            let entity = this.downgrade();
            part = part.on_props(move |window, cx| {
                entity.update(cx, |tabs, cx| tabs.sync_panel(&key, window, cx)).ok();
            });
        }
        Some(part)
    }
}

/// `agent: { actions: ["setValue"], closed: true, reveals: true }`, with
/// `select`, which `setValue` covers, unlisted.
impl Agent for Tabs {
    const SURFACE: AgentSurface = AgentSurface::new().closed(true).reveals(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |tabs, args, window, cx| {
            let value = if args.is_missing(0) { None } else { Some(args.string(0)?) };
            Ok(tabs.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("select", |tabs, args, window, cx| {
            let value = args.string(0)?;
            Ok(tabs.select(value, ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new().with("value", self.value.get().as_ref().map(ToString::to_string))
    }
}
