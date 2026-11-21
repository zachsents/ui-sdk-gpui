//! Meters: storage used as a percent, which buttons change, a credit score
//! between 300 and 850, memory in gigabytes, a budget in euros in German,
//! seats as text of their own, and requests with a value label.

use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement, Render, Stateful,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_meter::{Meter, MeterOptions, NumberFormat};
use ui_sdk_number::{NumberFormatOptions, NumberStyle};

struct Example {
    storage: Entity<Meter>,
    used: f64,
    score: Entity<Meter>,
    memory: Entity<Meter>,
    budget: Entity<Meter>,
    seats: Entity<Meter>,
    requests: Entity<Meter>,
}

fn storage(used: f64) -> MeterOptions {
    MeterOptions::default().value(used)
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut meter = |options: MeterOptions| cx.new(|cx| Meter::new(options, window, cx));
        let used = 68.;
        Self {
            storage: meter(storage(used)),
            used,
            score: meter(
                MeterOptions::default()
                    .min(300.)
                    .max(850.)
                    .value(720.)
                    .format(NumberFormatOptions::default().style(NumberStyle::Decimal)),
            ),
            memory: meter(
                MeterOptions::default().max(16.).value(11.4).format(
                    NumberFormatOptions::default()
                        .style(NumberStyle::Unit)
                        .unit("gigabyte")
                        .maximum_fraction_digits(1),
                ),
            ),
            budget: meter(
                MeterOptions::default()
                    .max(2000.)
                    .value(1240.)
                    .locale("de-DE")
                    .format(
                        NumberFormatOptions::default()
                            .style(NumberStyle::Currency)
                            .currency("EUR"),
                    ),
            ),
            seats: meter(
                MeterOptions::default()
                    .max(40.)
                    .value(28.)
                    .format(NumberFormat::function(|value| format!("{value} of 40 seats"))),
            ),
            requests: meter(
                MeterOptions::default()
                    .max(1000.)
                    .value(750.)
                    .get_value_label(|value, details| {
                        format!("{value} of {} requests, {}", details.max, details.formatted)
                    }),
            ),
        }
    }

    fn set_used(&mut self, used: f64, cx: &mut Context<Self>) {
        self.used = used.clamp(0., 100.);
        let options = storage(self.used);
        self.storage.update(cx, |meter, cx| meter.set_options(options, cx));
        cx.notify();
    }
}

fn meter(entity: &Entity<Meter>, label: &'static str, cx: &App) -> impl IntoElement {
    let value = Meter::value(entity, cx);
    let text = value.state().text.clone();
    Meter::root(entity, cx)
        .flex()
        .flex_col()
        .gap_1()
        .w(px(260.))
        .child(
            div()
                .flex()
                .justify_between()
                .child(Meter::label(entity, cx).aria_label(label).child(label))
                .child(value.text_color(rgb(0x4b5563)).child(text)),
        )
        .child(
            Meter::track(entity, cx)
                .relative()
                .w_full()
                .h(px(8.))
                .rounded_full()
                .overflow_hidden()
                .bg(rgb(0xe5e7eb))
                .child(
                    Meter::indicator(entity, cx)
                        .with(|el, state| {
                            el.bg(if state.percent >= 90. {
                                rgb(0xdc2626)
                            } else {
                                rgb(0x2563eb)
                            })
                        })
                        .absolute()
                        .top_0()
                        .h_full()
                        .rounded_full(),
                ),
        )
}

fn button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .child(label)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let free = button("free", "Free up").on_click(cx.listener(|this, _, _window, cx| {
            this.set_used(this.used - 12., cx);
        }));
        let add = button("add", "Add files").on_click(cx.listener(|this, _, _window, cx| {
            this.set_used(this.used + 8., cx);
        }));
        let cx: &App = cx;
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_5()
            .p_6()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_4()
                    .child(meter(&self.storage, "Storage", cx))
                    .child(div().flex().gap_2().child(free).child(add)),
            )
            .child(meter(&self.score, "Credit score", cx))
            .child(meter(&self.memory, "Memory", cx))
            .child(meter(&self.budget, "Budget", cx))
            .child(meter(&self.seats, "Seats", cx))
            .child(meter(&self.requests, "API requests", cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(520.), px(520.)), cx);
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
