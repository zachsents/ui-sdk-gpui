//! Checkboxes: one with a label, one indeterminate, one disabled, and a group
//! with a parent checkbox and a disabled child.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, ease_in_out, px,
    rgb, size,
};
use ui_sdk_checkbox::{
    CheckedState, Checkbox, CheckboxGroup, CheckboxGroupOptions, CheckboxIndicatorProps,
    CheckboxOptions,
};
use ui_sdk_forms::{Label, LabelOptions};
use ui_sdk_motion::{PresenceTiming, Timing};

const TOPPINGS: [(&str, &str); 3] = [("cheese", "Cheese"), ("olives", "Olives"), ("peppers", "Peppers")];

struct Labelled {
    checkbox: Entity<Checkbox>,
    label: Label,
    text: &'static str,
}

impl Labelled {
    fn new(text: &'static str, options: CheckboxOptions, window: &mut Window, cx: &mut App) -> Self {
        Self {
            checkbox: cx.new(|cx| Checkbox::new(options, window, cx)),
            label: Label::new(LabelOptions::default().text(text)),
            text,
        }
    }
}

struct Example {
    singles: Vec<Labelled>,
    toppings: Entity<CheckboxGroup>,
    all: Labelled,
    items: Vec<Labelled>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let timing = PresenceTiming::new(Timing::ms(150).easing(ease_in_out));
        let singles = vec![
            Labelled::new(
                "Accept terms",
                CheckboxOptions::default().name("terms").indicator_timing(timing.clone()),
                window,
                cx,
            ),
            Labelled::new(
                "Some files",
                CheckboxOptions::default().default_checked(CheckedState::Indeterminate),
                window,
                cx,
            ),
            Labelled::new(
                "Security alerts",
                CheckboxOptions::default().default_checked(true).disabled(true),
                window,
                cx,
            ),
        ];
        let toppings = cx.new(|cx| {
            CheckboxGroup::new(
                CheckboxGroupOptions::default()
                    .name("toppings")
                    .default_value(["cheese"])
                    .all_values(TOPPINGS.map(|(value, _)| value)),
                window,
                cx,
            )
        });
        let all = Labelled::new(
            "Select all",
            CheckboxOptions::default().group(&toppings).parent(true),
            window,
            cx,
        );
        let items = TOPPINGS
            .iter()
            .map(|&(value, text)| {
                let options = CheckboxOptions::default()
                    .group(&toppings)
                    .value(value)
                    .disabled(value == "peppers")
                    .indicator_timing(timing.clone());
                Labelled::new(text, options, window, cx)
            })
            .collect();
        Self {
            singles,
            toppings,
            all,
            items,
        }
    }
}

fn row(item: &Labelled, cx: &App) -> impl IntoElement {
    let checkbox = Checkbox::root(&item.checkbox, cx)
        .size_4()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(rgb(0x6b7280))
        .with(|el, state| {
            el.bg(if state.checked || state.indeterminate { rgb(0x1f2937) } else { rgb(0xffffff) })
                .opacity(if state.disabled { 0.5 } else { 1.0 })
        })
        .children(
            Checkbox::indicator(&item.checkbox, CheckboxIndicatorProps::default(), cx).map(|indicator| {
                indicator.text_xs().text_color(rgb(0xffffff)).with(|el, state| {
                    el.opacity(state.presence.progress)
                        .child(if state.indeterminate { "–" } else { "✓" })
                })
            }),
        );
    item.label
        .root()
        .flex()
        .items_center()
        .gap_2()
        .child(checkbox)
        .child(item.text)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cx: &App = cx;
        div()
            .size_full()
            .flex()
            .gap_8()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(self.singles.iter().map(|item| row(item, cx))),
            )
            .child(
                CheckboxGroup::group(&self.toppings, cx)
                    .aria_label(SharedString::from("Toppings"))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(row(&self.all, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .pl_4()
                            .children(self.items.iter().map(|item| row(item, cx))),
                    ),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(420.), px(200.)), cx);
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
