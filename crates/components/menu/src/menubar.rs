//! Port of `components/menubar/menubar.ts`: a row of menus, like a desktop
//! app's, with one tab stop, arrow keys between the triggers and between open
//! menus, and pointing at another trigger switching the open menu.
//!
//! Each menu joins its menubar from its trigger, with `menubar` in its
//! options: the trigger becomes one of the menubar's items. What the menubar
//! sees of each is a [`MenubarMember`]. What it shares with its menus, which
//! ask it from inside their own updates, is plain state beside its entity.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    App, Context, Entity, FocusHandle, KeyDownEvent, SharedString, StatefulInteractiveElement, Subscription,
    WeakEntity, Window,
};
use ui_sdk_focus::{Orientation, RovingFocus, RovingFocusOptions, RovingMember, move_index, orientation_of};
use ui_sdk_kernel::accesskit::Orientation as AccessOrientation;
use ui_sdk_kernel::focus_events::on_blur;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, ChangeOptions, ConnectedController, Controller, InputEvent, Instance, Key, Need,
    Part, Reason,
};
use ui_sdk_platform::{Region, Target, is_rtl};
use ui_sdk_popups::{HoverEvent, PopupOpener, SafeArea, SafeAreaAxis, create_safe_area};
use ui_sdk_text::{TYPEAHEAD_TIMEOUT, Typeahead, is_typeahead_key};

use crate::menu::MenuModal;
use crate::menu_api::Focus;

/// Which way the menubar runs, which picks the arrow keys that move between
/// its menus. A vertical menubar opens its menus to the side.
pub type MenubarOrientation = Orientation;

/// Opens, closes and focuses a menubar's menus from outside it, before it's
/// made or without its entity at hand, like the F10 shortcut desktop apps
/// give their menubars. Pass it as the `controller` option; its methods are
/// [`MenubarControllerExt`]'s.
pub type MenubarController = Controller<Menubar>;

/// A controller for a menubar (`createMenubarController`).
pub fn create_menubar_controller() -> MenubarController {
    Controller::new()
}

/// A menubar's methods on its controller. Calls before the menubar attaches,
/// and once it's gone, do nothing. Call them outside the menubar's own
/// update.
pub trait MenubarControllerExt {
    /// Moves focus to the trigger that holds the menubar's tab stop: the one
    /// used last, or the first.
    fn focus(&self, window: &mut Window, cx: &mut App);
    /// Whether one of its menus is open.
    fn is_open(&self, cx: &App) -> bool;
    /// Closes the open menu, if any, and its submenus. Returns false when the
    /// close was canceled or it was already closed.
    fn close(&self, options: ChangeOptions, window: &mut Window, cx: &mut App) -> Option<bool>;
}

impl MenubarControllerExt for MenubarController {
    fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(bar) = self.entity(cx) {
            let shared = bar.read(cx).shared.clone();
            shared.roving.focus(None, window, cx);
        }
    }

    fn is_open(&self, cx: &App) -> bool {
        self.read(cx, |bar, _| bar.is_open()).unwrap_or(false)
    }

    fn close(&self, options: ChangeOptions, window: &mut Window, cx: &mut App) -> Option<bool> {
        let bar = self.entity(cx)?;
        let shared = bar.read(cx).shared.clone();
        Some(shared.close(options, window, cx))
    }
}

/// The options `Menubar` takes (`MenubarOptions`).
#[derive(Clone, Default)]
pub struct MenubarOptions {
    /// Which arrow keys move between menus: Left and Right, or Up and Down.
    /// A vertical menubar opens its menus to the side. Horizontal when unset.
    pub orientation: Option<MenubarOrientation>,
    /// Whether arrow keys wrap from the last menu to the first, between
    /// triggers and between open menus. True when unset.
    pub r#loop: Option<bool>,
    /// Stops every menu from opening. Triggers stay focusable and report
    /// themselves disabled, so the menubar keeps its tab stop and stays
    /// discoverable.
    pub disabled: bool,
    /// The `modal` of every menu in the menubar that doesn't set its own.
    /// `Pointer`: an outside press only closes the open menu. `True`: also
    /// blocks the rest of the window and its scrolling, and traps focus,
    /// except the menubar. `False`: outside presses reach what they hit.
    /// `Pointer` when unset.
    pub modal: Option<MenuModal>,
    /// Opens, closes and focuses the menubar from anywhere. Create one with
    /// [`create_menubar_controller`].
    pub controller: Option<MenubarController>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl MenubarOptions {
    pub fn orientation(mut self, orientation: MenubarOrientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = Some(r#loop);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn modal(mut self, modal: impl Into<MenuModal>) -> Self {
        self.modal = Some(modal.into());
        self
    }

    pub fn controller(mut self, controller: MenubarController) -> Self {
        self.controller = Some(controller);
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root part's `with` and the app read (`MenubarState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenubarState {
    /// Which arrow keys move between menus, from the `orientation` option.
    pub orientation: MenubarOrientation,
    /// Whether the whole menubar is disabled.
    pub disabled: bool,
    /// Whether one of its menus is open (`data-open`).
    pub open: bool,
    /// The modal setting its menus inherit.
    pub modal: MenuModal,
    /// Whether one of its triggers has an icon (`data-has-icons`).
    pub has_icons: bool,
}

/// How a menubar opens one of its menus.
#[derive(Clone, Debug)]
pub struct MenubarOpen {
    /// Where focus goes: its first or last item, or the menu itself.
    pub focus: Focus,
    pub reason: Reason,
    pub event: Option<InputEvent>,
    pub by: Option<PopupOpener>,
}

/// How a menubar closes one of its menus.
#[derive(Clone, Debug)]
pub struct MenubarClose {
    pub reason: Reason,
    pub event: Option<InputEvent>,
    /// Whether focus returns to the menu's trigger.
    pub focus: bool,
}

/// What a menubar sees of each menu in it (`MenubarMember`). Menus join from
/// their trigger. Each runs outside the menubar's update.
#[derive(Clone)]
pub struct MenubarMember {
    pub is_open: Rc<dyn Fn() -> bool>,
    pub disabled: Rc<dyn Fn() -> bool>,
    pub open: Rc<dyn Fn(MenubarOpen, &mut Window, &mut App)>,
    /// Returns false when the menu refused to close.
    pub close: Rc<dyn Fn(MenubarClose, &mut Window, &mut App) -> bool>,
}

// What the menubar does for Left and Right inside an open menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BarArrow {
    // The key went to another menu.
    Moved,
    // The key back toward a vertical menubar closes the menu, like a
    // submenu.
    Close,
}

// A menu in the menubar, by the menu's id, as its trigger last rendered.
#[derive(Clone)]
struct Joined {
    key: SharedString,
    member: MenubarMember,
    region: Region,
    handle: FocusHandle,
    label: SharedString,
    // Whether an icon is in the trigger, which marks the menubar.
    has_icon: Rc<dyn Fn() -> bool>,
}

/// What a menubar shares with its menus and the handlers on their triggers,
/// which run outside its update.
pub(crate) struct MenubarShared {
    pub(crate) entity: WeakEntity<Menubar>,
    pub(crate) orientation: Cell<MenubarOrientation>,
    r#loop: Cell<bool>,
    pub(crate) disabled: Cell<bool>,
    pub(crate) modal: Cell<MenuModal>,
    // The tab stop, between triggers by their menu's id.
    pub(crate) roving: RovingFocus<SharedString>,
    members: RefCell<Vec<Joined>>,
    // The open menu's id.
    open: RefCell<Option<SharedString>>,
    typeahead: RefCell<Typeahead>,
    // Pointing at a disabled trigger closes the open menu, but the menubar
    // stays in menu mode while that trigger has focus, so the next trigger
    // pointed at opens its menu. Escape and focus leaving the trigger end it.
    held: RefCell<Option<FocusHandle>>,
    // Crossing other triggers on the way to the open menu doesn't switch to
    // them.
    pub(crate) aim: SafeArea,
    pointer_on: RefCell<Option<SharedString>>,
    // The root's element, which a modal menu leaves usable.
    pub(crate) root: Region,
}

// A menu's link to the menubar it sits in.
#[derive(Clone)]
pub(crate) struct MenubarLink {
    pub(crate) entity: WeakEntity<Menubar>,
    pub(crate) shared: Rc<MenubarShared>,
}

impl MenubarLink {
    pub(crate) fn of(menubar: &Entity<Menubar>, cx: &App) -> Self {
        Self {
            entity: menubar.downgrade(),
            shared: menubar.read(cx).shared.clone(),
        }
    }
}

impl MenubarShared {
    fn joined(&self, key: &SharedString) -> Option<Joined> {
        self.members.borrow().iter().find(|joined| joined.key == *key).cloned()
    }

    fn open_member(&self) -> Option<MenubarMember> {
        let open = self.open.borrow().clone()?;
        self.joined(&open).map(|joined| joined.member)
    }

    fn notify(&self, cx: &mut App) {
        if let Some(entity) = self.entity.upgrade() {
            cx.notify(entity.entity_id());
        }
    }

    // A menu joins from its trigger as it renders, which takes a place in
    // the tab stop. Returns whether the trigger holds the stop.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn join(
        &self,
        key: &SharedString,
        member: MenubarMember,
        handle: &FocusHandle,
        region: &Region,
        label: SharedString,
        has_icon: Rc<dyn Fn() -> bool>,
        claims: bool,
        cx: &App,
    ) -> bool {
        {
            let joined = Joined {
                key: key.clone(),
                member,
                region: region.clone(),
                handle: handle.clone(),
                label,
                has_icon,
            };
            let mut members = self.members.borrow_mut();
            match members.iter_mut().find(|other| other.key == *key) {
                Some(other) => *other = joined,
                None => members.push(joined),
            }
        }
        // Tab lands on the first enabled trigger, then on the one used last.
        // Before triggers bind, the first enabled one claims the stop. In a
        // disabled menubar every trigger is disabled and stays focusable, so
        // the first one does.
        self.roving
            .bind(key.clone(), handle, RovingMember::new().claims(claims), cx)
    }

    pub(crate) fn leave_member(&self, key: &SharedString) {
        if let Ok(mut members) = self.members.try_borrow_mut() {
            members.retain(|joined| joined.key != *key);
        }
    }

    // Whether a press lands on one of the menubar's triggers, which never
    // counts as outside its open menu: it switches menus instead.
    pub(crate) fn on_trigger(&self, target: &Target, window: &gpui::Window) -> bool {
        self.members
            .borrow()
            .iter()
            .any(|joined| joined.region.contains(target, window))
    }

    // One menu is open at a time. Another opening closes it, unless it
    // refuses.
    pub(crate) fn opening(
        &self,
        key: &SharedString,
        event: Option<&InputEvent>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let members = self.members.borrow().clone();
        members.iter().all(|joined| {
            joined.key == *key
                || !(joined.member.is_open)()
                || (joined.member.close)(
                    MenubarClose {
                        reason: Reason::SubmenuSwitch,
                        event: event.cloned(),
                        focus: false,
                    },
                    window,
                    cx,
                )
        })
    }

    pub(crate) fn open_changed(&self, key: &SharedString, open: bool, cx: &mut App) {
        self.aim.stop();
        if open {
            *self.held.borrow_mut() = None;
            *self.open.borrow_mut() = Some(key.clone());
        } else if self.open.borrow().as_ref() == Some(key) {
            *self.open.borrow_mut() = None;
        } else {
            return;
        }
        self.notify(cx);
    }

    // Moves focus to the trigger `next`, opening its menu when `open` says
    // so. A disabled menu takes focus on its trigger instead, and the open
    // menu closes (`moveTo`).
    fn move_to(&self, next: &SharedString, event: Option<InputEvent>, open: bool, window: &mut Window, cx: &mut App) {
        let Some(joined) = self.joined(next) else {
            return;
        };
        if open && !(joined.member.disabled)() {
            (joined.member.open)(
                MenubarOpen {
                    focus: Focus::First,
                    reason: Reason::TriggerPress,
                    event,
                    by: None,
                },
                window,
                cx,
            );
        } else {
            let closed = match self.open_member() {
                Some(member) => (member.close)(
                    MenubarClose {
                        reason: Reason::SubmenuSwitch,
                        event,
                        focus: false,
                    },
                    window,
                    cx,
                ),
                None => true,
            };
            if closed {
                window.focus(&joined.handle, cx);
            }
        }
        // A menu that refused to close keeps the menubar where it was.
        let current = self.open.borrow().clone();
        if current.is_none() || current.as_ref() == Some(next) {
            self.roving.remember(next, cx);
        }
    }

    // Left and Right inside an open menu, on an item without a submenu. A
    // horizontal menubar opens the neighbouring menu at its first item, as
    // the APG asks. A vertical one opens its menus to the side, so the key
    // back toward the menubar closes the menu like a submenu, and the other
    // does nothing (`menuArrow`).
    pub(crate) fn menu_arrow(
        &self,
        key: &SharedString,
        by: isize,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<BarArrow> {
        if self.orientation.get() == Orientation::Vertical {
            return (by == -1).then_some(BarArrow::Close);
        }
        let list = self.roving.items();
        let index = list
            .iter()
            .position(|item| item.key == *key)
            .map_or(-1, |index| index as isize);
        let next = list
            .get(move_index(index, by, list.len(), self.r#loop.get()))
            .map(|item| item.key.clone());
        if let Some(next) = next
            && next != *key
        {
            self.move_to(&next, event, true, window, cx);
        }
        Some(BarArrow::Moved)
    }

    // Keys on a trigger. Arrows along the menubar, Home, End and typeahead
    // move between triggers, and a menu open from the trigger moves along.
    // Returns where focus goes when the key opens the trigger's menu: the
    // arrow across the menubar opens it at its first item, and Up at its
    // last. gpui presses the trigger for Enter and Space (`triggerKeydown`).
    pub(crate) fn trigger_keydown(
        &self,
        event: &KeyDownEvent,
        key: &SharedString,
        disabled: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Focus> {
        let pressed = Key::of(&event.keystroke);
        if pressed == Key::Escape {
            *self.held.borrow_mut() = None;
        }
        let key_event = InputEvent::KeyDown(event.clone());
        let was_open = self.joined(key).is_some_and(|joined| (joined.member.is_open)());
        let moved = if event.keystroke.modifiers.shift {
            None
        } else {
            self.roving.keydown(event, key, window, cx)
        };
        if let Some(moved) = moved {
            if was_open && moved.key != moved.from {
                self.move_to(&moved.key, Some(key_event), true, window, cx);
            }
            return None;
        }
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented() || modifiers.alt || modifiers.control || modifiers.platform {
            return None;
        }
        if is_typeahead_key(&event.keystroke) && pressed != Key::Space {
            let list = self.roving.items();
            let labels: Vec<SharedString> = list
                .iter()
                .map(|item| self.joined(&item.key).map(|joined| joined.label).unwrap_or_default())
                .collect();
            let current = list.iter().position(|item| item.key == *key);
            let found = self.typeahead.borrow_mut().search(&pressed, &labels, current)?;
            window.prevent_default();
            let next = list[found].key.clone();
            self.move_to(&next, Some(key_event), was_open, window, cx);
            return None;
        }
        if disabled {
            return None;
        }
        let vertical = self.orientation.get() == Orientation::Vertical;
        let across = if is_rtl(cx) { Key::ArrowLeft } else { Key::ArrowRight };
        let first = if vertical {
            pressed == across
        } else {
            pressed == Key::ArrowDown
        };
        let last = !vertical && pressed == Key::ArrowUp;
        if !first && !last {
            return None;
        }
        window.prevent_default();
        Some(if last { Focus::Last } else { Focus::First })
    }

    // The pointer left a trigger. On its way to the open menu, crossing other
    // triggers doesn't switch to them (`leave`).
    pub(crate) fn leave(&self, key: &SharedString, event: &HoverEvent, menu: Option<Region>) {
        if self.pointer_on.borrow().as_ref() == Some(key) {
            *self.pointer_on.borrow_mut() = None;
        }
        let Some(menu) = menu else {
            return;
        };
        if self.open.borrow().as_ref() != Some(key) {
            return;
        }
        let along = if self.orientation.get() == Orientation::Vertical {
            SafeAreaAxis::X
        } else {
            SafeAreaAxis::Y
        };
        self.aim.start(event.position, menu, along);
    }

    // The pointer reached the open menu (`enterMenu`).
    pub(crate) fn enter_menu(&self) {
        self.aim.stop();
    }

    // Closes the open menu, returning focus to its trigger (`close`).
    pub(crate) fn close(&self, options: ChangeOptions, window: &mut Window, cx: &mut App) -> bool {
        let Some(member) = self.open_member() else {
            return false;
        };
        (member.close)(
            MenubarClose {
                reason: options.reason.unwrap_or(Reason::Imperative),
                event: options.event,
                focus: true,
            },
            window,
            cx,
        )
    }
}

/// A menubar (`menubar`). Its entity is its controller.
pub struct Menubar {
    instance: Instance,
    pub(crate) shared: Rc<MenubarShared>,
    controller: ConnectedController<Self>,
    held_blur: Option<Subscription>,
}

impl Menubar {
    pub fn new(options: MenubarOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("menubar").with_agent(cx);
        instance.debug = options.debug;
        let orientation = orientation_of(options.orientation, Orientation::Horizontal);
        let r#loop = options.r#loop.unwrap_or(true);
        let roving = RovingFocus::new(
            RovingFocusOptions::default().orientation(orientation).r#loop(r#loop),
            window,
            cx,
        );
        let shared = Rc::new(MenubarShared {
            entity: cx.weak_entity(),
            orientation: Cell::new(orientation),
            r#loop: Cell::new(r#loop),
            disabled: Cell::new(options.disabled),
            modal: Cell::new(options.modal.unwrap_or(MenuModal::Pointer)),
            roving,
            members: RefCell::new(Vec::new()),
            open: RefCell::new(None),
            typeahead: RefCell::new(Typeahead::new(TYPEAHEAD_TIMEOUT)),
            held: RefCell::new(None),
            aim: create_safe_area(),
            pointer_on: RefCell::new(None),
            root: Region::new(),
        });
        Self {
            instance,
            shared,
            controller: ConnectedController::new(options.controller, cx),
            held_blur: None,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). Every option is controlled.
    pub fn set_options(&mut self, options: MenubarOptions, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        self.controller.set(options.controller);
        let orientation = orientation_of(options.orientation, Orientation::Horizontal);
        let r#loop = options.r#loop.unwrap_or(true);
        let modal = options.modal.unwrap_or(MenuModal::Pointer);
        let shared = &self.shared;
        let changed = shared.orientation.get() != orientation
            || shared.r#loop.get() != r#loop
            || shared.disabled.get() != options.disabled
            || shared.modal.get() != modal;
        shared.orientation.set(orientation);
        shared.r#loop.set(r#loop);
        shared.disabled.set(options.disabled);
        shared.modal.set(modal);
        shared.roving.set_orientation(orientation);
        shared.roving.set_loop(r#loop);
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The menubar's state, as its root part styles from it.
    pub fn state(&self) -> MenubarState {
        MenubarState {
            orientation: self.shared.orientation.get(),
            disabled: self.shared.disabled.get(),
            open: self.is_open(),
            modal: self.shared.modal.get(),
            has_icons: self.shared.members.borrow().iter().any(|joined| (joined.has_icon)()),
        }
    }

    /// Whether one of its menus is open (`isOpen`).
    pub fn is_open(&self) -> bool {
        self.shared.open.borrow().is_some()
    }

    /// Moves focus to the trigger that holds the menubar's tab stop: the one
    /// used last, or the first (`focus`).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.shared.roving.focus(None, window, cx);
    }

    /// Closes the open menu, if any, and its submenus, returning focus to its
    /// trigger. Returns false when the close was canceled or it was already
    /// closed (`close`). Call it outside the open menu's own update.
    pub fn close(&self, options: ChangeOptions, window: &mut Window, cx: &mut App) -> bool {
        self.shared.close(options, window, cx)
    }

    // While a menu is open, resting the pointer on another trigger switches
    // to its menu, like a desktop menubar (`hover`).
    pub(crate) fn hover(
        &mut self,
        key: &SharedString,
        event: &HoverEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shared = self.shared.clone();
        let open = shared.open.borrow().clone();
        let Some(joined) = shared.joined(key) else {
            return;
        };
        if (open.is_none() && shared.held.borrow().is_none()) || open.as_ref() == Some(key) || event.touch {
            return;
        }
        *shared.pointer_on.borrow_mut() = Some(key.clone());
        let rested_key = key.clone();
        let rested = event.clone();
        if shared.aim.contains(
            event.position,
            move |bar: &mut Self, window, cx| {
                let pointed = bar.shared.pointer_on.borrow().as_ref() == Some(&rested_key);
                if pointed {
                    bar.hover(&rested_key, &rested, window, cx);
                }
            },
            window,
            cx,
        ) {
            return;
        }
        shared.aim.stop();
        if (joined.member.disabled)() {
            shared.move_to(key, Some(event.event.clone()), false, window, cx);
            if shared.open.borrow().is_some() {
                return;
            }
            *shared.held.borrow_mut() = Some(joined.handle.clone());
            let held = joined.handle.clone();
            self.held_blur = Some(on_blur(
                &joined.handle,
                window,
                cx,
                move |bar: &mut Self, _window, _cx| {
                    let mut current = bar.shared.held.borrow_mut();
                    if current.as_ref() == Some(&held) {
                        *current = None;
                    }
                },
            ));
            return;
        }
        shared.roving.remember(key, cx);
        (joined.member.open)(
            MenubarOpen {
                focus: Focus::Popup,
                reason: Reason::TriggerHover,
                event: Some(event.event.clone()),
                by: Some(PopupOpener::Pointer),
            },
            window,
            cx,
        );
    }

    /// The `root` part (`root`): the `menubar` that holds the menus'
    /// triggers. Name it with `aria_label` when the window has another.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<MenubarState> {
        let bar = this.read(cx);
        bar.shared.roving.begin(cx);
        let state = bar.state();
        let orientation = match state.orientation {
            Orientation::Horizontal => AccessOrientation::Horizontal,
            Orientation::Vertical => AccessOrientation::Vertical,
        };
        let part = Part::new(&bar.instance, "root", None, state)
            .aria_role(Role::Menubar)
            .unneeded(Need::Keys, "Each menu's trigger, an item of the menubar, handles them.")
            .aria_orientation(orientation);
        bar.shared.root.measure(part)
    }
}

/// `agent: { actions: ["close"] }` and `controlled: []`: nothing here is a
/// controllable `open`.
impl Agent for Menubar {
    const ACTIONS: &'static [AgentAction<Self>] = &[AgentAction::new("close", |bar, _args, window, cx| {
        let shared = bar.shared.clone();
        Ok(shared.close(ChangeOptions::default(), window, cx).into())
    })];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&[]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        let state = self.state();
        let orientation = match state.orientation {
            Orientation::Horizontal => "horizontal",
            Orientation::Vertical => "vertical",
        };
        AgentState::new()
            .with("orientation", orientation)
            .with("disabled", state.disabled)
            .with("open", state.open)
            .with("modal", state.modal.as_str())
    }
}
