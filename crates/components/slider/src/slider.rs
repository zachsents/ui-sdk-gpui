//! Port of `components/slider/slider.ts`: a number or a range picked along a
//! scale, with draggable thumbs and the arrow keys.
//!
//! The slider's state and what it does are `slider_api.rs`'s; the pure
//! decisions, like where a thumb may go, `slider_values.rs`'s. Here are the
//! options, the parts and what each part's state holds.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

use gpui::{
    AccessibleAction, App, Context, Entity, InteractiveElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, accesskit::ActionData, relative,
};
use ui_sdk_drag_and_drop::{DragSession, create_drag_session, on_pointer_down};
use ui_sdk_forms::{Association, ElementInternals, associate, focus_with};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, Aria, ChangeDetails, ChangeOptions, Controllable, InputEvent, Instance,
    Live, Part, Parts, Reason,
};
use ui_sdk_number::NumberFormat;
use ui_sdk_platform::is_rtl;

use crate::slider_api::{Gesture, Keys, Settings, format_value, normalized, settings, thumb_value};
use crate::slider_generic::SliderValue;
use crate::slider_values::{percent_of, round_percent, thumb_bounds};

/// The slider's axis. Vertical sliders grow upward.
pub type SliderOrientation = gpui::Orientation;

/// Why a slider's value wants to change: a press on the track moved the
/// nearest thumb there (`TrackPress`), a thumb was dragged (`Drag`), an
/// arrow, Page Up, Page Down, Home or End stepped it (`StepKey`), a screen
/// reader adjusted it (`AccessibilityAdjust`), its form reset (`Reset`), or
/// a call (`Imperative`).
pub type SliderValueChangeReason = Reason;

/// Options for every write method: the reason and event `on_value_change`
/// receives.
pub type SliderChangeOptions = ChangeOptions;

/// What `on_value_change` and `on_value_commit` receive after the value: the
/// reason, source and event, with `cancel()`, as every change's details
/// have, and the thumb that moved. A commit ignores `cancel()`.
pub struct SliderValueChangeDetails<'a> {
    details: &'a ChangeDetails,
    /// The thumb that moved, or `None` when the whole value changed, like on
    /// a form reset.
    pub thumb: Option<usize>,
}

impl<'a> SliderValueChangeDetails<'a> {
    pub(crate) fn new(details: &'a ChangeDetails, thumb: Option<usize>) -> Self {
        Self { details, thumb }
    }

    /// The change's own details, with its reason, source and event.
    pub fn details(&self) -> &ChangeDetails {
        self.details
    }
}

impl Deref for SliderValueChangeDetails<'_> {
    type Target = ChangeDetails;

    fn deref(&self) -> &ChangeDetails {
        self.details
    }
}

impl fmt::Debug for SliderValueChangeDetails<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SliderValueChangeDetails")
            .field("details", self.details)
            .field("thumb", &self.thumb)
            .finish()
    }
}

/// What `on_value_commit` receives after the value: the details of the
/// interaction that ended, and the thumb that moved.
pub type SliderValueCommitDetails<'a> = SliderValueChangeDetails<'a>;

/// `on_value_change`: the requested value and what explains it.
pub type SliderValueChange<V> = Rc<dyn Fn(&V, &SliderValueChangeDetails<'_>, &mut Window, &mut App)>;

/// `on_value_commit`: the committed value and what explains it.
pub type SliderValueCommit<V> = Rc<dyn Fn(&V, &SliderValueCommitDetails<'_>, &mut Window, &mut App)>;

/// The options `Slider` takes. `V` is the value: `f64` for one thumb, or
/// `Vec<f64>` with one number per thumb.
#[derive(Clone)]
pub struct SliderOptions<V> {
    /// The value: a number for one thumb, or one number per thumb, like
    /// `vec![20.0, 80.0]` for a range. Pass it to control the slider.
    pub value: Option<V>,
    /// The value at first, when uncontrolled. A form reset returns to it.
    /// Defaults to `min`.
    pub default_value: Option<V>,
    /// Runs while the value changes: on every step of a drag or a key.
    /// `details.thumb` says which thumb moved, `details.reason` why, and
    /// `details.cancel()` stops it. It runs inside the slider's update, so it
    /// may update any entity but the slider.
    pub on_value_change: Option<SliderValueChange<V>>,
    /// Runs once an interaction that changed the value ends: the pointer
    /// lifts, the key comes up, or a screen reader adjusts it. For saving.
    pub on_value_commit: Option<SliderValueCommit<V>>,
    /// The lowest value. Defaults to 0.
    pub min: f64,
    /// The highest value. When the steps from `min` don't land on it, the
    /// highest step below it is the end. Defaults to 100.
    pub max: f64,
    /// How far one step moves, from `min`. Defaults to 1.
    pub step: f64,
    /// How far Page Up, Page Down and Shift with an arrow key move. Defaults
    /// to 10 steps.
    pub large_step: Option<f64>,
    /// How many steps apart neighbouring thumbs stay. Thumbs never pass each
    /// other. Defaults to 0.
    pub min_steps_between_thumbs: f64,
    /// The slider's axis. Vertical sliders grow upward. Defaults to
    /// horizontal.
    pub orientation: SliderOrientation,
    /// Disables it like a native control: thumbs can't be focused or moved,
    /// and nothing is submitted.
    pub disabled: bool,
    /// Keeps it focusable and submitted, but stops drags, keys and screen
    /// reader adjustments from changing it.
    pub read_only: bool,
    /// Marks it required. A slider always has a value, so it never blocks a
    /// form.
    pub required: bool,
    /// The name it submits under: the number, or a range as `"start/end"`.
    /// Without one, it submits nothing.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
    /// How values read, for screen readers and the `value` part:
    /// `Intl.NumberFormat` options, like a currency, or a function of the
    /// value.
    pub format: Option<NumberFormat>,
    /// The locale values are formatted in. Defaults to the page's language.
    pub locale: Option<SharedString>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl<V> Default for SliderOptions<V> {
    fn default() -> Self {
        Self {
            value: None,
            default_value: None,
            on_value_change: None,
            on_value_commit: None,
            min: 0.0,
            max: 100.0,
            step: 1.0,
            large_step: None,
            min_steps_between_thumbs: 0.0,
            orientation: SliderOrientation::Horizontal,
            disabled: false,
            read_only: false,
            required: false,
            name: None,
            form: None,
            format: None,
            locale: None,
            debug: false,
        }
    }
}

impl<V> SliderOptions<V> {
    pub fn value(mut self, value: V) -> Self {
        self.value = Some(value);
        self
    }

    pub fn default_value(mut self, value: V) -> Self {
        self.default_value = Some(value);
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&V, &SliderValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_value_commit(
        mut self,
        callback: impl Fn(&V, &SliderValueCommitDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_commit = Some(Rc::new(callback));
        self
    }

    pub fn min(mut self, min: f64) -> Self {
        self.min = min;
        self
    }

    pub fn max(mut self, max: f64) -> Self {
        self.max = max;
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    pub fn large_step(mut self, large_step: f64) -> Self {
        self.large_step = Some(large_step);
        self
    }

    pub fn min_steps_between_thumbs(mut self, steps: f64) -> Self {
        self.min_steps_between_thumbs = steps;
        self
    }

    pub fn orientation(mut self, orientation: SliderOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn form(mut self, form: impl Into<SharedString>) -> Self {
        self.form = Some(form.into());
        self
    }

    pub fn format(mut self, format: impl Into<NumberFormat>) -> Self {
        self.format = Some(format.into());
        self
    }

    pub fn locale(mut self, locale: impl Into<SharedString>) -> Self {
        self.locale = Some(locale.into());
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root, control and track parts' `with` and the app read.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderState<V> {
    /// The value: a number, or one number per thumb.
    pub value: V,
    /// Whether it's disabled, by itself, its field or a fieldset.
    pub disabled: bool,
    /// Whether it's read-only: it stays focusable and submitted, but drags
    /// and keys don't change it.
    pub read_only: bool,
    /// Whether it's marked required.
    pub required: bool,
    /// Whether a thumb is being dragged.
    pub dragging: bool,
    /// The slider's axis.
    pub orientation: SliderOrientation,
}

/// What a thumb's `with` reads: the slider's state, with the thumb's own
/// value in place of the whole one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderThumbState {
    /// Which value the thumb shows.
    pub index: usize,
    /// The thumb's own value.
    pub thumb_value: f64,
    /// Where the thumb sits, from 0 to 100 (`--slider-thumb-position`).
    pub percent: f64,
    /// Whether it's disabled, by itself, its field or a fieldset.
    pub disabled: bool,
    /// Whether it's read-only.
    pub read_only: bool,
    /// Whether it's marked required.
    pub required: bool,
    /// Whether a thumb, this one or another, is being dragged.
    pub dragging: bool,
    /// Whether this thumb is the one being dragged (`data-dragging` on a
    /// thumb).
    pub dragged: bool,
    /// The slider's axis.
    pub orientation: SliderOrientation,
}

/// What the indicator's `with` reads: the slider's state, with the filled
/// part's place along the track.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderIndicatorState<V> {
    pub value: V,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub dragging: bool,
    pub orientation: SliderOrientation,
    /// Where the filled part starts, from 0 to 100
    /// (`--slider-indicator-start`).
    pub start: f64,
    /// How much of the track it fills, from 0 to 100
    /// (`--slider-indicator-size`).
    pub size: f64,
}

/// What the `value` part's `with` reads: the slider's state, with the
/// formatted value.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderValueState<V> {
    pub value: V,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub dragging: bool,
    pub orientation: SliderOrientation,
    /// The formatted value, a range joined with an en dash.
    pub text: SharedString,
}

/// The keys a thumb handles. Every arrow moves it whatever its orientation,
/// as a native range's do.
const THUMB_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "shift+ArrowUp",
    "shift+ArrowDown",
    "shift+ArrowLeft",
    "shift+ArrowRight",
    "PageUp",
    "PageDown",
    "Home",
    "End",
];

/// A slider (`slider`). Its entity is its controller. `V` is its value:
/// `f64` for one thumb, `Vec<f64>` for one number per thumb.
pub struct Slider<V: SliderValue = f64> {
    pub(crate) instance: Instance,
    pub(crate) value: Controllable<V>,
    pub(crate) on_value_change: Option<SliderValueChange<V>>,
    pub(crate) on_value_commit: Option<SliderValueCommit<V>>,
    // What a form reset returns to: `default_value`, or the first value.
    pub(crate) initial: V,
    // Whether the options pass a `default_value`, which a controlled slider
    // resets to.
    pub(crate) has_default: bool,
    pub(crate) settings: Settings,
    // The thumb being dragged.
    pub(crate) active: Option<usize>,
    // The thumb that moved last, drawn above the others in a range.
    pub(crate) last: Option<usize>,
    pub(crate) gesture: Option<Gesture<V>>,
    pub(crate) keys: Option<Keys<V>>,
    pub(crate) drags: DragSession<Self>,
    // The root's part in forms.
    pub(crate) internals: ElementInternals,
    // The thumbs in the page, in order, with their focus handles.
    pub(crate) thumbs: Parts<usize, ()>,
    // Each thumb's part in forms, made the first time it renders, which
    // stands for its native range input: what a label or a field finds as
    // the control of a slider with one thumb.
    pub(crate) thumb_internals: RefCell<HashMap<usize, ElementInternals>>,
    _subscriptions: Vec<Subscription>,
}

impl<V: SliderValue> Slider<V> {
    pub fn new(options: SliderOptions<V>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = settings(&options);
        let range = settings.step_range();
        let start = options
            .value
            .clone()
            .or_else(|| options.default_value.clone())
            .unwrap_or_else(|| V::shaped(vec![settings.min]));
        let first = normalized(&start, range);
        let initial = normalized(options.default_value.as_ref().unwrap_or(&first), range);
        let mut value = Controllable::new("value", first);
        value.set_controlled(options.value.as_ref().map(|value| normalized(value, range)));
        let mut instance = Instance::new("slider").with_agent(cx);
        instance.debug = options.debug;
        let internals = ElementInternals::new(cx);
        let thumbs = Parts::new();
        // A range is a group: a label naming it, or a submit, focuses its
        // first thumb.
        let first_thumb = {
            let entity = cx.weak_entity();
            focus_with(&internals, move |window, cx| {
                let handle = entity
                    .upgrade()
                    .and_then(|slider| slider.read(cx).thumb_focus_handle(0, cx));
                if let Some(handle) = handle {
                    window.focus(&handle, cx);
                }
            })
        };
        let association = associate(
            &internals,
            Association::new()
                .reset(cx, |slider: &mut Self, window, cx| slider.reset(window, cx))
                .disabled(cx, |slider: &mut Self, disabled, window, cx| {
                    slider.set_form_disabled(disabled, window, cx)
                }),
        );
        // A field disabling it mid-drag ends the drag too: the field's
        // write notifies it.
        let this = cx.entity();
        let field = cx.observe_in(&this, window, |slider: &mut Self, _, window, cx| {
            slider.end_if_locked(window, cx)
        });
        let slider = Self {
            instance,
            value,
            on_value_change: options.on_value_change,
            on_value_commit: options.on_value_commit,
            initial,
            has_default: options.default_value.is_some(),
            settings,
            active: None,
            last: None,
            gesture: None,
            keys: None,
            drags: create_drag_session(cx),
            internals,
            thumbs,
            thumb_internals: RefCell::new(HashMap::new()),
            _subscriptions: vec![first_thumb, association, field],
        };
        slider.sync_form();
        slider.internals.set_control(slider.values().len() > 1, cx);
        slider
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A new `default_value` changes what a form reset returns to,
    /// and new bounds clamp an uncontrolled value without a change.
    pub fn set_options(&mut self, options: SliderOptions<V>, window: &mut Window, cx: &mut Context<Self>) {
        let settings = settings(&options);
        let range = settings.step_range();
        let mut changed = self.settings != settings;
        // Like `defaultChecked`, changing it changes what a reset returns to.
        if let Some(default) = &options.default_value {
            self.initial = normalized(default, range);
        }
        self.has_default = options.default_value.is_some();
        self.settings = settings;
        changed |= self
            .value
            .set_controlled(options.value.as_ref().map(|value| normalized(value, range)));
        changed |= self.clamp_to_bounds(options.value.is_some());
        self.on_value_change = options.on_value_change;
        self.on_value_commit = options.on_value_commit;
        self.instance.debug = options.debug;
        self.end_if_locked(window, cx);
        self.sync_form();
        self.internals.set_control(self.values().len() > 1, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the slider keep its
    /// own. It's clamped and sorted for drawing.
    pub fn set_controlled_value(&mut self, value: Option<V>, cx: &mut Context<Self>) {
        let range = self.settings.step_range();
        if self
            .value
            .set_controlled(value.as_ref().map(|value| normalized(value, range)))
        {
            self.after_change(cx);
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// What the root, control and track parts' state holds.
    pub fn state(&self) -> SliderState<V> {
        SliderState {
            value: self.get_value(),
            disabled: self.is_disabled(),
            read_only: self.settings.read_only,
            required: self.settings.required,
            dragging: self.active.is_some(),
            orientation: self.settings.orientation,
        }
    }

    /// The state the thumb `index` renders with.
    pub fn thumb_state(&self, index: usize) -> SliderThumbState {
        let value = thumb_value(&self.values(), index, &self.settings);
        SliderThumbState {
            index,
            thumb_value: value,
            percent: percent_of(value, self.settings.step_range()),
            disabled: self.is_disabled(),
            read_only: self.settings.read_only,
            required: self.settings.required,
            dragging: self.active.is_some(),
            dragged: self.active == Some(index),
            orientation: self.settings.orientation,
        }
    }

    /// A value as screen readers and the `value` part read it, in the
    /// slider's format and locale.
    pub fn format_value(&self, value: f64, cx: &App) -> SharedString {
        format_value(value, &self.settings, cx)
    }

    // The filled part: from the start to a single thumb, or between the first
    // and last.
    fn indicator_place(&self) -> (f64, f64) {
        let range = self.settings.step_range();
        let all = self.values();
        let start = if all.len() > 1 {
            percent_of(all[0], range)
        } else {
            0.0
        };
        let end = percent_of(all.last().copied().unwrap_or(f64::NAN), range);
        (start, round_percent(end - start))
    }

    // The thumb's part in forms, made the first time it renders: a control a
    // label or a field finds when the slider has one thumb, which takes
    // focus as the thumb.
    fn thumb_internals_for(this: &Entity<Self>, slider: &Self, index: usize, cx: &App) -> ElementInternals {
        let handle = slider.thumbs.focus_handle(&index, cx);
        let internals = slider
            .thumb_internals
            .borrow_mut()
            .entry(index)
            .or_insert_with(|| {
                let internals = ElementInternals::detached();
                internals.node().set_observer(Some(this.entity_id()));
                internals
            })
            .clone();
        internals.set_focus_handle((!slider.is_disabled()).then_some(handle));
        let values = slider.values();
        let submitted = crate::slider_api::format_range(&values);
        slider.bind_thumb(&internals, values.len() <= 1, &submitted);
        internals
    }

    /// The `root` part, around everything: a group, which a label or a field
    /// names while it has several thumbs, with the slider's form side. Build
    /// it before the thumbs: it starts their render, so the order they render
    /// in is page order. It follows a drag's pointer, so render it on every
    /// frame. Name a range with `aria_label` or a field; a single thumb is
    /// the one a label names.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<SliderState<V>> {
        let slider = this.read(cx);
        slider.sync_form();
        slider.thumbs.begin();
        // A group takes the labels and the field's descriptions; whether
        // it's disabled or read-only is each thumb's to say.
        let mut aria = slider.internals.aria();
        aria.disabled = false;
        aria.read_only = false;
        aria.required = false;
        Part::new(&slider.instance, "root", None, slider.state())
            .map_el(|el| slider.internals.track(el))
            .aria_role(Role::Group)
            .aria(aria)
            .child(slider.drags.listener())
    }

    /// The `control` part: where presses land. A press on the track moves
    /// the nearest thumb there and starts a drag; a press on a thumb drags
    /// it from where it is. Give it some height around a thin track.
    pub fn control(this: &Entity<Self>, cx: &App) -> Part<SliderState<V>> {
        let slider = this.read(cx);
        let entity = this.downgrade();
        let part = Part::new(&slider.instance, "control", None, slider.state());
        on_pointer_down(part, move |event, window, cx| {
            entity
                .update(cx, |slider, cx| slider.press(event, window, cx))
                .ok();
        })
    }

    /// The `track` part: the rail the thumbs move along. Thumbs and the
    /// indicator are placed in fractions of it, so give it a size. It lays
    /// out along the slider's axis, which centers the thumbs across it.
    pub fn track(this: &Entity<Self>, cx: &App) -> Part<SliderState<V>> {
        let slider = this.read(cx);
        let state = slider.state();
        let vertical = state.orientation == SliderOrientation::Vertical;
        let part = Part::new(&slider.instance, "track", None, state)
            .relative()
            .flex();
        if vertical {
            part.flex_col()
        } else {
            part.flex_row()
        }
    }

    /// The `indicator` part, inside the track: it fills the track up to a
    /// single thumb, or between the first and last thumb of a range.
    pub fn indicator(this: &Entity<Self>, cx: &App) -> Part<SliderIndicatorState<V>> {
        let slider = this.read(cx);
        let state = slider.state();
        let (start, size) = slider.indicator_place();
        let vertical = state.orientation == SliderOrientation::Vertical;
        let rtl = is_rtl(cx);
        let indicator = SliderIndicatorState {
            value: state.value,
            disabled: state.disabled,
            read_only: state.read_only,
            required: state.required,
            dragging: state.dragging,
            orientation: state.orientation,
            start,
            size,
        };
        let part = Part::new(&slider.instance, "indicator", None, indicator).absolute();
        // A value that isn't a number fills nothing.
        let (start, size) = (finite_or_zero(start) / 100.0, finite_or_zero(size) / 100.0);
        if vertical {
            part.bottom(relative(start as f32))
                .h(relative(size as f32))
                .left_0()
                .right_0()
        } else {
            let part = if rtl {
                part.right(relative(start as f32))
            } else {
                part.left(relative(start as f32))
            };
            part.w(relative(size as f32)).top_0().bottom_0()
        }
    }

    /// The `thumb` part for the value at `index`, from 0, inside the track:
    /// what's dragged, a slider that takes focus, keys and a screen reader's
    /// adjustments. Give each thumb of a range its own index and its own
    /// `aria_label`, like "Minimum price"; a single thumb takes its name from
    /// a label or a field. Give it a size along the track: it's centered on
    /// its value. Style its focus with `focus_visible`.
    pub fn thumb(this: &Entity<Self>, index: usize, cx: &App) -> Part<SliderThumbState> {
        let slider = this.read(cx);
        slider.thumbs.register(index, ());
        let handle = slider.thumbs.focus_handle(&index, cx);
        let internals = Self::thumb_internals_for(this, slider, index, cx);
        let state = slider.thumb_state(index);
        let range = slider.settings.range();
        // Each thumb's range ends at its neighbours, as the APG's multi-thumb
        // slider says.
        let bounds = thumb_bounds(&slider.values(), index, &range);
        let text = slider.format_value(state.thumb_value, cx);
        let disabled = state.disabled;
        let part = Part::new(&slider.instance, "thumb", Some(&index.to_string()), state)
            .map_el(|el| internals.track(el))
            .aria_role(Role::Slider)
            .focus_mode(FocusMode::Native, (!disabled).then_some(&handle), disabled)
            .aria_numeric_value(state.thumb_value)
            .aria_min_numeric_value(bounds.lower)
            .aria_max_numeric_value(bounds.upper)
            .aria_numeric_value_step(range.step)
            .aria_orientation(state.orientation)
            .aria(internals.aria())
            .aria(
                Aria::new()
                    .value_text(text)
                    .disabled(disabled)
                    .read_only(state.read_only),
            );
        let part = place_thumb(part, state.orientation, state.percent, is_rtl(cx));
        let keys = this.downgrade();
        let ups = this.downgrade();
        let increments = this.downgrade();
        let decrements = this.downgrade();
        let sets = this.downgrade();
        part.on_keys(THUMB_KEYS, move |event, window, cx| {
            keys.update(cx, |slider, cx| slider.keydown(event, index, window, cx))
                .ok();
        })
        .on_key_up(move |event, window, cx| {
            ups.update(cx, |slider, cx| {
                slider.commit_keys(Some(InputEvent::from(event)), window, cx)
            })
            .ok();
        })
        .on_a11y_action(AccessibleAction::Increment, move |_, window, cx| {
            increments
                .update(cx, |slider, cx| slider.adjust_by(index, 1.0, window, cx))
                .ok();
        })
        .on_a11y_action(AccessibleAction::Decrement, move |_, window, cx| {
            decrements
                .update(cx, |slider, cx| slider.adjust_by(index, -1.0, window, cx))
                .ok();
        })
        .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
            let Some(ActionData::NumericValue(target)) = data else {
                return;
            };
            let target = *target;
            sets.update(cx, |slider, cx| slider.adjust(index, target, window, cx))
                .ok();
        })
    }

    /// The `value` part: the formatted value, a range joined with an en
    /// dash. It's a status that's never announced on its own, since each
    /// thumb already says its value; put its text in gpui's `text!` so a
    /// screen reader reads it where it is.
    pub fn value(this: &Entity<Self>, cx: &App) -> Part<SliderValueState<V>> {
        let slider = this.read(cx);
        let state = slider.state();
        let text: Vec<String> = slider
            .values()
            .into_iter()
            .map(|value| slider.format_value(value, cx).to_string())
            .collect();
        let value = SliderValueState {
            value: state.value,
            disabled: state.disabled,
            read_only: state.read_only,
            required: state.required,
            dragging: state.dragging,
            orientation: state.orientation,
            text: text.join(" – ").into(),
        };
        Part::new(&slider.instance, "value", None, value)
            .aria_role(Role::Status)
            .aria(Aria::new().live(Live::Off))
    }
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

// Centered on its value in either direction, with no need to know which:
// along the track, the insets make a box as long as the track, centered on
// the value, and the thumb centers in it with the auto margins, which a
// thumb with a size takes. Across it, the track lays out along its axis, so
// `align-self` centers a thumb of any size (`thumbStyle`).
fn place_thumb(
    part: Part<SliderThumbState>,
    orientation: SliderOrientation,
    percent: f64,
    rtl: bool,
) -> Part<SliderThumbState> {
    // A value that isn't a number leaves the thumb at the start.
    let at = (finite_or_zero(percent) / 100.0) as f32;
    let part = part.absolute().self_center();
    if orientation == SliderOrientation::Vertical {
        return part.top(relative(0.5 - at)).bottom(relative(at - 0.5)).my_auto();
    }
    let (start, end) = (relative(at - 0.5), relative(0.5 - at));
    let part = if rtl {
        part.right(start).left(end)
    } else {
        part.left(start).right(end)
    };
    part.mx_auto()
}

/// `agent: { actions: ["setValue"] }` and `commits: ["value"]`.
impl<V: SliderValue> Agent for Slider<V> {
    const ACTIONS: &'static [AgentAction<Self>] =
        &[AgentAction::new("setValue", |slider, args, window, cx| {
            let value = V::from_agent(args, 0)?;
            Ok(slider
                .set_value(value, ChangeOptions::default(), window, cx)
                .into())
        })];

    const COMMITS: &'static [&'static str] = &["value"];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", self.value.get().to_agent())
            .with("disabled", self.is_disabled())
            .with("readOnly", self.settings.read_only)
            .with("required", self.settings.required)
            .with("min", self.settings.min)
            .with("max", self.settings.max)
            .with("step", self.settings.step)
    }
}

impl<V: SliderValue> Slider<V> {
    /// The thumb that's being dragged, if one is.
    pub fn dragging_thumb(&self) -> Option<usize> {
        self.active
    }

    /// The thumbs in the page, in order, as of their last render.
    pub fn thumbs(&self) -> Vec<usize> {
        self.thumbs.keys()
    }
}
