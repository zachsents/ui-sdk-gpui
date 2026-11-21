//! Port of `components/select/select-api.ts`: select's state and api:
//! opening and closing, choosing items, the value and its labels, and what
//! the form asks of it.

use std::cell::{Cell, RefCell};
use std::fmt::Debug;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, MouseMoveEvent, Pixels, Point,
    ScrollHandle, SharedString, Size, Subscription, WeakEntity, Window, point, px, size,
};
use ui_sdk_forms::{
    Association, ElementInternals, INPUT_AND_CHANGE, associate, dispatch_change, is_disabled, keeps_value_on_reset,
};
use ui_sdk_kernel::bound_keys::bound;
use ui_sdk_kernel::focus_events::on_focus_in;
use ui_sdk_kernel::modality::{Modality, ModalityTracker, modality_of, move_focus, track_modality};
use ui_sdk_kernel::{ChangeOptions, Controllable, InputEvent, Instance, Reason};
use ui_sdk_lists::async_options::AsyncOptionsStatus;
use ui_sdk_lists::item_icon::{ItemIconsFeature, item_icons_feature, option_icon};
use ui_sdk_lists::listbox::create_filtered_listbox;
use ui_sdk_lists::listbox_engine::{
    HighlightOptions, ItemKey, ListData, ListEntry, ListFilter, ListFocus, ListHighlightDetails, ListScroller,
    ListboxEngine, ListboxEngineConfig, item_key, reveal_in_list,
};
use ui_sdk_lists::listbox_keys::{is_select_all, option_id};
use ui_sdk_lists::listbox_parts::{FormState, hidden_input_part};
use ui_sdk_lists::listbox_popup::{ListSettled, list_content_config};
use ui_sdk_lists::listbox_search::{
    AnnounceResults, AnnounceResultsOptions, ItemTexts, ListAnnouncementFn, LoadMore, LoadMoreFn, announce_results,
    create_load_more, opens_into_list,
};
use ui_sdk_lists::listbox_select_all::SelectAll;
use ui_sdk_lists::listbox_value::{ListValue, ValueOptions, ValueSelection, ValueSelectionConfig, create_value_selection};
use ui_sdk_native::{TextField, TextFieldOptions};
use ui_sdk_platform::Anchor;
use ui_sdk_popups::{
    CreatePopupOptions, HideRequest, InitialFocusFn, Popup, PopupCloseOptions, PopupHost, PopupOpenOptions,
    PopupOpener, PopupOptions, PopupPending, PopupRules, PopupTriggerProps, Presentation, ReturnMode, SheetConfig,
    ShowRequest, WeakPopup, create_popup, opener_of, select_preset,
};
use ui_sdk_text::js_trim;

use crate::select::{
    HighlightCallback, ItemEqualFn, ItemToLabelFn, ItemToValueFn, SelectOptions, SelectValue, SelectValueProps,
};

/// Which item takes the highlight once the list opens (`InitialHighlight`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum InitialHighlight {
    /// The selected item, or the first that can be chosen.
    #[default]
    Selected,
    /// The first item that can be chosen.
    First,
    /// The last item that can be chosen.
    Last,
}

/// The trigger's key among the popup's triggers. A select has one.
pub(crate) const TRIGGER: &str = "trigger";

/// A request to open (`Show`): the event behind it, how it opens, why, and
/// which item takes the highlight.
#[derive(Clone, Debug, Default)]
pub(crate) struct Show {
    pub(crate) event: Option<InputEvent>,
    pub(crate) by: Option<PopupOpener>,
    pub(crate) reason: Option<Reason>,
    pub(crate) focus: Option<InitialHighlight>,
}

/// Where the selected item sits below the content's top, which lines it up
/// over the trigger: measured once per open, so scrolling the list doesn't
/// move it, or off when it can't line up.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Alignment {
    #[default]
    Pending,
    Measured(Pixels),
    Off,
}

/// What the popup's placement reads on every frame, which can't reach the
/// select itself: the popup, and how the selected item lines up over the
/// trigger.
pub(crate) struct Placement<T: ItemKey + Clone + PartialEq + Debug + 'static> {
    pub(crate) popup: RefCell<Option<WeakPopup<Select<T>>>>,
    /// The content's `align_item_with_trigger`, as it last rendered.
    pub(crate) enabled: Cell<bool>,
    pub(crate) alignment: Cell<Alignment>,
    /// The window's size as of the last frame.
    pub(crate) viewport: Cell<Option<Size<Pixels>>>,
}

impl<T: ItemKey + Clone + PartialEq + Debug + 'static> Placement<T> {
    // The anchor that puts the selected item over the trigger, or `None`
    // when it doesn't fit (`alignedAnchor`).
    pub(crate) fn aligned(&self) -> Option<Anchor> {
        if !self.enabled.get() {
            return None;
        }
        let Alignment::Measured(offset) = self.alignment.get() else {
            return None;
        };
        let popup = self.popup.borrow().as_ref()?.upgrade()?;
        if popup.layer().opener() == PopupOpener::Touch {
            return None;
        }
        let trigger = popup.trigger_element()?.bounds()?;
        let content = popup.content_region().bounds()?;
        let view = self.viewport.get()?;
        let top = trigger.top() + trigger.size.height / 2. - offset;
        if top < px(8.) || top + content.size.height > view.height - px(8.) {
            return None;
        }
        let rect = Bounds::new(point(trigger.left(), top), size(trigger.size.width, px(0.)));
        Some(Anchor::rect(move |_, _| Some(rect)))
    }
}

// A press on the trigger that opened or closed the list, until it ends.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Press {
    pub(crate) position: Point<Pixels>,
    pub(crate) time: Instant,
    pub(crate) opening: bool,
}

/// A select. Its entity is its controller.
pub struct Select<T: ItemKey + Clone + PartialEq + Debug + 'static = SharedString> {
    pub(crate) instance: Instance,
    pub(crate) popup: Popup<Self>,
    pub(crate) value: Controllable<SelectValue<T>>,
    // What a form reset returns to.
    pub(crate) initial: SelectValue<T>,
    // Whether the app passed a default value, which a form reset of a
    // controlled select needs.
    has_default: bool,
    pub(crate) query: Controllable<SharedString>,
    // The list is in the page, hidden, so its items register for typing on
    // the closed trigger.
    pub(crate) prepared: bool,
    pub(crate) multiple: bool,
    pub(crate) disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) name: Option<SharedString>,
    pub(crate) form: Option<SharedString>,
    pub(crate) auto_complete: Option<SharedString>,
    pub(crate) placeholder: Option<SharedString>,
    pub(crate) items: Option<ListData<T>>,
    pub(crate) empty_items: Option<ListData<T>>,
    pub(crate) filter: Option<ListFilter<T>>,
    pub(crate) status: AsyncOptionsStatus,
    pub(crate) on_load_more: Option<LoadMoreFn>,
    pub(crate) announcement: Option<ListAnnouncementFn>,
    pub(crate) item_to_value: Option<ItemToValueFn<T>>,
    pub(crate) item_to_label: Option<ItemToLabelFn<T>>,
    pub(crate) is_item_equal_to_value: Option<ItemEqualFn<T>>,
    pub(crate) on_highlight: Option<HighlightCallback<T>>,
    pub(crate) r#loop: bool,
    // Real focus moved into the list for a touch screen reader, instead of
    // staying in the search field.
    pub(crate) focus_in_list: bool,
    // Focus stays on the trigger while the list is open, which points at
    // the highlighted item.
    pub(crate) virtual_focus: bool,
    pub(crate) pending_focus: InitialHighlight,
    // How the list last opened, for the content's modality.
    pub(crate) opened_by: PopupOpener,
    // How the user works in the open list: the keyboard or a pointer.
    pub(crate) modality: Option<ModalityTracker>,
    // A sheet scrolls its selected item into view once its items rendered.
    pub(crate) reveal_pending: bool,
    // Where focus goes on open waits for the items to join the list.
    initial_pending: bool,
    pub(crate) placement: Rc<Placement<T>>,
    // A press opened the list, so the click that follows it is ignored.
    pub(crate) press_click: bool,
    pub(crate) press: Option<Press>,
    // The value as last seen, so a value from outside the select is told
    // apart.
    previous: SelectValue<T>,
    // Whether the trigger rendered once, which asks for the items of a value
    // only they can label.
    pub(crate) trigger_bound: bool,
    // Whether the search field rendered, which starts the check that
    // something names it.
    pub(crate) search_bound: bool,
    // The value part's props as it last rendered, which the trigger's value
    // reads too.
    pub(crate) value_props: RefCell<SelectValueProps>,
    // Whether a value icon part rendered, which shows the selected item's
    // icon.
    pub(crate) wants_value_icon: Cell<bool>,
    pub(crate) listbox: ListboxEngine<Self, T>,
    pub(crate) selection: ValueSelection<Self, T>,
    pub(crate) load_more: LoadMore<Self>,
    pub(crate) announcer: AnnounceResults<Self>,
    pub(crate) settled: ListSettled,
    pub(crate) item_texts: ItemTexts,
    pub(crate) icons: ItemIconsFeature,
    pub(crate) search: Entity<TextField>,
    // The search field's own options, which its typing joins.
    search_base: TextFieldOptions,
    pub(crate) trigger_handle: FocusHandle,
    pub(crate) clear_handle: FocusHandle,
    pub(crate) scroll_handle: ScrollHandle,
    pub(crate) internals: ElementInternals,
    pub(crate) entity: WeakEntity<Self>,
    window: AnyWindowHandle,
    _subscriptions: Vec<Subscription>,
}

impl<T: ItemKey + Clone + PartialEq + Debug + 'static> PopupHost for Select<T> {
    type Payload = ();

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

impl<T: ItemKey + Clone + PartialEq + Debug + 'static> Select<T> {
    pub fn new(options: SelectOptions<T>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("select").with_agent(cx);
        instance.debug = options.debug;
        let multiple = options.multiple;
        let empty = ListValue::empty(multiple);
        let first = options
            .value
            .clone()
            .or_else(|| options.default_value.clone())
            .unwrap_or_else(|| empty.clone());
        let mut value = Controllable::new("value", first.clone());
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        let mut query = Controllable::new(
            "query",
            options.query.clone().unwrap_or_else(|| options.default_query.clone()),
        );
        query.set_controlled(options.query.clone());
        query.set_on_change(options.on_query_change.clone());

        let config = ListboxEngineConfig::new(|this: &Self, value: &T| this.key_of(value))
            .focus(|this| {
                if this.is_virtual() {
                    ListFocus::Virtual
                } else {
                    ListFocus::Real
                }
            })
            .items(|this| this.items.clone())
            .empty_items(|this| this.empty_items.clone())
            // Typing on the closed trigger searches every item, so they show
            // only while searching.
            .show_empty(|this| this.searchable() && this.popup.is_mounted() && js_trim(this.query.get()).is_empty())
            .filter(|this| {
                this.searchable()
                    .then(|| this.filter.clone().unwrap_or(ListFilter::Contains))
            })
            .rank(|this| this.filter.as_ref().is_some_and(ListFilter::is_custom))
            .query(|this| this.filter_query())
            .r#loop(|this| this.r#loop)
            .skip_disabled(true)
            .active(|this| this.popup.is_open())
            .on_highlight(|this, key, index, event, window, cx| this.highlight_moved(key, index, event, window, cx))
            .on_last_reached(|this, window, cx| {
                let load_more = this.load_more.clone();
                load_more.reached(this, window, cx);
            })
            .container(|this| Some(this.popup.content_focus_handle()))
            .scroll_root(|this| Some(ListScroller::new(this.scroll_handle.clone())))
            .on_register(|this, _key, window, cx| this.initial_settled(window, cx));
        let listbox = create_filtered_listbox(config, window, cx);
        let selection = create_value_selection(
            ValueSelectionConfig::new(
                listbox.clone(),
                |this: &Self| this.value.get().as_slice(),
                |this| this.multiple,
                |this| ValueOptions {
                    item_to_value: this.item_to_value.clone(),
                    item_to_label: this.item_to_label.clone(),
                    is_item_equal_to_value: this.is_item_equal_to_value.clone(),
                },
            ),
            cx,
        );
        let load_more = create_load_more(
            |this: &Self| this.listbox.count(this),
            |this| this.status,
            |this| this.on_load_more.clone(),
        );

        let placement = Rc::new(Placement {
            popup: RefCell::new(None),
            enabled: Cell::new(false),
            alignment: Cell::new(Alignment::Pending),
            viewport: Cell::new(None),
        });
        let popup = create_popup(
            &instance,
            CreatePopupOptions::new(select_preset())
                .rules(Self::popup_rules())
                .options(Self::popup_options(&options)),
            window,
            cx,
        );
        *placement.popup.borrow_mut() = Some(popup.weak());
        // The trigger's focus handle exists before it renders, so the select
        // takes focus from the start.
        let (trigger_handle, _, _) = popup.bind_trigger(&TRIGGER.into(), &PopupTriggerProps::default(), cx);

        let internals = ElementInternals::new(cx);
        // A label pointing at the select, or a submit that finds it invalid,
        // focuses the trigger.
        internals.set_focus_handle(Some(trigger_handle.clone()));
        let entity = cx.weak_entity();
        let search = {
            let entity = entity.clone();
            let base = options.search.clone();
            cx.new(|cx| TextField::new(search_options(entity, base, options.disabled), window, cx))
        };
        let subscriptions = vec![
            // Focus on the trigger renders the list hidden, so typing finds
            // its items.
            on_focus_in(&trigger_handle, window, cx, |this: &mut Self, _window, cx| this.prepare(cx)),
            associate(
                &internals,
                Association::new().reset(cx, |this: &mut Self, window, cx| this.reset(window, cx)),
            ),
        ];

        let this = Self {
            instance,
            popup,
            value,
            initial: options.default_value.clone().unwrap_or(empty),
            has_default: options.default_value.is_some(),
            query,
            prepared: false,
            multiple,
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            name: options.name,
            form: options.form,
            auto_complete: options.auto_complete,
            placeholder: options.placeholder,
            items: options.items,
            empty_items: options.empty_items,
            filter: options.filter,
            status: options.status,
            on_load_more: options.on_load_more,
            announcement: options.announcement,
            item_to_value: options.item_to_value,
            item_to_label: options.item_to_label,
            is_item_equal_to_value: options.is_item_equal_to_value,
            on_highlight: options.on_highlight,
            r#loop: options.r#loop,
            focus_in_list: false,
            virtual_focus: options.virtual_focus,
            pending_focus: InitialHighlight::Selected,
            opened_by: PopupOpener::Imperative,
            modality: None,
            reveal_pending: false,
            initial_pending: false,
            placement,
            press_click: false,
            press: None,
            previous: first,
            trigger_bound: false,
            search_bound: false,
            value_props: RefCell::new(SelectValueProps::default()),
            wants_value_icon: Cell::new(false),
            listbox,
            selection,
            load_more,
            announcer: announce_results(
                AnnounceResultsOptions {
                    query: Rc::new(|this: &Self| this.filter_query()),
                    count: Rc::new(|this: &Self| this.listbox.count(this)),
                    loading: Rc::new(|this: &Self| this.status == AsyncOptionsStatus::Loading),
                    shown: Rc::new(|this: &Self| this.popup.is_open() && this.searchable()),
                    silent: Rc::new(|this: &Self| bound(&this.instance, "status")),
                    announcement: Rc::new(|this: &Self| this.announcement.clone()),
                },
                window,
                cx,
            ),
            settled: ListSettled::new(),
            item_texts: ItemTexts::new(),
            icons: item_icons_feature(option_icon("select"), cx),
            search,
            search_base: options.search,
            trigger_handle,
            clear_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            internals,
            entity,
            window: window.window_handle(),
            _subscriptions: subscriptions,
        };
        this.sync_form();
        this
    }

    // The options the popup reads from the select's.
    fn popup_options(options: &SelectOptions<T>) -> PopupOptions<()> {
        let on_open_change = options.on_open_change.clone();
        PopupOptions {
            open: options.open,
            default_open: options.default_open,
            on_open_change: on_open_change.map(|callback| {
                Rc::new(
                    move |open: &bool,
                          details: &ui_sdk_kernel::ChangeDetails,
                          _: Option<&SharedString>,
                          window: &mut Window,
                          cx: &mut App| { callback(open, details, window, cx) },
                ) as ui_sdk_popups::PopupOpenChangeCallback
            }),
            on_open_change_complete: options.on_open_change_complete.clone(),
            modal: options.modal,
            presentation: options.presentation,
            disabled: options.disabled,
            dismissible: options.dismissible,
            on_dismiss_attempt: options.on_dismiss_attempt.clone(),
            ..PopupOptions::default()
        }
    }

    // What only a select's popup does, beside its preset.
    fn popup_rules() -> PopupRules<Self> {
        PopupRules {
            dismiss: Some(Rc::new(|this: &mut Self, reason, details, window, cx| {
                // Tab: focus moves on from the trigger, like from a native select.
                if reason == Reason::FocusOut {
                    window.focus(&this.trigger_handle, cx);
                    this.hide(
                        HideRequest::new().reason(reason).event(details.event).focus(ReturnMode::Leave),
                        window,
                        cx,
                    );
                    return;
                }
                this.hide(
                    HideRequest {
                        reason: Some(reason),
                        event: details.event,
                        source: None,
                        focus: Some(details.focus),
                        return_value: details.return_value,
                    },
                    window,
                    cx,
                );
            })),
            on_open_change: Some(Rc::new(|this: &mut Self, open, _closed, _window, cx| {
                if open {
                    return;
                }
                this.placement.alignment.set(Alignment::Pending);
                this.listbox.reset_typing();
                this.listbox.set_highlighted(None);
                cx.notify();
            })),
            on_open_change_complete: Some(Rc::new(|this: &mut Self, _open, closed, window, cx| {
                // The search empties once the list has left, so it doesn't jump
                // while it animates out.
                let Some(closed) = closed else {
                    return;
                };
                this.focus_in_list = false;
                if !this.query.get().is_empty() {
                    this.change_query(SharedString::default(), closed.reason, None, window, cx);
                }
                cx.notify();
            })),
            on_show: Some(Rc::new(|this: &mut Self, _layer, _window, cx| {
                // A sheet starts with the selected item in view, whatever opened
                // it, like a native select's, since a tap focuses the sheet
                // rather than the item.
                if this.popup.presentation_value() == Presentation::Sheet {
                    this.reveal_pending = true;
                }
                this.modality = Some(track_modality(modality_of(this.opened_by.as_str())));
                cx.notify();
                Some(Box::new(|this: &mut Self, _window: &mut Window, cx: &mut Context<Self>| {
                    this.modality = None;
                    cx.notify();
                }) as ui_sdk_popups::ShowCleanup<Self>)
            })),
            open: Some(Rc::new(|this: &mut Self, request, step, window, cx| {
                if this.is_disabled() {
                    return;
                }
                this.pending_focus = request
                    .request
                    .extra
                    .as_ref()
                    .and_then(|extra| extra.downcast_ref::<InitialHighlight>())
                    .copied()
                    .unwrap_or_default();
                // A touch screen reader gets real focus in the list, since it
                // ignores the active descendant, as a key would, rather than
                // the popup a tap focuses.
                let event = request.request.event.as_ref();
                let into_list = opens_into_list(event);
                this.focus_in_list = into_list;
                this.opened_by = if into_list {
                    PopupOpener::Keyboard
                } else {
                    request.request.by.unwrap_or_else(|| opener_of(event))
                };
                let by = this.opened_by;
                step.open(this, Some(by), window, cx);
            })),
            bind: Some(Rc::new(|this: &Self| this.content_bind())),
            ..PopupRules::default()
        }
    }

    // How the content binds as the list opens: placed below the trigger, or
    // over it with the selected item on the trigger, and focus going where
    // the select says (`bindListContent`).
    fn content_bind(&self) -> SheetConfig<Self> {
        let entity = self.entity.clone();
        let initial_focus: InitialFocusFn = Rc::new(move |window: &mut Window, cx: &mut App| {
            entity.upgrade()?.update(cx, |this, cx| this.initial_focus(window, cx))
        });
        let trigger = self.popup.weak();
        let anchor: Rc<dyn Fn() -> Option<Anchor>> =
            Rc::new(move || trigger.upgrade()?.trigger_element().map(Anchor::element));
        let aligned: Rc<dyn Fn() -> Option<Anchor>> = {
            let placement = self.placement.clone();
            Rc::new(move || placement.aligned())
        };
        let mut config = list_content_config(&self.popup, &self.instance, Some(anchor), Some(aligned), Some(initial_focus));
        // A select's sheet rests at its own size.
        config.snap_points = None;
        config
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`syncSelectState`).
    pub fn set_options(&mut self, options: SelectOptions<T>, cx: &mut Context<Self>) {
        let before = self.popup.is_open();
        let mut changed = self.popup.set_options(Self::popup_options(&options), cx);
        // A controlled close clears the highlight in the same pass, and keeps
        // the list in the page hidden, which focus returning to the trigger
        // would ask for while the options are still being handed over.
        if let Some(false) = ui_sdk_popups::derive_open(options.open, before, |open| open) {
            self.listbox.set_highlighted(None);
            self.prepared = true;
            changed = true;
        }
        let multiple = options.multiple;
        // A list that starts or stops holding several values keeps what was
        // chosen, in its new shape, unless the value is controlled.
        if multiple != self.multiple {
            self.initial = self.initial.clone().shaped(multiple);
            if options.value.is_none() {
                let shaped = self.value.get().clone().shaped(multiple);
                self.value.set_held(shaped);
            }
        }
        changed |= self.value.set_controlled(options.value.clone());
        self.value.set_on_change(options.on_value_change.clone());
        // Like a native select's default, changing it changes what a reset
        // returns to.
        if let Some(default) = &options.default_value {
            self.initial = default.clone();
        }
        self.has_default = options.default_value.is_some();
        let query_changed = self.query.set_controlled(options.query.clone());
        self.query.set_on_change(options.on_query_change.clone());
        changed |= query_changed
            || self.multiple != multiple
            || self.disabled != options.disabled
            || self.read_only != options.read_only
            || self.required != options.required
            || self.name != options.name
            || self.form != options.form
            || self.auto_complete != options.auto_complete
            || self.placeholder != options.placeholder
            || self.items != options.items
            || self.empty_items != options.empty_items
            || self.filter != options.filter
            || self.status != options.status
            || self.r#loop != options.r#loop
            || self.virtual_focus != options.virtual_focus;
        if self.disabled != options.disabled || !same_search(&self.search_base, &options.search) {
            let entity = self.entity.clone();
            let disabled = options.disabled;
            let base = options.search.clone();
            self.search
                .update(cx, |field, cx| field.set_options(search_options(entity, base, disabled), cx));
        }
        self.search_base = options.search;
        self.multiple = multiple;
        self.disabled = options.disabled;
        self.read_only = options.read_only;
        self.required = options.required;
        self.name = options.name;
        self.form = options.form;
        self.auto_complete = options.auto_complete;
        self.placeholder = options.placeholder;
        self.items = options.items;
        self.empty_items = options.empty_items;
        self.filter = options.filter;
        self.status = options.status;
        self.on_load_more = options.on_load_more;
        self.announcement = options.announcement;
        self.item_to_value = options.item_to_value;
        self.item_to_label = options.item_to_label;
        self.is_item_equal_to_value = options.is_item_equal_to_value;
        self.on_highlight = options.on_highlight;
        self.r#loop = options.r#loop;
        self.virtual_focus = options.virtual_focus;
        self.instance.debug = options.debug;
        self.value_changed();
        self.sync_form();
        self.heard(cx);
        if query_changed {
            self.sync_search(cx);
        }
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The key of a value: `item_to_value`'s, else its own (`keyOf`).
    pub fn key_of(&self, value: &T) -> SharedString {
        match &self.item_to_value {
            Some(item_to_value) => item_to_value(value),
            None => item_key(value, None),
        }
    }

    /// Whether the select is disabled, by itself, its field or a fieldset
    /// (`isDisabled`). A field disables the select it's the control of, as
    /// the original's adapters pass the field's `disabled` into it.
    pub fn is_disabled(&self) -> bool {
        is_disabled(self.own_disabled(), self.internals.form_disabled())
    }

    // Disabled by itself or its field.
    pub(crate) fn own_disabled(&self) -> bool {
        self.disabled || self.internals.control_attrs().disabled
    }

    /// Whether it keeps its value but refuses changes.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether several items can be selected.
    pub fn is_multiple(&self) -> bool {
        self.multiple
    }

    /// How the list presents this time it's open.
    pub fn presentation(&self) -> Presentation {
        self.popup.presentation_value()
    }

    /// How the user works in the open list now, the content's
    /// `data-modality`.
    pub fn modality(&self) -> Option<Modality> {
        self.modality.as_ref().map(ModalityTracker::get)
    }

    // A search field is in the list, so typing filters and focus stays in
    // the field (`searchable`).
    pub(crate) fn searchable(&self) -> bool {
        bound(&self.instance, "input")
    }

    // Focus stays outside the list, in the search field or on the trigger,
    // while arrows move the highlight. A touch screen reader gets real focus
    // in the list (`virtual`).
    pub(crate) fn is_virtual(&self) -> bool {
        (self.searchable() || self.virtual_focus) && !self.focus_in_list
    }

    // The text the list is filtered by: the search field's, while the list
    // is in the page (`filterQuery`).
    pub(crate) fn filter_query(&self) -> SharedString {
        if self.searchable() && self.popup.is_mounted() {
            self.query.get().clone()
        } else {
            SharedString::default()
        }
    }

    // What the form reads (`formAttrs`).
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

    // Reports the value to the form, as the original's hidden inputs do:
    // the selected key under its name, or every key with `multiple`.
    pub(crate) fn sync_form(&self) {
        hidden_input_part(&self.internals, &self.form_state(), &self.selection.keys(self));
    }

    // What follows the state changing, as the original's store subscribers
    // hear it: the result count once typing pauses.
    fn heard(&self, cx: &App) {
        self.announcer.changed(self, cx);
    }

    // The search field shows the query, written once the current update is
    // over, since the field may be the one that changed it.
    pub(crate) fn sync_search(&self, cx: &mut Context<Self>) {
        let query = self.query.get().clone();
        let field = self.search.clone();
        let window = self.window;
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    field.update(cx, |field, cx| {
                        if *field.value() != query {
                            field.set_value(query, window, cx);
                        }
                    });
                })
                .ok();
        });
    }

    // Scrolls the selected item into view in the list (`revealSelected`).
    pub(crate) fn reveal_selected(&self) {
        let selected = self
            .listbox
            .entries(self)
            .iter()
            .find(|entry| self.selection.is_selected(self, &entry.key, entry.value.as_ref()))
            .map(|entry| entry.key.clone());
        if let Some(bounds) = selected.and_then(|key| self.listbox.bounds(&key)) {
            reveal_in_list(bounds, &ListScroller::new(self.scroll_handle.clone()));
        }
    }

    // Where focus goes once the list opens: the selected item, or the first
    // or last that can be chosen. With a search field, focus goes there and
    // the item is only highlighted (`initialFocus`). Composed items join the
    // list once the frame that rendered them is drawn, after the popup asks,
    // so then the highlight, and focus with it, waits for them.
    pub(crate) fn initial_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<FocusHandle> {
        if self.items.is_none() && self.listbox.entries(self).is_empty() && !self.settled.get() {
            self.initial_pending = true;
            return self.virtual_target(cx);
        }
        Some(self.resolve_initial(window, cx))
    }

    // Where focus stays while arrows move the highlight, when it does.
    fn virtual_target(&self, cx: &App) -> Option<FocusHandle> {
        if !self.is_virtual() {
            return None;
        }
        if self.searchable() {
            return Some(self.search.focus_handle(cx));
        }
        if self.virtual_focus {
            return Some(self.trigger_handle.clone());
        }
        Some(self.popup.content_focus_handle())
    }

    fn resolve_initial(&mut self, window: &mut Window, cx: &mut Context<Self>) -> FocusHandle {
        let listbox = self.listbox.clone();
        let list: Vec<ListEntry<T>> = listbox
            .entries(self)
            .iter()
            .filter(|entry| !entry.pinned && !listbox.disabled(self, entry))
            .cloned()
            .collect();
        let selected = list
            .iter()
            .find(|entry| self.selection.is_selected(self, &entry.key, entry.value.as_ref()));
        let target = match self.pending_focus {
            InitialHighlight::Last => list.last(),
            InitialHighlight::First => list.first(),
            InitialHighlight::Selected => selected.or(list.first()),
        }
        .cloned();
        self.pending_focus = InitialHighlight::Selected;
        if let Some(target) = &target {
            listbox.highlight(self, Some(target.key.clone()), HighlightOptions::without_focus(None), window, cx);
        }
        let content = self.popup.content_focus_handle();
        if let Some(handle) = self.virtual_target(cx) {
            return handle;
        }
        let Some(target) = target else {
            return content;
        };
        listbox
            .existing_focus_handle(&target.key)
            .filter(|_| listbox.is_connected(&target.key))
            .unwrap_or(content)
    }

    // The items the open list waited for have joined: the highlight goes
    // where it would have, and focus with it, unless it already moved into
    // the list.
    pub(crate) fn initial_settled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.initial_pending {
            return;
        }
        self.initial_pending = false;
        if !self.popup.is_open() {
            return;
        }
        let inside = self.popup.content_region().contains_focused(window, cx);
        let target = self.resolve_initial(window, cx);
        if !inside && !target.is_focused(window) {
            move_focus(&target, window, cx);
        }
    }

    // The search field's text changed (`setQuery`): the list filters, and the
    // best match is highlighted.
    pub(crate) fn change_query(
        &mut self,
        query: SharedString,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let options = ChangeOptions {
            reason: Some(reason),
            event: event.clone(),
            source: None,
        };
        if !self.query.change(&self.instance, query, options, window, cx) {
            return false;
        }
        cx.notify();
        self.heard(cx);
        self.sync_search(cx);
        if !self.popup.is_open() {
            return true;
        }
        let listbox = self.listbox.clone();
        let first = listbox.first_enabled(self);
        if listbox.highlighted() != first {
            listbox.highlight(self, first, HighlightOptions::without_focus(event), window, cx);
        }
        true
    }

    // Typing in the search field (`typed`).
    pub(crate) fn typed(&mut self, query: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.change_query(query, Reason::InputChange, None, window, cx);
    }

    // Asks for a value (`setValue`): a disabled or read-only select refuses
    // anything but a reset, and a change goes out to the form.
    pub(crate) fn change_value(
        &mut self,
        value: SelectValue<T>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        if reason != Reason::Reset && (self.is_disabled() || self.read_only) {
            return false;
        }
        let options = ChangeOptions {
            reason: Some(reason),
            ..options
        };
        let done = self.value.change(&self.instance, value, options, window, cx);
        if done {
            self.value_changed();
            self.sync_form();
            cx.notify();
            if reason != Reason::Reset && reason != Reason::Restore {
                dispatch_change(Some(self.internals.node()), INPUT_AND_CHANGE, window, cx);
            }
        }
        done
    }

    // Values whose items never rendered, like from a call: the list renders
    // hidden so they say their labels.
    fn value_changed(&mut self) {
        if *self.value.get() == self.previous {
            return;
        }
        self.previous = self.value.get().clone();
        if self.prepared || self.items.is_some() || self.item_to_label.is_some() {
            return;
        }
        if self.selection.needs_items(self, false) {
            self.prepared = true;
        }
    }

    // Chooses an item: selects it and closes, or toggles it with `multiple`.
    // A select-all option toggles every item it covers.
    pub(crate) fn choose(&mut self, key: &SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if is_select_all(key) {
            if !self.is_disabled() && !self.read_only {
                self.choose_all(key, event, window, cx);
            }
            return;
        }
        let listbox = self.listbox.clone();
        let Some(entry) = listbox.find(self, Some(key)) else {
            return;
        };
        if listbox.disabled(self, &entry) || self.is_disabled() || self.read_only {
            return;
        }
        let Some(value) = entry.value.clone() else {
            return;
        };
        listbox.remember(entry.key.clone(), entry.label.clone(), value.clone(), None);
        let why = ChangeOptions {
            reason: Some(Reason::ItemPress),
            event: event.clone(),
            source: None,
        };
        if self.multiple {
            let toggled = self.selection.toggled(self, key, &value);
            self.change_value(ListValue::Multiple(toggled), why, window, cx);
            return;
        }
        self.change_value(ListValue::Single(Some(value)), why, window, cx);
        self.hide(HideRequest::new().reason(Reason::ItemPress).event(event), window, cx);
    }

    // What pressing a select-all option does: its items added, or removed
    // once they're all selected.
    fn choose_all(&mut self, key: &SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let toggled = SelectAll {
            listbox: &self.listbox,
            selection: &self.selection,
            limit: None,
        }
        .toggled(self, key);
        if let Some(values) = toggled {
            let why = ChangeOptions {
                reason: Some(Reason::ItemPress),
                event,
                source: None,
            };
            self.change_value(ListValue::Multiple(values), why, window, cx);
        }
    }

    // The labels exist only for items that rendered: render the list hidden
    // so they do (`prepare`).
    pub(crate) fn prepare(&mut self, cx: &mut Context<Self>) {
        if !self.prepared {
            self.prepared = true;
            cx.notify();
        }
    }

    // The highlight moved.
    fn highlight_moved(
        &mut self,
        key: Option<&SharedString>,
        index: Option<usize>,
        event: Option<&InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        let Some(on_highlight) = self.on_highlight.clone() else {
            return;
        };
        let value = key
            .filter(|key| !is_select_all(key))
            .and_then(|key| self.listbox.value_of(self, key));
        let details = ListHighlightDetails {
            index,
            event: event.cloned(),
        };
        on_highlight(value.as_ref(), &details, window, cx);
    }

    // The pointer highlights what it moves onto, focusing it unless focus
    // stays in the search field (`hover`). gpui's mouse moves never come
    // from a finger.
    pub(crate) fn hover(&mut self, key: &SharedString, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let listbox = self.listbox.clone();
        if !self.popup.is_open() || listbox.is_item_disabled(self, key) {
            return;
        }
        let focused = self.is_virtual()
            || listbox
                .existing_focus_handle(key)
                .is_some_and(|handle| handle.is_focused(window));
        if listbox.highlighted().as_ref() != Some(key) || !focused {
            listbox.highlight(
                self,
                Some(key.clone()),
                HighlightOptions::event(Some(InputEvent::from(event))),
                window,
                cx,
            );
        }
    }

    // Whether no item matches. Composed items register once the content has
    // rendered (`isEmpty`).
    pub(crate) fn is_empty(&self) -> bool {
        (self.items.is_some() || self.settled.get()) && self.listbox.count(self) == 0
    }

    // The id the active descendant points at while focus stays in the search
    // field or on the trigger (`activeDescendant`).
    pub(crate) fn active_descendant(&self) -> Option<SharedString> {
        let highlighted = self.listbox.highlighted()?;
        if !self.popup.is_open() || self.focus_in_list || !self.listbox.is_connected(&highlighted) {
            return None;
        }
        Some(option_id(self.instance.id(), &highlighted))
    }

    // Lines the selected item up over the trigger, once the content and its
    // items have laid out, and keeps the window's size the placement reads.
    pub(crate) fn measure_alignment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.placement.viewport.set(Some(window.viewport_size()));
        // A list that rendered no items still settles where focus goes.
        if self.settled.get() {
            self.initial_settled(window, cx);
        }
        if self.reveal_pending && self.settled.get() {
            self.reveal_pending = false;
            self.reveal_selected();
        }
        if !self.placement.enabled.get() || self.placement.alignment.get() != Alignment::Pending || !self.popup.is_open() {
            return;
        }
        let selected = self
            .listbox
            .entries(self)
            .iter()
            .find(|entry| self.selection.is_selected(self, &entry.key, entry.value.as_ref()))
            .map(|entry| entry.key.clone());
        let touch = self.popup.layer().opener() == PopupOpener::Touch;
        let measured = match selected {
            _ if touch => Some(Alignment::Off),
            None => Some(Alignment::Off),
            Some(key) => match (self.listbox.bounds(&key), self.popup.content_region().bounds()) {
                (Some(item), Some(content)) => Some(Alignment::Measured(item.top() - content.top() + item.size.height / 2.)),
                // Items render with the content: one that hasn't by now won't
                // line up.
                _ if self.settled.get() => Some(Alignment::Off),
                _ => None,
            },
        };
        if let Some(alignment) = measured {
            self.placement.alignment.set(alignment);
            cx.notify();
        }
    }

    // Whether the content waits out of sight for the selected item to line
    // up over the trigger.
    pub(crate) fn aligning(&self) -> bool {
        self.placement.enabled.get()
            && self.placement.alignment.get() == Alignment::Pending
            && self.popup.is_open()
            && self.popup.layer().opener() != PopupOpener::Touch
            && !self.selection.keys(self).is_empty()
    }

    /// What a form reset does: back to `default_value`. A controlled select
    /// with no default keeps its value. It resets a disabled or read-only
    /// select too, as a native one.
    pub(crate) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let controlled = self.value.is_controlled().then_some(());
        let default = self.has_default.then_some(());
        if keeps_value_on_reset(controlled.as_ref(), default.as_ref()) {
            return;
        }
        let initial = self.initial.clone();
        self.change_value(initial, ChangeOptions::reason(Reason::Reset), window, cx);
    }

    // Opens the list (`show`).
    pub(crate) fn show(&mut self, show: Show, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        let mut request = ShowRequest::new().event(show.event);
        request.reason = show.reason;
        request.by = show.by;
        if let Some(focus) = show.focus {
            request = request.extra(focus);
        }
        let control = self.popup.control().clone();
        control.show(self, request, None, window, cx)
    }

    // Closes the list (`hide`).
    pub(crate) fn hide(&mut self, request: HideRequest, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let control = self.popup.control().clone();
        control.hide(self, request, window, cx)
    }

    /// Whether the list is open.
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// Opens or closes the list. Returns false when the change was canceled
    /// or it was already open or closed.
    pub fn set_open(&mut self, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.set_open(self, open, options, window, cx)
    }

    /// Opens the list, and resolves once it closes, with why and how. It
    /// resolves exactly once and never fails: on the next close from any
    /// source, when the signal aborts, or with `Unmount` when the select
    /// leaves the page. An already aborted signal never opens it.
    pub fn open(&mut self, options: PopupOpenOptions<()>, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        let popup = self.popup.clone();
        popup.open(self, options, window, cx)
    }

    /// Closes the list. Returns false when the close was canceled or it was
    /// already closed.
    pub fn close(&mut self, options: PopupCloseOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.close(self, options, window, cx)
    }

    /// Opens the list when it's closed, and closes it when it's open.
    /// Returns false when the change was canceled.
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.toggle(self, options, window, cx)
    }

    /// The selected value.
    pub fn value(&self) -> &SelectValue<T> {
        self.value.get()
    }

    /// Selects a value, as a user would. A disabled or read-only select
    /// ignores it. Returns false when the change was canceled, ignored or the
    /// value was already selected; for a controlled select, true means
    /// accepted.
    pub fn set_value(
        &mut self,
        value: impl Into<SelectValue<T>>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.change_value(value.into(), options, window, cx)
    }

    /// Selects nothing: `None`, or an empty list with `multiple`. Returns
    /// false when nothing changed.
    pub fn clear(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let empty = self.selection.empty(self);
        self.change_value(empty, options, window, cx)
    }

    /// The text in the search field.
    pub fn query(&self) -> &SharedString {
        self.query.get()
    }

    /// Changes the text in the search field, which filters the list.
    /// Returns false when nothing changed.
    pub fn set_query(
        &mut self,
        query: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.change_query(query.into(), reason, options.event, window, cx)
    }

    /// Moves focus to the trigger.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.trigger_handle, cx);
    }
}

impl<T: ItemKey + Clone + PartialEq + Debug + 'static> Focusable for Select<T> {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.trigger_handle.clone()
    }
}

// The search field's options: the app's, with its typing filtering the
// list, and disabled with the select.
fn search_options<T: ItemKey + Clone + PartialEq + Debug + 'static>(
    select: WeakEntity<Select<T>>,
    base: TextFieldOptions,
    disabled: bool,
) -> TextFieldOptions {
    let own = base.on_input.clone();
    let disabled = disabled || base.disabled;
    let mut options = base.disabled(disabled);
    options.on_input = Some(Rc::new(move |field: &mut TextField, edit, window, cx| {
        if let Some(own) = &own {
            own(field, edit, window, cx);
        }
        let query = field.value().clone();
        select.update(cx, |select, cx| select.typed(query, window, cx)).ok();
    }));
    options
}

// Whether the app handed the search field the same options again, as far as
// what it shows goes.
fn same_search(a: &TextFieldOptions, b: &TextFieldOptions) -> bool {
    a.placeholder == b.placeholder && a.disabled == b.disabled && a.default_value == b.default_value
}
