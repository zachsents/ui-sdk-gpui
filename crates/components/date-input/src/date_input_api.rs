//! Port of `components/date-input/date-input-api.ts`: Date Input's and Time
//! Input's state and api: the fields the segments hold, typing, stepping
//! and clearing them, and what the form asks of the input.
//!
//! The original's `State` and `dateInputApi` are [`DateInputCore`], which
//! both components keep and hand to their parts through [`DateInputHost`].
//! Each segment you type in is a native text field, as the original's are
//! `<input>`s: the field's hooks are the original's `beforeinput`, `input`
//! and `focusout` listeners, every edit goes through them, and the field's
//! text is written from the state, so typing never writes it.

use std::rc::Rc;

use gpui::{
    AnyWindowHandle, App, AppContext, ClickEvent, Context, Entity, FocusHandle, Focusable,
    KeyDownEvent, Pixels, Point, SharedString, Subscription, WeakEntity, Window,
};
use icu_normalizer::ComposingNormalizer;
use icu_properties::props::{GeneralCategory, GeneralCategoryGroup};
use icu_properties::CodePointMapData;
use ui_sdk_calendar::{DateMatchers, matches_date};
use ui_sdk_date::{ParseDateOptions, parse_date, today_date};
use ui_sdk_forms::{ElementInternals, Node, focus_with, submit_implicitly};
use ui_sdk_kernel::modality::focus_event;
use ui_sdk_kernel::{
    Agent, AgentState, ChangeCallback, ChangeOptions, ChangeSource, Controllable, InputEvent,
    Instance, Key, Parts, Reason, is_composing,
};
use ui_sdk_native::{InputType, TextField, TextFieldOptions, TextFieldStyle};
use ui_sdk_platform::is_rtl;
use ui_sdk_text::{bind_page_locale, js_trim, locale_of};

use crate::date_input_model::{
    Buffer, DateInputGranularity, DateInputSegmentData, DateInputSegmentType, EditableType, End,
    Fields, Format, HourCycle, Kind, ValueFormat, bounds_of, clamp_day, compare_to, date_of,
    empty_fields, fields_of, hour_cycle_of, normalize_value, page_step_of, segments_of,
    step_field, step_to_end, type_digit, type_period, types_of, value_of,
};

/// Why a date or time input's value wants to change: typing or erasing
/// (`InputChange`), arrow, Page and Home or End keys (`StepKey`), a paste
/// (`Paste`), a form reset (`Reset`), or an api call (`Imperative`). The
/// original's `restore` has no meaning here.
pub type DateInputValueChangeReason = Reason;

/// Options for every write method.
pub type DateInputChangeOptions = ChangeOptions;

/// What Date Input's and Time Input's options share, and what their state
/// is made from.
#[derive(Clone, Default)]
pub struct SharedOptions {
    /// The value, or `Some(None)` for none. Pass it to control the input.
    pub value: Option<Option<SharedString>>,
    /// The value it starts with, when uncontrolled, and goes back to when
    /// its form resets.
    pub default_value: Option<Option<SharedString>>,
    pub on_value_change: Option<ChangeCallback<Option<SharedString>>>,
    pub granularity: Option<DateInputGranularity>,
    pub hour_cycle: Option<HourCycle>,
    pub min: Option<SharedString>,
    pub max: Option<SharedString>,
    pub unavailable: Option<DateMatchers>,
    pub locale: Option<SharedString>,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub name: Option<SharedString>,
    pub form: Option<SharedString>,
    pub field_style: Option<DateInputFieldStyle>,
    pub debug: bool,
}

/// The style each segment's text field paints with, by the segment's type:
/// what a stylesheet sets on each segment's `<input>`, like its caret, its
/// alignment and its width in `ch`.
pub type DateInputFieldStyle = Rc<dyn Fn(DateInputSegmentType) -> TextFieldStyle>;

// A segment's style: the one `field_style` gives its type, else none.
fn style_of(field_style: Option<&DateInputFieldStyle>, kind: EditableType) -> TextFieldStyle {
    field_style.map_or_else(TextFieldStyle::default, |style| style(kind.into()))
}

/// What a component built on the segments gives its parts: its core.
/// Date Input and Time Input implement it.
pub trait DateInputHost: Agent {
    fn date_input(&self) -> &DateInputCore;

    fn date_input_mut(&mut self) -> &mut DateInputCore;
}

// The event or the source an edit reports itself with.
#[derive(Clone, Debug, Default)]
pub(crate) struct Cause {
    event: Option<InputEvent>,
    source: Option<ChangeSource>,
}

impl Cause {
    fn key(event: &KeyDownEvent) -> Self {
        Self {
            event: Some(InputEvent::from(event)),
            source: None,
        }
    }

    // Text a person typed or pasted, which no gpui event carries.
    fn typed(source: ChangeSource) -> Self {
        Self {
            event: None,
            source: Some(source),
        }
    }

    fn focus(window: &Window) -> Self {
        Self {
            event: Some(focus_event(window)),
            source: None,
        }
    }

    fn options(&self, reason: Reason) -> ChangeOptions {
        ChangeOptions {
            reason: Some(reason),
            event: self.event.clone(),
            source: self.source,
        }
    }
}

// A segment's text field, its focus handle and its place in the page for
// the form. The handle is the field's own, kept here, since the field's
// hooks run inside its update, where the field can't be read.
pub(crate) struct SegmentField {
    pub(crate) field: Entity<TextField>,
    pub(crate) focus: FocusHandle,
    pub(crate) node: Node,
    options: TextFieldOptions,
}

/// The state and api Date Input and Time Input share (`State`,
/// `dateInputApi`).
pub struct DateInputCore {
    pub(crate) instance: Instance,
    pub(crate) kind: Kind,
    pub(crate) value: Controllable<Option<SharedString>>,
    pub(crate) initial: Option<SharedString>,
    has_default: bool,
    // What the segments hold while they differ from the value, like a date
    // typed halfway. `fields_for` names the value they were typed over; a
    // new value from anywhere else leaves them behind.
    fields: Fields,
    fields_for: Option<Option<SharedString>>,
    // The digits typed into a segment that may take another.
    buffer: Option<Buffer>,
    pub(crate) granularity: DateInputGranularity,
    hour_cycle: Option<HourCycle>,
    locale: Option<SharedString>,
    pub(crate) min: Option<SharedString>,
    pub(crate) max: Option<SharedString>,
    unavailable: Option<DateMatchers>,
    pub(crate) disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) name: Option<SharedString>,
    pub(crate) form: Option<SharedString>,
    field_style: Option<DateInputFieldStyle>,
    pub(crate) internals: ElementInternals,
    pub(crate) segments: Vec<(EditableType, SegmentField)>,
    // The segments as the last frame rendered them, in page order, with
    // their bounds.
    pub(crate) parts: Parts<SharedString, ()>,
    // The segment focus moved to once the last one was done, so the
    // separator typed next, as in 12/25, doesn't skip it.
    advanced_to: Option<EditableType>,
    window: AnyWindowHandle,
    sync_queued: bool,
    _subscriptions: Vec<Subscription>,
}

fn granularity_of(kind: Kind, options: &SharedOptions) -> DateInputGranularity {
    options.granularity.unwrap_or(match kind {
        Kind::Date => DateInputGranularity::Day,
        Kind::Time => DateInputGranularity::Minute,
    })
}

// A bound keeps a date alone, so a date-and-time input's `max` can take a whole day.
fn normalize_bound(bound: Option<&SharedString>, format: ValueFormat) -> Option<SharedString> {
    let bound = bound?;
    let date_alone = format.kind == Kind::Date && date_of(bound).is_some_and(|date| date.len() == bound.len());
    normalize_value(
        Some(bound),
        ValueFormat {
            granularity: if date_alone {
                DateInputGranularity::Day
            } else {
                DateInputGranularity::Second
            },
            ..format
        },
    )
}

fn value_for(kind: Kind, options: &SharedOptions, value: Option<&SharedString>) -> Option<SharedString> {
    normalize_value(
        value.map(|value| value.as_ref()),
        ValueFormat {
            kind,
            granularity: granularity_of(kind, options),
            hour_cycle: HourCycle::H24,
        },
    )
}

// `/[\s/.,:\-–]/`.
fn is_separator(c: char) -> bool {
    ui_sdk_text::is_js_space(c) || matches!(c, '/' | '.' | ',' | ':' | '-' | '–')
}

// `\p{L}`.
fn is_letter(c: char) -> bool {
    GeneralCategoryGroup::Letter.contains(CodePointMapData::<GeneralCategory>::new().get(c))
}

fn nfkc(text: &str) -> String {
    ComposingNormalizer::new_nfkc().normalize(text).into_owned()
}

fn placeholder_of(kind: EditableType) -> &'static str {
    if kind == EditableType::Year { "––––" } else { "––" }
}

impl DateInputCore {
    /// A date or time input's state from its options (`initialDateInputState`),
    /// with a text field for each segment it can show.
    pub fn new<H: DateInputHost>(
        name: &'static str,
        kind: Kind,
        options: SharedOptions,
        window: &mut Window,
        cx: &mut Context<H>,
    ) -> Self {
        let mut instance = Instance::new(name).with_agent(cx);
        instance.debug = options.debug;
        let first = options
            .value
            .clone()
            .or_else(|| options.default_value.clone())
            .flatten();
        let mut value = Controllable::new("value", value_for(kind, &options, first.as_ref()));
        value.set_controlled(
            options
                .value
                .as_ref()
                .map(|value| value_for(kind, &options, value.as_ref())),
        );
        value.set_on_change(options.on_value_change.clone());
        let initial = value_for(kind, &options, options.default_value.clone().flatten().as_ref());
        let internals = ElementInternals::new(cx);
        let weak = cx.weak_entity();
        let reset = internals.on_reset(cx, |host: &mut H, window, cx| host.date_input_mut().reset(window, cx));
        let disabled = internals.on_disabled(cx, |host: &mut H, _disabled, _window, cx| {
            host.date_input_mut().sync_field_options(cx);
            cx.notify();
        });
        // A label, or a submit that finds it invalid, focuses its first segment.
        let focused = {
            let weak = weak.clone();
            focus_with(&internals, move |window, cx| {
                let first = weak
                    .upgrade()
                    .and_then(|host| host.read(cx).date_input().first_segment_handle(cx));
                if let Some(first) = first {
                    window.focus(&first, cx);
                }
            })
        };
        let locale = bind_page_locale(cx, |host: &mut H, _locale, cx| {
            let core = host.date_input_mut();
            core.queue_sync(cx);
            core.report(cx);
            cx.notify();
        });
        let types: &[EditableType] = match kind {
            Kind::Date => &EditableType::ALL,
            Kind::Time => &[
                EditableType::Hour,
                EditableType::Minute,
                EditableType::Second,
                EditableType::DayPeriod,
            ],
        };
        let mut core = Self {
            instance,
            kind,
            value,
            initial,
            has_default: options.default_value.is_some(),
            fields: empty_fields(),
            fields_for: None,
            buffer: None,
            granularity: DateInputGranularity::Day,
            hour_cycle: None,
            locale: None,
            min: None,
            max: None,
            unavailable: None,
            disabled: false,
            read_only: false,
            required: false,
            name: None,
            form: None,
            field_style: None,
            internals,
            segments: Vec::new(),
            parts: Parts::new(),
            advanced_to: None,
            window: window.window_handle(),
            sync_queued: false,
            _subscriptions: vec![reset, disabled, focused, locale],
        };
        core.apply_settings(&options);
        let segments = core.segments_for(cx);
        for &kind in types {
            let text = segments
                .iter()
                .find(|segment| segment.key.as_ref() == kind.as_str())
                .map(|segment| segment.text.clone())
                .unwrap_or_default();
            let options = core.field_options(kind, weak.clone()).default_value(text);
            let field = cx.new(|cx| TextField::new(options.clone(), window, cx));
            let focus = field.read(cx).focus_handle(cx);
            core.segments.push((
                kind,
                SegmentField {
                    field,
                    focus,
                    node: Node::element(),
                    options,
                },
            ));
        }
        core.report(cx);
        core
    }

    // What every options object sets (`settings`). Returns whether what
    // renders changed.
    fn apply_settings(&mut self, options: &SharedOptions) -> bool {
        let kind = self.kind;
        let granularity = granularity_of(kind, options);
        let format = ValueFormat {
            kind,
            granularity,
            hour_cycle: HourCycle::H24,
        };
        let min = normalize_bound(options.min.as_ref(), format);
        let max = normalize_bound(options.max.as_ref(), format);
        let unavailable = match kind {
            Kind::Date => options.unavailable.clone(),
            Kind::Time => None,
        };
        let changed = self.granularity != granularity
            || self.hour_cycle != options.hour_cycle
            || self.locale != options.locale
            || self.min != min
            || self.max != max
            || self.unavailable != unavailable
            || self.disabled != options.disabled
            || self.read_only != options.read_only
            || self.required != options.required
            || self.name != options.name
            || self.form != options.form;
        self.granularity = granularity;
        self.hour_cycle = options.hour_cycle;
        self.locale = options.locale.clone();
        self.min = min;
        self.max = max;
        self.unavailable = unavailable;
        self.disabled = options.disabled;
        self.read_only = options.read_only;
        self.required = options.required;
        self.name = options.name.clone();
        self.form = options.form.clone();
        // The fields take a new style as they're synced.
        self.field_style = options.field_style.clone();
        self.instance.debug = options.debug;
        changed
    }

    /// Hands it new options (`syncDateInputState`). Returns whether what
    /// renders changed.
    pub fn sync<H: DateInputHost>(&mut self, options: &SharedOptions, cx: &mut Context<H>) -> bool {
        let kind = self.kind;
        let mut changed = self.apply_settings(options);
        if let Some(default) = &options.default_value {
            self.initial = value_for(kind, options, default.as_ref());
        }
        self.has_default = options.default_value.is_some();
        changed |= self.value.set_controlled(
            options
                .value
                .as_ref()
                .map(|value| value_for(kind, options, value.as_ref())),
        );
        self.value.set_on_change(options.on_value_change.clone());
        // A field whose options are the same is left alone, so a style is
        // synced whether or not anything else changed.
        self.sync_field_options(cx);
        if changed {
            self.changed(cx);
        }
        changed
    }

    /// Passes the controlled value, `Some(None)` for none, or `None` to let
    /// the input keep its own. Returns whether what renders changed.
    pub fn set_controlled_value<H: DateInputHost>(
        &mut self,
        value: Option<Option<SharedString>>,
        cx: &mut Context<H>,
    ) -> bool {
        let format = self.format(cx).value_format();
        let changed = self
            .value
            .set_controlled(value.map(|value| normalize_value(value.as_deref(), format)));
        if changed {
            self.changed(cx);
        }
        changed
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// Its part in forms.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    /// The value: an ISO date, date and time, or time, or `None` while empty
    /// or partly filled.
    pub fn get_value(&self) -> Option<&SharedString> {
        self.value.get().as_ref()
    }

    /// The language its segments show in: its `locale`, else the page's.
    pub fn locale(&self, cx: &App) -> SharedString {
        locale_of(self.locale.as_ref(), cx)
    }

    pub(crate) fn format(&self, cx: &App) -> Format {
        let locale = self.locale(cx);
        Format {
            kind: self.kind,
            granularity: self.granularity,
            hour_cycle: self.hour_cycle.unwrap_or_else(|| hour_cycle_of(&locale)),
            locale,
        }
    }

    fn editing(&self) -> bool {
        self.fields_for.as_ref() == Some(self.value.get())
    }

    // What the segments hold: what was typed over the value, else the value's own fields.
    fn shown_fields(&self, cx: &App) -> Fields {
        if self.editing() {
            self.fields
        } else {
            fields_of(self.value.get().as_deref(), self.format(cx).value_format())
        }
    }

    /// The segments in the order the locale shows them, with the text
    /// between them.
    pub fn segments_for(&self, cx: &App) -> Vec<DateInputSegmentData> {
        let buffer = if self.editing() { self.buffer.as_ref() } else { None };
        segments_of(&self.format(cx), &self.shown_fields(cx), buffer)
    }

    pub(crate) fn segment_for(&self, key: &str, cx: &App) -> Option<DateInputSegmentData> {
        self.segments_for(cx).into_iter().find(|segment| segment.key.as_ref() == key)
    }

    /// Whether some segments hold something while others are empty.
    pub fn is_partial(&self, cx: &App) -> bool {
        if self.value.get().is_some() {
            return false;
        }
        let fields = self.shown_fields(cx);
        types_of(self.format(cx).value_format())
            .into_iter()
            .any(|kind| fields.get(kind).is_some())
    }

    /// What's wrong with a whole value: before `min`, after `max`, or an
    /// unavailable date. `None` when it's fine or there's no value.
    pub fn problem_of(&self) -> Option<ValueProblem> {
        let value = self.value.get().as_ref()?;
        if let Some(min) = &self.min
            && compare_to(value, min).is_lt()
        {
            return Some(ValueProblem::RangeUnderflow);
        }
        if let Some(max) = &self.max
            && compare_to(value, max).is_gt()
        {
            return Some(ValueProblem::RangeOverflow);
        }
        let date = date_of(value)?;
        matches_date(&date, self.unavailable.as_ref()).then_some(ValueProblem::Unavailable)
    }

    /// Whether it can't be used: by itself, its field or a fieldset.
    pub fn is_disabled(&self) -> bool {
        self.disabled || self.internals.form_disabled() || self.internals.control_attrs().disabled
    }

    fn editable(&self) -> bool {
        !self.is_disabled() && !self.read_only
    }

    // The options a segment's text field takes, with the hooks that hand
    // every edit to the input.
    fn field_options<H: DateInputHost>(&self, kind: EditableType, weak: WeakEntity<H>) -> TextFieldOptions {
        let before = weak.clone();
        let input = weak.clone();
        let blur = weak;
        TextFieldOptions::default()
            .placeholder(placeholder_of(kind))
            .cols(if kind == EditableType::Year { 4 } else { 2 })
            .style(style_of(self.field_style.as_ref(), kind))
            .disabled(self.is_disabled())
            .read_only(self.read_only)
            .required(self.required)
            .select_on_focus(true)
            .on_before_input(move |_field, edit, window, cx| {
                if edit.composing && !edit.cancelable {
                    return;
                }
                // An input method's commit is written, then read back, as the
                // original reads what the browser wrote.
                if edit.input_type == InputType::InsertFromComposition {
                    return;
                }
                let cause = Cause::typed(edit.source);
                if edit.input_type == InputType::InsertLineBreak {
                    before
                        .update(cx, |host, cx| host.date_input_mut().finish(&cause, window, cx))
                        .ok();
                    return;
                }
                edit.prevent_default();
                let (input_type, data) = (edit.input_type, edit.data.clone());
                before
                    .update(cx, |host, cx| {
                        host.date_input_mut().before_input(kind, input_type, &data, &cause, window, cx)
                    })
                    .ok();
            })
            .on_input(move |field, edit, window, cx| {
                if edit.composing {
                    return;
                }
                let typed = field.value().clone();
                let cause = Cause::typed(edit.source);
                input
                    .update(cx, |host, cx| host.date_input_mut().adopt(kind, &typed, &cause, window, cx))
                    .ok();
            })
            .on_blur(move |_field, window, cx| {
                blur.update(cx, |host, cx| host.date_input_mut().blur(kind, window, cx)).ok();
            })
    }

    // Writes whether the segments are disabled, read-only and required, and
    // the style their type takes, to their text fields.
    pub(crate) fn sync_field_options(&mut self, cx: &mut App) {
        let (disabled, read_only, required) = (self.is_disabled(), self.read_only, self.required);
        let field_style = self.field_style.clone();
        for (kind, segment) in &mut self.segments {
            let style = style_of(field_style.as_ref(), *kind);
            if segment.options.disabled == disabled
                && segment.options.read_only == read_only
                && segment.options.required == required
                && segment.options.style == style
            {
                continue;
            }
            segment.options.disabled = disabled;
            segment.options.read_only = read_only;
            segment.options.required = required;
            segment.options.style = style;
            let options = segment.options.clone();
            segment.field.update(cx, |field, cx| field.set_options(options, cx));
        }
    }

    // Writes each segment's text to its field once the update is done, so a
    // field's own hook never writes it under itself, and selects the
    // focused one's, as the original does on every change.
    fn queue_sync<H: DateInputHost>(&mut self, cx: &mut Context<H>) {
        if std::mem::replace(&mut self.sync_queued, true) {
            return;
        }
        let window = self.window;
        let entity = cx.weak_entity();
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    entity
                        .update(cx, |host, cx| {
                            let core = host.date_input_mut();
                            core.sync_queued = false;
                            core.sync_fields(window, cx);
                        })
                        .ok();
                })
                .ok();
        });
    }

    fn sync_fields(&self, window: &mut Window, cx: &mut App) {
        let segments = self.segments_for(cx);
        for (kind, segment) in &self.segments {
            let text = segments
                .iter()
                .find(|each| each.key.as_ref() == kind.as_str())
                .map(|each| each.text.clone())
                .unwrap_or_default();
            segment.field.update(cx, |field, cx| {
                if *field.value() != text {
                    field.set_value(text, window, cx);
                }
                if field.is_focused(window) {
                    field.select(window, cx);
                }
            });
        }
    }

    // What every change asks for: the form hears it, the fields show it, and
    // the parts render it.
    fn changed<H: DateInputHost>(&mut self, cx: &mut Context<H>) {
        self.report(cx);
        self.queue_sync(cx);
        cx.notify();
    }

    pub(crate) fn field_of(&self, kind: EditableType) -> Option<&Entity<TextField>> {
        self.segment_of(kind).map(|segment| &segment.field)
    }

    fn segment_of(&self, kind: EditableType) -> Option<&SegmentField> {
        self.segments
            .iter()
            .find(|(each, _)| *each == kind)
            .map(|(_, segment)| segment)
    }

    pub(crate) fn node_of(&self, kind: EditableType) -> Option<&Node> {
        self.segments
            .iter()
            .find(|(each, _)| *each == kind)
            .map(|(_, segment)| &segment.node)
    }

    // The segments you type in, as the last frame rendered them, in page order.
    pub(crate) fn rendered_segments(&self, cx: &App) -> Vec<EditableType> {
        let segments = self.segments_for(cx);
        self.parts
            .keys()
            .into_iter()
            .filter_map(|key| {
                segments
                    .iter()
                    .find(|segment| segment.key == key)
                    .and_then(|segment| segment.r#type.editable())
            })
            .collect()
    }

    pub(crate) fn first_segment_handle(&self, cx: &App) -> Option<gpui::FocusHandle> {
        let first = *self.rendered_segments(cx).first()?;
        Some(self.segment_of(first)?.focus.clone())
    }

    /// Where focus goes for the input: its first segment, or before the
    /// segments render, the first the locale shows.
    pub fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        if let Some(handle) = self.first_segment_handle(cx) {
            return handle;
        }
        let first = self
            .segments_for(cx)
            .into_iter()
            .find_map(|segment| segment.r#type.editable())
            .and_then(|kind| self.segment_of(kind));
        match first.or_else(|| self.segments.first().map(|(_, segment)| segment)) {
            Some(segment) => segment.focus.clone(),
            None => cx.focus_handle(),
        }
    }

    /// Its state as the agent bridge reads it.
    pub fn agent_state(&self) -> AgentState {
        AgentState::new()
            .with("value", self.value.get().as_ref().map(ToString::to_string))
            .with("disabled", self.is_disabled())
            .with("readOnly", self.read_only)
            .with("required", self.required)
            .with("min", self.min.as_ref().map(ToString::to_string))
            .with("max", self.max.as_ref().map(ToString::to_string))
    }

    fn focused_segment(&self, window: &Window) -> Option<EditableType> {
        self.segments
            .iter()
            .find(|(_, segment)| segment.focus.is_focused(window))
            .map(|(kind, _)| *kind)
    }

    // Focuses a segment's field, as leaving the one focus is on finishes
    // what's typed there, as its `focusout` does.
    fn focus_segment_field<H: DateInputHost>(
        &mut self,
        kind: EditableType,
        cause: &Cause,
        window: &mut Window,
        cx: &mut Context<H>,
    ) -> bool {
        let Some((disabled, handle)) = self
            .segment_of(kind)
            .map(|segment| (segment.options.disabled, segment.focus.clone()))
        else {
            return false;
        };
        if disabled {
            return false;
        }
        if let Some(from) = self.focused_segment(window)
            && from != kind
            && self.buffer.as_ref().is_some_and(|buffer| buffer.r#type == from)
        {
            self.finish(cause, window, cx);
        }
        window.focus(&handle, cx);
        true
    }

    // The fields after an edit, and the value they make when all are
    // filled. A segment that may take another digit holds its value back, so
    // typing a year reports 2026 once, not 2, 20 and 202 on the way.
    fn edit<H: DateInputHost>(
        &mut self,
        fields: Fields,
        buffer: Option<Buffer>,
        reason: Reason,
        cause: &Cause,
        window: &mut Window,
        cx: &mut Context<H>,
    ) {
        let current = self.value.get().clone();
        if buffer.is_some() {
            self.fields = fields;
            self.buffer = buffer;
            self.fields_for = Some(current);
            self.changed(cx);
            return;
        }
        let next = value_of(&fields, self.format(cx).value_format());
        if next != current {
            self.value.change(&self.instance, next.clone(), cause.options(reason), window, cx);
        }
        self.fields = fields;
        self.buffer = None;
        self.fields_for = Some(next);
        self.changed(cx);
    }

    fn finish<H: DateInputHost>(&mut self, cause: &Cause, window: &mut Window, cx: &mut Context<H>) {
        if self.buffer.is_none() {
            return;
        }
        if !self.editing() {
            self.buffer = None;
            self.changed(cx);
            return;
        }
        let fields = clamp_day(self.fields);
        self.edit(fields, None, Reason::InputChange, cause, window, cx);
    }

    fn change_value<H: DateInputHost>(
        &mut self,
        value: Option<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<H>,
    ) -> bool {
        let format = self.format(cx).value_format();
        let next = value.as_deref().and_then(|value| normalize_value(Some(value), format));
        if value.is_some() && next.is_none() {
            return false;
        }
        let changed = next == *self.value.get() || self.value.change(&self.instance, next, options, window, cx);
        self.fields = empty_fields();
        self.fields_for = None;
        self.buffer = None;
        self.changed(cx);
        changed
    }

    /// Sets the value. Returns false when it isn't one the input can show,
    /// or when canceled.
    pub fn set_value<H: DateInputHost>(
        &mut self,
        value: Option<SharedString>,
        options: DateInputChangeOptions,
        window: &mut Window,
        cx: &mut Context<H>,
    ) -> bool {
        self.change_value(value, options, window, cx)
    }

    /// Empties every segment.
    pub fn clear<H: DateInputHost>(
        &mut self,
        options: DateInputChangeOptions,
        window: &mut Window,
        cx: &mut Context<H>,
    ) -> bool {
        self.change_value(None, options, window, cx)
    }

    // Moves focus to the segment `offset` from `from`.
    fn move_focus<H: DateInputHost>(
        &mut self,
        from: EditableType,
        offset: isize,
        cause: &Cause,
        window: &mut Window,
        cx: &mut Context<H>,
    ) -> bool {
        let list = self.rendered_segments(cx);
        let index = list.iter().position(|kind| *kind == from).map_or(-1, |index| index as isize);
        let target = index + offset;
        let Some(&target) = usize::try_from(target).ok().and_then(|target| list.get(target)) else {
            return false;
        };
        self.focus_segment_field(target, cause, window, cx)
    }

    // Where an empty segment starts when an arrow key steps it: today's
    // date, or the start of the day.
    fn start_of(&self, kind: EditableType, direction: i32, fields: &Fields, hour_cycle: HourCycle) -> i32 {
        let today = today_date(None);
        let part = |range: std::ops::Range<usize>| today[range].parse::<i32>().unwrap_or(1);
        let (min, max) = bounds_of(kind, fields, hour_cycle);
        match kind {
            EditableType::Year => part(0..4),
            EditableType::Month => part(5..7),
            EditableType::Day => part(8..10).min(max),
            _ if direction == 1 => min,
            _ => max,
        }
    }

    fn step<H: DateInputHost>(
        &mut self,
        kind: EditableType,
        key: &Key,
        cause: &Cause,
        window: &mut Window,
        cx: &mut Context<H>,
    ) {
        let fields = self.shown_fields(cx);
        let hour_cycle = self.format(cx).hour_cycle;
        let next = match key {
            Key::Home => step_to_end(&fields, kind, End::Min, hour_cycle),
            Key::End => step_to_end(&fields, kind, End::Max, hour_cycle),
            _ => {
                let direction = if matches!(key, Key::ArrowUp | Key::PageUp) { 1 } else { -1 };
                let size = if matches!(key, Key::PageUp | Key::PageDown) {
                    page_step_of(kind)
                } else {
                    1
                };
                let start = self.start_of(kind, direction, &fields, hour_cycle);
                step_field(&fields, kind, direction * size, hour_cycle, start)
            }
        };
        self.edit(next, None, Reason::StepKey, cause, window, cx);
    }

    // The segment after the one being typed in takes focus, or past the
    // last, what's typed is finished.
    fn advance<H: DateInputHost>(
        &mut self,
        current: &mut EditableType,
        cause: &Cause,
        window: &mut Window,
        cx: &mut Context<H>,
    ) -> bool {
        let list = self.rendered_segments(cx);
        let following = list
            .iter()
            .position(|kind| kind == current)
            .and_then(|index| list.get(index + 1))
            .copied();
        let Some(following) = following else {
            self.finish(cause, window, cx);
            return false;
        };
        self.focus_segment_field(following, cause, window, cx);
        *current = following;
        true
    }

    /// Characters typed into a segment. Digits fill it, a letter picks AM or
    /// PM, and a separator moves on. Once a segment is done the rest go to
    /// the next.
    fn insert<H: DateInputHost>(
        &mut self,
        kind: EditableType,
        text: &str,
        cause: &Cause,
        window: &mut Window,
        cx: &mut Context<H>,
    ) {
        let format = self.format(cx);
        let (hour_cycle, locale) = (format.hour_cycle, format.locale);
        let mut current = kind;
        for c in text.chars() {
            let fields = self.shown_fields(cx);
            let buffer = if self.editing() { self.buffer.clone() } else { None };
            let advanced = self.advanced_to == Some(current);
            self.advanced_to = None;
            if c.is_ascii_digit() && current != EditableType::DayPeriod {
                let typed = type_digit(&fields, buffer.as_ref(), current, &c.to_string(), hour_cycle);
                self.edit(typed.fields, typed.buffer, Reason::InputChange, cause, window, cx);
                if typed.done {
                    if !self.advance(&mut current, cause, window, cx) {
                        break;
                    }
                    self.advanced_to = Some(current);
                }
            } else if is_separator(c) {
                if advanced
                    || (buffer.as_ref().map(|buffer| buffer.r#type) != Some(current) && fields.get(current).is_none())
                {
                    continue;
                }
                self.finish(cause, window, cx);
                if !self.advance(&mut current, cause, window, cx) {
                    break;
                }
            } else if is_letter(c) {
                let period = type_period(&fields, &c.to_string(), &locale);
                if let Some(period) = period
                    && (current == EditableType::DayPeriod || hour_cycle == HourCycle::H12)
                {
                    self.edit(period, None, Reason::InputChange, cause, window, cx);
                    if current == EditableType::DayPeriod && !self.advance(&mut current, cause, window, cx) {
                        break;
                    }
                }
            }
        }
    }

    /// Backspace removes the last digit typed, then empties the segment,
    /// then moves back. Delete empties it.
    fn remove<H: DateInputHost>(
        &mut self,
        kind: EditableType,
        backward: bool,
        cause: &Cause,
        window: &mut Window,
        cx: &mut Context<H>,
    ) {
        let fields = self.shown_fields(cx);
        let buffer = if self.editing() { self.buffer.clone() } else { None };
        if backward
            && let Some(buffer) = buffer.as_ref().filter(|buffer| buffer.r#type == kind && buffer.text.len() > 1)
        {
            let mut text = buffer.text.clone();
            text.pop();
            let hour_cycle = self.format(cx).hour_cycle;
            let typed = type_digit(&fields.with(kind, None), None, kind, &text, hour_cycle);
            self.edit(
                typed.fields,
                Some(Buffer { r#type: kind, text }),
                Reason::InputChange,
                cause,
                window,
                cx,
            );
            return;
        }
        if fields.get(kind).is_some() || buffer.as_ref().is_some_and(|buffer| buffer.r#type == kind) {
            self.edit(fields.with(kind, None), None, Reason::InputChange, cause, window, cx);
            return;
        }
        if backward {
            self.move_focus(kind, -1, cause, window, cx);
        }
    }

    // A paste into any segment: a whole value, as ISO or as the locale
    // writes it.
    fn paste<H: DateInputHost>(&mut self, text: &str, cause: &Cause, window: &mut Window, cx: &mut Context<H>) {
        let text = js_trim(text);
        if text.is_empty() || !self.editable() {
            return;
        }
        let format = self.format(cx);
        let mut value = normalize_value(Some(text), format.value_format());
        if value.is_none()
            && self.kind == Kind::Date
            && let Some(date) = parse_date(text, Some(&format.locale), &ParseDateOptions::default())
        {
            value = normalize_value(Some(&date), format.value_format());
        }
        if let Some(value) = value {
            self.change_value(Some(value), cause.options(Reason::Paste), window, cx);
        }
    }

    /// A key pressed on a segment (`internal.keydown`).
    pub(crate) fn keydown<H: DateInputHost>(
        &mut self,
        kind: EditableType,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<H>,
    ) {
        let modifiers = event.keystroke.modifiers;
        if is_composing(window, cx) || modifiers.alt || modifiers.control || modifiers.platform {
            return;
        }
        let key = Key::of(&event.keystroke);
        let cause = Cause::key(event);
        match key {
            Key::ArrowLeft | Key::ArrowRight => {
                window.prevent_default();
                let forward = (key == Key::ArrowRight) != is_rtl(cx);
                self.move_focus(kind, if forward { 1 } else { -1 }, &cause, window, cx);
            }
            Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown | Key::Home | Key::End => {
                window.prevent_default();
                if self.editable() {
                    self.step(kind, &key, &cause, window, cx);
                }
            }
            Key::Backspace | Key::Delete => {
                // Handled here for every key, so the field never deletes
                // its text itself.
                window.prevent_default();
                if self.editable() {
                    self.remove(kind, key == Key::Backspace, &cause, window, cx);
                }
            }
            Key::Enter => {
                // The digits typed so far are kept first, and the form
                // submits with them, as Enter in a native one does.
                self.finish(&cause, window, cx);
                submit_implicitly(self.internals.form().as_ref(), window, cx);
            }
            _ => {}
        }
    }

    // Every edit goes through here, so the field never writes the segment's
    // text (`beforeinput`). Paste goes to `paste`, as the original's `paste`
    // listener takes it first.
    fn before_input<H: DateInputHost>(
        &mut self,
        kind: EditableType,
        input_type: InputType,
        data: &str,
        cause: &Cause,
        window: &mut Window,
        cx: &mut Context<H>,
    ) {
        if input_type == InputType::InsertFromPaste {
            self.paste(data, cause, window, cx);
            return;
        }
        if !self.editable() {
            return;
        }
        if input_type.is_insert() && !data.is_empty() {
            self.insert(kind, data, cause, window, cx);
        } else if input_type.is_delete() {
            self.remove(kind, input_type.is_backward(), cause, window, cx);
        }
    }

    /// Text the field wrote anyway, like an input method's: the digits in it
    /// fill the segment (`adopt`).
    fn adopt<H: DateInputHost>(
        &mut self,
        kind: EditableType,
        typed: &str,
        cause: &Cause,
        window: &mut Window,
        cx: &mut Context<H>,
    ) {
        let shown = self
            .segment_for(kind.as_str(), cx)
            .map(|segment| segment.text)
            .unwrap_or_default();
        // The field shows the segment's text again afterwards, whatever it
        // wrote (`write`).
        self.queue_sync(cx);
        if typed == shown.as_ref() || !self.editable() {
            return;
        }
        // East Asian input methods commit full-width digits and letters.
        let text = nfkc(typed);
        let digits: String = text.chars().filter(char::is_ascii_digit).collect();
        if kind == EditableType::DayPeriod || digits.is_empty() {
            if let Some(letter) = text.chars().filter(|c| is_letter(*c)).last() {
                self.insert(kind, &letter.to_string(), cause, window, cx);
                return;
            }
            let fields = self.shown_fields(cx).with(kind, None);
            self.edit(fields, None, Reason::InputChange, cause, window, cx);
            return;
        }
        let shown_digits = shown.chars().filter(char::is_ascii_digit).count();
        let added = if !shown.is_empty() && text.starts_with(shown.as_ref()) {
            digits.get(shown_digits..).unwrap_or("").to_string()
        } else {
            digits.clone()
        };
        let text = if added.is_empty() {
            digits.chars().last().map(String::from).unwrap_or_default()
        } else {
            added
        };
        self.insert(kind, &text, cause, window, cx);
    }

    /// Focuses a segment by its type, like `"month"`, or the first one.
    pub fn focus<H: DateInputHost>(&mut self, segment: Option<&str>, window: &mut Window, cx: &mut Context<H>) {
        let list = self.rendered_segments(cx);
        let target = match segment {
            Some(name) => list.into_iter().find(|kind| kind.as_str() == name),
            None => list.first().copied(),
        };
        if let Some(target) = target {
            let cause = Cause::focus(window);
            self.focus_segment_field(target, &cause, window, cx);
        }
    }

    /// The segment nearest a press on the box around the segments, or on the
    /// text between them, so the whole field takes a press like a native
    /// one. `None` for a press on a segment.
    pub(crate) fn nearest_segment(&self, position: Point<Pixels>, cx: &App) -> Option<EditableType> {
        if !self.editable() {
            return None;
        }
        let list: Vec<(EditableType, gpui::Bounds<Pixels>)> = self
            .rendered_segments(cx)
            .into_iter()
            .filter_map(|kind| Some((kind, self.parts.bounds(&SharedString::new_static(kind.as_str()))?)))
            .collect();
        if list.iter().any(|(_, bounds)| bounds.contains(&position)) {
            return None;
        }
        let distance = |bounds: &gpui::Bounds<Pixels>| {
            let left = f32::from(bounds.left() - position.x);
            let right = f32::from(position.x - bounds.right());
            left.max(right).max(0.)
        };
        list.into_iter()
            .fold(None::<(EditableType, f32)>, |best, (kind, bounds)| {
                let far = distance(&bounds);
                match best {
                    Some((_, nearest)) if nearest <= far => best,
                    _ => Some((kind, far)),
                }
            })
            .map(|(kind, _)| kind)
    }

    /// A press on the root, outside every segment: the nearest takes focus.
    pub(crate) fn press_root<H: DateInputHost>(&mut self, event: &ClickEvent, window: &mut Window, cx: &mut Context<H>) {
        if matches!(event, ClickEvent::Keyboard(_)) {
            return;
        }
        if let Some(kind) = self.nearest_segment(event.position(), cx) {
            let cause = Cause {
                event: Some(InputEvent::from(event)),
                source: None,
            };
            self.focus_segment_field(kind, &cause, window, cx);
        }
    }

    // A segment lost focus (`focusout`): what was typed in it is finished.
    // The segment this input moved focus to itself keeps its mark.
    fn blur<H: DateInputHost>(&mut self, kind: EditableType, window: &mut Window, cx: &mut Context<H>) {
        if let Some(advanced) = self.advanced_to
            && !self.segment_of(advanced).is_some_and(|segment| segment.focus.is_focused(window))
        {
            self.advanced_to = None;
        }
        if self.buffer.as_ref().is_some_and(|buffer| buffer.r#type == kind) {
            self.finish(&Cause::focus(window), window, cx);
        }
    }

    // A form reset puts the default back, unless the app controls the value
    // with no default.
    fn reset<H: DateInputHost>(&mut self, window: &mut Window, cx: &mut Context<H>) {
        let controlled = self.value.is_controlled().then_some(());
        let default = self.has_default.then_some(());
        if ui_sdk_forms::keeps_value_on_reset(controlled.as_ref(), default.as_ref()) {
            return;
        }
        let initial = self.initial.clone();
        self.change_value(initial, ChangeOptions::reason(Reason::Reset), window, cx);
    }
}

/// What's wrong with a whole value (`problemOf`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueProblem {
    /// Before `min`.
    RangeUnderflow,
    /// After `max`.
    RangeOverflow,
    /// A date `unavailable` names.
    Unavailable,
}
