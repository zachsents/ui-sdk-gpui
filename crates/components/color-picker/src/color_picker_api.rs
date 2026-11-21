//! Port of `components/color-picker/color-picker-api.ts`: a color picker's
//! state and what it does. The value is a CSS color string; the parts edit a
//! color held in the picker's space, so a hue survives dragging to gray and
//! back. Parts and their attributes are in `color_picker_parts.rs`.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, AppContext, Context, FocusHandle, Focusable, KeyDownEvent, Pixels, SharedString, WeakEntity, Window,
};
use ui_sdk_color::{
    Color, ColorFormat, ColorFormatOptions, ColorGamut, color_distance, format_color, is_in_gamut, to_gamut,
};
use ui_sdk_direct_manipulation::{Area, AreaAxis, AreaOptions, AreaValue};
use ui_sdk_drag_and_drop::{BindDragOptions, PointerEvent, is_drag_press};
use ui_sdk_forms::{
    FormEventType, FormValue, InternalsOptions, bind_internals, dispatch_change, keeps_value_on_reset,
};
use ui_sdk_kernel::modality::window_source;
use ui_sdk_kernel::{
    ChangeCallback, ChangeDetails, ChangeOptions, CommitCallback, InputEvent, Key, Reason, with_source,
};
use ui_sdk_native::{TextField, TextFieldEvent, TextFieldKind, TextFieldOptions as FieldOptions};
use ui_sdk_number::{StepRange, round_to, snap_to_step};
use ui_sdk_platform::is_rtl;

use crate::color_picker::{
    ChannelField, ColorPicker, ColorPickerAreaProps, ColorPickerOptions, ColorPickerSliderOrientation,
    ColorPickerSliderProps, ColorPickerValueChange, ColorPickerValueChangeDetails, ColorPickerValueCommit,
};
use crate::color_picker_channels::{
    Channel, ChannelSpec, ChannelUnit, ColorChannel, ColorPickerSpace, channel_of, color_in, default_axes,
    number_text, read_channel, shown, step_decimals, with_channel,
};
use crate::color_picker_parts::area_key;

/// What the options set, besides the value and the callbacks (`settings`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) space: ColorPickerSpace,
    pub(crate) format: ColorFormat,
    pub(crate) gamut: Option<ColorGamut>,
    pub(crate) disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) name: Option<SharedString>,
    pub(crate) form: Option<SharedString>,
}

impl Settings {
    /// The value a color writes: fit into the gamut, in the format
    /// (`output`).
    pub(crate) fn output(&self, color: &Color) -> SharedString {
        format_color(
            &shown(color, self.gamut),
            Some(self.format),
            ColorFormatOptions::default(),
        )
        .into()
    }
}

// Hex for HSV, which CSS can't write, and the space otherwise (`formatFor`).
fn format_for(space: ColorPickerSpace, format: Option<ColorFormat>) -> ColorFormat {
    format.unwrap_or(match space {
        ColorPickerSpace::Hsv => ColorFormat::Hex,
        ColorPickerSpace::Hsl => ColorFormat::Hsl,
        ColorPickerSpace::Srgb => ColorFormat::Srgb,
        ColorPickerSpace::Oklch => ColorFormat::Oklch,
    })
}

pub(crate) fn settings(options: &ColorPickerOptions) -> Settings {
    Settings {
        space: options.space,
        format: format_for(options.space, options.format),
        gamut: options.gamut,
        disabled: options.disabled,
        read_only: options.read_only,
        required: options.required,
        name: options.name.clone(),
        form: options.form.clone(),
    }
}

/// Two colors are the same swatch when they look alike, whatever they're
/// written in (`isSameColor`).
pub(crate) fn is_same_color(first: &Color, second: &Color) -> bool {
    color_distance(first, second) < 0.002 && (first.alpha - second.alpha).abs() < 0.005
}

/// A channel's value as screen readers hear it, like "212 degrees" or
/// "40%" (`valueText`).
pub fn value_text(value: f64, spec: &ChannelSpec) -> String {
    let rounded = number_text(round_to(value, step_decimals(spec.step)));
    if spec.unit == ChannelUnit::Degrees {
        format!("{rounded} degrees")
    } else {
        format!("{rounded}{}", spec.unit.as_str())
    }
}

/// A number input's value on its step, which the input would otherwise call
/// invalid (`onStep`).
pub(crate) fn on_step(value: f64, step: f64) -> f64 {
    round_to(value, (-step.log10().floor()).max(0.0) as i32)
}

// `on_value_change` with the color the value is.
fn change_callback(
    callback: Option<ColorPickerValueChange>,
    color: Color,
) -> Option<ChangeCallback<SharedString>> {
    let callback = callback?;
    Some(Rc::new(
        move |value: &SharedString, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
            callback(
                value,
                &ColorPickerValueChangeDetails::new(details, color),
                window,
                cx,
            )
        },
    ))
}

// `on_value_commit` with the color the value is.
fn commit_callback(
    callback: Option<ColorPickerValueCommit>,
    color: Color,
) -> Option<CommitCallback<SharedString>> {
    let callback = callback?;
    Some(Rc::new(
        move |value: &SharedString, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
            callback(
                value,
                &ColorPickerValueChangeDetails::new(details, color),
                window,
                cx,
            )
        },
    ))
}

fn length(pixels: Pixels) -> f64 {
    f64::from(f32::from(pixels))
}

// A number field's text read as a number, `valueAsNumber`: a floating-point
// number as HTML writes them, as the native field reads its own.
fn value_as_number(text: &str) -> Option<f64> {
    let text = text.trim();
    let digits = text.strip_prefix('-').unwrap_or(text);
    if !digits.starts_with(|c: char| c.is_ascii_digit() || c == '.') || text.ends_with('.') {
        return None;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

impl ColorPicker {
    /// Whether it's disabled, by itself, by a fieldset or by its field
    /// (`isDisabled`).
    pub fn is_disabled(&self) -> bool {
        self.settings.disabled || self.internals.form_disabled() || self.internals.control_attrs().disabled
    }

    /// Whether drags, keys and typing leave the color alone (`isLocked`).
    pub(crate) fn is_locked(&self) -> bool {
        self.is_disabled() || self.settings.read_only
    }

    /// Notes the value an interaction starts from (`begin`).
    pub(crate) fn begin(&mut self) {
        self.start = Some(self.value.get().clone());
        self.changed = None;
    }

    /// A channel of the picker's space (`channel`).
    pub(crate) fn channel(&self, name: Option<ColorChannel>) -> Channel {
        channel_of(self.settings.space, name)
    }

    /// The color fit into the gamut, as callbacks get it (`publicColor`).
    pub(crate) fn public_color(&self, color: &Color) -> Color {
        shown(color, self.settings.gamut)
    }

    /// The color the parts show moves first, then the value, which a format
    /// like hex may round to the same string for a small move (`setColor`).
    /// `notify` sends `Input` to the form.
    pub(crate) fn set_color(
        &mut self,
        color: Color,
        options: ChangeOptions,
        notify: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let value = self.settings.output(&color);
        let before = self.color;
        self.color = color;
        if value == *self.value.get() {
            self.after_change(window, cx);
            return true;
        }
        let public = self.public_color(&color);
        self.value
            .set_on_change(change_callback(self.on_value_change.clone(), public));
        let done = self
            .value
            .change(&self.instance, value.clone(), options, window, cx);
        if !done {
            self.color = before;
        } else {
            self.changed = Some(value);
            if notify {
                dispatch_change(Some(self.internals.node()), &[FormEventType::Input], window, cx);
            }
        }
        self.after_change(window, cx);
        done
    }

    /// Once an interaction ends: a color outside the gamut fits it, and the
    /// value commits if it changed (`commit`).
    pub(crate) fn commit(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.start.take();
        if let Some(gamut) = self.settings.gamut
            && !is_in_gamut(&self.color, gamut)
        {
            self.color = to_gamut(&self.color, gamut);
        }
        let value = self.changed.take().unwrap_or_else(|| self.value.get().clone());
        if from.is_some_and(|from| from != value) {
            let public = self.public_color(&self.color);
            self.value
                .set_on_commit(commit_callback(self.on_value_commit.clone(), public));
            let reason = options.reason.unwrap_or(Reason::Imperative);
            self.value
                .commit_value(&self.instance, &value, reason, options.event, window, cx);
            dispatch_change(Some(self.internals.node()), &[FormEventType::Change], window, cx);
        }
        self.after_change(window, cx);
    }

    /// A change made in one go, like a swatch press, which commits at once
    /// (`choose`).
    pub(crate) fn choose_color(
        &mut self,
        color: Color,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_locked() {
            return false;
        }
        self.begin();
        let done = self.set_color(color, options.clone(), true, window, cx);
        self.commit(options, window, cx);
        done
    }

    /// The area's channels, across and up.
    pub(crate) fn axes(&self, props: ColorPickerAreaProps) -> (Channel, Channel) {
        let defaults = default_axes(self.settings.space);
        (
            self.channel(Some(props.x.unwrap_or(defaults.x))),
            self.channel(Some(props.y.unwrap_or(defaults.y))),
        )
    }

    /// The area for a pair of axes, made the first time a part asks for it
    /// (`area`).
    pub(crate) fn area_for(
        &self,
        props: ColorPickerAreaProps,
        entity: WeakEntity<Self>,
        cx: &App,
    ) -> Area<Self> {
        if let Some((_, area)) = self.areas.borrow().iter().find(|(key, _)| *key == props) {
            return area.clone();
        }
        let axis = |spec: &Channel| {
            AreaAxis::new()
                .min(0.0)
                .max(spec.max)
                .step(spec.step)
                .label(spec.label)
        };
        let options = AreaOptions::new(
            move |picker: &Self| {
                let (x, y) = picker.axes(props);
                AreaValue::new(read_channel(&picker.color, &x), read_channel(&picker.color, &y))
            },
            move |picker: &mut Self, value, options, window, cx| {
                let (x, y) = picker.axes(props);
                let color = with_channel(&with_channel(&picker.color, &x, value.x), &y, value.y);
                picker.set_color(color, options, true, window, cx)
            },
        )
        .x(move |picker| axis(&picker.axes(props).0))
        .y(move |picker| axis(&picker.axes(props).1))
        .value_text(move |picker, value| {
            let (x, y) = picker.axes(props);
            format!(
                "{} {}, {} {}",
                x.label,
                value_text(value.x, &x),
                y.label,
                value_text(value.y, &y)
            )
            .into()
        })
        .on_commit(|picker, _value, options, window, cx| picker.commit(options, window, cx))
        .disabled(|picker| picker.is_locked());
        let area = Area::new(entity, options, cx);
        self.areas.borrow_mut().push((props, area.clone()));
        area
    }

    /// Whether the picker's color looks like a swatch's value (`isSelected`).
    pub(crate) fn is_selected(&self, value: &str) -> bool {
        color_in(value, self.settings.space)
            .is_some_and(|color| is_same_color(&self.public_color(&color), &self.public_color(&self.color)))
    }

    /// A channel's value now (`sliderValue`).
    pub(crate) fn slider_value(&self, name: Option<ColorChannel>) -> f64 {
        read_channel(&self.color, &self.channel(name))
    }

    /// Sets a channel, snapped to its step (`setChannel`).
    pub(crate) fn set_channel(
        &mut self,
        name: Option<ColorChannel>,
        value: f64,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let spec = self.channel(name);
        let snapped = snap_to_step(value, StepRange::new(0.0, spec.max, spec.step));
        if snapped == self.slider_value(name) {
            return false;
        }
        let color = with_channel(&self.color, &spec, snapped);
        self.set_color(color, options, true, window, cx)
    }

    /// The value, as a CSS color.
    pub fn get_value(&self) -> &SharedString {
        self.value.get()
    }

    /// Sets the value from any CSS color, as a user would. Returns false for
    /// text that isn't one, or when the change was canceled.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(color) = color_in(&value.into(), self.settings.space) else {
            return false;
        };
        self.set_color(color, options, false, window, cx)
    }

    /// Moves focus to the area's thumb, or the first slider's, or the text
    /// input, whichever is in the page.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(target) = self.focus_target(window, cx) {
            window.focus(&target, cx);
        }
    }

    // What `focus` moves focus to: the first area thumb in the page, else
    // the first slider thumb, else the input (`element(slot)`).
    pub(crate) fn focus_target(&self, window: &Window, cx: &App) -> Option<FocusHandle> {
        let bound = self.instance.bound_parts();
        let keys = |slot: &'static str| {
            bound
                .iter()
                .filter(move |part| part.slot == slot)
                .filter_map(|part| part.key.clone())
        };
        let areas = self.areas.borrow();
        let area = keys("area-thumb").find_map(|key| {
            areas
                .iter()
                .find(|(props, _)| {
                    let (x, y) = self.axes(*props);
                    area_key(&x, &y) == key.as_ref()
                })
                .map(|(_, area)| area.focus_handle())
        });
        let slider = keys("slider-thumb").find_map(|key| {
            ColorChannel::from_name(&key).and_then(|channel| self.sliders.existing_focus_handle(&channel))
        });
        let input = self.input.read(cx).focus_handle(cx);
        area.into_iter()
            .chain(slider)
            .chain(std::iter::once(input))
            .find(|handle| handle.is_rendered(window))
    }

    /// A press on the area's surface (`areaPointerdown`).
    pub(crate) fn area_pointer_down(
        &mut self,
        area: &Area<Self>,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The thumb hears its press before the area around it does, as one
        // press: the drag it started follows it, so the area leaves it be.
        if is_drag_press(event) {
            return;
        }
        self.begin();
        area.pointer_down(self, event, window, cx);
    }

    /// A key on the area's thumb (`areaKeydown`).
    pub(crate) fn area_keydown(
        &mut self,
        area: &Area<Self>,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin();
        area.keydown(self, event, window, cx);
    }

    /// A press on a slider's track moves its thumb there and follows the
    /// pointer (`sliderPointerdown`).
    pub(crate) fn slider_pointer_down(
        &mut self,
        event: &PointerEvent,
        props: ColorPickerSliderProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let drags = self.drags.clone();
        if !drags.accepts(self, event, window, cx) || self.is_locked() {
            return;
        }
        window.prevent_default();
        let name = self.channel(props.channel).name;
        let Some(rect) = self.sliders.bounds(&name) else {
            return;
        };
        let vertical = props.orientation == ColorPickerSliderOrientation::Vertical;
        let rtl = is_rtl(cx);
        let at = move |picker: &Self, point: gpui::Point<Pixels>| {
            let length_of = if vertical {
                length(rect.size.height)
            } else {
                length(rect.size.width)
            };
            let mut fraction = if length_of != 0.0 {
                if vertical {
                    (length(rect.bottom()) - length(point.y)) / length_of
                } else {
                    (length(point.x) - length(rect.left())) / length_of
                }
            } else {
                0.0
            };
            if !vertical && rtl {
                fraction = 1.0 - fraction;
            }
            fraction.clamp(0.0, 1.0) * picker.channel(props.channel).max
        };
        self.begin();
        let reason = Rc::new(Cell::new(Reason::TrackPress));
        let value = at(self, event.position);
        let options = ChangeOptions::event_of(Reason::TrackPress, event.event.clone());
        self.set_channel(props.channel, value, options, window, cx);
        if let Some(thumb) = self.sliders.existing_focus_handle(&name)
            && thumb.is_rendered(window)
        {
            window.focus(&thumb, cx);
        }
        let moved = reason.clone();
        drags.start(
            event,
            BindDragOptions::new()
                .on_move(move |picker: &mut Self, sample, window, cx| {
                    let value = at(picker, sample.position());
                    let options = ChangeOptions::event_of(Reason::Drag, sample.event.event.clone());
                    if picker.set_channel(props.channel, value, options, window, cx) {
                        moved.set(Reason::Drag);
                    }
                })
                .on_end(move |picker: &mut Self, end, _canceled, window, cx| {
                    picker.commit(
                        ChangeOptions::event_of(reason.get(), end.event.clone()),
                        window,
                        cx,
                    );
                }),
            window,
            cx,
        );
    }

    /// Arrows step, Shift ten steps, Page Up and Page Down a tenth of the
    /// range, Home and End jump to the ends, as on a slider
    /// (`sliderKeydown`).
    pub(crate) fn slider_keydown(
        &mut self,
        event: &KeyDownEvent,
        name: Option<ColorChannel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.alt || modifiers.control || modifiers.platform || self.is_locked() {
            return;
        }
        let spec = self.channel(name);
        let current = self.slider_value(name);
        let rtl = is_rtl(cx);
        let steps = spec.step * if modifiers.shift { 10.0 } else { 1.0 };
        let target = match Key::of(&event.keystroke) {
            Key::ArrowUp => current + steps,
            Key::ArrowDown => current - steps,
            Key::ArrowRight => current + if rtl { -steps } else { steps },
            Key::ArrowLeft => current - if rtl { -steps } else { steps },
            Key::PageUp => current + spec.max / 10.0,
            Key::PageDown => current - spec.max / 10.0,
            Key::Home => 0.0,
            Key::End => spec.max,
            _ => return,
        };
        window.prevent_default();
        self.begin();
        self.set_channel(
            name,
            target,
            ChangeOptions::event(Reason::StepKey, event),
            window,
            cx,
        );
        self.commit(ChangeOptions::event(Reason::StepKey, event), window, cx);
    }

    /// A channel input's number, on Enter, blur or a step of its arrows
    /// (`setChannelValue`).
    pub(crate) fn set_channel_value(
        &mut self,
        name: Option<ColorChannel>,
        value: f64,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_locked() || !value.is_finite() {
            return;
        }
        self.begin();
        let options = ChangeOptions {
            reason: Some(Reason::InputCommit),
            event,
            source: None,
        };
        self.set_channel(name, value, options.clone(), window, cx);
        self.commit(options, window, cx);
    }

    /// What the text input holds while someone types (`setText`).
    pub(crate) fn set_text(&mut self, text: SharedString, cx: &mut Context<Self>) {
        self.text = Some(text);
        cx.notify();
    }

    /// Reads the text typed into the input. Text that isn't a color goes
    /// back to the value (`commitText`).
    pub(crate) fn commit_text(
        &mut self,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.text.clone() else {
            return;
        };
        if let Some(color) = color_in(&text, self.settings.space)
            && !self.is_locked()
        {
            let options = ChangeOptions {
                reason: Some(Reason::InputCommit),
                event,
                source: None,
            };
            self.choose_color(color, options, window, cx);
        }
        self.text = None;
        self.after_change(window, cx);
    }

    /// Escape puts the input's text back to the value (`revertText`).
    pub(crate) fn revert_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.text = None;
        self.after_change(window, cx);
    }

    /// A swatch's value, chosen at once (`choose`).
    pub(crate) fn choose(
        &mut self,
        value: &str,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match color_in(value, self.settings.space) {
            Some(color) => self.choose_color(color, options, window, cx),
            None => false,
        }
    }

    /// A form reset: back to `default_value`. A controlled picker with no
    /// default keeps its value (`reset`).
    pub(crate) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let controlled = self.value.is_controlled().then_some(&());
        if !keeps_value_on_reset(controlled, self.has_default.then_some(&()))
            && let Some(color) = color_in(&self.initial, self.settings.space)
        {
            self.set_color(color, ChangeOptions::reason(Reason::Reset), false, window, cx);
        }
        // Fields read validity right after a reset.
        self.sync_form();
    }

    /// A fieldset around it disabled it or enabled it again
    /// (`setFormDisabled`).
    pub(crate) fn form_disabled_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_field_options(cx);
        self.end_if_locked(window, cx);
        cx.notify();
    }

    /// Disabling it or making it read-only mid-drag ends the drag where it
    /// is, like a slider's.
    pub(crate) fn end_if_locked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_locked() || !self.drags.is_running() {
            return;
        }
        let drags = self.drags.clone();
        drags.cancel(self, window, cx);
    }

    /// Reports the value to its form: the value under its name, and never a
    /// missing one, since a picker always has a value (`bindInternals`).
    pub(crate) fn sync_form(&self) {
        bind_internals(
            &self.internals,
            InternalsOptions {
                value: FormValue::Text(self.value.get().clone()),
                name: self.settings.name.clone(),
                value_missing: false,
                message: SharedString::default(),
                disabled: self.settings.disabled,
                read_only: self.settings.read_only,
                required: self.settings.required,
                form: self.settings.form.clone(),
                ..Default::default()
            },
        );
    }

    /// After a change: the form hears it, the inputs show it and the parts
    /// render it.
    pub(crate) fn after_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_form();
        self.write_fields(window, cx);
        cx.notify();
    }

    /// The text is written here, so the field never fights the caret: the
    /// input shows what's typed, or the value, and a channel input that
    /// isn't focused shows its channel (`write`).
    pub(crate) fn write_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text.clone().unwrap_or_else(|| self.value.get().clone());
        self.input.update(cx, |field, cx| {
            if *field.value() != text {
                field.set_value(text, window, cx);
            }
        });
        let channels: Vec<ColorChannel> = self.channel_fields.keys().copied().collect();
        for channel in channels {
            self.write_channel(channel, false, window, cx);
        }
    }

    // A channel input shows its channel, on its step, unless it has focus.
    // `always` writes it all the same, as after its own change or blur.
    fn write_channel(
        &mut self,
        channel: ColorChannel,
        always: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self
            .channel_fields
            .get(&channel)
            .map(|channel| channel.field.clone())
        else {
            return;
        };
        let spec = self.channel(Some(channel));
        let text: SharedString = number_text(on_step(self.slider_value(Some(channel)), spec.step)).into();
        field.update(cx, |field, cx| {
            if (always || !field.is_focused(window)) && *field.value() != text {
                field.set_value(text, window, cx);
            }
        });
    }

    /// The text input's native field options.
    pub(crate) fn input_options(
        settings: &Settings,
        form_disabled: bool,
        value: &SharedString,
    ) -> FieldOptions {
        FieldOptions {
            default_value: value.clone(),
            kind: TextFieldKind::Text,
            disabled: settings.disabled || form_disabled,
            read_only: settings.read_only,
            ..FieldOptions::default()
        }
    }

    // A channel input's native field options: a number field over the
    // channel's range and step.
    fn channel_options(&self, channel: ColorChannel, value: SharedString) -> FieldOptions {
        let spec = self.channel(Some(channel));
        FieldOptions {
            default_value: value,
            kind: TextFieldKind::Number,
            min: Some(0.0),
            max: Some(spec.max),
            step: Some(spec.step),
            disabled: self.is_disabled(),
            read_only: self.settings.read_only,
            ..FieldOptions::default()
        }
    }

    /// Hands the native fields what disables them from outside, and their
    /// ranges.
    pub(crate) fn sync_field_options(&self, cx: &mut App) {
        let disabled = self.is_disabled();
        let read_only = self.settings.read_only;
        self.input.update(cx, |field, cx| {
            let options = FieldOptions {
                default_value: field.default_value().clone(),
                kind: TextFieldKind::Text,
                disabled,
                read_only,
                ..FieldOptions::default()
            };
            field.set_options(options, cx);
        });
        for (channel, entry) in &self.channel_fields {
            let mut options = self.channel_options(*channel, SharedString::default());
            entry.field.update(cx, |field, cx| {
                options.default_value = field.default_value().clone();
                field.set_options(options, cx);
            });
        }
    }

    /// Makes a native field for each channel of the space, a channel input
    /// each, and lets those of another space go.
    pub(crate) fn sync_channel_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let channels = self.settings.space.channels();
        self.channel_fields
            .retain(|channel, _| channels.contains(channel));
        for channel in channels {
            if self.channel_fields.contains_key(&channel) {
                continue;
            }
            let spec = self.channel(Some(channel));
            let text: SharedString = number_text(on_step(self.slider_value(Some(channel)), spec.step)).into();
            let options = self.channel_options(channel, text);
            let field = cx.new(|cx| TextField::new(options, window, cx));
            // The field's own events come in order: its `change` as focus
            // leaves, then the blur that shows the channel again.
            let subscriptions = vec![cx.subscribe_in(
                &field,
                window,
                move |picker: &mut Self, _field, event, window, cx| match event {
                    TextFieldEvent::Change(text) => {
                        let value = value_as_number(text).unwrap_or(f64::NAN);
                        // A typed number reports how the window was used,
                        // though no event explains it.
                        let source = window_source(window);
                        with_source(source, || {
                            picker.set_channel_value(Some(channel), value, None, window, cx)
                        });
                        picker.write_channel(channel, true, window, cx);
                    }
                    TextFieldEvent::Blur => picker.write_channel(channel, true, window, cx),
                    _ => {}
                },
            )];
            self.channel_fields.insert(
                channel,
                ChannelField {
                    field,
                    _subscriptions: subscriptions,
                },
            );
        }
        self.sync_field_options(cx);
    }
}
