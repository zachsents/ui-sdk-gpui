//! Port of `components/toggle/toggle-group.ts`: toggles that share one
//! value, with one tab stop and arrow keys between them.
//!
//! The group keeps the value and the tab stop; each toggle is its own
//! [`Toggle`](crate::Toggle), made with the group in its `group` option,
//! which renders from the group and registers with it as it renders. A
//! group inside a toolbar puts its toggles in the toolbar's tab stop instead
//! of its own.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, Context, Entity, FocusHandle, KeyDownEvent, SharedString, Window};
use ui_sdk_focus::{
    Orientation, RovingFocus, RovingFocusOptions, RovingMember, RovingOrientation, RovingScope,
    in_order, orientation_of,
};
use ui_sdk_forms::{Node, watch_form_disabled};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentSurface, ChangeCallback, ChangeDetails, ChangeOptions,
    Controllable, Instance, Part, Parts, Reason,
};

/// Why the pressed toggles changed: a toggle was pressed (`ItemPress`), or a
/// call changed them (`Imperative`).
pub type ToggleGroupValueChangeReason = Reason;

/// Options for every write method.
pub type ToggleGroupChangeOptions = ChangeOptions;

/// Which way the toggles run, which picks the arrow keys that move between
/// them.
pub type ToggleGroupOrientation = Orientation;

/// The options `ToggleGroup` takes.
#[derive(Clone)]
pub struct ToggleGroupOptions {
    /// The pressed toggles' values. Pass it to control the group.
    pub value: Option<Vec<SharedString>>,
    /// The toggles pressed at first, when uncontrolled.
    pub default_value: Vec<SharedString>,
    /// Runs when a toggle wants to be pressed or unpressed. `details.reason`
    /// is `ItemPress` or `Imperative`, and `details.cancel()` stops it. It
    /// runs inside the group's update, and inside the toggle's for a press,
    /// so it may update any entity but those two.
    pub on_value_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Whether several toggles can be pressed at once. Otherwise pressing
    /// one unpresses the others.
    pub multiple: bool,
    /// Whether the pressed toggle can be unpressed when only one may be
    /// pressed. With `false`, one toggle stays pressed once any is. Ignored
    /// with `multiple`. True by default.
    pub deselectable: bool,
    /// Stops every toggle from being pressed. They stay focusable and report
    /// themselves disabled, so the group keeps its tab stop and stays
    /// discoverable.
    pub disabled: bool,
    /// Which arrow keys move between toggles: Left and Right, or Up and
    /// Down. Horizontal when unset.
    pub orientation: Option<ToggleGroupOrientation>,
    /// Whether arrow keys wrap from the last toggle to the first. True by
    /// default.
    pub r#loop: bool,
    /// The toolbar the group sits in, from `Toolbar::scope` or `scope_in`,
    /// whose arrow keys and tab stop its toggles join (`toolbar`).
    pub toolbar: Option<Rc<dyn RovingScope>>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for ToggleGroupOptions {
    fn default() -> Self {
        Self {
            value: None,
            default_value: Vec::new(),
            on_value_change: None,
            multiple: false,
            deselectable: true,
            disabled: false,
            orientation: None,
            r#loop: true,
            toolbar: None,
            debug: false,
        }
    }
}

impl ToggleGroupOptions {
    pub fn value(mut self, value: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.value = Some(value.into_iter().map(Into::into).collect());
        self
    }

    pub fn default_value(mut self, value: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.default_value = value.into_iter().map(Into::into).collect();
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    pub fn deselectable(mut self, deselectable: bool) -> Self {
        self.deselectable = deselectable;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn orientation(mut self, orientation: ToggleGroupOrientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
        self
    }

    pub fn toolbar(mut self, toolbar: Rc<dyn RovingScope>) -> Self {
        self.toolbar = Some(toolbar);
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the group part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToggleGroupState {
    /// The pressed toggles' values.
    pub value: Vec<SharedString>,
    /// Whether several toggles can be pressed at once.
    pub multiple: bool,
    /// Whether the whole group is disabled, by its `disabled` option, a
    /// disabled fieldset around it or the toolbar it sits in.
    pub disabled: bool,
    /// The group's `orientation` option, or horizontal when it's unset.
    pub orientation: ToggleGroupOrientation,
    /// Whether any toggle in it has an `icon` part (`data-has-icons`), as of
    /// the toggles' last render.
    pub has_icons: bool,
}

/// The value after pressing or unpressing one toggle, in page order
/// (`nextValue`).
pub fn next_value(
    value: &[SharedString],
    key: &SharedString,
    pressed: bool,
    multiple: bool,
    order: &[SharedString],
) -> Vec<SharedString> {
    if !multiple {
        return if pressed { vec![key.clone()] } else { Vec::new() };
    }
    let mut others: Vec<SharedString> = value.iter().filter(|item| *item != key).cloned().collect();
    if pressed {
        others.push(key.clone());
    }
    in_order(&others, order)
}

// A toggle in the group, as it last rendered: the key it joins the roving
// group by, which stays unique in a toolbar that holds several groups, its
// focus handle, and whether it's disabled.
#[derive(Clone)]
struct ToggleItem {
    roving_key: SharedString,
    focus: FocusHandle,
    disabled: bool,
    has_icon: bool,
}

// The group's own tab stop, which its toggles join while no toolbar holds
// theirs.
struct GroupScope {
    roving: RovingFocus<SharedString>,
    orientation: Cell<Orientation>,
    // Whether the whole group is disabled, as of its last render.
    disabled: Cell<bool>,
}

impl RovingScope for GroupScope {
    fn roving(&self) -> &RovingFocus<SharedString> {
        &self.roving
    }

    fn orientation(&self, _cx: &App) -> RovingOrientation {
        self.orientation.get()
    }

    fn disabled(&self, _cx: &App) -> bool {
        self.disabled.get()
    }

    // Shift with an arrow belongs to whatever extends a selection.
    fn keydown(&self, event: &KeyDownEvent, key: &SharedString, window: &mut Window, cx: &mut App) {
        if !event.keystroke.modifiers.shift {
            self.roving.keydown(event, key, window, cx);
        }
    }
}

// What a toolbar's check looks at, as of the last change.
#[derive(Clone, PartialEq)]
struct Checked {
    value: Vec<SharedString>,
    disabled: bool,
    scope_disabled: bool,
}

/// A toggle group (`toggleGroup`). Its entity is its controller.
pub struct ToggleGroup {
    instance: Instance,
    value: Controllable<Vec<SharedString>>,
    multiple: bool,
    deselectable: bool,
    disabled: bool,
    orientation: ToggleGroupOrientation,
    r#loop: bool,
    toolbar: Option<Rc<dyn RovingScope>>,
    // The group's place in the page, so a disabled fieldset around it
    // disables it.
    node: Node,
    own: Rc<GroupScope>,
    // The toggles by value, in page order.
    items: Parts<SharedString, ToggleItem>,
    previous: Checked,
}

impl ToggleGroup {
    pub fn new(options: ToggleGroupOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut value = Controllable::new("value", options.default_value.clone());
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        // The group's primitive is named "toggle", so its part is
        // "toggle-group".
        let mut instance = Instance::new("toggle").with_agent(cx);
        instance.debug = options.debug;
        let orientation = orientation_of(options.orientation, Orientation::Horizontal);
        // Tab lands on the first pressed toggle that's enabled, else the
        // first enabled one, until focus has been in the group. Then it's the
        // one focused last, like the APG toolbar. Before toggles settle, the
        // first pressed enabled toggle claims the stop, or with nothing
        // pressed the first enabled one. In a disabled group every toggle
        // stays focusable, so the first one does, as the settled stop
        // decides.
        let roving = RovingFocus::new(
            RovingFocusOptions::default()
                .orientation(orientation)
                .r#loop(options.r#loop),
            window,
            cx,
        );
        let own = Rc::new(GroupScope {
            roving,
            orientation: Cell::new(orientation),
            disabled: Cell::new(options.disabled),
        });
        let node = watch_form_disabled(cx);
        let mut group = Self {
            instance,
            value,
            multiple: options.multiple,
            deselectable: options.deselectable,
            disabled: options.disabled,
            orientation,
            r#loop: options.r#loop,
            toolbar: options.toolbar,
            node,
            own,
            items: Parts::new(),
            previous: Checked {
                value: Vec::new(),
                disabled: false,
                scope_disabled: false,
            },
        };
        group.previous = group.checked(cx);
        group
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: ToggleGroupOptions, cx: &mut Context<Self>) {
        let mut changed = self.value.set_controlled(options.value);
        self.value.set_on_change(options.on_value_change);
        let orientation = orientation_of(options.orientation, Orientation::Horizontal);
        changed |= self.multiple != options.multiple
            || self.deselectable != options.deselectable
            || self.disabled != options.disabled
            || self.orientation != orientation
            || self.r#loop != options.r#loop
            || self.toolbar.is_some() != options.toolbar.is_some();
        self.multiple = options.multiple;
        self.deselectable = options.deselectable;
        self.disabled = options.disabled;
        self.orientation = orientation;
        self.r#loop = options.r#loop;
        self.toolbar = options.toolbar;
        self.instance.debug = options.debug;
        self.own.roving.set_orientation(orientation);
        self.own.roving.set_loop(options.r#loop);
        self.own.orientation.set(orientation);
        self.own.disabled.set(self.is_disabled(cx));
        self.after_change(cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the group keep its own.
    pub fn set_controlled_value(&mut self, value: Option<Vec<SharedString>>, cx: &mut Context<Self>) {
        if self.value.set_controlled(value) {
            cx.notify();
        }
        self.after_change(cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub fn state(&self, cx: &App) -> ToggleGroupState {
        ToggleGroupState {
            value: self.value.get().clone(),
            multiple: self.multiple,
            disabled: self.is_disabled(cx),
            orientation: self.orientation,
            has_icons: self.items.entries().iter().any(|(_, item)| item.has_icon),
        }
    }

    /// The pressed toggles' values, in the order they appear.
    pub fn get_value(&self) -> &[SharedString] {
        self.value.get()
    }

    /// Whether the toggle with `value` is pressed.
    pub fn is_pressed(&self, value: &str) -> bool {
        self.value.get().iter().any(|item| item.as_ref() == value)
    }

    /// Whether several toggles can be pressed at once.
    pub fn is_multiple(&self) -> bool {
        self.multiple
    }

    /// Whether the whole group is disabled, by its `disabled` option, a
    /// disabled fieldset around it or the toolbar it sits in. A disabled
    /// toolbar disables the groups in it (`isGroupDisabled`).
    pub fn is_disabled(&self, cx: &App) -> bool {
        self.disabled || self.node.form_disabled() || self.scope_disabled(cx)
    }

    /// Which way the toggles run.
    pub fn orientation(&self) -> ToggleGroupOrientation {
        self.orientation
    }

    // Whether the toolbar the group sits in is disabled.
    fn scope_disabled(&self, cx: &App) -> bool {
        self.toolbar.as_ref().is_some_and(|toolbar| toolbar.disabled(cx))
    }

    /// The roving group its toggles join: the toolbar's it sits in, or its
    /// own. A group in a toolbar is no tab stop of its own.
    pub fn scope(&self) -> Rc<dyn RovingScope> {
        match &self.toolbar {
            Some(toolbar) => toolbar.clone(),
            None => self.own.clone(),
        }
    }

    /// Whether a toolbar around the group holds the tab stop.
    pub fn in_toolbar(&self) -> bool {
        self.toolbar.is_some()
    }

    fn checked(&self, cx: &App) -> Checked {
        Checked {
            value: self.value.get().clone(),
            disabled: self.is_disabled(cx),
            scope_disabled: self.scope_disabled(cx),
        }
    }

    // A toolbar around the group checks its stop when toggles in it change.
    // The group's own stop follows what's pressed as its toggles render.
    fn after_change(&mut self, cx: &mut Context<Self>) {
        let now = self.checked(cx);
        if now == self.previous {
            return;
        }
        self.previous = now;
        if let Some(toolbar) = &self.toolbar {
            toolbar.queue_check(cx);
        }
    }

    fn change(
        &mut self,
        value: Vec<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let changed = self.value.change(&self.instance, value, options, window, cx);
        if changed {
            cx.notify();
            self.after_change(cx);
        }
        changed
    }

    /// Whether pressing or unpressing a toggle would change anything the
    /// group allows: not while it or the toggle is disabled, not to the
    /// state it's in, and not unpressing the last one unless `multiple` or
    /// `deselectable` (`allows`).
    pub fn allows(&self, value: &str, pressed: bool, cx: &App) -> bool {
        if self.is_disabled(cx) || self.items.info(&SharedString::from(value.to_string())).is_some_and(|item| item.disabled) {
            return false;
        }
        if pressed == self.is_pressed(value) {
            return false;
        }
        pressed || self.multiple || self.deselectable
    }

    /// Presses or unpresses the toggle with `value` for the toggle itself,
    /// which heard its own `on_pressed_change` first (`press`).
    pub fn press(
        &mut self,
        value: SharedString,
        pressed: bool,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.allows(&value, pressed, cx) {
            return false;
        }
        let order = self.items.keys();
        let next = next_value(self.value.get(), &value, pressed, self.multiple, &order);
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        self.change(next, options, window, cx)
    }

    /// Sets the pressed toggles. Only the first counts unless `multiple` is
    /// set. Returns false when canceled.
    pub fn set_value(
        &mut self,
        value: Vec<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let value = if self.multiple {
            in_order(&value, &self.items.keys())
        } else {
            value.into_iter().take(1).collect()
        };
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        self.change(value, options, window, cx)
    }

    /// Presses a toggle, or unpresses a pressed one, as a user would.
    /// Disabled toggles and `deselectable(false)` refuse it.
    pub fn toggle(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let value = value.into();
        let pressed = !self.is_pressed(&value);
        self.press(value, pressed, options, window, cx)
    }

    /// Moves focus to a toggle, or to the one that holds the group's tab
    /// stop.
    pub fn focus(&self, value: Option<&str>, window: &mut Window, cx: &mut App) {
        match value {
            Some(value) => {
                if let Some(item) = self.items.info(&SharedString::from(value.to_string())) {
                    window.focus(&item.focus, cx);
                }
            }
            None if self.toolbar.is_none() => self.own.roving.focus(None, window, cx),
            None => {
                if let Some((_, item)) = self.items.entries().into_iter().next() {
                    window.focus(&item.focus, cx);
                }
            }
        }
    }

    /// Joins a toggle as it renders, by its value, and the roving group by
    /// its own key, with the focus handle its element tracks, whether it's
    /// disabled and whether it has an icon. Returns whether it holds the tab
    /// stop (`register`).
    pub fn register(
        &self,
        value: SharedString,
        roving_key: SharedString,
        handle: &FocusHandle,
        disabled: bool,
        has_icon: bool,
        cx: &App,
    ) -> bool {
        self.items.register(
            value.clone(),
            ToggleItem {
                roving_key: roving_key.clone(),
                focus: handle.clone(),
                disabled,
                has_icon,
            },
        );
        let preferred = self.is_pressed(&value);
        let member = RovingMember::new().disabled(disabled).preferred(preferred);
        let member = match &self.toolbar {
            // The toolbar's own rules decide its first stop.
            Some(_) => member,
            None => member.claims(self.is_disabled(cx) || (!disabled && (self.value.get().is_empty() || preferred))),
        };
        self.scope().bind(roving_key, handle, member, cx)
    }

    /// The roving key of the toggle with `value`, as it last rendered.
    pub fn roving_key_of(&self, value: &str) -> Option<SharedString> {
        self.items
            .info(&SharedString::from(value.to_string()))
            .map(|item| item.roving_key)
    }

    /// The `group` part, around the toggles: a group, not a toolbar, since a
    /// toolbar may hold one and a group takes no orientation. Build it before
    /// the toggles: it starts their render, so the order they render in is
    /// page order. Name it with `aria_label`.
    pub fn group(this: &Entity<Self>, cx: &App) -> Part<ToggleGroupState> {
        let group = this.read(cx);
        let state = group.state(cx);
        group.items.begin();
        if group.toolbar.is_none() {
            group.own.roving.begin(cx);
        }
        group.own.disabled.set(state.disabled);
        let node = group.node.clone();
        Part::new(&group.instance, "group", None, state)
            .aria_role(Role::Group)
            .map_el(|el| node.track(el))
    }
}

/// `agent: { actions: ["setValue", "toggle"], closed: true }`.
impl Agent for ToggleGroup {
    const SURFACE: AgentSurface = AgentSurface::new().closed(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |group, args, window, cx| {
            let value = args.list(0)?;
            Ok(group.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("toggle", |group, args, window, cx| {
            let value = args.string(0)?;
            Ok(group.toggle(value, ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, cx: &App) -> AgentState {
        let value: Vec<String> = self.value.get().iter().map(ToString::to_string).collect();
        AgentState::new()
            .with("value", value)
            .with("multiple", self.multiple)
            .with("disabled", self.is_disabled(cx))
    }
}
