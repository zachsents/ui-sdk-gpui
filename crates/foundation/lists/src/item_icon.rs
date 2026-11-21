//! Port of `foundation/lists/item-icon.ts`: the icon part every component
//! with items shares: a menu item's, a select item's, a tab's, a toggle's
//! and the rest. The icon is decorative, left out for assistive technology,
//! so an icon's glyph never joins the item's name, and it isn't part of the
//! text the item reports for typeahead, labels, filtering and highlighting.
//! Its item knows it has one, and the list around it that some item does
//! ([`has_icon`], [`has_icons`]), so items without one can line up, and it
//! can flip in right-to-left text.
//!
//! The original copies an icon's elements into a value that shows the
//! selected item's icon, like a select's trigger. gpui can't copy elements,
//! so an icon can bring its content as a function ([`ItemIconProps::content`]),
//! which the icon part renders and the value icon renders again.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{AnyElement, App, Context, EntityId, ParentElement, SharedString, Window};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, Instance, Part, flag_while_bound, key_while_bound, name_of};
use ui_sdk_platform::is_rtl;
use ui_sdk_text::{NameFrom, NameSources, check_name};

/// An icon's content, rendered where it's needed: in its item, and again in
/// a value that shows the selected item's icon.
pub type IconContent = Rc<dyn Fn() -> AnyElement>;

/// What an item's icon part takes (`ItemIconProps`).
#[derive(Clone, Default)]
pub struct ItemIconProps {
    /// Flips the icon horizontally in right-to-left text, for icons that
    /// point a way, like arrows and chevrons. Icons that picture a thing,
    /// like a clock or a check, stay as they are. Defaults to false.
    pub mirror: bool,
    /// The icon itself, which the part renders, and a value that shows the
    /// selected item's icon renders again. Without it, the part's own
    /// children are the icon, which the value can't show.
    pub content: Option<IconContent>,
}

impl ItemIconProps {
    pub fn mirror(mut self, mirror: bool) -> Self {
        self.mirror = mirror;
        self
    }

    pub fn content(mut self, content: impl Fn() -> AnyElement + 'static) -> Self {
        self.content = Some(Rc::new(content));
        self
    }
}

/// What an icon part styles from (`ItemIconState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemIconState {
    /// Whether it flips in right-to-left text (`data-mirror`).
    pub mirror: bool,
    /// Whether to flip it now: it mirrors and the app lays out right to
    /// left. gpui has no CSS to flip it with, so transform the icon, like an
    /// `svg` with `Transformation::scale(size(-1., 1.))`, while it's set.
    pub flipped: bool,
}

/// Where an icon sits: the component it's in, for warnings, and the roles
/// of the item it decorates and of the list that item is in, like an option
/// in a listbox (`IconPlacement`). The original finds both by selectors;
/// here the component names the item by its slot and key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IconPlacement {
    pub component: &'static str,
    pub item: Role,
    pub list: Option<Role>,
}

/// Where an option's icon is: in an option of a listbox (`optionIcon`).
pub const fn option_icon(component: &'static str) -> IconPlacement {
    IconPlacement {
        component,
        item: Role::Option,
        list: Some(Role::Listbox),
    }
}

/// Whether an icon part is in the page inside the item with `key`, its
/// item's `data-has-icon`. Asking makes the component render again as icons
/// come and go.
pub fn has_icon(instance: &Instance, key: &str) -> bool {
    key_while_bound(instance, "item-icon").iter().any(|item| item == key)
}

/// Whether some item's icon is in the page, the list's `data-has-icons`.
pub fn has_icons(instance: &Instance) -> bool {
    flag_while_bound(instance, "item-icon").unwrap_or(false)
}

/// An icon inside an item, like a menu item, an option, a tab or a toggle
/// (`itemIconPart`), for the slot its component places it in, like
/// `"item-icon"`, a tab's `"tab-icon"` or a toggle's `"icon"`, by its item's
/// key. Decorative, so it's left out for assistive technology; it renders
/// `content` when the props bring it.
pub fn icon_part(
    instance: &Instance,
    slot: &'static str,
    item: Option<&str>,
    props: &ItemIconProps,
    cx: &App,
) -> Part<ItemIconState> {
    let state = ItemIconState {
        mirror: props.mirror,
        flipped: props.mirror && is_rtl(cx),
    };
    let part = Part::new(instance, slot, item, state)
        .aria_role(Role::Group)
        .aria(Aria::new().hidden(true));
    match &props.content {
        Some(content) => part.child(content()),
        None => part,
    }
}

/// The icon of a list's item, in the `item-icon` slot ([`icon_part`]).
pub fn item_icon_part(instance: &Instance, item: Option<&str>, props: &ItemIconProps, cx: &App) -> Part<ItemIconState> {
    icon_part(instance, "item-icon", item, props, cx)
}

/// Warns, in development builds, when an item shows only an icon, which
/// names nothing: its item has no other name (`checkName`'s "An item with
/// only an icon"). It reads the name the item part of `slot`, with `key`,
/// rendered with last, so the item part keeps it (`Part::keep_name`) while
/// an icon is in it, and calls this once it has rendered. A labelled-by
/// counts as a name, since its targets' text can't be read.
pub fn check_icon_item(
    instance: &Instance,
    slot: &'static str,
    key: Option<&str>,
    placement: IconPlacement,
    window: &Window,
) {
    if !cfg!(debug_assertions) {
        return;
    }
    let Some(name) = name_of(instance, slot, key) else {
        return;
    };
    if !name.labelled_by.is_empty() {
        return;
    }
    let mut sources = NameSources::new(placement.item);
    if let Some(label) = name.aria_label {
        sources = sources.aria_label(label);
    }
    check_name(
        &instance.element_id(slot, key),
        &sources,
        "An item with only an icon",
        NameFrom {
            component: placement.component,
            id: Some(instance.id().clone()),
        },
        window,
    );
}

/// An item's icon, as a value showing the selected item's icon reads it.
#[derive(Clone)]
pub struct ItemIcon {
    pub content: IconContent,
    pub mirror: bool,
}

/// The icons of a list's items by key, for a value that shows the selected
/// item's icon (`createItemIcons`). An icon is remembered after its item
/// leaves the page, like a label, so a closed list's value keeps showing
/// it. Clones share them.
#[derive(Clone)]
pub struct ItemIcons {
    icons: Rc<RefCell<HashMap<SharedString, ItemIcon>>>,
    queued: Rc<Cell<bool>>,
    entity: EntityId,
}

/// The icons of the component of `cx`'s items, which renders again once an
/// icon it can show arrives (`createItemIcons`).
pub fn create_item_icons<C: 'static>(cx: &Context<C>) -> ItemIcons {
    ItemIcons {
        icons: Rc::new(RefCell::new(HashMap::new())),
        queued: Rc::new(Cell::new(false)),
        entity: cx.entity_id(),
    }
}

impl ItemIcons {
    /// The icon of an item.
    pub fn get(&self, key: &str) -> Option<ItemIcon> {
        self.icons.borrow().get(key).cloned()
    }

    /// Remembers an item's icon until another replaces it. An icon that's
    /// new to the list renders the component again, batched, since a long
    /// list registers all at once.
    pub fn register(&self, key: SharedString, icon: ItemIcon, cx: &App) {
        let fresh = {
            let mut icons = self.icons.borrow_mut();
            let fresh = icons.get(&key).is_none_or(|known| known.mirror != icon.mirror);
            icons.insert(key, icon);
            fresh
        };
        if !fresh || self.queued.replace(true) {
            return;
        }
        let queued = self.queued.clone();
        let entity = self.entity;
        cx.spawn(async move |cx| {
            queued.set(false);
            cx.update(|cx| cx.notify(entity));
        })
        .detach();
    }

    /// The selected item's icon, while exactly one item is selected and it
    /// has one (`selectedIcon`).
    pub fn selected(&self, keys: &[SharedString]) -> Option<ItemIcon> {
        match keys {
            [key] => self.get(key),
            _ => None,
        }
    }
}

/// Remembers an option's icon by its item's key, for a value that shows it
/// (`rememberIcon`). An icon without content has nothing to show again.
pub fn remember_icon(icons: &ItemIcons, item: Option<&str>, props: &ItemIconProps, cx: &App) {
    if let (Some(item), Some(content)) = (item, &props.content) {
        icons.register(
            SharedString::from(item.to_string()),
            ItemIcon {
                content: content.clone(),
                mirror: props.mirror,
            },
            cx,
        );
    }
}

/// The selected item's icon, outside the list: in a select's trigger, or
/// next to a combobox's input (`valueIconPart`). In the page while one item
/// is selected and it has an icon, which it renders again. Decorative.
pub fn value_icon_part(instance: &Instance, icons: &ItemIcons, keys: &[SharedString], cx: &App) -> Option<Part<ItemIconState>> {
    let icon = icons.selected(keys)?;
    let state = ItemIconState {
        mirror: icon.mirror,
        flipped: icon.mirror && is_rtl(cx),
    };
    Some(
        Part::new(instance, "value-icon", None, state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true))
            .child((icon.content)()),
    )
}

/// Item icons as a feature (`itemIconsFeature`): the icon inside an item,
/// the selected item's icon outside the list, and the icons by key the
/// second renders again. Make it in the component's constructor and keep it
/// in a field.
#[derive(Clone)]
pub struct ItemIconsFeature {
    pub icons: ItemIcons,
    pub placement: IconPlacement,
}

/// Item icons for the component of `cx`, its icons sitting as `placement`
/// says.
pub fn item_icons_feature<C: 'static>(placement: IconPlacement, cx: &Context<C>) -> ItemIconsFeature {
    ItemIconsFeature {
        icons: create_item_icons(cx),
        placement,
    }
}

impl ItemIconsFeature {
    /// The feature's `item-icon` part, which remembers its icon for the
    /// value.
    pub fn item_icon(&self, instance: &Instance, item: Option<&str>, props: &ItemIconProps, cx: &App) -> Part<ItemIconState> {
        remember_icon(&self.icons, item, props, cx);
        item_icon_part(instance, item, props, cx)
    }

    /// The feature's `value-icon` part, for the value's `keys`.
    pub fn value_icon(&self, instance: &Instance, keys: &[SharedString], cx: &App) -> Option<Part<ItemIconState>> {
        value_icon_part(instance, &self.icons, keys, cx)
    }
}
