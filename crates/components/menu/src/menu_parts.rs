//! Port of `components/menu/menu-parts.ts`: the menu's parts, one method
//! each, and what choosing an item does. The context trigger, the checkable
//! items and the item icon are the original's features, parts a menu ships
//! only once a page renders them; here they're methods like the rest.

use std::fmt::Debug;
use std::rc::Rc;

use gpui::{
    AbsoluteLength, AnyElement, App, Context, DefiniteLength, Div, Entity, InteractiveElement, Interactivity,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, Pixels, SharedString, Stateful, StatefulInteractiveElement,
    StyleRefinement, Styled, WeakEntity, Window, px,
};
use ui_sdk_direct_manipulation::{BindHoldOptions, Hold, HoldInput};
use ui_sdk_kernel::accesskit::{Orientation as AccessOrientation, Role as AccessRole};
use ui_sdk_kernel::part::with_role;
use ui_sdk_kernel::roles::{HandledKey, Role};
use ui_sdk_kernel::{
    ActionOutcome, Aria, FocusMode, HasPopup, InputEvent, Key, Part, Reason, SelectEvent, dev, focus_events,
    is_composing, key_while_bound,
};
use ui_sdk_lists::{has_icon, item_icon_part};
use ui_sdk_motion::Presence;
use ui_sdk_platform::{
    Anchor, ButtonElementOptions, Region, aria_keyshortcuts, button_element_attrs, button_role, is_apple, is_rtl,
};
use ui_sdk_popups::{
    HoverEvent, PopupContent, PopupContentElement, PopupContentProps, PopupOpener, PopupTriggerProps, PopupViewport,
    ReturnMode, SafeAreaAxis, bind_popup_content, owns_event, popup_trigger_attrs, popup_viewport, set_unfocusable,
};
use ui_sdk_shortcuts::{
    ShortcutOptions, ShortcutPart, ShortcutProps, adopt_platform, bind_part_shortcut, shortcut_of, shortcut_slot,
};

use crate::menu::{
    Menu, MenuCheckboxItemState, MenuContentState, MenuItemIconProps, MenuItemIconState, MenuItemIndicatorState,
    MenuItemState, MenuRadioGroupState, MenuRadioItemState, MenuTriggerState,
};
use crate::menu_api::{
    CheckboxProps, Close, ContentProps, ContextEntry, ContextTriggerProps, Entry, EntryItem, Focus, IndicatorProps,
    ItemProps, ItemShortcuts, KeyFollowUp, MenuShared, RadioGroupProps, RadioProps, Show, TriggerProps, Work,
    open_submenu, shortcut_items,
};
use crate::menubar::{BarArrow, MenubarClose, MenubarMember, MenubarOpen};

// The keys the content handles.
const CONTENT_KEYS: &[HandledKey] = &[
    "ArrowDown",
    "ArrowUp",
    "ArrowRight",
    "ArrowLeft",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Enter",
    " ",
    "Escape",
    "Tab",
    "typeahead",
];

/// The checkbox or radio item an `item-indicator` is in (`IndicatorProps`'
/// `item`, `itemKey` and `group`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MenuIndicatorItem {
    /// A checkbox item, by its value.
    Checkbox(SharedString),
    /// A radio item, by its group's name and its value.
    Radio(SharedString, SharedString),
}

impl MenuIndicatorItem {
    pub fn checkbox(value: impl Into<SharedString>) -> Self {
        MenuIndicatorItem::Checkbox(value.into())
    }

    pub fn radio(group: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        MenuIndicatorItem::Radio(group.into(), value.into())
    }

    fn key(&self) -> SharedString {
        match self {
            MenuIndicatorItem::Checkbox(value) => format!("checkbox:{value}").into(),
            MenuIndicatorItem::Radio(group, value) => format!("radio:{group}:{value}").into(),
        }
    }
}

// A radio item's key among the menu's items: under its group too, since
// radio groups may share values.
pub(crate) fn radio_key(group: &SharedString, value: &SharedString) -> SharedString {
    format!("{group}:{value}").into()
}

impl<P: Clone + PartialEq + Debug + 'static> MenuShared<P> {
    // An item's place and the focus handle its element tracks, made the
    // first time.
    pub(crate) fn region(&self, key: &SharedString, cx: &App) -> Region {
        self.regions
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| Region::with_focus_handle(cx.focus_handle()))
            .clone()
    }

    // Whether one of the menu's items has an icon, a submenu's trigger in it
    // too (`data-has-icons`). Asking renders the menu again as icons come
    // and go.
    pub(crate) fn has_icons(&self, instance: &ui_sdk_kernel::Instance) -> bool {
        ui_sdk_lists::has_icons(instance);
        let icons = |menu: &MenuShared<P>| -> Vec<SharedString> {
            menu.instance
                .bound_parts()
                .into_iter()
                .filter(|part| part.slot == "item-icon")
                .filter_map(|part| part.key)
                .collect()
        };
        let triggers = self.popup.bound_triggers();
        if icons(self).iter().any(|key| !triggers.contains(key)) {
            return true;
        }
        self.children().iter().any(|child| {
            let triggers = child.popup.bound_triggers();
            icons(child).iter().any(|key| triggers.contains(key))
        })
    }

    // Whether a group, or a radio group, is named by a label in the page.
    fn labelled(&self, group: &SharedString) -> bool {
        self.instance
            .bound_parts()
            .iter()
            .any(|part| part.slot == "group-label" && part.key.as_ref() == Some(group))
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Menu<P> {
    // Switches the shortcut names to the platform once a part renders
    // (`adoptPlatform`).
    fn adopt(entity: WeakEntity<Self>) -> impl FnOnce(&mut Window, &mut App) + 'static {
        move |_window, cx| {
            let Some(entity) = entity.upgrade() else {
                return;
            };
            if entity.read(cx).apple == is_apple(cx) {
                return;
            }
            entity.update(cx, |menu, cx| {
                if adopt_platform(&mut menu.apple, cx) {
                    cx.notify();
                }
            });
        }
    }

    // What the menubar sees of this menu, joined from its trigger.
    fn member(this: &Entity<Self>, key: &SharedString, cx: &App) -> MenubarMember {
        let shared = Rc::downgrade(&this.read(cx).shared);
        let opens = this.downgrade();
        let closes = this.downgrade();
        let open_key = key.clone();
        let disabled_key = key.clone();
        let open_shared = shared.clone();
        MenubarMember {
            is_open: Rc::new(move || open_shared.upgrade().is_some_and(|shared| shared.is_open())),
            disabled: Rc::new(move || {
                shared
                    .upgrade()
                    .is_some_and(|shared| shared.trigger_disabled(&disabled_key))
            }),
            open: Rc::new(move |open: MenubarOpen, window: &mut Window, cx: &mut App| {
                opens
                    .update(cx, |menu, cx| {
                        let payload = menu.popup.trigger_payload(&open_key);
                        menu.show(
                            Show {
                                focus: Some(open.focus),
                                reason: Some(open.reason),
                                event: open.event,
                                by: open.by,
                                trigger: Some(open_key.clone()),
                                payload: Some(payload),
                                ..Show::default()
                            },
                            window,
                            cx,
                        );
                    })
                    .ok();
            }),
            close: Rc::new(move |close: MenubarClose, window: &mut Window, cx: &mut App| {
                closes
                    .update(cx, |menu, cx| {
                        menu.hide(
                            Close {
                                reason: Some(close.reason),
                                event: close.event,
                                focus: Some(ReturnMode::from(close.focus)),
                                return_value: None,
                            },
                            window,
                            cx,
                        )
                    })
                    .unwrap_or(false)
            }),
        }
    }

    /// The `trigger` part (`trigger`): the button that opens the menu, with
    /// `key` for a menu several triggers share, each handing the content its
    /// `payload`. A submenu's trigger is an item of its parent menu, and a
    /// menubar's an item of the menubar; any other is a button. Mouse and pen
    /// open the menu on press, like native menus, and releasing over an item
    /// after dragging chooses it. Give its text with gpui's `text!(..)`, or
    /// name it with `aria_label`.
    pub fn trigger(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: impl Into<TriggerProps<P>>,
        cx: &App,
    ) -> Part<MenuTriggerState> {
        let key: SharedString = key.into();
        let props: TriggerProps<P> = props.into();
        let menu = this.read(cx);
        let shared = menu.shared.clone();
        shared.rtl.set(is_rtl(cx));
        let popup = menu.popup.clone();
        let nested = shared.is_nested();
        let trigger_props = PopupTriggerProps {
            disabled: props.disabled,
            focusable_when_disabled: props.focusable_when_disabled,
            payload: props.payload.clone(),
        };
        let (handle, region, _) = popup.bind_trigger(&key, &trigger_props, cx);
        let bar = if nested { None } else { shared.menubar.clone() };
        let bar_disabled = bar.as_ref().is_some_and(|bar| bar.shared.disabled.get());
        let disabled = props.disabled || bar_disabled;
        let active = if nested { None } else { shared.trigger_key() };
        let attrs = popup_trigger_attrs(
            popup.is_open(),
            popup.is_mounted(),
            active.as_deref(),
            menu.instance.element_id("content", None),
            Some(HasPopup::Menu),
            &key,
        );
        let highlighted = shared
            .parent()
            .is_some_and(|parent| parent.highlighted.borrow().as_ref() == Some(&shared.trigger_entry));
        let state = MenuTriggerState {
            open: attrs.open,
            disabled,
            highlighted,
            has_icon: (nested || bar.is_some()) && has_icon(&menu.instance, &key),
        };
        let mut aria = Aria::new().has_popup(attrs.has_popup);
        if let Some(controls) = attrs.controls {
            aria = aria.controls(controls);
        }
        let mut part = Part::new(&menu.instance, "trigger", Some(&key), state).aria_expanded(attrs.open);
        if nested {
            // A submenu's trigger is an item of its parent menu.
            if let Some(parent) = shared.parent() {
                parent.entries.register(
                    shared.trigger_entry.clone(),
                    Entry {
                        label: props.label.clone().unwrap_or_default(),
                        disabled: props.disabled,
                        item: EntryItem::Submenu(Rc::downgrade(&shared)),
                    },
                );
                parent
                    .regions
                    .borrow_mut()
                    .insert(shared.trigger_entry.clone(), region.clone());
                // In a sheet, a submenu drilled into shows only its trigger
                // above it.
                if let Some((_, trigger)) = parent.drilled()
                    && trigger.as_ref() != Some(&region)
                {
                    part = part.hidden();
                }
            }
            set_unfocusable(&handle, false);
            part = part
                .aria_role(Role::MenuItem)
                .focus_mode(FocusMode::Roving { tab_stop: false }, Some(&handle), false)
                .aria(aria.disabled(props.disabled));
        } else if let Some(bar) = &bar {
            // A menubar's triggers are its items: one tab stop between them,
            // and disabled ones stay focusable, as the APG menubar asks.
            let member = Self::member(this, &key, cx);
            let icon = {
                let instance = menu.instance.handle();
                let key = key.clone();
                Rc::new(move || {
                    instance
                        .bound_parts()
                        .iter()
                        .any(|part| part.slot == "item-icon" && part.key.as_ref() == Some(&key))
                }) as Rc<dyn Fn() -> bool>
            };
            let tab_stop = bar.shared.join(
                &shared.id,
                member,
                &handle,
                &region,
                props.label.clone().unwrap_or_default(),
                icon,
                bar_disabled || !props.disabled,
                cx,
            );
            set_unfocusable(&handle, false);
            part = part
                .aria_role(Role::MenuItem)
                .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
                .aria(aria.disabled(disabled));
        } else {
            menu.trigger_elements.borrow_mut().insert(key.clone(), props.element);
            let element = button_element_attrs(
                props.element,
                ButtonElementOptions {
                    disabled: props.disabled,
                    focusable: props.focusable_when_disabled,
                    ..ButtonElementOptions::default()
                },
            );
            set_unfocusable(&handle, element.disabled);
            let untabbable = element.disabled || element.tab_index.is_some_and(|index| index < 0);
            part = part
                .aria_role(button_role(props.element))
                .focus_mode(FocusMode::Native, Some(&handle), untabbable)
                .aria(aria.disabled(element.disabled || element.aria_disabled));
        }
        if !nested && let Some(shortcut) = props.shortcut.as_ref().filter(|shortcut| !shortcut.is_empty()) {
            part = part.aria_keyshortcuts(aria_keyshortcuts(shortcut, menu.apple));
        }
        let entity = this.downgrade();
        {
            let entity = entity.clone();
            let key = key.clone();
            part = part.map_el(move |el| {
                el.on_key_down(move |event, window, cx| {
                    if is_composing(window, cx) {
                        return;
                    }
                    if let Some(menu) = entity.upgrade() {
                        Self::trigger_keydown(&menu, &key, event, window, cx);
                    }
                })
            });
        }
        {
            let entity = entity.clone();
            part = part.on_mouse_move(move |event, window, cx| {
                let Some(menu) = entity.upgrade() else {
                    return;
                };
                let shared = menu.read(cx).shared.clone();
                if let Some(parent) = shared.parent().and_then(|parent| parent.entity.upgrade()) {
                    let key = shared.trigger_entry.clone();
                    parent.update(cx, |parent, cx| parent.hover_item(key, event, window, cx));
                }
                if let Some(bar) = &shared.menubar
                    && bar.shared.aim.active()
                {
                    let hover = HoverEvent::from_move(event, window);
                    bar.entity
                        .update(cx, |bar, cx| bar.hover(&shared.id, &hover, window, cx))
                        .ok();
                }
            });
        }
        {
            let entity = entity.clone();
            part = part.map_el(move |el| {
                el.on_hover(move |hovered, window, cx| {
                    let Some(menu) = entity.upgrade() else {
                        return;
                    };
                    let shared = menu.read(cx).shared.clone();
                    let hover = HoverEvent::from_window(window);
                    if *hovered {
                        if let Some(bar) = &shared.menubar {
                            bar.entity
                                .update(cx, |bar, cx| bar.hover(&shared.id, &hover, window, cx))
                                .ok();
                        }
                        return;
                    }
                    if hover.touch {
                        return;
                    }
                    let popup = shared.popup.layer().element();
                    if let Some(bar) = &shared.menubar {
                        bar.shared.leave(&shared.id, &hover, popup.clone());
                    }
                    let Some(parent) = shared.parent() else {
                        return;
                    };
                    match popup {
                        Some(popup) if shared.is_open() => {
                            parent.safe_area.start(hover.position, popup, SafeAreaAxis::X)
                        }
                        _ => parent.hover_intent.clear(),
                    }
                })
            });
        }
        {
            let entity = entity.clone();
            let key = key.clone();
            part = part.on_press(move |event, window, cx| {
                entity
                    .update(cx, |menu, cx| menu.trigger_press(&key, event, window, cx))
                    .ok();
            });
        }
        if !nested {
            // Mouse and pen open on press, like native menus. The click that
            // follows is ignored.
            {
                let entity = entity.clone();
                let key = key.clone();
                let region = region.clone();
                part = part.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    if window.last_input_was_touch() {
                        return;
                    }
                    let Some(menu) = entity.upgrade() else {
                        return;
                    };
                    let (disabled, payload) = {
                        let menu = menu.read(cx);
                        (menu.shared.trigger_disabled(&key), menu.popup.trigger_payload(&key))
                    };
                    if disabled {
                        return;
                    }
                    menu.update(cx, |menu, cx| {
                        menu.pointer_press(region.clone(), event, key.clone(), payload, window, cx)
                    });
                });
            }
            {
                let entity = entity.clone();
                part = part.capture_any_mouse_up(move |event, window, cx| {
                    let Some(menu) = entity.upgrade() else {
                        return;
                    };
                    if menu.read(cx).press.is_none() {
                        return;
                    }
                    menu.update(cx, |menu, cx| menu.press_release(event, window, cx));
                });
            }
            // A root trigger's shortcut opens the menu with its first item
            // focused, or closes it.
            let press = entity.clone();
            let shortcut_key = key.clone();
            part = bind_part_shortcut(
                part,
                props.shortcut.clone(),
                move |event, window, cx| {
                    press
                        .update(cx, |menu, cx| menu.trigger_shortcut(&shortcut_key, event, window, cx))
                        .ok();
                },
                ShortcutOptions::default().layer(region.clone()),
            );
            // Data menus run their items' shortcuts while closed.
            let items = if menu.item_shortcuts {
                shortcut_items(menu.items.as_ref())
            } else {
                None
            };
            part = part.child(ItemShortcuts {
                items,
                layer: region.clone(),
                menu: entity.clone(),
            });
        }
        region.measure(part.on_props(Self::adopt(entity)))
    }

    // Keys on a trigger (`trigger`'s `keydown`): a submenu's parent handles
    // them, a menubar moves between its triggers, and Up or Down opens the
    // menu at its last or first item. gpui presses the trigger for Enter and
    // Space.
    fn trigger_keydown(
        menu: &Entity<Self>,
        key: &SharedString,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (nested, in_bar, disabled, payload) = {
            let menu = menu.read(cx);
            (
                menu.shared.is_nested(),
                menu.shared.menubar.is_some(),
                menu.popup.trigger_disabled(key),
                menu.popup.trigger_payload(key),
            )
        };
        if nested {
            return;
        }
        if in_bar {
            Self::bar_keydown(menu, key, event, window, cx);
            return;
        }
        if disabled {
            return;
        }
        let focus = match Key::of(&event.keystroke) {
            Key::ArrowDown => Focus::First,
            Key::ArrowUp => Focus::Last,
            _ => return,
        };
        window.prevent_default();
        menu.update(cx, |menu, cx| {
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

    // A press on a trigger (`trigger`'s `press`).
    fn trigger_press(&mut self, key: &SharedString, event: InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.shared.is_nested() {
            if self.popup.trigger_disabled(key) {
                return;
            }
            if !self.popup.is_open() {
                // A click with no pointer behind it came from the keyboard or
                // a screen reader, which move focus in.
                let focus = match &event {
                    InputEvent::Click(gpui::ClickEvent::Keyboard(_)) | InputEvent::Accessibility => Focus::First,
                    _ => Focus::None,
                };
                self.show(
                    Show {
                        focus: Some(focus),
                        event: Some(event),
                        reason: Some(Reason::TriggerPress),
                        ..Show::default()
                    },
                    window,
                    cx,
                );
                return;
            }
            // In a sheet, the trigger stays above its submenu as the row that
            // goes back.
            if self.shared.presentation() != crate::menu_api::Presentation::Sheet {
                return;
            }
            self.hide(
                Close::new(Reason::TriggerPress, Some(event)).focus(ReturnMode::Leave),
                window,
                cx,
            );
            self.shared.focus_trigger_item(window, cx);
            return;
        }
        if self.shared.trigger_disabled(key) || self.consume_press_click() {
            return;
        }
        // Another trigger takes the open menu over, with its own payload.
        let payload = self.popup.trigger_payload(key);
        let active = self.shared.trigger_key();
        if self.popup.is_open() && active.as_ref().is_some_and(|active| active != key) {
            let control = self.popup.control().clone();
            control.take_over(
                self,
                key.clone(),
                payload,
                None,
                Reason::TriggerPress,
                Some(event),
                window,
                cx,
            );
            return;
        }
        // A click with no press before it came from the keyboard or a screen
        // reader.
        if self.popup.is_open() {
            self.hide(Close::new(Reason::TriggerPress, Some(event)), window, cx);
        } else {
            self.show(
                Show {
                    focus: Some(Focus::First),
                    event: Some(event),
                    reason: Some(Reason::TriggerPress),
                    trigger: Some(key.clone()),
                    payload: Some(payload),
                    ..Show::default()
                },
                window,
                cx,
            );
        }
    }

    // A root trigger's shortcut. A disabled menubar disables its triggers'
    // shortcuts too.
    fn trigger_shortcut(
        &mut self,
        key: &SharedString,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shared.trigger_disabled(key) {
            return;
        }
        let event = InputEvent::KeyDown(event.clone());
        if self.popup.is_open() {
            self.hide(Close::new(Reason::Shortcut, Some(event)), window, cx);
            return;
        }
        let payload = self.popup.trigger_payload(key);
        self.show(
            Show {
                focus: Some(Focus::First),
                event: Some(event),
                trigger: Some(key.clone()),
                payload: Some(payload),
                reason: Some(Reason::Shortcut),
                ..Show::default()
            },
            window,
            cx,
        );
    }

    /// The `content` part (`content`): the popup that holds the items, a
    /// `menu` named by the trigger that opened it, positioned against it.
    /// `None` while it's out of the window: it renders while mounted, and
    /// while closed with `keep_mounted`. Build the items inside it, after it,
    /// since the order they're built in is the order arrow keys move in. A
    /// submenu's content goes inside its parent's, where its trigger is.
    pub fn content(this: &Entity<Self>, props: ContentProps, cx: &App) -> Option<MenuContent<P>> {
        let menu = this.read(cx);
        let shared = menu.shared.clone();
        shared.rtl.set(is_rtl(cx));
        *shared.content.borrow_mut() = props.clone();
        let popup = menu.popup.clone();
        let popup_props = PopupContentProps {
            position: props.position(),
            final_focus: props.final_focus.clone(),
            keep_mounted: props.keep_mounted,
            timing: props.timing.clone(),
            sheet_backdrop: props.sheet_backdrop,
            ..PopupContentProps::default()
        };
        shared.entries.begin();
        let mounted = popup.is_mounted();
        if !mounted && !props.keep_mounted {
            popup.set_content_props(popup_props);
            return None;
        }
        let nested = shared.is_nested();
        let layer = popup.layer();
        let state = MenuContentState {
            open: popup.is_open(),
            nested,
            presentation: popup.presentation_value(),
            payload: shared.root().popup.payload(),
            presence: layer.presence().state(),
            placement: layer.positioner().and_then(|positioner| positioner.placement()),
            hidden: !mounted,
            sheet: popup.sheet().map(|sheet| sheet.part_state()),
            has_icons: shared.has_icons(&menu.instance),
        };
        // The content is named by the trigger that opened it, which a context
        // area or a controller's open without a trigger isn't. A submenu's
        // trigger always names it.
        let bound = popup.bound_triggers();
        let named = nested
            || match shared.trigger_key() {
                None => !bound.is_empty(),
                Some(key) => bound.contains(&key),
            };
        let mut aria = Aria::new().hidden(!mounted);
        if named && let Some(id) = popup.active_trigger_id(&menu.instance, &popup.state(menu)) {
            aria = aria.labelled_by(id);
        }
        let handle = popup.content_focus_handle();
        let scroll = shared.scroll.clone();
        let entity = this.downgrade();
        let mut part = Part::new(&menu.instance, "content", None, state)
            .aria_role(Role::Menu)
            .map_el(|el| el.track_focus(&handle.tab_stop(false)).track_scroll(&scroll))
            .aria(aria);
        {
            let entity = entity.clone();
            part = part.on_keys(CONTENT_KEYS, move |event, window, cx| {
                if matches!(Key::of(&event.keystroke), Key::Enter | Key::Space) {
                    return;
                }
                Self::content_key(&entity, event, window, cx);
            });
        }
        // Whose key it is is decided as the key arrives, before a submenu's
        // handler moves focus to its parent. Enter and Space are heard then
        // too, before the focused item, which gpui would press itself as the
        // key comes up.
        {
            let entity = entity.clone();
            let shared = shared.clone();
            part = part.map_el(move |el| {
                el.capture_key_down(move |event, window, cx| {
                    // Keys from a submenu reach its parent's content too. Only
                    // the owner handles them.
                    shared
                        .owns_key
                        .set(owns_event(&shared.popup.content_region(), window, cx));
                    if !matches!(Key::of(&event.keystroke), Key::Enter | Key::Space) || is_composing(window, cx) {
                        return;
                    }
                    if Self::content_key(&entity, event, window, cx) {
                        cx.stop_propagation();
                    }
                })
            });
        }
        {
            let entity = entity.clone();
            part = part.on_mouse_move(move |event, window, cx| {
                entity.update(cx, |menu, _cx| menu.track_pointer(event, window)).ok();
            });
        }
        {
            let entity = entity.clone();
            part = part.map_el(move |el| {
                el.on_hover(move |hovered, window, cx| {
                    let Some(menu) = entity.upgrade() else {
                        return;
                    };
                    let shared = menu.read(cx).shared.clone();
                    if *hovered {
                        if let Some(parent) = shared.parent() {
                            parent.safe_area.stop();
                        }
                        if let Some(bar) = &shared.menubar {
                            bar.shared.enter_menu();
                        }
                        return;
                    }
                    if window.last_input_was_touch() {
                        return;
                    }
                    // Moving into a submenu keeps the trigger highlighted.
                    if shared.open_child().is_some() {
                        return;
                    }
                    if shared.highlighted.borrow().is_some() {
                        shared.highlight(None, true, false, window, cx);
                    }
                })
            });
        }
        {
            let entity = entity.clone();
            part = part.on_props(move |window, cx| {
                let Some(menu) = entity.upgrade() else {
                    return;
                };
                menu.read(cx).shared.rem_size.set(window.rem_size());
                menu.update(cx, |menu, cx| menu.watch_entries(window, cx));
            });
        }
        let content = bind_popup_content(this, part, popup_props, cx);
        Some(MenuContent { inner: content, shared })
    }

    // Keys on the content, from its handlers, outside the menu's update.
    // Returns whether the menu took the key.
    fn content_key(entity: &WeakEntity<Self>, event: &KeyDownEvent, window: &mut Window, cx: &mut App) -> bool {
        let Some(menu) = entity.upgrade() else {
            return false;
        };
        let shared = menu.read(cx).shared.clone();
        if !shared.owns_key.get() {
            return false;
        }
        let follow = menu.update(cx, |menu, cx| menu.on_key_down(event, window, cx));
        if let KeyFollowUp::BarArrow(root, by) = follow
            && bar_arrow(&root, by, event, window, cx)
        {
            window.prevent_default();
        }
        true
    }

    // Focus landing on an item highlights it (the content's `focusin`).
    fn watch_entries(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.shared.entries.keys();
        self.item_focus.retain(|key, _| keys.contains(key));
        for key in keys {
            if self.item_focus.contains_key(&key) {
                continue;
            }
            let Some(handle) = self.shared.regions.borrow().get(&key).and_then(Region::focus_handle) else {
                continue;
            };
            let landed = key.clone();
            let subscription = focus_events::on_focus(&handle, window, cx, move |menu: &mut Self, _window, cx| {
                if menu.shared.highlighted.borrow().as_ref() == Some(&landed) || !menu.shared.entries.contains(&landed)
                {
                    return;
                }
                *menu.shared.highlighted.borrow_mut() = Some(landed.clone());
                cx.notify();
            });
            self.item_focus.insert(key, subscription);
        }
    }

    /// The `viewport` part (`viewport`): the window-sized element a root
    /// menu presented as a sheet sits in. Put the content inside it to style
    /// it; without one, the sheet hosts itself. `None` unless it presents as
    /// a sheet.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> Option<PopupViewport<Self>> {
        if this.read(cx).shared.is_nested() {
            return None;
        }
        popup_viewport(this, cx)
    }

    // An item's pointer moves highlight it (`pointermove`).
    fn hovers<S: 'static>(this: &Entity<Self>, part: Part<S>, key: SharedString) -> Part<S> {
        let entity = this.downgrade();
        part.on_mouse_move(move |event, window, cx| {
            entity
                .update(cx, |menu, cx| menu.hover_item(key.clone(), event, window, cx))
                .ok();
        })
    }

    /// An `item` part (`item`): an action, with `key` among the menu's
    /// items. Choosing it runs its `on_select` and the menu's `on_action`
    /// with its value, and closes the menu, unless `close_on_select` is
    /// false. Give its text with gpui's `text!(..)`, and the text typeahead
    /// matches in its `label`.
    pub fn item(this: &Entity<Self>, key: impl Into<SharedString>, props: ItemProps, cx: &App) -> Part<MenuItemState> {
        let key: SharedString = key.into();
        let menu = this.read(cx);
        let shared = &menu.shared;
        if dev::DEV && props.label.is_none() {
            dev::warn_once(
                format!("{}-item-{key}", shared.id),
                "An item has no label, so typeahead can't find it. Pass one.",
                Some(dev::Origin::new("menu", &shared.id)),
            );
        }
        let pending = menu.pending.contains(&key);
        let disabled = props.disabled || pending;
        let region = shared.region(&key, cx);
        let handle = region.focus_handle().expect("an item's region tracks its focus handle");
        shared.entries.register(
            key.clone(),
            Entry {
                label: props.label.clone().unwrap_or_default(),
                disabled,
                item: EntryItem::Item(props.clone()),
            },
        );
        let state = MenuItemState {
            highlighted: shared.highlighted.borrow().as_ref() == Some(&key),
            disabled: props.disabled,
            pending,
            error: menu
                .error
                .as_ref()
                .filter(|(failed, _)| *failed == key)
                .map(|(_, reason)| reason.clone()),
            destructive: props.destructive,
            has_icon: has_icon(&menu.instance, &key),
        };
        let mut part = Part::new(&menu.instance, "item", Some(&key), state)
            .aria_role(Role::MenuItem)
            .focus_mode(FocusMode::Roving { tab_stop: false }, Some(&handle), false)
            .aria(Aria::new().disabled(disabled).busy(pending));
        if let Some(shortcut) = props.shortcut.as_ref().filter(|shortcut| !shortcut.is_empty()) {
            part = part.aria_keyshortcuts(aria_keyshortcuts(shortcut, menu.apple));
        }
        if shared.drilled().is_some() {
            part = part.hidden();
        }
        let part = Self::hovers(this, part, key.clone());
        let entity = this.downgrade();
        let part = part.on_press(move |event, window, cx| {
            entity
                .update(cx, |menu, cx| {
                    menu.activate_item(&key, &props, Some(event), None, window, cx)
                })
                .ok();
        });
        region.measure(part)
    }

    /// A `group` part (`group`): items that belong together, with `key`,
    /// named by its `group_label` while one is in the window.
    pub fn group(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part<()> {
        let key: SharedString = key.into();
        let menu = this.read(cx);
        let mut aria = Aria::new();
        // A group is named by its label only while one is in the window, so
        // an unlabeled group points at no missing part.
        key_while_bound(&menu.instance, "group-label");
        if menu.shared.labelled(&key) {
            aria = aria.labelled_by(menu.instance.element_id("group-label", Some(&key)));
        }
        Part::new(&menu.instance, "group", Some(&key), ())
            .aria_role(Role::Group)
            .aria(aria)
    }

    /// A `group-label` part (`group-label`): the heading of the group, or
    /// radio group, `group` names. Name it with `aria_label`, which names the
    /// group.
    pub fn group_label(this: &Entity<Self>, group: impl Into<SharedString>, cx: &App) -> Part<()> {
        let group: SharedString = group.into();
        let menu = this.read(cx);
        let part =
            Part::new(&menu.instance, "group-label", Some(&group), ()).map_el(|el| with_role(el, AccessRole::Label));
        if menu.shared.drilled().is_some() {
            return part.hidden();
        }
        part
    }

    /// A `separator` part (`separator`): a line between items or groups,
    /// with `key`. It's never a focus stop.
    pub fn separator(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part<()> {
        let key: SharedString = key.into();
        let menu = this.read(cx);
        let part = Part::new(&menu.instance, "separator", Some(&key), ())
            .aria_role(Role::Separator)
            .aria_orientation(AccessOrientation::Horizontal);
        if menu.shared.drilled().is_some() {
            return part.hidden();
        }
        part
    }

    /// A `shortcut` part (`shortcut`): the keys of an item's shortcut, like
    /// `⌘S`, inside the item, with `key` for one per item. Without keys of
    /// its own, it shows the `shortcut` of the item it sits in. It's hidden
    /// from assistive technology, which hears the item's
    /// `aria-keyshortcuts`.
    pub fn shortcut(this: &Entity<Self>, key: Option<SharedString>, props: ShortcutProps, cx: &App) -> ShortcutPart {
        let menu = this.read(cx);
        let instance = menu.instance.handle();
        let entries = menu.shared.entries.clone();
        shortcut_slot(&menu.instance, key.as_deref(), props, None, menu.apple)
            .inherit_from(move |el, _cx| {
                shortcut_of(&instance, "item", el, false, |key| {
                    let entry = key.and_then(|key| entries.info(key))?;
                    match entry.item {
                        EntryItem::Item(props) => props.shortcut,
                        _ => None,
                    }
                })
            })
            .map_part(|part| part.on_props(Self::adopt(this.downgrade())))
    }

    /// A `context-trigger` part (`context-trigger`): an area that opens the
    /// menu at the pointer on a right-click, a Control-click on a Mac or a
    /// long press, and at itself on Shift+F10 or the context menu key, from
    /// anything inside it that has focus. A right-click elsewhere in the
    /// area moves the open menu there. Several areas can share one menu,
    /// each with its `key` and `payload`.
    pub fn context_trigger(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: ContextTriggerProps<P>,
        cx: &App,
    ) -> Part<crate::menu::MenuContextTriggerState> {
        let key: SharedString = key.into();
        let menu = this.read(cx);
        let shared = menu.shared.clone();
        let region = {
            let mut areas = shared.context_triggers.borrow_mut();
            let entry = areas.entry(key.clone()).or_insert_with(|| ContextEntry {
                region: Region::new(),
                props: props.clone(),
            });
            entry.props = props.clone();
            entry.region.clone()
        };
        if shared.trigger_key().is_none() {
            menu.popup.set_trigger_key(Some(key.clone()));
        }
        let state = crate::menu::MenuContextTriggerState {
            open: menu.popup.is_open() && shared.trigger_key().as_ref() == Some(&key),
            disabled: props.disabled,
        };
        let entity = this.downgrade();
        let mut part = Part::new(&menu.instance, "context-trigger", Some(&key), state);
        {
            let entity = entity.clone();
            let key = key.clone();
            let payload = props.payload.clone();
            let disabled = props.disabled;
            part = part.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                if disabled {
                    return;
                }
                let by = if window.last_input_was_touch() {
                    PopupOpener::Touch
                } else {
                    PopupOpener::Pointer
                };
                let at = Anchor::point(event.position);
                let down = InputEvent::MouseDown(event.clone());
                entity
                    .update(cx, |menu, cx| {
                        menu.context_open(at, Some(down), by, key.clone(), payload.clone(), window, cx)
                    })
                    .ok();
            });
        }
        // A Control-click on a Mac opens it too.
        {
            let entity = entity.clone();
            let key = key.clone();
            let payload = props.payload.clone();
            let disabled = props.disabled;
            part = part.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                if disabled || !event.modifiers.control || !is_apple(cx) {
                    return;
                }
                let at = Anchor::point(event.position);
                let down = InputEvent::MouseDown(event.clone());
                entity
                    .update(cx, |menu, cx| {
                        menu.context_open(
                            at,
                            Some(down),
                            PopupOpener::Pointer,
                            key.clone(),
                            payload.clone(),
                            window,
                            cx,
                        )
                    })
                    .ok();
            });
        }
        // Shift+F10 and the context menu key open it at the area.
        {
            let entity = entity.clone();
            let key = key.clone();
            let payload = props.payload.clone();
            let disabled = props.disabled;
            let at = region.clone();
            part = part.on_key_down(move |event, window, cx| {
                if disabled || is_composing(window, cx) {
                    return;
                }
                let pressed = Key::of(&event.keystroke);
                let menu_key = pressed == Key::Other("menu".into());
                let shift_f10 = pressed == Key::Function(10) && event.keystroke.modifiers.shift;
                if !menu_key && !shift_f10 {
                    return;
                }
                window.prevent_default();
                let down = InputEvent::KeyDown(event.clone());
                entity
                    .update(cx, |menu, cx| {
                        menu.context_open(
                            Anchor::element(at.clone()),
                            Some(down),
                            PopupOpener::Keyboard,
                            key.clone(),
                            payload.clone(),
                            window,
                            cx,
                        )
                    })
                    .ok();
            });
        }
        // iOS gives no secondary press, so a long press opens it.
        let hold = {
            let mut holds = menu.holds.borrow_mut();
            holds
                .entry(key.clone())
                .or_insert_with(|| Hold::new(entity.clone(), None, BindHoldOptions::new()))
                .clone()
        };
        let disabled_key = key.clone();
        let hold_key = key.clone();
        let payload = props.payload.clone();
        hold.set_options(
            BindHoldOptions::new()
                .keys([])
                .disabled(move |menu: &Self| {
                    menu.shared
                        .context_triggers
                        .borrow()
                        .get(&disabled_key)
                        .is_some_and(|entry| entry.props.disabled)
                })
                .on_press(|_menu, details, _window, _cx| details.input == HoldInput::Touch)
                .on_hold(move |menu: &mut Self, details, _count, window, cx| {
                    if let Some(position) = details.event.position() {
                        menu.context_open(
                            Anchor::point(position),
                            Some(details.event.input_event()),
                            PopupOpener::Touch,
                            hold_key.clone(),
                            payload.clone(),
                            window,
                            cx,
                        );
                    }
                    false
                }),
        );
        let part = hold.bind(part);
        region.measure(part)
    }

    /// A `checkbox-item` part (`checkbox-item`): an item that checks and
    /// unchecks an option, by its `value`, which the menu's
    /// `on_checked_change` names. It stays open by default.
    pub fn checkbox_item(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: CheckboxProps,
        cx: &App,
    ) -> Part<MenuCheckboxItemState> {
        let value: SharedString = value.into();
        let menu = this.read(cx);
        let shared = &menu.shared;
        let checked = menu.checkbox_value(&value, &props);
        let region = shared.region(&value, cx);
        let handle = region.focus_handle().expect("an item's region tracks its focus handle");
        shared.entries.register(
            value.clone(),
            Entry {
                label: props.label.clone().unwrap_or_default(),
                disabled: props.disabled,
                item: EntryItem::Checkbox(value.clone(), props.clone()),
            },
        );
        let state = MenuCheckboxItemState {
            highlighted: shared.highlighted.borrow().as_ref() == Some(&value),
            disabled: props.disabled,
            checked,
            has_icon: has_icon(&menu.instance, &value),
        };
        let mut part = Part::new(&menu.instance, "checkbox-item", Some(&value), state)
            .aria_role(Role::MenuItemCheckbox)
            .focus_mode(FocusMode::Roving { tab_stop: false }, Some(&handle), false)
            .aria(Aria::new().checked(checked).disabled(props.disabled));
        if shared.drilled().is_some() {
            part = part.hidden();
        }
        let part = Self::hovers(this, part, value.clone());
        let entity = this.downgrade();
        let part = part.on_press(move |event, window, cx| {
            entity
                .update(cx, |menu, cx| {
                    menu.toggle_checkbox(&value, &props, Some(event), window, cx)
                })
                .ok();
        });
        region.measure(part)
    }

    /// A `radio-group` part (`radio-group`): radio items that share one
    /// value, by its `name`, which the menu's `on_value_change` names.
    /// Build it before its items.
    pub fn radio_group(
        this: &Entity<Self>,
        name: impl Into<SharedString>,
        props: RadioGroupProps,
        cx: &App,
    ) -> Part<MenuRadioGroupState> {
        let name: SharedString = name.into();
        let menu = this.read(cx);
        menu.radio_groups.borrow_mut().insert(name.clone(), props);
        let state = MenuRadioGroupState {
            value: menu.radio_value(&name),
        };
        key_while_bound(&menu.instance, "group-label");
        let mut aria = Aria::new();
        if menu.shared.labelled(&name) {
            aria = aria.labelled_by(menu.instance.element_id("group-label", Some(&name)));
        }
        Part::new(&menu.instance, "radio-group", Some(&name), state)
            .aria_role(Role::Group)
            .aria(aria)
    }

    /// A `radio-item` part (`radio-item`): an item of the radio group
    /// `group` that selects its `value` when chosen. It stays open by
    /// default.
    pub fn radio_item(
        this: &Entity<Self>,
        group: impl Into<SharedString>,
        value: impl Into<SharedString>,
        props: RadioProps,
        cx: &App,
    ) -> Part<MenuRadioItemState> {
        let group: SharedString = group.into();
        let value: SharedString = value.into();
        let menu = this.read(cx);
        let shared = &menu.shared;
        let key = radio_key(&group, &value);
        let checked = menu.radio_value(&group).as_ref() == Some(&value);
        let region = shared.region(&key, cx);
        let handle = region.focus_handle().expect("an item's region tracks its focus handle");
        shared.entries.register(
            key.clone(),
            Entry {
                label: props.label.clone().unwrap_or_default(),
                disabled: props.disabled,
                item: EntryItem::Radio {
                    group: group.clone(),
                    value: value.clone(),
                    props: props.clone(),
                },
            },
        );
        let state = MenuRadioItemState {
            highlighted: shared.highlighted.borrow().as_ref() == Some(&key),
            disabled: props.disabled,
            checked,
            has_icon: has_icon(&menu.instance, &key),
        };
        let mut part = Part::new(&menu.instance, "radio-item", Some(&key), state)
            .aria_role(Role::MenuItemRadio)
            .focus_mode(FocusMode::Roving { tab_stop: false }, Some(&handle), false)
            .aria(Aria::new().checked(checked).disabled(props.disabled));
        if shared.drilled().is_some() {
            part = part.hidden();
        }
        let part = Self::hovers(this, part, key);
        let entity = this.downgrade();
        let part = part.on_press(move |event, window, cx| {
            entity
                .update(cx, |menu, cx| {
                    menu.select_radio(&group, &value, &props, Some(event), window, cx)
                })
                .ok();
        });
        region.measure(part)
    }

    /// An `item-icon` part (`item-icon`): an icon inside an item, a
    /// submenu's trigger or a menubar's trigger, by that item's key: an
    /// item's own, a checkbox item's value, `"group:value"` for a radio item,
    /// or the trigger's. It's left out for assistive technology and the text
    /// typeahead matches, and `mirror` flips it in right-to-left text. An
    /// item with only an icon still needs an `aria_label`.
    pub fn item_icon(
        this: &Entity<Self>,
        item: Option<SharedString>,
        props: MenuItemIconProps,
        cx: &App,
    ) -> Part<MenuItemIconState> {
        let menu = this.read(cx);
        item_icon_part(&menu.instance, item.as_deref(), &props, cx)
    }

    /// An `item-indicator` part (`item-indicator`): the check or dot inside
    /// the checkbox or radio `item`. `None` while its item is unchecked,
    /// unless `keep_mounted` is set. It's hidden from assistive technology,
    /// and animates in as its item becomes checked. Build it inside its item.
    pub fn item_indicator(
        this: &Entity<Self>,
        item: MenuIndicatorItem,
        props: IndicatorProps,
        cx: &App,
    ) -> Option<Part<MenuItemIndicatorState>> {
        let menu = this.read(cx);
        let (checked, disabled) = menu.indicator_state(&item);
        if !props.keep_mounted && !checked {
            return None;
        }
        let key = item.key();
        let presence = menu
            .indicators
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| Presence::new(true))
            .clone();
        if let Some(timing) = &props.timing {
            presence.set_timing(timing.clone());
        }
        // Animates in when its item becomes checked: as it comes into the
        // window checked.
        let entering = checked && !key_while_bound(&menu.instance, "item-indicator").contains(&key);
        let state = MenuItemIndicatorState {
            checked,
            disabled,
            presence: presence.state(),
        };
        let mut part = Part::new(&menu.instance, "item-indicator", Some(&key), state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true));
        if entering {
            let entity = this.downgrade();
            part = part.on_props(move |window, cx| {
                entity.update(cx, |_menu, cx| presence.animate_in(window, cx)).ok();
            });
        }
        Some(part)
    }

    // Whether an indicator's item is checked, and disabled, from the item's
    // latest props (`indicatorState`).
    fn indicator_state(&self, item: &MenuIndicatorItem) -> (bool, bool) {
        match item {
            MenuIndicatorItem::Checkbox(value) => match self.shared.entry_for(value).map(|entry| entry.item) {
                Some(EntryItem::Checkbox(_, props)) => (self.checkbox_value(value, &props), props.disabled),
                _ => (false, false),
            },
            MenuIndicatorItem::Radio(group, value) => {
                match self.shared.entry_for(&radio_key(group, value)).map(|entry| entry.item) {
                    Some(EntryItem::Radio { props, .. }) => {
                        (self.radio_value(group).as_ref() == Some(value), props.disabled)
                    }
                    _ => (false, false),
                }
            }
        }
    }

    // Runs the item with `key`, as Enter or a release over it does
    // (`entry.activate`).
    pub(crate) fn activate(
        &mut self,
        key: &SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.shared.entry_for(key) else {
            return;
        };
        match entry.item {
            EntryItem::Item(props) => self.activate_item(key, &props, event, None, window, cx),
            EntryItem::Checkbox(value, props) => self.toggle_checkbox(&value, &props, event, window, cx),
            EntryItem::Radio { group, value, props } => self.select_radio(&group, &value, &props, event, window, cx),
            EntryItem::Submenu(child) => {
                if let Some(child) = child.upgrade() {
                    open_submenu(&child, Focus::First, Reason::TriggerPress, event, window, cx);
                }
            }
        }
    }

    // Chooses a plain item (`activateItem`).
    pub(crate) fn activate_item(
        &mut self,
        key: &SharedString,
        props: &ItemProps,
        event: Option<InputEvent>,
        selection: Option<SelectEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if props.disabled || self.is_pending(key) {
            return;
        }
        self.clear_error(key, cx);
        let selection = selection.unwrap_or_else(|| SelectEvent::new(event.clone()));
        let mut results = Vec::new();
        if let Some(on_select) = props.on_select.clone() {
            results.push(on_select(&selection, window, cx));
        }
        // Keeps the menu open, whether the item was clicked, pressed or
        // chosen with a key.
        let prevented = selection.default_prevented();
        if !prevented && let Some(value) = &props.value {
            results.push(self.run_action(value, event.clone(), window, cx));
        }
        let mut closes = !prevented && props.close_on_select;
        let mut work: Vec<Work> = Vec::new();
        for result in results {
            match result {
                ActionOutcome::Done => {}
                ActionOutcome::Canceled => closes = false,
                ActionOutcome::Pending(pending) => work.push(pending),
                ActionOutcome::Failed(reason) => work.push(Box::pin(async move { Err(reason) })),
            }
        }
        if !work.is_empty() {
            self.wait(
                key.clone(),
                work,
                move |menu, window, cx| {
                    if closes {
                        menu.action(None, event, window, cx);
                    }
                },
                window,
                cx,
            );
        } else if closes {
            self.action(None, event, window, cx);
        }
    }

    // Toggles a checkbox item (`toggleCheckbox`).
    fn toggle_checkbox(
        &mut self,
        value: &SharedString,
        props: &CheckboxProps,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if props.disabled {
            return;
        }
        if self.set_checkbox(value, props, event.clone(), Reason::ItemPress, window, cx) && props.close_on_select {
            self.action(None, event, window, cx);
        }
    }

    // Chooses a radio item (`selectRadio`). Choosing the selected item
    // changes nothing, but still closes a menu that closes on select.
    fn select_radio(
        &mut self,
        group: &SharedString,
        value: &SharedString,
        props: &RadioProps,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if props.disabled {
            return;
        }
        let chosen = self.radio_value(group).as_ref() == Some(value)
            || self.set_radio(group, value, event.clone(), Reason::ItemPress, window, cx);
        if chosen && props.close_on_select {
            self.action(None, event, window, cx);
        }
    }
}

// Left and Right inside a menubar's menu, on an item without a submenu. The
// menubar decides, so a menu outside one carries none of this (`barArrow`).
// It runs outside every update, since moving to another menu closes this
// one.
fn bar_arrow<P: Clone + PartialEq + Debug + 'static>(
    root: &Rc<MenuShared<P>>,
    by: isize,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(bar) = root.menubar.clone() else {
        return false;
    };
    let key_event = InputEvent::KeyDown(event.clone());
    match bar.shared.menu_arrow(&root.id, by, Some(key_event.clone()), window, cx) {
        Some(BarArrow::Close) => {
            root.entity
                .update(cx, |menu, cx| {
                    menu.hide(Close::new(Reason::EscapeKey, Some(key_event)), window, cx)
                })
                .ok();
            true
        }
        Some(BarArrow::Moved) => true,
        None => false,
    }
}

/// The `content` part as the app styles it and fills it with items: a part
/// that renders above the window, beside its trigger. It's a part like any
/// other, with the content's state.
pub struct MenuContent<P: Clone + PartialEq + Debug + 'static> {
    inner: PopupContent<Menu<P>, MenuContentState<P>>,
    shared: Rc<MenuShared<P>>,
}

impl<P: Clone + PartialEq + Debug + 'static> MenuContent<P> {
    /// The content's state.
    pub fn state(&self) -> &MenuContentState<P> {
        self.inner.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &MenuContentState<P>) -> Stateful<Div>) -> Self {
        self.inner = self.inner.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters, as on any part.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.inner = self.inner.aria(aria);
        self
    }

    /// Changes the part underneath with any of [`Part`]'s methods.
    pub fn map_part(mut self, map: impl FnOnce(Part<MenuContentState<P>>) -> Part<MenuContentState<P>>) -> Self {
        self.inner = self.inner.map_part(map);
        self
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Styled for MenuContent<P> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.style()
    }
}

impl<P: Clone + PartialEq + Debug + 'static> InteractiveElement for MenuContent<P> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inner.interactivity()
    }
}

impl<P: Clone + PartialEq + Debug + 'static> StatefulInteractiveElement for MenuContent<P> {}

impl<P: Clone + PartialEq + Debug + 'static> ParentElement for MenuContent<P> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.inner.extend(elements)
    }
}

impl<P: Clone + PartialEq + Debug + 'static> IntoElement for MenuContent<P> {
    type Element = PopupContentElement<Menu<P>, MenuContentState<P>>;

    // Distance from the popup's top edge to its content, its padding and
    // border, so a submenu's first item lines up with the item that opened
    // it (`firstItemOffset`).
    fn into_element(mut self) -> Self::Element {
        let rem = self.shared.rem_size.get();
        let style = self.inner.style();
        let padding = match style.padding.top {
            Some(DefiniteLength::Absolute(length)) => length.to_pixels(rem),
            _ => px(0.),
        };
        let border = style
            .border_widths
            .top
            .map_or(px(0.), |length: AbsoluteLength| length.to_pixels(rem));
        let inset: Pixels = padding + border;
        self.shared.inset.set(inset);
        self.inner.into_element()
    }
}
