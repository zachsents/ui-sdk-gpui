//! Port of `components/combobox/combobox-parts.ts`: the combobox's parts and
//! what they carry. Each slot is a part method on [`Combobox`]; slots there
//! are many of take their key, like an item its value.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, Entity, Focusable, Interactivity, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled,
};
use ui_sdk_focus::hold_focus;
use ui_sdk_kernel::dev;
use ui_sdk_kernel::part::with_role;
use ui_sdk_kernel::roles::{HandledKey, Need, Role};
use ui_sdk_kernel::{
    Aria, AutoComplete, ChangeOptions, ChangeSource, FocusMode, InputEvent, Part, PartElement, PartInfo,
    Reason, flag_while_bound,
};
use ui_sdk_lists::{
    FormAttrs, FormState, ItemIndicatorState, ItemMeta, ListEmptyState, ListItemTextState, ListPopup,
    ListStatusState, OptionFocus, OptionGroupState, OptionSeparatorState, SelectAll, SelectAllPartProps,
    bind_item, check_icon_item, empty_part, form_attrs, group_label_part, has_icon, item_indicator_part,
    item_text_part, list_content_part, list_id, list_name, match_mode, option_group_part, option_icon,
    option_id, option_separator_part, position_attrs, select_all_feature, select_all_key, status_part,
};
use ui_sdk_native::TextField;
use ui_sdk_popups::{PopupArrowState, PopupContentProps, PopupViewport, ShowRequest, opener_of, popup_viewport};
use ui_sdk_text::{js_trim, locale_of};

use crate::combobox::{
    ComboboxChipState, ComboboxChipsOverflowState, ComboboxClearProps, ComboboxClearState,
    ComboboxContentProps, ComboboxContentState, ComboboxCreateProps, ComboboxCreateState, ComboboxInputState,
    ComboboxItemIconProps, ComboboxItemIconState, ComboboxItemIndicatorProps, ComboboxItemProps,
    ComboboxItemState, ComboboxSelectAllState, ComboboxTriggerState, ComboboxValueProps, ComboboxValueState,
};
use crate::combobox_api::{CREATE, Combobox, ComboboxItemValue, FromInput};
use crate::combobox_chips::ChipsOverflow;

// The keys the input takes, by the original's names: the list's steps, Enter
// and Tab, and with chips the keys that remove and reach them. Escape closes
// the list through its layer, not here.
const INPUT_KEYS: &[HandledKey] = &["ArrowDown", "ArrowUp", "PageDown", "PageUp", "ctrl+n", "ctrl+p", "Enter", "Tab"];
const INPUT_KEYS_MULTIPLE: &[HandledKey] = &[
    "ArrowDown",
    "ArrowUp",
    "PageDown",
    "PageUp",
    "ctrl+n",
    "ctrl+p",
    "Enter",
    "Tab",
    "Backspace",
    "ArrowLeft",
    "ArrowRight",
];

impl<T: ComboboxItemValue> Combobox<T> {
    pub(crate) fn form_state(&self) -> FormState {
        FormState {
            multiple: self.multiple,
            disabled: self.own_disabled(),
            form_disabled: self.form_disabled,
            read_only: self.read_only,
            required: self.required,
            name: self.submit_name(),
            form: self.form.clone(),
        }
    }

    pub(crate) fn input_state(&self) -> ComboboxInputState {
        ComboboxInputState {
            open: self.popup.is_open(),
            disabled: self.is_disabled(),
            read_only: self.read_only,
            required: self.required,
        }
    }

    // What the content and list parts read.
    pub(crate) fn list_popup(&self) -> ListPopup {
        ListPopup {
            listed: flag_while_bound(&self.instance, "list") == Some(true),
            labeled: flag_while_bound(&self.instance, "label") == Some(true),
            field_name: self.field_name.clone(),
            multiple: self.multiple,
            status: self.status,
            prepared: self.prepared,
        }
    }

    /// The element id the input's active descendant points at, once that
    /// item is in the page. None while real focus is in the list
    /// (`activeDescendant`).
    pub(crate) fn active_descendant(&self) -> Option<ElementId> {
        let highlighted = self.listbox.highlighted()?;
        if !self.popup.is_open() || self.focus_in_list || !self.listbox.is_connected(&highlighted) {
            return None;
        }
        Some(ElementId::Name(option_id(self.instance.id(), &highlighted)))
    }

    // What the content and the list show (`contentState`), with the
    // popup's presence and placement.
    pub(crate) fn content_state(&self) -> ComboboxContentState<T> {
        let layer = self.popup.layer();
        ComboboxContentState {
            open: self.popup.is_open(),
            presentation: self.popup.presentation_value(),
            multiple: self.multiple,
            status: self.status,
            items: self.visible_items(),
            presence: layer.presence().state(),
            placement: layer.positioner().and_then(|positioner| positioner.placement()),
            hidden: !self.popup.is_mounted(),
            modal: self.popup.modal(),
            sheet: self.popup.sheet().map(|sheet| sheet.part_state()),
            modality: self
                .modality
                .as_ref()
                .map(|modality| modality.get())
                .unwrap_or(ui_sdk_kernel::Modality::Keyboard),
        }
    }

    // Whether no item matches. Composed items register once the content
    // has rendered, so until then it's not known (`isEmpty`).
    pub(crate) fn is_empty(&self) -> bool {
        (self.items.is_some() || self.settled.get()) && self.listbox.count(self) == 0
    }

    // Whether a chip collapsed into the overflow part: it's past the ones
    // that fit (`collapsed`).
    pub(crate) fn collapsed(&self, key: &SharedString) -> bool {
        let Some(visible) = self.visible_chips_now() else {
            return false;
        };
        self.selection
            .keys(self)
            .iter()
            .position(|own| own == key)
            .is_some_and(|index| index >= visible)
    }

    // How many chips fit, while an overflow part is in the page.
    fn visible_chips_now(&self) -> Option<usize> {
        if flag_while_bound(&self.instance, "chips-overflow") != Some(true) {
            return None;
        }
        self.visible_chips
    }

    /// The `label` part: the input's visible label. Give it its text as an
    /// `aria_label`, which names the input and the list. A press on it
    /// focuses the input, as a native label's does.
    pub fn label(this: &Entity<Self>, cx: &App) -> Part<FormAttrs> {
        let combobox = this.read(cx);
        let region = combobox.regions.label.clone();
        let entity = this.downgrade();
        Part::new(&combobox.instance, "label", None, form_attrs(&combobox.form_state()))
            .map_el(move |el| region.measure(with_role(el, gpui::Role::Label)))
            .on_press(move |_event, window, cx| {
                entity.update(cx, |this, cx| this.focus(window, cx)).ok();
            })
    }

    /// The `input` part: the text field that filters the list and holds the
    /// chosen item's label, the native field the combobox makes, with the
    /// combobox role over the field's. Style it as the input; the text draws
    /// with its text style and the `field_style` option, and the chips read
    /// its minimum width as it lays out. While the list presents as a sheet,
    /// render it inside the `header` part.
    pub fn input(this: &Entity<Self>, cx: &App) -> Part<ComboboxInputState> {
        let combobox = this.read(cx);
        combobox.sync_form();
        let instance = &combobox.instance;
        let state = combobox.input_state();
        let field = combobox.field.read(cx);
        let focus = field.focus_handle(cx);
        let mounted = combobox.popup.is_mounted();
        let list = combobox.list_popup();
        let mut aria = Aria::new()
            .autocomplete(AutoComplete::List)
            .required(state.required)
            .read_only(state.read_only)
            .disabled(state.disabled)
            .active_descendant(combobox.active_descendant());
        if mounted {
            aria = aria.controls(list_id(list.listed, instance));
        }
        if list.labeled {
            aria = aria.labelled_by(instance.element_id("label", None));
        }
        let internals = combobox.internals.clone();
        let (el, _) = TextField::root(&combobox.field, cx).into_element_parts();
        let keys = if combobox.multiple { INPUT_KEYS_MULTIPLE } else { INPUT_KEYS };
        let keyed = this.downgrade();
        let tapped = this.downgrade();
        let pressed = this.downgrade();
        let bound = this.downgrade();
        let chips = combobox.chips.clone();
        let part = Part::new(instance, "input", None, state)
            .map_el(move |_| internals.track(el))
            // What names it: the list takes its name, and development builds
            // check it has one.
            .keep_name()
            .aria_role(Role::Combobox)
            .focus_mode(FocusMode::Native, Some(&focus), state.disabled)
            .aria_expanded(state.open)
            .aria(aria)
            // What a Field or a label pointing at the input names it with,
            // which wins as an author's own does.
            .aria(combobox.internals.aria())
            .on_keys(keys, move |event, window, cx| {
                keyed
                    .update(cx, |this, cx| this.on_input_key_down(event, window, cx))
                    .ok();
            })
            // A tap that opens a sheet leaves the keyboard closed: it doesn't
            // focus the input, and the sheet takes focus.
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, window, cx| {
                let event = InputEvent::mouse_down(event, window);
                if event.source() != ChangeSource::Touch {
                    return;
                }
                tapped
                    .update(cx, |this, cx| {
                        let sheet = this.resolves_sheet(window);
                        if !this.sheet_input.tap(this.popup.is_open(), sheet) {
                            return;
                        }
                        window.prevent_default();
                        cx.stop_propagation();
                        let by = opener_of(Some(&event));
                        let request = ShowRequest::new()
                            .event(Some(event.clone()))
                            .by(by)
                            .reason(Reason::TriggerPress);
                        this.show(request, window, cx);
                    })
                    .ok();
            })
            // A press on the input opens the list. Enter and Space type.
            .on_press(move |event, window, cx| {
                if matches!(event, InputEvent::Click(ClickEvent::Keyboard(_))) {
                    return;
                }
                pressed
                    .update(cx, |this, cx| {
                        if this.popup.is_open() {
                            return;
                        }
                        let by = opener_of(Some(&event));
                        this.show(
                            ShowRequest::new()
                                .event(Some(event))
                                .by(by)
                                .reason(Reason::TriggerPress)
                                .extra(FromInput),
                            window,
                            cx,
                        );
                    })
                    .ok();
            })
            .on_props(move |window, cx| {
                bound.update(cx, |this, cx| this.input_rendered(window, cx)).ok();
            })
            .on_layout(move |layout, _window, _cx| chips.input_styled(layout.style()));
        combobox.regions.input.measure(part)
    }

    /// The `trigger` part: a button that opens and closes the list. Out of
    /// the tab order beside an input, since the input already does it, and in
    /// it when the input is in the popup. A press on it leaves focus in the
    /// input. ArrowDown and ArrowUp open the list at its first or last item.
    pub fn trigger(this: &Entity<Self>, cx: &App) -> Part<ComboboxTriggerState> {
        let combobox = this.read(cx);
        let instance = &combobox.instance;
        let state = combobox.input_state();
        let list = combobox.list_popup();
        let mut aria = Aria::new().disabled(state.disabled);
        if combobox.popup.is_mounted() {
            aria = aria.controls(list_id(list.listed, instance));
        }
        let handle = combobox.trigger_focus.clone();
        let inline = combobox.inline_input();
        let keyed = this.downgrade();
        let pressed = this.downgrade();
        let part = Part::new(instance, "trigger", None, state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&handle), state.disabled || inline)
            .aria_expanded(state.open)
            .aria(aria)
            .on_any_mouse_down(hold_focus)
            .on_keys(&["ArrowDown", "ArrowUp"], move |event, window, cx| {
                keyed
                    .update(cx, |this, cx| this.on_trigger_key_down(event, window, cx))
                    .ok();
            })
            .on_press(move |event, window, cx| {
                pressed
                    .update(cx, |this, cx| {
                        // A disabled native button takes no press.
                        if this.is_disabled() {
                            return;
                        }
                        this.focus(window, cx);
                        if this.popup.is_open() {
                            this.hide(Reason::TriggerPress, Some(event), window, cx);
                            return;
                        }
                        // Enter and Space press it too, as a click with no
                        // pointer behind it.
                        let by = opener_of(Some(&event));
                        this.show(
                            ShowRequest::new()
                                .event(Some(event))
                                .by(by)
                                .reason(Reason::TriggerPress),
                            window,
                            cx,
                        );
                    })
                    .ok();
            });
        combobox.regions.trigger.measure(part)
    }

    /// The `content` part: the popup that holds the list, placed below the
    /// input or the chips, or a sheet with `presentation`. It's the listbox
    /// itself until a `list` part renders inside it. `None` while it's out of
    /// the page. Make it the element that scrolls, as it reveals the
    /// highlighted item in itself.
    pub fn content(this: &Entity<Self>, props: ComboboxContentProps, cx: &App) -> Option<ComboboxContent<T>> {
        let combobox = this.read(cx);
        let list = combobox.list_popup();
        let popup_props = PopupContentProps {
            position: props.position,
            keep_mounted: props.keep_mounted,
            timing: props.timing,
            sheet_backdrop: props.sheet_backdrop,
            ..PopupContentProps::default()
        };
        let content = list_content_part(
            this,
            &list,
            popup_props,
            |this: &mut Self, event, window, cx| this.on_list_key_down(event, window, cx),
            cx,
        )?;
        let state = combobox.content_state();
        let settled = combobox.settled.clone();
        let entity_id = this.entity_id();
        let scroll = combobox.scroller.handle.clone();
        let load_more = combobox.load_more.clone();
        let entity = this.downgrade();
        let pressed = this.downgrade();
        let moved = this.downgrade();
        let content = content.map_part(move |part| {
            let part = settled.track(entity_id, part.track_scroll(&scroll));
            load_more
                .bind(part, scroll, entity)
                .on_any_mouse_down(move |_, _window, cx| {
                    pressed
                        .update(cx, |this, cx| {
                            if this.modality.as_mut().is_some_and(|modality| modality.pointer_down()) {
                                cx.notify();
                            }
                        })
                        .ok();
                })
                .on_mouse_move(move |event, _window, cx| {
                    let position = event.position;
                    moved
                        .update(cx, |this, cx| {
                            if this.modality.as_mut().is_some_and(|modality| modality.pointer_move(position)) {
                                cx.notify();
                            }
                        })
                        .ok();
                })
        });
        Some(ComboboxContent { content, state })
    }

    /// The `viewport` part: the window-sized element a list presented as a
    /// sheet sits in. Put the content inside it. `None` unless it presents
    /// as a sheet.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> Option<PopupViewport<Self>> {
        popup_viewport(this, cx)
    }

    /// The `header` part: the top of a sheet's content, which the input
    /// renders in while the sheet is open, so there's one input and one
    /// query. `None` unless it presents as a sheet and is open.
    pub fn header(this: &Entity<Self>, cx: &App) -> Option<Part<()>> {
        let combobox = this.read(cx);
        let shown = combobox.popup.presentation_value() == ui_sdk_popups::Presentation::Sheet && combobox.popup.is_open();
        shown.then(|| Part::new(&combobox.instance, "header", None, ()))
    }

    /// The `list` part: the listbox inside the content, when the content
    /// holds more than the list, like a header or a status. Its items render
    /// from `state.items` for items as data.
    pub fn list(this: &Entity<Self>, cx: &App) -> Part<ComboboxContentState<T>> {
        let combobox = this.read(cx);
        let list = combobox.list_popup();
        let instance = &combobox.instance;
        let state = combobox.content_state();
        Part::new(instance, "list", None, state)
            .aria_role(Role::Listbox)
            .unneeded(
                Need::Keys,
                "The combobox's input handles them, or the content around the list once focus is in it.",
            )
            .aria(
                Aria::new()
                    .multiselectable(list.multiple)
                    .busy(list.status == ui_sdk_lists::AsyncOptionsStatus::Loading)
                    .merge(list_name(list.labeled, list.field_name.as_ref(), instance)),
            )
    }

    /// An `item` part: an option. Pressing it chooses it, or toggles it with
    /// `multiple`. Typing hides it while its label and keywords don't match.
    /// Without a `label`, its text is its value's key, since gpui can't read
    /// the text an item renders.
    pub fn item(this: &Entity<Self>, props: ComboboxItemProps<T>, cx: &App) -> Part<ComboboxItemState> {
        let combobox = this.read(cx);
        let instance = &combobox.instance;
        let key = combobox.key_of(&props.value);
        let meta = ItemMeta::new(props.value.clone())
            .label(props.label.clone())
            .text(Some(key.clone()))
            .keywords(props.keywords.clone())
            .disabled(props.disabled)
            .group(props.group.clone())
            .index(props.index)
            .link(props.link);
        let state = combobox.item_state(&key, &props);
        let shown = combobox.listbox.matches(combobox, &key, &meta);
        let hovered = this.downgrade();
        let pressed = this.downgrade();
        let hover_key = key.clone();
        let press_key = key.clone();
        let mut part = Part::new(instance, "item", Some(&key), state)
            .with_element_id(option_id(instance.id(), &key))
            .aria_role(Role::Option);
        // Real focus reaches items only after a screen reader's tap moved it
        // into the list.
        part = if combobox.focus_in_list {
            let handle = combobox.listbox.focus_handle(&key, cx);
            part.focus_mode(FocusMode::Roving { tab_stop: false }, Some(&handle), false)
        } else {
            part.focus_mode(FocusMode::ActiveDescendant, None, false)
        };
        part = part
            .aria_selected(state.selected)
            .aria(Aria::new().disabled(state.disabled).hidden(!shown))
            .on_any_mouse_down(hold_focus)
            .on_mouse_move(move |event, window, cx| {
                // Touch never highlights, since a finger scrolling past
                // isn't choosing.
                if window.last_input_was_touch() {
                    return;
                }
                let event = InputEvent::MouseMove(event.clone());
                hovered
                    .update(cx, |this, cx| this.hover(&hover_key, event, window, cx))
                    .ok();
            })
            .on_press(move |event, window, cx| {
                pressed
                    .update(cx, |this, cx| this.choose(press_key.clone(), Some(event), window, cx))
                    .ok();
            });
        if let Some(position) = position_attrs(props.index, &combobox.listbox, combobox) {
            part = position.apply(part);
        }
        if !shown {
            part = part.hidden();
        }
        // An item that shows only an icon names nothing.
        if dev::DEV && has_icon(instance, &key) {
            let entity = this.downgrade();
            let key = key.clone();
            part = part.keep_name().on_props(move |window, cx| {
                if let Some(combobox) = entity.upgrade() {
                    check_icon_item(&combobox.read(cx).instance, "item", Some(&key), option_icon("combobox"), window);
                }
            });
        }
        bind_item(combobox, &combobox.listbox, &combobox.selection, key, meta, part, cx)
    }

    /// An `item-text` part: the item's text, by its item's key, split where
    /// the input's text matched, so the app marks those pieces. `text` is its
    /// item's label.
    pub fn item_text(this: &Entity<Self>, item: &T, text: impl Into<SharedString>, cx: &App) -> Part<ListItemTextState> {
        let combobox = this.read(cx);
        let key = combobox.key_of(item);
        let mode = match_mode(Some(&combobox.options.filter));
        let state = combobox.item_texts.state(&key, &text.into(), &combobox.filter_query(), mode);
        item_text_part(&combobox.instance, &key, state)
    }

    /// An `item-indicator` part: the check inside a selected item, by its
    /// item's value. `None` while the item isn't selected, unless
    /// `keep_mounted` keeps it in the page.
    pub fn item_indicator(
        this: &Entity<Self>,
        item: &T,
        props: ComboboxItemIndicatorProps,
        cx: &App,
    ) -> Option<Part<ItemIndicatorState>> {
        let combobox = this.read(cx);
        let key = combobox.key_of(item);
        let selected = combobox.selection.is_selected(combobox, &key, None);
        item_indicator_part(&combobox.instance, &key, selected, props.keep_mounted)
    }

    /// A `group` part, by a key of the app's own: a group of items, named
    /// by its `group-label`, and hidden while none of its items shows. Give
    /// the items inside it the same key as their `group`.
    pub fn group(this: &Entity<Self>, key: &str, cx: &App) -> Part<OptionGroupState> {
        let combobox = this.read(cx);
        option_group_part(&combobox.instance, key, &combobox.listbox, combobox)
    }

    /// A `group-label` part, by its group's key: the group's heading, which
    /// names it. Give it its text as an `aria_label`.
    pub fn group_label(this: &Entity<Self>, group: &str, cx: &App) -> Part<()> {
        let combobox = this.read(cx);
        group_label_part(&combobox.instance, group)
    }

    /// A `separator` part: a line between items or groups, hidden from
    /// assistive technology, and while typing filters items you render.
    pub fn separator(this: &Entity<Self>, key: Option<&str>, cx: &App) -> Part<OptionSeparatorState> {
        let combobox = this.read(cx);
        let filtering = !combobox.filter_query().is_empty() && combobox.items.is_none();
        option_separator_part(&combobox.instance, key, filtering)
    }

    /// The `empty` part: shown while no item matches. It stays in the page,
    /// so its message is announced when it appears; render it from its
    /// state's `empty`.
    pub fn empty(this: &Entity<Self>, cx: &App) -> Part<ListEmptyState> {
        let combobox = this.read(cx);
        let state = ListEmptyState {
            loading: combobox.status == ui_sdk_lists::AsyncOptionsStatus::Loading,
            empty: combobox.is_empty(),
            query: combobox.query.get().clone(),
        };
        empty_part(&combobox.instance, state)
    }

    /// The `status` part: a polite live region for loading and result
    /// counts. Its content is yours. While it's in the page, the combobox
    /// doesn't announce result counts itself.
    pub fn status(this: &Entity<Self>, cx: &App) -> Part<ListStatusState> {
        let combobox = this.read(cx);
        let state = ListStatusState {
            status: combobox.status,
            loading: combobox.status == ui_sdk_lists::AsyncOptionsStatus::Loading,
            count: combobox.listbox.count(combobox),
        };
        status_part(&combobox.instance, state)
    }

    /// The `clear` part: a button that empties the input and selects
    /// nothing, then keeps focus in the input. `None` while there's nothing
    /// to clear, unless `keep_mounted` keeps it in the page.
    pub fn clear_part(this: &Entity<Self>, props: ComboboxClearProps, cx: &App) -> Option<Part<ComboboxClearState>> {
        let combobox = this.read(cx);
        let locked = combobox.is_locked();
        let something = !combobox.value.get().is_empty() || !combobox.query.get().is_empty();
        if !props.keep_mounted && !(something && !locked) {
            return None;
        }
        let disabled = combobox.is_disabled();
        let handle = combobox.clear_focus.clone();
        let pressed = this.downgrade();
        let part = Part::new(&combobox.instance, "clear", None, ComboboxClearState { disabled: locked })
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&handle), disabled)
            .aria(Aria::new().disabled(disabled))
            .on_any_mouse_down(hold_focus)
            .on_press(move |event, window, cx| {
                pressed
                    .update(cx, |this, cx| {
                        this.clear(ChangeOptions::event_of(Reason::ClearPress, event), window, cx);
                        this.focus(window, cx);
                    })
                    .ok();
            });
        Some(combobox.regions.clear.measure(part))
    }

    /// The `value` part's state: the selected value with its labels, for
    /// rendering chips or a summary. It renders no element of its own.
    pub fn value(this: &Entity<Self>, props: ComboboxValueProps, cx: &App) -> ComboboxValueState<T> {
        let combobox = this.read(cx);
        let labels = combobox.selection.labels(combobox).to_vec();
        let options = ui_sdk_lists::FormatLabelsOptions {
            max_labels: props.max_labels,
            get_more_label: props.get_more_label,
        };
        let locale = locale_of(None, cx);
        ComboboxValueState {
            value: combobox.value.get().clone(),
            label: ui_sdk_lists::format_labels(&labels, &options, Some(&locale)),
            labels,
        }
    }

    /// The `arrow` part: an arrow on the list's edge that points at the
    /// input, following the side chosen after collision handling.
    pub fn arrow(this: &Entity<Self>, cx: &App) -> Part<PopupArrowState> {
        ui_sdk_lists::arrow_part(this, cx)
    }

    /// The `create` part: an option that creates an item from the input's
    /// text, shown while the text matches no item's label exactly and is
    /// `min_length` long. `on_create` saves it; choosing it doesn't change
    /// the value. `None` while it doesn't show.
    pub fn create(this: &Entity<Self>, props: ComboboxCreateProps, cx: &App) -> Option<Part<ComboboxCreateState>> {
        let combobox = this.read(cx);
        let shown = combobox.create_shown(&props);
        *combobox.create_props.borrow_mut() = props;
        if !shown {
            return None;
        }
        let instance = &combobox.instance;
        let key: SharedString = CREATE.into();
        let highlighted = combobox.listbox.highlighted().as_ref() == Some(&key);
        let query: SharedString = js_trim(combobox.query.get()).to_string().into();
        let state = ComboboxCreateState {
            query: query.clone(),
            highlighted,
            pending: combobox.pending,
            error: combobox.error.clone(),
        };
        let pending = combobox.pending;
        let hovered = this.downgrade();
        let pressed = this.downgrade();
        // It never takes real focus, even after a screen reader moved focus
        // into the list, as the original gives it no `tabindex`.
        let part = Part::new(instance, "create", None, state)
            .with_element_id(option_id(instance.id(), CREATE))
            .aria_role(Role::Option)
            .focus_mode(FocusMode::ActiveDescendant, None, false)
            .aria_selected(false)
            .aria(Aria::new().disabled(pending).busy(pending))
            .on_any_mouse_down(hold_focus)
            .on_mouse_move(move |event, window, cx| {
                if window.last_input_was_touch() {
                    return;
                }
                let event = InputEvent::MouseMove(event.clone());
                hovered
                    .update(cx, |this, cx| {
                        let key: SharedString = CREATE.into();
                        if this.listbox.highlighted().as_ref() == Some(&key) {
                            return;
                        }
                        let listbox = this.listbox.clone();
                        listbox.highlight(this, Some(key), ui_sdk_lists::HighlightOptions::event(Some(event)), window, cx);
                    })
                    .ok();
            })
            .on_press(move |event, window, cx| {
                pressed.update(cx, |this, cx| this.create_item(Some(event), window, cx)).ok();
            });
        let meta = ItemMeta::built_in(Some(format!("Create \u{201c}{query}\u{201d}").into()));
        Some(combobox.listbox.bind_item(key, meta, part, cx))
    }

    /// A `select-all` part, by the key of the group it's in, or `None` for
    /// the whole list: an option that selects every item that matches and
    /// can be chosen, and unselects them once they all are, read as checked,
    /// unchecked or mixed. `text` is what it shows, for typeahead. `None`
    /// without `multiple`.
    pub fn select_all(
        this: &Entity<Self>,
        group: Option<&str>,
        text: impl Into<SharedString>,
        cx: &App,
    ) -> Option<Part<ComboboxSelectAllState>> {
        let combobox = this.read(cx);
        let key = select_all_key(group);
        let limit = if combobox.multiple { combobox.max_selected } else { None };
        let highlighted = combobox.listbox.highlighted();
        let state = SelectAll {
            listbox: &combobox.listbox,
            selection: &combobox.selection,
            limit,
        }
        .state(combobox, &key, highlighted.as_ref());
        let props = SelectAllPartProps {
            key,
            state,
            multiple: combobox.multiple,
            focus_in_list: combobox.focus_in_list,
            text: Some(text.into()),
            choose: Rc::new(|this: &mut Self, key: &SharedString, event, window, cx| {
                this.choose(key.clone(), Some(event), window, cx)
            }),
            hover: Rc::new(|this: &mut Self, key: &SharedString, event: &MouseMoveEvent, window, cx| {
                if !window.last_input_was_touch() {
                    this.hover(key, InputEvent::MouseMove(event.clone()), window, cx);
                }
            }),
        };
        select_all_feature(OptionFocus::ActiveDescendant).part(this, &combobox.instance, &combobox.listbox, props, cx)
    }

    /// An `item-icon` part, by its item's value: an icon inside an item,
    /// left out for assistive technology, typeahead, filtering and match
    /// marks. With `props.content`, the `value-icon` shows it again; `mirror`
    /// flips it in right-to-left text.
    pub fn item_icon(this: &Entity<Self>, item: &T, props: ComboboxItemIconProps, cx: &App) -> Part<ComboboxItemIconState> {
        let combobox = this.read(cx);
        let key = combobox.key_of(item);
        combobox.icons.item_icon(&combobox.instance, Some(&key), &props, cx)
    }

    /// The `value-icon` part: the selected item's icon next to the input,
    /// rendered again from its `item-icon`'s content, while one item is
    /// selected and its icon has content. `None` otherwise.
    pub fn value_icon(this: &Entity<Self>, cx: &App) -> Option<Part<ComboboxItemIconState>> {
        let combobox = this.read(cx);
        let keys = combobox.selection.keys(combobox);
        combobox.icons.value_icon(&combobox.instance, &keys, cx)
    }

    /// The `chips` part: the selected values as chips, around the input, a
    /// toolbar named by the combobox's label. A press on its empty space
    /// focuses the input.
    pub fn chips(this: &Entity<Self>, cx: &App) -> ComboboxChips {
        let combobox = this.read(cx);
        combobox.chip_parts.begin();
        let instance = &combobox.instance;
        let labeled = flag_while_bound(instance, "label") == Some(true);
        // A Field's label names the input, so the chips take that name too,
        // unless the combobox's label or the app names them.
        let aria = if labeled {
            Aria::new().labelled_by(instance.element_id("label", None))
        } else {
            let mut aria = Aria::new();
            aria.labelled_by = combobox.internals.aria().labelled_by;
            aria
        };
        let handle = combobox.chips_focus.clone();
        let pressed = this.downgrade();
        let part = Part::new(instance, "chips", None, form_attrs(&combobox.form_state()))
            .aria_role(Role::Toolbar)
            .unneeded(Need::Keys, "Each chip handles the arrows that move between chips.")
            .aria_orientation(gpui::Orientation::Horizontal)
            .aria(aria)
            .map_el(move |el| el.track_focus(&handle.tab_stop(false)))
            .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                // A press that reached a chip, a button or the input is theirs.
                if window.default_prevented() {
                    return;
                }
                window.prevent_default();
                pressed.update(cx, |this, cx| this.focus(window, cx)).ok();
            });
        ComboboxChips {
            part: combobox.regions.chips.measure(part),
            chips: combobox.chips.clone(),
        }
    }

    /// A `chip` part: one selected value, a group named by its value's
    /// label. Arrows move between chips, and Backspace or Delete removes it.
    /// Collapsed into the overflow part, it renders hidden.
    pub fn chip(this: &Entity<Self>, value: T, cx: &App) -> Part<ComboboxChipState<T>> {
        let combobox = this.read(cx);
        let instance = &combobox.instance;
        let key = combobox.key_of(&value);
        combobox.chip_parts.register(key.clone(), PartInfo::default());
        let handle = combobox.chip_parts.focus_handle(&key, cx);
        let label = combobox.selection.label_of(combobox, &value);
        let hidden = combobox.collapsed(&key);
        let state = ComboboxChipState {
            value: value.clone(),
            label: label.clone(),
            disabled: combobox.is_locked(),
            hidden,
        };
        let keyed = this.downgrade();
        let chip_key = key.clone();
        let mut part = Part::new(instance, "chip", Some(&key), state)
            .aria_role(Role::Group)
            .map_el(move |el| el.track_focus(&handle.tab_stop(false)))
            .aria(Aria::new().disabled(combobox.is_disabled()).hidden(hidden))
            .on_key_down(move |event, window, cx| {
                keyed
                    .update(cx, |this, cx| this.on_chip_key_down(event, &chip_key, &value, window, cx))
                    .ok();
            });
        if !label.is_empty() {
            part = part.aria_label(label);
        }
        if hidden { part.hidden() } else { part }
    }

    /// A `chip-remove` part, by its chip's value: a button inside the chip
    /// that removes its value, then keeps focus in the input. Name it with
    /// `aria_label`.
    pub fn chip_remove(this: &Entity<Self>, value: T, cx: &App) -> Part<ComboboxClearState> {
        let combobox = this.read(cx);
        let key = combobox.key_of(&value);
        let disabled = combobox.is_disabled();
        let pressed = this.downgrade();
        Part::new(&combobox.instance, "chip-remove", Some(&key), ComboboxClearState { disabled: combobox.is_locked() })
            .aria_role(Role::Button)
            .unneeded(Need::Focus, "A press on it leaves focus in the input.")
            .aria(Aria::new().disabled(disabled))
            .on_any_mouse_down(hold_focus)
            .on_press(move |event, window, cx| {
                pressed
                    .update(cx, |this, cx| {
                        this.remove(&value, Some(event), window, cx);
                        this.focus(window, cx);
                    })
                    .ok();
            })
    }

    /// The `chips-overflow` part: how many chips don't fit on the chips'
    /// line, once some collapse, like "+2 more". Rendering it inside `chips`
    /// turns collapsing on. It's always in the page, hidden while every chip
    /// fits, so its width is known.
    pub fn chips_overflow(this: &Entity<Self>, cx: &App) -> Part<ComboboxChipsOverflowState> {
        let combobox = this.read(cx);
        let state = combobox.overflow_state();
        let hidden = combobox.visible_chips.is_none();
        let entity = this.downgrade();
        let part = Part::new(&combobox.instance, "chips-overflow", None, state).on_props(move |window, cx| {
            entity.update(cx, |this, cx| this.decide_chips(window, cx)).ok();
        });
        if hidden { part.hidden() } else { part }
    }
}

/// The `content` part, which renders above the window as the popup places
/// it, with the combobox's state beside the popup's.
pub struct ComboboxContent<T: ComboboxItemValue> {
    content: ui_sdk_popups::PopupContent<Combobox<T>, ui_sdk_popups::PopupContentState<()>>,
    state: ComboboxContentState<T>,
}

impl<T: ComboboxItemValue> ComboboxContent<T> {
    pub fn state(&self) -> &ComboboxContentState<T> {
        &self.state
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &ComboboxContentState<T>) -> Stateful<Div>) -> Self {
        let state = self.state.clone();
        self.content = self.content.with(move |el, _| style(el, &state));
        self
    }

    /// Adds ARIA beyond gpui's setters.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.content = self.content.aria(aria);
        self
    }
}

impl<T: ComboboxItemValue> Styled for ComboboxContent<T> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.content.style()
    }
}

impl<T: ComboboxItemValue> InteractiveElement for ComboboxContent<T> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.content.interactivity()
    }
}

impl<T: ComboboxItemValue> StatefulInteractiveElement for ComboboxContent<T> {}

impl<T: ComboboxItemValue> ParentElement for ComboboxContent<T> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements)
    }
}

impl<T: ComboboxItemValue> IntoElement for ComboboxContent<T> {
    type Element = ui_sdk_popups::PopupContentElement<Combobox<T>, ui_sdk_popups::PopupContentState<()>>;

    fn into_element(self) -> Self::Element {
        self.content.into_element()
    }
}

/// The `chips` part, which renders as its [`Part`] and tells the overflow
/// its padding, borders and gap as it renders.
pub struct ComboboxChips {
    part: Part<FormAttrs>,
    chips: ChipsOverflow,
}

impl ComboboxChips {
    pub fn state(&self) -> &FormAttrs {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &FormAttrs) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.part = self.part.aria(aria);
        self
    }
}

impl Styled for ComboboxChips {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl InteractiveElement for ComboboxChips {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl StatefulInteractiveElement for ComboboxChips {}

impl ParentElement for ComboboxChips {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl IntoElement for ComboboxChips {
    type Element = PartElement;

    fn into_element(mut self) -> Self::Element {
        self.chips.chips_styled(self.part.style());
        self.part.into_element()
    }
}
