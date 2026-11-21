//! Port of `components/listbox/listbox-root-parts.ts`: Listbox's parts and
//! what they carry.

use std::fmt::Debug;

use gpui::{App, ClickEvent, ElementId, Entity, InteractiveElement, KeyboardButton, SharedString, StatefulInteractiveElement};
use ui_sdk_focus::{Orientation, hold_focus};
use ui_sdk_kernel::dev;
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::{HandledKey, Role};
use ui_sdk_kernel::{Aria, InputEvent, Part};
use ui_sdk_lists::item_icon::{ItemIconProps, ItemIconState, check_icon_item, has_icon, item_icon_part, option_icon};
use ui_sdk_lists::listbox_engine::{ItemKey, ItemMeta};
use ui_sdk_lists::listbox_keys::option_id;
use ui_sdk_lists::listbox_parts::{
    FormState, OptionGroupState, OptionSeparatorState, bind_form_root, group_label_part, item_indicator_part,
    no_items, option_group_part, option_separator_part, position_attrs, register_item,
};

use crate::listbox_root::{
    ListboxItemIndicatorProps, ListboxItemIndicatorState, ListboxItemProps, ListboxItemState, ListboxState,
};
use crate::listbox_root_api::Listbox;
use crate::listbox_root_keyboard::register_root;

// The keys the root handles, by its orientation and whether several items
// can be selected.
const VERTICAL_KEYS: &[HandledKey] = &["ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown", "Enter", " ", "typeahead"];
const VERTICAL_MULTIPLE_KEYS: &[HandledKey] = &[
    "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown", "Enter", " ", "typeahead", "mod+a",
];
const HORIZONTAL_KEYS: &[HandledKey] = &[
    "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown", "Enter", " ", "typeahead",
];
const HORIZONTAL_MULTIPLE_KEYS: &[HandledKey] = &[
    "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown", "Enter", " ", "typeahead", "mod+a",
];

impl<T: ItemKey + Clone + PartialEq + Debug + 'static> Listbox<T> {
    /// What the root part styles from (`rootState`).
    pub fn state(&self) -> ListboxState<T> {
        ListboxState {
            multiple: self.multiple,
            orientation: self.orientation,
            disabled: self.is_disabled(),
            read_only: self.read_only,
            required: self.required,
            items: self
                .listbox
                .visible_data(self)
                .map(|visible| visible.list)
                .unwrap_or_else(no_items),
        }
    }

    // What the form reads (`formAttrs`, `bindFormRoot`).
    pub(crate) fn form_state(&self) -> FormState {
        FormState {
            multiple: self.multiple,
            disabled: self.disabled,
            form_disabled: self.internals.form_disabled(),
            read_only: self.read_only,
            required: self.required,
            name: self.name.clone(),
            form: self.form.clone(),
        }
    }

    // An item's value. A data item's value comes from the data, where
    // `is_item_equal_to_value` expects it (`valueOfItem`).
    fn value_of_item(&self, key: &SharedString, props: &ListboxItemProps<T>) -> T {
        if self.listbox.data_items(self).is_some() {
            return self.listbox.value_of(self, key).unwrap_or_else(|| props.value.clone());
        }
        props.value.clone()
    }

    fn item_state(&self, key: &SharedString, props: &ListboxItemProps<T>) -> ListboxItemState {
        let value = self.value_of_item(key, props);
        ListboxItemState {
            highlighted: self.listbox.highlighted().as_ref() == Some(key),
            selected: self.selection.is_selected(self, key, Some(&value)),
            disabled: props.disabled || self.is_disabled(),
        }
    }

    // Whether an item holds the list's one tab stop. Before the items have
    // rendered, the first selected item claims it, or with nothing selected,
    // the first enabled one.
    fn is_stop(&self, key: &SharedString, props: &ListboxItemProps<T>) -> bool {
        if let Some(stop) = &self.stop {
            return stop == key;
        }
        let eligible = if self.selection.keys(self).is_empty() {
            !props.disabled
        } else {
            let value = self.value_of_item(key, props);
            self.selection.is_selected(self, key, Some(&value))
        };
        self.claim.claim(key, eligible)
    }

    /// The `root` part: the listbox itself. It takes focus only to hand it
    /// to the option that holds the tab stop, unless `virtual_focus` keeps
    /// focus here. Give it an `aria_label`, or name it with a label. To
    /// scroll a long list, give it a height and `overflow_y_scroll`: the
    /// options keys move to scroll into view. Build it before its items, which
    /// it starts the render of, so the order they render in is page order.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<ListboxState<T>> {
        let listbox = this.read(cx);
        listbox.listbox.begin(cx);
        register_root(&listbox.root_handle);
        let state = listbox.state();
        let disabled = state.disabled;
        let keys = match (state.orientation, state.multiple) {
            (Orientation::Horizontal, true) => HORIZONTAL_MULTIPLE_KEYS,
            (Orientation::Horizontal, false) => HORIZONTAL_KEYS,
            (Orientation::Vertical, true) => VERTICAL_MULTIPLE_KEYS,
            (Orientation::Vertical, false) => VERTICAL_KEYS,
        };
        let orientation = state.orientation;
        // The labels pointing at the list name it, and a field around it
        // describes it.
        bind_form_root(&listbox.internals, &listbox.form_state(), &listbox.selection.keys(listbox));
        let mut aria = Aria::new()
            .multiselectable(state.multiple)
            .disabled(disabled)
            .read_only(state.read_only)
            .required(state.required);
        if listbox.virtual_focus
            && let Some(highlighted) = listbox.listbox.highlighted()
        {
            aria = aria.active_descendant(Some(ElementId::Name(option_id(listbox.instance.id(), &highlighted))));
        }
        // Never a tab stop of its own, even disabled, unless `virtual_focus`
        // keeps focus here.
        let handle = listbox.root_handle.clone().tab_stop(listbox.virtual_focus && !disabled);
        let scroll = listbox.scroll_handle.clone();
        let internals = listbox.internals.clone();
        let keyed = this.downgrade();
        let part = Part::new(&listbox.instance, "root", None, state)
            .aria_role(Role::Listbox)
            .aria_orientation(orientation)
            .map_el(|el| internals.track(el).track_focus(&handle).track_scroll(&scroll))
            .aria(aria)
            .aria(listbox.internals.aria())
            .on_keys(keys, move |event, window, cx| {
                keyed
                    .update(cx, |listbox, cx| listbox.on_key_down(event, window, cx))
                    .ok();
            });
        if !dev::DEV {
            return part;
        }
        let bound = this.downgrade();
        part.keep_name().on_props(move |window, cx| {
            bound.update(cx, |listbox, cx| listbox.root_rendered(window, cx)).ok();
        })
    }

    /// The `item` part for an option, keyed by its value. It holds the
    /// list's one tab stop while it's the stop, takes focus as keys move to
    /// it, and with `virtual_focus` is the root's active descendant instead.
    /// Its `text` names it; an `aria_label` of your own wins.
    pub fn item(this: &Entity<Self>, props: ListboxItemProps<T>, cx: &App) -> Part<ListboxItemState> {
        let listbox = this.read(cx);
        let key = listbox.key_of(&props.value);
        let state = listbox.item_state(&key, &props);
        let list_disabled = listbox.is_disabled();
        let virtual_focus = listbox.virtual_focus;
        let instance = &listbox.instance;
        let mut part = Part::new(instance, "item", Some(&key), state)
            .with_element_id(ElementId::Name(option_id(instance.id(), &key)))
            .aria_role(Role::Option);
        part = if virtual_focus {
            part.focus_mode(FocusMode::ActiveDescendant, None, false)
        } else if list_disabled {
            part.focus_mode(FocusMode::Roving { tab_stop: false }, None, true)
        } else {
            let handle = listbox.listbox.focus_handle(&key, cx);
            let tab_stop = listbox.is_stop(&key, &props);
            part.focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
        };
        part = part
            .aria_selected(state.selected)
            .aria(Aria::new().disabled(state.disabled));
        if let Some(position) = position_attrs(props.index, &listbox.listbox, listbox) {
            part = position.apply(part);
        }
        if let Some(name) = props.text.clone().or_else(|| props.label.clone()) {
            part = part.aria_label(name);
        }
        if virtual_focus {
            // With `virtual_focus`, the pointer moves the highlight, and a
            // press keeps focus on the list. gpui's mouse moves never come
            // from a finger.
            let hovered = this.downgrade();
            let hover_key = key.clone();
            part = part
                .on_mouse_move(move |event, window, cx| {
                    hovered
                        .update(cx, |listbox, cx| {
                            let highlighted = listbox.listbox.highlighted();
                            if highlighted.is_some() && highlighted.as_ref() != Some(&hover_key) {
                                listbox.highlight_virtually(&hover_key, Some(InputEvent::from(event)), window, cx);
                            }
                        })
                        .ok();
                })
                .on_any_mouse_down(hold_focus);
        }
        let pressed = this.downgrade();
        let press_key = key.clone();
        part = part.on_press(move |event, window, cx| {
            pressed
                .update(cx, |listbox, cx| {
                    // The list's keys choose the focused option. The click gpui
                    // makes of Enter on it only follows a link, as the press a
                    // link's Enter sends.
                    if let InputEvent::Click(ClickEvent::Keyboard(click)) = &event {
                        let link = listbox.listbox.element(&press_key).is_some_and(|element| element.link);
                        if !link || click.button != KeyboardButton::Enter {
                            return;
                        }
                    }
                    if listbox.virtual_focus {
                        listbox.focus_stop(window, cx);
                        listbox.highlight_virtually(&press_key, Some(event.clone()), window, cx);
                    }
                    listbox.press(&press_key, Some(event), window, cx);
                })
                .ok();
        });
        // An item that shows only an icon names nothing.
        if dev::DEV && has_icon(instance, &key) {
            let entity = this.downgrade();
            let key = key.clone();
            part = part.keep_name().on_props(move |window, cx| {
                if let Some(listbox) = entity.upgrade() {
                    check_icon_item(&listbox.read(cx).instance, "item", Some(&key), option_icon("listbox"), window);
                }
            });
        }
        let value = listbox.value_of_item(&key, &props);
        let meta = ItemMeta::new(value)
            .label(props.label.clone())
            .text(props.text.clone())
            .disabled(props.disabled)
            .group(props.group.clone())
            .index(props.index)
            .link(props.link);
        register_item(&listbox.listbox, key, meta, part, cx)
    }

    /// The `item-indicator` part: the check inside the item with `item`'s
    /// value, while it's selected, or always with `keep_mounted`. `None`
    /// while it's out of the page.
    pub fn item_indicator(
        this: &Entity<Self>,
        item: &T,
        props: ListboxItemIndicatorProps,
        cx: &App,
    ) -> Option<Part<ListboxItemIndicatorState>> {
        let listbox = this.read(cx);
        let key = listbox.key_of(item);
        let selected = listbox.selection.is_selected(listbox, &key, None);
        item_indicator_part(&listbox.instance, &key, selected, props.keep_mounted)
    }

    /// The `item-icon` part, inside the item with `item`'s value. Screen
    /// readers and typeahead skip it, and with `mirror` it says to flip in
    /// right-to-left text.
    pub fn item_icon(this: &Entity<Self>, item: &T, props: ItemIconProps, cx: &App) -> Part<ItemIconState> {
        let listbox = this.read(cx);
        let key = listbox.key_of(item);
        item_icon_part(&listbox.instance, Some(&key), &props, cx)
    }

    /// A `group` of items, by a key of your own, named by its group label.
    /// Nothing filters a listbox, so it never hides. Give its items the same
    /// key as their `group`.
    pub fn group(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part<OptionGroupState> {
        let listbox = this.read(cx);
        let key = key.into();
        option_group_part(&listbox.instance, &key, &listbox.listbox, listbox)
    }

    /// The `group-label` of the group with `group`'s key, which names the
    /// group while it's in the page. Give it its text as an `aria_label`.
    pub fn group_label(this: &Entity<Self>, group: impl Into<SharedString>, cx: &App) -> Part<()> {
        let listbox = this.read(cx);
        let group = group.into();
        group_label_part(&listbox.instance, &group)
    }

    /// A `separator` between items or groups, by a key of your own. A
    /// listbox may only hold options and groups, so separators are left out
    /// for assistive technology.
    pub fn separator(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part<OptionSeparatorState> {
        let listbox = this.read(cx);
        let key = key.into();
        option_separator_part(&listbox.instance, Some(&key), false)
    }
}
