//! Port of `components/select/select-parts.ts`: select's parts and what
//! they carry.

use std::fmt::Debug;
use std::rc::Rc;

use gpui::{
    App, ClickEvent, DispatchPhase, ElementId, Entity, Focusable, InteractiveElement, IntoElement, KeyboardButton,
    MouseButton, MouseUpEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, WeakEntity, canvas,
};
use ui_sdk_forms::labels_of;
use ui_sdk_kernel::bound_keys::bound;
use ui_sdk_kernel::dev;
use ui_sdk_kernel::part::{FocusMode, with_role};
use ui_sdk_kernel::roles::{HandledKey, Need, Role};
use ui_sdk_kernel::{Aria, AutoComplete, ChangeOptions, ChangeSource, HasPopup, InputEvent, Key, Part, Reason, flag_while_bound};
use ui_sdk_lists::async_options::AsyncOptionsStatus;
use ui_sdk_lists::item_icon::{ItemIconProps, ItemIconState, check_icon_item, has_icon, option_icon};
use ui_sdk_lists::listbox_engine::{ItemKey, ItemMeta, ListEmptyState, ListFilter, ListStatusState};
use ui_sdk_lists::listbox_keys::{option_id, select_all_key};
use ui_sdk_lists::listbox_parts::{
    FormAttrs, ItemIndicatorState, OptionGroupState, OptionSeparatorState, PositionAttrs, arrow_part, bind_item,
    empty_part, form_attrs, group_label_part, is_locked, item_indicator_part, no_items, option_group_part,
    option_separator_part, position_attrs, status_part, warn_if_unnamed,
};
use ui_sdk_lists::listbox_search::{ListItemTextState, item_text_part, match_mode};
use ui_sdk_lists::listbox_select_all::{ListSelectAllState, OptionFocus, SelectAll, SelectAllPartProps, select_all_feature};
use ui_sdk_lists::listbox_value::{FormatLabelsOptions, format_labels};
use ui_sdk_native::TextField;
use ui_sdk_popups::{
    Popup, PopupArrowState, PopupContent, PopupContentProps, PopupTriggerProps, PopupViewport, bind_popup_content,
    popup_viewport, set_unfocusable,
};
use ui_sdk_text::locale_of;

use crate::select::{
    SelectClearProps, SelectClearState, SelectContentProps, SelectContentState, SelectInputState,
    SelectItemIndicatorProps, SelectItemProps, SelectItemState, SelectTriggerProps, SelectTriggerState,
    SelectValueProps, SelectValueState,
};
use crate::select_api::{Select, TRIGGER};

// The keys the trigger handles: with `virtual_focus`, the open list's too,
// since focus stays here.
const TRIGGER_KEYS: &[HandledKey] = &["ArrowDown", "ArrowUp", "Home", "End", "Enter", " ", "typeahead"];
const VIRTUAL_TRIGGER_KEYS: &[HandledKey] = &[
    "ArrowDown", "ArrowUp", "PageDown", "PageUp", "ctrl+n", "ctrl+p", "Home", "End", "Enter", " ", "Tab", "Escape",
    "typeahead",
];

// The keys the content handles. The search field keeps Home, End, Space and
// typed text for its own editing.
const CONTENT_KEYS: &[HandledKey] = &[
    "ArrowDown", "ArrowUp", "PageDown", "PageUp", "ctrl+n", "ctrl+p", "Home", "End", "Enter", " ", "Tab", "Escape",
    "typeahead",
];
const SEARCH_CONTENT_KEYS: &[HandledKey] = &[
    "ArrowDown", "ArrowUp", "PageDown", "PageUp", "ctrl+n", "ctrl+p", "Enter", "Tab", "Escape",
];

// Hears the press that opened or closed the list end, anywhere in the
// window, while the trigger renders.
fn release_listener<T: ItemKey + Clone + PartialEq + Debug + 'static>(entity: WeakEntity<Select<T>>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, _, window, _cx| {
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                    return;
                }
                entity
                    .update(cx, |select, cx| select.pointer_release(event, window, cx))
                    .ok();
            });
        },
    )
    .absolute()
    .size_0()
}

// Whether a keyboard click is one the list's keys already handled: gpui
// clicks a focused option as Enter or Space comes up. Only a link's Enter
// is left to the click, which follows the link and chooses it.
fn handled_by_keys(event: &InputEvent, link: bool) -> bool {
    match event {
        InputEvent::Click(ClickEvent::Keyboard(click)) => !link || click.button != KeyboardButton::Enter,
        _ => false,
    }
}

impl<T: ItemKey + Clone + PartialEq + Debug + 'static> Select<T> {
    fn trigger_state(&self, props: &SelectTriggerProps) -> SelectTriggerState {
        SelectTriggerState {
            open: self.popup.is_open(),
            disabled: self.is_disabled() || props.disabled,
            read_only: self.read_only,
            required: self.required,
            placeholder: self.value.get().is_empty(),
        }
    }

    fn form_attrs(&self) -> FormAttrs {
        form_attrs(&self.form_state())
    }

    /// What the value part shows: the labels, and the placeholder while
    /// nothing is selected.
    pub fn value_state(&self, props: &SelectValueProps, cx: &App) -> SelectValueState<T> {
        let labels = self.selection.labels(self);
        let locale = locale_of(None, cx);
        let options = FormatLabelsOptions {
            max_labels: props.max_labels,
            get_more_label: props.get_more_label.clone(),
        };
        let label = format_labels(&labels, &options, Some(&locale));
        let placeholder = labels.is_empty();
        let text = if placeholder {
            props
                .placeholder
                .clone()
                .or_else(|| self.placeholder.clone())
                .unwrap_or_default()
        } else {
            label.clone()
        };
        SelectValueState {
            value: self.value.get().clone(),
            label,
            labels,
            placeholder,
            text,
        }
    }

    /// What the content and list parts style from (`contentState`).
    pub fn content_state(&self) -> SelectContentState<T> {
        let layer = self.popup.layer();
        SelectContentState {
            open: self.popup.is_open(),
            presentation: self.popup.presentation_value(),
            multiple: self.multiple,
            items: self
                .listbox
                .visible_data(self)
                .map(|visible| visible.list)
                .unwrap_or_else(no_items),
            status: self.status,
            modality: self.modality(),
            presence: layer.presence().state(),
            placement: layer.positioner().and_then(|positioner| positioner.placement()),
            hidden: !self.popup.is_mounted(),
            sheet: self.popup.sheet().map(|sheet| sheet.part_state()),
        }
    }

    // The list is named by the label part, else by what names the trigger,
    // its field's labels or the trigger itself, which names a node labelled
    // by it with its own name (`listName`).
    fn list_name(&self) -> Aria {
        let instance = &self.instance;
        if flag_while_bound(instance, "label") == Some(true) {
            return Aria::new().labelled_by(instance.element_id("label", None));
        }
        let field = self.internals.aria();
        if !field.labelled_by.is_empty() {
            let mut aria = Aria::new();
            aria.labelled_by = field.labelled_by;
            return aria;
        }
        Aria::new().labelled_by(instance.element_id("trigger", Some(TRIGGER)))
    }

    // What the trigger learns once each frame that rendered it is drawn.
    pub(crate) fn trigger_rendered(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        self.placement.viewport.set(Some(window.viewport_size()));
        if self.trigger_bound {
            return;
        }
        self.trigger_bound = true;
        // A value whose label or icon only its item knows: render the list
        // hidden so the item registers, once the parts that render now have.
        if self.items.is_none()
            && self.item_to_label.is_none()
            && self.selection.needs_items(self, self.wants_value_icon.get())
        {
            self.prepare(cx);
        }
        warn_if_unnamed(
            |this: &Self| &this.instance,
            "trigger",
            Some(TRIGGER),
            "The trigger has no accessible name. Add a label part, a Field label or an aria_label.",
            |this: &Self, _cx| {
                flag_while_bound(&this.instance, "label") == Some(true) || !labels_of(this.internals.node()).is_empty()
            },
            window,
            cx,
        );
    }

    // The search field rendered: development builds check, once, that
    // something names it.
    pub(crate) fn search_rendered(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if std::mem::replace(&mut self.search_bound, true) {
            return;
        }
        warn_if_unnamed(
            |this: &Self| &this.instance,
            "input",
            None,
            "The search field has no accessible name. Add an aria_label.",
            |_this: &Self, _cx| false,
            window,
            cx,
        );
    }

    /// The `label` part: a visible label that names the trigger. Pressing it
    /// focuses the trigger without opening the list, like a native select's
    /// label. Give it its text as an `aria_label`, which is the trigger's
    /// name.
    pub fn label(this: &Entity<Self>, cx: &App) -> Part<FormAttrs> {
        let select = this.read(cx);
        let entity = this.downgrade();
        Part::new(&select.instance, "label", None, select.form_attrs())
            .map_el(|el| with_role(el, gpui::Role::Label))
            .on_press(move |_event, window, cx| {
                entity.update(cx, |select, cx| select.focus(window, cx)).ok();
            })
    }

    /// The `trigger` part: the button that opens the list, a combobox named
    /// by the label. Typing on it selects the item that matches. A mouse
    /// press opens the list, and releasing over an item after a drag or a
    /// hold chooses it.
    pub fn trigger(this: &Entity<Self>, props: SelectTriggerProps, cx: &App) -> Part<SelectTriggerState> {
        let select = this.read(cx);
        let popup = select.popup.clone();
        let key: SharedString = TRIGGER.into();
        let (handle, region, _) = popup.bind_trigger(&key, &PopupTriggerProps::default().disabled(props.disabled), cx);
        let state = select.trigger_state(&props);
        let instance = &select.instance;
        select.sync_form();
        set_unfocusable(&handle, state.disabled);
        let labeled = flag_while_bound(instance, "label");
        let listed = bound(instance, "list");
        // Whether a field or a label of the app's names it.
        let named = !select.internals.control_attrs().labelled_by.is_empty() || !labels_of(select.internals.node()).is_empty();
        let mut aria = Aria::new()
            .has_popup(HasPopup::Listbox)
            .read_only(state.read_only)
            .required(state.required)
            .disabled(state.disabled);
        if popup.is_mounted() {
            aria = aria.controls(instance.element_id(if listed { "list" } else { "content" }, None));
        }
        // Until a label part is known to be missing, it names the trigger,
        // unless something else does.
        if labeled == Some(true) || (labeled.is_none() && !named) {
            aria = aria.labelled_by(instance.element_id("label", None));
        }
        if select.virtual_focus && !select.searchable() {
            aria = aria.active_descendant(select.active_descendant().map(ElementId::Name));
        }
        let value = select.value_state(&select.value_props.borrow(), cx);
        let keys = if select.virtual_focus { VIRTUAL_TRIGGER_KEYS } else { TRIGGER_KEYS };
        let internals = select.internals.clone();
        let mut part = Part::new(instance, "trigger", Some(&key), state)
            .aria_role(Role::Combobox)
            .focus_mode(FocusMode::Native, Some(&handle), state.disabled)
            .aria_expanded(state.open)
            .aria_value(value.text.clone())
            .aria(aria)
            .aria(select.internals.aria())
            .map_el(|el| internals.track(el));
        let entity = this.downgrade();
        {
            let entity = entity.clone();
            part = part.capture_key_down(move |event, window, cx| {
                if entity
                    .upgrade()
                    .is_some_and(|select| select.read(cx).takes_activation_key(&event.keystroke))
                {
                    window.prevent_default();
                }
            });
        }
        {
            let entity = entity.clone();
            part = part.on_keys(keys, move |event, window, cx| {
                // Escape while the open list takes the trigger's keys is the
                // popup's own: it closes the list before ancestors hear it.
                let listing = entity.upgrade().is_some_and(|select| {
                    let select = select.read(cx);
                    select.is_open() && select.virtual_focus && !select.searchable() && !select.focus_in_list
                });
                if listing && Key::of(&event.keystroke) == Key::Escape {
                    Popup::content_keydown(&entity, event, window, cx);
                    return;
                }
                entity
                    .update(cx, |select, cx| select.on_trigger_key_down(event, window, cx))
                    .ok();
            });
        }
        {
            // Mouse and pen open on press, like native selects. The click
            // that follows is ignored.
            let entity = entity.clone();
            part = part.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let touch = InputEvent::mouse_down(event, window).source() == ChangeSource::Touch;
                if touch || props.disabled {
                    return;
                }
                entity
                    .update(cx, |select, cx| {
                        if !select.is_disabled() {
                            select.pointer_press(event, window, cx);
                        }
                    })
                    .ok();
            });
        }
        {
            let entity = entity.clone();
            part = part.on_hover(move |hovered, _window, cx| {
                if *hovered {
                    entity.update(cx, |select, cx| select.prepare(cx)).ok();
                }
            });
        }
        {
            let entity = entity.clone();
            part = part.on_press(move |event, window, cx| {
                entity
                    .update(cx, |select, cx| {
                        if props.disabled || select.consume_press_click() {
                            return;
                        }
                        if select.is_open() {
                            let request = ui_sdk_popups::HideRequest::new()
                                .reason(Reason::TriggerPress)
                                .event(Some(event));
                            select.hide(request, window, cx);
                        } else {
                            let show = crate::select_api::Show {
                                event: Some(event),
                                by: None,
                                reason: Some(Reason::TriggerPress),
                                focus: None,
                            };
                            select.show(show, window, cx);
                        }
                    })
                    .ok();
            });
        }
        if select.press.is_some() {
            part = part.child(release_listener(entity.clone()));
        }
        if dev::DEV {
            part = part.keep_name();
        }
        {
            let entity = entity.clone();
            part = part.on_props(move |window, cx| {
                entity
                    .update(cx, |select, cx| select.trigger_rendered(window, cx))
                    .ok();
            });
        }
        region.measure(part)
    }

    /// The `value` part: the selected item's label, or the placeholder. It's
    /// `value_part`, since `value` is the selected value itself. Its
    /// state's `text` is what shows, which the trigger reports as its value
    /// to assistive technology, so render it as plain text.
    pub fn value_part(this: &Entity<Self>, props: SelectValueProps, cx: &App) -> Part<SelectValueState<T>> {
        let select = this.read(cx);
        let state = select.value_state(&props, cx);
        *select.value_props.borrow_mut() = props;
        Part::new(&select.instance, "value", None, state)
    }

    /// The `value-icon` part: the selected item's icon, rendered again from
    /// its item icon's content, like before the value in the trigger. In the
    /// page while one item is selected and its icon has content.
    pub fn value_icon(this: &Entity<Self>, cx: &App) -> Option<Part<ItemIconState>> {
        let select = this.read(cx);
        select.wants_value_icon.set(true);
        select
            .icons
            .value_icon(&select.instance, &select.selection.keys(select), cx)
    }

    /// The `content` part: the popup that holds the items, placed below the
    /// trigger, or over it with `align_item_with_trigger`. It's the listbox
    /// itself until a list part renders inside it. `None` while it's out of
    /// the page: it renders while the list is open or animating out, with
    /// `keep_mounted`, and hidden while closed once the trigger asked for its
    /// items. Give it a height limit and `overflow_y_scroll` to scroll the
    /// highlighted item into view.
    pub fn content(
        this: &Entity<Self>,
        props: SelectContentProps,
        cx: &App,
    ) -> Option<PopupContent<Self, SelectContentState<T>>> {
        let select = this.read(cx);
        let popup = select.popup.clone();
        select.placement.enabled.set(props.align_item_with_trigger);
        let popup_props = PopupContentProps {
            position: props.position.clone(),
            keep_mounted: props.keep_mounted || select.prepared,
            timing: props.timing.clone(),
            sheet_backdrop: props.sheet_backdrop,
            ..PopupContentProps::default()
        };
        let mounted = popup.is_mounted();
        if !mounted && !popup_props.keep_mounted {
            popup.set_content_props(popup_props);
            return None;
        }
        select.listbox.begin(cx);
        let instance = &select.instance;
        let listed = bound(instance, "list");
        let searching = select.searchable() && !select.focus_in_list;
        let state = select.content_state();
        let handle = popup.content_focus_handle().tab_stop(false);
        let mut aria = Aria::new().hidden(!mounted);
        let mut part = Part::new(instance, "content", None, state).map_el(|el| el.track_focus(&handle));
        if !listed {
            aria = aria
                .multiselectable(select.multiple)
                .busy(select.status == AsyncOptionsStatus::Loading)
                .merge(select.list_name());
            let scroll = select.scroll_handle.clone();
            part = part.aria_role(Role::Listbox).map_el(|el| el.track_scroll(&scroll));
            part = select.load_more.bind(part, scroll, this.downgrade());
        }
        let keys = if searching { SEARCH_CONTENT_KEYS } else { CONTENT_KEYS };
        let entity = this.downgrade();
        {
            let entity = entity.clone();
            part = part.aria(aria).on_keys(keys, move |event, window, cx| {
                // Tab and Escape are the popup's own: Escape closes the list
                // before ancestors hear it, and Tab closes it and moves on
                // from the trigger.
                let key = Key::of(&event.keystroke);
                if key == Key::Tab || key == Key::Escape {
                    Popup::content_keydown(&entity, event, window, cx);
                    return;
                }
                // Keys from a popup nested inside bubble through. Only the
                // owner handles them.
                let owned = entity.upgrade().is_some_and(|select| {
                    ui_sdk_popups::owns_event(&select.read(cx).popup.content_region(), window, cx)
                });
                if !owned {
                    return;
                }
                entity
                    .update(cx, |select, cx| select.on_content_key_down(event, window, cx))
                    .ok();
            });
        }
        {
            let entity = entity.clone();
            part = part.on_any_mouse_down(move |_event, _window, cx| {
                entity
                    .update(cx, |select, cx| {
                        if let Some(modality) = select.modality.as_mut()
                            && modality.pointer_down()
                        {
                            cx.notify();
                        }
                    })
                    .ok();
            });
        }
        {
            let entity = entity.clone();
            part = part.on_mouse_move(move |event, _window, cx| {
                entity
                    .update(cx, |select, cx| {
                        if let Some(modality) = select.modality.as_mut()
                            && modality.pointer_move(event.position)
                        {
                            cx.notify();
                        }
                    })
                    .ok();
            });
        }
        // It waits out of sight until the selected item lines up over the
        // trigger.
        if select.aligning() {
            part = part.opacity(0.);
        }
        part = select.settled.track(this.entity_id(), part);
        {
            let entity = entity.clone();
            part = part.on_props(move |window, cx| {
                entity
                    .update(cx, |select, cx| select.measure_alignment(window, cx))
                    .ok();
            });
        }
        Some(bind_popup_content(this, part, popup_props, cx))
    }

    /// The `viewport` part: the window-sized element the list sits in while
    /// it presents as a sheet, which renders above the window with the
    /// sheet's dim. Put the content inside it. `None` unless it presents as a
    /// sheet and is in the page.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> Option<PopupViewport<Self>> {
        popup_viewport(this, cx)
    }

    /// The `list` part: the listbox, when the content holds more than items,
    /// like a search field. Give it the height limit and the
    /// `overflow_y_scroll` then, so the field stays outside what scrolls.
    pub fn list(this: &Entity<Self>, cx: &App) -> Part<SelectContentState<T>> {
        let select = this.read(cx);
        select.listbox.begin(cx);
        let scroll = select.scroll_handle.clone();
        let part = Part::new(&select.instance, "list", None, select.content_state())
            .aria_role(Role::Listbox)
            .unneeded(Need::Keys, "The content around it handles them.")
            .aria(
                Aria::new()
                    .multiselectable(select.multiple)
                    .busy(select.status == AsyncOptionsStatus::Loading)
                    .merge(select.list_name()),
            )
            .map_el(|el| el.track_scroll(&scroll));
        select.load_more.bind(part, scroll, this.downgrade())
    }

    /// The `input` part: a search field inside the content that filters the
    /// list while focus stays in it, as the APG's select-only combobox with a
    /// search box asks. The trigger stays the select's button, holding one
    /// value. Name it with an `aria_label`; its own look and placeholder come
    /// from the `search` option.
    pub fn input(this: &Entity<Self>, cx: &App) -> Part<SelectInputState> {
        let select = this.read(cx);
        let instance = &select.instance;
        let disabled = select.is_disabled();
        let state = SelectInputState {
            open: select.popup.is_open(),
            disabled,
        };
        let handle = select.search.read(cx).focus_handle(cx);
        let (el, _) = TextField::root(&select.search, cx).into_element_parts();
        let aria = Aria::new()
            .autocomplete(AutoComplete::List)
            .controls(instance.element_id("list", None))
            .active_descendant(select.active_descendant().map(ElementId::Name))
            .disabled(disabled);
        let part = Part::new(instance, "input", None, state)
            .map_el(move |_| el)
            .aria_role(Role::Combobox)
            .focus_mode(FocusMode::Native, Some(&handle), disabled)
            .unneeded(Need::Keys, "The content around it handles them as they bubble from the field.")
            .aria_expanded(state.open)
            .aria(aria);
        if !dev::DEV {
            return part;
        }
        let entity = this.downgrade();
        part.keep_name().on_props(move |window, cx| {
            entity.update(cx, |select, cx| select.search_rendered(window, cx)).ok();
        })
    }

    /// The `item` part for an option, keyed by its value. Pressing it
    /// chooses it, or toggles it with `multiple`. It's hidden while the
    /// search hides it. Its `text` names it; an `aria_label` of your own
    /// wins.
    pub fn item(this: &Entity<Self>, props: SelectItemProps<T>, cx: &App) -> Part<SelectItemState> {
        let select = this.read(cx);
        let listbox = select.listbox.clone();
        let key = select.key_of(&props.value);
        let instance = &select.instance;
        let state = SelectItemState {
            highlighted: listbox.highlighted().as_ref() == Some(&key),
            selected: select.selection.is_selected(select, &key, Some(&props.value)),
            disabled: props.disabled,
        };
        let meta = ItemMeta::new(props.value.clone())
            .label(props.label.clone())
            .text(props.text.clone())
            .keywords(props.keywords.clone())
            .disabled(props.disabled)
            .group(props.group.clone())
            .index(props.index)
            .link(props.link);
        let shows = listbox.matches(select, &key, &meta);
        let is_virtual = select.is_virtual();
        let mut part = Part::new(instance, "item", Some(&key), state)
            .with_element_id(ElementId::Name(option_id(instance.id(), &key)))
            .aria_role(Role::Option);
        part = if is_virtual {
            part.focus_mode(FocusMode::ActiveDescendant, None, false)
        } else {
            let handle = listbox.focus_handle(&key, cx);
            part.focus_mode(FocusMode::Roving { tab_stop: false }, Some(&handle), false)
        };
        part = part
            .aria_selected(state.selected)
            .aria(Aria::new().disabled(state.disabled));
        if let Some(position) = position_attrs(props.index, &listbox, select) {
            part = PositionAttrs::apply(position, part);
        }
        if let Some(name) = props.text.clone().or_else(|| props.label.clone()) {
            part = part.aria_label(name);
        }
        // Typing hides the items it doesn't match.
        if !shows {
            part = part.hidden();
        }
        let entity = this.downgrade();
        {
            let entity = entity.clone();
            let key = key.clone();
            part = part.on_mouse_move(move |event, window, cx| {
                entity.update(cx, |select, cx| select.hover(&key, event, window, cx)).ok();
            });
        }
        // Presses keep focus in the search field or on the trigger.
        if is_virtual {
            part = part.on_any_mouse_down(ui_sdk_focus::hold_focus);
        }
        {
            let entity = entity.clone();
            let key = key.clone();
            let link = props.link;
            part = part.on_press(move |event, window, cx| {
                if handled_by_keys(&event, link) {
                    return;
                }
                entity
                    .update(cx, |select, cx| select.choose(&key, Some(event), window, cx))
                    .ok();
            });
        }
        // An item that shows only an icon names nothing.
        if dev::DEV && has_icon(instance, &key) {
            let entity = entity.clone();
            let key = key.clone();
            part = part.keep_name().on_props(move |window, cx| {
                if let Some(select) = entity.upgrade() {
                    check_icon_item(&select.read(cx).instance, "item", Some(&key), option_icon("select"), window);
                }
            });
        }
        bind_item(select, &listbox, &select.selection, key, meta, part, cx)
    }

    /// The `item-text` part: the text of the item with `item`'s value, split
    /// where the search matched it, so the matches can be marked. Render
    /// `state.segments`, like with gpui's `StyledText::with_highlights`.
    pub fn item_text(this: &Entity<Self>, item: &T, text: impl Into<SharedString>, cx: &App) -> Part<ListItemTextState> {
        let select = this.read(cx);
        let key = select.key_of(item);
        let filter = select.filter.clone().unwrap_or(ListFilter::Contains);
        let state = select
            .item_texts
            .state(&key, &text.into(), &select.filter_query(), match_mode(Some(&filter)));
        item_text_part(&select.instance, &key, state)
    }

    /// The `item-indicator` part: the check inside the item with `item`'s
    /// value, while it's selected, or always with `keep_mounted`. `None`
    /// while it's out of the page.
    pub fn item_indicator(
        this: &Entity<Self>,
        item: &T,
        props: SelectItemIndicatorProps,
        cx: &App,
    ) -> Option<Part<ItemIndicatorState>> {
        let select = this.read(cx);
        let key = select.key_of(item);
        let selected = select.selection.is_selected(select, &key, None);
        item_indicator_part(&select.instance, &key, selected, props.keep_mounted)
    }

    /// The `item-icon` part inside the item with `item`'s value. Screen
    /// readers, typeahead and the search skip it, and with `mirror` it says
    /// to flip in right-to-left text. With `content`, the value icon shows
    /// it again while its item is selected.
    pub fn item_icon(this: &Entity<Self>, item: &T, props: ItemIconProps, cx: &App) -> Part<ItemIconState> {
        let select = this.read(cx);
        let key = select.key_of(item);
        select.icons.item_icon(&select.instance, Some(&key), &props, cx)
    }

    /// A `group` of items, by a key of your own, named by its group label,
    /// and hidden while none of its items shows. Give its items the same key
    /// as their `group`.
    pub fn group(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part<OptionGroupState> {
        let select = this.read(cx);
        let key = key.into();
        option_group_part(&select.instance, &key, &select.listbox, select)
    }

    /// The `group-label` of the group with `group`'s key, which names the
    /// group. Give it its text as an `aria_label`.
    pub fn group_label(this: &Entity<Self>, group: impl Into<SharedString>, cx: &App) -> Part<()> {
        let select = this.read(cx);
        let group = group.into();
        group_label_part(&select.instance, &group)
    }

    /// A `separator` between items or groups, by a key of your own, left out
    /// for assistive technology. It hides while the search filters items that
    /// aren't data.
    pub fn separator(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part<OptionSeparatorState> {
        let select = this.read(cx);
        let key = key.into();
        let filtering = !select.filter_query().is_empty() && select.items.is_none();
        option_separator_part(&select.instance, Some(&key), filtering)
    }

    /// The `empty` part: a polite status that stays in the page and shows
    /// its message while no item matches, so it's announced as it appears.
    /// Give it children while its state is `empty`.
    pub fn empty(this: &Entity<Self>, cx: &App) -> Part<ListEmptyState> {
        let select = this.read(cx);
        let state = ListEmptyState {
            loading: select.status == AsyncOptionsStatus::Loading,
            empty: select.is_empty(),
            query: select.query.get().clone(),
        };
        empty_part(&select.instance, state)
    }

    /// The `status` part: a polite live region for loading and result
    /// counts. Its content is yours, from its state.
    pub fn status(this: &Entity<Self>, cx: &App) -> Part<ListStatusState> {
        let select = this.read(cx);
        let state = ListStatusState {
            status: select.status,
            loading: select.status == AsyncOptionsStatus::Loading,
            count: select.listbox.count(select),
        };
        status_part(&select.instance, state)
    }

    /// The `clear` part: a button that selects nothing and returns focus to
    /// the trigger. In the page while something is selected and the select
    /// can change, or always with `keep_mounted`. Keep it beside the
    /// trigger, not inside it.
    pub fn clear_part(this: &Entity<Self>, props: SelectClearProps, cx: &App) -> Option<Part<SelectClearState>> {
        let select = this.read(cx);
        let locked = is_locked(select.own_disabled(), select.internals.form_disabled(), select.read_only);
        let has_value = !select.value.get().is_empty();
        if !props.keep_mounted && !(has_value && !locked) {
            return None;
        }
        let disabled = select.is_disabled();
        let entity = this.downgrade();
        Some(
            Part::new(&select.instance, "clear", None, SelectClearState { disabled: locked })
                .aria_role(Role::Button)
                .focus_mode(FocusMode::Native, Some(&select.clear_handle), disabled)
                .aria(Aria::new().disabled(disabled))
                .on_press(move |event, window, cx| {
                    entity
                        .update(cx, |select, cx| {
                            select.focus(window, cx);
                            select.clear(ChangeOptions::event_of(Reason::ClearPress, event), window, cx);
                        })
                        .ok();
                }),
        )
    }

    /// The `arrow` part: an arrow on the popup's edge that points at the
    /// trigger, placed from its state once the list is placed.
    pub fn arrow(this: &Entity<Self>, cx: &App) -> Part<PopupArrowState> {
        arrow_part(this, cx)
    }

    /// The `select-all` part: an option that selects every item that matches,
    /// with `multiple`, in the group with `group`'s key or in the whole list.
    /// It reads as checked, unchecked or mixed. `text` is what it shows, for
    /// typeahead. `None` without `multiple`.
    pub fn select_all(
        this: &Entity<Self>,
        group: Option<&str>,
        text: Option<SharedString>,
        cx: &App,
    ) -> Option<Part<ListSelectAllState>> {
        let select = this.read(cx);
        let key = select_all_key(group);
        let state = SelectAll {
            listbox: &select.listbox,
            selection: &select.selection,
            limit: None,
        }
        .state(select, &key, select.listbox.highlighted().as_ref());
        let props = SelectAllPartProps {
            key,
            state,
            multiple: select.multiple,
            focus_in_list: select.focus_in_list,
            text,
            choose: Rc::new(|select: &mut Self, key, event, window, cx| select.choose(key, Some(event), window, cx)),
            hover: Rc::new(|select: &mut Self, key, event, window, cx| select.hover(key, event, window, cx)),
        };
        select_all_feature(OptionFocus::Roving).part(this, &select.instance, &select.listbox, props, cx)
    }
}
