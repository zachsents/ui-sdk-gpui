//! Port of `components/listbox/listbox-root.ts`: an always-visible list of
//! options to choose one or several from, like a `<select size>` or
//! `<select multiple>`, on the listbox engine Select and Combobox share.
//! This module is the public api: the options, the value, the reasons and
//! what each part styles from. The state and the api live in
//! [`listbox_root_api`](crate::listbox_root_api), keys and presses in
//! [`listbox_root_keyboard`](crate::listbox_root_keyboard), and parts in
//! [`listbox_root_parts`](crate::listbox_root_parts).

use std::rc::Rc;

use gpui::{App, SharedString, Window};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentArgs, AgentError, AgentItem, AgentState, AgentSurface, AgentValue,
    ChangeCallback, ChangeDetails, ChangeOptions, Reason,
};
use ui_sdk_lists::listbox_engine::ItemKey;
use ui_sdk_lists::listbox_engine::{ListData, ListHighlightDetails};
use ui_sdk_lists::item_icon::{ItemIconProps, ItemIconState};
use ui_sdk_lists::listbox_parts::ItemIndicatorState;
use ui_sdk_lists::listbox_value::ListValue;

use crate::listbox_root_api::Listbox;

/// Why a listbox's value wants to change: a press, Space or Enter on an
/// option (`ItemPress`), keys that select as they move, Shift ranges with
/// arrows and Ctrl or ⌘ with A (`ListNavigation`), a form reset (`Reset`)
/// or restore (`Restore`), and a call from the app (`Imperative`).
pub type ListboxValueChangeReason = Reason;

/// Options for every write method on a listbox.
pub type ListboxChangeOptions = ChangeOptions;

/// Which arrow keys move between items: Up and Down, or Left and Right,
/// swapped in right-to-left text.
pub type ListboxOrientation = gpui::Orientation;

/// The value of a listbox (`ListboxValue`): one item's value or nothing,
/// `Single`, or with `multiple` a list, `Multiple`.
pub type ListboxValue<T> = ListValue<T>;

/// Whether the selection follows focus (`activation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListboxActivation {
    /// Arrow keys, Home, End and typing select the option they move to, and
    /// with `multiple` a press replaces the selection while Ctrl or ⌘ adds to
    /// it, like a native multiple select.
    Automatic,
    /// Keys only move focus, and Space, Enter or a press selects, or toggles
    /// with `multiple`.
    Manual,
}

impl ListboxActivation {
    /// The original's name for it, like `"automatic"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ListboxActivation::Automatic => "automatic",
            ListboxActivation::Manual => "manual",
        }
    }
}

/// `item_to_value`: the string an item's value submits as and is identified
/// by.
pub type ItemToValueFn<T> = Rc<dyn Fn(&T) -> SharedString>;

/// `is_item_equal_to_value`: whether an item's value is the selected value.
pub type ItemEqualFn<T> = Rc<dyn Fn(&T, &T) -> bool>;

/// `on_highlight`: the value focus moved to, or `None`, and where it is. It
/// runs inside the listbox's update, so it may update any entity but the
/// listbox.
pub type HighlightCallback<T> = Rc<dyn Fn(Option<&T>, &ListHighlightDetails, &mut Window, &mut App)>;

/// The options `Listbox` takes (`ListboxOptions`).
pub struct ListboxOptions<T> {
    /// The selected value. Pass it to control the listbox: `Single(None)`
    /// selects nothing, and with `multiple` it's `Multiple`.
    pub value: Option<ListboxValue<T>>,
    /// The value selected at first, when uncontrolled. A form reset returns
    /// to it. Without one, nothing: `None`, or an empty list with `multiple`.
    pub default_value: Option<ListboxValue<T>>,
    /// Runs when a user, a form reset or a call wants to change the value.
    /// `details.reason` is one of [`ListboxValueChangeReason`]'s. It runs
    /// once the listbox's update is over, so it may update any entity, the
    /// listbox too.
    pub on_value_change: Option<ChangeCallback<ListboxValue<T>>>,
    /// Runs as the value is asked to change, before it does, inside the
    /// listbox's update: `details.cancel()` stops it. It may update any
    /// entity but the listbox.
    pub on_before_value_change: Option<ChangeCallback<ListboxValue<T>>>,
    /// Whether several items can be selected. The value is then a list,
    /// Shift selects ranges, and Ctrl or ⌘ with A selects every item.
    pub multiple: bool,
    /// Whether the selection follows focus. Defaults to `Automatic`, or
    /// `Manual` with `multiple`.
    pub activation: Option<ListboxActivation>,
    /// Whether pressing the selected item of a single listbox unselects it,
    /// so nothing is selected.
    pub deselectable: bool,
    /// Which arrow keys move between items. Horizontal lists use Left and
    /// Right, swapped in right-to-left text. Defaults to vertical.
    pub orientation: Option<ListboxOrientation>,
    /// Whether arrow keys wrap from the last item to the first. Shift with an
    /// arrow never wraps. Defaults to true.
    pub r#loop: bool,
    /// Keeps focus on the list itself, which points at the highlighted option
    /// as its active descendant, instead of moving focus onto the options.
    pub virtual_focus: bool,
    /// Whether the list scrolls the option keys move to into view, and the
    /// selected option when it first shows. Turn it off when something else
    /// scrolls the list, like a scroll picker. Defaults to true.
    pub keep_in_view: bool,
    /// The items as data, which lets typing find items a virtualized list
    /// hasn't rendered. The root's state hands them back for rendering.
    pub items: Option<ListData<T>>,
    /// The string an item's value submits as and is identified by, for
    /// values of your own type. Strings pass through.
    pub item_to_value: Option<ItemToValueFn<T>>,
    /// Whether an item's value is the selected value, for values that aren't
    /// equal but stand for the same item.
    pub is_item_equal_to_value: Option<ItemEqualFn<T>>,
    /// Runs when focus moves to another item, with its index among the
    /// items, so a virtualized list can scroll to it.
    pub on_highlight: Option<HighlightCallback<T>>,
    /// Disables the listbox like a native control: its items can't be
    /// focused or chosen, and the value isn't submitted.
    pub disabled: bool,
    /// Keeps the listbox focusable and its value submitted, but stops
    /// changes.
    pub read_only: bool,
    /// Blocks form submission until a value is selected, with the browser's
    /// own message.
    pub required: bool,
    /// The name the value submits under. Without one, the listbox submits
    /// nothing.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when the listbox isn't inside that
    /// form.
    pub form: Option<SharedString>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl<T> Default for ListboxOptions<T> {
    fn default() -> Self {
        Self {
            value: None,
            default_value: None,
            on_value_change: None,
            on_before_value_change: None,
            multiple: false,
            activation: None,
            deselectable: false,
            orientation: None,
            r#loop: true,
            virtual_focus: false,
            keep_in_view: true,
            items: None,
            item_to_value: None,
            is_item_equal_to_value: None,
            on_highlight: None,
            disabled: false,
            read_only: false,
            required: false,
            name: None,
            form: None,
            debug: false,
        }
    }
}

impl<T: Clone> Clone for ListboxOptions<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            default_value: self.default_value.clone(),
            on_value_change: self.on_value_change.clone(),
            on_before_value_change: self.on_before_value_change.clone(),
            multiple: self.multiple,
            activation: self.activation,
            deselectable: self.deselectable,
            orientation: self.orientation,
            r#loop: self.r#loop,
            virtual_focus: self.virtual_focus,
            keep_in_view: self.keep_in_view,
            items: self.items.clone(),
            item_to_value: self.item_to_value.clone(),
            is_item_equal_to_value: self.is_item_equal_to_value.clone(),
            on_highlight: self.on_highlight.clone(),
            disabled: self.disabled,
            read_only: self.read_only,
            required: self.required,
            name: self.name.clone(),
            form: self.form.clone(),
            debug: self.debug,
        }
    }
}

impl<T> ListboxOptions<T> {
    pub fn value(mut self, value: impl Into<ListboxValue<T>>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn default_value(mut self, value: impl Into<ListboxValue<T>>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&ListboxValue<T>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&ListboxValue<T>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    pub fn activation(mut self, activation: ListboxActivation) -> Self {
        self.activation = Some(activation);
        self
    }

    pub fn deselectable(mut self, deselectable: bool) -> Self {
        self.deselectable = deselectable;
        self
    }

    pub fn orientation(mut self, orientation: ListboxOrientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
        self
    }

    pub fn virtual_focus(mut self, virtual_focus: bool) -> Self {
        self.virtual_focus = virtual_focus;
        self
    }

    pub fn keep_in_view(mut self, keep_in_view: bool) -> Self {
        self.keep_in_view = keep_in_view;
        self
    }

    pub fn items(mut self, items: impl Into<ListData<T>>) -> Self {
        self.items = Some(items.into());
        self
    }

    pub fn item_to_value(mut self, item_to_value: impl Fn(&T) -> SharedString + 'static) -> Self {
        self.item_to_value = Some(Rc::new(item_to_value));
        self
    }

    pub fn is_item_equal_to_value(mut self, equal: impl Fn(&T, &T) -> bool + 'static) -> Self {
        self.is_item_equal_to_value = Some(Rc::new(equal));
        self
    }

    pub fn on_highlight(
        mut self,
        callback: impl Fn(Option<&T>, &ListHighlightDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_highlight = Some(Rc::new(callback));
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn form(mut self, form: impl Into<SharedString>) -> Self {
        self.form = Some(form.into());
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props of an item (`ListboxItemProps`).
#[derive(Clone, Debug, PartialEq)]
pub struct ListboxItemProps<T> {
    /// What the value becomes when the item is chosen.
    pub value: T,
    /// The text typing finds, when it isn't the text the item shows. Not the
    /// accessible name: `text`, or an `aria_label` of your own, is that.
    pub label: Option<SharedString>,
    /// The text the item shows, which typing finds and a screen reader reads
    /// it as. gpui can't read text back from what the item renders, so give
    /// it here; without it, typing finds nothing unless `label` says.
    pub text: Option<SharedString>,
    /// Keeps the item from being chosen. Arrow keys, typing and ranges skip
    /// it, and it reports `aria-disabled`.
    pub disabled: bool,
    /// The item's position among all items, for virtualized lists that
    /// render only some. It reports its place in the set.
    pub index: Option<usize>,
    /// The group it's in, by the key its group part has.
    pub group: Option<SharedString>,
    /// It's a link, which the app follows in its own press on the part
    /// (`on_press`): Enter clicks it, so it follows itself and the press
    /// chooses it.
    pub link: bool,
}

impl<T> ListboxItemProps<T> {
    pub fn new(value: impl Into<T>) -> Self {
        Self {
            value: value.into(),
            label: None,
            text: None,
            disabled: false,
            index: None,
            group: None,
            link: false,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn index(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }

    pub fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn link(mut self, link: bool) -> Self {
        self.link = link;
        self
    }
}

/// The props of the check inside an item (`ListboxItemIndicatorProps`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListboxItemIndicatorProps {
    /// Whether the indicator stays in the page while its item isn't
    /// selected, so it can be styled or animated in both states.
    pub keep_mounted: bool,
}

impl ListboxItemIndicatorProps {
    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }
}

/// What the `root` part styles from (`ListboxState`).
#[derive(Clone, Debug, PartialEq)]
pub struct ListboxState<T> {
    /// Whether several items can be selected.
    pub multiple: bool,
    /// Which arrow keys move between items (`data-orientation`).
    pub orientation: ListboxOrientation,
    /// Whether the listbox is disabled, by itself, a Field or a fieldset
    /// (`data-disabled`).
    pub disabled: bool,
    /// Whether the listbox keeps its value but refuses changes, from its
    /// `read_only` option. A form reset still restores it (`data-readonly`).
    pub read_only: bool,
    /// Whether a value is required (`data-required`).
    pub required: bool,
    /// The data items, for rendering a virtualized list yourself. Empty
    /// without `items`.
    pub items: ListData<T>,
}

/// What the `item` part styles from (`ListboxItemState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListboxItemState {
    /// Whether the item has focus, or with `virtual_focus` the highlight
    /// (`data-highlighted`).
    pub highlighted: bool,
    /// Whether its value is in the listbox's value, compared with
    /// `is_item_equal_to_value` when you pass one (`data-selected`).
    pub selected: bool,
    /// Whether the item can't be chosen, by itself or a disabled listbox
    /// (`data-disabled`).
    pub disabled: bool,
}

/// The props the `item-icon` part takes (`ListboxItemIconProps`).
pub type ListboxItemIconProps = ItemIconProps;

/// What the `item-icon` part styles from (`ListboxItemIconState`).
pub type ListboxItemIconState = ItemIconState;

/// What the `item-indicator` part styles from (`ListboxItemIndicatorState`):
/// whether its item is selected. While it isn't, the indicator isn't shown
/// unless `keep_mounted` is set.
pub type ListboxItemIndicatorState = ItemIndicatorState;

/// `agent: { actions: ["setValue", "clear", "selectAll"], closed: true }`:
/// an agent names items by their keys, as the state shows them.
impl<T: ItemKey + Clone + PartialEq + std::fmt::Debug + 'static> Agent for Listbox<T> {
    const SURFACE: AgentSurface = AgentSurface::new().closed(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |listbox, args, window, cx| {
            let value = listbox.agent_value(args)?;
            Ok(listbox.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |listbox, _args, window, cx| {
            Ok(listbox.clear(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("selectAll", |listbox, _args, window, cx| {
            Ok(listbox.select_all(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        let keys = self.selection.keys(self);
        let value = if self.multiple {
            AgentValue::from(keys.iter().map(|key| key.to_string()).collect::<Vec<_>>())
        } else {
            keys.first().map_or(AgentValue::Null, |key| AgentValue::from(key.to_string()))
        };
        AgentState::new()
            .with("value", value)
            .with("multiple", self.multiple)
            .with("disabled", self.is_disabled())
            .with("readOnly", self.read_only)
            .with("required", self.required)
    }

    fn agent_items(&self, _cx: &App) -> Option<Vec<AgentItem>> {
        Some(
            self.listbox
                .entries(self)
                .iter()
                .map(|entry| AgentItem::new(entry.key.clone(), entry.label.clone()).disabled(entry.disabled))
                .collect(),
        )
    }
}

impl<T: ItemKey + Clone + PartialEq + std::fmt::Debug + 'static> Listbox<T> {
    // The value an agent's keys name: one key or none, or a list with
    // `multiple`. A key no item has is refused, since the value is always
    // one of the items.
    fn agent_value(&self, args: &AgentArgs) -> Result<ListboxValue<T>, AgentError> {
        let value_of = |key: &SharedString| {
            self.listbox
                .value_of(self, key)
                .ok_or_else(|| AgentError::refused(format!("No item has the value \"{key}\".")))
        };
        if self.multiple {
            let keys = args.list(0)?;
            return Ok(ListValue::Multiple(keys.iter().map(value_of).collect::<Result<_, _>>()?));
        }
        if args.is_missing(0) {
            return Ok(ListValue::Single(None));
        }
        Ok(ListValue::Single(Some(value_of(&args.string(0)?)?)))
    }
}
