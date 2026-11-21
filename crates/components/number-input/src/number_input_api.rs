//! Port of `components/number-input/number-input-api.ts`: a number input's
//! state and what it does: typing, keys, buttons, the wheel, scrubs and
//! commits. The parts and their attributes are in `number_input.rs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, ClickEvent, Context, Focusable, KeyDownEvent, KeyUpEvent, Modifiers, Pixels, Point, ScrollEvent,
    ScrollSource, SharedString, Window, px,
};
use ui_sdk_direct_manipulation::{BindRepeatOptions, HoldDetails, HoldInput, RepeatDetails};
use ui_sdk_drag_and_drop::{BindDragOptions, PointerEvent, is_drag};
use ui_sdk_forms::{
    FormEventType, FormValue, InternalsOptions, ValueMissingKind, bind_internals, dispatch_change,
    keeps_value_on_reset, value_missing_message,
};
use ui_sdk_kernel::modality::focus_event;
use ui_sdk_kernel::{ChangeOptions, ChangeSource, InputEvent, Key, Reason, resolve_source, with_source};
use ui_sdk_native::{BeforeEdit, FieldHook, InputType, TextField};
use ui_sdk_number::{
    Arrows, NumberBound, NumberFormat, NumberFormatOptions, NumberKeyOptions, NumberMove, NumberStyle,
    StepRange, StepSize, clamp, decimals_of, display_options, format_number, is_partial_number, max_on_grid,
    parse_number, round_to, snap_to_step, step_from_key, step_value,
};
use ui_sdk_text::{js_trim, locale_from};

use crate::number_input::{
    NumberInput, NumberInputOptions, NumberInputScrubAreaOrientation, NumberInputScrubAreaProps,
};

/// What the options set, besides the value and the callbacks (`settings`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) step: f64,
    pub(crate) large_step: f64,
    pub(crate) small_step: f64,
    pub(crate) format: Option<NumberFormatOptions>,
    pub(crate) locale: Option<SharedString>,
    pub(crate) wheel: bool,
    pub(crate) disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) name: Option<SharedString>,
    pub(crate) form: Option<SharedString>,
}

impl Settings {
    /// Its bounds and step (`rangeOf`).
    pub(crate) fn range(&self) -> StepRange {
        StepRange::new(self.min, self.max, self.step)
    }

    /// The text is read with the options it shows with, whose decimals can
    /// change the plural of a unit or currency name, like "1,5 японской
    /// иены" (`readFormat`).
    pub(crate) fn read_format(&self) -> NumberFormatOptions {
        display_options(self.format.as_ref(), self.step)
    }
}

pub(crate) fn settings(options: &NumberInputOptions) -> Settings {
    // A percent's value is a fraction, so a step of 1 would snap 25% to 0%,
    // and each fraction digit it shows is two more places in the value.
    let format = options.format.as_ref();
    let fallback = match format {
        Some(format) if format.style == Some(NumberStyle::Percent) => {
            let digits = format
                .maximum_fraction_digits
                .or(format.minimum_fraction_digits)
                .unwrap_or(0);
            tenth_power(2 + i32::from(digits))
        }
        _ => 1.0,
    };
    let step = options.step.filter(|step| *step > 0.0).unwrap_or(fallback);
    Settings {
        min: options
            .min
            .filter(|min| !min.is_nan())
            .unwrap_or(f64::NEG_INFINITY),
        max: options.max.filter(|max| !max.is_nan()).unwrap_or(f64::INFINITY),
        step,
        large_step: options
            .large_step
            .filter(|step| *step > 0.0)
            .unwrap_or(step * 10.0),
        small_step: options
            .small_step
            .filter(|step| *step > 0.0)
            .unwrap_or_else(|| round_to(step / 10.0, decimals_of(step) + 1)),
        format: options.format.clone(),
        locale: options.locale.clone(),
        wheel: options.wheel,
        disabled: options.disabled,
        read_only: options.read_only,
        required: options.required,
        name: options.name.clone(),
        form: options.form.clone(),
    }
}

// `10 ** -digits`, as JavaScript reads it: the nearest double to the
// decimal.
fn tenth_power(digits: i32) -> f64 {
    format!("1e-{digits}").parse().unwrap_or(0.0)
}

/// A value as the input shows it, formatted in its locale (`display`).
pub(crate) fn display(value: Option<f64>, settings: &Settings, locale: &str) -> SharedString {
    let format = NumberFormat::Options(settings.read_format());
    format_number(value, Some(locale), Some(&format)).into()
}

/// What the parse hook reads, kept current with the settings and the locale:
/// the field asks it while it edits, when the input may be mid-update.
#[derive(Clone, Debug, Default)]
pub(crate) struct Reading {
    pub(crate) locale: SharedString,
    pub(crate) format: NumberFormatOptions,
    pub(crate) allow_negative: bool,
}

impl Reading {
    pub(crate) fn of(settings: &Settings, locale: &SharedString) -> Self {
        Self {
            locale: locale.clone(),
            format: settings.read_format(),
            allow_negative: settings.min < 0.0,
        }
    }
}

/// Refuses an edit that couldn't become a number, like a letter. Deleting is
/// always allowed (`beforeinput`).
pub(crate) fn before_input(reading: Rc<RefCell<Reading>>) -> FieldHook<BeforeEdit> {
    Rc::new(move |field: &mut TextField, edit: &BeforeEdit, _window, _cx| {
        // A line break carries no text, the browser's `data` of `null`.
        if edit.composing || !edit.input_type.is_insert() || edit.input_type == InputType::InsertLineBreak {
            return;
        }
        let value = field.value();
        let (Some(before), Some(after)) = (value.get(..edit.range.start), value.get(edit.range.end..)) else {
            return;
        };
        let next = format!("{before}{}{after}", edit.data);
        let reading = reading.borrow();
        if !is_partial_number(
            &next,
            Some(&reading.locale),
            Some(&reading.format),
            reading.allow_negative,
        ) {
            edit.prevent_default();
        }
    })
}

/// What changed the value since the interaction began, reported at the next
/// commit (`pending`).
#[derive(Clone, Debug)]
pub(crate) struct Pending {
    pub(crate) reason: Reason,
    pub(crate) event: Option<InputEvent>,
    // How it was caused, for a commit whose event doesn't say.
    pub(crate) source: ChangeSource,
}

// The kind of DOM event an event stands for, so a commit's details carry an
// event of the kind its reason lists (`event.constructor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventKind {
    Keyboard,
    Wheel,
    Focus,
    Pinch,
    Pointer,
}

fn event_kind(event: &InputEvent) -> EventKind {
    match event {
        InputEvent::KeyDown(_) | InputEvent::KeyUp(_) => EventKind::Keyboard,
        InputEvent::Scroll(_) => EventKind::Wheel,
        InputEvent::Focus(_) => EventKind::Focus,
        InputEvent::Pinch(_) => EventKind::Pinch,
        _ => EventKind::Pointer,
    }
}

/// Which step a gesture moves by: Shift picks the large one, and Alt the
/// small one (`stepSize`).
pub(crate) fn step_size(modifiers: Modifiers) -> StepSize {
    if modifiers.shift {
        StepSize::Large
    } else if modifiers.alt {
        StepSize::Small
    } else {
        StepSize::Step
    }
}

// The modifiers a click carries, or none for a screen reader's.
fn click_modifiers(event: &InputEvent) -> Modifiers {
    match event {
        InputEvent::Click(click) => click.modifiers(),
        _ => Modifiers::default(),
    }
}

/// A number as `String(value)` writes it, which the form submits.
pub(crate) fn number_text(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if value == 0.0 {
        return "0".into();
    }
    if (1e-6..1e21).contains(&value.abs()) {
        return value.to_string();
    }
    let text = format!("{value:e}");
    match text.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
        _ => text,
    }
}

impl NumberInput {
    /// The language the input formats and reads numbers in: its `locale`,
    /// else the page's, else English (`localeOf`).
    pub(crate) fn locale(&self) -> SharedString {
        locale_from(self.settings.locale.as_ref(), self.page_locale.as_ref())
    }

    /// Whether it's disabled, by itself, by a fieldset or by its field
    /// (`isDisabled`).
    pub fn is_disabled(&self) -> bool {
        self.settings.disabled || self.internals.form_disabled() || self.internals.control_attrs().disabled
    }

    /// Whether typing, steps and scrubs leave it alone (`isLocked`).
    pub(crate) fn is_locked(&self) -> bool {
        self.is_disabled() || self.settings.read_only
    }

    /// Whether the value is at the bound a step in this direction moves
    /// toward (`atBound`).
    pub(crate) fn at_bound(&self, direction: f64, step: f64) -> bool {
        let Some(value) = *self.value.get() else {
            return false;
        };
        if direction > 0.0 {
            value
                >= max_on_grid(StepRange {
                    step,
                    ..self.settings.range()
                })
        } else {
            value <= self.settings.min
        }
    }

    /// Whether it can step in this direction (`canStep`).
    pub(crate) fn can_step(&self, direction: f64, step: f64) -> bool {
        !self.is_locked() && !self.at_bound(direction, step)
    }

    // Reads text in the input's locale and format.
    fn parse(&self, text: &str) -> Option<f64> {
        parse_number(text, Some(&self.locale()), Some(&self.settings.read_format()))
    }

    // What the text reads as: `None` when it's empty or isn't a number.
    pub(crate) fn typed(&self) -> Option<f64> {
        if js_trim(&self.text).is_empty() {
            return None;
        }
        self.parse(&self.text)
    }

    /// The value typed so far, or the value when nothing is typed (`current`).
    fn current(&self) -> Option<f64> {
        if js_trim(&self.text).is_empty() {
            return None;
        }
        self.parse(&self.text).or(*self.value.get())
    }

    /// A value as the input shows it.
    pub(crate) fn display(&self, value: Option<f64>) -> SharedString {
        display(value, &self.settings, &self.locale())
    }

    /// The text shows the value, formatted (`showValue`).
    pub(crate) fn show_value(&mut self) {
        self.text = self.display(*self.value.get());
    }

    /// A value from outside, like a controlled one, shows unless it's what's
    /// typed. A new format, locale or step shows the value again, unless the
    /// user is typing. The original subscribes to its store for this; here
    /// each change ends with it.
    pub(crate) fn follow(&mut self) {
        let format = self.format_key();
        if format != self.shown_format {
            self.shown_format = format;
            if !self.edited {
                self.show_value();
            }
        }
        let value = *self.value.get();
        if value == self.shown {
            return;
        }
        self.shown = value;
        if self.typed() != value {
            self.text = self.display(value);
        }
    }

    // What decides how the value shows: the format, the locale and the step.
    pub(crate) fn format_key(&self) -> (Option<NumberFormatOptions>, SharedString, f64) {
        (self.settings.format.clone(), self.locale(), self.settings.step)
    }

    // The parse hook's copy of what it reads.
    pub(crate) fn sync_reading(&self) {
        *self.reading.borrow_mut() = Reading::of(&self.settings, &self.locale());
    }

    /// Reports the value to its form, as the original's hidden input and its
    /// native input do between them: the number as is, like `1234.5`, never
    /// formatted, or empty for an empty field, under `name`; and a missing
    /// value while it's required and its text is empty. A read-only field
    /// isn't validated.
    pub(crate) fn sync_form(&self) {
        let value = match *self.value.get() {
            Some(value) => number_text(value),
            None => String::new(),
        };
        bind_internals(
            &self.internals,
            InternalsOptions {
                value: FormValue::Text(value.into()),
                name: self.settings.name.clone(),
                value_missing: self.settings.required && self.text.is_empty(),
                message: value_missing_message(ValueMissingKind::Text),
                disabled: self.settings.disabled,
                read_only: self.settings.read_only,
                required: self.settings.required,
                barred: self.settings.read_only,
                form: self.settings.form.clone(),
                ..Default::default()
            },
        );
    }

    /// After a change: the text follows, the form hears it, the field shows
    /// the text and the parts render.
    pub(crate) fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.follow();
        self.sync_form();
        self.write_text(window, cx);
        cx.notify();
    }

    /// The text is written here, so the field never fights the caret
    /// (`write`).
    pub(crate) fn write_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.write_queued = false;
        let text = self.text.clone();
        self.field.update(cx, |field, cx| {
            if *field.value() != text {
                field.set_value(text, window, cx);
            }
        });
    }

    /// Writes the text once a window is at hand, for a change no window
    /// came with, like the page's language changing.
    pub(crate) fn queue_write(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.write_queued, true) {
            return;
        }
        let window = self.window;
        let entity = cx.weak_entity();
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    entity
                        .update(cx, |input, cx| {
                            if input.write_queued {
                                input.write_text(window, cx);
                            }
                        })
                        .ok();
                })
                .ok();
        });
    }

    /// Asks for a value an interaction typed or stepped to, and notes it for
    /// the commit (`setValue`).
    pub(crate) fn set_value_from(
        &mut self,
        value: Option<f64>,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let before = *self.value.get();
        let source = resolve_source(reason, event.as_ref());
        let options = ChangeOptions {
            reason: Some(reason),
            event: event.clone(),
            source: None,
        };
        let done = self.value.change(&self.instance, value, options, window, cx);
        if done {
            if self.pending.is_none() {
                self.start_value = before;
            }
            self.pending = Some(Pending {
                reason,
                event,
                source,
            });
            // `change` waits for the commit, like a native input's.
            dispatch_change(Some(self.internals.node()), &[FormEventType::Input], window, cx);
        }
        done
    }

    /// Reports the value once an interaction ends, with the event that ended
    /// it when it's the same kind as the one behind the change, so
    /// `details.event` stays the kind its reason lists, like a key up for a
    /// key's step (`commit`). `value` is the value it commits when the app
    /// hasn't passed it back yet.
    pub(crate) fn commit(
        &mut self,
        end: Option<InputEvent>,
        value: Option<Option<f64>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.wheel_timer = None;
        let value = value.unwrap_or(*self.value.get());
        let Some(last) = self.pending.take() else {
            return;
        };
        if self.start_value == value {
            return;
        }
        let event = match (end, &last.event) {
            (Some(end), Some(event)) if event_kind(&end) == event_kind(event) => Some(end),
            _ => last.event.clone(),
        };
        with_source(last.source, || {
            self.value
                .commit_value(&self.instance, &value, last.reason, event, window, cx)
        });
        dispatch_change(Some(self.internals.node()), &[FormEventType::Change], window, cx);
    }

    /// Steps from what's typed, or from 0 when empty, kept within the
    /// bounds. `steps` is signed, like -2 for two steps down. A small step
    /// snaps to its own grid, so a fine change isn't rounded away by `step`
    /// (`stepBy`).
    pub(crate) fn step_by(
        &mut self,
        steps: f64,
        size: StepSize,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let settings = &self.settings;
        let grid = if size == StepSize::Small {
            settings.small_step
        } else {
            settings.step
        };
        if !self.can_step(steps, grid) {
            return false;
        }
        let range = settings.range();
        let next = match self.current() {
            None => step_value(0.0, steps, range),
            Some(from) => match size {
                StepSize::Large => snap_to_step(
                    clamp(from + steps * settings.large_step, settings.min, settings.max),
                    range,
                ),
                StepSize::Small => step_value(
                    from,
                    steps,
                    StepRange {
                        step: settings.small_step,
                        ..range
                    },
                ),
                StepSize::Step => step_value(from, steps, range),
            },
        };
        let done = self.set_value_from(Some(next), reason, event, window, cx);
        // The text follows a step, even one that lands on the same value.
        self.edited = false;
        self.show_value();
        self.changed(window, cx);
        done
    }

    /// A step from code isn't an interaction, so like `set_value` it
    /// commits nothing, and leaves an interaction in progress to commit as
    /// it would (`stepFromCode`).
    fn step_from_code(
        &mut self,
        steps: f64,
        size: StepSize,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let interaction = self.pending.clone();
        let reason = options.reason.unwrap_or(Reason::Imperative);
        let done = self.step_by(steps, size, reason, options.event, window, cx);
        self.pending = interaction;
        done
    }

    /// The value, or `None` when the field is empty.
    pub fn get_value(&self) -> Option<f64> {
        *self.value.get()
    }

    /// Sets the value, as a user would. It's clamped and snapped to the
    /// step. Returns false when nothing changed or the change was canceled.
    pub fn set_value(
        &mut self,
        value: Option<f64>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let next = value.map(|value| {
            snap_to_step(
                clamp(value, self.settings.min, self.settings.max),
                self.settings.range(),
            )
        });
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        let done = self.value.change(&self.instance, next, options, window, cx);
        self.changed(window, cx);
        done
    }

    /// Steps the value up, by `large_step` with `large`. Returns false when
    /// it can't step or the change was canceled.
    pub fn increment(
        &mut self,
        options: ChangeOptions,
        large: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let size = if large { StepSize::Large } else { StepSize::Step };
        self.step_from_code(1.0, size, options, window, cx)
    }

    /// Steps the value down, by `large_step` with `large`. Returns false when
    /// it can't step or the change was canceled.
    pub fn decrement(
        &mut self,
        options: ChangeOptions,
        large: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let size = if large { StepSize::Large } else { StepSize::Step };
        self.step_from_code(-1.0, size, options, window, cx)
    }

    /// Moves focus to the input. A disabled input can't take it.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let field = self.field.read(cx);
        if field.is_disabled() {
            return;
        }
        let handle = field.focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// The value follows the text while it's a number within the bounds.
    /// Committing clamps and snaps it (`type`).
    pub(crate) fn type_text(
        &mut self,
        text: SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edited = true;
        self.text = text;
        if js_trim(&self.text).is_empty() {
            self.set_value_from(None, Reason::InputChange, event, window, cx);
        } else if let Some(parsed) = self.parse(&self.text)
            && parsed >= self.settings.min
            && parsed <= self.settings.max
        {
            self.set_value_from(Some(parsed), Reason::InputChange, event, window, cx);
        }
        self.changed(window, cx);
    }

    /// Enter or leaving the field: parses, clamps, snaps and formats what's
    /// typed. Text that isn't a number goes back to the last value
    /// (`commitText`).
    pub(crate) fn commit_text(
        &mut self,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_locked() {
            return;
        }
        if !self.edited {
            self.show_value();
            self.commit(event, None, window, cx);
            self.changed(window, cx);
            return;
        }
        self.edited = false;
        // A controlled value arrives after this event, so its commit reports
        // the value just asked for.
        let mut asked = *self.value.get();
        let empty = js_trim(&self.text).is_empty();
        let parsed = if empty { None } else { self.parse(&self.text) };
        let next = match parsed {
            Some(parsed) => Some(snap_to_step(
                clamp(parsed, self.settings.min, self.settings.max),
                self.settings.range(),
            )),
            None if empty => None,
            None => asked,
        };
        // Asking for the value it has would stand over a change in progress,
        // like a step whose `on_value_change` moved focus away.
        if next != asked && self.set_value_from(next, Reason::InputCommit, event.clone(), window, cx) {
            asked = next;
        }
        self.show_value();
        let controlled = self.value.is_controlled();
        self.commit(event, controlled.then_some(asked), window, cx);
        self.changed(window, cx);
    }

    /// A key on the control (`keydown`).
    pub(crate) fn keydown(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = Key::of(&event.keystroke);
        if key == Key::Enter {
            self.commit_text(Some(InputEvent::from(event)), window, cx);
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform {
            return;
        }
        // Left and Right move the caret, and Alt leaves Page Up, Page Down,
        // Home and End to it too.
        let options = NumberKeyOptions::default().arrows(Arrows::Vertical);
        let Some(motion) = step_from_key(&event.keystroke, options, window, cx) else {
            return;
        };
        let arrow = matches!(
            key,
            Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight
        );
        if modifiers.alt && !arrow {
            return;
        }
        match motion {
            NumberMove::Step { direction, size } => {
                window.prevent_default();
                self.step_by(
                    direction.sign(),
                    size,
                    Reason::StepKey,
                    Some(InputEvent::from(event)),
                    window,
                    cx,
                );
            }
            NumberMove::Bound(bound) => {
                // Home and End go to a bound only when there is one.
                // Otherwise they move the caret.
                let target = match bound {
                    NumberBound::Min => self.settings.min,
                    NumberBound::Max => max_on_grid(self.settings.range()),
                };
                if !target.is_finite() || self.is_locked() {
                    return;
                }
                window.prevent_default();
                self.set_value_from(
                    Some(target),
                    Reason::StepKey,
                    Some(InputEvent::from(event)),
                    window,
                    cx,
                );
                self.show_value();
                self.changed(window, cx);
            }
        }
    }

    /// A key coming up commits the steps its press made (`keyup`).
    pub(crate) fn keyup(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.reason == Reason::StepKey)
        {
            self.commit(Some(InputEvent::from(event)), None, window, cx);
        }
    }

    /// The wheel over the control (`wheel`).
    pub(crate) fn wheel(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Only a focused field takes the wheel, so the page still scrolls
        // past it. Pinch zoom and sideways scrolls are left alone.
        // A finger's pan sends no wheel in a browser.
        if !self.settings.wheel
            || !self.field.read(cx).is_focused(window)
            || event.modifiers.control
            || event.source == ScrollSource::Touch
        {
            return;
        }
        // gpui's deltas point the way the content moves, the browser's the
        // way the view does.
        let delta = event.delta.pixel_delta(window.line_height());
        let (dx, dy) = (-f32::from(delta.x), -f32::from(delta.y));
        if dx.abs() > dy.abs() || dy == 0.0 {
            return;
        }
        cx.stop_propagation();
        let steps = if dy < 0.0 { 1.0 } else { -1.0 };
        let size = step_size(event.modifiers);
        self.step_by(
            steps,
            size,
            Reason::Wheel,
            Some(InputEvent::from(event)),
            window,
            cx,
        );
        // A wheel has no release, so a pause commits.
        let end = InputEvent::from(event);
        self.wheel_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            this.update_in(cx, |input, window, cx| {
                input.commit(Some(end), None, window, cx);
                input.changed(window, cx);
            })
            .ok();
        }));
    }

    /// How an increment or decrement button repeats while it's held
    /// (`bindButton`). Its keys are gpui's: a keyboard click, which
    /// [`click_button`](Self::click_button) takes.
    pub(crate) fn button_options(direction: f64) -> BindRepeatOptions<Self> {
        let reason = if direction > 0.0 {
            Reason::IncrementPress
        } else {
            Reason::DecrementPress
        };
        BindRepeatOptions::new()
            .keys([])
            .disabled(move |input: &Self| input.button_disabled(direction))
            .on_press(move |input, press, window, cx| input.button_press(direction, press, window, cx))
            .on_step(move |input, step: &RepeatDetails, window, cx| {
                let size = if step.modifiers().shift {
                    StepSize::Large
                } else {
                    StepSize::Step
                };
                input.step_by(direction, size, reason, Some(step.event.clone()), window, cx)
                    && input.can_step(direction, input.settings.step)
            })
            .on_end(|input, end, window, cx| {
                input.commit(Some(end.event.clone()), None, window, cx);
                input.changed(window, cx);
            })
    }

    // Whether a button's press can't start: its own `disabled`, or the
    // native `disabled` its part renders with.
    fn button_disabled(&self, direction: f64) -> bool {
        let own = self.button_prop(direction).get();
        own || self.is_natively_disabled(own, direction)
    }

    pub(crate) fn button_prop(&self, direction: f64) -> &Cell<bool> {
        if direction > 0.0 {
            &self.increment_disabled
        } else {
            &self.decrement_disabled
        }
    }

    /// Whether a step button can't step: it's disabled itself, or the field
    /// can't step that way (`isBlocked`).
    pub(crate) fn is_blocked(&self, own: bool, direction: f64) -> bool {
        own || !self.can_step(direction, self.settings.step)
    }

    /// Read-only buttons stay in the page as they were, but do nothing: the
    /// native `disabled` a button renders with.
    pub(crate) fn is_natively_disabled(&self, own: bool, direction: f64) -> bool {
        own || (self.is_blocked(own, direction) && !self.settings.read_only)
    }

    fn button_press(
        &mut self,
        direction: f64,
        press: &HoldDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let drags = self.drags.clone();
        let accepted = press
            .event
            .pointer()
            .is_none_or(|event| drags.accepts(self, event, window, cx));
        if !accepted || !self.can_step(direction, self.settings.step) {
            return false;
        }
        // Keeps focus where it is, so a phone's keyboard stays as it was. A
        // finger's press is left unclaimed, so a scroll from the button
        // still scrolls.
        if press.input == HoldInput::Mouse {
            window.prevent_default();
            self.focus(window, cx);
        }
        true
    }

    /// Keyboard and screen reader presses, which carry no pointer
    /// (`clickButton`).
    pub(crate) fn click_button(
        &mut self,
        event: &InputEvent,
        direction: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pressless = matches!(
            event,
            InputEvent::Click(ClickEvent::Keyboard(_)) | InputEvent::Accessibility
        );
        if !pressless {
            return;
        }
        let reason = if direction > 0.0 {
            Reason::IncrementPress
        } else {
            Reason::DecrementPress
        };
        let size = if click_modifiers(event).shift {
            StepSize::Large
        } else {
            StepSize::Step
        };
        self.step_by(direction, size, reason, Some(event.clone()), window, cx);
        self.commit(Some(event.clone()), None, window, cx);
        self.changed(window, cx);
    }

    /// A press on a scrub area: a drag across it steps the value, and a
    /// click focuses the input (`scrub`).
    pub(crate) fn scrub(
        &mut self,
        event: &PointerEvent,
        props: NumberInputScrubAreaProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let drags = self.drags.clone();
        if !drags.accepts(self, event, window, cx) || self.is_locked() {
            return;
        }
        // No focus moves while dragging. Focus waits for a press that turns
        // out to be a click.
        window.prevent_default();
        let size = props.pixels_per_step.filter(|size| *size > 0.0).unwrap_or(2.0);
        let vertical = props.orientation == NumberInputScrubAreaOrientation::Vertical;
        let start = event.position;
        // Distance adds up across moves, so slow drags step as far as fast
        // ones.
        let distance = Rc::new(Cell::new(0.0_f64));
        let dragged = Rc::new(Cell::new(false));
        let cursor = Rc::new(Cell::new(start));
        self.scrubbing = true;
        cx.notify();
        let moved = (distance, dragged.clone(), cursor);
        drags.start(
            event,
            BindDragOptions::new()
                .lock(true)
                .on_move(move |input: &mut Self, sample, window, cx| {
                    let (distance, dragged, cursor) = &moved;
                    let travel = sample.position() - start;
                    dragged.set(dragged.get() || is_drag(travel.x, travel.y) || sample.locked);
                    if sample.locked {
                        // The real cursor is hidden and still, so draw one
                        // that wraps at the edges.
                        let view = window.viewport_size();
                        let at = cursor.get();
                        let next = Point::new(
                            wrap(at.x + sample.movement_x, view.width),
                            wrap(at.y + sample.movement_y, view.height),
                        );
                        cursor.set(next);
                        input.cursor = Some(next);
                        cx.notify();
                    } else {
                        cursor.set(sample.position());
                    }
                    let along = if vertical {
                        -f64::from(f32::from(sample.movement_y))
                    } else {
                        f64::from(f32::from(sample.movement_x))
                    };
                    distance.set(distance.get() + along);
                    // Every step this frame's distance covers, as one change.
                    let steps = (distance.get() / size).trunc();
                    if steps == 0.0 {
                        return;
                    }
                    distance.set(distance.get() - steps * size);
                    let event = Some(sample.event.event.clone());
                    input.step_by(
                        steps,
                        step_size(sample.event.modifiers),
                        Reason::Scrub,
                        event,
                        window,
                        cx,
                    );
                })
                .on_end(move |input: &mut Self, end, canceled, window, cx| {
                    input.scrubbing = false;
                    input.cursor = None;
                    input.commit(Some(end.event.clone()), None, window, cx);
                    // A click focuses the input with its text selected, ready
                    // to type.
                    if !dragged.get() && !canceled {
                        input.focus_and_select(window, cx);
                    }
                    input.changed(window, cx);
                }),
            window,
            cx,
        );
    }

    /// Focuses the input with its text selected (`focusAndSelect`).
    pub(crate) fn focus_and_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.field.update(cx, |field, cx| {
            field.focus(window, cx);
            field.select(window, cx);
        });
    }

    /// A form reset: back to `default_value`. A controlled input with no
    /// default keeps its value (`reset`).
    pub(crate) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if keeps_value_on_reset(
            self.value.is_controlled().then_some(&()),
            self.has_default.then_some(&()),
        ) {
            self.sync_form();
            return;
        }
        self.edited = false;
        self.pending = None;
        let initial = self.initial;
        self.value.change(
            &self.instance,
            initial,
            ChangeOptions::reason(Reason::Reset),
            window,
            cx,
        );
        self.show_value();
        self.changed(window, cx);
    }

    /// Focus left the control: what's typed commits (`focusout`).
    pub(crate) fn blurred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_text(Some(focus_event(window)), window, cx);
    }
}

// Wraps a position around a length, like a cursor past the screen's edge.
fn wrap(position: Pixels, size: Pixels) -> Pixels {
    let (position, size) = (f32::from(position), f32::from(size));
    if size <= 0.0 {
        return px(0.0);
    }
    px(((position % size) + size) % size)
}
