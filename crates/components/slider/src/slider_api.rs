//! Port of `components/slider/slider-api.ts`: a slider's state and what it
//! does: values, keys, drags and commits. The parts and their attributes are
//! in `slider.rs`, the pure decisions in `slider_values.rs`.

use std::rc::Rc;

use gpui::{
    App, Bounds, Context, FocusHandle, KeyDownEvent, Pixels, Point, SharedString, Subscription, Window,
};
use ui_sdk_drag_and_drop::{BindDragOptions, PointerEvent};
use ui_sdk_forms::{
    ElementInternals, FormEventType, FormValue, INPUT_AND_CHANGE, InternalsOptions, Node, bind_internals,
    dispatch_change, is_disabled, keeps_value_on_reset,
};
use ui_sdk_kernel::focus_events::on_blur;
use ui_sdk_kernel::{ChangeCallback, ChangeDetails, ChangeOptions, CommitCallback, InputEvent, Reason};
use ui_sdk_number::{NumberFormat, StepRange, display_options, format_number};
use ui_sdk_platform::is_rtl;
use ui_sdk_text::locale_of;

use crate::slider::{
    Slider, SliderOptions, SliderOrientation, SliderValueChange, SliderValueChangeDetails, SliderValueCommit,
};
use crate::slider_generic::SliderValue;
use crate::slider_values::{
    KeyTargetOptions, SliderRange, fraction_along, key_target, move_thumb, normalize_values, stack_at,
    thumb_bounds, thumb_for_direction, thumb_for_press, value_at,
};

/// What the options set, besides the value and the callbacks (`settings`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) step: f64,
    pub(crate) large_step: f64,
    pub(crate) min_steps_between_thumbs: f64,
    pub(crate) orientation: SliderOrientation,
    pub(crate) disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) name: Option<SharedString>,
    pub(crate) form: Option<SharedString>,
    pub(crate) format: Option<NumberFormat>,
    pub(crate) locale: Option<SharedString>,
}

impl Settings {
    /// Its bounds and step.
    pub(crate) fn step_range(&self) -> StepRange {
        StepRange::new(self.min, self.max, self.step)
    }

    /// Its bounds and steps (`rangeOf`).
    pub(crate) fn range(&self) -> SliderRange {
        SliderRange {
            min: self.min,
            max: self.max,
            step: self.step,
            large_step: self.large_step,
            min_steps_between_thumbs: self.min_steps_between_thumbs,
        }
    }
}

// The bounds, with a step that moves (`bounds`).
fn bounds<V>(options: &SliderOptions<V>) -> StepRange {
    let step = if options.step > 0.0 { options.step } else { 1.0 };
    StepRange::new(options.min, options.max, step)
}

pub(crate) fn settings<V>(options: &SliderOptions<V>) -> Settings {
    let range = bounds(options);
    Settings {
        min: range.min,
        max: range.max,
        step: range.step,
        large_step: options.large_step.unwrap_or(range.step * 10.0),
        min_steps_between_thumbs: options.min_steps_between_thumbs,
        orientation: options.orientation,
        disabled: options.disabled,
        read_only: options.read_only,
        required: options.required,
        name: options.name.clone(),
        form: options.form.clone(),
        format: options.format.clone(),
        locale: options.locale.clone(),
    }
}

/// Values from outside are clamped and sorted for drawing. A controlled
/// value isn't rewritten (`normalized`).
pub(crate) fn normalized<V: SliderValue>(value: &V, range: StepRange) -> V {
    V::shaped(normalize_values(&value.values(), range))
}

/// One form value for several numbers, `"start/end"` like an ISO interval
/// (`formatRange`).
pub fn format_range(values: &[f64]) -> String {
    values
        .iter()
        .map(|value| number_text(*value))
        .collect::<Vec<_>>()
        .join("/")
}

/// What the form submits: the number, or `"start/end"` for a range
/// (`submitted`).
pub(crate) fn submitted(values: &[f64]) -> String {
    format_range(values)
}

// A number as `String(value)` writes it.
fn number_text(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = value.abs();
    if (1e-6..1e21).contains(&magnitude) {
        return value.to_string();
    }
    let text = format!("{value:e}");
    match text.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
        _ => text,
    }
}

/// Intl options show at least as many decimals as the step, so a step of
/// 0.001 never reads as 0. A function formats the value as it likes
/// (`formatValue`).
pub(crate) fn format_value(value: f64, settings: &Settings, cx: &App) -> SharedString {
    let locale = locale_of(settings.locale.as_ref(), cx);
    let text = match &settings.format {
        Some(format @ NumberFormat::Function(_)) => format_number(Some(value), Some(&locale), Some(format)),
        other => {
            let options = display_options(other.as_ref().and_then(NumberFormat::options), settings.step);
            format_number(Some(value), Some(&locale), Some(&NumberFormat::Options(options)))
        }
    };
    text.into()
}

/// A press being dragged (`Gesture`).
pub(crate) struct Gesture<V> {
    // `None` while a press on stacked thumbs waits for the first move to
    // pick one.
    pub(crate) index: Option<usize>,
    pub(crate) stack: Vec<usize>,
    // How far from its thumb's value the press was, kept as it moves, so the
    // thumb stays under the pointer instead of jumping to it.
    pub(crate) offset: f64,
    pub(crate) start: V,
    pub(crate) reason: Reason,
    pub(crate) event: Option<InputEvent>,
}

/// Keys held down, which commit when they come up, so holding one commits
/// once.
pub(crate) struct Keys<V> {
    pub(crate) start: V,
    pub(crate) thumb: usize,
    pub(crate) event: Option<InputEvent>,
    // Focus leaving the thumb, which stands for the keyup it won't get.
    pub(crate) _blur: Option<Subscription>,
}

impl<V: SliderValue> Slider<V> {
    /// One number per thumb.
    pub(crate) fn values(&self) -> Vec<f64> {
        self.value.get().values()
    }

    pub(crate) fn range(&self) -> SliderRange {
        self.settings.range()
    }

    /// Whether it's disabled, by itself, its field or a fieldset
    /// (`isDisabled`).
    pub fn is_disabled(&self) -> bool {
        is_disabled(self.disabled_option(), self.internals.form_disabled())
    }

    // Its `disabled`, or its field's, as the original's adapters pass the
    // field's in. The field writes it on the control it finds: the root of
    // a range, or the thumb of a single value.
    fn disabled_option(&self) -> bool {
        self.settings.disabled
            || self.internals.control_attrs().disabled
            || self
                .thumb_internals
                .borrow()
                .values()
                .any(|internals| internals.control_attrs().disabled)
    }

    /// Whether drags, keys and screen reader adjustments leave it alone
    /// (`isLocked`).
    pub(crate) fn is_locked(&self) -> bool {
        self.is_disabled() || self.settings.read_only
    }

    // The node the form hears the slider's events from (`formElement`): the
    // root's part in forms.
    fn form_node(&self) -> Node {
        self.internals.node().clone()
    }

    /// Reports the value to its form, as the original's `bindInternals` does
    /// on every change: the number, or `"start/end"` for a range, under its
    /// name, and never a missing value, since a slider always has one. With
    /// one thumb, the thumb is the control a label or a field finds; when a
    /// field gives it a name and the slider has none of its own, it submits
    /// the value under it, as the root does for a range.
    pub(crate) fn sync_form(&self) {
        let values = self.values();
        let submitted = submitted(&values);
        let anchor = self
            .thumb_internals
            .borrow()
            .iter()
            .min_by_key(|(index, _)| **index)
            .map(|(_, internals)| internals.node().clone());
        bind_internals(
            &self.internals,
            InternalsOptions {
                value: FormValue::Text(submitted.clone().into()),
                name: self.settings.name.clone(),
                value_missing: false,
                message: SharedString::default(),
                anchor,
                disabled: self.disabled_option(),
                read_only: self.settings.read_only,
                required: self.settings.required,
                form: self.settings.form.clone(),
                ..Default::default()
            },
        );
        let single = values.len() <= 1;
        for internals in self.thumb_internals.borrow().values() {
            self.bind_thumb(internals, single, &submitted);
        }
    }

    // What a thumb reports: what a field's name submits when it's the
    // control, and whether it's disabled or read-only.
    pub(crate) fn bind_thumb(&self, internals: &ElementInternals, single: bool, submitted: &str) {
        let value = if single && self.settings.name.is_none() {
            FormValue::Text(submitted.to_string().into())
        } else {
            FormValue::None
        };
        bind_internals(
            internals,
            InternalsOptions {
                value,
                disabled: self.disabled_option(),
                read_only: self.settings.read_only,
                form: self.settings.form.clone(),
                ..Default::default()
            },
        );
    }

    // After the value changed: the form hears it, a label finds the thumb
    // while there's one and the group while there are more, and the parts
    // render it.
    pub(crate) fn after_change(&mut self, cx: &mut Context<Self>) {
        self.sync_form();
        self.internals.set_control(self.values().len() > 1, cx);
        cx.notify();
    }

    // Asks for a value, with the thumb that moved, through `on_value_change`.
    pub(crate) fn change_value(
        &mut self,
        value: V,
        options: ChangeOptions,
        thumb: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.value
            .set_on_change(change_callback(self.on_value_change.clone(), thumb));
        let done = self.value.change(&self.instance, value, options, window, cx);
        if done {
            self.after_change(cx);
        }
        done
    }

    // `notify` is off when what the slider stands in for, a thumb's native
    // range input, already sends `Input` and `Change` to the form, like after
    // a screen reader's adjustment (`setValues`).
    pub(crate) fn set_values(
        &mut self,
        next: Option<Vec<f64>>,
        reason: Reason,
        event: Option<InputEvent>,
        thumb: Option<usize>,
        notify: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // `move_thumb` gives nothing when no thumb moved.
        let Some(next) = next else {
            return false;
        };
        let options = ChangeOptions {
            reason: Some(reason),
            event,
            source: None,
        };
        let done = self.change_value(V::shaped(next), options, thumb, window, cx);
        if done {
            if thumb.is_some() {
                self.last = thumb;
            }
            // `Change` waits for the commit, like a native range's.
            if notify {
                dispatch_change(Some(&self.form_node()), &[FormEventType::Input], window, cx);
            }
        }
        done
    }

    pub(crate) fn commit(
        &mut self,
        start: V,
        reason: Reason,
        event: Option<InputEvent>,
        thumb: Option<usize>,
        notify: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if &start == self.value.get() {
            return;
        }
        self.value
            .set_on_commit(commit_callback(self.on_value_commit.clone(), thumb));
        self.value.commit(&self.instance, reason, event, window, cx);
        if notify {
            dispatch_change(Some(&self.form_node()), &[FormEventType::Change], window, cx);
        }
    }

    // Sorted first, then each thumb snapped and kept from its neighbours
    // (`setValue`).
    fn set_value_with(
        &mut self,
        value: V,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let range = self.range();
        let sorted = normalized(&value, self.settings.step_range()).values();
        let next = (0..sorted.len()).fold(sorted.clone(), |result, index| {
            move_thumb(&result, index, sorted[index], &range).unwrap_or(result)
        });
        if next == self.values() {
            return false;
        }
        self.change_value(V::shaped(next), options, None, window, cx)
    }

    /// The value: a number, or one number per thumb.
    pub fn get_value(&self) -> V {
        self.value.get().clone()
    }

    /// Sets the value, as a user would. It's clamped, snapped to the step and
    /// sorted. A disabled or read-only slider ignores it. Returns false when
    /// nothing changed or the change was canceled.
    pub fn set_value(
        &mut self,
        value: V,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_locked() {
            return false;
        }
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        self.set_value_with(value, options, window, cx)
    }

    /// Moves focus to a thumb, `0` for the first, once it's in the page. A
    /// disabled slider's thumbs take no focus.
    pub fn focus(&self, thumb: usize, window: &mut Window, cx: &mut App) {
        if let Some(handle) = self.thumb_focus_handle(thumb, cx) {
            window.focus(&handle, cx);
        }
    }

    // The focus handle of a thumb in the page that can take focus.
    pub(crate) fn thumb_focus_handle(&self, thumb: usize, cx: &App) -> Option<FocusHandle> {
        if self.is_disabled() || !self.thumbs.contains(&thumb) {
            return None;
        }
        Some(self.thumbs.focus_handle(&thumb, cx))
    }

    // The bounds of the part of `slot` as of its last layout.
    fn part_bounds(&self, slot: &str) -> Option<Bounds<Pixels>> {
        self.instance
            .bound_parts()
            .into_iter()
            .find(|part| part.slot == slot)
            .and_then(|part| part.bounds)
    }

    // How the original stacks thumbs: the dragged one above the rest, then
    // the one that moved last. gpui paints the thumbs in the order they
    // render, so this only decides which one a press over overlapping thumbs
    // lands on.
    fn z_index(&self, index: usize) -> u8 {
        if self.active == Some(index) {
            2
        } else if self.last == Some(index) {
            1
        } else {
            0
        }
    }

    // The thumb a press landed on: the one on top where thumbs overlap, and a
    // later one over an earlier one at the same height
    // (`closest('[data-slot="slider-thumb"]')`).
    fn pressed_thumb(&self, point: Point<Pixels>) -> Option<usize> {
        let count = self.values().len();
        self.instance
            .bound_parts()
            .into_iter()
            .filter(|part| part.slot == "thumb")
            .filter_map(|part| {
                let index = part.key.as_ref()?.parse::<usize>().ok()?;
                let bounds = part.bounds?;
                (index < count && bounds.contains(&point)).then_some(index)
            })
            .max_by_key(|index| (self.z_index(*index), *index))
    }

    // The value a point along the track stands for, from the track's box as
    // the press found it.
    fn value_along(&self, point: Point<Pixels>, track: Bounds<Pixels>, rtl: bool) -> f64 {
        let vertical = self.settings.orientation == SliderOrientation::Vertical;
        value_at(
            fraction_along(point, track, rtl, vertical),
            self.settings.step_range(),
        )
    }

    /// A press on the control. The track's box is read once, so moves never
    /// read layout (`internal.press`).
    pub(crate) fn press(&mut self, event: &PointerEvent, window: &mut Window, cx: &mut Context<Self>) {
        let drags = self.drags.clone();
        if !drags.accepts(self, event, window, cx) || self.is_locked() || self.gesture.is_some() {
            return;
        }
        let Some(track) = self.part_bounds("track").or_else(|| self.part_bounds("control")) else {
            return;
        };
        let rtl = is_rtl(cx);
        let thumb = self.pressed_thumb(event.position);
        // No native focus that would land elsewhere, like on a focusable
        // element around the slider.
        window.prevent_default();
        let pressed = self.value_along(event.position, track, rtl);
        let Some(focused) = self.begin(pressed, thumb, Some(event.event.clone()), window, cx) else {
            return;
        };
        self.focus(focused, window, cx);
        drags.start(
            event,
            BindDragOptions::new()
                .on_move(move |slider: &mut Self, sample, window, cx| {
                    let pressed = slider.value_along(sample.position(), track, rtl);
                    slider.move_to(pressed, Some(sample.event.event.clone()), window, cx);
                })
                .on_end(|slider: &mut Self, end, _canceled, window, cx| {
                    slider.end(Some(end.event.clone()), window, cx);
                }),
            window,
            cx,
        );
    }

    /// Starts a drag at a value: on a thumb, which keeps its offset from the
    /// press and doesn't move, or on the track, which moves the nearest
    /// thumb there. Returns the thumb that takes focus (`internal.begin`).
    pub(crate) fn begin(
        &mut self,
        pressed: f64,
        thumb: Option<usize>,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let start = self.get_value();
        let values = self.values();
        let (index, stack, offset) = match thumb {
            Some(thumb) => {
                let stack = stack_at(&values, thumb);
                let index = if stack.len() > 1 { None } else { Some(thumb) };
                let offset = pressed - values.get(thumb).copied().unwrap_or(f64::NAN);
                (index, stack, offset)
            }
            None => {
                let press = thumb_for_press(&values, pressed);
                if let Some(index) = press.index {
                    let range = self.range();
                    self.set_values(
                        move_thumb(&values, index, pressed, &range),
                        Reason::TrackPress,
                        event.clone(),
                        Some(index),
                        true,
                        window,
                        cx,
                    );
                }
                (press.index, press.stack, 0.0)
            }
        };
        let focused = index.or_else(|| stack.first().copied())?;
        self.gesture = Some(Gesture {
            index,
            stack,
            offset,
            start,
            reason: Reason::TrackPress,
            event,
        });
        self.active = Some(focused);
        cx.notify();
        Some(focused)
    }

    /// The pointer moved to a value (`internal.move`).
    pub(crate) fn move_to(
        &mut self,
        pressed: f64,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(gesture) = &self.gesture else {
            return;
        };
        let target = pressed - gesture.offset;
        let values = self.values();
        let index = match gesture.index {
            Some(index) => index,
            None => {
                // Stacked thumbs: the direction of the first move picks one.
                let first = gesture
                    .stack
                    .first()
                    .and_then(|first| values.get(*first))
                    .copied();
                let direction = js_sign(target - first.unwrap_or(f64::NAN));
                if direction == 0.0 {
                    return;
                }
                let Some(index) = thumb_for_direction(&gesture.stack, direction) else {
                    return;
                };
                if let Some(gesture) = &mut self.gesture {
                    gesture.index = Some(index);
                }
                self.focus(index, window, cx);
                self.active = Some(index);
                cx.notify();
                index
            }
        };
        let range = self.range();
        if self.set_values(
            move_thumb(&values, index, target, &range),
            Reason::Drag,
            event.clone(),
            Some(index),
            true,
            window,
            cx,
        ) && let Some(gesture) = &mut self.gesture
        {
            gesture.reason = Reason::Drag;
            gesture.event = event;
        }
    }

    /// The pointer lifted, or the drag ended some other way (`internal.end`).
    pub(crate) fn end(&mut self, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let ended = self.gesture.take();
        if self.active.take().is_some() {
            cx.notify();
        }
        if let Some(ended) = ended {
            self.commit(ended.start, ended.reason, event, ended.index, true, window, cx);
        }
    }

    /// A screen reader's adjustment, the original's `input` from the thumb's
    /// native range input, like a swipe on iOS. It commits at once, and the
    /// form hears `Input` and `Change` from the thumb, as it would from the
    /// native input (`internal.adjust`).
    pub(crate) fn adjust(&mut self, index: usize, target: f64, window: &mut Window, cx: &mut Context<Self>) {
        // A disabled native input takes no adjustment.
        if self.is_locked() {
            return;
        }
        let start = self.get_value();
        let range = self.range();
        let next = move_thumb(&self.values(), index, target, &range);
        let moved = next.is_some();
        let event = Some(InputEvent::Accessibility);
        self.set_values(
            next,
            Reason::AccessibilityAdjust,
            event.clone(),
            Some(index),
            false,
            window,
            cx,
        );
        self.commit(
            start,
            Reason::AccessibilityAdjust,
            event,
            Some(index),
            false,
            window,
            cx,
        );
        let thumb = self
            .thumb_internals
            .borrow()
            .get(&index)
            .map(|internals| internals.node().clone());
        if moved && let Some(thumb) = thumb {
            dispatch_change(Some(&thumb), INPUT_AND_CHANGE, window, cx);
        }
    }

    /// A screen reader's increment or decrement: a step up or down, as a
    /// native range input takes it.
    pub(crate) fn adjust_by(
        &mut self,
        index: usize,
        steps: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self.values().get(index).copied() else {
            return;
        };
        self.adjust(index, current + steps * self.settings.step, window, cx);
    }

    /// A key on a thumb (`internal.keydown`).
    pub(crate) fn keydown(
        &mut self,
        event: &KeyDownEvent,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_disabled() {
            return;
        }
        let values = self.values();
        let range = self.range();
        let current = values.get(index).copied().unwrap_or(self.settings.min);
        let options = KeyTargetOptions {
            rtl: is_rtl(cx),
            orientation: self.settings.orientation,
        };
        let bounds = thumb_bounds(&values, index, &range);
        let Some(target) = key_target(&event.keystroke, options, current, bounds, &range, window, cx) else {
            return;
        };
        window.prevent_default();
        if self.settings.read_only {
            return;
        }
        if self.keys.is_none() {
            // Focus leaving while the key is down sends its keyup elsewhere,
            // so the last keydown stands for it.
            let blur = self.thumbs.existing_focus_handle(&index).map(|handle| {
                on_blur(&handle, window, cx, |slider: &mut Self, window, cx| {
                    slider.commit_keys(None, window, cx)
                })
            });
            self.keys = Some(Keys {
                start: self.get_value(),
                thumb: index,
                event: None,
                _blur: blur,
            });
        }
        if let Some(keys) = &mut self.keys {
            keys.event = Some(InputEvent::from(event));
        }
        self.set_values(
            move_thumb(&values, index, target, &range),
            Reason::StepKey,
            Some(InputEvent::from(event)),
            Some(index),
            true,
            window,
            cx,
        );
    }

    /// The key came up, or focus left while it was down, so its keyup goes
    /// elsewhere and the last keydown stands for it (`internal.commitKeys`).
    pub(crate) fn commit_keys(
        &mut self,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(keys) = self.keys.take() else {
            return;
        };
        let Keys {
            start,
            thumb,
            event: last,
            ..
        } = keys;
        self.commit(
            start,
            Reason::StepKey,
            event.or(last),
            Some(thumb),
            true,
            window,
            cx,
        );
    }

    /// A form reset: back to `default_value`, or the first value. A
    /// controlled slider with no default keeps its value (`internal.reset`).
    pub(crate) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let controlled = self.value.is_controlled().then_some(());
        let default = self.has_default.then_some(());
        if !keeps_value_on_reset(controlled.as_ref(), default.as_ref()) {
            let initial = self.initial.clone();
            self.change_value(initial, ChangeOptions::reason(Reason::Reset), None, window, cx);
        }
        // Fields read validity right after a reset.
        self.sync_form();
    }

    /// A fieldset around it disabled it or enabled it again
    /// (`internal.setFormDisabled`).
    pub(crate) fn set_form_disabled(&mut self, _disabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.end_if_locked(window, cx);
        cx.notify();
    }

    /// Disabling it or making it read-only mid-drag ends the drag.
    pub(crate) fn end_if_locked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.gesture.is_none() || !self.is_locked() {
            return;
        }
        let drags = self.drags.clone();
        drags.cancel(self, window, cx);
        self.end(None, window, cx);
    }

    /// New bounds clamp an uncontrolled value, without a change event, like
    /// a native range (`deriveFromOptions`). Returns whether it moved.
    pub(crate) fn clamp_to_bounds(&mut self, controlled: bool) -> bool {
        let range = self.settings.step_range();
        if controlled || !(range.min <= range.max) {
            return false;
        }
        let value = normalized(self.value.get(), range);
        if &value == self.value.get() {
            return false;
        }
        self.value.set_held(value)
    }
}

// `Math.sign`: NaN stays NaN.
fn js_sign(value: f64) -> f64 {
    if value.is_nan() || value == 0.0 {
        value
    } else {
        value.signum()
    }
}

// `on_value_change` with the thumb that moved.
fn change_callback<V: SliderValue>(
    callback: Option<SliderValueChange<V>>,
    thumb: Option<usize>,
) -> Option<ChangeCallback<V>> {
    let callback = callback?;
    Some(Rc::new(
        move |value: &V, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
            callback(value, &SliderValueChangeDetails::new(details, thumb), window, cx)
        },
    ))
}

// `on_value_commit` with the thumb that moved.
fn commit_callback<V: SliderValue>(
    callback: Option<SliderValueCommit<V>>,
    thumb: Option<usize>,
) -> Option<CommitCallback<V>> {
    let callback = callback?;
    Some(Rc::new(
        move |value: &V, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
            callback(value, &SliderValueChangeDetails::new(details, thumb), window, cx)
        },
    ))
}

/// A thumb's own value, or `min` for a thumb past the values there are
/// (`thumbValue`).
pub(crate) fn thumb_value(values: &[f64], index: usize, settings: &Settings) -> f64 {
    values.get(index).copied().unwrap_or(settings.min)
}
