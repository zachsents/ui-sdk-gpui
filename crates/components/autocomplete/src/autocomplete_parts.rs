//! Port of `components/autocomplete/autocomplete-parts.ts`: the
//! autocomplete's parts and what they carry. Each slot is a part method on
//! [`Autocomplete`]; slots there are many of take their key, like a
//! suggestion its value.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Div, Entity, Focusable, Interactivity, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, SharedString, Stateful, StatefulInteractiveElement, StyleRefinement,
    Styled,
};
use ui_sdk_focus::hold_focus;
use ui_sdk_kernel::dev;
use ui_sdk_kernel::part::with_role;
use ui_sdk_kernel::roles::{HandledKey, Need, Role};
use ui_sdk_kernel::{
    Aria, AutoComplete, ChangeOptions, ChangeSource, FocusMode, InputEvent, Modality, Part, Reason,
    flag_while_bound,
};
use ui_sdk_lists::{
    AsyncOptionsStatus, AutocompleteCoreItem, FormAttrs, FormState, ListEmptyState, ListItemTextState, ListPopup,
    ListStatusState, OptionGroupState, OptionPressFn, OptionSeparatorState, empty_part, field_part, form_attrs,
    check_icon_item, group_label_part, has_icon, item_text_part, list_content_part, list_id, list_name,
    option_group_part, option_icon, option_part, option_separator_part, status_part,
};
use ui_sdk_native::TextField;
use ui_sdk_popups::{
    PopupArrowState, PopupContentProps, PopupOpener, PopupViewport, Presentation, ShowRequest, opener_of,
    popup_viewport,
};

use crate::autocomplete::{
    AutocompleteClearProps, AutocompleteClearState, AutocompleteContentProps, AutocompleteContentState,
    AutocompleteInputState, AutocompleteItemIconProps, AutocompleteItemIconState, AutocompleteItemProps,
    AutocompleteItemState, AutocompleteTriggerState,
};
use crate::autocomplete_api::Autocomplete;

// The keys the field takes, by the original's names: the core's steps,
// Enter, Escape and Tab, and with inline completion ArrowRight and End,
// which accept it.
const INPUT_KEYS: &[HandledKey] = &[
    "ArrowDown",
    "ArrowUp",
    "PageDown",
    "PageUp",
    "ctrl+n",
    "ctrl+p",
    "Enter",
    "Escape",
    "Tab",
];
const INPUT_KEYS_INLINE: &[HandledKey] = &[
    "ArrowDown",
    "ArrowUp",
    "PageDown",
    "PageUp",
    "ctrl+n",
    "ctrl+p",
    "Enter",
    "Escape",
    "Tab",
    "ArrowRight",
    "End",
];

impl Autocomplete {
    pub(crate) fn form_state(&self) -> FormState {
        FormState {
            multiple: false,
            disabled: self.own_disabled(),
            form_disabled: self.form_disabled,
            read_only: self.read_only,
            required: self.required,
            name: self.submit_name(),
            form: self.form.clone(),
        }
    }

    pub(crate) fn input_state(&self) -> AutocompleteInputState {
        AutocompleteInputState {
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
            multiple: false,
            status: self.status,
            prepared: false,
        }
    }

    // What the content and the list show (`contentState`), with the popup's
    // presence and placement.
    pub(crate) fn content_state(&self) -> AutocompleteContentState {
        let layer = self.popup.layer();
        AutocompleteContentState {
            open: self.popup.is_open(),
            presentation: self.popup.presentation_value(),
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
                .unwrap_or(Modality::Keyboard),
        }
    }

    /// The `label` part: the field's visible label. Give it its text as an
    /// `aria_label`, which names the field and the list. A press on it does
    /// what a native label's does: it focuses the field, and passes the
    /// press on to a field that has focus already.
    pub fn label(this: &Entity<Self>, cx: &App) -> Part<FormAttrs> {
        let autocomplete = this.read(cx);
        let region = autocomplete.regions.label.clone();
        let entity = this.downgrade();
        Part::new(&autocomplete.instance, "label", None, form_attrs(&autocomplete.form_state()))
            .map_el(move |el| region.measure(with_role(el, gpui::Role::Label)))
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |this, cx| {
                        // A field that takes focus now opens its suggestions
                        // from the focus, before the press reaches it.
                        let focused = this.field.read(cx).is_focused(window);
                        this.focus(window, cx);
                        if focused {
                            this.input_pressed(event, window, cx);
                        }
                    })
                    .ok();
            })
    }

    /// The `input` part: the text field, the native field the autocomplete
    /// makes, with the combobox role over the field's. It's the form
    /// control, so the text submits under its `name`. Style it as the input;
    /// the text draws with its text style and the `field_style` option.
    /// While the list presents as a sheet, render it inside the `header`
    /// part.
    pub fn input(this: &Entity<Self>, cx: &App) -> Part<AutocompleteInputState> {
        let autocomplete = this.read(cx);
        autocomplete.sync_form(cx);
        let instance = &autocomplete.instance;
        let state = autocomplete.input_state();
        let focus = autocomplete.field.read(cx).focus_handle(cx);
        let list = autocomplete.list_popup();
        let inline = autocomplete.completes();
        let mut aria = Aria::new()
            .autocomplete(if inline { AutoComplete::Both } else { AutoComplete::List })
            .required(state.required)
            .read_only(state.read_only)
            .disabled(state.disabled);
        if autocomplete.popup.is_mounted() {
            aria = aria.controls(list_id(list.listed, instance));
        }
        if list.labeled {
            aria = aria.labelled_by(instance.element_id("label", None));
        }
        let internals = autocomplete.internals.clone();
        let (el, _) = TextField::root(&autocomplete.field, cx).into_element_parts();
        let keys = if inline { INPUT_KEYS_INLINE } else { INPUT_KEYS };
        let tapped = this.downgrade();
        let pressed = this.downgrade();
        let bound = this.downgrade();
        // What names it: the list takes its name, and development builds
        // check it has one.
        let part = Part::new(instance, "input", None, state)
            .map_el(move |_| internals.track(el))
            .keep_name();
        let part = field_part(
            this,
            instance,
            part,
            state.open,
            autocomplete.core.active_descendant(autocomplete),
            keys,
            |this: &mut Self, event, window, cx| this.on_input_key_down(event, window, cx),
        )
        .focus_mode(FocusMode::Native, Some(&focus), state.disabled)
        .aria(aria)
        // What a Field or a label pointing at the field names it with, which
        // wins as an author's own does.
        .aria(autocomplete.internals.aria())
        // A tap that opens a sheet leaves the keyboard closed: it doesn't
        // focus the field, and the sheet takes focus.
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
        // A press on the field reopens suggestions that open on focus, like
        // after Escape closed them. Enter and Space type.
        .on_press(move |event, window, cx| {
            if matches!(event, InputEvent::Click(ClickEvent::Keyboard(_))) {
                return;
            }
            pressed
                .update(cx, |this, cx| this.input_pressed(event, window, cx))
                .ok();
        })
        .on_props(move |window, cx| {
            bound.update(cx, |this, cx| this.input_rendered(window, cx)).ok();
        });
        autocomplete.regions.input.measure(part)
    }

    /// The `trigger` part: a button that opens and closes the suggestions.
    /// Out of the tab order, since the field already does it. A press on it
    /// leaves focus in the field.
    pub fn trigger(this: &Entity<Self>, cx: &App) -> Part<AutocompleteTriggerState> {
        let autocomplete = this.read(cx);
        let instance = &autocomplete.instance;
        let state = autocomplete.input_state();
        let list = autocomplete.list_popup();
        let mut aria = Aria::new().disabled(state.disabled);
        if autocomplete.popup.is_mounted() {
            aria = aria.controls(list_id(list.listed, instance));
        }
        let handle = autocomplete.trigger_focus.clone();
        let pressed = this.downgrade();
        let part = Part::new(instance, "trigger", None, state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&handle), true)
            .aria_expanded(state.open)
            .aria(aria)
            .on_any_mouse_down(hold_focus)
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
                        } else {
                            this.show(
                                ShowRequest::new()
                                    .event(Some(event))
                                    .by(PopupOpener::Pointer)
                                    .reason(Reason::TriggerPress),
                                window,
                                cx,
                            );
                        }
                    })
                    .ok();
            });
        autocomplete.regions.trigger.measure(part)
    }

    /// The `clear` part: a button that empties the field, then keeps focus
    /// in it. Out of the tab order. `None` while there's no text to clear,
    /// unless `keep_mounted` keeps it in the page.
    pub fn clear_part(this: &Entity<Self>, props: AutocompleteClearProps, cx: &App) -> Option<Part<AutocompleteClearState>> {
        let autocomplete = this.read(cx);
        let locked = autocomplete.is_locked();
        if !props.keep_mounted && (autocomplete.get_value().is_empty() || locked) {
            return None;
        }
        let disabled = autocomplete.is_disabled();
        let handle = autocomplete.clear_focus.clone();
        let pressed = this.downgrade();
        let part = Part::new(&autocomplete.instance, "clear", None, AutocompleteClearState { disabled: locked })
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&handle), true)
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
        Some(autocomplete.regions.clear.measure(part))
    }

    /// The `content` part: the popup that holds the suggestions, placed
    /// below the field, or a sheet with `presentation`. It's the listbox
    /// itself until a `list` part renders inside it. `None` while it's out of
    /// the page. Make it the element that scrolls, as it reveals the
    /// highlighted suggestion in itself.
    pub fn content(this: &Entity<Self>, props: AutocompleteContentProps, cx: &App) -> Option<AutocompleteContent> {
        let autocomplete = this.read(cx);
        let list = autocomplete.list_popup();
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
        let state = autocomplete.content_state();
        let settled = autocomplete.settled.clone();
        let entity_id = this.entity_id();
        let scroll = autocomplete.scroller.handle.clone();
        let load_more = autocomplete.core.load_more.clone();
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
        Some(AutocompleteContent { content, state })
    }

    /// The `viewport` part: the window-sized element suggestions presented
    /// as a sheet sit in. Put the content inside it. `None` unless they
    /// present as a sheet.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> Option<PopupViewport<Self>> {
        popup_viewport(this, cx)
    }

    /// The `header` part: the top of a sheet's content, which the field
    /// renders in while the sheet is open, so there's one field and one
    /// text. `None` unless they present as a sheet and are open.
    pub fn header(this: &Entity<Self>, cx: &App) -> Option<Part<()>> {
        let autocomplete = this.read(cx);
        let shown = autocomplete.popup.presentation_value() == Presentation::Sheet && autocomplete.popup.is_open();
        shown.then(|| Part::new(&autocomplete.instance, "header", None, ()))
    }

    /// The `list` part: the listbox inside the content, when the content
    /// holds more than the list, like a header or a status. Its suggestions
    /// render from `state.items` for items as data.
    pub fn list(this: &Entity<Self>, cx: &App) -> Part<AutocompleteContentState> {
        let autocomplete = this.read(cx);
        let list = autocomplete.list_popup();
        let instance = &autocomplete.instance;
        Part::new(instance, "list", None, autocomplete.content_state())
            .aria_role(Role::Listbox)
            .unneeded(
                Need::Keys,
                "The autocomplete's field handles them, or the content around the list once focus is in it.",
            )
            .aria(
                Aria::new()
                    .busy(list.status == AsyncOptionsStatus::Loading)
                    .merge(list_name(list.labeled, list.field_name.as_ref(), instance)),
            )
    }

    /// An `item` part: a suggestion. Pressing it fills the field with its
    /// label, or its value without one, and closes the list, then submits
    /// with `submit_on_select`. Typing hides it while its label and keywords
    /// don't match. Without a `label`, its text is its value, since gpui
    /// can't read the text it renders.
    pub fn item(this: &Entity<Self>, props: AutocompleteItemProps, cx: &App) -> Part<AutocompleteItemState> {
        let autocomplete = this.read(cx);
        let key = props.value.clone();
        let item = AutocompleteCoreItem {
            text: Some(props.value.clone()),
            value: props.value,
            label: props.label,
            keywords: props.keywords,
            disabled: props.disabled,
            index: props.index,
            group: props.group,
            link: props.link,
        };
        let press: OptionPressFn<Self> = Rc::new(|this: &mut Self, key: &SharedString, event, window, cx| {
            this.press(key.clone(), Some(event), window, cx)
        });
        let part = option_part(this, &autocomplete.instance, "item", &autocomplete.core, autocomplete, &key, &item, press, cx)
            .map_state(|option| AutocompleteItemState {
                highlighted: option.highlighted,
                disabled: option.disabled,
            });
        // A suggestion that shows only an icon names nothing.
        if !dev::DEV || !has_icon(&autocomplete.instance, &key) {
            return part;
        }
        let entity = this.downgrade();
        part.keep_name().on_props(move |window, cx| {
            if let Some(autocomplete) = entity.upgrade() {
                check_icon_item(&autocomplete.read(cx).instance, "item", Some(&key), option_icon("autocomplete"), window);
            }
        })
    }

    /// An `item-text` part: the suggestion's text, by its value, split where
    /// the field's text matched, so the app marks those pieces. `text` is
    /// its suggestion's label.
    pub fn item_text(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        text: impl Into<SharedString>,
        cx: &App,
    ) -> Part<ListItemTextState> {
        let autocomplete = this.read(cx);
        let key = value.into();
        let core = &autocomplete.core;
        let state = autocomplete.item_texts.state(
            &key,
            &text.into(),
            &core.filter_query(autocomplete),
            core.match_mode(autocomplete),
        );
        item_text_part(&autocomplete.instance, &key, state)
    }

    /// An `item-icon` part, by its suggestion's value: an icon inside a
    /// suggestion, left out for assistive technology, filtering, match marks
    /// and the text put in the field. `mirror` flips it in right-to-left
    /// text.
    pub fn item_icon(
        this: &Entity<Self>,
        value: &str,
        props: AutocompleteItemIconProps,
        cx: &App,
    ) -> Part<AutocompleteItemIconState> {
        let autocomplete = this.read(cx);
        autocomplete.icons.item_icon(&autocomplete.instance, Some(value), &props, cx)
    }

    /// A `group` part, by a key of the app's own: a group of suggestions,
    /// named by its `group-label`, and hidden while none of its suggestions
    /// shows. Give the suggestions inside it the same key as their `group`.
    pub fn group(this: &Entity<Self>, key: &str, cx: &App) -> Part<OptionGroupState> {
        let autocomplete = this.read(cx);
        option_group_part(&autocomplete.instance, key, &autocomplete.core.listbox, autocomplete)
    }

    /// A `group-label` part, by its group's key: the group's heading, which
    /// names it. Give it its text as an `aria_label`.
    pub fn group_label(this: &Entity<Self>, group: &str, cx: &App) -> Part<()> {
        let autocomplete = this.read(cx);
        group_label_part(&autocomplete.instance, group)
    }

    /// A `separator` part: a line between suggestions or groups, hidden from
    /// assistive technology, and while typing filters suggestions you
    /// render.
    pub fn separator(this: &Entity<Self>, key: Option<&str>, cx: &App) -> Part<OptionSeparatorState> {
        let autocomplete = this.read(cx);
        let filtering = !autocomplete.query.is_empty() && autocomplete.items.is_none();
        option_separator_part(&autocomplete.instance, key, filtering)
    }

    /// The `empty` part: shown while no suggestion matches. It stays in the
    /// page, so its message is announced when it appears; render it from its
    /// state's `empty`.
    pub fn empty(this: &Entity<Self>, cx: &App) -> Part<ListEmptyState> {
        let autocomplete = this.read(cx);
        let state = ListEmptyState {
            loading: autocomplete.status == AsyncOptionsStatus::Loading,
            empty: autocomplete.core.is_empty(autocomplete),
            query: autocomplete.query.clone(),
        };
        empty_part(&autocomplete.instance, state)
    }

    /// The `status` part: a polite live region for loading and result
    /// counts. Its content is yours. While it's in the page, the
    /// autocomplete doesn't announce result counts itself.
    pub fn status(this: &Entity<Self>, cx: &App) -> Part<ListStatusState> {
        let autocomplete = this.read(cx);
        let state = ListStatusState {
            status: autocomplete.status,
            loading: autocomplete.status == AsyncOptionsStatus::Loading,
            count: autocomplete.core.item_count(autocomplete),
        };
        status_part(&autocomplete.instance, state)
    }

    /// The `arrow` part: an arrow on the suggestions' edge that points at
    /// the field, following the side chosen after collision handling.
    pub fn arrow(this: &Entity<Self>, cx: &App) -> Part<PopupArrowState> {
        ui_sdk_lists::arrow_part(this, cx)
    }
}

/// The `content` part, which renders above the window as the popup places
/// it, with the autocomplete's state beside the popup's.
pub struct AutocompleteContent {
    content: ui_sdk_popups::PopupContent<Autocomplete, ui_sdk_popups::PopupContentState<()>>,
    state: AutocompleteContentState,
}

impl AutocompleteContent {
    pub fn state(&self) -> &AutocompleteContentState {
        &self.state
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &AutocompleteContentState) -> Stateful<Div>) -> Self {
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

impl Styled for AutocompleteContent {
    fn style(&mut self) -> &mut StyleRefinement {
        self.content.style()
    }
}

impl InteractiveElement for AutocompleteContent {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.content.interactivity()
    }
}

impl StatefulInteractiveElement for AutocompleteContent {}

impl ParentElement for AutocompleteContent {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements)
    }
}

impl IntoElement for AutocompleteContent {
    type Element = ui_sdk_popups::PopupContentElement<Autocomplete, ui_sdk_popups::PopupContentState<()>>;

    fn into_element(self) -> Self::Element {
        self.content.into_element()
    }
}
