//! Port of `components/date-picker/date-picker-api.ts`: Date Picker's state
//! and api: the text typed in its inputs and the dates it reads, the popup,
//! presets and clearing, and what the form asks of it.
//!
//! The original's `State` is Calendar's state, the popup's and the picker's
//! own. Here Calendar's is a [`CalendarModelState`], which the picker hands
//! Calendar's api and parts through [`CalendarHost`]; the popup's is its
//! [`Popup`], reached through [`PopupHost`]; and the rest are the picker's
//! fields. `datePickerApi` is the picker's methods.
//!
//! The original writes each input's text whenever the state changes, from a
//! store subscription. Here the picker observes itself: once an update that
//! notified it is over, the inputs show their text and the form hears the
//! value. A change the app hands over with `set_options`, which may come
//! while the window draws, shows in the inputs at once.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, AppContext, Context, Focusable, KeyDownEvent, SharedString, Window};
use ui_sdk_calendar::calendar_api::{self, CalendarHost};
use ui_sdk_calendar::{
    CalendarMode, CalendarModelState, CalendarOptions, CalendarSelectKind, CalendarValue, ISOMonth,
    is_allowed_range, normalize_value, selected_dates,
};
use ui_sdk_date::{
    DateTimeFormatOptions, ISODate, MonthWidth, NumericWidth, ParseDateOptions, clamp_date, format_date,
    is_iso_date, parse_date,
};
use ui_sdk_forms::{
    ElementInternals, FormData, FormEventType, FormValue, InternalsOptions, Node, Problem, ValidityFlag,
    ValueMissingKind, bind_internals, dispatch_change, focus_with, keeps_value_on_reset, submit_implicitly,
    value_missing_message,
};
use ui_sdk_kernel::modality::focus_event;
use ui_sdk_kernel::{ChangeOptions, InputEvent, Instance, InstanceHandle, Key, Parts, Reason, restore_focus};
use ui_sdk_native::{TextField, TextFieldOptions};
use ui_sdk_platform::{Align, Anchor, PositionOptions, Region, Target};
use ui_sdk_popups::{
    AttachConfig, CreatePopupOptions, InitialFocusFn, Popup, PopupClose, PopupCloseOptions, PopupHost,
    PopupOpenChangeCallback, PopupOpenOptions, PopupOpener, PopupOptions, PopupPending, PopupRules,
    PlacementDefaults, SheetConfig, ShowRequest, HideRequest, content_anchor, create_popup,
    date_picker_preset, derive_open, placement_options,
};
use ui_sdk_text::{bind_page_locale, js_trim};

use crate::date_picker::{
    DatePicker, DatePickerChangeOptions, DatePickerMode, DatePickerOptions, DatePickerState,
    DatePickerValue, PickerInput,
};

/// The part keys of the inputs: the one that holds the start, or the date,
/// and a range's end.
pub(crate) const ENDS: [&str; 2] = ["start", "end"];

/// The value as `JSON.stringify` writes it, which tells values apart
/// (`valueKey`).
pub(crate) fn value_key(value: &DatePickerValue) -> String {
    let quoted = |date: &ISODate| format!("\"{date}\"");
    match value {
        CalendarValue::Empty => "null".to_string(),
        CalendarValue::Single(date) => quoted(date),
        CalendarValue::Multiple(dates) => {
            format!("[{}]", dates.iter().map(quoted).collect::<Vec<_>>().join(","))
        }
        CalendarValue::Range(start, end) => format!("[{},{}]", quoted(start), quoted(end)),
    }
}

/// A date as an input writes it: the locale's numbers, without the marks
/// that set text direction (`shortDate`).
pub(crate) fn short_date(date: &str, locale: &str) -> String {
    let options = DateTimeFormatOptions::default()
        .year(NumericWidth::Numeric)
        .month(MonthWidth::TwoDigit)
        .day(NumericWidth::TwoDigit);
    format_date(date, Some(locale), Some(&options)).replace(['\u{200E}', '\u{200F}'], "")
}

/// The date an input holds: the value's date, or a range's end, or while a
/// range is being picked, its start (`dateAt`).
pub(crate) fn date_at(model: &CalendarModelState, index: usize) -> Option<ISODate> {
    if model.mode() != CalendarMode::Range {
        return if index > 0 { None } else { model.value().as_single().cloned() };
    }
    if let Some(anchor) = model.anchor() {
        return if index > 0 { None } else { Some(anchor.clone()) };
    }
    model
        .value()
        .as_range()
        .map(|(start, end)| if index > 0 { end.clone() } else { start.clone() })
}

// What an input's text reads as (`read`): a date, nothing, or text that isn't
// a date.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Reading {
    Date(ISODate),
    Empty,
    NotADate,
}

// What a range's ends submit under, or a date's (`nameAt`).
fn name_at(picker: &DatePicker, index: usize, name: Option<&SharedString>) -> Option<SharedString> {
    if picker.model.mode() != CalendarMode::Range {
        return if index > 0 { None } else { name.cloned() };
    }
    let own = if index > 0 { &picker.end_name } else { &picker.start_name };
    own.clone().or_else(|| name.cloned())
}

// What the popup's rules read: the picker's parts in the page, through its
// instance, and the regions they lay out in. The rules run while the picker
// may be mid-update, so they never read the picker itself.
#[derive(Clone)]
struct Anchors {
    instance: InstanceHandle,
    trigger: Region,
    inputs: [Region; 2],
}

impl Anchors {
    fn bound(&self, slot: &str, key: Option<&str>) -> bool {
        self.instance
            .bound_parts()
            .iter()
            .any(|part| part.slot == slot && key.is_none_or(|key| part.key.as_deref() == Some(key)))
    }

    // The input that holds the start, or without it, the other
    // (`inputElement`).
    fn input(&self) -> Option<Region> {
        ENDS.iter()
            .position(|key| self.bound("input", Some(key)))
            .map(|index| self.inputs[index].clone())
    }

    // The trigger, or without one, the input that holds the start
    // (`triggerElement`).
    fn trigger(&self) -> Option<Region> {
        if self.bound("trigger", None) {
            return Some(self.trigger.clone());
        }
        self.input()
    }

    // The popup lines up with the start input, or the trigger without one
    // (`anchorElement`).
    fn anchor(&self) -> Option<Region> {
        self.input()
            .or_else(|| self.bound("trigger", None).then(|| self.trigger.clone()))
    }
}

// The options an input's text field takes, with the hooks that hand what
// happens in it to the picker, as the original's listeners on the input do.
fn field_options(index: usize, picker: gpui::WeakEntity<DatePicker>) -> TextFieldOptions {
    let typed = picker.clone();
    let committed = picker.clone();
    let left = picker;
    TextFieldOptions::default()
        .on_input(move |field, _edit, window, cx| {
            let text = field.value().clone();
            typed
                .update(cx, |picker, cx| picker.typed(index, text, window, cx))
                .ok();
        })
        // What the user typed is committed, and the form hears it, as from a
        // native input's `change`.
        .on_change(move |_field, window, cx| {
            committed
                .update(cx, |picker, cx| {
                    dispatch_change(Some(picker.internals.node()), &[FormEventType::Change], window, cx);
                })
                .ok();
        })
        .on_blur(move |_field, window, cx| {
            let event = focus_event(window);
            left.update(cx, |picker, cx| picker.commit(index, Some(event), window, cx))
                .ok();
        })
}

impl DatePickerOptions {
    // Calendar's options, which the picker's calendar state is made from and
    // synced with. A date picker's days are never deselected, and its focus
    // and range start aren't the app's to refuse.
    pub(crate) fn calendar_options(&self) -> CalendarOptions {
        CalendarOptions {
            mode: self.mode.calendar_mode(),
            value: self.value.clone(),
            default_value: self.default_value.clone(),
            on_value_change: self.on_value_change.clone(),
            on_before_value_change: self.on_before_value_change.clone(),
            month: self.month.clone(),
            default_month: self.default_month.clone(),
            on_month_change: self.on_month_change.clone(),
            min: self.min.clone(),
            max: self.max.clone(),
            unavailable: self.unavailable.clone(),
            disabled: self.disabled,
            read_only: self.read_only,
            deselectable: false,
            on_focused_change: None,
            on_anchor_change: None,
            months: self.months,
            page_by: self.page_by,
            locale: self.locale.clone(),
            week_starts_on: self.week_starts_on,
            weekday_format: self.weekday_format,
            month_format: self.month_format,
            fixed_weeks: self.fixed_weeks,
            show_outside_days: self.show_outside_days,
            today: self.today.clone(),
            time_zone: self.time_zone.clone(),
            min_days: self.min_days,
            max_days: self.max_days,
            allow_unavailable: self.allow_unavailable,
            debug: self.debug,
        }
    }

    // What the popup reads from the picker's options.
    pub(crate) fn popup_options(&self) -> PopupOptions<()> {
        let on_open_change = self.on_open_change.clone().map(|callback| {
            Rc::new(move |open: &bool, details: &ui_sdk_kernel::ChangeDetails, _value: Option<&SharedString>, window: &mut Window, cx: &mut App| {
                callback(open, details, window, cx)
            }) as PopupOpenChangeCallback
        });
        PopupOptions {
            open: self.open,
            default_open: self.default_open,
            on_open_change,
            on_open_change_complete: self.on_open_change_complete.clone(),
            presentation: Some(self.presentation),
            disabled: self.disabled,
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            ..PopupOptions::default()
        }
    }
}

impl DatePicker {
    /// A date picker from its options (`initialDatePickerState` and
    /// `datePickerApi`), with a native text field for each input it can show.
    pub fn new(options: DatePickerOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("date-picker").with_agent(cx);
        CalendarValue::register_data();
        instance.debug = options.debug;
        let model = CalendarModelState::new(&options.calendar_options(), window, cx);
        let internals = ElementInternals::new(cx);
        // Enter in it submits a form with no other text field, as in a text
        // input.
        internals.set_blocks_implicit_submission(true);
        if options.id.is_some() {
            internals.set_id(options.id.clone(), cx);
        }
        let weak = cx.weak_entity();
        let locale = model.locale(cx);
        let placeholders = [options.placeholder.clone(), options.end_placeholder.clone()];
        let inputs = [0, 1].map(|index| {
            let text: SharedString = date_at(&model, index)
                .map(|date| short_date(&date, &locale).into())
                .unwrap_or_default();
            let mut field = field_options(index, weak.clone()).default_value(text);
            field.placeholder = placeholders[index].clone();
            field.style = options.field_style;
            field.disabled = options.disabled;
            field.read_only = options.read_only;
            field.required = options.required;
            let entity = cx.new(|cx| TextField::new(field.clone(), window, cx));
            let handle = entity.read(cx).focus_handle(cx);
            PickerInput {
                field: entity,
                node: Node::element(),
                region: Region::with_focus_handle(handle),
                options: field,
            }
        });
        let trigger_focus = cx.focus_handle();
        let trigger_region = Region::with_focus_handle(trigger_focus.clone());
        let anchors = Anchors {
            instance: instance.handle(),
            trigger: trigger_region.clone(),
            inputs: [inputs[0].region.clone(), inputs[1].region.clone()],
        };
        let focus_target: Rc<RefCell<Option<ISODate>>> = Rc::default();
        let rules = {
            let trigger = anchors.clone();
            let contains = anchors;
            PopupRules {
                trigger: Some(Rc::new(move || trigger.trigger())),
                // Pressing the trigger toggles the popup, and pressing an
                // input leaves it to focus.
                contains: Some(Rc::new(move |target: &Target, _event, window: &Window, _cx: &App| {
                    contains.bound("trigger", None) && contains.trigger.contains(target, window)
                })),
                on_open_change: Some(Rc::new(
                    |picker: &mut Self, open: bool, _closed: Option<&PopupClose>, window: &mut Window, cx: &mut Context<Self>| {
                        if open {
                            picker.opened(window, cx);
                        }
                    },
                )),
                on_open_change_complete: Some(Rc::new(
                    |picker: &mut Self, open: bool, _closed: Option<&PopupClose>, window: &mut Window, cx: &mut Context<Self>| {
                        if !open {
                            picker.closed(window, cx);
                        }
                    },
                )),
                bind: Some(Rc::new(|picker: &Self| picker.content_bind())),
                ..PopupRules::default()
            }
        };
        let popup = create_popup(
            &instance,
            CreatePopupOptions::new(date_picker_preset())
                .rules(rules)
                .options(options.popup_options()),
            window,
            cx,
        );
        // A fieldset or a Field that disables it notifies the picker through
        // its internals, and shows in its inputs as the picker settles.
        let subscriptions = vec![
            internals.on_reset(cx, |picker: &mut Self, window, cx| picker.reset(window, cx)),
            // A label, or a submit that finds it invalid, focuses an input.
            focus_with(&internals, move |window, cx| {
                weak.update(cx, |picker, cx| picker.focus_input(window, cx)).ok();
            }),
            bind_page_locale(cx, |_picker: &mut Self, _locale, cx| cx.notify()),
            {
                let this = cx.entity();
                cx.observe_in(&this, window, |picker, _, window, cx| picker.settle(window, cx))
            },
        ];
        let mode = options.mode.calendar_mode();
        let initial = normalize_value(mode, &options.default_value.clone().unwrap_or_default());
        let picker = Self {
            first_month: model.month().clone(),
            last: model.value().clone(),
            instance,
            model,
            popup,
            internals,
            drafts: [None, None],
            draft_for: String::new(),
            invalid: [false, false],
            initial,
            controlled: options.value.is_some(),
            has_default: options.default_value.is_some(),
            disabled: options.disabled,
            close_on_select: options.close_on_select,
            name: options.name.clone(),
            start_name: options.start_name.clone(),
            end_name: options.end_name.clone(),
            form: options.form.clone(),
            required: options.required,
            id: options.id.clone(),
            placeholders,
            field_style: options.field_style,
            awaited: false,
            inputs,
            trigger_focus,
            trigger_region,
            clear_focus: cx.focus_handle(),
            presets: Parts::new(),
            focus_target,
            _subscriptions: subscriptions,
        };
        picker.report(cx);
        picker
    }

    // What every options object sets (`settings`). Returns whether what
    // renders changed.
    fn apply_settings(&mut self, options: &DatePickerOptions, cx: &mut App) -> bool {
        let placeholders = [options.placeholder.clone(), options.end_placeholder.clone()];
        let changed = self.disabled != options.disabled
            || self.required != options.required
            || self.placeholders != placeholders
            || self.field_style != options.field_style;
        self.disabled = options.disabled;
        self.close_on_select = options.close_on_select;
        self.name = options.name.clone();
        self.start_name = options.start_name.clone();
        self.end_name = options.end_name.clone();
        self.form = options.form.clone();
        self.required = options.required;
        self.placeholders = placeholders;
        self.field_style = options.field_style;
        if self.id != options.id {
            self.id = options.id.clone();
            self.internals.set_id(options.id.clone(), cx);
        }
        changed
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`syncDatePickerState`). It takes the window: a new controlled value
    /// shows in the inputs at once.
    pub fn set_options(&mut self, options: DatePickerOptions, window: &mut Window, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        // A controlled open starts focus from the selected date, in the same
        // pass (`deriveOpen`).
        if derive_open(options.open, self.popup.is_open(), |open| open) == Some(true) {
            self.model.reset_focus(cx);
        }
        let mut changed = self.model.sync(&options.calendar_options());
        changed |= self.apply_settings(&options, cx);
        if let Some(default) = &options.default_value {
            self.initial = normalize_value(options.mode.calendar_mode(), default);
        }
        self.controlled = options.value.is_some();
        self.has_default = options.default_value.is_some();
        changed |= self.popup.set_options(options.popup_options(), cx);
        self.arrived(changed, window, cx);
    }

    /// Passes the controlled value, or `None` to let the picker keep its
    /// own. It shows in the inputs at once.
    pub fn set_controlled_value(&mut self, value: Option<DatePickerValue>, window: &mut Window, cx: &mut Context<Self>) {
        self.controlled = value.is_some();
        let changed = self.model.set_controlled_value(value);
        self.arrived(changed, window, cx);
    }

    /// Passes the controlled first month, or `None` to let the calendar keep
    /// its own.
    pub fn set_controlled_month(&mut self, month: Option<ISOMonth>, cx: &mut Context<Self>) {
        if self.model.set_controlled_month(month) {
            cx.notify();
        }
    }

    /// Passes the controlled `open`, or `None` to let the popup keep its
    /// own. It opens or closes once the current update is over.
    pub fn set_controlled_open(&mut self, open: Option<bool>, cx: &mut Context<Self>) {
        if derive_open(open, self.popup.is_open(), |open| open) == Some(true) {
            self.model.reset_focus(cx);
        }
        self.popup.set_controlled_open(open, cx);
    }

    // What the app handed over shows at once: the inputs' text and options,
    // and the form's value. A value that arrived after `set_value` asked for
    // it turns the calendar once the update is over, since that reports a
    // month change.
    fn arrived(&mut self, changed: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_fields(window, cx);
        self.report(cx);
        if *self.model.value() != self.last {
            cx.defer_in(window, |picker, window, cx| picker.settle(window, cx));
        }
        if changed {
            cx.notify();
        }
    }

    // What follows every change, as the original's store subscriptions: a
    // value the app passed back after `set_value` turns the calendar to it,
    // the inputs show their text, and the form hears the value.
    pub(crate) fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if *self.model.value() != self.last {
            self.last = self.model.value().clone();
            if std::mem::take(&mut self.awaited) {
                self.show_value(window, cx);
            }
        }
        self.sync_fields(window, cx);
        self.report(cx);
    }

    // Writes each input's text and options to its text field. The text is
    // written from the state, so typing never fights the caret, and a field's
    // own hook never runs it, since the field is mid-update there.
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let disabled = self.is_disabled();
        let read_only = self.model.is_read_only();
        let required = self.required;
        let style = self.field_style;
        for index in 0..2 {
            let text = self.text_of(index, cx);
            let placeholder = self.placeholders[index].clone();
            let input = &mut self.inputs[index];
            let options = &mut input.options;
            if options.disabled != disabled
                || options.read_only != read_only
                || options.required != required
                || options.placeholder != placeholder
                || options.style != style
            {
                options.disabled = disabled;
                options.read_only = read_only;
                options.required = required;
                options.placeholder = placeholder;
                options.style = style;
                let options = options.clone();
                input.field.update(cx, |field, cx| field.set_options(options, cx));
            }
            input.field.update(cx, |field, cx| {
                if *field.value() != text {
                    field.set_value(text, window, cx);
                }
            });
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The calendar's state, which it shares with Calendar, like how many
    /// months show.
    pub fn model(&self) -> &CalendarModelState {
        &self.model
    }

    /// Its part in forms.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    /// Whether it can't be used: by itself, its field or a fieldset.
    pub fn is_disabled(&self) -> bool {
        calendar_api::is_disabled(self)
    }

    /// What the root part's state holds.
    pub fn state(&self) -> DatePickerState {
        DatePickerState {
            value: self.model.value().clone(),
            mode: DatePickerMode::of(self.model.mode()),
            open: self.popup.is_open(),
            disabled: self.is_disabled(),
            read_only: self.model.is_read_only(),
            invalid: self.invalid_at(0) || self.invalid_at(1),
        }
    }

    // What an input shows: what was typed over the current value, else the
    // date it holds (`textOf`).
    pub(crate) fn text_of(&self, index: usize, cx: &App) -> SharedString {
        let draft = if self.draft_for == value_key(self.model.value()) {
            self.drafts[index].clone()
        } else {
            None
        };
        if let Some(draft) = draft {
            return draft;
        }
        date_at(&self.model, index)
            .map(|date| short_date(&date, &self.model.locale(cx)).into())
            .unwrap_or_default()
    }

    // Whether an input's text isn't a date that can be picked, once Enter or
    // blur committed it (`invalidAt`).
    pub(crate) fn invalid_at(&self, index: usize) -> bool {
        self.draft_for == value_key(self.model.value()) && self.invalid[index]
    }

    // What the form submits: the date, or each of a range's ends under its
    // name (`submitted`).
    fn submitted(&self) -> FormValue {
        if self.model.mode() != CalendarMode::Range {
            return FormValue::Text(date_at(&self.model, 0).unwrap_or_default());
        }
        // A field's name stands in for a missing one, as the original's
        // adapters pass it as the picker's.
        let name = self.name.clone().or_else(|| self.internals.control_attrs().name);
        let value = self.model.value().as_range();
        let mut data = FormData::new();
        for index in 0..2 {
            if let Some(name) = name_at(self, index, name.as_ref()) {
                let date = value
                    .map(|(start, end)| if index > 0 { end.clone() } else { start.clone() })
                    .unwrap_or_default();
                data.append(name, date);
            }
        }
        FormValue::Data(data)
    }

    // The inputs in the page, by index, in the order they rendered.
    pub(crate) fn rendered_inputs(&self) -> Vec<usize> {
        self.instance
            .bound_parts()
            .into_iter()
            .filter(|part| part.slot == "input")
            .filter_map(|part| ENDS.iter().position(|key| part.key.as_deref() == Some(key)))
            .fold(Vec::new(), |mut inputs, index| {
                if !inputs.contains(&index) {
                    inputs.push(index);
                }
                inputs
            })
    }

    // The first input whose text isn't a date, the start before the end
    // (`invalidInput`).
    pub(crate) fn invalid_input(&self) -> Option<usize> {
        self.rendered_inputs().into_iter().find(|index| self.invalid_at(*index))
    }

    // The input that holds the start, or without it, the other
    // (`inputElement`).
    fn input_element(&self) -> Option<usize> {
        let rendered = self.rendered_inputs();
        [0, 1].into_iter().find(|index| rendered.contains(index))
    }

    // Reports the value and its validity to the form (`bindInternals`): what
    // it submits, a missing date while it's required and an input is empty,
    // and text that isn't a date as a bad input, with the input as where the
    // message points, as a native input's own `required` and custom validity
    // would. A read-only picker is kept out of validation, like a read-only
    // input. Runs on every render and after every change.
    pub(crate) fn report(&self, cx: &App) {
        let disabled = self.is_disabled();
        let read_only = self.model.is_read_only();
        let rendered = self.rendered_inputs();
        let invalid = self.invalid_input().filter(|_| !disabled).map(|index| {
            Problem::new(ValidityFlag::BadInput, INVALID_DATE_MESSAGE).anchor(self.inputs[index].node.clone())
        });
        let empty = rendered
            .iter()
            .copied()
            .find(|index| self.text_of(*index, cx).is_empty());
        bind_internals(
            &self.internals,
            InternalsOptions {
                value: self.submitted(),
                name: self.name.clone(),
                value_missing: self.required && empty.is_some(),
                message: value_missing_message(ValueMissingKind::Text),
                invalid,
                anchor: empty.map(|index| self.inputs[index].node.clone()),
                disabled,
                read_only,
                required: self.required,
                barred: read_only,
                form: self.form.clone(),
            },
        );
    }

    /// Whether the popup is open (`isOpen`).
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// Opens or closes the popup. Returns false when the change was
    /// canceled or it was already open or closed.
    pub fn set_open(&mut self, open: bool, options: DatePickerChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.set_open(self, open, options, window, cx)
    }

    /// Opens the popup, and resolves once it closes, with why and how. It
    /// takes a reason, an event and a signal that closes the popup, with
    /// `Imperative`, when it aborts. Resolves exactly once and never fails:
    /// on the next close from any source, when the signal aborts, or with
    /// `Unmount` when the date picker leaves.
    pub fn open(&mut self, options: PopupOpenOptions<()>, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        let popup = self.popup.clone();
        popup.open(self, options, window, cx)
    }

    /// Closes the popup. Returns false when the close was canceled or it was
    /// already closed.
    pub fn close(&mut self, options: PopupCloseOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.close(self, options, window, cx)
    }

    /// Opens the popup if it's closed, and closes it if it's open. Returns
    /// false when the change was canceled.
    pub fn toggle(&mut self, options: DatePickerChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.toggle(self, options, window, cx)
    }

    /// The selected value: an ISO date, a start and an end, or `Empty`.
    pub fn get_value(&self) -> &DatePickerValue {
        self.model.value()
    }

    /// Selects a value. Returns false when canceled, unchanged, disabled,
    /// read-only, or holding a date people can't pick, like one past `max`.
    /// The calendar turns to it, or for a controlled value, to the value the
    /// app passes back.
    pub fn set_value(
        &mut self,
        value: DatePickerValue,
        options: DatePickerChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.model.is_pickable(&value, self.is_disabled()) {
            return false;
        }
        if self.model.set_range_anchor(None, None) {
            cx.notify();
        }
        let before = self.model.value().clone();
        let result = calendar_api::change_value(self, value.clone(), options, window, cx);
        // The calendar turns to a date set from code, as it does to a typed
        // one: to the value now held, or for a controlled one, to the value
        // its owner passes back.
        let held = self.model.value().clone();
        let asked = normalize_value(self.model.mode(), &value);
        if held != before || value_key(&held) == value_key(&asked) {
            self.show_value(window, cx);
        } else if result && self.controlled {
            self.awaited = true;
        }
        result
    }

    /// Selects nothing, as the Clear part does. Returns false when canceled
    /// or unchanged.
    pub fn clear(&mut self, options: DatePickerChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        self.pick_value(CalendarValue::Empty, options, window, cx)
    }

    /// The first month the calendar shows, like `"2026-10"`.
    pub fn get_month(&self) -> &ISOMonth {
        self.model.month()
    }

    /// Shows a month first. Returns false when canceled or unchanged.
    pub fn set_month(
        &mut self,
        month: impl Into<ISOMonth>,
        options: DatePickerChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        calendar_api::set_month(self, month.into(), options, window, cx)
    }

    /// Shows the months before, as the Previous part does.
    pub fn previous(&mut self, options: DatePickerChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        calendar_api::previous(self, options, window, cx)
    }

    /// Shows the months after, as the Next part does.
    pub fn next(&mut self, options: DatePickerChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        calendar_api::next(self, options, window, cx)
    }

    /// Focuses a date in the open calendar, turning the months to show it,
    /// or with `None` the date with the tab stop.
    pub fn focus(&mut self, date: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        calendar_api::focus(self, date, window, cx);
    }

    /// Shows the month a month or year select picked, as its change does:
    /// `value` is the month, 1 to 12, or the year. It's kept between `min`
    /// and `max`. Returns false when canceled or unchanged.
    pub fn select(
        &mut self,
        kind: CalendarSelectKind,
        value: i32,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        calendar_api::select(self, kind, value, event, window, cx)
    }

    // Turns the months to the selected date (`showValue`).
    fn show_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(first) = selected_dates(self.model.value()).into_iter().next() {
            calendar_api::reveal(self, &first, ChangeOptions::reason(Reason::Imperative), window, cx);
        }
    }

    // The popup opened, from any source: focus starts from the selected
    // date, not where it was left, and each open shows the selected date,
    // wherever browsing left the months, or with nothing selected, the month
    // the picker started on.
    fn opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.reset_focus(cx);
        match selected_dates(self.model.value()).into_iter().next() {
            Some(date) => {
                let target = clamp_date(
                    &date,
                    self.model.min().map(|min| min.as_ref()),
                    self.model.max().map(|max| max.as_ref()),
                );
                calendar_api::reveal(self, &target, ChangeOptions::reason(Reason::Imperative), window, cx);
            }
            None if *self.model.month() != self.first_month => {
                let month = self.first_month.clone();
                calendar_api::change_month(self, month, ChangeOptions::reason(Reason::Imperative), window, cx);
            }
            None => {}
        }
    }

    // The popup finished closing. A press on a day of another month turns
    // the months as it closes the popup, and the calendar moves focus to
    // that day once it renders, which the original's closing popup, inert,
    // refuses. gpui can't make it inert, so focus left in it goes back where
    // the popup returns focus.
    fn closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.popup.content_region().contains_focused(window, cx) {
            return;
        }
        let layer = self.popup.layer().clone();
        if let Some(target) = layer.return_target(None, false, window, cx) {
            restore_focus(&target, window, cx);
        }
    }

    // Asks the popup to open (`control.show`).
    pub(crate) fn show(&mut self, request: ShowRequest<()>, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        let control = self.popup.control().clone();
        control.show(self, request, None, window, cx)
    }

    // Asks the popup to close (`control.hide`).
    pub(crate) fn hide(&mut self, request: HideRequest, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let control = self.popup.control().clone();
        control.hide(self, request, window, cx)
    }

    fn clear_drafts(&mut self) {
        self.drafts = [None, None];
        self.invalid = [false, false];
    }

    // Text typed in an input (`setText`). Text typed over another value than
    // the drafts' starts them over.
    pub(crate) fn set_text(&mut self, index: usize, text: SharedString) {
        let key = value_key(self.model.value());
        let fresh = self.draft_for == key;
        let mut drafts = if fresh { self.drafts.clone() } else { [None, None] };
        drafts[index] = Some(text);
        self.drafts = drafts;
        self.draft_for = key;
        if !fresh {
            self.invalid = [false, false];
        }
    }

    // An edit in an input's field: its text becomes the input's draft, and
    // the form hears it, as from a native input.
    fn typed(&mut self, index: usize, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.set_text(index, text);
        dispatch_change(Some(self.internals.node()), &[FormEventType::Input], window, cx);
        cx.notify();
    }

    fn mark_invalid(&mut self, index: usize, cx: &mut Context<Self>) {
        self.invalid[index] = true;
        cx.notify();
    }

    // Reads an input's text: a date, nothing when empty, or not a date.
    fn read(&self, text: &str, cx: &App) -> Reading {
        if js_trim(text).is_empty() {
            return Reading::Empty;
        }
        let locale = self.model.locale(cx);
        let options = ParseDateOptions {
            reference_date: self.model.today().cloned(),
        };
        parse_date(text, Some(&locale), &options).map_or(Reading::NotADate, Reading::Date)
    }

    /// Enter or blur on an input: a typed date becomes the value, or the
    /// input is marked invalid (`commit`).
    pub(crate) fn commit(&mut self, index: usize, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft_for != value_key(self.model.value()) {
            return;
        }
        let Some(text) = self.drafts[index].clone() else {
            return;
        };
        let drafts = self.drafts.clone();
        let was_invalid = self.invalid;
        let parsed = self.read(&text, cx);
        let blocked = matches!(&parsed, Reading::Date(date) if self.model.blocked_date(date));
        if parsed == Reading::NotADate || blocked {
            return self.mark_invalid(index, cx);
        }
        let options = || ChangeOptions {
            reason: Some(Reason::InputCommit),
            event: event.clone(),
            source: None,
        };
        let typed = match &parsed {
            Reading::Date(date) => Some(date.clone()),
            _ => None,
        };

        if self.model.mode() != CalendarMode::Range {
            let value = typed.clone().map_or(CalendarValue::Empty, CalendarValue::Single);
            calendar_api::change_value(self, value, options(), window, cx);
            self.clear_drafts();
            if let Some(date) = &typed {
                calendar_api::reveal(self, date, options(), window, cx);
            }
            cx.notify();
            return;
        }

        let ends = [0, 1].map(|at| {
            if at == index {
                return parsed.clone();
            }
            match &drafts[at] {
                None => date_at(&self.model, at).map_or(Reading::Empty, Reading::Date),
                Some(draft) => self.read(draft, cx),
            }
        });
        match &ends {
            [Reading::Date(start), Reading::Date(end)] => {
                if start > end || !self.model.with_range_rules(|rules| is_allowed_range(start, end, rules)) {
                    return self.mark_invalid(index, cx);
                }
                let value = CalendarValue::Range(start.clone(), end.clone());
                calendar_api::change_value(self, value, options(), window, cx);
                self.model.set_range_anchor(None, None);
                self.clear_drafts();
                if let Some(date) = &typed {
                    calendar_api::reveal(self, date, options(), window, cx);
                }
            }
            [Reading::Empty, Reading::Empty] => {
                calendar_api::change_value(self, CalendarValue::Empty, options(), window, cx);
                self.model.set_range_anchor(None, None);
                self.clear_drafts();
            }
            _ => {
                // One end: no value yet. The other end waits in its input,
                // and a start waits in the calendar for its end too.
                calendar_api::change_value(self, CalendarValue::Empty, options(), window, cx);
                let locale = self.model.locale(cx);
                let kept = [0, 1].map(|at| {
                    Some(match &ends[at] {
                        Reading::Date(date) => short_date(date, &locale).into(),
                        Reading::Empty => SharedString::default(),
                        Reading::NotADate => drafts[at].clone().unwrap_or_default(),
                    })
                });
                let mut invalid = [false, false];
                invalid[1 - index] = was_invalid[1 - index];
                let start = match &ends[0] {
                    Reading::Date(date) => Some(date.clone()),
                    _ => None,
                };
                self.model.set_range_anchor(start.clone(), start);
                self.drafts = kept;
                self.draft_for = value_key(self.model.value());
                self.invalid = invalid;
                if let Some(date) = &typed {
                    calendar_api::reveal(self, date, options(), window, cx);
                }
            }
        }
        cx.notify();
    }

    // A value a preset or Clear selects (`pickValue`): false when people
    // couldn't pick it now, like one past `max`. A preset's turns the
    // calendar to it.
    pub(crate) fn pick_value(
        &mut self,
        value: DatePickerValue,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.model.is_pickable(&value, self.is_disabled()) {
            return false;
        }
        let normalized = normalize_value(self.model.mode(), &value);
        self.model.set_range_anchor(None, None);
        self.clear_drafts();
        let preset = options.reason == Some(Reason::PresetPress);
        let reveal = options.clone();
        let changed = calendar_api::change_value(self, normalized.clone(), options, window, cx);
        let first = match &normalized {
            CalendarValue::Single(date) | CalendarValue::Range(date, _) => Some(date.clone()),
            _ => None,
        };
        if let Some(first) = first.filter(|first| is_iso_date(first))
            && preset
        {
            calendar_api::reveal(self, &first, reveal, window, cx);
        }
        cx.notify();
        changed
    }

    // A form reset puts the default back. A controlled picker with no
    // default keeps its value.
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if keeps_value_on_reset(self.controlled.then_some(&()), self.has_default.then_some(&())) {
            return;
        }
        self.model.set_range_anchor(None, None);
        self.clear_drafts();
        let initial = self.initial.clone();
        calendar_api::change_value(self, initial, ChangeOptions::reason(Reason::Reset), window, cx);
        // Fields read the validity right after a reset.
        self.report(cx);
        cx.notify();
    }

    // Focuses the input whose text isn't a date, else the start's
    // (`focusInput`).
    pub(crate) fn focus_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.invalid_input().or_else(|| self.input_element()) else {
            return;
        };
        self.inputs[index]
            .field
            .update(cx, |field, cx| field.focus(window, cx));
    }

    // Keys on an input (`inputKeydown`): Enter commits the typed date, and
    // submits its form as a native input does, and Alt with Down Arrow opens
    // the calendar.
    pub(crate) fn input_keydown(&mut self, event: &KeyDownEvent, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let key = Key::of(&event.keystroke);
        if key == Key::Enter {
            self.commit(index, Some(InputEvent::from(event)), window, cx);
            self.report(cx);
            if !window.default_prevented() {
                submit_implicitly(self.internals.form().as_ref(), window, cx);
            }
        } else if key == Key::ArrowDown && event.keystroke.modifiers.alt && !self.popup.is_open() {
            window.prevent_default();
            let request = ShowRequest::new()
                .event(Some(InputEvent::from(event)))
                .by(PopupOpener::Keyboard);
            self.show(request, window, cx);
        }
    }

    // The day focus goes to as the popup opens: the one with the tab stop,
    // else the selected date, today or the first that can be picked. Kept
    // for the popup, which reads it once the content has rendered.
    pub(crate) fn note_focus_target(&self) {
        let date = self.model.focused().unwrap_or_else(|| self.model.preferred_date());
        *self.focus_target.borrow_mut() = Some(date);
    }

    // How the content binds: placed against its anchor prop, else the start
    // input, else the trigger, read every frame, and focus going to the
    // calendar's tab stop: the selected date, else today, else the first
    // that can be picked, else the content.
    fn content_bind(&self) -> SheetConfig<Self> {
        let popup = self.popup.weak();
        let anchors = Anchors {
            instance: self.instance.handle(),
            trigger: self.trigger_region.clone(),
            inputs: [self.inputs[0].region.clone(), self.inputs[1].region.clone()],
        };
        let content_props = {
            let popup = popup.clone();
            move || popup.upgrade().map(|popup| popup.content_props()).unwrap_or_default()
        };
        let anchor: Rc<dyn Fn() -> Option<Anchor>> = {
            let anchors = anchors.clone();
            let content_props = content_props.clone();
            Rc::new(move || content_anchor(content_props().position.anchor, || anchors.anchor().map(Anchor::element)))
        };
        let position: Rc<dyn Fn() -> PositionOptions> = Rc::new(move || {
            placement_options(&content_props().position, PlacementDefaults::new(Align::Start), None)
        });
        let initial_focus: InitialFocusFn = {
            let roving = self.model.roving().clone();
            let target = self.focus_target.clone();
            Rc::new(move |_window: &mut Window, cx: &mut App| {
                let date = target.borrow().clone();
                if let Some(date) = date.filter(|date| anchors.bound("day", Some(date))) {
                    return Some(roving.focus_handle(&date, cx));
                }
                popup.upgrade().map(|popup| popup.content_focus_handle())
            })
        };
        SheetConfig {
            attach: AttachConfig {
                anchor: Some(anchor),
                position: Some(position),
                initial_focus: Some(initial_focus),
                ..AttachConfig::default()
            },
            ..SheetConfig::default()
        }
    }
}

/// The message the form and a Field show for an input whose text isn't a
/// date that can be picked. English, like the trigger's label.
pub const INVALID_DATE_MESSAGE: &str = "Enter a valid date.";

impl PopupHost for DatePicker {
    type Payload = ();

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

/// Calendar's state and parts, over the picker's: a fieldset or a Field
/// disables it, and picking a date, or a range's end, closes the popup
/// unless `close_on_select` is off.
impl CalendarHost for DatePicker {
    fn calendar(&self) -> &CalendarModelState {
        &self.model
    }

    fn calendar_mut(&mut self) -> &mut CalendarModelState {
        &mut self.model
    }

    fn calendar_instance(&self) -> &Instance {
        &self.instance
    }

    fn calendar_split(&mut self) -> (&Instance, &mut CalendarModelState) {
        (&self.instance, &mut self.model)
    }

    fn calendar_form_disabled(&self) -> bool {
        self.internals.form_disabled() || self.internals.control_attrs().disabled
    }

    fn calendar_picked(&mut self, event: Option<&InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_on_select {
            let request = HideRequest::new().reason(Reason::ItemPress).event(event.cloned());
            self.hide(request, window, cx);
        }
    }
}
