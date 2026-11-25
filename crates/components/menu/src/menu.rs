//! Port of `components/menu/menu.ts`: a list of actions and options opened
//! from a button or a right-click, with groups, checkbox and radio items,
//! submenus, typeahead and shortcuts. The menu's options, the details its
//! callbacks receive, menus as data, the state its parts style from, and the
//! menu itself.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::{self, Debug};
use std::ops::Deref;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gpui::{
    App, Context, Entity, MouseButton, ScrollHandle, SharedString, Subscription, Task, WeakEntity, Window, px,
};
use ui_sdk_direct_manipulation::Hold;
use ui_sdk_kernel::{
    ActionOutcome, Agent, AgentAction, AgentState, AnnouncePending, ChangeCallback, ChangeDetails, ChangeOptions,
    ChangeSource, ConnectedController, InputEvent, Instance, PENDING, Parts, Reason, key_while_bound, while_destroying,
};
use ui_sdk_lists::{ItemIconProps, ItemIconState, ListItem, ListSeparator};
use ui_sdk_motion::{Presence, PresenceState};
use ui_sdk_platform::{Anchor, ButtonElement, Placement, Target, initial_apple, is_apple, is_rtl};
use ui_sdk_popups::{
    AbortSignal, AttachConfig, CreatePopupOptions, DismissAttemptCallback, DrillIn, EscapeFn, HoverIntentOptions,
    InitialFocusFn, Modal, OpenChangeCompleteCallback, Popup, PopupBehaviorOverrides, PopupBlur, PopupCloseOptions,
    PopupController, PopupFocusTarget, PopupHost, PopupOpenChangeCallback, PopupOpenOptions, PopupOptions,
    PopupPending, PopupRules, PresentationSetting, SheetConfig, SheetPartState, create_hover_intent, create_popup,
    create_popup_controller, create_safe_area, derive_open, menu_preset,
};
use ui_sdk_shortcuts::{ShortcutProps, ShortcutState};
use ui_sdk_text::{TYPEAHEAD_TIMEOUT, Typeahead};

use crate::menu_api::{
    CheckboxProps, Close, ContentProps, ContextTriggerProps, Focus, IndicatorProps, ItemProps, MenuShared,
    Presentation, PressSession, RadioGroupProps, RadioProps, Shared, Show, SubTriggerProps, TriggerProps,
};
use crate::menubar::{Menubar, MenubarLink};

/// Why a menu or submenu opened or closed: `TriggerPress`, `TriggerHover`,
/// `ItemPress`, `OutsidePress`, `EscapeKey`, `ClosePress`, `FocusOut`,
/// `WindowBlur`, `SubmenuSwitch`, `ParentClose`, `TriggerRelease`, `Back`,
/// `Shortcut`, `Swipe` or `Imperative`.
pub type MenuOpenChangeReason = Reason;

/// The dismissals `dismissible` refuses that `on_dismiss_attempt` hears
/// about: `OutsidePress`, `EscapeKey`, `Back` and `Swipe`.
pub type MenuDismissReason = Reason;

/// Why the menu's payload changed: a trigger or context area opened it,
/// another one took it over while open (`TriggerPress`, `TriggerHover`,
/// `Shortcut`), or a controller opened it or gave the open one another
/// payload (`Imperative`).
pub type MenuPayloadChangeReason = Reason;

/// Why a checkbox item wants to change: `ItemPress` or `Imperative`.
pub type MenuCheckedChangeReason = Reason;

/// Why a radio group's value wants to change: `ItemPress` or `Imperative`.
pub type MenuValueChangeReason = Reason;

/// What a menu's `on_action` receives besides the item's value.
#[derive(Clone, Debug, Default)]
pub struct MenuActionDetails {
    /// The press or key that ran the item. `None` when a shortcut ran it
    /// while the menu was closed.
    pub event: Option<InputEvent>,
}

/// Runs for every plain item with a value, in the menu and its submenus.
/// Return `ActionOutcome::Pending` to mark the item pending until its work
/// settles: the item is `pending`, busy and disabled, the menu stays open,
/// and choosing it again waits. It closes once the work succeeds; a failure
/// keeps it open with the reason in the item's `error` state until the next
/// try. `Canceled` keeps the menu open. It runs inside the menu's update,
/// since the menu reads what it returns, so it may update any entity but the
/// menu.
pub type MenuActionCallback = Rc<dyn Fn(&SharedString, &MenuActionDetails, &mut Window, &mut App) -> ActionOutcome>;

/// What a menu's `on_checked_change` and `on_before_checked_change` receive
/// besides the new state: the change's details, with `cancel()`, and which
/// item changed.
pub struct MenuCheckedChangeDetails<'a> {
    details: &'a ChangeDetails,
    /// The checkbox item's value.
    pub value: SharedString,
}

impl<'a> MenuCheckedChangeDetails<'a> {
    pub(crate) fn new(details: &'a ChangeDetails, value: SharedString) -> Self {
        Self { details, value }
    }

    /// The change's own details, with its reason, source and event.
    pub fn details(&self) -> &ChangeDetails {
        self.details
    }
}

impl Deref for MenuCheckedChangeDetails<'_> {
    type Target = ChangeDetails;

    fn deref(&self) -> &ChangeDetails {
        self.details
    }
}

impl Debug for MenuCheckedChangeDetails<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MenuCheckedChangeDetails")
            .field("details", self.details)
            .field("value", &self.value)
            .finish()
    }
}

/// What a menu's `on_value_change` and `on_before_value_change` receive
/// besides the new value: the change's details, with `cancel()`, and which
/// group changed.
pub struct MenuValueChangeDetails<'a> {
    details: &'a ChangeDetails,
    /// The radio group's name: a data menu's group `name`, or the key a
    /// composed group was made with.
    pub group: SharedString,
}

impl<'a> MenuValueChangeDetails<'a> {
    pub(crate) fn new(details: &'a ChangeDetails, group: SharedString) -> Self {
        Self { details, group }
    }

    /// The change's own details, with its reason, source and event.
    pub fn details(&self) -> &ChangeDetails {
        self.details
    }
}

impl Deref for MenuValueChangeDetails<'_> {
    type Target = ChangeDetails;

    fn deref(&self) -> &ChangeDetails {
        self.details
    }
}

impl Debug for MenuValueChangeDetails<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MenuValueChangeDetails")
            .field("details", self.details)
            .field("group", &self.group)
            .finish()
    }
}

/// `on_checked_change` and `on_before_checked_change`: the new state and
/// which item changed.
pub type MenuCheckedChangeCallback = Rc<dyn Fn(&bool, &MenuCheckedChangeDetails<'_>, &mut Window, &mut App)>;

/// `on_value_change` and `on_before_value_change`: the new value and which
/// group changed.
pub type MenuValueChangeCallback = Rc<dyn Fn(&SharedString, &MenuValueChangeDetails<'_>, &mut Window, &mut App)>;

/// Menus as data: entries, and plain strings for items whose value and
/// label are the string (`MenuData`).
pub type MenuData = Vec<MenuDataItem>;

/// What a data menu's plain item adds to a list's item (`ListItem & {..}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItemData {
    /// Whether running the item closes the menu. Defaults to true.
    pub close_on_select: bool,
    /// A keyboard shortcut like `"mod+n"`, which runs the item while the
    /// menu is closed.
    pub shortcut: Option<SharedString>,
    /// Marks the item as dangerous, like Delete.
    pub destructive: bool,
}

impl Default for MenuItemData {
    fn default() -> Self {
        Self {
            close_on_select: true,
            shortcut: None,
            destructive: false,
        }
    }
}

impl MenuItemData {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn close_on_select(mut self, close: bool) -> Self {
        self.close_on_select = close;
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }
}

/// A plain item of a data menu: a list's item, its value, label, keywords
/// and whether it's disabled, with what a menu item adds.
pub type MenuDataEntry = ListItem<SharedString, MenuItemData>;

/// One of a data radio group's items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuDataRadio {
    pub value: SharedString,
    pub label: SharedString,
    pub disabled: bool,
}

impl MenuDataRadio {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// One entry of a data menu: an item, a separator, a group, a checkbox item,
/// a radio group or a submenu (`MenuDataItem`).
#[derive(Clone, Debug, PartialEq)]
pub enum MenuDataItem {
    Item(MenuDataEntry),
    Separator(ListSeparator),
    Group {
        label: Option<SharedString>,
        items: MenuData,
    },
    Checkbox {
        value: SharedString,
        label: SharedString,
        checked: Option<bool>,
        default_checked: bool,
        disabled: bool,
        /// Whether toggling closes the menu. Defaults to false.
        close_on_select: bool,
    },
    RadioGroup {
        name: SharedString,
        label: Option<SharedString>,
        /// The selected value, to control the group: `Some(None)` selects
        /// nothing.
        value: Option<Option<SharedString>>,
        default_value: Option<SharedString>,
        items: Vec<MenuDataRadio>,
    },
    Submenu {
        label: SharedString,
        items: MenuData,
        disabled: bool,
    },
}

impl MenuDataItem {
    /// A line between items.
    pub fn separator() -> Self {
        MenuDataItem::Separator(ListSeparator)
    }
}

impl From<&str> for MenuDataItem {
    fn from(text: &str) -> Self {
        MenuDataItem::Item(ListItem::from(text))
    }
}

impl From<MenuDataEntry> for MenuDataItem {
    fn from(entry: MenuDataEntry) -> Self {
        MenuDataItem::Item(entry)
    }
}

/// Options for opening a menu from a controller or its api: where focus
/// goes once it opens, and the popup's own (`MenuOpenOptions`).
pub struct MenuOpenOptions<P = ()> {
    /// Where focus goes once it opens. Defaults to the first item.
    pub focus: Option<Focus>,
    pub reason: Option<Reason>,
    pub event: Option<InputEvent>,
    pub source: Option<ChangeSource>,
    /// Data for the content to render, like the row it acts on. On an open
    /// menu it replaces the current one, and an anchor passed with it moves
    /// the menu. `Some(None)` clears it.
    pub payload: Option<Option<P>>,
    /// Positions the menu against this instead of its trigger.
    pub anchor: Option<Anchor>,
    /// Closes the menu, with reason `Imperative`, when it aborts.
    pub signal: Option<AbortSignal>,
}

impl<P> Default for MenuOpenOptions<P> {
    fn default() -> Self {
        Self {
            focus: None,
            reason: None,
            event: None,
            source: None,
            payload: None,
            anchor: None,
            signal: None,
        }
    }
}

impl<P> MenuOpenOptions<P> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn focus(mut self, focus: Focus) -> Self {
        self.focus = Some(focus);
        self
    }

    pub fn reason(mut self, reason: Reason) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn event(mut self, event: impl Into<InputEvent>) -> Self {
        self.event = Some(event.into());
        self
    }

    pub fn source(mut self, source: ChangeSource) -> Self {
        self.source = Some(source);
        self
    }

    pub fn payload(mut self, payload: impl Into<Option<P>>) -> Self {
        self.payload = Some(payload.into());
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.anchor = Some(anchor.into());
        self
    }

    pub fn signal(mut self, signal: AbortSignal) -> Self {
        self.signal = Some(signal);
        self
    }
}

impl<P> From<PopupOpenOptions<P>> for MenuOpenOptions<P> {
    fn from(options: PopupOpenOptions<P>) -> Self {
        Self {
            focus: None,
            reason: options.reason,
            event: options.event,
            source: options.source,
            payload: options.payload,
            anchor: options.anchor,
            signal: options.signal,
        }
    }
}

/// Opens, closes and inspects a menu from outside it, before it's made or
/// without its entity at hand: a command palette, a hotkey. Pass it as the
/// `controller` option; its methods are `PopupControllerExt`'s. `P` is the
/// type of the data triggers and `open()` pass to the content.
pub type MenuController<P = ()> = PopupController<Menu<P>>;

/// A controller for a menu (`createMenuController`).
pub fn create_menu_controller<P: Clone + PartialEq + Debug + 'static>() -> MenuController<P> {
    create_popup_controller()
}

/// The `modal` a menu takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuModal {
    /// Outside presses reach what they hit.
    False,
    /// Also blocks the rest of the window and its scrolling, and traps focus.
    True,
    /// An outside press only closes the menu.
    Pointer,
}

impl MenuModal {
    /// The original's value, like `"pointer"`.
    pub fn as_str(self) -> &'static str {
        match self {
            MenuModal::False => "false",
            MenuModal::True => "true",
            MenuModal::Pointer => "pointer",
        }
    }
}

impl From<bool> for MenuModal {
    fn from(modal: bool) -> Self {
        if modal { MenuModal::True } else { MenuModal::False }
    }
}

impl From<MenuModal> for Modal {
    fn from(modal: MenuModal) -> Self {
        match modal {
            MenuModal::False => Modal::False,
            MenuModal::True => Modal::True,
            MenuModal::Pointer => Modal::Pointer,
        }
    }
}

/// The options `Menu` takes (`MenuOptions`). A submenu takes `open`,
/// `default_open`, `on_open_change`, `on_before_open_change`,
/// `on_open_change_complete`, `loop`, `delay`, `controller` and `debug`, with
/// its parent in `parent`.
pub struct MenuOptions<P: Clone + PartialEq + Debug + 'static = ()> {
    /// Whether the menu is open. Pass it to control the menu.
    pub open: Option<bool>,
    /// Whether the menu starts open, when uncontrolled. Defaults to false.
    pub default_open: bool,
    /// Runs when the menu opens or closes, with what closed it.
    /// `details.reason` says why (see [`MenuOpenChangeReason`]). It runs once
    /// the menu's update is over, so it may update any entity, the menu too.
    pub on_open_change: Option<PopupOpenChangeCallback>,
    /// Runs as the menu is asked to open or close, before it does, inside the
    /// menu's update, with what closed it: `details.cancel()` stops it. It
    /// may update any entity but the menu.
    pub on_before_open_change: Option<PopupOpenChangeCallback>,
    /// Runs once the menu has finished opening or closing, after any
    /// animation.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Runs when the payload the content shows changes: a trigger or context
    /// area with another payload opens the menu, or takes the open menu over,
    /// which fires no `on_open_change`, and so does a controller's
    /// `open(payload)` on the open menu. It runs once the menu's update is
    /// over, so it may update any entity, the menu too.
    pub on_payload_change: Option<ChangeCallback<Option<P>>>,
    /// Runs as the payload is asked to change, before it does, inside the
    /// menu's update: `details.cancel()` keeps the previous payload. It may
    /// update any entity but the menu.
    pub on_before_payload_change: Option<ChangeCallback<Option<P>>>,
    /// Runs for every plain item with a value, including items in submenus
    /// (see [`MenuActionCallback`]).
    pub on_action: Option<MenuActionCallback>,
    /// What screen readers hear once an item has been pending for a second,
    /// so a slow action doesn't pass in silence. Set it in the app's
    /// language. Empty says nothing. Defaults to "Loading".
    pub pending_announcement: Option<SharedString>,
    /// Runs when any checkbox item in the menu or its submenus toggles, with
    /// the item's value in `details.value`. Made for data menus. It runs once
    /// the menu's update is over, so it may update any entity, the menu too.
    pub on_checked_change: Option<MenuCheckedChangeCallback>,
    /// Runs as any checkbox item in the menu or its submenus is asked to
    /// toggle, before it does, inside the menu's update, with the item's
    /// value in `details.value`: `details.cancel()` stops it. It may update
    /// any entity but the menu.
    pub on_before_checked_change: Option<MenuCheckedChangeCallback>,
    /// Runs when any radio group in the menu or its submenus changes, with
    /// the group in `details.group`. Made for data menus. It runs once the
    /// menu's update is over, so it may update any entity, the menu too.
    pub on_value_change: Option<MenuValueChangeCallback>,
    /// Runs as any radio group in the menu or its submenus is asked to
    /// change, before it does, inside the menu's update, with the group in
    /// `details.group`: `details.cancel()` stops it. It may update any entity
    /// but the menu.
    pub on_before_value_change: Option<MenuValueChangeCallback>,
    /// The menu as data. Its items' shortcuts run while the menu is closed.
    pub items: Option<MenuData>,
    /// Whether a data menu runs its items' shortcuts while closed. Defaults
    /// to true.
    pub item_shortcuts: bool,
    /// Whether arrow keys wrap from the last item to the first. Defaults to
    /// true.
    pub r#loop: bool,
    /// How long the pointer rests on a submenu trigger before its submenu
    /// opens. Defaults to 100 milliseconds.
    pub delay: Duration,
    /// How the menu presents. `Sheet` slides it up from the bottom of the
    /// window, where it can be swiped away and submenus open in its place,
    /// and `Auto` does that on small touch screens. Decided each time it
    /// opens. A context menu and a menu in a menubar are always popovers.
    /// Defaults to `Default`, a popover.
    pub presentation: PresentationSetting,
    /// Opens, closes and inspects the menu from anywhere. Create one with
    /// [`create_menu_controller`].
    pub controller: Option<MenuController<P>>,
    /// `Pointer`: an outside press only closes the menu. `True`: also blocks
    /// the rest of the window and its scrolling, and traps focus. `False`:
    /// outside presses reach what they hit. Defaults to `Pointer`, or `True`
    /// while it presents as a sheet, or its menubar's.
    pub modal: Option<MenuModal>,
    /// Whether Escape, an outside press, focus or the window leaving, the
    /// back gesture and a swipe can close it. Triggers, items and the
    /// controller always can. Defaults to true.
    pub dismissible: bool,
    /// Runs when `dismissible` refused a dismissal made on purpose (see
    /// [`MenuDismissReason`]), once the menu's update is over.
    pub on_dismiss_attempt: Option<DismissAttemptCallback>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
    /// The menu this one is a submenu of, read once, as the menu is made.
    pub parent: Option<WeakEntity<Menu<P>>>,
    /// The menubar a root menu sits in, read once, as the menu is made. Its
    /// trigger becomes one of the menubar's items.
    pub menubar: Option<WeakEntity<Menubar>>,
}

impl<P: Clone + PartialEq + Debug + 'static> Default for MenuOptions<P> {
    fn default() -> Self {
        Self {
            open: None,
            default_open: false,
            on_open_change: None,
            on_before_open_change: None,
            on_open_change_complete: None,
            on_payload_change: None,
            on_before_payload_change: None,
            on_action: None,
            pending_announcement: None,
            on_checked_change: None,
            on_before_checked_change: None,
            on_value_change: None,
            on_before_value_change: None,
            items: None,
            item_shortcuts: true,
            r#loop: true,
            delay: Duration::from_millis(100),
            presentation: PresentationSetting::Default,
            controller: None,
            modal: None,
            dismissible: true,
            on_dismiss_attempt: None,
            debug: false,
            parent: None,
            menubar: None,
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Clone for MenuOptions<P> {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_before_open_change: self.on_before_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            on_before_payload_change: self.on_before_payload_change.clone(),
            on_action: self.on_action.clone(),
            pending_announcement: self.pending_announcement.clone(),
            on_checked_change: self.on_checked_change.clone(),
            on_before_checked_change: self.on_before_checked_change.clone(),
            on_value_change: self.on_value_change.clone(),
            on_before_value_change: self.on_before_value_change.clone(),
            items: self.items.clone(),
            item_shortcuts: self.item_shortcuts,
            r#loop: self.r#loop,
            delay: self.delay,
            presentation: self.presentation,
            controller: self.controller.clone(),
            modal: self.modal,
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            debug: self.debug,
            parent: self.parent.clone(),
            menubar: self.menubar.clone(),
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> MenuOptions<P> {
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// `on_open_change`, for a callback that doesn't read what closed it.
    pub fn on_open_change(mut self, callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change = Some(Rc::new(move |open, details, _, window, cx| {
            callback(open, details, window, cx)
        }));
        self
    }

    /// `on_open_change` with what closed it, like a controller's
    /// `close` return value.
    pub fn on_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    /// `on_before_open_change`, for a callback that doesn't read what closes
    /// it.
    pub fn on_before_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change = Some(Rc::new(move |open, details, _, window, cx| {
            callback(open, details, window, cx)
        }));
        self
    }

    /// `on_before_open_change` with what closes it, like a controller's
    /// `close` return value.
    pub fn on_before_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn on_payload_change(
        mut self,
        callback: impl Fn(&Option<P>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_payload_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_payload_change(
        mut self,
        callback: impl Fn(&Option<P>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_payload_change = Some(Rc::new(callback));
        self
    }

    pub fn on_action(
        mut self,
        callback: impl Fn(&SharedString, &MenuActionDetails, &mut Window, &mut App) -> ActionOutcome + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(callback));
        self
    }

    pub fn pending_announcement(mut self, text: impl Into<SharedString>) -> Self {
        self.pending_announcement = Some(text.into());
        self
    }

    pub fn on_checked_change(
        mut self,
        callback: impl Fn(&bool, &MenuCheckedChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_checked_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_checked_change(
        mut self,
        callback: impl Fn(&bool, &MenuCheckedChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_checked_change = Some(Rc::new(callback));
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&SharedString, &MenuValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&SharedString, &MenuValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn items(mut self, items: MenuData) -> Self {
        self.items = Some(items);
        self
    }

    pub fn item_shortcuts(mut self, item_shortcuts: bool) -> Self {
        self.item_shortcuts = item_shortcuts;
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
        self
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn presentation(mut self, presentation: PresentationSetting) -> Self {
        self.presentation = presentation;
        self
    }

    pub fn controller(mut self, controller: MenuController<P>) -> Self {
        self.controller = Some(controller);
        self
    }

    pub fn modal(mut self, modal: impl Into<MenuModal>) -> Self {
        self.modal = Some(modal.into());
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(
        mut self,
        callback: impl Fn(&ui_sdk_kernel::DismissAttemptDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    /// Makes this a submenu of `parent`.
    pub fn parent(mut self, parent: &Entity<Menu<P>>) -> Self {
        self.parent = Some(parent.downgrade());
        self
    }

    /// Puts the menu in `menubar`.
    pub fn menubar(mut self, menubar: &Entity<Menubar>) -> Self {
        self.menubar = Some(menubar.downgrade());
        self
    }

    // What the popup reads of them.
    fn popup(&self) -> PopupOptions<P> {
        PopupOptions {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_before_open_change: self.on_before_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            on_before_payload_change: self.on_before_payload_change.clone(),
            modal: None,
            presentation: Some(self.presentation),
            open_on_hover: false,
            delay: None,
            close_delay: None,
            disabled: false,
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
        }
    }

    // What every menu of the tree reads of the root's.
    fn shared(&self) -> Shared<P> {
        Shared {
            on_action: self.on_action.clone(),
            on_checked_change: self.on_checked_change.clone(),
            on_before_checked_change: self.on_before_checked_change.clone(),
            on_value_change: self.on_value_change.clone(),
            on_before_value_change: self.on_before_value_change.clone(),
            pending_announcement: self.pending_announcement.clone(),
            modal: self.modal,
            presentation: self.presentation,
            _payload: std::marker::PhantomData,
        }
    }
}

/// The props the `trigger` slot takes. A trigger outside the menu, like one
/// per table row, is a trigger like any other: any part of the window can
/// take the menu's entity.
pub type MenuTriggerProps<P = ()> = TriggerProps<P>;

/// The props the `context-trigger` slot takes.
pub type MenuContextTriggerProps<P = ()> = ContextTriggerProps<P>;

/// The props the `item` slot takes.
pub type MenuItemProps = ItemProps;

/// The props the `checkbox-item` slot takes.
pub type MenuCheckboxItemProps = CheckboxProps;

/// The props the `radio-group` slot takes.
pub type MenuRadioGroupProps = RadioGroupProps;

/// The props the `radio-item` slot takes.
pub type MenuRadioItemProps = RadioProps;

/// The props the `item-indicator` slot takes.
pub type MenuItemIndicatorProps = IndicatorProps;

/// Where focus goes when the menu closes while focus was inside it.
/// `Trigger` returns it to the trigger and `None` leaves it. A focus handle
/// or a function picks a specific element.
pub type MenuFinalFocus = PopupFocusTarget;

/// The props the `content` slot takes.
pub type MenuContentProps = ContentProps;

/// The props a submenu's `trigger` takes, the trigger inside a parent menu.
pub type MenuSubTriggerProps = SubTriggerProps;

/// The props the `shortcut` slot takes: keys of its own, else the shortcut
/// of the item it sits in.
pub type MenuShortcutProps = ShortcutProps;

/// What the `shortcut` part styles from: its keys formatted for this
/// platform, `⌘S` on Apple platforms and `Ctrl+S` elsewhere.
pub type MenuShortcutState = ShortcutState;

/// What the `trigger` part styles from (`MenuTriggerState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuTriggerState {
    /// Whether the menu is open from this trigger.
    pub open: bool,
    /// Whether the trigger's `disabled` prop is set, or the menubar it's in
    /// is disabled.
    pub disabled: bool,
    /// A submenu's trigger: whether it has keyboard focus or is under the
    /// pointer in its parent menu (`data-highlighted`).
    pub highlighted: bool,
    /// A submenu's or a menubar's trigger, an item: whether it has an icon
    /// (`data-has-icon`).
    pub has_icon: bool,
}

/// What the `context-trigger` part styles from (`MenuContextTriggerState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuContextTriggerState {
    /// Whether the menu is open from this area.
    pub open: bool,
    /// Whether the area's `disabled` prop is set.
    pub disabled: bool,
}

/// What the `content` part styles from (`MenuContentState`), with what the
/// original writes as attributes and variables: its presence, where it's
/// placed, and its sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuContentState<P = ()> {
    /// Whether the menu is open. False while it animates out.
    pub open: bool,
    /// Whether this is a submenu.
    pub nested: bool,
    /// How the menu is shown.
    pub presentation: Presentation,
    /// The payload of the last open, from the trigger that opened it or a
    /// controller. Submenus show their root's.
    pub payload: Option<P>,
    /// Its enter and exit, the original's `data-starting-style` and
    /// `data-ending-style`.
    pub presence: PresenceState,
    /// Where it's placed, as of the last frame: its side, alignment and room
    /// (`data-side`, `--available-height`, ..).
    pub placement: Option<Placement>,
    /// Kept in the page while closed, it renders hidden.
    pub hidden: bool,
    /// The sheet it presents as, while it does.
    pub sheet: Option<SheetPartState>,
    /// Whether one of its items has an icon (`data-has-icons`).
    pub has_icons: bool,
}

/// What the `item` part styles from (`MenuItemState`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuItemState {
    /// Whether the item has keyboard focus or is under the pointer.
    pub highlighted: bool,
    /// Whether the item can't be chosen.
    pub disabled: bool,
    /// Whether its `on_select` or `on_action` work is running.
    pub pending: bool,
    /// Why its last action failed, until it runs again.
    pub error: Option<SharedString>,
    /// Whether it's styled as dangerous (`data-destructive`).
    pub destructive: bool,
    /// Whether it has an icon (`data-has-icon`).
    pub has_icon: bool,
}

/// What the `checkbox-item` part styles from (`MenuCheckboxItemState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuCheckboxItemState {
    /// Whether the item has keyboard focus or is under the pointer.
    pub highlighted: bool,
    /// Whether the item can't be chosen.
    pub disabled: bool,
    /// Whether the item is on: its `checked` prop when you pass one,
    /// otherwise `default_checked` until a press changes it.
    pub checked: bool,
    /// Whether it has an icon (`data-has-icon`).
    pub has_icon: bool,
}

/// What the `radio-item` part styles from (`MenuRadioItemState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuRadioItemState {
    /// Whether the item has keyboard focus or is under the pointer.
    pub highlighted: bool,
    /// Whether the item can't be chosen.
    pub disabled: bool,
    /// Whether the item is its group's value.
    pub checked: bool,
    /// Whether it has an icon (`data-has-icon`).
    pub has_icon: bool,
}

/// What the `radio-group` part styles from (`MenuRadioGroupState`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuRadioGroupState {
    /// The selected value, or `None` when nothing is selected.
    pub value: Option<SharedString>,
}

/// What the `item-icon` part styles from: whether it flips in right-to-left
/// text.
pub type MenuItemIconState = ItemIconState;

/// The props the `item-icon` slot takes: whether it flips in right-to-left
/// text, and the icon itself.
pub type MenuItemIconProps = ItemIconProps;

/// What the `item-indicator` part styles from (`MenuItemIndicatorState`).
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItemIndicatorState {
    /// Whether its checkbox item is on, or its radio item is its group's
    /// value. While false, the indicator isn't shown unless `keep_mounted`
    /// is set.
    pub checked: bool,
    /// Whether the item can't be chosen.
    pub disabled: bool,
    /// Its enter as its item becomes checked.
    pub presence: PresenceState,
}

/// A menu (`menu`), or a submenu of one. Its entity is its controller.
pub struct Menu<P: Clone + PartialEq + Debug + 'static = ()> {
    pub(crate) instance: Instance,
    pub(crate) popup: Popup<Self>,
    pub(crate) shared: Rc<MenuShared<P>>,
    pub(crate) controller: ConnectedController<Self>,
    pub(crate) items: Option<MenuData>,
    pub(crate) item_shortcuts: bool,
    pub(crate) r#loop: bool,
    pub(crate) delay: Duration,
    // Apple keyboard conventions for shortcut names.
    pub(crate) apple: bool,
    // What presses set on checkbox items and radio groups that aren't
    // controlled (`values`).
    pub(crate) checked: HashMap<SharedString, bool>,
    pub(crate) radios: HashMap<SharedString, SharedString>,
    // Each radio group's props, by name, as of its last render.
    pub(crate) radio_groups: RefCell<HashMap<SharedString, RadioGroupProps>>,
    // Items whose `on_select` or `on_action` work hasn't settled, and the
    // last one that failed.
    pub(crate) pending: Vec<SharedString>,
    pub(crate) error: Option<(SharedString, SharedString)>,
    pub(crate) announce: AnnouncePending,
    pub(crate) typeahead: Typeahead,
    // Counts closes, so an item's work that settles after the menu closed
    // doesn't close it again once it's reopened.
    pub(crate) session: u64,
    // `None` until the pointer first moves, so the first move always counts.
    pub(crate) pointer: Option<gpui::Point<gpui::Pixels>>,
    pub(crate) press_click: bool,
    // Opened by resting the pointer on the trigger, so the press that
    // usually follows keeps it open instead of closing it.
    pub(crate) opened_by_hover: bool,
    pub(crate) press: Option<PressSession>,
    // What each trigger acts as, by key (`triggerElements`).
    pub(crate) trigger_elements: RefCell<HashMap<SharedString, ButtonElement>>,
    // The long press of each context area, by key.
    pub(crate) holds: RefCell<HashMap<SharedString, Hold<Self>>>,
    // Each item's indicator enter, by its item.
    pub(crate) indicators: RefCell<HashMap<SharedString, Presence>>,
    // Focus landing on each item, which highlights it (`focusin`).
    pub(crate) item_focus: HashMap<SharedString, Subscription>,
    pub(crate) left_check: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl<P: Clone + PartialEq + Debug + 'static> PopupHost for Menu<P> {
    type Payload = P;

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

// What only a menu does, beside its preset's behavior.
fn rules<P: Clone + PartialEq + Debug + 'static>(me: Weak<MenuShared<P>>, nested: bool) -> PopupRules<Menu<P>> {
    let trigger = {
        let me = me.clone();
        Rc::new(move || me.upgrade().and_then(|shared| shared.trigger_element())) as ui_sdk_popups::RegionFn
    };
    // A context area counts as inside only for the secondary button, which
    // moves the open menu instead of closing it.
    let contains = {
        let me = me.clone();
        Rc::new(move |target: &Target, event: Option<&InputEvent>, window: &Window, cx: &App| {
            let Some(shared) = me.upgrade() else {
                return false;
            };
            // Submenus close first, so focus in one rendered elsewhere still
            // returns with this menu's.
            if shared
                .children()
                .iter()
                .any(|child| child.tree_contains(target, window))
            {
                return true;
            }
            // A press on another trigger of the menubar switches menus
            // instead.
            if shared
                .menubar
                .as_ref()
                .is_some_and(|bar| bar.shared.on_trigger(target, window))
            {
                return true;
            }
            // So does a press on another trigger of this menu, which takes it
            // over instead of closing and reopening it. Focus moving there
            // still leaves it.
            if event.is_some() && shared.presses_trigger(target, window) {
                return true;
            }
            // A secondary press on another of its context areas moves it there.
            let secondary = event.is_some_and(|event| secondary_press(event, cx));
            if secondary && shared.presses_context(target, window) {
                return true;
            }
            let (el, context) = shared.active_trigger();
            let Some(el) = el else {
                return false;
            };
            if !el.contains(target, window) {
                return false;
            }
            !context || secondary
        }) as ui_sdk_popups::ContainsFn
    };
    PopupRules {
        trigger: Some(trigger),
        dismiss: Some(Rc::new(|this: &mut Menu<P>, reason, details, window, cx| {
            this.dismiss(reason, details, window, cx)
        })),
        contains: Some(contains),
        modal: Some(Rc::new(|this: &Menu<P>| this.modal_effects())),
        persistent: Some(Rc::new(|this: &Menu<P>| this.persistent())),
        parent: nested.then(|| {
            Rc::new(|this: &Menu<P>| this.shared.parent().and_then(|parent| parent.popup.layer().layer()))
                as Rc<dyn Fn(&Menu<P>) -> Option<ui_sdk_platform::Layer>>
        }),
        presentation: Some(Rc::new(|this: &Menu<P>| this.presentation_setting())),
        on_open_change: Some(Rc::new(|this: &mut Menu<P>, open, _closed, window, cx| {
            this.layer_open_changed(open, window, cx)
        })),
        on_escape: nested.then(|| {
            Rc::new(
                |this: &mut Menu<P>, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Menu<P>>| {
                    this.escape(event, window, cx)
                },
            ) as EscapeFn<Menu<P>>
        }),
        open: Some(Rc::new(|this: &mut Menu<P>, request, step, window, cx| {
            this.open_rule(request, step, window, cx)
        })),
        reopen: Some(Rc::new(|this: &mut Menu<P>, request, _step, window, cx| {
            this.reopen_rule(request, window, cx)
        })),
        close: Some(Rc::new(|this: &mut Menu<P>, request, reason, step, window, cx| {
            this.close_rule(request, reason, step, window, cx)
        })),
        bind: Some(Rc::new(|this: &Menu<P>| this.content_bind())),
        ..PopupRules::default()
    }
}

// A press with the secondary button, or a Control-click on a Mac: what
// opens a context menu.
fn secondary_press(event: &InputEvent, cx: &App) -> bool {
    match event {
        InputEvent::MouseDown(down) => {
            down.button == MouseButton::Right
                || (down.button == MouseButton::Left && down.modifiers.control && is_apple(cx))
        }
        _ => false,
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Menu<P> {
    // How the content binds: placed against the menu's anchor, with focus
    // going where the open asked. In a sheet, a submenu shows in its
    // parent's place, under its trigger. What it places and focuses is read
    // as it happens, from the shared state.
    fn content_bind(&self) -> SheetConfig<Self> {
        let me = Rc::downgrade(&self.shared);
        let anchor = {
            let me = me.clone();
            Rc::new(move || me.upgrade().and_then(|shared| shared.anchor())) as Rc<dyn Fn() -> Option<Anchor>>
        };
        let position = {
            let me = me.clone();
            Rc::new(move || me.upgrade().map(|shared| shared.position()).unwrap_or_default())
                as Rc<dyn Fn() -> ui_sdk_platform::PositionOptions>
        };
        let initial_focus = {
            let me = me.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                me.upgrade().and_then(|shared| shared.run_pending_focus(window, cx))
            }) as InitialFocusFn
        };
        let drill_in = self.shared.parent.clone().map(|parent| {
            Rc::new(move || {
                let parent = parent.upgrade().and_then(|parent| parent.popup.layer().element());
                let trigger = me.upgrade().and_then(|shared| shared.trigger_element());
                (parent, trigger)
            }) as DrillIn
        });
        SheetConfig {
            attach: AttachConfig {
                anchor: Some(anchor),
                position: Some(position),
                initial_focus: Some(initial_focus),
                ..AttachConfig::default()
            },
            drill_in,
            ..SheetConfig::default()
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Menu<P> {
    /// A menu, or a submenu with `parent` in its options. Apps make it with
    /// `cx.new(|cx| Menu::new(options, window, cx))`, outside the parent's
    /// own update for a submenu.
    pub fn new(options: MenuOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("menu").with_agent(cx);
        instance.debug = options.debug;
        let parent = options
            .parent
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .map(|parent| parent.read(cx).shared.clone());
        let nested = parent.is_some();
        // The menubar a root menu sits in. Submenus belong to their root's.
        let menubar = if nested {
            None
        } else {
            options
                .menubar
                .as_ref()
                .and_then(WeakEntity::upgrade)
                .map(|bar| MenubarLink::of(&bar, cx))
        };
        let id = instance.id().clone();
        let entity = cx.weak_entity();
        let handle = instance.handle();
        let shared = Rc::new_cyclic(|me: &Weak<MenuShared<P>>| {
            // The root closes the whole tree when the window loses focus.
            let behavior =
                PopupBehaviorOverrides::default().blur(if nested { PopupBlur::Never } else { PopupBlur::Always });
            let popup = create_popup(
                &instance,
                CreatePopupOptions::new(menu_preset())
                    .rules(rules(me.clone(), nested))
                    .behavior(behavior)
                    .options(options.popup()),
                window,
                cx,
            );
            let hover_intent = create_hover_intent(
                HoverIntentOptions {
                    trigger: Rc::new(|_: &Menu<P>| None),
                    popup: Rc::new(|_: &Menu<P>| None),
                    closes_on_leave: Rc::new(|_: &Menu<P>| false),
                    close_delay: Rc::new(|_: &Menu<P>| Duration::ZERO),
                    close: Rc::new(|_: &mut Menu<P>, _, _, _| {}),
                },
                cx,
            );
            MenuShared {
                id: id.clone(),
                entity,
                instance: handle,
                popup,
                parent: parent.as_ref().map(Rc::downgrade),
                menubar,
                children: RefCell::new(Vec::new()),
                entries: Parts::new(),
                regions: RefCell::new(HashMap::new()),
                highlighted: RefCell::new(None),
                open_child: RefCell::new(None),
                pending_focus: Cell::new(Focus::None),
                trigger_entry: format!("{id}-trigger").into(),
                context_triggers: RefCell::new(HashMap::new()),
                settings: RefCell::new(options.shared()),
                safe_area: create_safe_area(),
                hover_intent,
                scroll: ScrollHandle::new(),
                owns_key: Cell::new(false),
                content: RefCell::new(ContentProps::default()),
                inset: Cell::new(px(0.)),
                rem_size: Cell::new(px(16.)),
                rtl: Cell::new(is_rtl(cx)),
            }
        });
        if let Some(parent) = &parent {
            parent.children.borrow_mut().push(Rc::downgrade(&shared));
        }
        // The triggers in the page, whose leaving may close the menu.
        key_while_bound(&instance, "trigger");
        let this = cx.entity();
        let watch = cx.observe_in(&this, window, |menu: &mut Self, _, window, cx| {
            menu.check_triggers(window, cx)
        });
        let release = cx.on_release(|menu: &mut Self, _cx| {
            while_destroying(|| {
                if let Some(bar) = &menu.shared.menubar {
                    bar.shared.leave_member(&menu.shared.id);
                }
            })
        });
        let popup = shared.popup.clone();
        Self {
            instance,
            popup,
            shared,
            controller: ConnectedController::new(options.controller.clone(), cx),
            items: options.items,
            item_shortcuts: options.item_shortcuts,
            r#loop: options.r#loop,
            delay: options.delay,
            apple: initial_apple(cx),
            checked: HashMap::new(),
            radios: HashMap::new(),
            radio_groups: RefCell::new(HashMap::new()),
            pending: Vec::new(),
            error: None,
            announce: AnnouncePending::new(),
            typeahead: Typeahead::new(TYPEAHEAD_TIMEOUT),
            session: 0,
            pointer: None,
            press_click: false,
            opened_by_hover: false,
            press: None,
            trigger_elements: RefCell::new(HashMap::new()),
            holds: RefCell::new(HashMap::new()),
            indicators: RefCell::new(HashMap::new()),
            item_focus: HashMap::new(),
            left_check: None,
            _subscriptions: vec![watch, release],
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). `parent` and `menubar` are read once, as the menu is made.
    pub fn set_options(&mut self, options: MenuOptions<P>, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        self.controller.set(options.controller.clone());
        // A controlled close clears the highlight in the same pass.
        if derive_open(options.open, self.popup.is_open(), |open| !open) == Some(true) {
            self.shared.highlighted.borrow_mut().take();
        }
        *self.shared.settings.borrow_mut() = options.shared();
        let changed = self.items != options.items
            || self.item_shortcuts != options.item_shortcuts
            || self.r#loop != options.r#loop;
        self.items = options.items.clone();
        self.item_shortcuts = options.item_shortcuts;
        self.r#loop = options.r#loop;
        self.delay = options.delay;
        let popup = self.popup.clone();
        popup.set_options(options.popup(), cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled `open`, or `None` to let the menu keep its own.
    pub fn set_controlled_open(&mut self, open: Option<bool>, cx: &mut Context<Self>) {
        if derive_open(open, self.popup.is_open(), |open| !open) == Some(true) {
            self.shared.highlighted.borrow_mut().take();
        }
        let popup = self.popup.clone();
        popup.set_controlled_open(open, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The popup it's built on, the original's `internal.popup`.
    pub fn popup(&self) -> &Popup<Self> {
        &self.popup
    }

    /// Whether it's open. False as soon as it starts closing, while it still
    /// animates out (`isOpen`).
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// Whether it's in the window, including while it animates out
    /// (`isMounted`).
    pub fn is_mounted(&self) -> bool {
        self.popup.is_mounted()
    }

    /// Whether it's a submenu.
    pub fn is_nested(&self) -> bool {
        self.shared.is_nested()
    }

    /// The payload of the last open, from a controller or the trigger that
    /// opened it (`getPayload`). A submenu's content shows its root's.
    pub fn payload(&self) -> Option<P> {
        self.popup.payload()
    }

    /// How it presents while open (`presentation`).
    pub fn presentation(&self) -> Presentation {
        self.shared.presentation()
    }

    /// The item that has keyboard focus or is under the pointer, by key.
    pub fn highlighted(&self) -> Option<SharedString> {
        self.shared.highlighted.borrow().clone()
    }

    /// Opens or closes it. Returns false when the change was canceled or it
    /// was already open or closed (`setOpen`).
    pub fn set_open(
        &mut self,
        open: bool,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let control = self.popup.control().clone();
        control.set_open(self, open, options, window, cx)
    }

    /// Opens it, and resolves once it closes, with why, how and any return
    /// value (`open`). Focus goes to its first item unless `focus` says
    /// otherwise. On an open menu, a payload replaces the current one and an
    /// anchor moves it.
    pub fn open(&mut self, options: MenuOpenOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        self.show(
            Show {
                focus: options.focus,
                reason: Some(options.reason.unwrap_or(Reason::Imperative)),
                event: options.event,
                source: options.source,
                payload: options.payload,
                anchor: options.anchor,
                signal: options.signal,
                ..Show::default()
            },
            window,
            cx,
        )
    }

    /// Closes it, and its submenus. `return_value` is passed to the future
    /// `open()` returned (`close`).
    pub fn close(&mut self, options: PopupCloseOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.hide(
            Close {
                reason: options.reason,
                event: options.event,
                focus: None,
                return_value: options.return_value,
            },
            window,
            cx,
        )
    }

    /// Opens it when it's closed, and closes it when it's open (`toggle`).
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let control = self.popup.control().clone();
        control.toggle(self, options, window, cx)
    }

    /// Moves focus to the trigger that opened it last, or the first trigger
    /// (`focus`).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.shared.focus_trigger(window, cx);
    }

    // The trigger that opened the menu left, like a row a filter hid, so
    // nothing updates its payload anymore. A task later, so a trigger that
    // renders again lands first (the trigger's cleanup).
    fn check_triggers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shared.is_nested() || self.left_check.is_some() || !self.popup.is_open() {
            return;
        }
        let Some(key) = self.popup.trigger_key() else {
            return;
        };
        if self.popup.bound_triggers().contains(&key) || self.popup.trigger_region(&key).is_none() {
            return;
        }
        self.left_check = Some(cx.spawn_in(window, async move |this, cx| {
            this.update_in(cx, |menu, window, cx| {
                menu.left_check = None;
                let left = menu.popup.trigger_key().as_ref() == Some(&key)
                    && menu.popup.is_open()
                    && menu.popup.layer().element().is_some()
                    && menu.shared.trigger_element().is_none();
                if left {
                    menu.hide(Close::new(Reason::Imperative, None), window, cx);
                }
            })
            .ok();
        }));
    }
}

/// `agent: { actions: ["setOpen"] }`, with `open`, `close` and `toggle`,
/// which `setOpen` covers, unlisted.
impl<P: Clone + PartialEq + Debug + 'static> Agent for Menu<P> {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", |menu, args, window, cx| {
            let open = args.boolean(0)?;
            Ok(menu.set_open(open, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("open", |menu, _args, window, cx| {
            let _pending = menu.open(MenuOpenOptions::default(), window, cx);
            Ok(PENDING.into())
        }),
        AgentAction::unlisted("close", |menu, _args, window, cx| {
            Ok(menu.close(PopupCloseOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |menu, _args, window, cx| {
            Ok(menu.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        let pending: Vec<String> = self.pending.iter().map(ToString::to_string).collect();
        let error: Option<String> = self.error.as_ref().map(|(_, reason)| reason.to_string());
        AgentState::new()
            .with("open", self.is_open())
            .with("pending", pending)
            .with("error", error)
    }
}
