//! Port of `components/autocomplete/autocomplete-api.ts`: the
//! autocomplete's state and API, on the lists crate's autocomplete core: the
//! text, the popup, inline completion, choosing, submitting, and form
//! resets.
//!
//! The state is the [`Autocomplete`]'s fields and the API its methods. The
//! original's store subscribers, which work out what follows a change in the
//! same pass, are the calls each change makes as it happens: the core's
//! `observe` after the text, the items or the popup change, and
//! `value_arrived` after the value does.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, KeyDownEvent, KeystrokeEvent, SharedString,
    Subscription, Window,
};
use ui_sdk_focus::Step;
use ui_sdk_forms::{
    Association, ElementInternals, FormEventType, ValueInputOptions, ValueMissingKind, ValueSource, associate,
    bind_value_input, dispatch_change, keeps_value_on_reset, labels_of, notify_value, submit_implicitly,
};
use ui_sdk_kernel::modality::{ModalityTracker, modality_of, restore_focus, track_input, track_modality, window_source};
use ui_sdk_kernel::{
    ChangeCallback, ChangeOptions, Controllable, InputEvent, Instance, Key, Reason, flag_while_bound, focus_events,
    name_of, press_of, with_source,
};
use ui_sdk_lists::{
    AsyncOptionsStatus, AutoHighlight, AutocompleteCore, AutocompleteCoreConfig, AutocompleteCoreState, FieldName,
    HighlightCause, HighlightOptions, ItemIconsFeature, ItemTexts, ListData, ListEdge, ListEntry,
    ListHighlightDetails, ListScroller, ListSettled, ListboxEngine, create_autocomplete_core, highlight_from_code,
    highlighted_value, item_icons_feature, list_content_config, list_key_down, opens_into_list, option_icon,
};
use ui_sdk_motion::SheetSnapReason;
use ui_sdk_native::{Edit, InputType, SelectionDirection, TextField, TextFieldEvent, TextFieldOptions};
use ui_sdk_platform::{Anchor, Region, Target};
use ui_sdk_popups::{
    CreatePopupOptions, HideRequest, InitialFocusFn, Popup, PopupHost, PopupOpenOptions, PopupOpener, PopupOptions,
    PopupPending, PopupRules, Presentation, SheetConfig, SheetInput, ShowRequest, bind_sheet_input,
    combobox_preset, create_popup, derive_open, opener_of,
};
use ui_sdk_text::normalize;

use crate::autocomplete::{AutocompleteChosen, AutocompleteOptions, AutocompleteSubmitDetails};

// What a show from the field itself carries: a tap or a press on the field
// is for typing, so it never moves real focus into the list.
pub(crate) struct FromInput;

// The parts around the list. Presses on them don't dismiss it, and focus
// moving among them doesn't close it (`controlSlots`).
#[derive(Clone, Default)]
pub(crate) struct ControlRegions {
    pub(crate) input: Region,
    pub(crate) trigger: Region,
    pub(crate) clear: Region,
    pub(crate) label: Region,
}

impl ControlRegions {
    // Whether a press or focus lands on one of them (`ownsElement`).
    pub(crate) fn owns(&self, target: &Target, window: &Window) -> bool {
        [&self.input, &self.trigger, &self.clear, &self.label]
            .iter()
            .any(|region| region.contains(target, window))
    }
}

/// An autocomplete. Its entity is its controller.
pub struct Autocomplete {
    pub(crate) instance: Instance,
    pub(crate) popup: Popup<Self>,
    // The field, the native text field whose element is the `input` part.
    pub(crate) field: Entity<TextField>,
    pub(crate) internals: ElementInternals,
    pub(crate) core: AutocompleteCore<Self>,
    pub(crate) options: AutocompleteOptions,

    // The text, with an inline completion if one shows.
    pub(crate) value: Controllable<SharedString>,
    // What the user typed, which filters the list. It differs from the value
    // while an inline completion or a highlighted suggestion fills the
    // field.
    pub(crate) query: SharedString,
    // The value a form reset returns to.
    pub(crate) initial: SharedString,
    pub(crate) items: Option<ListData<SharedString>>,
    pub(crate) status: AsyncOptionsStatus,
    pub(crate) disabled: bool,
    pub(crate) form_disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) name: Option<SharedString>,
    pub(crate) form: Option<SharedString>,
    // Real focus moved into the list for a touch screen reader, instead of an
    // active descendant.
    pub(crate) focus_in_list: bool,
    // How the user works in the list while it shows (`trackModality`).
    pub(crate) modality: Option<ModalityTracker>,

    // What names the field when no label part does.
    pub(crate) field_name: Option<FieldName>,
    // The content has rendered with its items, so an empty list is really
    // empty.
    pub(crate) settled: ListSettled,
    // The element suggestions reveal in: the content.
    pub(crate) scroller: ListScroller,
    pub(crate) item_texts: ItemTexts,
    pub(crate) icons: ItemIconsFeature,
    pub(crate) trigger_focus: FocusHandle,
    pub(crate) clear_focus: FocusHandle,
    pub(crate) regions: ControlRegions,
    pub(crate) sheet_input: SheetInput,
    // The value the field was last brought up to (`shown`).
    shown: Option<SharedString>,
    input_bound: bool,

    // The text this autocomplete asked for last. Any other that arrives, like
    // a controlled value, also becomes what filters.
    requested: SharedString,
    opened_by: PopupOpener,
    // The highlight to place once the list has opened, after an arrow key
    // opened it.
    pending_highlight: Option<ListEdge>,
    // The value as `value_arrived` last heard it.
    previous_value: SharedString,
    // The suggestion a choice reports to `on_value_change`, while it asks.
    chosen: Rc<RefCell<Option<AutocompleteChosen>>>,
    _subscriptions: Vec<Subscription>,
}

impl PopupHost for Autocomplete {
    type Payload = ();

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

// The options the field takes from the autocomplete's: the input's
// attributes, with `autocomplete`, `autocapitalize` and `spellcheck` off.
fn field_options(options: &AutocompleteOptions, value: &SharedString, disabled: bool) -> TextFieldOptions {
    TextFieldOptions {
        default_value: value.clone(),
        placeholder: options.placeholder.clone(),
        disabled,
        read_only: options.read_only,
        required: options.required,
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

// The popup's options from the autocomplete's.
fn popup_options(options: &AutocompleteOptions) -> PopupOptions<()> {
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

// `on_value_change`, with the suggestion a choice reports (`details.item`).
fn value_callback(
    options: &AutocompleteOptions,
    chosen: &Rc<RefCell<Option<AutocompleteChosen>>>,
) -> Option<ChangeCallback<SharedString>> {
    let callback = options.on_value_change.clone()?;
    let chosen = chosen.clone();
    Some(Rc::new(move |value, details, window, cx| {
        let item = chosen.borrow().clone();
        callback(value, details, item.as_ref(), window, cx)
    }))
}

// The first suggestion whose label starts with the text, for inline
// completion (`completionFor`).
fn completion_for(
    text: &str,
    listbox: &ListboxEngine<Autocomplete>,
    this: &Autocomplete,
) -> Option<ListEntry<SharedString>> {
    let needle = normalize(text);
    if needle.is_empty() {
        return None;
    }
    listbox
        .entries(this)
        .iter()
        .find(|entry| !listbox.disabled(this, entry) && normalize(&entry.label).starts_with(needle.as_ref()))
        .cloned()
}

// The byte offset after the first `chars` characters of `text`.
fn char_offset(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(offset, _)| offset)
}

// Data compares by identity, as the original's store does.
fn same_items(a: &Option<ListData<SharedString>>, b: &Option<ListData<SharedString>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.ptr_eq(b),
        _ => false,
    }
}

impl Autocomplete {
    pub fn new(options: AutocompleteOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A change's source follows how the window was used (`inputSource`).
        track_input(window, cx);
        let mut instance = Instance::new("autocomplete").with_agent(cx);
        instance.debug = options.debug;
        let starting = options
            .value
            .clone()
            .or_else(|| options.default_value.clone())
            .unwrap_or_default();
        let chosen: Rc<RefCell<Option<AutocompleteChosen>>> = Rc::default();
        let mut value = Controllable::new("value", starting.clone());
        value.set_controlled(options.value.clone());
        value.set_on_change(value_callback(&options, &chosen));

        let field = cx.new(|cx| TextField::new(field_options(&options, &starting, options.disabled), window, cx));
        let field_handle = field.read(cx).focus_handle(cx);
        let internals = ElementInternals::new(cx);
        internals.set_focus_handle(Some(field_handle.clone()));
        internals.set_blocks_implicit_submission(true);

        let trigger_focus = cx.focus_handle();
        let clear_focus = cx.focus_handle();
        let regions = ControlRegions {
            input: Region::with_focus_handle(field_handle.clone()),
            trigger: Region::with_focus_handle(trigger_focus.clone()),
            clear: Region::with_focus_handle(clear_focus.clone()),
            label: Region::new(),
        };
        let popup = create_popup(
            &instance,
            CreatePopupOptions::new(combobox_preset())
                .rules(Self::rules(&regions))
                .options(popup_options(&options)),
            window,
            cx,
        );

        let core = create_autocomplete_core(Self::core_config(&instance), window, cx);

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
            // change what the field says, and the core looks at what else
            // changed, like the list settling.
            cx.observe_in(&this, window, |this: &mut Self, _, window, cx| {
                this.sync_field(cx);
                this.observe(window, cx);
            }),
        ];

        let autocomplete = Self {
            instance,
            popup,
            field,
            internals,
            core,
            value,
            query: starting.clone(),
            initial: options.default_value.clone().unwrap_or_default(),
            items: options.items.clone(),
            status: options.status,
            disabled: options.disabled,
            form_disabled: false,
            read_only: options.read_only,
            required: options.required,
            name: options.name.clone(),
            form: options.form.clone(),
            focus_in_list: false,
            modality: None,
            field_name: None,
            settled: ListSettled::new(),
            scroller: ListScroller::default(),
            item_texts: ItemTexts::new(),
            icons: item_icons_feature(option_icon("autocomplete"), cx),
            trigger_focus,
            clear_focus,
            regions,
            sheet_input: bind_sheet_input(),
            shown: Some(starting.clone()),
            input_bound: false,
            requested: starting.clone(),
            opened_by: PopupOpener::Imperative,
            pending_highlight: None,
            previous_value: starting,
            chosen,
            _subscriptions: subscriptions,
            options,
        };
        autocomplete.sync_form(cx);
        autocomplete
    }

    // What the shared core reads from the autocomplete.
    fn core_config(instance: &Instance) -> AutocompleteCoreConfig<Self> {
        AutocompleteCoreConfig {
            id: instance.id().clone(),
            state: Rc::new(|this: &Self| AutocompleteCoreState {
                open: this.popup.is_open(),
                status: this.status,
                settled: this.settled.get(),
                focus_in_list: this.focus_in_list,
            }),
            query: Rc::new(|this: &Self| this.query.clone()),
            items: Rc::new(|this: &Self| this.items.clone()),
            empty_items: Some(Rc::new(|this: &Self| this.options.empty_items.clone())),
            // Suggestions are text, so a filter for them reads every item the
            // engine passes.
            filter: Rc::new(|this: &Self| this.options.filter.clone()),
            auto_highlight: Rc::new(|this: &Self| {
                if this.options.auto_highlight || this.completes() {
                    AutoHighlight::Typing
                } else {
                    AutoHighlight::Off
                }
            }),
            highlight: Rc::new(|this: &mut Self, key, why, window, cx| this.core_highlight(key, why, window, cx)),
            preferred: Some(Rc::new(|this: &Self, listbox: &ListboxEngine<Self>| {
                if this.completes() {
                    completion_for(&this.query, listbox, this).map(|entry| entry.key)
                } else {
                    None
                }
            })),
            open: Some(Rc::new(|this: &mut Self, event: &KeyDownEvent, edge, window, cx| {
                if !event.keystroke.modifiers.alt {
                    this.pending_highlight = Some(edge);
                }
                this.show(
                    ShowRequest::new()
                        .event(Some(InputEvent::from(event)))
                        .by(PopupOpener::Keyboard),
                    window,
                    cx,
                );
            })),
            on_highlight: Some(Rc::new(|this: &mut Self, key, index, event, window, cx| {
                this.highlight_moved(key, index, event, window, cx)
            })),
            container: Some(Rc::new(|this: &Self| Some(this.popup.content_focus_handle()))),
            scroll_root: Some(Rc::new(|this: &Self| Some(this.scroller.clone()))),
            settles_mount: None,
            r#loop: Some(Rc::new(|this: &Self| this.options.r#loop)),
            on_load_more: Some(Rc::new(|this: &Self| this.options.on_load_more.clone())),
            announcement: Some(Rc::new(|this: &Self| this.options.announcement.clone())),
            silent: Some(Rc::new(|this: &Self| flag_while_bound(&this.instance, "status") == Some(true))),
        }
    }

    // How the list binds, as the lists crate binds one: a sheet opens at half
    // the window at most, and grows for the keyboard. Focus stays in the
    // field, unless a screen reader's tap moved it into the list. An arrow
    // key that opened the list highlights now.
    fn content_bind(&self) -> SheetConfig<Self> {
        let anchor = {
            let input = self.regions.input.clone();
            Rc::new(move || Some(Anchor::element(input.clone()))) as Rc<dyn Fn() -> Option<Anchor>>
        };
        let entity = self.popup.layer().entity();
        let initial_focus = Rc::new(move |window: &mut Window, cx: &mut App| {
            entity
                .update(cx, |this, cx| this.initial_focus(window, cx))
                .ok()
                .flatten()
        }) as InitialFocusFn;
        list_content_config(&self.popup, &self.instance, Some(anchor), None, Some(initial_focus))
    }

    // The popup's rules beyond the preset's.
    fn rules(regions: &ControlRegions) -> PopupRules<Self> {
        let input = regions.input.clone();
        let contains_regions = regions.clone();
        PopupRules {
            trigger: Some(Rc::new(move || Some(input.clone()))),
            // Focus stays in the field, whatever closes the list.
            return_focus: Some(false),
            dismiss: Some(Rc::new(|this: &mut Self, reason, details, window, cx| {
                this.hide(reason, details.event, window, cx);
            })),
            contains: Some(Rc::new(move |target, _event, window, _cx| contains_regions.owns(target, window))),
            on_open_change: Some(Rc::new(|this: &mut Self, open, _closed, window, cx| {
                this.open_changed(open, window, cx)
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
                // tap on the field is for typing.
                let from_input = request
                    .request
                    .extra
                    .as_ref()
                    .is_some_and(|extra| extra.downcast_ref::<FromInput>().is_some());
                this.focus_in_list = opens_into_list(request.request.event.as_ref()) && !from_input;
                let by = this.opened_by;
                step.open(this, Some(by), window, cx);
            })),
            // Focus is in the field already, unless it left or a screen
            // reader moved it into the list.
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

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A value passed to control it shows in the field at once, so
    /// it takes the window.
    pub fn set_options(&mut self, options: AutocompleteOptions, window: &mut Window, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        let mut changed = false;
        // A controlled close clears the highlight in the same pass.
        if derive_open(options.open, self.popup.is_open(), |open| !open).unwrap_or(false) {
            changed |= self.core.listbox.set_highlighted(None);
            self.focus_in_list = false;
        }
        let items_changed = !same_items(&self.items, &options.items);
        changed |= items_changed
            || !same_items(&self.options.empty_items, &options.empty_items)
            || self.status != options.status
            || self.disabled != options.disabled
            || self.read_only != options.read_only
            || self.required != options.required
            || self.name != options.name
            || self.form != options.form
            || self.options.inline_complete != options.inline_complete
            || self.options.filter != options.filter;
        self.items = options.items.clone();
        self.status = options.status;
        self.disabled = options.disabled;
        self.read_only = options.read_only;
        self.required = options.required;
        self.name = options.name.clone();
        self.form = options.form.clone();
        if let Some(initial) = &options.default_value {
            self.initial = initial.clone();
        }
        changed |= self.value.set_controlled(options.value.clone());
        self.value.set_on_change(value_callback(&options, &self.chosen));
        changed |= self.popup.set_options(popup_options(&options), cx);
        self.options = options;
        // Data that drops the highlighted suggestion settles the highlight
        // in the same pass, so it never points at a suggestion that left.
        if items_changed {
            let target = self.core.settled_highlight(self, false);
            changed |= self.core.listbox.set_highlighted(target);
        }
        // A value from outside, like a controlled one, also becomes what
        // filters.
        self.value_arrived();
        self.sync_input(window, cx);
        self.sync_field(cx);
        self.sync_form(cx);
        let core = self.core.clone();
        core.options_changed(self, window, cx);
        // What the new options changed reaches the core once they're handed
        // over, so the app's callbacks don't run inside its render.
        cx.defer_in(window, |this, window, cx| this.observe(window, cx));
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The native field the input part renders, like to read where its caret
    /// is.
    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// The shared core: the listbox engine its suggestions register with,
    /// the highlight and the text that filters.
    pub fn core(&self) -> &AutocompleteCore<Self> {
        &self.core
    }

    /// Its part in forms.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    /// What the user typed, which filters the list. It differs from the
    /// value while an inline completion or a highlighted suggestion fills
    /// the field.
    pub fn query(&self) -> &SharedString {
        &self.query
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

    // The name the text submits under: its own, or its Field's.
    pub(crate) fn submit_name(&self) -> Option<SharedString> {
        self.name.clone().or_else(|| self.internals.control_attrs().name)
    }

    // Disabled or read-only: nothing can be cleared (`isLocked`).
    pub(crate) fn is_locked(&self) -> bool {
        self.is_disabled() || self.read_only
    }

    pub(crate) fn editable(&self) -> bool {
        !self.is_disabled() && !self.read_only
    }

    pub(crate) fn completes(&self) -> bool {
        self.options.inline_complete
    }

    // What the core reads changed: it settles the highlight, and screen
    // readers hear the count once typing pauses.
    pub(crate) fn observe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.core.clone();
        core.observe(self, window, cx);
    }

    pub(crate) fn show(&mut self, request: ShowRequest<()>, window: &mut Window, cx: &mut Context<Self>) {
        let control = self.popup.control().clone();
        control.show(self, request, None, window, cx);
    }

    pub(crate) fn hide(&mut self, reason: Reason, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let control = self.popup.control().clone();
        control.hide(self, HideRequest::new().reason(reason).event(event), window, cx)
    }

    // The core moves the highlight. Arrow keys fill in what they highlight,
    // with inline completion.
    fn core_highlight(
        &mut self,
        key: Option<SharedString>,
        why: Option<HighlightCause>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let event = why.as_ref().and_then(|why| why.event.clone());
        let listbox = self.core.listbox.clone();
        listbox.highlight(self, key.clone(), HighlightOptions::event(event.clone()), window, cx);
        if let Some(key) = key
            && self.completes()
            && why.is_some_and(|why| why.reason == Reason::ListNavigation)
        {
            self.fill_in(&key, event, window, cx);
        }
    }

    // A change to the text (`setValue`). `query` is what filters, which is
    // the text unless a suggestion fills the field in. Returns whether it
    // went through.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn change_value(
        &mut self,
        value: SharedString,
        reason: Reason,
        event: Option<InputEvent>,
        query: Option<SharedString>,
        item: Option<AutocompleteChosen>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_locked() && reason != Reason::Reset {
            return false;
        }
        self.requested = value.clone();
        self.core.cause(reason, event.clone());
        let query = query.unwrap_or_else(|| value.clone());
        let done = &value == self.value.get() || {
            *self.chosen.borrow_mut() = item;
            let options = ChangeOptions {
                reason: Some(reason),
                event,
                source: None,
            };
            let done = self.value.change(&self.instance, value.clone(), options, window, cx);
            *self.chosen.borrow_mut() = None;
            done
        };
        if done {
            // A value that `on_value_change` set in its place stored its own
            // query.
            if self.requested == value {
                self.query = query;
            }
            self.value_arrived();
            self.sync_input(window, cx);
            self.sync_form(cx);
            self.observe(window, cx);
            cx.notify();
            return true;
        }
        // Canceled: the field shows the value it had.
        let current = self.value.get().clone();
        if self.field.read(cx).value() != &current {
            self.write_field(current, window, cx);
        }
        false
    }

    // The user typed. The text filters, the suggestions open, and with
    // `inline_complete` an insertion at the end completes the first match,
    // keeping the case the user typed, with the rest selected (`typed`).
    fn typed(&mut self, edit: &Edit, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        let text = edit.value.clone();
        let completing = self.completes()
            && !edit.composing
            && edit.input_type.as_str().starts_with("insert")
            && edit.input_type != InputType::InsertReplacementText
            && self.field.read(cx).selection_end() == text.len();
        let matched = if completing {
            completion_for(&text, &self.core.listbox, self)
        } else {
            None
        };
        let completed: SharedString = match &matched {
            Some(entry) => {
                let rest = &entry.label[char_offset(&entry.label, text.chars().count())..];
                format!("{text}{rest}").into()
            }
            None => text.clone(),
        };
        if !self.change_value(completed.clone(), Reason::InputChange, None, Some(text.clone()), None, window, cx) {
            return;
        }
        if matched.is_some() && completed != text {
            self.write_field(completed.clone(), window, cx);
            let start = text.len();
            self.field.update(cx, |field, cx| {
                field.set_selection_range(start, completed.len(), SelectionDirection::None, window, cx)
            });
        }
        if !self.popup.is_open() {
            if !text.is_empty() {
                self.pending_highlight = Some(ListEdge::First);
            }
            self.show(
                ShowRequest::new()
                    .by(PopupOpener::Keyboard)
                    .reason(Reason::InputChange),
                window,
                cx,
            );
        }
    }

    // Fills the field with a suggestion, as arrow keys do with
    // `inline_complete`. The typed part stays what filters, and the rest is
    // selected (`fillIn`).
    fn fill_in(&mut self, key: &SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(label) = self.core.listbox.find(self, Some(key)).map(|entry| entry.label) else {
            return;
        };
        let query = self.query.clone();
        if !self.change_value(label.clone(), Reason::ListNavigation, event, Some(query.clone()), None, window, cx) {
            return;
        }
        self.write_field(label.clone(), window, cx);
        let start = if normalize(&label).starts_with(normalize(&query).as_ref()) {
            char_offset(&label, query.chars().count())
        } else {
            label.len()
        };
        self.field.update(cx, |field, cx| {
            field.set_selection_range(start, label.len(), SelectionDirection::None, window, cx)
        });
    }

    // `on_submit` hears the text. Returns whether the form submits next.
    fn submit(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let details = AutocompleteSubmitDetails::new(event);
        if let Some(on_submit) = self.options.on_submit.clone() {
            let value = self.value.get().clone();
            on_submit(&value, &details, window, cx);
        }
        !details.default_prevented()
    }

    /// Chooses the suggestion with `key`, as a press on it does: the field
    /// fills with its label, and the list closes. Returns false when it's
    /// disabled or the field can't change (`choose`).
    pub fn choose(&mut self, key: SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let listbox = self.core.listbox.clone();
        let Some(entry) = listbox.find(self, Some(&key)) else {
            return false;
        };
        if listbox.disabled(self, &entry) || !self.editable() {
            return false;
        }
        let item = AutocompleteChosen {
            value: entry.key.clone(),
            label: entry.label.clone(),
        };
        self.change_value(entry.label.clone(), Reason::ItemPress, event.clone(), None, Some(item), window, cx);
        let value = self.value.get().clone();
        if self.field.read(cx).value() != &value {
            self.write_field(value, window, cx);
        }
        self.hide(Reason::ItemPress, event, window, cx);
        true
    }

    // A press on a suggestion. With `submit_on_select`, the form submits, as
    // Enter would (`press`).
    pub(crate) fn press(&mut self, key: SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.choose(key, event.clone(), window, cx) || !self.options.submit_on_select {
            return;
        }
        if self.submit(event, window, cx) {
            submit_implicitly(self.internals.form().as_ref(), window, cx);
        }
    }

    // Enter: the highlighted suggestion fills the field, and submits only
    // with `submit_on_select`. With none highlighted, `on_submit` hears the
    // text and the form submits next, unless it stops it (`onEnter`).
    fn on_enter(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let input_event = InputEvent::from(event);
        let open = self.popup.is_open();
        if open && let Some(highlighted) = self.core.listbox.highlighted() {
            // Links follow themselves, so routers and modifier keys work.
            if self.core.listbox.element(&highlighted).is_some_and(|element| element.link)
                && let Some(press) = press_of(&self.instance, "item", Some(&highlighted))
            {
                window.prevent_default();
                press.click(input_event, window, cx);
                return;
            }
            if !self.choose(highlighted, Some(input_event.clone()), window, cx) {
                window.prevent_default();
                return;
            }
            // A chosen suggestion submits only with `submit_on_select`.
            // Otherwise Enter only fills it in.
            if !self.options.submit_on_select || !self.submit(Some(input_event), window, cx) {
                window.prevent_default();
            }
            return;
        }
        if open {
            self.hide(Reason::InputCommit, Some(input_event.clone()), window, cx);
        }
        // The form submits next, unless `on_submit` stops it.
        if !self.submit(Some(input_event), window, cx) {
            window.prevent_default();
        }
    }

    /// Keys in the field (`onInputKeyDown`): the core's steps through the
    /// list, Enter, and ArrowRight or End accepting an inline completion.
    /// Escape takes a completion back out, then closes the list through its
    /// layer.
    pub(crate) fn on_input_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(modality) = self.modality.as_mut()
            && modality.key(&event.keystroke)
        {
            cx.notify();
        }
        let core = self.core.clone();
        if core.navigate(self, event, window, cx) {
            return;
        }
        let open = self.popup.is_open();
        let alt = event.keystroke.modifiers.alt;
        let input_event = InputEvent::from(event);
        match Key::of(&event.keystroke) {
            // Alt+ArrowUp closes, as the APG asks.
            Key::ArrowUp => {
                if alt && open {
                    window.prevent_default();
                    self.hide(Reason::EscapeKey, Some(input_event), window, cx);
                }
            }
            // Alt+ArrowDown opens without moving the highlight.
            Key::ArrowDown => {
                if alt && !open {
                    window.prevent_default();
                    self.show(
                        ShowRequest::new()
                            .event(Some(input_event))
                            .by(PopupOpener::Keyboard),
                        window,
                        cx,
                    );
                }
            }
            Key::Enter => self.on_enter(event, window, cx),
            // Moving the caret to the end accepts an inline completion, so
            // Escape keeps it.
            Key::ArrowRight | Key::End => {
                if self.completes() && self.value.get() != &self.query && !event.keystroke.modifiers.shift {
                    self.core.cause(Reason::InputChange, Some(input_event));
                    self.query = self.value.get().clone();
                    self.observe(window, cx);
                    cx.notify();
                }
            }
            // An inline completion or a filled-in suggestion comes back out
            // before the list's layer closes it.
            Key::Escape => {
                let query = self.query.clone();
                if open
                    && self.value.get() != &query
                    && self.change_value(query.clone(), Reason::EscapeKey, Some(input_event), None, None, window, cx)
                {
                    self.write_field(query, window, cx);
                }
            }
            Key::Tab => {
                if open {
                    self.hide(Reason::FocusOut, Some(input_event), window, cx);
                }
            }
            _ => {}
        }
    }

    // A press on the field reopens suggestions that open on focus, like
    // after Escape closed them (the input's `press`).
    pub(crate) fn input_pressed(&mut self, event: InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_open() || !self.options.open_on_focus {
            return;
        }
        let by = opener_of(Some(&event));
        self.show(
            ShowRequest::new()
                .event(Some(event))
                .by(by)
                .reason(Reason::TriggerPress)
                .extra(FromInput),
            window,
            cx,
        );
    }

    // A form reset puts the default text back (`bindReset`).
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if keeps_value_on_reset(self.options.value.as_ref(), self.options.default_value.as_ref()) {
            return;
        }
        let initial = self.initial.clone();
        self.change_value(initial, Reason::Reset, None, None, None, window, cx);
        let value = self.value.get().clone();
        if self.field.read(cx).value() != &value {
            self.write_field(value, window, cx);
        }
        self.sync_form(cx);
    }
}

impl Autocomplete {
    /// Whether the suggestions are open (`isOpen`).
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// Opens or closes the suggestions. Returns false when the change was
    /// canceled or they were already open or closed (`setOpen`).
    pub fn set_open(&mut self, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.set_open(self, open, options, window, cx)
    }

    /// Opens the suggestions, and resolves once they close, with why and
    /// how. Resolves exactly once: on the next close from any source, when
    /// its signal aborts, or with `Unmount` when the autocomplete leaves
    /// (`open`).
    pub fn open(&mut self, options: PopupOpenOptions<()>, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        let popup = self.popup.clone();
        popup.open(self, options, window, cx)
    }

    /// Closes the suggestions. Returns false when the close was canceled or
    /// they were already closed (`close`).
    pub fn close(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.close(self, options.into(), window, cx)
    }

    /// Opens the suggestions when they're closed, and closes them when
    /// they're open. Returns false when the change was canceled (`toggle`).
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.toggle(self, options, window, cx)
    }

    /// The text (`getValue`).
    pub fn get_value(&self) -> &SharedString {
        self.value.get()
    }

    /// Changes the text, which filters the suggestions. Returns false when
    /// the change was canceled or nothing changed (`setValue`). Reasons
    /// default to `Imperative`.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let value = value.into();
        if &value == self.value.get() {
            return false;
        }
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.change_value(value, reason, options.event, None, None, window, cx)
    }

    /// Empties the field. Returns false when the change was canceled or
    /// nothing changed (`clear`).
    pub fn clear(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.value.get().is_empty() {
            return false;
        }
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.change_value(SharedString::default(), reason, options.event, None, None, window, cx)
    }

    /// The value of the highlighted suggestion, or `None` when none is
    /// (`getHighlighted`).
    pub fn get_highlighted(&self) -> Option<SharedString> {
        let key = self.core.listbox.highlighted();
        highlighted_value(&self.core.listbox, self, key.as_deref())
    }

    /// Highlights the suggestion with this value in the open list, scrolled
    /// into view, or none with `None`. Returns false when the suggestions
    /// are closed, or no enabled suggestion has the value (`highlight`).
    pub fn highlight(&mut self, value: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.popup.is_open() {
            return false;
        }
        let key = value.map(|value| SharedString::from(value.to_string()));
        let current = self.core.listbox.highlighted();
        let listbox = self.core.listbox.clone();
        highlight_from_code(&listbox, self, key, current.as_ref(), window, cx)
    }

    /// Moves focus to the field (`focus`).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.field.read(cx).focus_handle(cx);
        if !self.is_disabled() {
            window.focus(&handle, cx);
        }
    }

    // The highlight moved: `on_highlight` hears where it went.
    fn highlight_moved(
        &mut self,
        key: Option<&SharedString>,
        index: Option<usize>,
        event: Option<&InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(on_highlight) = self.options.on_highlight.clone() else {
            return;
        };
        let details = ListHighlightDetails {
            index,
            event: event.cloned(),
        };
        on_highlight(key, &details, window, cx);
    }

    // A value from outside, like a controlled one or the api, also becomes
    // what filters (the original's value subscriber). Call it after the
    // value changes.
    fn value_arrived(&mut self) {
        let value = self.value.get().clone();
        if value == self.previous_value {
            return;
        }
        self.previous_value = value.clone();
        if value == self.requested {
            return;
        }
        self.requested = value.clone();
        // It settles the highlight as `Imperative`, not with a reason a
        // list navigation left behind, which would fill a suggestion in over
        // it.
        self.core.cause(Reason::Imperative, None);
        self.query = value;
    }

    // The list started opening or closing. Closing forgets the highlight.
    fn open_changed(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.field.update(cx, |field, cx| field.set_expanded(open, cx));
        if open {
            return;
        }
        self.pending_highlight = None;
        if self.core.listbox.set_highlighted(None) {
            cx.notify();
        }
        self.focus_in_list = false;
        self.observe(window, cx);
    }

    // Where focus goes once the list opens: it stays in the field, unless a
    // screen reader's tap moved it into the list, to the first suggestion
    // (`initialFocus`).
    fn initial_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<FocusHandle> {
        let target = self.pending_highlight.take();
        let listbox = self.core.listbox.clone();
        if self.focus_in_list {
            let Some(first) = listbox.first_enabled(self) else {
                return Some(self.popup.content_focus_handle());
            };
            listbox.highlight(self, Some(first.clone()), HighlightOptions::without_focus(None), window, cx);
            return Some(
                listbox
                    .element(&first)
                    .and_then(|element| element.handle)
                    .unwrap_or_else(|| self.popup.content_focus_handle()),
            );
        }
        // Typing that opened the list highlights its best match, arrow keys
        // the first or last.
        if target == Some(ListEdge::First) && !self.query.is_empty() {
            let core = self.core.clone();
            core.settle(self, true, None, window, cx);
        } else if let Some(target) = target {
            let step = match target {
                ListEdge::First => Step::First,
                ListEdge::Last => Step::Last,
            };
            let key = listbox.step_key(self, step, None);
            listbox.highlight(self, key, HighlightOptions::default(), window, cx);
        }
        // In a sheet the field moved into its header, and loses focus on the
        // way if the sheet showed later. It takes focus back here.
        (self.popup.presentation_value() == Presentation::Sheet).then(|| self.field.read(cx).focus_handle(cx))
    }

    /// Keys in the list, when a screen reader's tap moved real focus there
    /// (`onListKeyDown`).
    pub(crate) fn on_list_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_in_list || self.field.read(cx).is_focused(window) {
            return;
        }
        let listbox = self.core.listbox.clone();
        let highlighted = listbox.highlighted();
        list_key_down(
            self,
            event,
            &listbox,
            highlighted,
            |this, key, event, window, cx| this.press(key, Some(event), window, cx),
            |this, window, cx| {
                this.focus_in_list = false;
                let handle = this.field.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            },
            window,
            cx,
        );
    }

    // What the field tells once its update is over, which the original hears
    // with listeners on the input, and the browser tells the form.
    fn field_event(&mut self, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Input(edit) => {
                let node = self.internals.node().clone();
                notify_value(&node, ValueSource::Input, window, cx);
                dispatch_change(Some(&node), &[FormEventType::Input], window, cx);
                // Typing reports how it was typed, though no event explains it.
                with_source(edit.source, || self.typed(edit, window, cx));
                self.sync_form(cx);
            }
            TextFieldEvent::Write(_) => {
                notify_value(self.internals.node(), ValueSource::Write, window, cx);
                self.sync_form(cx);
            }
            TextFieldEvent::Change(_) => {
                dispatch_change(Some(self.internals.node()), &[FormEventType::Change], window, cx);
            }
            _ => {}
        }
    }

    // Focus came to the field: a sheet grows for the keyboard, and
    // suggestions that open on focus open.
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
        if self.options.open_on_focus && !self.popup.is_open() {
            self.show(
                ShowRequest::new()
                    .event(Some(InputEvent::Focus(window_source(window))))
                    .by(PopupOpener::Keyboard)
                    .reason(Reason::TriggerFocus)
                    .extra(FromInput),
                window,
                cx,
            );
        }
    }

    // Focus left the field: the list closes, unless focus went to the
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
        let event = InputEvent::Focus(window_source(window));
        self.hide(Reason::FocusOut, Some(event), window, cx);
    }

    // Whether a press or focus lands on the control around the list: its
    // field, trigger, clear button or label (`ownsElement`).
    pub(crate) fn owns(&self, target: &Target, window: &Window) -> bool {
        self.regions.owns(target, window)
    }

    // Enter in the field submits its form implicitly, the browser's own
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

    fn set_form_disabled(&mut self, disabled: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if self.form_disabled != disabled {
            self.form_disabled = disabled;
            self.sync_field(cx);
            self.sync_form(cx);
            cx.notify();
        }
    }

    // Writes text into the field from code, the original's `input.value =`.
    fn write_field(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.field.update(cx, |field, cx| field.set_value(text, window, cx));
    }

    // Brings the field up to the value (the field part's `write`). Only new
    // text is written: text on its way to the owner of a controlled value is
    // ahead of it, and writing the old text back would put the caret at the
    // end.
    pub(crate) fn sync_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.value.get().clone();
        if self.shown.as_ref() == Some(&value) {
            return;
        }
        self.shown = Some(value.clone());
        self.field.update(cx, |field, cx| {
            if field.value() != &value {
                field.set_value(value, window, cx);
            }
        });
    }

    // Hands the field what the autocomplete decides of it: disabled by
    // itself or a fieldset, read-only, required, the input's attributes, and
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

    // What the form reads of the field: its text under its name, missing
    // while required and empty, as a native input reports itself.
    pub(crate) fn sync_form(&self, cx: &App) {
        bind_value_input(
            &self.internals,
            ValueInputOptions {
                value: self.field.read(cx).value().clone(),
                name: self.submit_name(),
                required: self.required,
                disabled: self.own_disabled(),
                read_only: self.read_only,
                form: self.form.clone(),
                missing: Some(ValueMissingKind::Text),
            },
        );
    }

    // Whether the list would present as a sheet if it opened now.
    pub(crate) fn resolves_sheet(&self, window: &Window) -> bool {
        self.popup
            .presentation()
            .is_some_and(|presentation| presentation.resolve(self, window) == Presentation::Sheet)
    }

    // The field rendered: the list learns what names it, focus comes back to
    // a field that moved into a sheet's header, and development builds check
    // it's named.
    pub(crate) fn input_rendered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.field.read(cx).focus_handle(cx);
        let own = self.instance.element_id("label", None);
        let labelled_by = self.internals.aria().labelled_by;
        let aria_label = name_of(&self.instance, "input", None).and_then(|name| name.aria_label);
        let next = ui_sdk_lists::field_name_of(&labelled_by, &own, &[], aria_label.as_ref());
        let changed = ui_sdk_lists::bind_field_name(&mut self.field_name, next);
        if !self.input_bound {
            self.input_bound = true;
            ui_sdk_lists::warn_if_unnamed(
                |this: &Self| &this.instance,
                "input",
                None,
                "The field has no accessible name. Add a label part, a Field label or an aria_label.",
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

    // The visible suggestions as data, for rendering a virtualized list.
    pub(crate) fn visible_items(&self) -> ListData<SharedString> {
        self.core
            .listbox
            .visible_data(self)
            .map(|visible| visible.list)
            .unwrap_or_default()
    }
}
