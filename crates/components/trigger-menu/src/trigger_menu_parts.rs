//! Port of `components/trigger-menu/trigger-menu-parts.ts`: the trigger
//! menu's parts and the ARIA they carry.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Deferred, Div, Element, Entity, Focusable, InteractiveElement, IntoElement, MouseMoveEvent,
    ParentElement, SharedString, Stateful, StatefulInteractiveElement, StyleRefinement, Styled, deferred,
};
use ui_sdk_focus::press_keeps_focus;
use ui_sdk_kernel::part::{FocusMode, PartElement, with_aria};
use ui_sdk_kernel::roles::{Need, Role};
use ui_sdk_kernel::{Aria, InputEvent, Live, Part};
use ui_sdk_lists::{
    AsyncOptionsStatus, HighlightOptions, ItemIconProps, ItemIconState, ItemMeta, group_label_part, group_labelled_by,
    has_icon, has_icons, item_icon_part,
};
use ui_sdk_native::TextField;
use ui_sdk_platform::{Anchor, LAYER_PRIORITY, positioned};
use ui_sdk_popups::{PopupContent, PopupContentState, bind_popup_content};
use ui_sdk_text::TextRect;

use crate::trigger_menu::{
    TriggerMenuContentProps, TriggerMenuContentState, TriggerMenuEmptyState, TriggerMenuHintState,
    TriggerMenuInputState, TriggerMenuItemProps, TriggerMenuItemState, TriggerMenuStatusState,
};
use crate::trigger_menu_api::{TriggerMenu, TriggerMenuField};
use crate::trigger_menu_keyboard::INPUT_KEYS;

/// The props an item's icon takes.
pub type TriggerMenuItemIconProps = ItemIconProps;

/// What an item's icon styles from.
pub type TriggerMenuItemIconState = ItemIconState;

/// What a group styles from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TriggerMenuGroupState {
    /// Whether none of its items match, so it lays out as `display: none`.
    pub hidden: bool,
}

/// What a separator styles from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TriggerMenuSeparatorState {
    /// Whether a query filters the items it divides, so it lays out as
    /// `display: none`.
    pub hidden: bool,
}

const LIST_KEYS: &str = "The editor handles them, and keeps focus while they move the highlight.";

impl TriggerMenu {
    // What the content and the list style from.
    fn content_state(&self) -> TriggerMenuContentState {
        TriggerMenuContentState {
            open: self.popup.is_open(),
            trigger: self.trigger.clone(),
            query: self.query.clone(),
            argument: self.argument.clone(),
            status: self.loading_status(),
            items: self
                .listbox
                .visible_data(self)
                .map(|visible| visible.list)
                .unwrap_or_default(),
            has_icons: has_icons(&self.instance),
            popup: None,
        }
    }

    /// The `input` part: the native field the menu suggests into, its
    /// `<textarea>`, or a combobox when the field is single-line, its
    /// `<input>`. The field is the menu's editor while the part is in the
    /// page. Style it as the field, and name it with `aria_label`.
    pub fn input(this: &Entity<Self>, field: &Entity<TextField>, cx: &App) -> Part<TriggerMenuInputState> {
        let menu = this.read(cx);
        let text_field = field.read(cx);
        let kind = if text_field.is_multi_line() {
            TriggerMenuField::Multiline
        } else {
            TriggerMenuField::Input
        };
        let open = menu.popup.is_open();
        let state = TriggerMenuInputState {
            open,
            trigger: menu.trigger.clone(),
        };
        let disabled = text_field.is_disabled();
        let own = Aria::new()
            .disabled(disabled)
            .read_only(text_field.is_read_only())
            .required(text_field.is_required());
        let focus = text_field.focus_handle(cx);
        let expanded_now = text_field.is_expanded();
        let (aria, expanded) = menu.field_aria(kind);
        let role = match kind {
            TriggerMenuField::Input => Role::Combobox,
            TriggerMenuField::Multiline => Role::Textbox,
        };
        let (el, _) = TextField::root(field, cx).into_element_parts();
        let keys = this.downgrade();
        let mut part = Part::new(&menu.instance, "input", None, state)
            .map_el(move |_| el)
            .aria_role(role)
            .focus_mode(FocusMode::Native, Some(&focus), disabled)
            .aria(own)
            .aria(aria)
            .on_keys(INPUT_KEYS, move |event, window, cx| {
                keys.update(cx, |menu, cx| menu.on_key_down(event, window, cx)).ok();
            });
        if let Some(expanded) = expanded {
            part = part.aria_expanded(expanded);
        }
        // The field becomes the menu's own editor once it's in the page.
        if !menu.owns_field(field) || menu.field != kind {
            let entity = this.downgrade();
            let field = field.clone();
            part = part.on_props(move |window, cx| {
                entity
                    .update(cx, |menu, cx| {
                        if !menu.owns_field(&field) || menu.field != kind {
                            menu.set_own_editor(Some(&field), kind, window, cx);
                        }
                    })
                    .ok();
            });
        }
        // Its arrow keys belong to the open menu, which input history reads
        // (`data-open`).
        if expanded_now != open {
            let field = field.downgrade();
            part = part.on_props(move |_, cx| {
                field.update(cx, |field, cx| field.set_expanded(open, cx)).ok();
            });
        }
        menu.editor_region.measure(part)
    }

    /// An editor of the app's own, for the `editor` option: its element, with
    /// the ARIA the menu writes on the field, and measured, so a press in it
    /// isn't outside the menu (`applyAttrs` on the editor's element).
    pub fn editor_element<E>(this: &Entity<Self>, el: E, cx: &App) -> PartElement<E>
    where
        E: Element + ParentElement + Styled,
    {
        let menu = this.read(cx);
        *menu.editor_id.borrow_mut() = el.id();
        let (aria, _) = menu.field_aria(TriggerMenuField::Multiline);
        with_aria(menu.editor_region.measure(el), aria)
    }

    /// The `content` part: the popup that holds the suggestions, anchored at
    /// the caret, above the window. `None` while it's out of the page. A
    /// `list` inside it is the listbox; without one, the content is. To
    /// scroll a long list, give it `overflow_y_scroll()`: the highlight
    /// scrolls into view in it.
    pub fn content(
        this: &Entity<Self>,
        props: TriggerMenuContentProps,
        cx: &App,
    ) -> Option<PopupContent<Self, TriggerMenuContentState>> {
        let menu = this.read(cx);
        *menu.content_props.borrow_mut() = props.clone();
        let popup = menu.popup.clone();
        let popup_state = popup.state(menu);
        if !popup_state.mounted && !props.keep_mounted {
            // Out of the page: when it renders again, its items settle again.
            menu.settled.set(false);
            popup.set_content_props(props.popup());
            return None;
        }
        let listed = menu.listed();
        let layer = popup.layer();
        let mut state = menu.content_state();
        state.popup = Some(PopupContentState {
            open: popup_state.open,
            modal: popup_state.modal,
            presentation: popup_state.presentation,
            payload: None,
            presence: layer.presence().state(),
            placement: layer.positioner().and_then(|positioner| positioner.placement()),
            hidden: !popup_state.mounted,
            sheet: None,
        });
        let busy = !listed && state.status == AsyncOptionsStatus::Loading;
        let mut part = Part::new(&menu.instance, "content", None, state);
        // A list inside it is the listbox instead.
        part = if listed {
            part.aria_role(Role::Presentation)
        } else {
            part.aria_role(Role::Listbox).unneeded(Need::Keys, LIST_KEYS)
        };
        let handle = popup.content_focus_handle();
        let scroll = menu.scroll.clone();
        // Pressing the list keeps focus, the caret and the on-screen keyboard
        // in the editor.
        part = part.map_el(move |el| press_keeps_focus(el.track_focus(&handle.tab_stop(false)).track_scroll(&scroll)));
        let mut aria = Aria::new().hidden(!popup_state.mounted);
        if !listed {
            aria = aria.merge(menu.list_label()).busy(busy);
        }
        part = part.aria(aria);
        // Items render with it. Once that frame is drawn, an empty list is
        // really empty.
        if !menu.settled.get() {
            let entity = this.downgrade();
            part = part.on_props(move |window, cx| {
                entity.update(cx, |menu, cx| menu.content_settled(window, cx)).ok();
            });
        }
        menu.listbox.begin(cx);
        Some(bind_popup_content(this, part, props.popup(), cx))
    }

    /// The `list` part: the listbox, inside the content.
    pub fn list(this: &Entity<Self>, cx: &App) -> Part<TriggerMenuContentState> {
        let menu = this.read(cx);
        let state = menu.content_state();
        let busy = state.status == AsyncOptionsStatus::Loading;
        menu.listbox.begin(cx);
        Part::new(&menu.instance, "list", None, state)
            .aria_role(Role::Listbox)
            .unneeded(Need::Keys, LIST_KEYS)
            .aria(menu.list_label().busy(busy))
    }

    /// An `item` part: a suggestion. Choosing it inserts it into the text.
    /// Its `label` is the text it's matched by, inserts and is named by;
    /// gpui can't read the text it shows, so without one its value is.
    pub fn item(this: &Entity<Self>, props: TriggerMenuItemProps, cx: &App) -> Part<TriggerMenuItemState> {
        let menu = this.read(cx);
        let key = props.value.clone();
        let highlighted = menu.listbox.highlighted().as_ref() == Some(&key);
        let state = TriggerMenuItemState {
            highlighted,
            disabled: props.disabled,
            has_icon: has_icon(&menu.instance, &key),
        };
        // gpui can't read the text an item shows, so without a label its
        // value stands for it.
        let meta = ItemMeta::new(key.clone())
            .label(props.label.clone())
            .text(Some(key.clone()))
            .keywords(props.keywords.clone())
            .disabled(props.disabled)
            .group(props.group.clone());
        // Typing hides items without rendering them.
        let matches = menu.listbox.matches(menu, &key, &meta);
        menu.item_props.borrow_mut().insert(key.clone(), props.clone());
        let hover = this.downgrade();
        let hover_key = key.clone();
        let press = this.downgrade();
        let press_key = key.clone();
        let mut part = Part::new(&menu.instance, "item", Some(&key), state)
            .aria_role(Role::Option)
            .focus_mode(FocusMode::ActiveDescendant, None, false)
            .aria_selected(highlighted)
            .aria(Aria::new().disabled(props.disabled))
            .map_el(press_keeps_focus)
            .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                hover
                    .update(cx, |menu, cx| {
                        if !menu.popup.is_open() || menu.listbox.highlighted().as_ref() == Some(&hover_key) {
                            return;
                        }
                        let listbox = menu.listbox.clone();
                        let options = HighlightOptions::event(Some(InputEvent::MouseMove(event.clone())));
                        listbox.highlight(menu, Some(hover_key.clone()), options, window, cx);
                    })
                    .ok();
            })
            .on_press(move |event, window, cx| {
                press
                    .update(cx, |menu, cx| menu.choose(&press_key, Some(event), false, window, cx))
                    .ok();
            });
        if let Some(label) = props.label {
            part = part.aria_label(label);
        }
        if !matches {
            part = part.hidden();
        }
        menu.listbox.bind_item(key, meta, part, cx)
    }

    /// An `item-icon` part: an icon inside the item with key `item`, before
    /// its text. Screen readers skip it, and filtering and what a choice
    /// inserts ignore it.
    pub fn item_icon(
        this: &Entity<Self>,
        item: impl Into<SharedString>,
        props: TriggerMenuItemIconProps,
        cx: &App,
    ) -> Part<TriggerMenuItemIconState> {
        let menu = this.read(cx);
        let item = item.into();
        item_icon_part(&menu.instance, Some(&item), &props, cx)
    }

    /// A `group` part: a group of items, named by its label, by the key its
    /// items name in their `group` prop. It hides while none of its items
    /// match.
    pub fn group(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part<TriggerMenuGroupState> {
        let menu = this.read(cx);
        let key = key.into();
        let hidden = !menu.listbox.group_matches(menu, &key);
        let mut aria = Aria::new().hidden(hidden);
        if let Some(label) = group_labelled_by(&menu.instance, &key) {
            aria = aria.labelled_by(label);
        }
        let part = Part::new(&menu.instance, "group", Some(&key), TriggerMenuGroupState { hidden })
            .aria_role(Role::Group)
            .aria(aria);
        if hidden { part.hidden() } else { part }
    }

    /// A `group-label` part: the heading of the group with key `group`,
    /// which names it. Name it with `aria_label`.
    pub fn group_label(this: &Entity<Self>, group: impl Into<SharedString>, cx: &App) -> Part<()> {
        let menu = this.read(cx);
        let group = group.into();
        group_label_part(&menu.instance, &group)
    }

    /// A `separator` part: a line between items or groups, left out for
    /// screen readers, since a listbox may only hold options and groups.
    /// Hidden while a query filters items that aren't data.
    pub fn separator(this: &Entity<Self>, key: Option<&str>, cx: &App) -> Part<TriggerMenuSeparatorState> {
        let menu = this.read(cx);
        let hidden = !menu.query.is_empty() && menu.items.is_none();
        let part =
            Part::new(&menu.instance, "separator", key, TriggerMenuSeparatorState { hidden }).aria_role(Role::None);
        if hidden { part.hidden() } else { part }
    }

    /// The `empty` part: shown while no item matches, inside the content.
    /// Rendering it keeps the menu open with no matches; without it, the
    /// menu closes until something matches. Put its message in it while its
    /// state is `empty`: it's a polite status, so the message is announced.
    pub fn empty(this: &Entity<Self>, cx: &App) -> Part<TriggerMenuEmptyState> {
        let menu = this.read(cx);
        let state = TriggerMenuEmptyState {
            loading: menu.loading_status() == AsyncOptionsStatus::Loading,
            empty: menu.is_empty(),
            query: menu.query.clone(),
        };
        Part::new(&menu.instance, "empty", None, state)
            .aria_role(Role::Status)
            .aria(Aria::new().live(Live::Polite).atomic(true))
    }

    /// The `status` part: a polite status outside the popup. With `text`, it
    /// says the count and the keys a moment after typing pauses; without, it
    /// says what you put in it. Render one, visually hidden or not: without
    /// it, the menu speaks through the window's announcer.
    pub fn status(this: &Entity<Self>, text: bool, cx: &App) -> Part<TriggerMenuStatusState> {
        let menu = this.read(cx);
        let loading = menu.loading_status() == AsyncOptionsStatus::Loading;
        let state = TriggerMenuStatusState {
            open: menu.popup.is_open(),
            status: menu.loading_status(),
            loading,
            count: menu.item_count(),
            argument: menu.argument.clone(),
        };
        menu.status_writes.set(text);
        let mut part = Part::new(&menu.instance, "status", None, state)
            .aria_role(Role::Status)
            .aria(Aria::new().live(Live::Polite).atomic(true));
        if text {
            let message = menu.status_text.clone();
            part = part
                .aria_label(message.clone())
                .aria_value(message.clone())
                .child(message);
        }
        // Says the state once it's in the page.
        if !ui_sdk_kernel::part::held(&menu.instance, "status", None) {
            let entity = this.downgrade();
            part = part.on_props(move |window, cx| {
                entity.update(cx, |menu, cx| menu.announce_state(window, cx)).ok();
            });
        }
        part
    }

    /// The `hint` part: ghost text at the caret for the arguments still to
    /// type, like `<status>`. It renders above the window beside the caret,
    /// takes no presses, and screen readers skip it. With `text`, it shows
    /// the hints. `None` while there are none.
    pub fn hint(this: &Entity<Self>, text: bool, cx: &App) -> Option<TriggerMenuHint> {
        let menu = this.read(cx);
        if menu.hint.is_empty() {
            return None;
        }
        let state = TriggerMenuHintState {
            argument: menu.argument.clone(),
            hint: menu.hint.clone(),
        };
        let weak = this.downgrade();
        let last: Rc<Cell<Option<TextRect>>> = Rc::default();
        let positioner = menu.hint_positioner.clone();
        positioner.set_options(TriggerMenu::hint_options(cx));
        positioner.set_anchor(Some(Anchor::rect(move |window, cx| {
            let rect = weak.upgrade().and_then(|menu| menu.read(cx).caret_rect(window, cx));
            if rect.is_some() {
                last.set(rect);
            }
            last.get()
        })));
        let mut part = Part::new(&menu.instance, "hint", None, state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true));
        if text {
            part = part.child(menu.hint.clone());
        }
        Some(TriggerMenuHint { part, positioner })
    }
}

/// The hint part, which renders above the window beside the caret. It's
/// styled and filled like any part.
pub struct TriggerMenuHint {
    part: Part<TriggerMenuHintState>,
    positioner: ui_sdk_platform::Positioner,
}

impl TriggerMenuHint {
    /// The hint's state.
    pub fn state(&self) -> &TriggerMenuHintState {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &TriggerMenuHintState) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters, as on any part.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.part = self.part.aria(aria);
        self
    }
}

impl Styled for TriggerMenuHint {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl ParentElement for TriggerMenuHint {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl IntoElement for TriggerMenuHint {
    type Element = Deferred;

    fn into_element(self) -> Self::Element {
        deferred(positioned(&self.positioner, self.part)).priority(LAYER_PRIORITY)
    }
}
