//! Sliders: one thumb, a range, a range kept apart, a vertical one, a
//! disabled one and a controlled one.

use gpui::{
    App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb,
    size, text,
};
use ui_sdk_number::{NumberFormatOptions, NumberStyle};
use ui_sdk_slider::{Slider, SliderOptions, SliderOrientation, SliderValue};

struct Example {
    volume: Entity<Slider<f64>>,
    price: Entity<Slider<Vec<f64>>>,
    hours: Entity<Slider<Vec<f64>>>,
    bass: Entity<Slider<f64>>,
    brightness: Entity<Slider<f64>>,
    opacity: Entity<Slider<f64>>,
    opacity_value: f64,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let volume = cx.new(|cx| Slider::new(SliderOptions::default().default_value(50.0), window, cx));
        let price = cx.new(|cx| {
            Slider::new(
                SliderOptions::default()
                    .default_value(vec![200.0, 800.0])
                    .max(1000.0)
                    .step(10.0)
                    .format(
                        NumberFormatOptions::default()
                            .style(NumberStyle::Currency)
                            .currency("USD"),
                    ),
                window,
                cx,
            )
        });
        let hours = cx.new(|cx| {
            Slider::new(
                SliderOptions::default()
                    .default_value(vec![9.0, 17.0])
                    .max(24.0)
                    .min_steps_between_thumbs(4.0),
                window,
                cx,
            )
        });
        let bass = cx.new(|cx| {
            Slider::new(
                SliderOptions::default()
                    .default_value(30.0)
                    .orientation(SliderOrientation::Vertical),
                window,
                cx,
            )
        });
        let brightness = cx.new(|cx| {
            Slider::new(
                SliderOptions::default().default_value(70.0).disabled(true),
                window,
                cx,
            )
        });
        let this = cx.entity().downgrade();
        let opacity = cx.new(|cx| {
            Slider::new(
                SliderOptions::default()
                    .value(0.6)
                    .max(1.0)
                    .step(0.05)
                    .format(NumberFormatOptions::default().style(NumberStyle::Percent))
                    .on_value_change(move |value, _details, _window, cx| {
                        let value = *value;
                        this.update(cx, |example, cx| {
                            example.opacity_value = value;
                            cx.notify();
                        })
                        .ok();
                    }),
                window,
                cx,
            )
        });
        Self {
            volume,
            price,
            hours,
            bass,
            brightness,
            opacity,
            opacity_value: 0.6,
        }
    }
}

// A slider with a thumb for each name, and its value above the track.
fn slider<V: SliderValue>(
    entity: &Entity<Slider<V>>,
    title: &'static str,
    thumbs: &[&'static str],
    cx: &App,
) -> impl IntoElement {
    let vertical = entity.read(cx).state().orientation == SliderOrientation::Vertical;
    // A range is a group named for what it picks; a single thumb names
    // itself.
    let range = thumbs.len() > 1;
    let thumbs: Vec<_> = thumbs
        .iter()
        .enumerate()
        .map(|(index, name)| {
            Slider::thumb(entity, index, cx)
                .aria_label(SharedString::from(*name))
                .size_4()
                .rounded_full()
                .border_2()
                .bg(rgb(0xffffff))
                .with(|el, state| {
                    el.border_color(if state.disabled {
                        rgb(0x9ca3af)
                    } else if state.dragged {
                        rgb(0x2563eb)
                    } else {
                        rgb(0x1f2937)
                    })
                })
                .focus_visible(|style| style.border_color(rgb(0x2563eb)))
        })
        .collect();
    let track = Slider::track(entity, cx)
        .rounded_full()
        .bg(rgb(0xe5e7eb))
        .child(Slider::indicator(entity, cx).rounded_full().with(|el, state| {
            el.bg(if state.disabled {
                rgb(0x9ca3af)
            } else {
                rgb(0x1f2937)
            })
        }))
        .children(thumbs);
    let control = Slider::control(entity, cx).flex().items_center().justify_center();
    let control = if vertical {
        control.w(px(20.)).h(px(120.)).child(track.w(px(4.)).h_full())
    } else {
        control.w(px(200.)).h(px(20.)).child(track.h(px(4.)).w_full())
    };
    let root = Slider::root(entity, cx).flex().flex_col().gap_1();
    let root = if range {
        root.aria_label(SharedString::from(title))
    } else {
        root
    };
    root.child(
        div()
            .flex()
            .gap_2()
            .text_sm()
            .child(title)
            .child(Slider::value(entity, cx).with(|el, state| el.child(text!(state.text.clone())))),
    )
    .child(control)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let opacity = self.opacity_value;
        self.opacity
            .update(cx, |slider, cx| slider.set_controlled_value(Some(opacity), cx));
        let cx: &App = cx;
        div()
            .size_full()
            .flex()
            .items_start()
            .gap_8()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_4()
                    .child(slider(&self.volume, "Volume", &["Volume"], cx))
                    .child(slider(
                        &self.price,
                        "Price",
                        &["Minimum price", "Maximum price"],
                        cx,
                    ))
                    .child(slider(&self.hours, "Hours", &["Opens", "Closes"], cx))
                    .child(slider(&self.brightness, "Brightness", &["Brightness"], cx))
                    .child(slider(&self.opacity, "Opacity", &["Opacity"], cx)),
            )
            .child(slider(&self.bass, "Bass", &["Bass"], cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(480.), px(420.)), cx);
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
