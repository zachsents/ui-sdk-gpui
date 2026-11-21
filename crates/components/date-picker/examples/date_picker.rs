//! Date pickers: one date with its format hint and Clear, with weekends
//! unavailable, and a range over two months with presets.

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px,
    rgb, size,
};
use ui_sdk_date_picker::{
    CalendarDayState, CalendarValue, DateMatcher, DatePicker, DatePickerContentProps, DatePickerMode,
    DatePickerOptions, PresentationSetting,
};

struct Example {
    due: Entity<DatePicker>,
    stay: Entity<DatePicker>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        ui_sdk_focus::bind_tab_keys(cx);
        let due = cx.new(|cx| {
            DatePicker::new(
                DatePickerOptions::default()
                    .default_value(CalendarValue::single("2026-10-16"))
                    .min("2026-10-05")
                    .max("2026-12-31")
                    .unavailable(DateMatcher::weekday([0, 6]))
                    .name("due")
                    .on_value_change(|value, details, _window, _cx| {
                        log::info!("{value:?} ({})", details.reason.as_str());
                    }),
                window,
                cx,
            )
        });
        let stay = cx.new(|cx| {
            DatePicker::new(
                DatePickerOptions::default()
                    .mode(DatePickerMode::Range)
                    .months(2)
                    .default_month("2026-10")
                    .start_name("check-in")
                    .end_name("check-out")
                    .presentation(PresentationSetting::Auto),
                window,
                cx,
            )
        });
        Self { due, stay }
    }
}

fn day_color(state: &CalendarDayState) -> gpui::Rgba {
    if state.selected {
        rgb(0x1f2937)
    } else if state.preview {
        rgb(0xe5e7eb)
    } else {
        rgb(0xffffff)
    }
}

fn day(picker: &Entity<DatePicker>, date: SharedString, offset: usize, cx: &App) -> AnyElement {
    DatePicker::day(picker, date, offset, cx)
        .with(|el, state| {
            el.bg(day_color(state))
                .text_color(if state.selected {
                    rgb(0xffffff)
                } else if state.disabled || state.outside {
                    rgb(0x9ca3af)
                } else {
                    rgb(0x111827)
                })
                .when(state.today, |el| el.border_1().border_color(rgb(0x2563eb)))
                .when(state.unavailable, |el| el.line_through())
                .child(state.label.clone())
        })
        .size(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .focus(|style| style.border_2().border_color(rgb(0x2563eb)))
        .into_any_element()
}

fn grid(picker: &Entity<DatePicker>, offset: usize, cx: &App) -> impl IntoElement {
    let part = DatePicker::grid(picker, offset, cx);
    let weeks = part.state().weeks.clone();
    part.flex()
        .flex_col()
        .gap_1()
        .child(
            DatePicker::weekdays(picker, offset, cx).flex().gap_1().children((0..7).map(|index| {
                DatePicker::weekday(picker, offset, index, cx)
                    .with(|el, state| el.child(state.label.clone()))
                    .w(px(32.))
                    .flex()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(0x6b7280))
            })),
        )
        .children(weeks.into_iter().enumerate().map(|(index, week)| {
            DatePicker::week(picker, offset, index, cx).flex().gap_1().children(week.into_iter().map(|cell| {
                if cell.hidden {
                    div().size(px(32.)).into_any_element()
                } else {
                    day(picker, cell.date, offset, cx)
                }
            }))
        }))
}

fn button<S>(part: ui_sdk_kernel::Part<S>) -> ui_sdk_kernel::Part<S> {
    part.px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .bg(rgb(0xffffff))
        .focus(|style| style.border_color(rgb(0x2563eb)))
}

// The popup: Previous, the heading and Next, a grid for each month shown,
// and the presets given.
fn popup(picker: &Entity<DatePicker>, presets: &[(&'static str, CalendarValue)], cx: &App) -> Option<AnyElement> {
    let months = picker.read(cx).model().months();
    let step = |part: ui_sdk_kernel::Part<ui_sdk_date_picker::CalendarStepState>, glyph: &'static str| {
        part.with(|el, state| el.text_color(if state.disabled { rgb(0xd1d5db) } else { rgb(0x111827) }))
            .px_2()
            .rounded_md()
            .focus(|style| style.bg(rgb(0xe5e7eb)))
            .child(glyph)
    };
    let content = DatePicker::content(picker, DatePickerContentProps::default(), cx)?
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xe5e7eb))
        .rounded_lg()
        .shadow_md()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(step(DatePicker::previous_part(picker, cx), "‹"))
                .child(DatePicker::heading(picker, cx).with(|el, state| el.child(state.label.clone())))
                .child(step(DatePicker::next_part(picker, cx), "›")),
        )
        .child(div().flex().gap_4().children((0..months).map(|offset| grid(picker, offset, cx))))
        .when(!presets.is_empty(), |content| {
            content.child(div().flex().gap_2().children(presets.iter().map(|(label, value)| {
                button(DatePicker::preset(picker, value.clone(), cx))
                    .with(|el, state| el.when(state.selected, |el| el.bg(rgb(0x1f2937)).text_color(rgb(0xffffff))))
                    .aria_label(*label)
                    .child(*label)
            })))
        });
    Some(content.into_any_element())
}

fn input(picker: &Entity<DatePicker>, end: bool, label: &'static str, cx: &App) -> impl IntoElement {
    DatePicker::input(picker, end, cx)
        .with(|el, state| el.border_color(if state.invalid { rgb(0xdc2626) } else { rgb(0xd1d5db) }))
        .aria_label(label)
        .w(px(120.))
        .border_1()
        .rounded_md()
        .px_2()
        .py_1()
}

fn trigger(picker: &Entity<DatePicker>, cx: &App) -> impl IntoElement {
    button(DatePicker::trigger(picker, cx))
        .with(|el, state| el.when(state.open, |el| el.bg(rgb(0xe5e7eb))))
        .child("Choose")
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let due = &self.due;
        let stay = &self.stay;
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_8()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(
                DatePicker::root(due, cx)
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(input(due, false, "Due date", cx))
                            .child(trigger(due, cx))
                            .child(
                                button(DatePicker::clear_part(due, cx))
                                    .with(|el, state| el.when(state.empty || state.disabled, |el| el.opacity(0.5)))
                                    .child("Clear"),
                            ),
                    )
                    .child(
                        DatePicker::hint(due, cx)
                            .with(|el, state| el.child(state.placeholder.clone()))
                            .text_xs()
                            .text_color(rgb(0x6b7280)),
                    )
                    .children(popup(due, &[], cx)),
            )
            .child(
                DatePicker::root(stay, cx)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(input(stay, false, "Check-in", cx))
                    .child(input(stay, true, "Check-out", cx))
                    .child(trigger(stay, cx))
                    .children(popup(
                        stay,
                        &[
                            ("Oct 5–9", CalendarValue::range("2026-10-05", "2026-10-09")),
                            ("Oct 12–16", CalendarValue::range("2026-10-12", "2026-10-16")),
                        ],
                        cx,
                    )),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(760.), px(560.)), cx);
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
