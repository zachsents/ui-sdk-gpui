//! Date and time inputs: a date, a date and time on a 12-hour clock, a date
//! in German, and times on a 24-hour clock with seconds, in a form that
//! Enter submits.

use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, FontFeatures, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Styled, Window, WindowBounds, WindowOptions,
    div, px, rgb, size, transparent_black,
};
use ui_sdk_date_input::{
    DateInput, DateInputGranularity, DateInputOptions, DateInputSegmentType, DateMatcher,
    HourCycle, TextFieldAlign, TextFieldStyle, TimeInput, TimeInputGranularity, TimeInputOptions,
};
use ui_sdk_forms::{Form, FormOptions, Label, LabelOptions};

struct Example {
    start: Entity<DateInput>,
    meeting: Entity<DateInput>,
    german: Entity<DateInput>,
    alarm: Entity<TimeInput>,
    closed: Entity<TimeInput>,
    form: Form,
    labels: Vec<Label>,
}

const TEXTS: [&str; 5] = ["Start date", "Meeting", "Datum", "Alarm", "Closes"];

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let start = cx.new(|cx| {
            DateInput::new(
                DateInputOptions::default()
                    .default_value(Some("2026-11-02"))
                    .min("2026-01-01")
                    .max("2026-12-31")
                    .unavailable(DateMatcher::weekday([0, 6]))
                    .required(true)
                    .name("start")
                    .field_style(segment_field)
                    .on_value_change(|value, details, _window, _cx| {
                        log::info!("start {value:?} ({})", details.reason.as_str());
                    }),
                window,
                cx,
            )
        });
        let meeting = cx.new(|cx| {
            DateInput::new(
                DateInputOptions::default()
                    .granularity(DateInputGranularity::Minute)
                    .hour_cycle(HourCycle::H12)
                    .name("meeting")
                    .field_style(segment_field),
                window,
                cx,
            )
        });
        let german = cx.new(|cx| {
            DateInput::new(
                DateInputOptions::default()
                    .default_value(Some("2026-11-02"))
                    .locale("de-DE")
                    .name("datum")
                    .field_style(segment_field),
                window,
                cx,
            )
        });
        let alarm = cx.new(|cx| {
            TimeInput::new(
                TimeInputOptions::default()
                    .default_value(Some("07:30:00"))
                    .granularity(TimeInputGranularity::Second)
                    .hour_cycle(HourCycle::H24)
                    .name("alarm")
                    .field_style(segment_field),
                window,
                cx,
            )
        });
        let closed = cx.new(|cx| {
            TimeInput::new(
                TimeInputOptions::default()
                    .default_value(Some("18:00"))
                    .disabled(true)
                    .name("closes")
                    .field_style(segment_field),
                window,
                cx,
            )
        });
        let form = Form::new(FormOptions::default().on_submit(|data, _details, _window, _cx| {
            log::info!("submitted {}", data.to_url_encoded());
        }));
        let labels = TEXTS
            .into_iter()
            .map(|text| Label::new(LabelOptions::default().text(text)))
            .collect();
        Self {
            start,
            meeting,
            german,
            alarm,
            closed,
            form,
            labels,
        }
    }
}

// The box around the segments, and the segments in the locale's order.
fn field(
    root: ui_sdk_kernel::Part<ui_sdk_date_input::DateInputState>,
    segment: impl Fn(&ui_sdk_date_input::DateInputSegmentData) -> AnyElement,
) -> impl IntoElement {
    let segments = root.state().segments.clone();
    root.with(|el, state| {
        el.border_color(if state.invalid { rgb(0xdc2626) } else { rgb(0xd1d5db) })
            .when(state.disabled, |el| el.text_color(rgb(0x9ca3af)))
    })
    .flex()
    .items_center()
    .border_1()
    .rounded_md()
    .px_2()
    .py_1()
    .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
    .children(segments.iter().map(segment))
}

fn segment_field(segment: DateInputSegmentType) -> TextFieldStyle {
    let style = TextFieldStyle::default()
        .text_align(TextFieldAlign::Center)
        .caret_color(transparent_black())
        .selection_color(transparent_black());
    match segment {
        DateInputSegmentType::Year => style.width_ch(4.),
        DateInputSegmentType::DayPeriod => style.width_ch(2.6),
        _ => style.width_ch(2.),
    }
}

fn segment_style(
    part: ui_sdk_kernel::Part<ui_sdk_date_input::DateInputSegmentState>,
) -> AnyElement {
    part.with(|el, state| {
        if state.editable {
            el.h(px(24.)).px(px(2.)).rounded(px(5.))
        } else {
            el.child(state.text.clone()).px(px(1.)).text_color(rgb(0x6b7280))
        }
    })
    .focus(|style| style.bg(rgb(0x2563eb)).text_color(rgb(0xffffff)))
    .into_any_element()
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dates = [&self.start, &self.meeting, &self.german];
        let times = [&self.alarm, &self.closed];
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, entity) in dates.into_iter().enumerate() {
            let input = field(DateInput::root(entity, cx), |segment| {
                segment_style(DateInput::segment(entity, segment, cx))
            });
            rows.push(row(&self.labels[index], TEXTS[index], input).into_any_element());
        }
        for (index, entity) in times.into_iter().enumerate() {
            let at = dates.len() + index;
            let input = field(TimeInput::root(entity, cx), |segment| {
                segment_style(TimeInput::segment(entity, segment, cx))
            });
            rows.push(row(&self.labels[at], TEXTS[at], input).into_any_element());
        }
        div()
            .size_full()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(self.form.root().flex().flex_col().gap_3().children(rows))
    }
}

fn row(label: &Label, text: &'static str, input: impl IntoElement) -> impl IntoElement {
    label
        .root()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(96.)).child(SharedString::new_static(text)))
        .child(input)
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        ui_sdk_focus::bind_tab_keys(cx);
        let bounds = Bounds::centered(None, size(px(420.), px(300.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Example::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
