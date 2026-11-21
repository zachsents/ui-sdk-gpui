//! Port of `components/date-input/date-input-parts.ts`: Date Input's and
//! Time Input's parts: the group and a segment, and the validity they report
//! to a form.

use gpui::{
    App, ClickEvent, Entity, Focusable, InteractiveElement, MouseButton, MouseDownEvent, SharedString,
    StatefulInteractiveElement,
};
use ui_sdk_forms::{
    FormValue, InternalsOptions, Problem, ValidityFlag, ValueMissingKind, bind_internals,
    value_missing_message,
};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, Part};
use ui_sdk_native::TextField;

use crate::date_input_api::{DateInputCore, DateInputHost, ValueProblem};
use crate::date_input_model::{DateInputSegmentData, DateInputSegmentType, Kind};

/// What the root part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateInputState {
    /// The value, or `None` while empty or partly filled.
    pub value: Option<SharedString>,
    /// The segments in the order the locale shows them, with the text
    /// between them. Render one part for each.
    pub segments: Vec<DateInputSegmentData>,
    /// Whether it can't be used, by itself, its field or a fieldset.
    pub disabled: bool,
    /// Whether the value can be seen but not changed.
    pub read_only: bool,
    /// Whether the value is before `min`, after `max` or an unavailable date.
    pub invalid: bool,
    /// Whether a value must be entered (`data-required`).
    pub required: bool,
    /// Whether every segment is empty (`data-empty`).
    pub empty: bool,
}

/// What a segment's `with` reads: the segment, and the input's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateInputSegmentState {
    /// Tells segments apart: the type, or `"literal-<n>"`.
    pub key: SharedString,
    /// What it holds (`data-type`).
    pub r#type: DateInputSegmentType,
    /// What it shows. Empty while it holds nothing.
    pub text: SharedString,
    /// The number it holds, or `None` while empty. AM is 0 and PM is 1.
    pub value: Option<i32>,
    pub min: i32,
    pub max: i32,
    /// Whether it holds nothing yet (`data-empty`); it shows its placeholder.
    pub empty: bool,
    /// Whether it's a segment you type in, rather than the text between.
    pub editable: bool,
    /// Its name in the locale, like "month".
    pub label: SharedString,
    /// Whether it can't be used.
    pub disabled: bool,
    /// Whether its value can't be changed.
    pub read_only: bool,
    /// Whether the value is before `min`, after `max` or an unavailable date.
    pub invalid: bool,
    /// Whether a value must be entered (`data-required`).
    pub required: bool,
}

impl DateInputSegmentState {
    fn of(segment: &DateInputSegmentData, core: &DateInputCore) -> Self {
        Self {
            key: segment.key.clone(),
            r#type: segment.r#type,
            text: segment.text.clone(),
            value: segment.value,
            min: segment.min,
            max: segment.max,
            empty: segment.empty,
            editable: segment.editable,
            label: segment.label.clone(),
            disabled: core.is_disabled(),
            read_only: core.read_only,
            invalid: core.problem_of().is_some(),
            required: core.required,
        }
    }
}

// English, like Date Picker's, for what the platform has no message of its own.
fn bad_input_message(kind: Kind) -> &'static str {
    match kind {
        Kind::Time => "Enter a valid time.",
        Kind::Date => "Enter a valid date.",
    }
}

const UNAVAILABLE_MESSAGE: &str = "This date isn't available.";

// The message for a value before `min` or after `max`: the original's
// English fallback for the browser's own, which no native input here can
// give.
fn range_message(core: &DateInputCore, problem: ValueProblem) -> String {
    let underflow = problem == ValueProblem::RangeUnderflow;
    let bound = if underflow { core.min.as_ref() } else { core.max.as_ref() };
    let bound = bound.map_or("", |bound| bound.as_ref());
    if underflow {
        format!("Value must be {bound} or later.")
    } else {
        format!("Value must be {bound} or earlier.")
    }
}

// The validity flag for the value's problem. A read-only control is barred
// from validation, like a native one.
fn problem_flag(core: &DateInputCore, cx: &App) -> Option<ValidityFlag> {
    if core.read_only {
        return None;
    }
    if core.is_partial(cx) {
        return Some(ValidityFlag::BadInput);
    }
    core.problem_of().map(|problem| match problem {
        ValueProblem::RangeUnderflow => ValidityFlag::RangeUnderflow,
        ValueProblem::RangeOverflow => ValidityFlag::RangeOverflow,
        ValueProblem::Unavailable => ValidityFlag::CustomError,
    })
}

fn problem_message(core: &DateInputCore, cx: &App) -> String {
    if core.is_partial(cx) {
        return bad_input_message(core.kind).to_string();
    }
    match core.problem_of() {
        Some(ValueProblem::Unavailable) => UNAVAILABLE_MESSAGE.to_string(),
        Some(problem) => range_message(core, problem),
        None => String::new(),
    }
}

impl DateInputCore {
    /// Reports the value and its validity to the form, the first segment
    /// carrying the message (`bindInternals`). Runs on every render and
    /// after every change.
    pub(crate) fn report(&self, cx: &App) {
        let value_missing = self.required && !self.read_only && self.value.get().is_none() && !self.is_partial(cx);
        let anchor = self
            .rendered_segments(cx)
            .first()
            .and_then(|first| self.node_of(*first))
            .cloned();
        let invalid = problem_flag(self, cx).map(|flag| {
            let problem = Problem::new(flag, problem_message(self, cx));
            match anchor.clone() {
                Some(anchor) => problem.anchor(anchor),
                None => problem,
            }
        });
        bind_internals(
            &self.internals,
            InternalsOptions {
                value: FormValue::Text(self.value.get().clone().unwrap_or_default()),
                name: self.name.clone(),
                value_missing,
                message: value_missing_message(ValueMissingKind::Text),
                invalid,
                anchor,
                disabled: self.disabled,
                read_only: self.read_only,
                required: self.required,
                barred: false,
                form: self.form.clone(),
            },
        );
    }

    /// What the root part's state holds.
    pub fn state(&self, cx: &App) -> DateInputState {
        DateInputState {
            value: self.value.get().clone(),
            segments: self.segments_for(cx),
            disabled: self.is_disabled(),
            read_only: self.read_only,
            invalid: self.problem_of().is_some(),
            required: self.required,
            empty: self.value.get().is_none() && !self.is_partial(cx),
        }
    }
}

/// The `root` part: a group around the segments, named by its field's label
/// or an `aria_label`. It takes part in forms: it submits the value under
/// its name, an empty string while empty or partly typed. A press on it
/// outside the segments, or on the text between them, focuses the nearest
/// segment.
pub fn root<H: DateInputHost>(this: &Entity<H>, cx: &App) -> Part<DateInputState> {
    let host = this.read(cx);
    let core = host.date_input();
    // Reported from the segments the last frame rendered, the first of which
    // carries the message, before this frame's render again.
    core.report(cx);
    core.parts.begin();
    let state = core.state(cx);
    let internals = core.internals.clone();
    let mut aria = internals.aria();
    // A group takes no disabled, required or read-only state of its own;
    // its segments carry them.
    aria.disabled = false;
    aria.required = false;
    aria.read_only = false;
    let pressed = this.downgrade();
    let clicked = this.downgrade();
    let rendered = this.downgrade();
    Part::new(&core.instance, "root", None, state)
        .aria_role(Role::Group)
        .aria(aria)
        .map_el(|el| internals.track(el))
        // A field or fieldset that disables it shows in its segments' fields.
        .on_props(move |_window, cx| {
            rendered
                .update(cx, |host, cx| host.date_input_mut().sync_field_options(cx))
                .ok();
        })
        // Keeps focus in the field until the click below picks a segment.
        .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, window, cx| {
            let nearest = pressed
                .upgrade()
                .and_then(|host| host.read(cx).date_input().nearest_segment(event.position, cx));
            if nearest.is_some() {
                window.prevent_default();
            }
        })
        .on_click(move |event: &ClickEvent, window, cx| {
            clicked
                .update(cx, |host, cx| host.date_input_mut().press_root(event, window, cx))
                .ok();
        })
}

/// The keys a segment handles.
pub const SEGMENT_KEYS: &[&str] = &[
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "ArrowDown",
    "PageUp",
    "PageDown",
    "Home",
    "End",
];

/// A `segment` part, for one of the root's `segments`. A segment you type in
/// is a text field, a spinbutton named by its unit, like "month", with its
/// value, range and text; an empty one reads as "Empty". The text between
/// segments is hidden from assistive technology, which hears each segment's
/// name instead.
pub fn segment<H: DateInputHost>(
    this: &Entity<H>,
    segment: &DateInputSegmentData,
    cx: &App,
) -> Part<DateInputSegmentState> {
    let host = this.read(cx);
    let core = host.date_input();
    // A segment as it stands now, so a part rendered from an older list stays current.
    let current = core.segment_for(&segment.key, cx).unwrap_or_else(|| segment.clone());
    let state = DateInputSegmentState::of(&current, core);
    core.parts.register(current.key.clone(), ());
    let field = current.r#type.editable().and_then(|kind| Some((kind, core.field_of(kind)?.clone())));
    let Some((kind, field)) = field else {
        return Part::new(&core.instance, "segment", Some(&current.key), state).aria(Aria::new().hidden(true));
    };
    let handle = field.read(cx).focus_handle(cx);
    let (el, _) = TextField::root(&field, cx).into_element_parts();
    let el = match core.node_of(kind) {
        Some(node) => node.track(el),
        None => el,
    };
    let el = core.parts.measure(current.key.clone(), el);
    let aria = Aria::new()
        .value_text(if current.empty {
            SharedString::new_static("Empty")
        } else {
            current.text.clone()
        })
        .invalid(state.invalid)
        .disabled(state.disabled)
        .read_only(state.read_only)
        .required(state.required);
    let disabled = state.disabled;
    let keyed = this.downgrade();
    let part = Part::new(&core.instance, "segment", Some(&current.key), state)
        .map_el(move |_| el)
        .aria_role(Role::SpinButton)
        .focus_mode(FocusMode::Native, Some(&handle), disabled)
        .aria_label(current.label.clone())
        .aria_min_numeric_value(f64::from(current.min))
        .aria_max_numeric_value(f64::from(current.max))
        .aria(aria)
        .on_keys(SEGMENT_KEYS, move |event, window, cx| {
            keyed
                .update(cx, |host, cx| host.date_input_mut().keydown(kind, event, window, cx))
                .ok();
        });
    match current.value {
        Some(value) => part.aria_numeric_value(f64::from(value)),
        None => part,
    }
}
