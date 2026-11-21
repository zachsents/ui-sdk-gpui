//! A settings window with a select, a slider and an alert dialog, which
//! `docs/getting-started.md` builds step by step.

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Div, Entity, Rgba, SharedString, Window, WindowBounds, WindowOptions, div, px,
    rgb, rgba, size,
};
use ui_sdk::components::dialog::{
    AlertDialog, AlertDialogOptions, DialogCloseProps, DialogCloseState, DialogContentProps, DialogTriggerProps,
};
use ui_sdk::components::select::{
    Select, SelectContentProps, SelectItemProps, SelectOptions, SelectTriggerProps, SelectValueProps,
};
use ui_sdk::components::slider::{Slider, SliderOptions};
use ui_sdk::foundation::platform::live_regions;
use ui_sdk::kernel::{ActionOutcome, ChangeOptions, Part};

const THEMES: &[(&str, &str)] = &[("light", "Light"), ("dark", "Dark"), ("system", "System")];
const DEFAULT_THEME: &str = "system";
const DEFAULT_TEXT_SIZE: f64 = 16.0;

struct Settings {
    theme: Entity<Select>,
    text_size: Entity<Slider<f64>>,
    reset: Entity<AlertDialog>,
}

impl Settings {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let theme = cx.new(|cx| {
            Select::new(
                SelectOptions::default().default_value(Some(SharedString::from(DEFAULT_THEME))),
                window,
                cx,
            )
        });
        let text_size = cx.new(|cx| {
            Slider::new(
                SliderOptions::default()
                    .min(12.0)
                    .max(24.0)
                    .default_value(DEFAULT_TEXT_SIZE),
                window,
                cx,
            )
        });
        let reset = cx.new(|cx| AlertDialog::new(AlertDialogOptions::default(), window, cx));
        Self {
            theme,
            text_size,
            reset,
        }
    }

    fn theme_select(&self, cx: &App) -> impl IntoElement {
        let label = Select::label(&self.theme, cx).aria_label("Theme").child("Theme");
        let value = Select::value_part(&self.theme, SelectValueProps::default(), cx)
            .with(|el, state| el.child(state.text.clone()));
        let trigger = Select::trigger(&self.theme, SelectTriggerProps::default(), cx)
            .flex()
            .justify_between()
            .w(px(200.))
            .px_3()
            .py_1()
            .border_1()
            .border_color(rgb(0xd1d5db))
            .rounded_md()
            .focus_visible(|style| style.border_color(rgb(0x2563eb)))
            .child(value)
            .child("▾");
        let content = Select::content(&self.theme, SelectContentProps::default(), cx).map(|content| {
            content
                .flex()
                .flex_col()
                .min_w(px(200.))
                .p_1()
                .bg(rgb(0xffffff))
                .border_1()
                .border_color(rgb(0xd1d5db))
                .rounded_md()
                .shadow_md()
                .children(THEMES.iter().map(|&(value, name)| {
                    Select::item(&self.theme, SelectItemProps::new(value).text(name), cx)
                        .with(|el, state| {
                            el.when(state.highlighted, |el| el.bg(rgb(0xeff6ff)))
                                .when(state.selected, |el| el.font_weight(gpui::FontWeight::SEMIBOLD))
                        })
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .child(name)
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(label)
            .child(trigger)
            .children(content)
    }

    fn text_size_slider(&self, cx: &App) -> impl IntoElement {
        let thumb = Slider::thumb(&self.text_size, 0, cx)
            .aria_label("Text size")
            .size_4()
            .rounded_full()
            .border_2()
            .border_color(rgb(0x1f2937))
            .bg(rgb(0xffffff))
            .focus_visible(|style| style.border_color(rgb(0x2563eb)));
        let track = Slider::track(&self.text_size, cx)
            .w_full()
            .h(px(4.))
            .rounded_full()
            .bg(rgb(0xe5e7eb))
            .child(Slider::indicator(&self.text_size, cx).rounded_full().bg(rgb(0x1f2937)))
            .child(thumb);
        let control = Slider::control(&self.text_size, cx)
            .flex()
            .items_center()
            .w(px(200.))
            .h(px(20.))
            .child(track);
        let value = Slider::value(&self.text_size, cx).with(|el, state| el.child(state.text.clone()));
        Slider::root(&self.text_size, cx)
            .flex()
            .flex_col()
            .gap_1()
            .child(div().flex().gap_2().child("Text size").child(value))
            .child(control)
    }

    fn reset_dialog(&self, cx: &App) -> impl IntoElement {
        let trigger = AlertDialog::trigger(&self.reset, "reset", DialogTriggerProps::default(), cx)
            .px_3()
            .py_1()
            .rounded_md()
            .bg(rgb(0xf3f4f6))
            .aria_label("Reset")
            .child("Reset");
        let backdrop = AlertDialog::backdrop(&self.reset, None, cx).map(|backdrop| backdrop.bg(rgba(0x0000004d)));
        let theme = self.theme.clone();
        let text_size = self.text_size.clone();
        let confirm = DialogCloseProps::default()
            .value("reset")
            .action(move |_event, _details, window, cx| {
                theme.update(cx, |select, cx| {
                    select.set_value(
                        Some(SharedString::from(DEFAULT_THEME)),
                        ChangeOptions::default(),
                        window,
                        cx,
                    );
                });
                text_size.update(cx, |slider, cx| {
                    slider.set_value(DEFAULT_TEXT_SIZE, ChangeOptions::default(), window, cx);
                });
                ActionOutcome::Done
            });
        let content = AlertDialog::content(&self.reset, DialogContentProps::default(), cx).map(|content| {
            content
                .with(|el, _| {
                    el.m_auto()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(280.))
                        .p_4()
                        .rounded_md()
                        .bg(rgb(0xffffff))
                })
                .child(AlertDialog::title(&self.reset, cx).aria_label("Reset settings?").child("Reset settings?"))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(button(
                            AlertDialog::close_part(&self.reset, "cancel", DialogCloseProps::default(), cx),
                            "Cancel",
                            rgb(0xf3f4f6),
                        ))
                        .child(
                            button(AlertDialog::close_part(&self.reset, "confirm", confirm, cx), "Reset", rgb(0xfee2e2))
                                .text_color(rgb(0x991b1b)),
                        ),
                )
        });
        div().child(trigger).children(backdrop).children(content)
    }
}

fn button(part: Part<DialogCloseState>, label: &'static str, color: Rgba) -> Part<DialogCloseState> {
    part.aria_label(label).px_3().py_1().rounded_md().bg(color).child(label)
}

fn preview(theme: &str, text_size: f64) -> Div {
    let (background, foreground) = match theme {
        "dark" => (rgb(0x111827), rgb(0xf9fafb)),
        _ => (rgb(0xf9fafb), rgb(0x111827)),
    };
    div()
        .p_3()
        .rounded_md()
        .bg(background)
        .text_color(foreground)
        .text_size(px(text_size as f32))
        .child("Aa")
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.read(cx).value().first().cloned().unwrap_or_default();
        let text_size = self.text_size.read(cx).get_value();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_4()
            .p_6()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(self.theme_select(cx))
            .child(self.text_size_slider(cx))
            .child(preview(&theme, text_size))
            .child(self.reset_dialog(cx))
            .child(live_regions(window))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        ui_sdk::init("ui-sdk-settings", cx);
        let bounds = Bounds::centered(None, size(px(420.), px(420.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Settings::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
