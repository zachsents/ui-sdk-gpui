//! Number inputs: scrubbed by their labels, bounded, stepped by a quarter,
//! formatted as money and percents, in German, with the wheel, controlled,
//! read-only and disabled.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_number::{NumberFormatOptions, NumberStyle};
use ui_sdk_number_input::{
    NumberInput, NumberInputButtonProps, NumberInputOptions, NumberInputScrubAreaProps, TextFieldAlign,
    TextFieldStyle,
};

struct Example {
    quantity: Entity<NumberInput>,
    guests: Entity<NumberInput>,
    hours: Entity<NumberInput>,
    price: Entity<NumberInput>,
    discount: Entity<NumberInput>,
    preis: Entity<NumberInput>,
    font_size: Entity<NumberInput>,
    servings: Entity<NumberInput>,
    servings_value: Option<f64>,
    seats: Entity<NumberInput>,
    sold_out: Entity<NumberInput>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut make = |options: NumberInputOptions, cx: &mut Context<Self>| {
            let options = options.field_style(TextFieldStyle::default().text_align(TextFieldAlign::Center));
            cx.new(|cx| NumberInput::new(options, window, cx))
        };
        let quantity = make(
            NumberInputOptions::default()
                .default_value(1.0)
                .min(0.0)
                .on_value_commit(|value, details, _window, _cx| {
                    log::info!("quantity {value:?} ({})", details.reason.as_str());
                }),
            cx,
        );
        let guests = make(
            NumberInputOptions::default().default_value(2.0).min(1.0).max(8.0),
            cx,
        );
        let hours = make(
            NumberInputOptions::default()
                .default_value(8.0)
                .min(0.0)
                .max(24.0)
                .step(0.25),
            cx,
        );
        let price = make(
            NumberInputOptions::default().default_value(4.5).min(0.0).format(
                NumberFormatOptions::default()
                    .style(NumberStyle::Currency)
                    .currency("USD"),
            ),
            cx,
        );
        let discount = make(
            NumberInputOptions::default()
                .default_value(0.15)
                .min(0.0)
                .max(1.0)
                .format(NumberFormatOptions::default().style(NumberStyle::Percent)),
            cx,
        );
        let preis = make(
            NumberInputOptions::default()
                .default_value(1234.5)
                .locale("de-DE")
                .format(
                    NumberFormatOptions::default()
                        .style(NumberStyle::Currency)
                        .currency("EUR"),
                ),
            cx,
        );
        let font_size = make(
            NumberInputOptions::default()
                .default_value(16.0)
                .min(8.0)
                .max(72.0)
                .wheel(true),
            cx,
        );
        let this = cx.entity().downgrade();
        let servings = make(
            NumberInputOptions::default()
                .value(4.0)
                .min(1.0)
                .max(12.0)
                .on_value_change(move |value, _details, _window, cx| {
                    let value = *value;
                    this.update(cx, |example, cx| {
                        example.servings_value = value;
                        cx.notify();
                    })
                    .ok();
                }),
            cx,
        );
        let seats = make(
            NumberInputOptions::default().default_value(5.0).read_only(true),
            cx,
        );
        let sold_out = make(
            NumberInputOptions::default().default_value(3.0).disabled(true),
            cx,
        );
        Self {
            quantity,
            guests,
            hours,
            price,
            discount,
            preis,
            font_size,
            servings,
            servings_value: Some(4.0),
            seats,
            sold_out,
        }
    }
}

// A field whose label scrubs the value, with its step buttons around it.
fn field(entity: &Entity<NumberInput>, label: &'static str, cx: &App) -> impl IntoElement {
    let button = |part: ui_sdk_kernel::Part<ui_sdk_number_input::NumberInputButtonState>,
                  name: &'static str,
                  sign: &'static str| {
        part.aria_label(SharedString::from(name))
            .w(px(28.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .with(|el, state| {
                el.text_color(if state.disabled {
                    rgb(0xd1d5db)
                } else {
                    rgb(0x111827)
                })
            })
            .child(sign)
    };
    NumberInput::root(entity, cx)
        .flex()
        .flex_col()
        .gap_1()
        .child(
            NumberInput::scrub_area(entity, NumberInputScrubAreaProps::default(), cx)
                .text_sm()
                .text_color(rgb(0x4b5563))
                .cursor_ew_resize()
                .child(label),
        )
        .child(
            NumberInput::group(entity, cx)
                .flex()
                .items_center()
                .h(px(32.))
                .border_1()
                .rounded_md()
                .with(|el, state| {
                    el.border_color(if state.scrubbing {
                        rgb(0x2563eb)
                    } else {
                        rgb(0xd1d5db)
                    })
                    .bg(if state.disabled {
                        rgb(0xf3f4f6)
                    } else {
                        rgb(0xffffff)
                    })
                })
                .child(button(
                    NumberInput::decrement_button(entity, NumberInputButtonProps::default(), cx),
                    "Decrease",
                    "−",
                ))
                .child(
                    NumberInput::control(entity, cx)
                        .aria_label(SharedString::from(label))
                        .w(px(112.))
                        .h_full(),
                )
                .child(button(
                    NumberInput::increment_button(entity, NumberInputButtonProps::default(), cx),
                    "Increase",
                    "+",
                )),
        )
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let servings = self.servings_value;
        self.servings.update(cx, |input, cx| {
            input.set_controlled_value(Some(servings), window, cx)
        });
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
                    .child(field(&self.quantity, "Quantity", cx))
                    .child(field(&self.guests, "Guests", cx))
                    .child(field(&self.hours, "Hours", cx))
                    .child(field(&self.price, "Price", cx))
                    .child(field(&self.discount, "Discount", cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_4()
                    .child(field(&self.preis, "Preis", cx))
                    .child(field(&self.font_size, "Font size", cx))
                    .child(field(&self.servings, "Servings", cx))
                    .child(field(&self.seats, "Seats", cx))
                    .child(field(&self.sold_out, "Sold out", cx)),
            )
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
