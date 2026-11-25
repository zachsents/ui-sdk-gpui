//! Port of `components/combobox/combobox-api.ts`: the combobox's state and
//! API: opening and closing, the text and how it filters, choosing and
//! removing items, the create option, and what the form asks of it.
//!
//! The state is the [`Combobox`]'s fields and the API its methods. The
//! original's store subscribers, which work out what follows a change in the
//! same pass, are the methods each change calls as it happens:
//! `value_arrived` for a value, `highlight_moved` for the highlight, and
//! `list_changed` once items render, which settles the highlight as the
//! original's microtasks do.

use std::any::Any;
use std::cell::RefCell;
use std::fmt::Debug;
use std::rc::Rc;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, KeystrokeEvent, SharedString, Subscription,
    Window,
};
use ui_sdk_focus::Step;
use ui_sdk_forms::{
    Association, ElementInternals, INPUT_AND_CHANGE, associate, dispatch_change, keeps_value_on_reset, labels_of,
    submit_implicitly,
};
use ui_sdk_kernel::action::ActionHandlers;
use ui_sdk_kernel::change::log_with;
use ui_sdk_kernel::modality::{ModalityTracker, modality_of, restore_focus, track_input, track_modality};
use ui_sdk_kernel::watchers;
use ui_sdk_kernel::{
    ActionOutcome, AnnouncePending, ChangeDetails, ChangeOptions, Controllable, InputEvent, Instance, Key, Parts,
    Reason, focus_events, held, name_of, resolve_source, run_action, with_source,
};
use ui_sdk_lists::{
    AnnounceResults, AnnounceResultsOptions, AsyncOptionsStatus, FieldName, HighlightOptions, ItemKey,
    ItemIconsFeature, ItemTexts, ListAnnouncement, ListAnnouncementFn, ListData, ListEntry, ListFocus, ListHighlightDetails,
    ListScroller, ListSettled, ListValue, ListboxEngine, ListboxEngineConfig, LoadMore, LoadMoreFn,
    SelectAll, ValueOptions, ValueSelection, ValueSelectionConfig, announce_results, create_filtered_listbox,
    create_load_more, create_value_selection, describe_list, flatten_data, highlight_from_code,
    highlighted_value, is_select_all, item_icons_feature, item_key, list_content_config, opens_into_list,
    option_icon,
};
use ui_sdk_motion::SheetSnapReason;
use ui_sdk_native::{TextField, TextFieldEvent, TextFieldOptions};
use ui_sdk_platform::{Anchor, Region, Target, announce};
use ui_sdk_popups::{
    CreatePopupOptions, HideRequest, Popup, PopupClose, PopupHost, PopupOpenOptions, PopupOpener,
    PopupOptions, PopupPending, PopupRules, Presentation, SheetConfig, SheetInput, ShowRequest,
    bind_sheet_input, combobox_preset, create_popup, derive_open,
};
use ui_sdk_text::{js_trim, normalize};

use crate::combobox::{
    ComboboxAutoHighlight, ComboboxChipsOverflowState, ComboboxCreateProps, ComboboxItemProps, ComboboxItemState,
    ComboboxOptions, ComboboxValue,
};
use crate::combobox_chips::ChipsOverflow;

/// The create option's key in the highlight. Never a real item's.
pub const CREATE: &str = "\u{0}create";

/// What a combobox's value can be: any value an item holds, compared by
/// value, keyed by [`ItemKey`] unless `item_to_value` says otherwise. Text,
/// numbers and types of your own with an `ItemKey` are.
pub trait ComboboxItemValue: Clone + PartialEq + Debug + ItemKey + 'static {}

impl<T: Clone + PartialEq + Debug + ItemKey + 'static> ComboboxItemValue for T {}

/// The item the highlight goes to once the list has opened, after an arrow
/// key opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    First,
    Last,
}

impl Edge {
    fn step(self) -> Step {
        match self {
            Edge::First => Step::First,
            Edge::Last => Step::Last,
        }
    }
}

// What a show from the input itself carries: a tap on the input is for
// typing, so it never moves real focus into the list.
pub(crate) struct FromInput;

// The parts that make up the control around the list. Presses on them don't
// dismiss it, and focus moving among them doesn't close it (`controlSlots`).
#[derive(Clone, Default)]
pub(crate) struct ControlRegions {
    pub(crate) input: Region,
    pub(crate) chips: Region,
    pub(crate) trigger: Region,
    pub(crate) clear: Region,
    pub(crate) label: Region,
}

impl ControlRegions {
    fn all(&self) -> [&Region; 5] {
        [&self.input, &self.chips, &self.trigger, &self.clear, &self.label]
    }

    // Whether a press or focus lands on one of them (`ownsElement`).
    pub(crate) fn owns(&self, target: &Target, window: &gpui::Window) -> bool {
        self.all().iter().any(|region| region.contains(target, window))
    }
}

/// A combobox. Its entity is its controller.
pub struct Combobox<T: ComboboxItemValue> {
    pub(crate) instance: Instance,
    pub(crate) popup: Popup<Self>,
    // The input, the native field whose element is the `input` part.
    pub(crate) field: Entity<TextField>,
    pub(crate) internals: ElementInternals,
    pub(crate) listbox: ListboxEngine<Self, T>,
    pub(crate) selection: ValueSelection<Self, T>,
    pub(crate) options: ComboboxOptions<T>,

    pub(crate) value: Controllable<ComboboxValue<T>>,
    // The value a form reset returns to.
    pub(crate) initial: ComboboxValue<T>,
    pub(crate) query: Controllable<SharedString>,
    // Typed since the last choice, so the list filters. A single combobox
    // shows its value's label without filtering until the user types.
    pub(crate) dirty: bool,
    pub(crate) status: AsyncOptionsStatus,
    pub(crate) items: Option<ListData<T>>,
    pub(crate) multiple: bool,
    pub(crate) max_selected: Option<usize>,
    pub(crate) disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) name: Option<SharedString>,
    pub(crate) form: Option<SharedString>,
    pub(crate) form_disabled: bool,
    // A create option's work is running, or failed.
    pub(crate) pending: bool,
    pub(crate) error: Option<SharedString>,
    // The label version the value's labels were last shown with.
    pub(crate) labels: u64,
    // Counts batches of items rendering, so parts that depend on the list
    // update.
    pub(crate) mounts: u64,
    // The list is in the page, hidden, so items tell the value its label.
    pub(crate) prepared: bool,
    // Real focus moved into the list for a touch screen reader, instead of an
    // active descendant.
    pub(crate) focus_in_list: bool,
    // How many chips fit before the rest collapse into an overflow part, or
    // `None` for all.
    pub(crate) visible_chips: Option<usize>,
    // How the user works in the list while it shows (`trackModality`).
    pub(crate) modality: Option<ModalityTracker>,

    // What names the field when no label part does.
    pub(crate) field_name: Option<FieldName>,
    // The content has rendered with its items, so an empty list is really
    // empty.
    pub(crate) settled: ListSettled,
    // The element items reveal in: the content.
    pub(crate) scroller: ListScroller,
    pub(crate) item_texts: ItemTexts,
    // The icons of its items, which a value icon shows again.
    pub(crate) icons: ItemIconsFeature,
    pub(crate) trigger_focus: FocusHandle,
    pub(crate) clear_focus: FocusHandle,
    pub(crate) chips_focus: FocusHandle,
    // The input renders inside the content, like in a popup a button opens,
    // or in a sheet's header.
    pub(crate) input_in_content: bool,
    pub(crate) input_bound: bool,
    // The create part's props from its last render.
    pub(crate) create_props: RefCell<ComboboxCreateProps>,
    // The chips in the page, in order, with their focus handles.
    pub(crate) chip_parts: Parts<SharedString>,
    pub(crate) chips: ChipsOverflow,
    pub(crate) regions: ControlRegions,
    pub(crate) sheet_input: SheetInput,
    // The text last written to the field.
    pub(crate) shown: Option<SharedString>,

    mounts_queued: bool,
    opened_by: PopupOpener,
    // A choice is setting the value, and sets the text itself with its own
    // reason.
    committing: bool,
    // The highlight to place once the list has opened, after an arrow key
    // opened it.
    pub(crate) pending_highlight: Option<Edge>,
    // The highlight `auto_highlight` placed last, until anything else moves
    // it.
    placed: Option<SharedString>,
    settling: bool,
    // The highlight as `highlight_moved` last heard it.
    highlighted_seen: Option<SharedString>,
    // A label a value from options showed in its own pass, past refusing.
    shown_query: Option<(SharedString, SharedString)>,
    // Counts closes, so a create that settles after the list closed doesn't
    // close it again once it's reopened.
    session: u64,
    // The value deleting the text emptied while the list is open, which
    // Escape brings back.
    cleared_value: Option<ComboboxValue<T>>,
    // What the list last listed for: items, mounts and query.
    listed: (Option<ListData<T>>, u64, SharedString),
    // The value as `value_arrived` last heard it.
    previous_value: ComboboxValue<T>,
    announce_pending: AnnouncePending,
    pub(crate) results: AnnounceResults<Self>,
    pub(crate) load_more: LoadMore<Self>,
    _subscriptions: Vec<Subscription>,
}

impl<T: ComboboxItemValue> PopupHost for Combobox<T> {
    type Payload = ();

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

// Text as a value of `T`, when `T` holds text: the original keeps text that
// names no item as the value itself.
pub(crate) fn text_value<T: Clone + 'static>(text: &SharedString) -> Option<T> {
    let shared: &dyn Any = text;
    if let Some(value) = shared.downcast_ref::<T>() {
        return Some(value.clone());
    }
    let owned = text.to_string();
    let owned: &dyn Any = &owned;
    owned.downcast_ref::<T>().cloned()
}

// A value as text, when it's text (`typeof value === "string"`).
pub(crate) fn value_text<T: 'static>(value: &T) -> Option<SharedString> {
    let any: &dyn Any = value;
    if let Some(text) = any.downcast_ref::<SharedString>() {
        return Some(text.clone());
    }
    if let Some(text) = any.downcast_ref::<String>() {
        return Some(SharedString::from(text.clone()));
    }
    any.downcast_ref::<&'static str>().map(|text| SharedString::new_static(text))
}

// A single combobox's text for its value: from `item_to_label`, the items
// data, the items rendered so far, then the value itself (`initialQuery`).
fn initial_query<T: ComboboxItemValue>(
    options: &ComboboxOptions<T>,
    value: &ComboboxValue<T>,
    rendered: Option<&dyn Fn(&SharedString) -> Option<SharedString>>,
) -> SharedString {
    let Some(first) = value.first() else {
        return SharedString::default();
    };
    if options.multiple {
        return SharedString::default();
    }
    let key = item_key(first, options.item_to_value.as_deref());
    if let Some(item_to_label) = &options.item_to_label {
        return item_to_label(first);
    }
    if let Some(items) = &options.items {
        let to_key = |value: &T| item_key(value, options.item_to_value.as_deref());
        if let Some(entry) = flatten_data(items, &to_key).into_iter().find(|entry| entry.key == key) {
            return entry.label;
        }
    }
    if let Some(label) = rendered.and_then(|rendered| rendered(&key)) {
        return label;
    }
    value_text(first).unwrap_or(key)
}

// The options the field takes from the combobox's: the input's attributes.
fn field_options<T: ComboboxItemValue>(
    options: &ComboboxOptions<T>,
    query: &SharedString,
    disabled: bool,
) -> TextFieldOptions {
    TextFieldOptions {
        default_value: query.clone(),
        placeholder: options.placeholder.clone(),
        disabled,
        read_only: options.read_only,
        required: options.required,
        // `autocomplete="off"`, `autocapitalize="none"`, `spellcheck="false"`.
        configuration: gpui::TextInputConfiguration {
            autocorrect: false,
            autocapitalize: gpui::Autocapitalize::None,
            suggestions: false,
            ..Default::default()
        },
        style: options.field_style,
        ..TextFieldOptions::default()
    }
}

// Says that a value was removed, like a chip, which leaves the screen with
// no other trace (`announceRemoved`).
fn announce_removed(
    label: &SharedString,
    announcement: Option<&ListAnnouncementFn>,
    window: &mut Window,
    cx: &mut App,
) {
    let change = ListAnnouncement::Removed { label: label.clone() };
    let message = match announcement {
        Some(announcement) => announcement(&change),
        None => Some(describe_list(&change)),
    };
    if let Some(message) = message
        && !message.is_empty()
    {
        announce(&message, window, cx);
    }
}

impl<T: ComboboxItemValue> Combobox<T> {
    pub fn new(options: ComboboxOptions<T>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A change's source follows how the window was used (`inputSource`).
        track_input(window, cx);
        let mut instance = Instance::new("combobox").with_agent(cx);
        instance.debug = options.debug;
        let multiple = options.multiple;
        let starting = options
            .value
            .clone()
            .or_else(|| options.default_value.clone())
            .unwrap_or_else(|| ListValue::empty(multiple));
        let initial = options
            .default_value
            .clone()
            .unwrap_or_else(|| ListValue::empty(multiple));
        let mut value = Controllable::new("value", starting.clone()).after_update();
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());
        let query_text = options
            .query
            .clone()
            .or_else(|| options.default_query.clone())
            .unwrap_or_else(|| initial_query(&options, &starting, None));
        let mut query = Controllable::new("query", query_text.clone());
        query.set_controlled(options.query.clone());
        query.set_on_change(options.on_query_change.clone());

        let field = cx.new(|cx| TextField::new(field_options(&options, &query_text, options.disabled), window, cx));
        let field_handle = field.read(cx).focus_handle(cx);
        let internals = ElementInternals::new(cx);
        internals.set_focus_handle(Some(field_handle.clone()));
        internals.set_blocks_implicit_submission(true);

        let trigger_focus = cx.focus_handle();
        let clear_focus = cx.focus_handle();
        let chips_focus = cx.focus_handle();
        let regions = ControlRegions {
            input: Region::with_focus_handle(field_handle.clone()),
            chips: Region::with_focus_handle(chips_focus.clone()),
            trigger: Region::with_focus_handle(trigger_focus.clone()),
            clear: Region::with_focus_handle(clear_focus.clone()),
            label: Region::new(),
        };
        let popup = create_popup(
            &instance,
            CreatePopupOptions::new(combobox_preset())
                .rules(Self::rules(&instance, &regions))
                .options(popup_options(&options)),
            window,
            cx,
        );

        let listbox = create_filtered_listbox(Self::list_config(), window, cx);
        let selection = create_value_selection(
            ValueSelectionConfig::new(
                listbox.clone(),
                |this: &Self| this.value.get().as_slice(),
                |this: &Self| this.multiple,
                |this: &Self| ValueOptions {
                    item_to_value: this.options.item_to_value.clone(),
                    item_to_label: this.options.item_to_label.clone(),
                    is_item_equal_to_value: this.options.is_item_equal_to_value.clone(),
                },
            ),
            cx,
        );

        let this = cx.entity();
        let subscriptions = vec![
            cx.subscribe_in(&field, window, |this: &mut Self, _, event: &TextFieldEvent, window, cx| {
                this.field_event(event, window, cx)
            }),
            focus_events::on_focus(&field_handle, window, cx, |this: &mut Self, window, cx| {
                this.input_focused(window, cx)
            }),
            focus_events::on_blur(&field_handle, window, cx, |this: &mut Self, window, cx| {
                this.input_blurred(window, cx)
            }),
            cx.observe_keystrokes(Self::keystroke),
            associate(
                &internals,
                Association::new()
                    .reset(cx, |this: &mut Self, window, cx| this.reset(window, cx))
                    .disabled(cx, |this: &mut Self, disabled, window, cx| {
                        this.set_form_disabled(disabled, window, cx)
                    }),
            ),
            // Its parts coming and going, like the list part or a label,
            // change what the input says.
            cx.observe_in(&this, window, |this: &mut Self, _, _window, cx| this.sync_field(cx)),
        ];

        let status = options.status;
        let items = options.items.clone();
        let listed = (items.clone(), 0, query.get().clone());
        let previous_value = value.get().clone();
        let load_more = create_load_more(
            |this: &Self| this.listbox.count(this),
            |this: &Self| this.status,
            |this: &Self| this.options.on_load_more.clone() as Option<LoadMoreFn>,
        );
        let combobox = Self {
            instance,
            popup,
            field,
            internals,
            listbox,
            selection,
            value,
            initial,
            query,
            dirty: false,
            status,
            items,
            multiple,
            max_selected: options.max_selected,
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            name: options.name.clone(),
            form: options.form.clone(),
            form_disabled: false,
            pending: false,
            error: None,
            labels: 0,
            mounts: 0,
            prepared: false,
            focus_in_list: false,
            visible_chips: None,
            modality: None,
            field_name: None,
            settled: ListSettled::new(),
            scroller: ListScroller::default(),
            item_texts: ItemTexts::new(),
            icons: item_icons_feature(option_icon("combobox"), cx),
            trigger_focus,
            clear_focus,
            chips_focus,
            input_in_content: false,
            input_bound: false,
            create_props: RefCell::new(ComboboxCreateProps::default()),
            chip_parts: Parts::new(),
            chips: ChipsOverflow::new(),
            regions,
            sheet_input: bind_sheet_input(),
            shown: Some(query_text),
            mounts_queued: false,
            opened_by: PopupOpener::Imperative,
            committing: false,
            pending_highlight: None,
            placed: None,
            settling: false,
            highlighted_seen: None,
            shown_query: None,
            session: 0,
            cleared_value: None,
            listed,
            previous_value,
            announce_pending: AnnouncePending::new(),
            results: announce_results(
                AnnounceResultsOptions {
                    query: Rc::new(|this: &Self| this.filter_query()),
                    count: Rc::new(|this: &Self| this.listbox.count(this)),
                    loading: Rc::new(|this: &Self| this.status == AsyncOptionsStatus::Loading),
                    shown: Rc::new(|this: &Self| this.popup.is_open()),
                    silent: Rc::new(|this: &Self| held(&this.instance, "status", None)),
                    announcement: Rc::new(|this: &Self| this.options.announcement.clone() as Option<ListAnnouncementFn>),
                },
                window,
                cx,
            ),
            load_more,
            _subscriptions: subscriptions,
            options,
        };
        combobox.sync_form();
        combobox
    }

    // What the engine reads from the combobox.
    fn list_config() -> ListboxEngineConfig<Self, T> {
        ListboxEngineConfig::new(|this: &Self, value: &T| this.key_of(value))
            .focus(|this| {
                if this.focus_in_list {
                    ListFocus::Real
                } else {
                    ListFocus::Virtual
                }
            })
            .items(|this| this.items.clone())
            .empty_items(|this| this.options.empty_items.clone())
            // While the input is empty, not while it shows the value's label.
            .show_empty(|this| js_trim(this.query.get()).is_empty())
            .filter(|this| Some(this.options.filter.clone()))
            .rank(|this| this.options.filter.is_custom())
            .query(|this| this.filter_query())
            .r#loop(|this| this.options.r#loop)
            .skip_disabled(true)
            .active(|this| this.popup.is_open())
            .blocked(|this, entry| {
                !entry.pinned && this.max_reached() && !this.selection.is_selected(this, &entry.key, entry.value.as_ref())
            })
            .on_highlight(|this, key, index, event, window, cx| this.highlight_moved(key, index, event, window, cx))
            .on_last_reached(|this, window, cx| this.last_reached(window, cx))
            .container(|this| Some(this.popup.content_focus_handle()))
            .scroll_root(|this| Some(this.scroller.clone()))
            // Parts that depend on the list, like the empty message, groups
            // and the active descendant, update once per batch of items.
            .on_register(|this, _key, window, cx| this.items_registered(window, cx))
    }

    // How the list binds, as the lists crate binds one: a sheet opens at half
    // the window at most, and grows for the keyboard. Focus stays in the
    // input, unless a screen reader's tap moved it into the list. An arrow
    // key that opened the list highlights now.
    fn content_bind(&self) -> SheetConfig<Self> {
        let anchor = {
            let handle = self.instance.handle();
            let regions = self.regions.clone();
            Rc::new(move || Some(Anchor::element(anchor_region(&handle, &regions)))) as Rc<dyn Fn() -> Option<Anchor>>
        };
        let entity = self.popup.layer().entity();
        let initial_focus = Rc::new(move |window: &mut Window, cx: &mut App| {
            entity
                .update(cx, |this, cx| this.initial_focus(window, cx))
                .ok()
                .flatten()
        }) as ui_sdk_popups::InitialFocusFn;
        list_content_config(&self.popup, &self.instance, Some(anchor), None, Some(initial_focus))
    }

    // The popup's rules beyond the preset's.
    fn rules(instance: &Instance, regions: &ControlRegions) -> PopupRules<Self> {
        let handle = instance.handle();
        let anchor_regions = regions.clone();
        let contains_regions = regions.clone();
        PopupRules {
            // The chips when they're in the page, else the input.
            trigger: Some(Rc::new(move || Some(anchor_region(&handle, &anchor_regions)))),
            // Focus is in the input already.
            return_focus: Some(false),
            contains: Some(Rc::new(move |target, _event, window, _cx| contains_regions.owns(target, window))),
            on_open_change: Some(Rc::new(|this: &mut Self, open, closed, window, cx| {
                this.open_changed(open, closed, window, cx)
            })),
            on_show: Some(Rc::new(|this: &mut Self, _layer, _window, _cx| {
                this.modality = Some(track_modality(modality_of(this.opened_by.as_str())));
                Some(Box::new(|this: &mut Self, _window, _cx| this.modality = None))
            })),
            open: Some(Rc::new(|this: &mut Self, request, step, window, cx| {
                if !this.editable() {
                    return;
                }
                // A touch open never focuses the popup, so the on-screen
                // keyboard stays, unless it opens a sheet, which takes focus
                // so the keyboard stays closed.
                let sheet = this.resolves_sheet(window);
                this.opened_by = match request.request.by {
                    Some(PopupOpener::Touch) if !sheet => PopupOpener::Pointer,
                    by => by.unwrap_or(PopupOpener::Imperative),
                };
                // A touch screen reader's tap on a trigger moves real focus
                // into the list, since it ignores the active descendant. A
                // tap on the input is for typing.
                let from_input = request
                    .request
                    .extra
                    .as_ref()
                    .is_some_and(|extra| extra.downcast_ref::<FromInput>().is_some());
                this.focus_in_list = opens_into_list(request.request.event.as_ref()) && !from_input;
                let by = this.opened_by;
                step.open(this, Some(by), window, cx);
            })),
            // Unless it left, or a screen reader moved it into the list,
            // which hands it back to the input.
            close: Some(Rc::new(|this: &mut Self, _request, _reason, step, window, cx| {
                let in_list = this.focus_in_list && this.popup.content_region().contains_focused(window, cx);
                step.close(this, window, cx);
                if in_list && !this.popup.is_open() {
                    let handle = this.field.read(cx).focus_handle(cx);
                    restore_focus(&handle, window, cx);
                }
            })),
            bind: Some(Rc::new(|this: &Self| this.content_bind())),
            ..PopupRules::default()
        }
    }
}

// The element the list is placed against and nests under: the chips when
// they're in the page, else the input (`anchorElement`).
fn anchor_region(handle: &ui_sdk_kernel::InstanceHandle, regions: &ControlRegions) -> Region {
    if handle.bound_parts().iter().any(|part| part.slot == "chips") {
        regions.chips.clone()
    } else {
        regions.input.clone()
    }
}

// The popup's options from the combobox's.
fn popup_options<T: ComboboxItemValue>(options: &ComboboxOptions<T>) -> PopupOptions<()> {
    let mut popup = PopupOptions::default();
    popup.open = options.open;
    popup.default_open = options.default_open;
    if let Some(callback) = options.on_open_change.clone() {
        popup = popup.on_open_change(move |open, details, window, cx| callback(open, details, window, cx));
    }
    popup.on_open_change_complete = options.on_open_change_complete.clone();
    popup.modal = options.modal;
    popup.presentation = options.presentation;
    popup.dismissible = options.dismissible;
    popup.on_dismiss_attempt = options.on_dismiss_attempt.clone();
    popup
}

impl<T: ComboboxItemValue> Combobox<T> {
    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A value or a query passed to control it shows in the input
    /// at once, so it takes the window.
    pub fn set_options(&mut self, options: ComboboxOptions<T>, window: &mut Window, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        let query_before = self.query.get().clone();
        // A controlled close clears the highlight in the same pass.
        if derive_open(options.open, self.popup.is_open(), |open| !open).unwrap_or(false) {
            self.listbox.set_highlighted(None);
            self.focus_in_list = false;
            self.track_highlight();
        }
        // A list that starts or stops holding several values keeps what was
        // chosen, in its new shape, unless the value is controlled.
        if options.multiple != self.multiple {
            self.initial = self.initial.clone().shaped(options.multiple);
            if options.value.is_none() {
                let shaped = self.value.get().clone().shaped(options.multiple);
                self.value.set_held(shaped);
            }
        }
        // A value from options shows its label in the same pass, and items
        // that load after the value, like from a request, put its label in
        // place of the raw value the input showed.
        if !self.dirty && !options.multiple && options.query.is_none() && !self.committing {
            let shown = self.query.get().clone();
            if let Some(value) = options.value.as_ref().filter(|value| *value != self.value.get()) {
                // Items that arrive with the value name it, not the ones they replace.
                let rendered = |key: &SharedString| self.listbox.label_of(self, key);
                let label = initial_query(&options, value, Some(&rendered));
                if label != shown {
                    self.shown_query = Some((shown, label.clone()));
                    self.query.set_held(label);
                }
            } else if !same_items(&options.items, &self.items)
                && let Some(first) = self.value.get().first().cloned()
                && shown == item_key(&first, options.item_to_value.as_deref())
            {
                let label = initial_query(&options, &ListValue::Single(Some(first)), None);
                self.query.set_held(label);
            }
        }
        let auto_changed = options.auto_highlight != self.options.auto_highlight;
        let mut changed = self.multiple != options.multiple
            || self.max_selected != options.max_selected
            || self.disabled != options.disabled
            || self.read_only != options.read_only
            || self.required != options.required
            || self.name != options.name
            || self.form != options.form
            || self.status != options.status
            || !same_items(&self.items, &options.items);
        self.multiple = options.multiple;
        self.max_selected = options.max_selected;
        self.disabled = options.disabled;
        self.read_only = options.read_only;
        self.required = options.required;
        self.name = options.name.clone();
        self.form = options.form.clone();
        self.status = options.status;
        self.items = options.items.clone();
        if let Some(initial) = &options.default_value {
            self.initial = initial.clone();
        }
        changed |= self.value.set_controlled(options.value.clone());
        self.value.set_on_change(options.on_value_change.clone());
        self.value.set_on_before_change(options.on_before_value_change.clone());
        changed |= self.query.set_controlled(options.query.clone());
        self.query.set_on_change(options.on_query_change.clone());
        changed |= self.popup.set_options(popup_options(&options), cx);
        self.options = options;
        // What a value from options asks of the query reaches
        // `on_query_change` once the options are handed over, so the app's
        // callback doesn't run inside its render. Its label already shows.
        cx.defer_in(window, |this, window, cx| this.value_arrived(window, cx));
        self.list_changed(window, cx);
        // Turned on after typing, `auto_highlight` highlights the best match
        // once the options are handed over.
        if auto_changed {
            cx.defer_in(window, |this, window, cx| {
                if this.popup.is_open() {
                    this.settle_highlight(false, window, cx);
                }
            });
        }
        self.sync_input(window, cx);
        self.sync_field(cx);
        self.sync_form();
        if changed || self.query.get() != &query_before {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The native field the input part renders, like to read where its
    /// caret is.
    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// The listbox engine its items register with.
    pub fn listbox(&self) -> &ListboxEngine<Self, T> {
        &self.listbox
    }

    /// The selection model over its value.
    pub fn selection(&self) -> &ValueSelection<Self, T> {
        &self.selection
    }

    /// Its part in forms.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    /// The key a value is identified by and submits as: `item_to_value`'s,
    /// else the value's own [`ItemKey`] (`keyOf`).
    pub fn key_of(&self, value: &T) -> SharedString {
        item_key(value, self.options.item_to_value.as_deref())
    }

    // Its own `disabled`, or its Field's, which disables the control it
    // holds.
    pub(crate) fn own_disabled(&self) -> bool {
        self.disabled || self.internals.control_attrs().disabled
    }

    // Disabled by itself, its Field or a fieldset.
    pub(crate) fn is_disabled(&self) -> bool {
        self.own_disabled() || self.form_disabled
    }

    // The name the value submits under: its own, or its Field's.
    pub(crate) fn submit_name(&self) -> Option<SharedString> {
        self.name.clone().or_else(|| self.internals.control_attrs().name)
    }

    // Disabled or read-only: nothing can be cleared or removed (`isLocked`).
    pub(crate) fn is_locked(&self) -> bool {
        self.is_disabled() || self.read_only
    }

    pub(crate) fn editable(&self) -> bool {
        !self.is_disabled() && !self.read_only
    }

    pub(crate) fn max_reached(&self) -> bool {
        self.multiple
            && self
                .max_selected
                .is_some_and(|max| self.value.get().as_slice().len() >= max)
    }

    /// The text the list filters by. A single combobox showing its value's
    /// label shows every item until the user types (`filterQuery`).
    pub fn filter_query(&self) -> SharedString {
        if self.multiple || self.dirty {
            self.query.get().clone()
        } else {
            SharedString::default()
        }
    }

    // The value typed text becomes, with `allow_custom_value`.
    pub(crate) fn custom_value_of(&self, text: &SharedString) -> Option<T> {
        match &self.options.custom_value {
            Some(custom_value) => Some(custom_value(text)),
            None => text_value(text),
        }
    }

    pub(crate) fn show(&mut self, request: ShowRequest<()>, window: &mut Window, cx: &mut Context<Self>) {
        let control = self.popup.control().clone();
        control.show(self, request, None, window, cx);
    }

    pub(crate) fn hide(&mut self, reason: Reason, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let control = self.popup.control().clone();
        control.hide(self, HideRequest::new().reason(reason).event(event), window, cx)
    }

    // A change to the text (`setQuery`). `dirty` says the user typed it.
    pub(crate) fn change_query(
        &mut self,
        query: SharedString,
        reason: Reason,
        event: Option<InputEvent>,
        dirty: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.dirty = dirty;
        let options = ChangeOptions {
            reason: Some(reason),
            event,
            source: None,
        };
        let changed = self.query.change(&self.instance, query, options, window, cx);
        self.sync_input(window, cx);
        self.list_changed(window, cx);
        cx.notify();
        changed
    }

    // A label a value from options showed in its own pass is past refusing,
    // so `on_query_change` hears it, and canceling keeps the text
    // (`reportQuery`).
    fn report_query(&mut self, from: SharedString, to: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let source = resolve_source(Reason::Imperative, None);
        let details = ChangeDetails::new(Reason::Imperative, source, None);
        log_with(&self.instance, "query", &to, Reason::Imperative, Some(source));
        if let Some(callback) = self.options.on_query_change.clone() {
            with_source(source, || callback(&to, &details, window, cx));
        }
        watchers::emit_change_of(&self.instance, "query", &to, &from, &details, false, self.query.is_controlled());
    }

    // Sets the value for a choice. The caller sets the text, so the value
    // arriving doesn't put the label in with `Imperative` (`commit`).
    fn commit(&mut self, value: ComboboxValue<T>, reason: Reason, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.committing = true;
        let done = self.change_value(value, reason, event, window, cx);
        self.committing = false;
        done
    }

    fn selected_label(&self) -> SharedString {
        match self.value.get().first() {
            Some(value) => self.selection.label_of(self, value),
            None => SharedString::default(),
        }
    }

    // Text nobody chose becomes the value, with `allow_custom_value`
    // (`commitCustom`).
    pub(crate) fn commit_custom(&mut self, reason: Reason, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let query: SharedString = js_trim(self.query.get()).to_string().into();
        if query.is_empty() {
            return false;
        }
        let Some(value) = self.custom_value_of(&query) else {
            return false;
        };
        let key = self.key_of(&value);
        self.listbox.remember(key.clone(), query.clone(), value.clone(), None);
        if self.multiple {
            if !self.selection.is_selected(self, &key, Some(&value)) && !self.max_reached() {
                let mut values = self.selection.values(self);
                values.push(value);
                self.change_value(ListValue::Multiple(values), Reason::InputChange, event.clone(), window, cx);
            }
            if !self.options.keep_query {
                self.change_query(SharedString::default(), reason, event, false, window, cx);
            }
            return true;
        }
        if self.commit(ListValue::Single(Some(value)), Reason::InputChange, event.clone(), window, cx) {
            self.dirty = false;
        } else {
            let label = self.selected_label();
            self.change_query(label, reason, event, false, window, cx);
        }
        true
    }

    // Closing with text nobody chose puts the input back: the value's label,
    // or nothing. With `allow_custom_value`, a single combobox keeps the text
    // as its value (`revertQuery`).
    fn revert_query(&mut self, reason: Reason, window: &mut Window, cx: &mut Context<Self>) {
        let cleared = self.cleared_value.take();
        let (dirty, multiple) = (self.dirty, self.multiple);
        if !multiple && dirty && self.options.allow_custom_value && self.commit_custom(reason, None, window, cx) {
            return;
        }
        if multiple {
            if !self.options.keep_query && !self.query.get().is_empty() {
                self.change_query(SharedString::default(), reason, None, false, window, cx);
            }
            return;
        }
        if !dirty {
            return;
        }
        if let Some(cleared) = cleared
            && reason == Reason::EscapeKey
            && self.value.get().is_empty()
        {
            self.commit(cleared, Reason::EscapeKey, None, window, cx);
        }
        let label = self.selected_label();
        self.change_query(label, reason, None, false, window, cx);
    }

    // A change to the value (`setValue`): refused while disabled or
    // read-only, except by a form reset, and heard by the form as the user's
    // change.
    pub(crate) fn change_value(
        &mut self,
        value: ComboboxValue<T>,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if reason != Reason::Reset && (self.is_disabled() || self.read_only) {
            return false;
        }
        let options = ChangeOptions {
            reason: Some(reason),
            event,
            source: None,
        };
        let done = self.value.change(&self.instance, value, options, window, cx);
        self.value_arrived(window, cx);
        if done && reason != Reason::Reset && reason != Reason::Restore {
            dispatch_change(Some(self.internals.node()), INPUT_AND_CHANGE, window, cx);
        }
        self.sync_form();
        cx.notify();
        done
    }

    // What `auto_highlight` highlights: the first match while there's text,
    // an item over a create option, and without text the selected item, or
    // with `Always` the first. `None` when it highlights nothing of its own
    // (`autoTarget`).
    fn auto_target(&self) -> Option<Option<SharedString>> {
        let mode = self.options.auto_highlight;
        let entries = self.listbox.entries(self);
        let list: Vec<_> = entries
            .iter()
            .filter(|entry| !is_select_all(&entry.key) && !entry.disabled)
            .collect();
        let items: Vec<_> = list.iter().filter(|entry| !entry.pinned).collect();
        if !self.filter_query().is_empty() {
            return mode.is_on().then(|| {
                items
                    .first()
                    .map(|entry| entry.key.clone())
                    .or_else(|| list.first().map(|entry| entry.key.clone()))
            });
        }
        if !matches!(mode, ComboboxAutoHighlight::Always | ComboboxAutoHighlight::Selected) {
            return None;
        }
        let selected = items
            .iter()
            .find(|entry| self.selection.is_selected(self, &entry.key, entry.value.as_ref()));
        if mode == ComboboxAutoHighlight::Selected {
            return selected.map(|entry| Some(entry.key.clone()));
        }
        Some(selected.or(items.first()).map(|entry| entry.key.clone()))
    }

    // After the text or the list changed. A highlight the user moved stays
    // while its item is there, like through more items loading at the end,
    // and one `auto_highlight` placed follows the list, like async results
    // arriving (`settleHighlight`).
    pub(crate) fn settle_highlight(&mut self, fresh: bool, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.listbox.highlighted();
        let kept = current
            .as_ref()
            .is_some_and(|current| self.listbox.entries(self).iter().any(|entry| &entry.key == current));
        let target = self.auto_target();
        let follows = target.is_some() && (fresh || !kept || current == self.placed);
        let next = match (&target, follows) {
            (Some(target), true) => target.clone(),
            _ if kept => current.clone(),
            _ => None,
        };
        self.settling = true;
        if next != current {
            let listbox = self.listbox.clone();
            listbox.highlight(self, next, HighlightOptions::default(), window, cx);
        }
        self.settling = false;
        if follows && let Some(target) = target {
            self.placed = target;
        }
    }

    /// Chooses the item with `key`, as a press on it does: it becomes the
    /// value, or toggles in it with `multiple`, the input shows its label,
    /// and a single combobox closes. The create and select-all options run
    /// their own (`choose`).
    pub fn choose(&mut self, key: SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if key.as_ref() == CREATE {
            return self.create_item(event, window, cx);
        }
        if is_select_all(&key) {
            if self.editable() {
                self.toggle_all(&key, event, window, cx);
            }
            return;
        }
        let Some(entry) = self.listbox.find(self, Some(&key)) else {
            return;
        };
        let Some(value) = entry.value.clone() else {
            return;
        };
        if self.listbox.disabled(self, &entry) || !self.editable() {
            return;
        }
        let query = self.query.get().clone();
        self.listbox
            .remember(key.clone(), entry.label.clone(), value.clone(), None);
        if self.multiple {
            // A canceled choice leaves the text, which is what the user is
            // filtering with.
            let toggled = self.selection.toggled(self, &key, &value);
            if !self.change_value(ListValue::Multiple(toggled), Reason::ItemPress, event.clone(), window, cx) {
                return;
            }
            if !self.options.keep_query && !query.is_empty() {
                self.change_query(SharedString::default(), Reason::ItemPress, event, false, window, cx);
            } else {
                self.dirty = false;
            }
            return;
        }
        let chosen = self.commit(ListValue::Single(Some(value)), Reason::ItemPress, event.clone(), window, cx);
        let label = if chosen { entry.label.clone() } else { self.selected_label() };
        self.change_query(label, Reason::ItemPress, event.clone(), false, window, cx);
        self.hide(Reason::ItemPress, event, window, cx);
    }

    /// Takes a value out, as a chip's remove part does, and says so to
    /// screen readers, since a chip leaves no trace on screen (`remove`).
    pub fn remove(&mut self, value: &T, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        let key = self.key_of(value);
        let without = self.selection.without(self, &key);
        let next = ListValue::from_values(without, self.multiple);
        if self.change_value(next, Reason::RemovePress, event, window, cx) {
            let label = self.selection.label_of(self, value);
            let announcement = self.options.announcement.clone();
            announce_removed(&label, announcement.as_ref(), window, cx);
        }
    }

    // Whether the create option shows: there's text enough, the user typed
    // it, and no item's label is exactly it, among all data items or the
    // composed ones that match (`createShown`).
    pub(crate) fn create_shown(&self, props: &ComboboxCreateProps) -> bool {
        let query = js_trim(self.query.get());
        if self.is_locked() {
            return false;
        }
        if query.chars().count() < props.min_length.max(1) {
            return false;
        }
        if !self.multiple && !self.dirty {
            return false;
        }
        let candidates = self
            .listbox
            .data_items(self)
            .unwrap_or_else(|| self.listbox.entries(self));
        item_labelled(query, &candidates).is_none()
    }

    // Runs the create option's `on_create` with the text, pending while its
    // work runs. The list closes, or the input empties with `multiple`, once
    // it's done (`create`).
    pub(crate) fn create_item(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let props = self.create_props.borrow().clone();
        if !self.create_shown(&props) {
            return;
        }
        let started = self.session;
        let asked = self.query.get().clone();
        let text: SharedString = js_trim(&asked).to_string().into();
        let source = resolve_source(Reason::ItemPress, event.as_ref());
        let on_create = props.on_create.clone();
        let run = move |_details: &ui_sdk_kernel::ActionDetails, window: &mut Window, cx: &mut App| match &on_create {
            Some(on_create) => on_create(&text, window, cx),
            None => ActionOutcome::Done,
        };
        self.error = None;
        let done_event = event.clone();
        run_action(
            self,
            &run,
            source,
            ActionHandlers {
                pending: Box::new(|this: &mut Self, pending, window, cx| this.set_pending(pending, window, cx)),
                done: Box::new(move |this: &mut Self, window, cx| this.created(started, &asked, done_event.clone(), window, cx)),
                failed: Box::new(|this: &mut Self, error, _window, cx| {
                    this.error = Some(error);
                    cx.notify();
                }),
            },
            window,
            cx,
        );
        cx.notify();
    }

    fn created(&mut self, started: u64, asked: &SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.multiple {
            if started == self.session {
                self.hide(Reason::ItemPress, event, window, cx);
            }
        } else if !self.options.keep_query && self.query.get() == asked {
            self.change_query(SharedString::default(), Reason::ItemPress, event, false, window, cx);
        }
    }

    // The create option going pending or settling, which screen readers hear
    // after a second (`announcePending`).
    fn set_pending(&mut self, pending: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending == pending {
            return;
        }
        self.pending = pending;
        log_with(&self.instance, "pending", &pending, Reason::ItemPress, None);
        let text = self.options.pending_announcement.clone();
        let silent = text.as_ref().is_some_and(|text| text.is_empty());
        self.announce_pending.update(
            pending,
            text,
            move |text, window, cx| {
                if !silent {
                    announce(text, window, cx);
                }
            },
            window,
            cx,
        );
        cx.notify();
    }

    // The user typed: the text filters, the list opens, and an emptied
    // single combobox selects nothing (`typed`).
    pub(crate) fn typed(&mut self, text: SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        let multiple = self.multiple;
        if !self.change_query(text.clone(), Reason::InputChange, event.clone(), true, window, cx) {
            // Canceled: the input shows the text it had.
            let query = self.query.get().clone();
            if self.field.read(cx).value() != &query {
                self.shown = Some(query.clone());
                self.field.update(cx, |field, cx| field.set_value(query, window, cx));
            }
        }
        if !multiple
            && text.is_empty()
            && let Some(selected) = self.value.get().first().cloned()
            && self.change_value(ListValue::Single(None), Reason::InputChange, event.clone(), window, cx)
        {
            self.cleared_value = Some(ListValue::Single(Some(selected)));
        }
        if !self.popup.is_open() {
            // Items that mount with the list get the first highlight once
            // they have.
            if self.options.auto_highlight.is_on() && !text.is_empty() {
                self.pending_highlight = Some(Edge::First);
            }
            self.show(
                ShowRequest::new()
                    .event(event)
                    .by(PopupOpener::Keyboard)
                    .reason(Reason::InputChange),
                window,
                cx,
            );
        }
        self.settle_highlight(true, window, cx);
    }
}

// The item among `candidates` whose label is the text, ignoring case and
// accents (`itemLabelled`).
pub(crate) fn item_labelled<T: Clone>(query: &str, candidates: &[ListEntry<T>]) -> Option<ListEntry<T>> {
    let needle = normalize(query);
    candidates
        .iter()
        .find(|entry| !entry.pinned && normalize(&entry.label) == needle)
        .cloned()
}

// Data compares by identity, as the original's store does.
fn same_items<T>(a: &Option<ListData<T>>, b: &Option<ListData<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.ptr_eq(b),
        _ => false,
    }
}

impl<T: ComboboxItemValue> Combobox<T> {
    /// Whether the list is open (`isOpen`).
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// Opens or closes the list. Returns false when the change was canceled
    /// or it was already open or closed (`setOpen`).
    pub fn set_open(&mut self, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.set_open(self, open, options, window, cx)
    }

    /// Opens the list, and resolves once it closes, with why and how.
    /// Resolves exactly once: on the next close from any source, when its
    /// signal aborts, or with `Unmount` when the combobox leaves (`open`).
    pub fn open(&mut self, options: PopupOpenOptions<()>, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        let popup = self.popup.clone();
        popup.open(self, options, window, cx)
    }

    /// Closes the list. Returns false when the close was canceled or it was
    /// already closed (`close`).
    pub fn close(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.close(self, options.into(), window, cx)
    }

    /// Opens the list when it's closed, and closes it when it's open.
    /// Returns false when the change was canceled (`toggle`).
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.toggle(self, options, window, cx)
    }

    /// The selected value (`getValue`).
    pub fn get_value(&self) -> &ComboboxValue<T> {
        self.value.get()
    }

    /// Selects a value, as a user would. A disabled or read-only combobox
    /// ignores it. Returns false when the change was canceled or nothing
    /// changed (`setValue`). Reasons default to `Imperative`.
    pub fn set_value(
        &mut self,
        value: impl Into<ComboboxValue<T>>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.change_value(value.into(), reason, options.event, window, cx)
    }

    /// Selects nothing and empties the input. Returns false when the change
    /// was canceled or nothing changed (`clear`).
    pub fn clear(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.editable() {
            return false;
        }
        let reason = options.reason.unwrap_or(Reason::Imperative);
        let selected = !self.value.get().is_empty();
        // A canceled change keeps the value, and the input keeps its label.
        if selected && !self.change_value(ListValue::empty(self.multiple), reason, options.event.clone(), window, cx) {
            return false;
        }
        self.cleared_value = None;
        let query_reason = if reason == Reason::ClearPress { Reason::ClearPress } else { Reason::Imperative };
        let emptied = self.change_query(SharedString::default(), query_reason, options.event, false, window, cx);
        selected || emptied
    }

    /// The input's text (`getQuery`).
    pub fn get_query(&self) -> &SharedString {
        self.query.get()
    }

    /// Changes the input's text, which filters the list. Returns false when
    /// the change was canceled or nothing changed (`setQuery`).
    pub fn set_query(
        &mut self,
        query: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        let changed = self.change_query(query.into(), reason, options.event, true, window, cx);
        self.settle_highlight(true, window, cx);
        changed
    }

    /// The value of the highlighted item, or `None` when none is, or when
    /// it's the create or select-all option (`getHighlighted`).
    pub fn get_highlighted(&self) -> Option<T> {
        let key = self.listbox.highlighted();
        highlighted_value(&self.listbox, self, key.as_deref())
    }

    /// Highlights the item with this value in the open list, scrolled into
    /// view, or none with `None`. Returns false when the list is closed, or
    /// no enabled item has the value (`highlight`).
    pub fn highlight(&mut self, value: Option<&T>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.popup.is_open() {
            return false;
        }
        let key = value.map(|value| self.key_of(value));
        let current = self.listbox.highlighted();
        let listbox = self.listbox.clone();
        highlight_from_code(&listbox, self, key, current.as_ref(), window, cx)
    }

    /// Moves focus to the input (`focus`).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.field.read(cx).focus_handle(cx);
        if !self.is_disabled() {
            window.focus(&handle, cx);
        }
    }

    // The highlight moved, by the engine or the combobox: one
    // `auto_highlight` didn't place is the user's, and `on_highlight` hears
    // where it went.
    fn highlight_moved(
        &mut self,
        key: Option<&SharedString>,
        index: Option<usize>,
        event: Option<&InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.track_highlight();
        let Some(on_highlight) = self.options.on_highlight.clone() else {
            return;
        };
        let value = key
            .filter(|key| !key.starts_with('\u{0}'))
            .and_then(|key| self.listbox.value_of(self, key));
        let details = ListHighlightDetails {
            index,
            event: event.cloned(),
        };
        on_highlight(value.as_ref(), &details, window, cx);
    }

    // The highlight as it is now, for a move nobody reported.
    pub(crate) fn track_highlight(&mut self) {
        let now = self.listbox.highlighted();
        if now != self.highlighted_seen {
            self.highlighted_seen = now;
            if !self.settling {
                self.placed = None;
            }
        }
    }

    fn last_reached(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let load_more = self.load_more.clone();
        load_more.reached(self, window, cx);
    }

    // Items joined or left the list, which the engine says once the frame
    // that rendered them is drawn: parts that depend on the list update once
    // per batch (`onRegister`).
    fn items_registered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mounts_queued {
            return;
        }
        self.mounts_queued = true;
        cx.defer_in(window, |this, window, cx| {
            this.mounts_queued = false;
            this.mounts += 1;
            this.list_changed(window, cx);
            cx.notify();
        });
    }

    // What follows the list or the text changing: a label that arrived
    // shows, the highlight settles once the options that brought the items
    // or a controlled query are handed over, and screen readers hear the
    // count once typing pauses.
    fn list_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let labels = self.listbox.labels_version();
        if labels != self.labels {
            self.labels = labels;
            self.show_arrived_label(window, cx);
        }
        let listed = (self.items.clone(), self.mounts, self.query.get().clone());
        let same = same_items(&listed.0, &self.listed.0) && listed.1 == self.listed.1 && listed.2 == self.listed.2;
        if !same {
            self.listed = listed;
            cx.defer_in(window, |this, window, cx| {
                if this.popup.is_open() {
                    this.settle_highlight(false, window, cx);
                }
            });
        }
        self.results.changed(self, cx);
    }

    // A label that arrives after the value, like from items that render
    // later, replaces the raw value an uncontrolled input showed
    // (`showArrivedLabel`).
    fn show_arrived_label(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(first) = self.value.get().first().cloned() else {
            return;
        };
        if self.multiple || self.dirty || self.options.query.is_some() {
            return;
        }
        let label = self.selection.label_of(self, &first);
        let query = self.query.get().clone();
        if query == self.key_of(&first) && label != query {
            self.query.set_held(label);
            self.sync_input(window, cx);
            cx.notify();
        }
    }

    /// A value set from outside, like the api or a controlled option, shows
    /// its label in a single combobox's input, unless the user is typing
    /// (the original's value subscriber). Call it after the value changes.
    pub(crate) fn value_arrived(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.value.get() == &self.previous_value {
            return;
        }
        self.previous_value = self.value.get().clone();
        // Values whose items never rendered, like from the api: render them
        // hidden so they say their labels.
        if self.selection.needs_items(self, false)
            && self.items.is_none()
            && self.options.item_to_label.is_none()
            && !self.prepared
        {
            self.prepared = true;
            cx.notify();
        }
        if let Some((from, to)) = self.shown_query.take()
            && self.query.get() == &to
        {
            self.report_query(from, to, window, cx);
            self.sync_input(window, cx);
            return;
        }
        if self.multiple || self.dirty || self.committing {
            return;
        }
        let label = match self.value.get().first().cloned() {
            Some(first) => self.selection.label_of(self, &first),
            None => SharedString::default(),
        };
        if self.query.get() == &label {
            return;
        }
        self.change_query(label, Reason::Imperative, None, false, window, cx);
    }

    // The list started opening or closing. Closing forgets the highlight
    // and puts back text nobody chose, with the reason it closed.
    fn open_changed(&mut self, open: bool, closed: Option<&PopupClose>, window: &mut Window, cx: &mut Context<Self>) {
        self.field.update(cx, |field, cx| field.set_expanded(open, cx));
        if open {
            return;
        }
        self.session += 1;
        self.pending_highlight = None;
        self.placed = None;
        if self.listbox.set_highlighted(None) {
            cx.notify();
        }
        self.track_highlight();
        self.focus_in_list = false;
        if let Some(closed) = closed {
            let reason = closed.reason;
            self.revert_query(reason, window, cx);
        }
    }

    // Where focus goes once the list opens: it stays in the input, unless a
    // screen reader's tap moved it into the list, to the selected item or
    // the first. An arrow key that opened the list highlights now
    // (`initialFocus`).
    fn initial_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<FocusHandle> {
        let target = self.pending_highlight.take();
        if !self.focus_in_list {
            let mode = self.options.auto_highlight;
            if mode == ComboboxAutoHighlight::Always
                || (mode == ComboboxAutoHighlight::Selected && self.auto_target().is_some())
            {
                self.settle_highlight(false, window, cx);
            } else if target == Some(Edge::First) && mode.is_on() && !self.filter_query().is_empty() {
                self.settle_highlight(true, window, cx);
            } else if let Some(target) = target {
                let listbox = self.listbox.clone();
                listbox.r#move(self, target.step(), None, window, cx);
            }
            // In a sheet the input moved into its header, and loses focus on
            // the way if the sheet showed later. It takes focus back here.
            return (self.popup.presentation_value() == Presentation::Sheet).then(|| self.field.read(cx).focus_handle(cx));
        }
        let entries = self.listbox.entries(self);
        let list: Vec<_> = entries
            .iter()
            .filter(|entry| !entry.pinned && !entry.disabled)
            .collect();
        let item = list
            .iter()
            .find(|entry| self.selection.is_selected(self, &entry.key, entry.value.as_ref()))
            .or(list.first())
            .map(|entry| entry.key.clone());
        let Some(item) = item else {
            return Some(self.popup.content_focus_handle());
        };
        let listbox = self.listbox.clone();
        listbox.highlight(self, Some(item.clone()), HighlightOptions::without_focus(None), window, cx);
        Some(
            self.listbox
                .element(&item)
                .and_then(|element| element.handle)
                .unwrap_or_else(|| self.popup.content_focus_handle()),
        )
    }

    // What the field tells once its update is over, which the original hears
    // with listeners on the input.
    fn field_event(&mut self, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let TextFieldEvent::Input(edit) = event {
            let value = edit.value.clone();
            // Typing reports how it was typed, though no event explains it.
            with_source(edit.source, || self.typed(value, None, window, cx));
        }
    }

    fn input_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet_input.focused();
        // A field taking focus in a sheet grows it to its largest point, for
        // the keyboard.
        if self.popup.presentation_value() == Presentation::Sheet
            && let Some(sheet) = self.popup.sheet()
        {
            let points = sheet.snap_points();
            if points.len() > 1
                && let Some(largest) = points.last()
            {
                let point = largest.value.clone();
                sheet.snap_to(self, &point, SheetSnapReason::Imperative, window, cx);
            }
        }
    }

    // Focus left the input: the list closes, unless focus went to the
    // control around it or into the list (`focusout`).
    fn input_blurred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = window.focused(cx);
        self.sheet_input.blurred(next.as_ref());
        if !self.popup.is_open() {
            return;
        }
        // A sheet stays open when the keyboard goes away, with what was typed.
        if next.is_none() && self.popup.presentation_value() == Presentation::Sheet {
            return;
        }
        if let Some(next) = &next {
            let target = Target::Focus(next.clone());
            if self.owns(&target, window) || self.popup.content_region().contains(&target, window) {
                return;
            }
        }
        // Focus leaving the window, like to another app, is the layer's to decide.
        if next.is_none() && !window.is_window_active() {
            return;
        }
        let event = InputEvent::Focus(ui_sdk_kernel::modality::window_source(window));
        self.hide(Reason::FocusOut, Some(event), window, cx);
    }

    // Whether a press or focus lands on the control around the list: its
    // input, chips, trigger, clear button or label (`ownsElement`).
    pub(crate) fn owns(&self, target: &Target, window: &Window) -> bool {
        self.regions.owns(target, window)
    }

    // Enter in the input submits its form implicitly, the browser's own
    // action for a native input: once every listener had the key and none
    // prevented it.
    fn keystroke(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.action.is_some() || window.default_prevented() || Key::of(&event.keystroke) != Key::Enter {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || (!ui_sdk_platform::is_apple(cx) && modifiers.alt) {
            return;
        }
        let field = self.field.read(cx);
        if !field.is_focused(window) || field.is_composing() || field.is_disabled() {
            return;
        }
        let form = self.internals.form();
        submit_implicitly(form.as_ref(), window, cx);
    }

    // A form reset puts the default value back, with its label (`reset`).
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if keeps_value_on_reset(self.options.value.as_ref(), self.options.default_value.as_ref()) {
            return;
        }
        let initial = self.initial.clone();
        self.change_value(initial.clone(), Reason::Reset, None, window, cx);
        let label = match initial.first() {
            Some(first) if !self.multiple => self.selection.label_of(self, first),
            _ => SharedString::default(),
        };
        self.change_query(label, Reason::Reset, None, false, window, cx);
        self.sync_form();
    }

    fn set_form_disabled(&mut self, disabled: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if self.form_disabled != disabled {
            self.form_disabled = disabled;
            self.sync_field(cx);
            cx.notify();
        }
    }

    // Writes the query into the field when it changed. Text on its way to
    // the owner of a controlled query is ahead of it, and writing the old
    // text back would move the caret to the end.
    pub(crate) fn sync_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.get().clone();
        if self.shown.as_ref() == Some(&query) {
            return;
        }
        self.shown = Some(query.clone());
        self.field.update(cx, |field, cx| {
            if field.value() != &query {
                field.set_value(query, window, cx);
            }
        });
    }

    // Hands the field what the combobox decides of it: disabled by itself
    // or a fieldset, read-only, required, the input's attributes, and
    // whether the list is open, like one that starts open or is controlled.
    pub(crate) fn sync_field(&self, cx: &mut App) {
        let disabled = self.is_disabled();
        let default = self.field.read(cx).default_value().clone();
        let options = field_options(&self.options, &default, disabled);
        let open = self.popup.is_open();
        self.field.update(cx, |field, cx| {
            field.set_options(options, cx);
            field.set_expanded(open, cx);
        });
    }

    // What the form reads of the value, through the internals, as the
    // original's hidden inputs (`hiddenInputPart`, `valueInputPart`).
    pub(crate) fn sync_form(&self) {
        let keys = self.selection.keys(self);
        ui_sdk_lists::hidden_input_part(&self.internals, &self.form_state(), &keys);
    }

    // A select-all option was chosen: its items join the value, or leave it
    // once they all are in it.
    fn toggle_all(&mut self, key: &SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let limit = if self.multiple { self.max_selected } else { None };
        let value = SelectAll {
            listbox: &self.listbox,
            selection: &self.selection,
            limit,
        }
        .toggled(self, key);
        if let Some(value) = value {
            self.change_value(ListValue::Multiple(value), Reason::ItemPress, event, window, cx);
        }
    }

    // The pointer highlights what it moves onto (`hover`).
    pub(crate) fn hover(&mut self, key: &SharedString, event: InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.popup.is_open() || self.listbox.is_item_disabled(self, key) {
            return;
        }
        if self.listbox.highlighted().as_ref() != Some(key) {
            let listbox = self.listbox.clone();
            listbox.highlight(self, Some(key.clone()), HighlightOptions::event(Some(event)), window, cx);
        }
    }

    // Whether the list would present as a sheet if it opened now.
    pub(crate) fn resolves_sheet(&self, window: &Window) -> bool {
        self.popup
            .presentation()
            .is_some_and(|presentation| presentation.resolve(self, window) == Presentation::Sheet)
    }

    // Whether the input sits outside the content, so a trigger beside it can
    // leave the tab order (`inlineInput`).
    pub(crate) fn inline_input(&self) -> bool {
        held(&self.instance, "input", None) && !self.input_in_content
    }

    // The input rendered: the list learns what names it, a value whose label
    // only its item knows renders the list hidden, focus comes back to an
    // input that moved into a sheet's header, and development builds check
    // it's named.
    pub(crate) fn input_rendered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.field.read(cx).focus_handle(cx);
        let inside = self.popup.content_region().contains_focus(&handle, window);
        let mut changed = inside != self.input_in_content;
        self.input_in_content = inside;
        let own = self.instance.element_id("label", None);
        let labelled_by = self.internals.aria().labelled_by;
        let aria_label = name_of(&self.instance, "input", None).and_then(|name| name.aria_label);
        let next = ui_sdk_lists::field_name_of(&labelled_by, &own, &[], aria_label.as_ref());
        changed |= ui_sdk_lists::bind_field_name(&mut self.field_name, next);
        if !self.input_bound {
            self.input_bound = true;
            if self.items.is_none()
                && self.options.item_to_label.is_none()
                && self.selection.needs_items(self, self.has_value_icon())
                && !self.prepared
            {
                self.prepared = true;
                changed = true;
            }
            ui_sdk_lists::warn_if_unnamed(
                |this: &Self| &this.instance,
                "input",
                None,
                "The input has no accessible name. Add a label part, a Field label or an aria_label.",
                |this: &Self, _cx| !labels_of(this.internals.node()).is_empty(),
                window,
                cx,
            );
        }
        self.sheet_input
            .rendered(&handle, self.popup.presentation_value(), window, cx);
        if changed {
            cx.notify();
        }
    }

    // Whether a value icon rendered, which copies the selected item's icon.
    pub(crate) fn has_value_icon(&self) -> bool {
        held(&self.instance, "value-icon", None)
    }

    // The visible items as data, for rendering a virtualized list.
    pub(crate) fn visible_items(&self) -> ListData<T> {
        self.listbox
            .visible_data(self)
            .map(|visible| visible.list)
            .unwrap_or_default()
    }

    pub(crate) fn item_state(&self, key: &SharedString, props: &ComboboxItemProps<T>) -> ComboboxItemState {
        let selected = self.selection.is_selected(self, key, Some(&props.value));
        let max_reached = !selected && self.max_reached();
        ComboboxItemState {
            highlighted: self.listbox.highlighted().as_ref() == Some(key),
            selected,
            disabled: props.disabled || max_reached,
            max_reached,
        }
    }

    // The chips that don't fit (`overflowState`).
    pub(crate) fn overflow_state(&self) -> ComboboxChipsOverflowState {
        let labels: Vec<SharedString> = match self.visible_chips {
            Some(visible) => self.selection.labels(self).iter().skip(visible).cloned().collect(),
            None => Vec::new(),
        };
        ComboboxChipsOverflowState {
            count: labels.len(),
            labels,
        }
    }

    // Decides how many chips fit, once the frame that drew the overflow part
    // is over (`bindChipsOverflow`'s `decide`).
    pub(crate) fn decide_chips(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chips: Vec<SharedString> = self.selection.keys(self).to_vec();
        let visible = self.chips.decide(&chips, &self.instance.bound_parts(), window);
        if visible != self.visible_chips {
            self.visible_chips = visible;
            cx.notify();
        }
    }
}
