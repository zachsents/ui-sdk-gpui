//! Port of `components/menu/menu-api.ts`: `menu`'s api, which its parts and
//! its controller call, the props its parts take, and what the menus of one
//! tree see of each other.
//!
//! A submenu is a menu of its own, an entity made with its parent in its
//! options. The menus of a tree reach each other through what they share
//! (`MenuShared`, the original's `MenuNode`): a parent's items and
//! highlight, the open child, the root's callbacks and modal setting, each
//! popup. A menu updates another inside its own update, which gpui allows for
//! any entity but the one updating, so a submenu that closes its whole tree,
//! after an item ran or Tab, asks its root once its own update is over: the
//! root closes its submenus first, the asking one included.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, Element, ElementId, FocusHandle, GlobalElementId, Hsla, InspectorElementId, KeyDownEvent,
    LayoutId, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollHandle, SharedString, Style, WeakEntity, Window,
    point, px,
};
use ui_sdk_kernel::{
    ActionError, ActionOutcome, ChangeCallback, ChangeSource, InputEvent, InstanceHandle, Key, Parts, Reason,
    SelectEvent, change_source, with_source,
};
use ui_sdk_motion::PresenceTiming;
use ui_sdk_platform::{
    Align, Anchor, ButtonElement, PositionOptions, PositionProps, Region, Side, Target, composed_closest,
};
use ui_sdk_popups::{
    AbortSignal, CloseDetails, CloseStep, HideRequest, HoverEvent, OpenStep, PlacementDefaults, PopupFocusTarget,
    PopupLayer, PopupModal, PopupOpener, PopupPending, PresentationSetting, ReturnMode, SafeArea, SettledShow,
    ShowRequest, drilled_into, modal_triple, opener_of, placement_options, sheet_modal,
};
use ui_sdk_shortcuts::{RegisteredShortcut, ShortcutKeys, ShortcutOptions, shortcuts};
use ui_sdk_text::is_typeahead_key;

use crate::menu::{Menu, MenuData, MenuDataItem, MenuModal};
use crate::menubar::MenubarLink;

/// Where focus goes once a menu opens (`Focus`): its first or last item, the
/// menu itself, or nowhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Focus {
    #[default]
    First,
    Last,
    Popup,
    None,
}

/// How a menu is shown (`Presentation`): beside its trigger, `Popover`, or
/// as a sheet from the bottom of the window, `Sheet`.
pub type Presentation = ui_sdk_popups::Presentation;

/// Runs when an item is selected, with what selected it. Call
/// `event.prevent_default()` to keep the menu open and skip `on_action`.
/// Return `ActionOutcome::Pending` to mark the item pending until its work
/// settles. It runs inside the menu's update, so it may update any entity
/// but the menu.
pub type SelectCallback = Rc<dyn Fn(&SelectEvent, &mut Window, &mut App) -> ActionOutcome>;

/// The props the `item` slot takes (`ItemProps`).
#[derive(Clone)]
pub struct ItemProps {
    /// Passed to `on_action` when the item runs.
    pub value: Option<SharedString>,
    /// The text typeahead matches. gpui keeps no text to read off an element,
    /// so an item without one is never found by typing. It isn't the
    /// accessible name: use `aria_label` or `text!(..)` for that.
    pub label: Option<SharedString>,
    /// Keeps the item focusable but stops it from running, as the menu
    /// pattern asks. Sets `disabled` in its state and `aria-disabled`.
    pub disabled: bool,
    /// Styles the item as dangerous, like Delete. Sets `destructive` in its
    /// state.
    pub destructive: bool,
    /// A keyboard shortcut like `"mod+n"`. A Shortcut part inside the item
    /// shows `⌘N` on Apple platforms and `Ctrl+N` elsewhere, and the item
    /// gets `aria-keyshortcuts`.
    pub shortcut: Option<SharedString>,
    /// Whether running the item closes the menu. Defaults to true.
    pub close_on_select: bool,
    /// Runs when the item is selected, by pointer or keyboard, with the event
    /// that selected it, or none when no event did. `event.prevent_default()`
    /// keeps the menu open and skips `on_action`; `ActionOutcome::Pending`
    /// marks the item pending until it settles, like `on_action`.
    pub on_select: Option<SelectCallback>,
}

impl Default for ItemProps {
    fn default() -> Self {
        Self {
            value: None,
            label: None,
            disabled: false,
            destructive: false,
            shortcut: None,
            close_on_select: true,
            on_select: None,
        }
    }
}

impl ItemProps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn close_on_select(mut self, close: bool) -> Self {
        self.close_on_select = close;
        self
    }

    pub fn on_select(
        mut self,
        on_select: impl Fn(&SelectEvent, &mut Window, &mut App) -> ActionOutcome + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }
}

/// The props the `trigger` slot takes (`TriggerProps`).
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerProps<P = ()> {
    /// Stops the trigger from opening the menu. A native button can't be
    /// focused or pressed, unless `focusable_when_disabled` is set.
    pub disabled: bool,
    /// Keeps a disabled trigger in the tab order with `aria-disabled`, so it
    /// can host a tooltip that says why. Presses still do nothing.
    pub focusable_when_disabled: bool,
    /// Data for the content to render when this trigger opens the menu, like
    /// the row it belongs to.
    pub payload: Option<P>,
    /// A keyboard shortcut that opens the menu from anywhere, like `"mod+j"`.
    /// `mod` is Command on Apple platforms and Control elsewhere. Sets
    /// `aria-keyshortcuts`.
    pub shortcut: Option<SharedString>,
    /// The text typeahead matches, for a submenu's trigger, an item of its
    /// parent menu, and a menubar's, an item of the menubar. gpui keeps no
    /// text to read off an element.
    pub label: Option<SharedString>,
    /// What the trigger acts as: a native button, the default, a link, or
    /// another element that acts as a button. The original reads it off the
    /// element it renders.
    pub element: ButtonElement,
}

impl<P> Default for TriggerProps<P> {
    fn default() -> Self {
        Self {
            disabled: false,
            focusable_when_disabled: false,
            payload: None,
            shortcut: None,
            label: None,
            element: ButtonElement::Button,
        }
    }
}

impl<P> TriggerProps<P> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn focusable_when_disabled(mut self, focusable: bool) -> Self {
        self.focusable_when_disabled = focusable;
        self
    }

    pub fn payload(mut self, payload: P) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn element(mut self, element: ButtonElement) -> Self {
        self.element = element;
        self
    }
}

/// The props a submenu's `trigger` takes, the trigger inside a parent menu
/// (`SubTriggerProps`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubTriggerProps {
    /// Keeps the submenu trigger focusable but stops it from opening its
    /// submenu, as the menu pattern asks. Sets `disabled` in its state and
    /// `aria-disabled`.
    pub disabled: bool,
    /// The text typeahead matches in the parent menu. gpui keeps no text to
    /// read off an element.
    pub label: Option<SharedString>,
}

impl SubTriggerProps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl<P> From<SubTriggerProps> for TriggerProps<P> {
    fn from(props: SubTriggerProps) -> Self {
        Self {
            disabled: props.disabled,
            label: props.label,
            ..Self::default()
        }
    }
}

/// The props the `context-trigger` slot takes (`ContextTriggerProps`).
#[derive(Clone, Debug, PartialEq)]
pub struct ContextTriggerProps<P = ()> {
    /// Stops a right-click, a long press or Shift+F10 from opening the menu
    /// here.
    pub disabled: bool,
    /// Data for the content to render when this area opens the menu, like
    /// the row it belongs to.
    pub payload: Option<P>,
}

impl<P> Default for ContextTriggerProps<P> {
    fn default() -> Self {
        Self {
            disabled: false,
            payload: None,
        }
    }
}

impl<P> ContextTriggerProps<P> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn payload(mut self, payload: P) -> Self {
        self.payload = Some(payload);
        self
    }
}

/// The props the `checkbox-item` slot takes (`CheckboxProps`). The item's
/// value, which identifies it in the menu's `on_checked_change`, is its key.
#[derive(Clone, Default)]
pub struct CheckboxProps {
    /// The text typeahead matches. It isn't the accessible name: use
    /// `aria_label` or `text!(..)` for that.
    pub label: Option<SharedString>,
    /// Keeps the item focusable but stops it from toggling, as the menu
    /// pattern asks.
    pub disabled: bool,
    /// Whether the item is checked. Pass it to control the item.
    pub checked: Option<bool>,
    /// Whether the item starts checked, when uncontrolled. Defaults to false.
    pub default_checked: bool,
    /// Runs when the item wants to toggle, before the menu's
    /// `on_checked_change`. `details.reason` is `ItemPress` or `Imperative`,
    /// and `details.cancel()` stops it. It runs inside the menu's update, so
    /// it may update any entity but the menu.
    pub on_checked_change: Option<ChangeCallback<bool>>,
    /// Whether toggling closes the menu. Defaults to false.
    pub close_on_select: bool,
}

impl CheckboxProps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    pub fn default_checked(mut self, checked: bool) -> Self {
        self.default_checked = checked;
        self
    }

    pub fn on_checked_change(
        mut self,
        callback: impl Fn(&bool, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_checked_change = Some(Rc::new(callback));
        self
    }

    pub fn close_on_select(mut self, close: bool) -> Self {
        self.close_on_select = close;
        self
    }
}

/// The props the `radio-group` slot takes (`RadioGroupProps`). The group's
/// `name`, which identifies it in the menu's `on_value_change`, is its key.
#[derive(Clone, Default)]
pub struct RadioGroupProps {
    /// The selected value. Pass it to control the group: `Some(None)` selects
    /// nothing.
    pub value: Option<Option<SharedString>>,
    /// The value selected at first, when uncontrolled.
    pub default_value: Option<SharedString>,
    /// Runs when the selection wants to change, before the menu's
    /// `on_value_change`. `details.reason` is `ItemPress` or `Imperative`. It
    /// runs once the menu's update is over, so it may update any entity, the
    /// menu too.
    pub on_value_change: Option<ChangeCallback<SharedString>>,
    /// Runs as the selection is asked to change, before it does and before
    /// the menu's `on_before_value_change`, inside the menu's update:
    /// `details.cancel()` stops it. It may update any entity but the menu.
    pub on_before_value_change: Option<ChangeCallback<SharedString>>,
}

impl RadioGroupProps {
    pub fn new() -> Self {
        Self::default()
    }

    /// Controls the group with `value` selected.
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(Some(value.into()));
        self
    }

    /// Controls the group with nothing selected.
    pub fn value_none(mut self) -> Self {
        self.value = Some(None);
        self
    }

    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&SharedString, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&SharedString, &ui_sdk_kernel::ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }
}

/// The props the `radio-item` slot takes (`RadioProps`). Its value, selected
/// in its group when it's chosen, is its key, beside its group's name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RadioProps {
    /// The text typeahead matches. It isn't the accessible name: use
    /// `aria_label` or `text!(..)` for that.
    pub label: Option<SharedString>,
    /// Keeps the item focusable but stops it from being chosen, as the menu
    /// pattern asks.
    pub disabled: bool,
    /// Whether choosing the item closes the menu. Defaults to false.
    pub close_on_select: bool,
}

impl RadioProps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn close_on_select(mut self, close: bool) -> Self {
        self.close_on_select = close;
        self
    }
}

/// The props the `item-indicator` slot takes (`IndicatorProps`). The item
/// it's in is its own argument.
#[derive(Clone, Default)]
pub struct IndicatorProps {
    /// Whether the indicator stays in the page while its item is unchecked,
    /// so it can be styled or animated in both states. Defaults to false.
    pub keep_mounted: bool,
    /// How it animates in as its item becomes checked, the transition its
    /// CSS would declare.
    pub timing: Option<PresenceTiming>,
}

impl IndicatorProps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = Some(timing);
        self
    }
}

/// The props the `content` slot takes (`ContentProps`): where the menu goes
/// beside its trigger, where focus goes as it closes, and whether it stays in
/// the page while closed.
#[derive(Clone, Default)]
pub struct ContentProps {
    /// Which side of the trigger the menu opens on. Defaults to the bottom;
    /// submenus and a vertical menubar's menus open to the side, mirrored in
    /// right-to-left layouts.
    pub side: Option<Side>,
    /// How the menu lines up with the trigger along that side. Defaults to
    /// `Start`.
    pub align: Option<Align>,
    /// Gap between the trigger and the menu. Defaults to 4, and 0 for a
    /// submenu.
    pub side_offset: Option<Pixels>,
    /// Shift along the trigger. Submenus default to lining up their first
    /// item with the trigger.
    pub align_offset: Option<Pixels>,
    /// Space to keep from the window's edges. Defaults to 8.
    pub collision_padding: Option<Pixels>,
    /// Space to keep between an arrow and the menu's corners. Defaults to 6.
    pub arrow_padding: Option<Pixels>,
    /// Positions the menu against this instead of its trigger.
    pub anchor: Option<Anchor>,
    /// Where focus goes when the menu closes while focus is inside it.
    /// `Trigger`, the default, returns it to the trigger and `None` leaves
    /// it; a focus handle or a function picks a specific element.
    pub final_focus: Option<PopupFocusTarget>,
    /// Whether the menu stays in the page, hidden, while closed. Its content
    /// keeps its state between opens. Defaults to false.
    pub keep_mounted: bool,
    /// How it animates in and out, the transitions its CSS would declare.
    pub timing: Option<PresenceTiming>,
    /// The dim behind it while it presents as a sheet, at its darkest
    /// (`--sheet-backdrop`). Defaults to black at 0.2.
    pub sheet_backdrop: Option<Hsla>,
}

impl ContentProps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn side(mut self, side: Side) -> Self {
        self.side = Some(side);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = Some(align);
        self
    }

    pub fn side_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.side_offset = Some(offset.into());
        self
    }

    pub fn align_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.align_offset = Some(offset.into());
        self
    }

    pub fn collision_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.collision_padding = Some(padding.into());
        self
    }

    pub fn arrow_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.arrow_padding = Some(padding.into());
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.anchor = Some(anchor.into());
        self
    }

    pub fn final_focus(mut self, focus: impl Into<PopupFocusTarget>) -> Self {
        self.final_focus = Some(focus.into());
        self
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = Some(timing);
        self
    }

    pub fn sheet_backdrop(mut self, color: impl Into<Hsla>) -> Self {
        self.sheet_backdrop = Some(color.into());
        self
    }

    pub(crate) fn position(&self) -> PositionProps {
        PositionProps {
            side: self.side,
            align: self.align,
            side_offset: self.side_offset,
            align_offset: self.align_offset,
            collision_padding: self.collision_padding,
            arrow_padding: self.arrow_padding,
            anchor: self.anchor.clone(),
        }
    }
}

// What an entry is and runs, as it rendered: a plain item, a checkbox or
// radio item, or a submenu's trigger (`Entry`'s `kind`, `activate` and
// `openSubmenu`).
#[derive(Clone)]
pub(crate) enum EntryItem<P: Clone + PartialEq + Debug + 'static> {
    Item(ItemProps),
    Checkbox(SharedString, CheckboxProps),
    Radio {
        group: SharedString,
        value: SharedString,
        props: RadioProps,
    },
    Submenu(Weak<MenuShared<P>>),
}

// An item in the page, by key, with what it rendered with (`Entry`): what
// typeahead matches and whether it can run.
#[derive(Clone)]
pub(crate) struct Entry<P: Clone + PartialEq + Debug + 'static> {
    pub(crate) label: SharedString,
    pub(crate) disabled: bool,
    pub(crate) item: EntryItem<P>,
}

impl<P: Clone + PartialEq + Debug + 'static> Entry<P> {
    // The submenu it opens, when it's a submenu's trigger.
    pub(crate) fn submenu(&self) -> Option<Rc<MenuShared<P>>> {
        match &self.item {
            EntryItem::Submenu(child) => child.upgrade(),
            _ => None,
        }
    }

    pub(crate) fn opens_submenu(&self) -> bool {
        matches!(self.item, EntryItem::Submenu(_))
    }
}

// Every item of a data menu with a shortcut, by value, and in groups and
// enabled submenus (`eachShortcut`).
pub(crate) fn each_shortcut(items: &MenuData, run: &mut impl FnMut(&SharedString, &SharedString)) {
    for item in items {
        match item {
            MenuDataItem::Item(item) => {
                if let Some(shortcut) = item.extra.shortcut.as_ref().filter(|shortcut| !shortcut.is_empty())
                    && !item.disabled
                {
                    run(&item.value, shortcut);
                }
            }
            MenuDataItem::Group { items, .. } => each_shortcut(items, run),
            // A disabled submenu's items can't run, by shortcut either.
            MenuDataItem::Submenu { items, disabled, .. } if !disabled => each_shortcut(items, run),
            _ => {}
        }
    }
}

// Work an item's `on_select` or `on_action` started, which marks it pending.
pub(crate) type Work = Pin<Box<dyn Future<Output = Result<(), ActionError>>>>;

// A request to open (`Show`): where focus goes, and the popup's own request.
pub(crate) struct Show<P> {
    pub(crate) focus: Option<Focus>,
    pub(crate) event: Option<InputEvent>,
    pub(crate) by: Option<PopupOpener>,
    pub(crate) reason: Option<Reason>,
    pub(crate) source: Option<ChangeSource>,
    pub(crate) trigger: Option<SharedString>,
    pub(crate) payload: Option<Option<P>>,
    pub(crate) anchor: Option<Anchor>,
    pub(crate) signal: Option<AbortSignal>,
}

impl<P> Default for Show<P> {
    fn default() -> Self {
        Self {
            focus: None,
            event: None,
            by: None,
            reason: None,
            source: None,
            trigger: None,
            payload: None,
            anchor: None,
            signal: None,
        }
    }
}

impl<P> Show<P> {
    pub(crate) fn focus(focus: Focus, event: Option<InputEvent>) -> Self {
        Self {
            focus: Some(focus),
            event,
            ..Self::default()
        }
    }
}

// A request to close (`Close`). `focus` is `Return`, `Leave` or
// `UnlessMoved`, the original's `true`, `false` and `"unless-moved"`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Close {
    pub(crate) reason: Option<Reason>,
    pub(crate) event: Option<InputEvent>,
    pub(crate) focus: Option<ReturnMode>,
    pub(crate) return_value: Option<SharedString>,
}

impl Close {
    pub(crate) fn new(reason: Reason, event: Option<InputEvent>) -> Self {
        Self {
            reason: Some(reason),
            event,
            ..Self::default()
        }
    }

    pub(crate) fn focus(mut self, focus: ReturnMode) -> Self {
        self.focus = Some(focus);
        self
    }
}

// What a menu's key press asks of the menubar once the menu's update is
// over, since moving to another menu closes this one.
pub(crate) enum KeyFollowUp<P: Clone + PartialEq + Debug + 'static> {
    None,
    // Left or Right on an item without a submenu, for the root's menubar.
    BarArrow(Rc<MenuShared<P>>, isize),
}

// The root's callbacks and settings, which every menu of the tree reads.
pub(crate) struct Shared<P: Clone + PartialEq + Debug + 'static> {
    pub(crate) on_action: Option<crate::menu::MenuActionCallback>,
    pub(crate) on_checked_change: Option<crate::menu::MenuCheckedChangeCallback>,
    pub(crate) on_value_change: Option<crate::menu::MenuValueChangeCallback>,
    pub(crate) on_before_value_change: Option<crate::menu::MenuValueChangeCallback>,
    pub(crate) pending_announcement: Option<SharedString>,
    pub(crate) modal: Option<MenuModal>,
    pub(crate) presentation: PresentationSetting,
    pub(crate) _payload: std::marker::PhantomData<P>,
}

// A context area: where it is, and what it rendered with.
pub(crate) struct ContextEntry<P> {
    pub(crate) region: Region,
    pub(crate) props: ContextTriggerProps<P>,
}

/// What the menus of one tree see of each other (`MenuNode`): what any of
/// them reads or changes of another while that one may be in the middle of
/// its own update. Parts' handlers, which run outside every update, reach it
/// too.
pub(crate) struct MenuShared<P: Clone + PartialEq + Debug + 'static> {
    pub(crate) id: SharedString,
    pub(crate) entity: WeakEntity<Menu<P>>,
    pub(crate) instance: InstanceHandle,
    pub(crate) popup: ui_sdk_popups::Popup<Menu<P>>,
    pub(crate) parent: Option<Weak<MenuShared<P>>>,
    pub(crate) menubar: Option<MenubarLink>,
    pub(crate) children: RefCell<Vec<Weak<MenuShared<P>>>>,
    // Items by key, in page order, with what each rendered with.
    pub(crate) entries: Parts<SharedString, Entry<P>>,
    // Where each item is and the focus handle its element tracks, for
    // focus, presses and scrolling. A submenu's trigger brings its own.
    pub(crate) regions: RefCell<HashMap<SharedString, Region>>,
    pub(crate) highlighted: RefCell<Option<SharedString>>,
    // Id of the open submenu. The tree's open path, one level per menu.
    pub(crate) open_child: RefCell<Option<SharedString>>,
    pub(crate) pending_focus: Cell<Focus>,
    // A submenu's trigger, by its key among its parent's items.
    pub(crate) trigger_entry: SharedString,
    pub(crate) context_triggers: RefCell<HashMap<SharedString, ContextEntry<P>>>,
    pub(crate) settings: RefCell<Shared<P>>,
    pub(crate) safe_area: SafeArea,
    // The pointer resting on a submenu's trigger, before its submenu opens.
    pub(crate) hover_intent: ui_sdk_popups::HoverIntent<Menu<P>>,
    pub(crate) scroll: ScrollHandle,
    // Whether the key being dispatched is this menu's, decided before any
    // handler moves focus, as the original decides by the key's target.
    pub(crate) owns_key: Cell<bool>,
    pub(crate) content: RefCell<ContentProps>,
    // How far the content's first item sits from its top edge, from its
    // padding and border, as it last rendered.
    pub(crate) inset: Cell<Pixels>,
    pub(crate) rem_size: Cell<Pixels>,
    pub(crate) rtl: Cell<bool>,
}

// Asks a menu to close, unless it's closed already (`closes`). Focus stays.
pub(crate) fn closes<P: Clone + PartialEq + Debug + 'static>(
    menu: &Rc<MenuShared<P>>,
    reason: Reason,
    event: Option<InputEvent>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if !menu.is_open() {
        return true;
    }
    menu.entity
        .update(cx, |menu, cx| {
            menu.hide(Close::new(reason, event).focus(ReturnMode::Leave), window, cx)
        })
        .unwrap_or(true)
}

// Opens a submenu from its trigger in the parent (`openSubmenu`).
pub(crate) fn open_submenu<P: Clone + PartialEq + Debug + 'static>(
    child: &Rc<MenuShared<P>>,
    focus: Focus,
    reason: Reason,
    event: Option<InputEvent>,
    window: &mut Window,
    cx: &mut App,
) {
    child
        .entity
        .update(cx, |menu, cx| {
            menu.show(
                Show {
                    focus: Some(focus),
                    reason: Some(reason),
                    event,
                    ..Show::default()
                },
                window,
                cx,
            );
        })
        .ok();
}

impl<P: Clone + PartialEq + Debug + 'static> MenuShared<P> {
    pub(crate) fn parent(&self) -> Option<Rc<MenuShared<P>>> {
        self.parent.as_ref().and_then(Weak::upgrade)
    }

    pub(crate) fn is_nested(&self) -> bool {
        self.parent.is_some()
    }

    // The root of the tree, which closes it and holds its callbacks.
    pub(crate) fn root(self: &Rc<Self>) -> Rc<MenuShared<P>> {
        match self.parent() {
            Some(parent) => parent.root(),
            None => self.clone(),
        }
    }

    pub(crate) fn children(&self) -> Vec<Rc<MenuShared<P>>> {
        let mut children = self.children.borrow_mut();
        children.retain(|child| child.strong_count() > 0);
        children.iter().filter_map(Weak::upgrade).collect()
    }

    pub(crate) fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    pub(crate) fn presentation(&self) -> Presentation {
        self.popup.presentation_value()
    }

    pub(crate) fn open_child(&self) -> Option<Rc<MenuShared<P>>> {
        self.children().into_iter().find(|child| child.is_open())
    }

    // Tells the menu's parts something they render changed.
    pub(crate) fn notify(&self, cx: &mut App) {
        if let Some(entity) = self.entity.upgrade() {
            cx.notify(entity.entity_id());
        }
    }

    fn bound_keys(&self, slot: &str) -> Vec<SharedString> {
        self.instance
            .bound_parts()
            .into_iter()
            .filter(|part| part.slot == slot)
            .filter_map(|part| part.key)
            .collect()
    }

    // The trigger or context area that opened the menu last, by key, or the
    // first to render. A trigger that left while the menu was closed hands it
    // back to the first one, as the trigger's cleanup does.
    pub(crate) fn trigger_key(&self) -> Option<SharedString> {
        let key = self.popup.trigger_key()?;
        let gone = self.popup.trigger_region(&key).is_some()
            && !self.popup.bound_triggers().contains(&key)
            && !self.context_bound(&key);
        if gone && !self.is_open() {
            return None;
        }
        Some(key)
    }

    fn context_bound(&self, key: &SharedString) -> bool {
        self.bound_keys("context-trigger").contains(key)
    }

    // The trigger that opened the menu last, a button or a context area, or
    // the first trigger, and whether it's a context area.
    pub(crate) fn active_trigger(&self) -> (Option<Region>, bool) {
        let buttons = self.popup.bound_triggers();
        let context = |key: &SharedString| {
            self.context_bound(key)
                .then(|| {
                    self.context_triggers
                        .borrow()
                        .get(key)
                        .map(|entry| entry.region.clone())
                })
                .flatten()
        };
        match self.trigger_key() {
            None => {
                let first = buttons
                    .first()
                    .and_then(|key| self.popup.trigger_region(key))
                    .or_else(|| self.bound_keys("context-trigger").first().and_then(context));
                (first, false)
            }
            Some(key) => {
                if buttons.contains(&key)
                    && let Some(region) = self.popup.trigger_region(&key)
                {
                    return (Some(region), false);
                }
                (context(&key), true)
            }
        }
    }

    pub(crate) fn trigger_element(&self) -> Option<Region> {
        self.active_trigger().0
    }

    pub(crate) fn focus_trigger(&self, window: &mut Window, cx: &mut App) {
        if let Some(handle) = self.trigger_element().and_then(|trigger| trigger.focus_handle()) {
            window.focus(&handle, cx);
        }
    }

    // Whether the trigger with `key` can't open the menu: its own `disabled`,
    // or the menubar's (`triggerDisabled`).
    pub(crate) fn trigger_disabled(&self, key: &SharedString) -> bool {
        self.popup.trigger_disabled(key) || self.bar_disabled()
    }

    pub(crate) fn bar_disabled(&self) -> bool {
        self.menubar.as_ref().is_some_and(|bar| bar.shared.disabled.get())
    }

    // Whether a secondary press lands on one of the menu's context areas,
    // which takes the menu over where it's pressed.
    pub(crate) fn presses_context(&self, target: &Target, window: &Window) -> bool {
        let bound = self.bound_keys("context-trigger");
        self.context_triggers
            .borrow()
            .iter()
            .any(|(key, entry)| bound.contains(key) && !entry.props.disabled && entry.region.contains(target, window))
    }

    // Whether a press lands on one of the menu's enabled triggers.
    pub(crate) fn presses_trigger(&self, target: &Target, window: &Window) -> bool {
        self.popup.bound_triggers().iter().any(|key| {
            !self.trigger_disabled(key)
                && self
                    .popup
                    .trigger_region(key)
                    .is_some_and(|region| region.contains(target, window))
        })
    }

    pub(crate) fn tree_contains(&self, target: &Target, window: &Window) -> bool {
        // A submenu that just closed, before its parent, still holds focus
        // until the window draws again.
        let own = self
            .popup
            .layer()
            .element()
            .unwrap_or_else(|| self.popup.content_region());
        if own.contains(target, window) {
            return true;
        }
        self.children().iter().any(|child| child.tree_contains(target, window))
    }

    // An item in the page, by key.
    pub(crate) fn entry_for(&self, key: &SharedString) -> Option<Entry<P>> {
        self.entries.info(key)
    }

    // The focus handle an item's element tracks.
    pub(crate) fn focus_handle(&self, key: &SharedString) -> Option<FocusHandle> {
        self.regions.borrow().get(key).and_then(Region::focus_handle)
    }

    // The item that has focus, if one does (`entryOf(activeElement)`).
    pub(crate) fn focused_entry(&self, window: &Window) -> Option<SharedString> {
        self.entries
            .keys()
            .into_iter()
            .find(|key| self.focus_handle(key).is_some_and(|handle| handle.is_focused(window)))
    }

    // The item at a point in this menu or any open submenu, with the menu it
    // belongs to (`findEntry`).
    pub(crate) fn find_entry(
        self: &Rc<Self>,
        target: &Target,
        window: &Window,
    ) -> Option<(Rc<MenuShared<P>>, SharedString)> {
        let candidates: Vec<(SharedString, Region)> = {
            let regions = self.regions.borrow();
            self.entries
                .keys()
                .into_iter()
                .filter_map(|key| regions.get(&key).map(|region| (key, region.clone())))
                .collect()
        };
        if let Some(key) = composed_closest(target, candidates, window) {
            return Some((self.clone(), key));
        }
        self.children()
            .iter()
            .find_map(|child| child.find_entry(target, window))
    }

    pub(crate) fn highlight(
        self: &Rc<Self>,
        key: Option<SharedString>,
        focus: bool,
        scroll: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let found = key.as_ref().filter(|key| self.entries.contains(key)).cloned();
        if let Some(child) = self.open_child() {
            let trigger = self
                .entries
                .contains(&child.trigger_entry)
                .then(|| child.trigger_entry.clone());
            if found != trigger && key.is_some() && !closes(&child, Reason::SubmenuSwitch, None, window, cx) {
                return;
            }
        }
        let changed = *self.highlighted.borrow() != key;
        *self.highlighted.borrow_mut() = key;
        if changed {
            self.notify(cx);
        }
        if !focus {
            return;
        }
        match found {
            Some(key) => {
                if let Some(handle) = self.focus_handle(&key) {
                    window.focus(&handle, cx);
                }
                if scroll {
                    self.scroll_into_view(&key, cx);
                }
            }
            None => {
                if let Some(handle) = self.popup.layer().element().and_then(|el| el.focus_handle()) {
                    window.focus(&handle, cx);
                }
            }
        }
    }

    // Scrolls the content just enough to show an item (`scrollIntoView`
    // with `block: "nearest"`), when the content scrolls.
    fn scroll_into_view(&self, key: &SharedString, cx: &mut App) {
        let Some(item) = self.regions.borrow().get(key).and_then(Region::bounds) else {
            return;
        };
        if self.scroll.max_offset().y <= px(0.) {
            return;
        }
        let viewport = self.scroll.bounds();
        let offset = self.scroll.offset();
        let scrolled = -offset.y;
        let start = item.top() - viewport.top() + scrolled;
        let next =
            ui_sdk_focus::scroll_to_show(scrolled, viewport.size.height, start, item.size.height, px(0.), px(0.));
        if next != scrolled {
            self.scroll.set_offset(point(offset.x, -next));
            self.notify(cx);
        }
    }

    // For submenus: highlights and focuses this menu's trigger in the parent.
    pub(crate) fn focus_trigger_item(&self, window: &mut Window, cx: &mut App) {
        let Some(parent) = self.parent() else {
            return;
        };
        if parent.entries.contains(&self.trigger_entry) {
            parent.highlight(Some(self.trigger_entry.clone()), true, false, window, cx);
        }
    }

    // Moves focus to the item asked for, or returns the popup for the layer
    // to focus.
    pub(crate) fn run_pending_focus(self: &Rc<Self>, window: &mut Window, cx: &mut App) -> Option<FocusHandle> {
        let focus = self.pending_focus.replace(Focus::None);
        let list = self.entries.keys();
        match focus {
            Focus::First if !list.is_empty() => {
                self.highlight(list.first().cloned(), true, true, window, cx);
                None
            }
            Focus::Last if !list.is_empty() => {
                self.highlight(list.last().cloned(), true, true, window, cx);
                None
            }
            Focus::Popup => self.popup.layer().element().and_then(|el| el.focus_handle()),
            _ => None,
        }
    }

    pub(crate) fn child_open_changed(&self, child: &SharedString, open: bool, cx: &mut App) {
        let changed = {
            let mut current = self.open_child.borrow_mut();
            if open {
                let changed = current.as_ref() != Some(child);
                *current = Some(child.clone());
                changed
            } else if current.as_ref() == Some(child) {
                *current = None;
                true
            } else {
                false
            }
        };
        if changed {
            self.notify(cx);
        }
    }

    // The root's `modal`: the app's, else the menubar's, and modal while it
    // presents as a sheet, else `Pointer` (`modalSetting`).
    pub(crate) fn modal_setting(self: &Rc<Self>) -> MenuModal {
        let root = self.root();
        let own = root.settings.borrow().modal;
        let setting = own.or_else(|| root.menubar.as_ref().map(|bar| bar.shared.modal.get()));
        sheet_modal(root.presentation(), setting, MenuModal::Pointer, MenuModal::True)
    }

    // The text typeahead matches for each item, in page order.
    pub(crate) fn labels(&self) -> Vec<SharedString> {
        self.entries
            .entries()
            .into_iter()
            .map(|(_, entry)| entry.label)
            .collect()
    }

    // While a submenu shows in this menu's place, in a sheet: that
    // submenu's content and the trigger kept above it. Every other part of
    // the menu hides then.
    pub(crate) fn drilled(&self) -> Option<(Region, Option<Region>)> {
        drilled_into(&self.popup.content_region())
    }

    // Where the content goes: a controller's or a takeover's anchor, the
    // content's own, else the trigger (`anchor`).
    pub(crate) fn anchor(&self) -> Option<Anchor> {
        self.popup
            .control()
            .anchor()
            .or_else(|| self.content.borrow().anchor.clone())
            .or_else(|| self.trigger_element().map(Anchor::element))
    }

    // How the content is placed, read on every frame (`position`).
    pub(crate) fn position(&self) -> PositionOptions {
        let latest = self.content.borrow().clone();
        let nested = self.is_nested();
        let vertical_bar = self
            .menubar
            .as_ref()
            .is_some_and(|bar| bar.shared.orientation.get() == gpui::Orientation::Vertical);
        // Submenus and the menus of a vertical menubar open to the side.
        let side = if nested || vertical_bar {
            if self.rtl.get() { Side::Left } else { Side::Right }
        } else {
            Side::Bottom
        };
        let first_item = if nested { self.inset.get() } else { px(0.) };
        let defaults = PlacementDefaults {
            side: Some(side),
            align: Align::Start,
            side_offset: Some(px(if nested { 0. } else { 4. })),
            align_offset: Some(-first_item),
        };
        placement_options(&latest.position(), defaults, None)
    }

    // The root's announcement for a pending item, `None` for the default.
    pub(crate) fn pending_announcement(self: &Rc<Self>) -> Option<SharedString> {
        self.root().settings.borrow().pending_announcement.clone()
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Menu<P> {
    /// Opens the menu, as `show` does in the original: through the popup's
    /// open steps, which the menu's rules decide.
    pub(crate) fn show(&mut self, show: Show<P>, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        let request = ShowRequest {
            reason: show.reason,
            event: show.event,
            source: show.source,
            payload: show.payload,
            anchor: show.anchor,
            signal: show.signal,
            by: show.by,
            trigger: show.trigger,
            extra: show.focus.map(|focus| Rc::new(focus) as Rc<dyn std::any::Any>),
        };
        let control = self.popup.control().clone();
        control.show(self, request, None, window, cx)
    }

    /// Closes the menu, as `hide` does in the original. Returns whether it
    /// closed.
    pub(crate) fn hide(&mut self, close: Close, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let request = HideRequest {
            reason: close.reason,
            event: close.event,
            source: None,
            focus: close.focus,
            return_value: close.return_value,
        };
        let control = self.popup.control().clone();
        control.hide(self, request, window, cx)
    }

    // Asks the root to close the whole tree: at once when this is the root,
    // else once this update is over, since the root closes this menu first.
    pub(crate) fn hide_tree(&mut self, close: Close, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.shared.root();
        if Rc::ptr_eq(&root, &self.shared) {
            self.hide(close, window, cx);
            return;
        }
        // The close reports the source it would have had during this update.
        let source = ui_sdk_kernel::details::current_source();
        window.defer(cx, move |window, cx| {
            root.entity
                .update(cx, |menu, cx| {
                    ui_sdk_kernel::with_optional_source(source, || menu.hide(close, window, cx));
                })
                .ok();
        });
    }

    // A dismissal the popup layer saw, like Escape or an outside press.
    pub(crate) fn dismiss(
        &mut self,
        reason: Reason,
        details: CloseDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Tab: focus moves on from the root trigger, and the whole tree closes.
        if reason == Reason::FocusOut {
            let root = self.shared.root();
            root.focus_trigger(window, cx);
            self.hide_tree(Close::new(reason, details.event).focus(ReturnMode::Leave), window, cx);
            return;
        }
        // An outside press leaves focus wherever the press put it.
        let focus = if reason != Reason::OutsidePress && details.focus == ReturnMode::Return {
            ReturnMode::Return
        } else {
            ReturnMode::Leave
        };
        self.hide(
            Close {
                reason: Some(reason),
                event: details.event,
                focus: Some(focus),
                return_value: details.return_value,
            },
            window,
            cx,
        );
    }

    // Submenus share their root's modality, and only the root blocks the
    // window.
    pub(crate) fn modal_effects(&self) -> PopupModal {
        let mut modal = modal_triple(self.shared.modal_setting().into());
        if self.shared.is_nested() {
            modal.inert_and_lock = false;
        }
        modal
    }

    // A modal menu in a menubar leaves the menubar usable, so another menu
    // can take over.
    pub(crate) fn persistent(&self) -> Vec<Region> {
        match &self.shared.menubar {
            Some(bar) if self.shared.modal_setting() == MenuModal::True => vec![bar.shared.root.clone()],
            _ => Vec::new(),
        }
    }

    // Submenus present like their root. A context menu and a menubar's menus
    // stay popovers.
    pub(crate) fn presentation_setting(&self) -> Option<PresentationSetting> {
        if let Some(parent) = self.shared.parent() {
            return Some(match parent.presentation() {
                Presentation::Sheet => PresentationSetting::Sheet,
                _ => PresentationSetting::Default,
            });
        }
        if self.shared.menubar.is_some() || self.shared.active_trigger().1 {
            return Some(PresentationSetting::Default);
        }
        Some(self.shared.settings.borrow().presentation)
    }

    pub(crate) fn layer_open_changed(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.shared.id.clone();
        if let Some(parent) = self.shared.parent() {
            parent.child_open_changed(&id, open, cx);
        }
        if let Some(bar) = &self.shared.menubar {
            bar.shared.open_changed(&id, open, cx);
        }
        if open {
            return;
        }
        self.session += 1;
        self.shared.hover_intent.clear();
        self.shared.safe_area.stop();
        for child in self.shared.children() {
            child
                .entity
                .update(cx, |menu, cx| {
                    menu.hide(
                        Close::new(Reason::ParentClose, None).focus(ReturnMode::Leave),
                        window,
                        cx,
                    )
                })
                .ok();
        }
        if self.shared.highlighted.borrow_mut().take().is_some() {
            cx.notify();
        }
    }

    // A submenu closes one level and focuses the item that opened it.
    pub(crate) fn escape(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.hide(
            Close::new(Reason::EscapeKey, Some(InputEvent::KeyDown(event.clone()))).focus(ReturnMode::Leave),
            window,
            cx,
        );
        self.shared.focus_trigger_item(window, cx);
    }

    // A sibling, or another menu of the menubar, that refuses to close stays
    // the open one.
    pub(crate) fn open_rule(
        &mut self,
        request: &SettledShow<P>,
        step: &OpenStep<'_, Self, P>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let event = request.request.event.clone();
        if let Some(parent) = self.shared.parent() {
            for sibling in parent.children() {
                if !Rc::ptr_eq(&sibling, &self.shared) && !closes(&sibling, Reason::SubmenuSwitch, None, window, cx) {
                    return;
                }
            }
        }
        if let Some(bar) = self.shared.menubar.clone()
            && !bar.shared.opening(&self.shared.id, event.as_ref(), window, cx)
        {
            return;
        }
        let focus = request
            .request
            .extra
            .as_ref()
            .and_then(|extra| extra.downcast_ref::<Focus>())
            .copied()
            .unwrap_or_default();
        self.shared.pending_focus.set(focus);
        self.opened_by_hover = request.reason == Reason::TriggerHover;
        let by = request.request.by.unwrap_or_else(|| opener_of(event.as_ref()));
        step.open(self, Some(by), window, cx);
    }

    // Another trigger takes the menu over where it's pressed, so an open
    // again only moves focus.
    pub(crate) fn reopen_rule(&mut self, request: &SettledShow<P>, window: &mut Window, cx: &mut Context<Self>) {
        let focus = request
            .request
            .extra
            .as_ref()
            .and_then(|extra| extra.downcast_ref::<Focus>())
            .copied()
            .unwrap_or_default();
        self.shared.pending_focus.set(focus);
        if let Some(handle) = self.shared.run_pending_focus(window, cx) {
            window.focus(&handle, cx);
        }
    }

    // Submenus close first, so one that refuses keeps its parent open. One
    // closing with its parent moves focus to its trigger item: a submenu
    // rendered above the window isn't inside its parent, which then wouldn't
    // return focus.
    pub(crate) fn close_rule(
        &mut self,
        request: &HideRequest,
        reason: Reason,
        step: &CloseStep<'_, Self, P>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(child) = self.shared.open_child() {
            let source = change_source(reason, request.event.as_ref());
            if !with_source(source, || closes(&child, Reason::ParentClose, None, window, cx)) {
                return;
            }
        }
        let held = reason == Reason::ParentClose
            && self
                .popup
                .layer()
                .element()
                .is_some_and(|el| el.contains_focused(window, cx));
        if step.close(self, window, cx) && held {
            self.shared.focus_trigger_item(window, cx);
        }
    }

    /// Whether a checkbox item is checked: its `checked` prop when it has
    /// one, else what a press set, else its `default_checked`
    /// (`checkboxValue`).
    pub fn checkbox_value(&self, value: &SharedString, props: &CheckboxProps) -> bool {
        props
            .checked
            .or_else(|| self.checked.get(value).copied())
            .unwrap_or(props.default_checked)
    }

    /// The selected value of a radio group, by its name: its `value` prop
    /// when it has one, else what a press set, else its `default_value`
    /// (`radioValue`).
    pub fn radio_value(&self, group: &SharedString) -> Option<SharedString> {
        let props = self.radio_groups.borrow().get(group).cloned();
        if let Some(value) = props.as_ref().and_then(|props| props.value.clone()) {
            return value;
        }
        self.radios
            .get(group)
            .cloned()
            .or_else(|| props.and_then(|props| props.default_value))
    }

    /// Toggles a checkbox item. Its own callback hears first, then the root
    /// menu's, and either can cancel (`setCheckbox`). Returns false when
    /// canceled.
    pub fn set_checkbox(
        &mut self,
        value: &SharedString,
        props: &CheckboxProps,
        event: Option<InputEvent>,
        reason: Reason,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let next = !self.checkbox_value(value, props);
        let source = change_source(reason, event.as_ref());
        let details = ui_sdk_kernel::ChangeDetails::new(reason, source, event.clone());
        if let Some(callback) = props.on_checked_change.clone() {
            with_source(source, || callback(&next, &details, window, cx));
        }
        if details.canceled() {
            return false;
        }
        let shared = ui_sdk_kernel::ChangeDetails::new(reason, source, event);
        let callback = self.shared.root().settings.borrow().on_checked_change.clone();
        if let Some(callback) = callback {
            let details = crate::menu::MenuCheckedChangeDetails::new(&shared, value.clone());
            with_source(source, || callback(&next, &details, window, cx));
        }
        if shared.canceled() {
            return false;
        }
        if props.checked.is_none() {
            self.checked.insert(value.clone(), next);
            cx.notify();
        }
        true
    }

    /// Selects a radio item in its group, the same way (`setRadio`). Returns
    /// false when it was selected already or the change was canceled.
    #[allow(clippy::too_many_arguments)]
    pub fn set_radio(
        &mut self,
        group: &SharedString,
        value: &SharedString,
        event: Option<InputEvent>,
        reason: Reason,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.radio_value(group).as_ref() == Some(value) {
            return false;
        }
        let props = self.radio_groups.borrow().get(group).cloned();
        let source = change_source(reason, event.as_ref());
        let details = ui_sdk_kernel::ChangeDetails::new(reason, source, event.clone());
        if let Some(callback) = props.as_ref().and_then(|props| props.on_before_value_change.clone()) {
            with_source(source, || callback(value, &details, window, cx));
        }
        if details.canceled() {
            return false;
        }
        let shared = ui_sdk_kernel::ChangeDetails::new(reason, source, event);
        let root = self.shared.root();
        let (before, after) = {
            let settings = root.settings.borrow();
            (settings.on_before_value_change.clone(), settings.on_value_change.clone())
        };
        if let Some(callback) = before {
            let details = crate::menu::MenuValueChangeDetails::new(&shared, group.clone());
            with_source(source, || callback(value, &details, window, cx));
        }
        if shared.canceled() {
            return false;
        }
        if props.as_ref().is_none_or(|props| props.value.is_none()) {
            self.radios.insert(group.clone(), value.clone());
            cx.notify();
        }
        // The group's and the menu's `on_value_change` hear it once the
        // update is over.
        let group_callback = props.and_then(|props| props.on_value_change);
        let (value, group) = (value.clone(), group.clone());
        window.defer(cx, move |window, cx| {
            if let Some(callback) = group_callback {
                with_source(source, || callback(&value, &details, window, cx));
            }
            if let Some(callback) = after {
                let details = crate::menu::MenuValueChangeDetails::new(&shared, group);
                with_source(source, || callback(&value, &details, window, cx));
            }
        });
        true
    }

    // Runs a plain item and closes the whole tree. Focus returns to the root
    // trigger, unless the item opened something that took it, like a dialog
    // (`action`).
    pub(crate) fn action(
        &mut self,
        value: Option<SharedString>,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(value) = value {
            self.run_action(&value, event.clone(), window, cx);
        }
        self.hide_tree(
            Close::new(Reason::ItemPress, event).focus(ReturnMode::UnlessMoved),
            window,
            cx,
        );
    }

    // The root's `on_action` for an item's value (`runAction`).
    pub(crate) fn run_action(
        &mut self,
        value: &SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ActionOutcome {
        let callback = self.shared.root().settings.borrow().on_action.clone();
        match callback {
            Some(callback) => callback(value, &crate::menu::MenuActionDetails { event }, window, cx),
            None => ActionOutcome::Done,
        }
    }

    // Marks `key` pending until `work` settles: `done` runs once it all
    // succeeded, unless the menu closed meanwhile, and a failure keeps the
    // reason in the item's `error` state (`wait`).
    pub(crate) fn wait(
        &mut self,
        key: SharedString,
        work: Vec<Work>,
        done: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let started = self.session;
        self.pending.push(key.clone());
        self.pending_changed(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = futures::future::try_join_all(work).await;
            this.update_in(cx, |menu, window, cx| {
                menu.pending.retain(|other| *other != key);
                match result {
                    Ok(_) => {
                        menu.pending_changed(window, cx);
                        if started == menu.session {
                            done(menu, window, cx);
                        }
                    }
                    Err(reason) => {
                        menu.error = Some((key, reason));
                        menu.pending_changed(window, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    // Says a pending item's announcement once it has been pending for a
    // second (`announcePending`), and renders the change.
    pub(crate) fn pending_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = !self.pending.is_empty();
        let text = self.shared.pending_announcement();
        let mut announce = std::mem::take(&mut self.announce);
        announce.update(
            pending,
            text,
            |text, window, cx| {
                if !text.is_empty() {
                    ui_sdk_platform::announce(text, window, cx);
                }
            },
            window,
            cx,
        );
        self.announce = announce;
        cx.notify();
    }

    /// Whether an item's `on_select` or `on_action` work is running
    /// (`isPending`).
    pub fn is_pending(&self, key: &SharedString) -> bool {
        self.pending.contains(key)
    }

    // Clears the error of an item that runs again (`clearError`).
    pub(crate) fn clear_error(&mut self, key: &SharedString, cx: &mut Context<Self>) {
        if self.error.as_ref().is_some_and(|(failed, _)| failed == key) {
            self.error = None;
            cx.notify();
        }
    }

    // A data menu's item shortcut: pending and failing like a press on the
    // item. `on_action` hears no event, since the key press happened outside
    // the menu (`runShortcut`).
    pub(crate) fn run_shortcut(&mut self, value: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let key: SharedString = format!("shortcut:{value}").into();
        if self.pending.contains(&key) {
            return;
        }
        self.clear_error(&key, cx);
        match self.run_action(value, None, window, cx) {
            ActionOutcome::Pending(work) => self.wait(key, vec![work], |_, _, _| {}, window, cx),
            ActionOutcome::Failed(reason) => {
                self.error = Some((key, reason));
                cx.notify();
            }
            ActionOutcome::Done | ActionOutcome::Canceled => {}
        }
    }

    // The pointer moved on an item (`hoverItem`).
    pub(crate) fn hover_item(
        &mut self,
        key: SharedString,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.last_input_was_touch() {
            return;
        }
        // Content scrolling under a resting pointer isn't a move.
        let moved = self.pointer != Some(event.position);
        self.pointer = Some(event.position);
        if !moved && self.shared.highlighted.borrow().is_some() {
            return;
        }
        self.hover(key, event.clone(), window, cx);
    }

    // Moves over separators and gaps count, so coming back to the same item
    // reads as a move (`trackPointer`).
    pub(crate) fn track_pointer(&mut self, event: &MouseMoveEvent, window: &Window) {
        if window.last_input_was_touch() {
            return;
        }
        self.pointer = Some(event.position);
    }

    pub(crate) fn hover(
        &mut self,
        key: SharedString,
        event: MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.shared.entry_for(&key) else {
            return;
        };
        let move_event = InputEvent::MouseMove(event.clone());
        if let Some(child) = self.shared.open_child()
            && child.trigger_entry != key
        {
            // Heading for the open submenu. Leave it open, and once the
            // pointer rests, apply where it rests, though it hasn't moved
            // since.
            let resting = (key.clone(), event.clone());
            let area = self.shared.safe_area.clone();
            if area.contains(
                event.position,
                move |menu: &mut Self, window, cx| menu.hover(resting.0, resting.1, window, cx),
                window,
                cx,
            ) {
                return;
            }
            self.shared.safe_area.stop();
            if !closes(&child, Reason::SubmenuSwitch, Some(move_event.clone()), window, cx) {
                return;
            }
        }
        let focused = self.shared.focused_entry(window).as_ref() == Some(&key);
        if self.shared.highlighted.borrow().as_ref() != Some(&key) || !focused {
            self.shared.highlight(Some(key.clone()), true, false, window, cx);
        }
        self.shared.hover_intent.clear();
        // In a sheet, a submenu replaces the items, so only a press opens it.
        if entry.opens_submenu() && !entry.disabled && self.shared.presentation() != Presentation::Sheet {
            let hover = HoverEvent::from_move(&event, window);
            let intent = self.shared.hover_intent.clone();
            let delay = self.delay;
            intent.enter(
                self,
                &hover,
                Some(Box::new(move |menu: &mut Self, window, cx| {
                    let Some(entry) = menu.shared.entry_for(&key) else {
                        return;
                    };
                    if entry.disabled {
                        return;
                    }
                    if let Some(child) = entry.submenu() {
                        open_submenu(&child, Focus::None, Reason::TriggerHover, Some(move_event), window, cx);
                    }
                })),
                delay,
                window,
                cx,
            );
        }
    }

    // Keys on a trigger in a menubar. The menubar moves between triggers, and
    // says when a key opens this menu, and at which end (`barKeydown`). It
    // runs outside every update, since moving to another menu closes this
    // one.
    pub(crate) fn bar_keydown(
        entity: &gpui::Entity<Self>,
        key: &SharedString,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (shared, disabled, payload) = {
            let menu = entity.read(cx);
            (
                menu.shared.clone(),
                menu.shared.trigger_disabled(key),
                menu.popup.trigger_payload(key),
            )
        };
        let Some(bar) = shared.menubar.clone() else {
            return;
        };
        let focus = bar.shared.trigger_keydown(event, &shared.id, disabled, window, cx);
        if let Some(focus) = focus {
            entity.update(cx, |menu, cx| {
                menu.show(
                    Show {
                        focus: Some(focus),
                        event: Some(InputEvent::KeyDown(event.clone())),
                        trigger: Some(key.clone()),
                        payload: Some(payload),
                        ..Show::default()
                    },
                    window,
                    cx,
                );
            });
        }
    }

    // Opens on press. Releasing over an item after dragging selects it, like
    // native menus. A quick click never selects, even if the menu covers the
    // trigger (`pointerPress`).
    pub(crate) fn pointer_press(
        &mut self,
        trigger: Region,
        event: &gpui::MouseDownEvent,
        key: SharedString,
        payload: Option<P>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.press_click = true;
        let open = self.popup.is_open();
        let active = self.shared.trigger_key();
        // Another trigger takes the open menu over. A drag from it selects,
        // as from a press that opens.
        let opening = !open || active.as_ref().is_some_and(|active| *active != key);
        let down = InputEvent::MouseDown(event.clone());
        if !open {
            self.show(
                Show {
                    focus: Some(Focus::Popup),
                    event: Some(down),
                    trigger: Some(key),
                    payload: Some(payload),
                    ..Show::default()
                },
                window,
                cx,
            );
        } else if opening {
            let control = self.popup.control().clone();
            control.take_over(self, key, payload, None, Reason::TriggerPress, Some(down), window, cx);
        } else if !self.opened_by_hover {
            self.hide(Close::new(Reason::TriggerPress, Some(down)), window, cx);
        }
        self.opened_by_hover = false;
        self.press = Some(PressSession {
            trigger,
            opening,
            start: event.position,
            at: Instant::now(),
        });
        cx.notify();
    }

    // The press that opened the menu ended (`pointerPress`'s `pointerup`).
    pub(crate) fn press_release(&mut self, up: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else {
            return;
        };
        let target = Target::Point(up.position);
        let on_trigger = press.trigger.contains(&target, window);
        // A release off the trigger sends it no click, so the next click,
        // like Enter, is a new press.
        if !on_trigger {
            self.press_click = false;
        }
        if !press.opening {
            return;
        }
        let held = press.at.elapsed() >= Duration::from_millis(200);
        let dx = f32::from(up.position.x - press.start.x);
        let dy = f32::from(up.position.y - press.start.y);
        let moved = dx.hypot(dy) > 4.0;
        if !held || (!moved && on_trigger) {
            return;
        }
        let event = InputEvent::MouseUp(up.clone());
        match self.shared.find_entry(&target, window) {
            Some((owner, key)) => {
                let Some(entry) = owner.entry_for(&key) else {
                    return;
                };
                if entry.disabled || entry.opens_submenu() {
                    return;
                }
                if Rc::ptr_eq(&owner, &self.shared) {
                    self.activate(&key, Some(event), window, cx);
                } else {
                    owner
                        .entity
                        .update(cx, |menu, cx| menu.activate(&key, Some(event), window, cx))
                        .ok();
                }
            }
            None => {
                if !on_trigger && !self.shared.root().tree_contains(&target, window) {
                    self.hide(
                        Close::new(Reason::TriggerRelease, Some(event)).focus(ReturnMode::Leave),
                        window,
                        cx,
                    );
                }
            }
        }
    }

    // A press opened the menu: whether a click follows, which it ignores
    // (`consumePressClick`).
    pub(crate) fn consume_press_click(&mut self) -> bool {
        std::mem::replace(&mut self.press_click, false)
    }

    // Opens at a point, or moves the open menu there: a right-click, a long
    // press, or Shift+F10 on something inside a context area (`contextOpen`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn context_open(
        &mut self,
        at: Anchor,
        event: Option<InputEvent>,
        by: PopupOpener,
        key: SharedString,
        payload: Option<P>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.popup.is_open() {
            let control = self.popup.control().clone();
            control.take_over(
                self,
                key,
                payload,
                Some(at),
                Reason::TriggerPress,
                event.clone(),
                window,
                cx,
            );
            if by == PopupOpener::Keyboard {
                self.show(Show::focus(Focus::First, event), window, cx);
            }
            return;
        }
        self.show(
            Show {
                focus: Some(if by == PopupOpener::Keyboard {
                    Focus::First
                } else {
                    Focus::Popup
                }),
                event,
                by: Some(by),
                anchor: Some(at),
                trigger: Some(key),
                payload: Some(payload),
                ..Show::default()
            },
            window,
            cx,
        );
    }

    /// Keys on the content (`onKeyDown`). The menubar's part of Left and
    /// Right comes back to run once the menu's update is over.
    pub(crate) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> KeyFollowUp<P> {
        let list = self.shared.entries.keys();
        let current_key = self
            .shared
            .focused_entry(window)
            .or_else(|| self.shared.highlighted.borrow().clone());
        let index = current_key
            .as_ref()
            .and_then(|current| list.iter().position(|key| key == current));
        let current = current_key.as_ref().and_then(|key| self.shared.entry_for(key));
        let r#loop = self.r#loop;
        let count = list.len() as isize;
        let shared = self.shared.clone();
        let r#move = |next: isize, window: &mut Window, cx: &mut Context<Self>| {
            window.prevent_default();
            if count == 0 {
                return;
            }
            let clamped = if r#loop {
                next.rem_euclid(count)
            } else {
                next.clamp(0, count - 1)
            };
            shared.highlight(Some(list[clamped as usize].clone()), true, true, window, cx);
        };

        // In right-to-left layouts, submenus open to the left, so the arrows
        // swap.
        let rtl = ui_sdk_platform::is_rtl(cx);
        let key = match Key::of(&event.keystroke) {
            Key::ArrowRight if rtl => Key::ArrowLeft,
            Key::ArrowLeft if rtl => Key::ArrowRight,
            key => key,
        };
        let key_event = InputEvent::KeyDown(event.clone());

        match key {
            Key::ArrowDown => {
                r#move(index.map_or(0, |index| index as isize + 1), window, cx);
                return KeyFollowUp::None;
            }
            Key::ArrowUp => {
                r#move(index.map_or(count - 1, |index| index as isize - 1), window, cx);
                return KeyFollowUp::None;
            }
            Key::Home | Key::PageUp => {
                r#move(0, window, cx);
                return KeyFollowUp::None;
            }
            Key::End | Key::PageDown => {
                r#move(count - 1, window, cx);
                return KeyFollowUp::None;
            }
            Key::ArrowRight => {
                if let Some(current) = &current
                    && current.opens_submenu()
                    && !current.disabled
                {
                    window.prevent_default();
                    if let Some(child) = current.submenu() {
                        open_submenu(&child, Focus::First, Reason::TriggerPress, Some(key_event), window, cx);
                    }
                    return KeyFollowUp::None;
                }
                return KeyFollowUp::BarArrow(self.shared.root(), 1);
            }
            Key::ArrowLeft => {
                // A submenu's close key, like Escape.
                if self.shared.is_nested() {
                    window.prevent_default();
                    self.escape(event, window, cx);
                    return KeyFollowUp::None;
                }
                return KeyFollowUp::BarArrow(self.shared.clone(), -1);
            }
            Key::Escape => {
                window.prevent_default();
                if self.shared.is_nested() {
                    self.escape(event, window, cx);
                } else {
                    self.hide(Close::new(Reason::EscapeKey, Some(key_event)), window, cx);
                }
                return KeyFollowUp::None;
            }
            Key::Tab => {
                // Focus moves on from the trigger, then everything closes.
                let layer: PopupLayer<Self> = self.popup.layer().clone();
                layer.keydown(self, event, window, cx);
                return KeyFollowUp::None;
            }
            Key::Enter | Key::Space => {
                self.activate_key(event, &list, index, current_key, current, window, cx);
                return KeyFollowUp::None;
            }
            _ => {}
        }

        if is_typeahead_key(&event.keystroke) {
            let labels = self.shared.labels();
            if let Some(found) = self.typeahead.search(&Key::of(&event.keystroke), &labels, index) {
                window.prevent_default();
                self.shared.highlight(Some(list[found].clone()), true, true, window, cx);
            }
        }
        KeyFollowUp::None
    }

    // Enter and Space on the content: they continue a search, open a
    // submenu or run the current item.
    #[allow(clippy::too_many_arguments)]
    fn activate_key(
        &mut self,
        event: &KeyDownEvent,
        list: &[SharedString],
        index: Option<usize>,
        current_key: Option<SharedString>,
        current: Option<Entry<P>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key_event = InputEvent::KeyDown(event.clone());
        // Space continues a search only when it still matches something.
        if Key::of(&event.keystroke) == Key::Space && self.typeahead.active() {
            let labels = self.shared.labels();
            if let Some(found) = self.typeahead.search(&Key::Space, &labels, index) {
                window.prevent_default();
                self.shared.highlight(Some(list[found].clone()), true, true, window, cx);
                return;
            }
            self.typeahead.reset();
        }
        let (Some(key), Some(current)) = (current_key, current) else {
            window.prevent_default();
            return;
        };
        if current.disabled || current.opens_submenu() {
            window.prevent_default();
            if current.disabled {
                return;
            }
            if let Some(child) = current.submenu() {
                open_submenu(&child, Focus::First, Reason::TriggerPress, Some(key_event), window, cx);
            }
            return;
        }
        // Activates before preventing the key's default, so
        // `prevent_default()` in `on_select` can still keep the menu open.
        self.activate(&key, Some(key_event), window, cx);
        window.prevent_default();
    }
}

// A press on a root trigger, until its release (`pointerPress`).
pub(crate) struct PressSession {
    pub(crate) trigger: Region,
    pub(crate) opening: bool,
    pub(crate) start: Point<Pixels>,
    pub(crate) at: Instant,
}

// Registers a data menu's item shortcuts in the trigger's layer, so a modal
// above it pauses them, and again whenever the items or `item_shortcuts`
// change (`bindItemShortcuts`). The registrations are the element state of
// an element the trigger renders, so they go once a frame doesn't render the
// trigger.
pub(crate) struct ItemShortcuts<P: Clone + PartialEq + Debug + 'static> {
    pub(crate) items: Option<Vec<(SharedString, SharedString)>>,
    pub(crate) layer: Region,
    pub(crate) menu: WeakEntity<Menu<P>>,
}

struct BoundItemShortcuts {
    items: Option<Vec<(SharedString, SharedString)>>,
    _registered: Vec<RegisteredShortcut>,
}

// The shortcuts a data menu's items name, by value, while `item_shortcuts`
// is on.
pub(crate) fn shortcut_items(items: Option<&MenuData>) -> Option<Vec<(SharedString, SharedString)>> {
    let items = items?;
    let mut found = Vec::new();
    each_shortcut(items, &mut |value, shortcut| {
        found.push((value.clone(), shortcut.clone()))
    });
    Some(found)
}

impl<P: Clone + PartialEq + Debug + 'static> Element for ItemShortcuts<P> {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("ui-sdk-menu-item-shortcuts".into()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        if let Some(id) = id {
            let items = self.items.clone();
            let layer = self.layer.clone();
            let menu = self.menu.clone();
            window.with_element_state::<BoundItemShortcuts, _>(id, |bound, _window| {
                let bound = match bound {
                    Some(bound) if bound.items == items => bound,
                    stale => {
                        drop(stale);
                        let registered = items
                            .iter()
                            .flatten()
                            .map(|(value, shortcut)| {
                                let value = value.clone();
                                let menu = menu.clone();
                                shortcuts().register(
                                    ShortcutKeys::Keys(shortcut.clone()),
                                    move |_event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
                                        menu.update(cx, |menu, cx| menu.run_shortcut(&value, window, cx)).ok();
                                    },
                                    ShortcutOptions::default().layer(layer.clone()),
                                    cx,
                                )
                            })
                            .collect();
                        BoundItemShortcuts {
                            items,
                            _registered: registered,
                        }
                    }
                };
                ((), bound)
            });
        }
        let style = Style {
            position: gpui::Position::Absolute,
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}
