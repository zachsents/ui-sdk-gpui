//! Color pickers: HSV with an area, hue and alpha sliders, a hex input and
//! presets; OKLCH on a painted area in the P3 gamut; sRGB with channel
//! inputs; and a read-only one.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_color_picker::{
    ColorChannel, ColorGamut, ColorPicker, ColorPickerAreaProps, ColorPickerChannelInputProps,
    ColorPickerOptions, ColorPickerSliderProps, ColorPickerSpace, ColorPickerSwatchItemProps,
};

const PRESETS: [&str; 6] = ["#ef4444", "#f97316", "#eab308", "#22c55e", "#0ea5e9", "#8b5cf6"];

struct Example {
    brand: Entity<ColorPicker>,
    accent: Entity<ColorPicker>,
    background: Entity<ColorPicker>,
    theme: Entity<ColorPicker>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let brand = cx.new(|cx| {
            ColorPicker::new(
                ColorPickerOptions::default()
                    .default_value("#0ea5e9")
                    .on_value_commit(|value, details, _window, _cx| {
                        log::info!("brand {value} ({})", details.reason.as_str());
                    }),
                window,
                cx,
            )
        });
        let accent = cx.new(|cx| {
            ColorPicker::new(
                ColorPickerOptions::default()
                    .space(ColorPickerSpace::Oklch)
                    .gamut(ColorGamut::P3)
                    .default_value("oklch(0.7 0.2 150)"),
                window,
                cx,
            )
        });
        let background = cx.new(|cx| {
            ColorPicker::new(
                ColorPickerOptions::default()
                    .space(ColorPickerSpace::Srgb)
                    .default_value("rgb(30 64 175)"),
                window,
                cx,
            )
        });
        let theme = cx.new(|cx| {
            ColorPicker::new(
                ColorPickerOptions::default()
                    .default_value("#8b5cf6")
                    .read_only(true),
                window,
                cx,
            )
        });
        Self {
            brand,
            accent,
            background,
            theme,
        }
    }
}

// An area with its gradients or its canvas, and its thumb.
fn area(entity: &Entity<ColorPicker>, painted: bool, cx: &App) -> impl IntoElement {
    let props = ColorPickerAreaProps::default();
    ColorPicker::area(entity, props, cx)
        .w(px(220.))
        .h(px(140.))
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .with(|el, state| el.children(state.background.map(|background| background.element())))
        .when(painted, |el| {
            el.child(ColorPicker::area_canvas(entity, props, cx))
        })
        .child(
            ColorPicker::area_thumb(entity, props, cx)
                .size(px(14.))
                .ml(px(-7.))
                .mt(px(-7.))
                .rounded_full()
                .border_2()
                .border_color(rgb(0xffffff))
                .with(|el, state| el.bg(state.fill)),
        )
}

// A slider for a channel, with its track's gradient and its thumb.
fn slider(entity: &Entity<ColorPicker>, channel: ColorChannel, cx: &App) -> impl IntoElement {
    let props = ColorPickerSliderProps::default().channel(channel);
    ColorPicker::slider(entity, props, cx)
        .w(px(220.))
        .h(px(12.))
        .rounded_full()
        .with(|el, state| el.child(state.background.element()))
        .child(
            ColorPicker::slider_thumb(entity, props, cx)
                .size(px(16.))
                .ml(px(-8.))
                .top(px(-2.))
                .rounded_full()
                .border_2()
                .border_color(rgb(0xffffff))
                .bg(rgb(0x111827)),
        )
}

fn swatch(entity: &Entity<ColorPicker>, cx: &App) -> impl IntoElement {
    ColorPicker::swatch(entity, cx)
        .size(px(32.))
        .rounded_md()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .with(|el, state| el.bg(state.fill))
}

fn input(entity: &Entity<ColorPicker>, cx: &App) -> impl IntoElement {
    ColorPicker::input(entity, cx)
        .aria_label("Hex")
        .w(px(180.))
        .px_2()
        .py_1()
        .border_1()
        .rounded_md()
        .border_color(rgb(0xd1d5db))
}

fn presets(entity: &Entity<ColorPicker>, cx: &App) -> impl IntoElement {
    ColorPicker::swatches(entity, cx)
        .flex()
        .gap_1()
        .children(PRESETS.iter().map(|value| {
            ColorPicker::swatch_item(entity, ColorPickerSwatchItemProps::new(*value), cx)
                .size(px(24.))
                .rounded_sm()
                .border_2()
                .with(|el, state| {
                    el.when_some(state.fill, |el, fill| el.bg(fill))
                        .border_color(if state.selected {
                            rgb(0x111827)
                        } else {
                            rgb(0xffffff)
                        })
                })
        }))
}

fn channels(entity: &Entity<ColorPicker>, cx: &App) -> impl IntoElement {
    div()
        .flex()
        .gap_2()
        .children(
            [ColorChannel::Red, ColorChannel::Green, ColorChannel::Blue].map(|channel| {
                ColorPicker::channel_input(
                    entity,
                    ColorPickerChannelInputProps::default().channel(channel),
                    cx,
                )
                .w(px(64.))
                .px_2()
                .py_1()
                .border_1()
                .rounded_md()
                .border_color(rgb(0xd1d5db))
            }),
        )
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cx: &App = cx;
        div()
            .size_full()
            .flex()
            .items_start()
            .gap_8()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_6()
                    .child(
                        ColorPicker::root(&self.brand, cx)
                            .aria_label("Brand color")
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_2()
                            .child(area(&self.brand, false, cx))
                            .child(slider(&self.brand, ColorChannel::Hue, cx))
                            .child(slider(&self.brand, ColorChannel::Alpha, cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(swatch(&self.brand, cx))
                                    .child(input(&self.brand, cx)),
                            )
                            .child(presets(&self.brand, cx)),
                    )
                    .child(
                        ColorPicker::root(&self.theme, cx)
                            .aria_label("Theme color")
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(swatch(&self.theme, cx))
                            .child(input(&self.theme, cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_6()
                    .child(
                        ColorPicker::root(&self.accent, cx)
                            .aria_label("Accent")
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_2()
                            .child(area(&self.accent, true, cx))
                            .child(slider(&self.accent, ColorChannel::Hue, cx))
                            .child(input(&self.accent, cx)),
                    )
                    .child(
                        ColorPicker::root(&self.background, cx)
                            .aria_label("Background")
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_2()
                            .child(swatch(&self.background, cx))
                            .child(channels(&self.background, cx)),
                    ),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(600.), px(520.)), cx);
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
