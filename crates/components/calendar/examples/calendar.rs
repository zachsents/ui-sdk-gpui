//! Calendars: one date with month and year selects, a range over two months,
//! and several dates with weekends unavailable.

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, StatefulInteractiveElement, Styled, Window, WindowBounds,
    WindowOptions, div, px, rgb, size,
};
use ui_sdk_calendar::{
    Calendar, CalendarDayState, CalendarMode, CalendarOptions, CalendarSelectKind, CalendarSelectOption,
    CalendarSelectState, CalendarStepState, DateMatcher,
};
use ui_sdk_kernel::Part;
use ui_sdk_select::{
    ListData, ListItem, Select, SelectContentProps, SelectItemProps, SelectOptions, SelectTriggerProps,
    SelectValueProps,
};

struct Example {
    single: Entity<Calendar>,
    month: Entity<Select<i32>>,
    year: Entity<Select<i32>>,
    range: Entity<Calendar>,
    multiple: Entity<Calendar>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let single = cx.new(|cx| {
            Calendar::new(
                CalendarOptions::default()
                    .default_month("1990-06")
                    .min("1920-01-01")
                    .max("2026-10-16")
                    .on_value_change(|value, details, _window, _cx| {
                        log::info!("{value:?} ({})", details.reason.as_str());
                    }),
                window,
                cx,
            )
        });
        // The single calendar's selects, which take their options from its
        // parts as it renders.
        let month = cx.new(|cx| Select::new(SelectOptions::default(), window, cx));
        let year = cx.new(|cx| Select::new(SelectOptions::default(), window, cx));
        let range = cx.new(|cx| {
            Calendar::new(
                CalendarOptions::default()
                    .mode(CalendarMode::Range)
                    .months(2)
                    .default_month("2026-10")
                    .min("2026-10-05")
                    .min_days(3.)
                    .max_days(8.),
                window,
                cx,
            )
        });
        let multiple = cx.new(|cx| {
            Calendar::new(
                CalendarOptions::default()
                    .mode(CalendarMode::Multiple)
                    .default_month("2026-11")
                    .unavailable(DateMatcher::weekday([0, 6]))
                    .fixed_weeks(true)
                    .show_outside_days(false),
                window,
                cx,
            )
        });
        Self {
            single,
            month,
            year,
            range,
            multiple,
        }
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

fn day(entity: &Entity<Calendar>, date: gpui::SharedString, offset: usize, cx: &App) -> AnyElement {
    Calendar::day(entity, date, offset, cx)
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

fn grid(entity: &Entity<Calendar>, offset: usize, cx: &App) -> impl IntoElement {
    let part = Calendar::grid(entity, offset, cx);
    let weeks = part.state().weeks.clone();
    part.flex()
        .flex_col()
        .gap_1()
        .child(
            Calendar::weekdays(entity, offset, cx).flex().gap_1().children((0..7).map(|index| {
                Calendar::weekday(entity, offset, index, cx)
                    .with(|el, state| el.child(state.label.clone()))
                    .w(px(32.))
                    .flex()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(0x6b7280))
            })),
        )
        .children(weeks.into_iter().enumerate().map(|(index, week)| {
            Calendar::week(entity, offset, index, cx).flex().gap_1().children(week.into_iter().map(|cell| {
                if cell.hidden {
                    div().size(px(32.)).into_any_element()
                } else {
                    day(entity, cell.date, offset, cx)
                }
            }))
        }))
}

fn step(part: Part<CalendarStepState>, glyph: &'static str) -> Part<CalendarStepState> {
    part.with(|el, state| el.text_color(if state.disabled { rgb(0xd1d5db) } else { rgb(0x111827) }))
        .px_2()
        .rounded_md()
        .focus(|style| style.bg(rgb(0xe5e7eb)))
        .child(glyph)
}

// With month and year selects, the heading stays out of sight, so screen
// readers still hear the months change.
fn calendar(entity: &Entity<Calendar>, selects: Option<Div>, cx: &App) -> impl IntoElement {
    let months = entity.read(cx).model().months();
    let heading = Calendar::heading(entity, cx).with(|el, state| el.child(state.label.clone()));
    Calendar::root(entity, cx)
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_between()
                .gap_1()
                .child(step(Calendar::previous_part(entity, cx), "‹"))
                .map(|header| match selects {
                    Some(selects) => header
                        .child(heading.absolute().size(px(1.)).overflow_hidden())
                        .child(selects),
                    None => header.child(heading),
                })
                .child(step(Calendar::next_part(entity, cx), "›")),
        )
        .child(div().flex().gap_4().children((0..months).map(|offset| grid(entity, offset, cx))))
}

// What a select takes from the calendar's month or year select part: its
// options, its value and whether it's disabled. A choice runs inside the
// select's update, so it updates the calendar, not the select, which
// follows the calendar's month as it renders.
fn select_options(calendar: &Entity<Calendar>, kind: CalendarSelectKind, state: &CalendarSelectState) -> SelectOptions<i32> {
    let items: ListData<i32> = state
        .options
        .iter()
        .map(|option| ListItem::new(option.value, option.label.clone()).disabled(option.disabled))
        .collect();
    let calendar = calendar.downgrade();
    SelectOptions::default()
        .items(items)
        .value(Some(state.value))
        .disabled(state.disabled)
        .on_value_change(move |value, details, window, cx| {
            if let Some(&value) = value.first() {
                calendar
                    .update(cx, |calendar, cx| calendar.select(kind, value, details.event.clone(), window, cx))
                    .ok();
            }
        })
}

fn item(select: &Entity<Select<i32>>, option: &CalendarSelectOption, cx: &App) -> impl IntoElement {
    let indicator = Select::item_indicator(select, &option.value, Default::default(), cx)
        .map(|indicator| indicator.child("✓"));
    let props = SelectItemProps::new(option.value)
        .text(option.label.clone())
        .disabled(option.disabled);
    Select::item(select, props, cx)
        .with(|el, state| {
            el.when(state.highlighted, |el| el.bg(rgb(0xe5e7eb)))
                .when(state.disabled, |el| el.text_color(rgb(0x9ca3af)))
        })
        .flex()
        .justify_between()
        .gap_4()
        .px_2()
        .py_1()
        .rounded_md()
        .child(option.label.clone())
        .children(indicator)
}

// A select in the calendar's part for it: the trigger, named as the part,
// and the list of the part's options.
fn calendar_select(
    part: Part<CalendarSelectState>,
    select: &Entity<Select<i32>>,
    label: &'static str,
    width: Pixels,
    cx: &App,
) -> Part<CalendarSelectState> {
    let options = part.state().options.clone();
    let trigger = Select::trigger(select, SelectTriggerProps::default(), cx)
        .with(|el, state| el.when(state.disabled, |el| el.text_color(rgb(0xd1d5db))))
        .aria_label(label)
        .flex()
        .items_center()
        .justify_between()
        .w(width)
        .px_2()
        .py_0p5()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .rounded_md()
        .focus(|style| style.border_color(rgb(0x2563eb)))
        .child(Select::value_part(select, SelectValueProps::default(), cx).with(|el, state| el.child(state.text.clone())))
        .child("▾");
    let content = Select::content(select, SelectContentProps::default(), cx).map(|content| {
        content
            .flex()
            .flex_col()
            .min_w(width)
            .max_h(px(240.))
            .overflow_y_scroll()
            .p_1()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0xd1d5db))
            .rounded_md()
            .shadow_md()
            .children(options.iter().map(|option| item(select, option, cx)))
    });
    part.child(trigger).children(content)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let month = Calendar::month_select(&self.single, cx);
        let year = Calendar::year_select(&self.single, cx);
        let options = select_options(&self.single, CalendarSelectKind::Month, month.state());
        self.month.update(cx, |select, cx| select.set_options(options, cx));
        let options = select_options(&self.single, CalendarSelectKind::Year, year.state());
        self.year.update(cx, |select, cx| select.set_options(options, cx));
        let selects = div()
            .flex()
            .gap_1()
            .child(calendar_select(month, &self.month, "Month", px(112.), cx))
            .child(calendar_select(year, &self.year, "Year", px(72.), cx));
        div()
            .size_full()
            .flex()
            .flex_wrap()
            .items_start()
            .gap_8()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(calendar(&self.single, Some(selects), cx))
            .child(calendar(&self.range, None, cx))
            .child(calendar(&self.multiple, None, cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        ui_sdk_focus::bind_tab_keys(cx);
        let bounds = Bounds::centered(None, size(px(980.), px(620.)), cx);
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
