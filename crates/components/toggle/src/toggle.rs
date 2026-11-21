//! Port of `components/toggle/toggle.ts`: a button that stays pressed.
//!
//! Inside a toggle group or a toolbar, the group's value and roving tab stop
//! decide: a toggle made with a `group` follows the group's value and joins
//! its tab stop, or the toolbar's the group sits in, and one made with a
//! `toolbar` alone joins the toolbar's. A disabled fieldset around it
//! disables it too.

use std::rc::Rc;

use gpui::{App, Context, Entity, FocusHandle, Focusable, SharedString, Window};

use ui_sdk_focus::{RovingMember, RovingScope};
use ui_sdk_forms::{Node, watch_form_disabled};
use ui_sdk_kernel::dev;
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::{HandledKey, Role};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, Aria, ChangeCallback, ChangeDetails, ChangeOptions, Controllable,
    Instance, Part, Reason, change_source, create_details, flag_while_bound, with_source,
};
use ui_sdk_lists::{IconPlacement, ItemIconProps, ItemIconState, check_icon_item, icon_part};

use crate::toggle_group::{ToggleGroup, ToggleGroupOrientation};

/// Why a toggle wants to be pressed or unpressed: a press (`Press`), or a
/// call (`Imperative`).
pub type TogglePressedChangeReason = Reason;

/// Options for every write method.
pub type ToggleChangeOptions = ChangeOptions;

/// The props the `icon` part takes: `mirror` flips it in right-to-left
/// text, and `content` is the icon itself.
pub type ToggleIconProps = ItemIconProps;

/// What the `icon` part's `with` reads: whether it mirrors, and whether to
/// flip it now.
pub type ToggleIconState = ItemIconState;

// Where a toggle's icon sits: in the toggle, in a group.
const ICON: IconPlacement = IconPlacement {
    component: "toggle",
    item: Role::Button,
    list: Some(Role::Group),
};

/// The options `Toggle` takes.
#[derive(Clone, Default)]
pub struct ToggleOptions {
    /// Whether it's pressed. Pass it to control the toggle. Inside a group,
    /// the group's value decides.
    pub pressed: Option<bool>,
    /// Whether it's pressed at first, when uncontrolled.
    pub default_pressed: bool,
    /// Runs when a press wants to change it. `details.reason` is `Press` or
    /// `Imperative`; `details.cancel()` stops it. Inside a group, it runs
    /// before the group's `on_value_change`, and canceling it cancels the
    /// group's change too. It runs inside the toggle's update, so it may
    /// update any entity but the toggle.
    pub on_pressed_change: Option<ChangeCallback<bool>>,
    /// Identifies the toggle inside a group, whose value lists the pressed
    /// toggles' values. Defaults to the toggle's generated id.
    pub value: Option<SharedString>,
    /// Stops presses. Alone, the toggle leaves the tab order, unless
    /// `focusable_when_disabled` is set. Inside a group or a toolbar, it
    /// stays focusable and reports itself disabled, so arrow keys still
    /// reach it.
    pub disabled: bool,
    /// Keeps a disabled toggle in the tab order, reporting it disabled.
    /// Toggles in a group or a toolbar always are.
    pub focusable_when_disabled: bool,
    /// The group the toggle belongs to (`group`), which owns whether it's
    /// pressed and its tab stop.
    pub group: Option<Entity<ToggleGroup>>,
    /// The toolbar the toggle sits in outside a group, from `Toolbar::scope`
    /// or `scope_in` (`toolbar`), whose arrow keys and tab stop it joins.
    pub toolbar: Option<Rc<dyn RovingScope>>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl ToggleOptions {
    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = Some(pressed);
        self
    }

    pub fn default_pressed(mut self, pressed: bool) -> Self {
        self.default_pressed = pressed;
        self
    }

    pub fn on_pressed_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_pressed_change = Some(Rc::new(callback));
        self
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn focusable_when_disabled(mut self, focusable: bool) -> Self {
        self.focusable_when_disabled = focusable;
        self
    }

    pub fn group(mut self, group: Entity<ToggleGroup>) -> Self {
        self.group = Some(group);
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

/// What the root part's `with` and the app read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToggleState {
    /// Whether it's pressed. Inside a group, it follows the group's value.
    pub pressed: bool,
    /// Whether it's disabled, by itself, its group, its toolbar or a
    /// fieldset around it.
    pub disabled: bool,
    /// Which way its group or toolbar runs, which tells it which way it sits
    /// (`data-orientation`). `None` alone.
    pub orientation: Option<ToggleGroupOrientation>,
    /// Whether an `icon` part is in it (`data-has-icon`), which helps line
    /// up icon and text content.
    pub has_icon: bool,
}

// The keys a toggle in a group or a toolbar hands to its roving group.
const ROVING_KEYS: &[HandledKey] = &["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"];

// What a toggle in a group or a toolbar takes from it (`fromGroup`,
// `fromToolbar`): the arrow keys and tab stop it joins, whether that
// disables it, and which way it runs. In a group, the toggle's own toolbar
// can be newer than the group's, like when a toolbar group around both is
// disabled.
struct Joined {
    scope: Rc<dyn RovingScope>,
    group_disabled: bool,
    orientation: ToggleGroupOrientation,
}

/// A toggle. Its entity is its controller.
pub struct Toggle {
    instance: Instance,
    pressed: Controllable<bool>,
    on_pressed_change: Option<ChangeCallback<bool>>,
    value: Option<SharedString>,
    disabled: bool,
    focusable_when_disabled: bool,
    group: Option<Entity<ToggleGroup>>,
    toolbar: Option<Rc<dyn RovingScope>>,
    // The root's place in the page, so a disabled fieldset around it
    // disables it.
    node: Node,
    focus_handle: FocusHandle,
}

impl Toggle {
    pub fn new(options: ToggleOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut pressed = Controllable::new("pressed", options.pressed.unwrap_or(options.default_pressed));
        pressed.set_controlled(options.pressed);
        pressed.set_on_change(options.on_pressed_change.clone());
        let mut instance = Instance::new("toggle").with_agent(cx);
        instance.debug = options.debug;
        Self {
            instance,
            pressed,
            on_pressed_change: options.on_pressed_change,
            value: options.value,
            disabled: options.disabled,
            focusable_when_disabled: options.focusable_when_disabled,
            group: options.group,
            toolbar: options.toolbar,
            node: watch_form_disabled(cx),
            focus_handle: cx.focus_handle(),
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). Inside a group, the group's value decides, so `pressed`
    /// waits until the toggle leaves it.
    pub fn set_options(&mut self, options: ToggleOptions, cx: &mut Context<Self>) {
        let mut changed = self.pressed.set_controlled(options.pressed);
        self.pressed.set_on_change(options.on_pressed_change.clone());
        self.on_pressed_change = options.on_pressed_change;
        changed |= self.disabled != options.disabled
            || self.focusable_when_disabled != options.focusable_when_disabled
            || self.value != options.value
            || self.group.as_ref().map(Entity::entity_id) != options.group.as_ref().map(Entity::entity_id)
            || self.toolbar.is_some() != options.toolbar.is_some();
        self.disabled = options.disabled;
        self.focusable_when_disabled = options.focusable_when_disabled;
        self.value = options.value;
        self.group = options.group;
        self.toolbar = options.toolbar;
        self.instance.debug = options.debug;
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the toggle keep its own.
    pub fn set_controlled_pressed(&mut self, pressed: Option<bool>, cx: &mut Context<Self>) {
        if self.pressed.set_controlled(pressed) {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// Its value: the `value` option, or its generated id (`keyOf`).
    pub fn value(&self) -> SharedString {
        self.value.clone().unwrap_or_else(|| self.instance.id().clone())
    }

    /// The group it belongs to.
    pub fn group(&self) -> Option<&Entity<ToggleGroup>> {
        self.group.as_ref()
    }

    // The arrow keys and tab stop it joins: its group's, or the toolbar's
    // outside a group (`scopeOf`), with what it takes from them.
    fn joined(&self, cx: &App) -> Option<Joined> {
        let own_toolbar = self.toolbar.as_ref().is_some_and(|toolbar| toolbar.disabled(cx));
        if let Some(group) = &self.group {
            let group = group.read(cx);
            return Some(Joined {
                scope: group.scope(),
                group_disabled: group.is_disabled(cx) || own_toolbar,
                orientation: group.orientation(),
            });
        }
        let toolbar = self.toolbar.clone()?;
        Some(Joined {
            group_disabled: own_toolbar,
            orientation: toolbar.orientation(cx),
            scope: toolbar,
        })
    }

    pub fn state(&self, cx: &App) -> ToggleState {
        let joined = self.joined(cx);
        self.state_with(joined.as_ref(), cx)
    }

    fn state_with(&self, joined: Option<&Joined>, cx: &App) -> ToggleState {
        ToggleState {
            pressed: self.is_pressed(cx),
            disabled: self.disabled_with(joined, cx),
            orientation: joined.map(|joined| joined.orientation),
            has_icon: self.has_icon(),
        }
    }

    /// Whether an `icon` part is in it. Asking makes the toggle render again
    /// as icons come and go.
    pub fn has_icon(&self) -> bool {
        flag_while_bound(&self.instance, "icon").unwrap_or(false)
    }

    // Disabled by itself, its group or toolbar, or a fieldset (`isDisabled`).
    fn disabled_with(&self, joined: Option<&Joined>, _cx: &App) -> bool {
        self.disabled || joined.is_some_and(|joined| joined.group_disabled) || self.node.form_disabled()
    }

    /// Whether it's disabled, by itself, its group, its toolbar or a
    /// fieldset around it.
    pub fn is_disabled(&self, cx: &App) -> bool {
        let joined = self.joined(cx);
        self.disabled_with(joined.as_ref(), cx)
    }

    /// Whether it's pressed. Inside a group, it follows the group's value.
    pub fn is_pressed(&self, cx: &App) -> bool {
        match &self.group {
            Some(group) => group.read(cx).is_pressed(&self.value()),
            None => *self.pressed.get(),
        }
    }

    /// Presses or unpresses it, as a user would. A disabled toggle ignores it.
    /// Returns false when canceled.
    pub fn set_pressed(
        &mut self,
        pressed: bool,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_disabled(cx) {
            return false;
        }
        let Some(group) = self.group.clone() else {
            let changed = self.pressed.change(&self.instance, pressed, options, window, cx);
            if changed {
                cx.notify();
            }
            return changed;
        };
        let key = self.value();
        if !group.read(cx).allows(&key, pressed, cx) {
            return false;
        }
        // The toggle hears first and can cancel. Then the group decides, and
        // the toggle follows its value.
        let reason = options.reason.unwrap_or(Reason::Imperative);
        let source = options
            .source
            .unwrap_or_else(|| change_source(reason, options.event.as_ref()));
        let details = create_details(reason, options.event.clone(), source);
        if let Some(callback) = self.on_pressed_change.clone() {
            with_source(source, || callback(&pressed, &details, window, cx));
        }
        if details.canceled() {
            return false;
        }
        let group_reason = if reason == Reason::Press { Reason::ItemPress } else { reason };
        let changed = group.update(cx, |group, cx| {
            group.press(
                key,
                pressed,
                ChangeOptions {
                    reason: Some(group_reason),
                    source: options.source,
                    event: options.event,
                },
                window,
                cx,
            )
        });
        if changed {
            cx.notify();
        }
        changed
    }

    /// Presses it, or unpresses it.
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let next = !self.is_pressed(cx);
        self.set_pressed(next, options, window, cx)
    }

    /// Moves focus to it.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus_handle, cx);
    }

    /// The `root` part: the button itself. In a group or a toolbar, it holds
    /// the tab stop when its roving group says so, and its arrow keys move
    /// between the toggles.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<ToggleState> {
        let toggle = this.read(cx);
        let joined = toggle.joined(cx);
        let state = toggle.state_with(joined.as_ref(), cx);
        let entity = this.downgrade();
        let node = toggle.node.clone();
        let mut part = Part::new(&toggle.instance, "root", None, state)
            .aria_role(Role::Button)
            .aria(Aria::new().pressed(state.pressed).disabled(state.disabled))
            .map_el(|el| node.track(el));
        // A toggle that shows only an icon names nothing.
        if dev::DEV && state.has_icon {
            let entity = entity.clone();
            part = part.keep_name().on_props(move |window, cx| {
                if let Some(toggle) = entity.upgrade() {
                    check_icon_item(&toggle.read(cx).instance, "root", None, ICON, window);
                }
            });
        }
        let Some(joined) = joined else {
            return part
                .focus_mode(
                    FocusMode::Native,
                    Some(&toggle.focus_handle),
                    state.disabled && !toggle.focusable_when_disabled,
                )
                .on_press(move |event, window, cx| {
                    entity
                        .update(cx, |toggle, cx| {
                            if toggle.is_disabled(cx) {
                                window.prevent_default();
                                return;
                            }
                            toggle.toggle(ChangeOptions::event_of(Reason::Press, event), window, cx);
                        })
                        .ok();
                });
        };
        // A toggle joins its group by its value, and the roving group by its
        // id, which stays unique in a toolbar that holds several groups.
        let roving_key = toggle.instance.id().clone();
        let tab_stop = match &toggle.group {
            Some(group) => group.read(cx).register(
                toggle.value(),
                roving_key.clone(),
                &toggle.focus_handle,
                state.disabled,
                state.has_icon,
                cx,
            ),
            None => joined.scope.bind(
                roving_key.clone(),
                &toggle.focus_handle,
                RovingMember::new().disabled(state.disabled),
                cx,
            ),
        };
        let scope = joined.scope;
        let pressed = entity.clone();
        // Toggles in a group or a toolbar stay focusable when disabled.
        part.focus_mode(FocusMode::Roving { tab_stop }, Some(&toggle.focus_handle), false)
            .on_press(move |event, window, cx| {
                pressed
                    .update(cx, |toggle, cx| {
                        if toggle.is_disabled(cx) {
                            window.prevent_default();
                            return;
                        }
                        // A press that moved no focus, like a screen reader's,
                        // would leave the arrow keys nowhere.
                        if !toggle.focus_handle.is_focused(window) {
                            window.focus(&toggle.focus_handle, cx);
                        }
                        toggle.toggle(ChangeOptions::event_of(Reason::Press, event), window, cx);
                    })
                    .ok();
            })
            .on_keys(ROVING_KEYS, move |event, window, cx| {
                scope.keydown(event, &roving_key, window, cx);
            })
    }

    /// The `icon` part: an icon inside the toggle, which screen readers
    /// skip, so the toggle's name stays its label. A toggle with only an icon
    /// still needs an `aria_label`. The toggle, and its group, know it has
    /// one. `mirror` flips it in right-to-left text, where its state's
    /// `flipped` says to.
    pub fn icon(this: &Entity<Self>, props: ToggleIconProps, cx: &App) -> Part<ToggleIconState> {
        icon_part(&this.read(cx).instance, "icon", None, &props, cx)
    }
}

impl Focusable for Toggle {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// `agent: { actions: ["setPressed"] }` and `controlled: ["pressed"]`, with
/// `toggle`, which `setPressed` covers, unlisted.
impl Agent for Toggle {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setPressed", |toggle, args, window, cx| {
            let pressed = args.boolean(0)?;
            Ok(toggle.set_pressed(pressed, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |toggle, _args, window, cx| {
            Ok(toggle.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["pressed"]);

    fn agent_state(&self, cx: &App) -> AgentState {
        AgentState::new()
            .with("pressed", self.is_pressed(cx))
            .with("disabled", self.is_disabled(cx))
    }
}
