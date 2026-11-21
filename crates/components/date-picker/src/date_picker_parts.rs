//! Port of `components/date-picker/date-picker-parts.ts`: Date Picker's parts
//! and what they carry: the inputs, the trigger, the popup, the hint, Clear
//! and presets, and Calendar's parts inside the popup.

use gpui::{App, Entity, Focusable, InteractiveElement, Role as AccessRole, StatefulInteractiveElement, Window};
use ui_sdk_calendar::calendar_api;
use ui_sdk_calendar::{
    CalendarDayState, CalendarGridState, CalendarHeadingState, CalendarMode, CalendarSelectState,
    CalendarStepState, CalendarValue, CalendarWeekdayState, normalize_value,
};
use ui_sdk_date::{ISODate, date_pattern, format_date};
use ui_sdk_kernel::part::{FocusMode, with_role};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, ChangeOptions, HasPopup, InputEvent, Part, Reason, flag_while_bound};
use ui_sdk_native::TextField;
use ui_sdk_popups::{
    HideRequest, Popup, PopupContent, PopupViewport, ShowRequest, bind_popup_content, opener_of,
    popup_trigger_attrs, popup_viewport, set_unfocusable,
};

use crate::date_picker::{
    DatePicker, DatePickerClearState, DatePickerContentProps, DatePickerContentState,
    DatePickerHintState, DatePickerInputState, DatePickerPresetState, DatePickerState,
    DatePickerTriggerState, DatePickerValue,
};
use crate::date_picker_api::{ENDS, value_key};

/// The keys an input handles: Enter commits the typed date, and Alt with
/// Down Arrow opens the calendar.
pub const INPUT_KEYS: &[&str] = &["Enter", "alt+ArrowDown"];

// The popup's name: English by default, like the trigger's.
fn content_label(picker: &DatePicker) -> &'static str {
    if picker.model.mode() == CalendarMode::Range {
        "Choose dates"
    } else {
        "Choose date"
    }
}

// The trigger's name, with the dates chosen (`triggerLabel`). English by
// default, like Calendar's Previous and Next.
fn trigger_label(picker: &DatePicker, cx: &App) -> String {
    let label = content_label(picker);
    let locale = picker.model.locale(cx);
    match picker.model.value() {
        CalendarValue::Range(start, end) => format!(
            "{label}, {} to {}",
            format_date(start, Some(&locale), None),
            format_date(end, Some(&locale), None)
        ),
        CalendarValue::Single(date) => format!("{label}, {}", format_date(date, Some(&locale), None)),
        _ => label.to_string(),
    }
}

impl DatePicker {
    // Whether a preset holds a value people couldn't pick now
    // (`presetDisabled`).
    fn preset_disabled(&self, value: &DatePickerValue) -> bool {
        !self.model.is_pickable(value, self.is_disabled())
    }

    // A press on the trigger: it opens the popup, or closes it while open.
    fn press_trigger(&mut self, event: InputEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.is_disabled() {
            return;
        }
        if self.popup.is_open() {
            let request = HideRequest::new().reason(Reason::TriggerPress).event(Some(event));
            self.hide(request, window, cx);
            return;
        }
        let by = opener_of(Some(&event));
        let request = ShowRequest::new()
            .event(Some(event))
            .by(by)
            .reason(Reason::TriggerPress);
        self.show(request, window, cx);
    }

    // A press on Clear: nothing is selected, and the input takes focus. A
    // disabled button takes no press.
    fn press_clear(&mut self, event: InputEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.is_disabled() || self.model.is_read_only() {
            return;
        }
        self.pick_value(CalendarValue::Empty, ChangeOptions::event_of(Reason::ClearPress, event), window, cx);
        self.focus_input(window, cx);
    }

    // A press on a preset: its value is selected, and the popup closes
    // unless `close_on_select` is off.
    fn press_preset(&mut self, value: &DatePickerValue, event: InputEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.preset_disabled(value) {
            return;
        }
        self.pick_value(
            value.clone(),
            ChangeOptions::event_of(Reason::PresetPress, event.clone()),
            window,
            cx,
        );
        if self.close_on_select {
            let request = HideRequest::new().reason(Reason::PresetPress).event(Some(event));
            self.hide(request, window, cx);
        }
    }

    /// The `root` part, around the picker's other parts. It takes part in
    /// forms: it submits the date under its name, as an ISO date, or each of
    /// a range's ends under theirs, and a form reset puts the default value
    /// back.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<DatePickerState> {
        let picker = this.read(cx);
        picker.report(cx);
        let internals = picker.internals.clone();
        Part::new(&picker.instance, "root", None, picker.state()).map_el(|el| internals.track(el))
    }

    /// An `input` part: a text field that reads a date typed in the locale's
    /// order, with any separator, month names or digits alone, on Enter or
    /// blur, and shows the date it holds in the locale's numbers. `end` makes
    /// it a range's end; the other holds the start, or the date. A Field, and
    /// a label for the picker, name the start's; name the end's with an
    /// `aria_label`. The hint describes both while it's in the page.
    pub fn input(this: &Entity<Self>, end: bool, cx: &App) -> Part<DatePickerInputState> {
        let picker = this.read(cx);
        let index = usize::from(end);
        let input = &picker.inputs[index];
        let disabled = picker.is_disabled();
        let read_only = picker.model.is_read_only();
        let invalid = picker.invalid_at(index);
        let state = DatePickerInputState {
            invalid,
            disabled,
            read_only,
        };
        let handle = input.field.read(cx).focus_handle(cx);
        let (el, _) = TextField::root(&input.field, cx).into_element_parts();
        let el = input.region.measure(input.node.track(el));
        let hinted = flag_while_bound(&picker.instance, "hint").unwrap_or(false);
        let mut aria = Aria::new()
            .invalid(invalid)
            .disabled(disabled)
            .read_only(read_only)
            .required(picker.required);
        if hinted {
            aria = aria.described_by(picker.instance.element_id("hint", None));
        }
        // What a Field and the labels write go on the start's, after its own
        // description.
        if !end {
            aria = aria.merge(picker.internals.aria());
        }
        let keyed = this.downgrade();
        let part = Part::new(&picker.instance, "input", Some(ENDS[index]), state)
            .map_el(move |_| el)
            .aria_role(Role::Textbox)
            .focus_mode(FocusMode::Native, Some(&handle), disabled)
            .aria(aria)
            .on_keys(INPUT_KEYS, move |event, window, cx| {
                keyed
                    .update(cx, |picker, cx| picker.input_keydown(event, index, window, cx))
                    .ok();
            });
        part
    }

    /// The `trigger` part: a button that opens the calendar, and closes it
    /// while it's open, with `aria-haspopup="dialog"` and `aria-expanded`.
    /// Named "Choose date" and the date chosen, or "Choose dates" and the
    /// range; give it an `aria_label` in another language.
    pub fn trigger(this: &Entity<Self>, cx: &App) -> Part<DatePickerTriggerState> {
        let picker = this.read(cx);
        let popup = picker.popup.clone();
        let popup_state = popup.state(picker);
        let disabled = picker.is_disabled();
        let state = DatePickerTriggerState {
            open: popup_state.open,
            disabled,
        };
        let attrs = popup_trigger_attrs(
            popup_state.open,
            popup_state.mounted,
            popup_state.trigger.as_deref(),
            picker.instance.element_id("content", None),
            Some(HasPopup::Dialog),
            "",
        );
        // A disabled button can't take focus, so focus a closing popup hands
        // back skips it.
        set_unfocusable(&picker.trigger_focus, disabled);
        let mut aria = Aria::new().has_popup(attrs.has_popup).disabled(disabled);
        if let Some(controls) = attrs.controls {
            aria = aria.controls(controls);
        }
        let pressed = this.downgrade();
        let keyed = this.downgrade();
        let part = Part::new(&picker.instance, "trigger", None, state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&picker.trigger_focus), disabled)
            .aria_label(trigger_label(picker, cx))
            .aria_expanded(attrs.open)
            .aria(aria)
            .on_press(move |event, window, cx| {
                pressed
                    .update(cx, |picker, cx| picker.press_trigger(event, window, cx))
                    .ok();
            })
            // Tab from the trigger enters the open popup, wherever it renders.
            .map_el(move |el| {
                el.on_key_down(move |event, window, cx| {
                    let Some(target) = keyed.upgrade() else {
                        return;
                    };
                    let popup = target.read(cx).popup.clone();
                    let took = target.update(cx, |picker, cx| popup.trigger_keydown(picker, event, window, cx));
                    if took {
                        cx.stop_propagation();
                    }
                })
            });
        picker.trigger_region.measure(part)
    }

    /// The `content` part: the popup, a dialog that holds the calendar's
    /// parts, named "Choose date" or "Choose dates"; give it an `aria_label`
    /// in another language. It renders above the window, below the input
    /// that holds the start, or the trigger without one, and lined up with
    /// its start, while it's open, animating out, or kept in the page with
    /// `keep_mounted`; `None` otherwise. Opening moves focus to the selected
    /// date, else today, else the first date that can be picked.
    pub fn content(
        this: &Entity<Self>,
        props: DatePickerContentProps,
        cx: &App,
    ) -> Option<PopupContent<Self, DatePickerContentState>> {
        let picker = this.read(cx);
        let popup = picker.popup.clone();
        let state = popup.state(picker);
        let popup_props = props.popup_props();
        if !state.mounted && !props.keep_mounted {
            popup.set_content_props(popup_props);
            return None;
        }
        // The days it holds render in it.
        picker.model.roving().begin(cx);
        picker.note_focus_target();
        let layer = popup.layer();
        let content_state = DatePickerContentState {
            open: state.open,
            presentation: state.presentation,
            presence: layer.presence().state(),
            placement: layer.positioner().and_then(|positioner| positioner.placement()),
            hidden: !state.mounted,
            sheet: popup.sheet().map(|sheet| sheet.part_state()),
        };
        let handle = popup.content_focus_handle();
        let keyed = this.downgrade();
        let part = Part::new(&picker.instance, "content", None, content_state)
            .aria_role(Role::Dialog)
            .map_el(move |el| el.track_focus(&handle.tab_stop(false)))
            .aria_label(content_label(picker))
            .aria(Aria::new().hidden(!state.mounted))
            // Keys from a popup inside this one bubble through it. Only the
            // owner handles them.
            .on_keys(&["Tab", "Escape"], move |event, window, cx| {
                Popup::content_keydown(&keyed, event, window, cx);
            });
        Some(bind_popup_content(this, part, popup_props, cx))
    }

    /// The `viewport` part: the full-window element the popup sits in while
    /// it presents as a sheet, with the sheet's dim. Put the content inside
    /// it. `None` unless it presents as a sheet.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> Option<PopupViewport<Self>> {
        popup_viewport(this, cx)
    }

    /// The `hint` part: the format to type, like "MM/DD/YYYY", or an example
    /// date, from its state. The inputs are described by it while it's in the
    /// page. It's named by its format; give it an `aria_label` with the
    /// example when it shows that.
    pub fn hint(this: &Entity<Self>, cx: &App) -> Part<DatePickerHintState> {
        let picker = this.read(cx);
        let pattern = date_pattern(Some(&picker.model.locale(cx)));
        let state = DatePickerHintState {
            placeholder: pattern.placeholder.clone().into(),
            example: pattern.example.into(),
        };
        Part::new(&picker.instance, "hint", None, state)
            .map_el(|el| with_role(el, AccessRole::Label))
            .aria_label(pattern.placeholder)
    }

    /// The `clear` part: a button that selects nothing and focuses the input,
    /// disabled while the picker is disabled or read-only. `clear` is the
    /// api's.
    pub fn clear_part(this: &Entity<Self>, cx: &App) -> Part<DatePickerClearState> {
        let picker = this.read(cx);
        let disabled = picker.is_disabled() || picker.model.is_read_only();
        let state = DatePickerClearState {
            empty: picker.model.value().is_empty() && picker.model.anchor().is_none(),
            disabled,
        };
        let pressed = this.downgrade();
        Part::new(&picker.instance, "clear", None, state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&picker.clear_focus), disabled)
            .aria(Aria::new().disabled(disabled))
            .on_press(move |event, window, cx| {
                pressed
                    .update(cx, |picker, cx| picker.press_clear(event, window, cx))
                    .ok();
            })
    }

    /// A `preset` part: a button that selects the value it holds, like the
    /// last 7 days, and closes the popup unless `close_on_select` is off.
    /// Relative dates are the app's to work out. It's pressed while its value
    /// is the one selected, and disabled while it holds one people can't
    /// pick, like a date past `max`.
    pub fn preset(this: &Entity<Self>, value: DatePickerValue, cx: &App) -> Part<DatePickerPresetState> {
        let picker = this.read(cx);
        let key = value_key(&value);
        let selected = value_key(picker.model.value()) == value_key(&normalize_value(picker.model.mode(), &value));
        let disabled = picker.preset_disabled(&value);
        let handle = picker.presets.focus_handle(&key.clone().into(), cx);
        let state = DatePickerPresetState { selected, disabled };
        let pressed = this.downgrade();
        Part::new(&picker.instance, "preset", Some(&key), state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&handle), disabled)
            .aria(Aria::new().pressed(selected).disabled(disabled))
            .on_press(move |event, window, cx| {
                pressed
                    .update(cx, |picker, cx| picker.press_preset(&value, event, window, cx))
                    .ok();
            })
    }

    /// Calendar's `heading` part: the months shown, like "October 2026",
    /// which screen readers hear change.
    pub fn heading(this: &Entity<Self>, cx: &App) -> Part<CalendarHeadingState> {
        calendar_api::heading(this, cx)
    }

    /// Calendar's `previous` part: a button that shows the months before.
    pub fn previous_part(this: &Entity<Self>, cx: &App) -> Part<CalendarStepState> {
        calendar_api::previous_part(this, cx)
    }

    /// Calendar's `next` part: a button that shows the months after.
    pub fn next_part(this: &Entity<Self>, cx: &App) -> Part<CalendarStepState> {
        calendar_api::next_part(this, cx)
    }

    /// Calendar's `month-select` part, where a select of the months goes. It
    /// calls [`DatePicker::select`] with the month picked.
    pub fn month_select(this: &Entity<Self>, cx: &App) -> Part<CalendarSelectState> {
        calendar_api::month_select(this, cx)
    }

    /// Calendar's `year-select` part, where a select of the years goes.
    pub fn year_select(this: &Entity<Self>, cx: &App) -> Part<CalendarSelectState> {
        calendar_api::year_select(this, cx)
    }

    /// Calendar's `grid` part: the month at `offset` from the first shown,
    /// from 0. Render its weekday headers and weeks inside it.
    pub fn grid(this: &Entity<Self>, offset: usize, cx: &App) -> Part<CalendarGridState> {
        calendar_api::grid(this, offset, cx)
    }

    /// Calendar's `weekdays` part: the header row of the grid at `offset`.
    pub fn weekdays(this: &Entity<Self>, offset: usize, cx: &App) -> Part {
        calendar_api::weekdays(this, offset, cx)
    }

    /// Calendar's `weekday` part: the header of column `index` in the grid at
    /// `offset`.
    pub fn weekday(this: &Entity<Self>, offset: usize, index: usize, cx: &App) -> Part<CalendarWeekdayState> {
        calendar_api::weekday(this, offset, index, cx)
    }

    /// Calendar's `week` part: the row of week `index` in the grid at
    /// `offset`.
    pub fn week(this: &Entity<Self>, offset: usize, index: usize, cx: &App) -> Part {
        calendar_api::week(this, offset, index, cx)
    }

    /// Calendar's `day` part: the cell of `date` in the grid at `offset`.
    /// Pass each cell of the grid's weeks that isn't hidden. Picking one, or
    /// a range's end, closes the popup unless `close_on_select` is off, and
    /// focus goes back to the trigger.
    pub fn day(this: &Entity<Self>, date: impl Into<ISODate>, offset: usize, cx: &App) -> Part<CalendarDayState> {
        calendar_api::day(this, date, offset, cx)
    }
}
