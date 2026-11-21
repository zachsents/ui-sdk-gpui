//! Waterfalls: a page's network requests with their sizes and a mark at
//! DOMContentLoaded, and a checkout trace whose spans expand, with buttons
//! that expand or collapse them all and zoom the axis.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled, Window,
    WindowBounds, WindowOptions, div, prelude::FluentBuilder, px, relative, rgb, size,
    transparent_black,
};
use ui_sdk_kernel::{ChangeOptions, Part};
use ui_sdk_number::{NumberFormat, NumberFormatOptions, NumberStyle, UnitDisplay};
use ui_sdk_waterfall::{
    Range, Waterfall, WaterfallAxisProps, WaterfallDataItem, WaterfallOptions, WaterfallRowState,
    visible_waterfall_rows,
};

fn requests() -> Vec<WaterfallDataItem> {
    vec![
        WaterfallDataItem::new("document", "/", 0., 182.),
        WaterfallDataItem::new("styles", "app.css", 190., 96.),
        WaterfallDataItem::new("script", "app.js", 192., 410.),
        WaterfallDataItem::new("font", "inter.woff2", 300., 220.),
        WaterfallDataItem::new("user", "api/user", 610., 240.),
        WaterfallDataItem::new("avatar", "avatar.png", 860., 150.),
    ]
}

const SIZES: &[(&str, &str)] = &[
    ("document", "14 kB"),
    ("styles", "8 kB"),
    ("script", "212 kB"),
    ("font", "48 kB"),
    ("user", "1 kB"),
    ("avatar", "36 kB"),
];

fn checkout() -> Vec<WaterfallDataItem> {
    vec![WaterfallDataItem::new("checkout", "POST /checkout", 0., 920.).children([
        WaterfallDataItem::new("auth", "auth.verify", 12., 64.),
        WaterfallDataItem::new("cart", "cart.load", 80., 310.).children([
            WaterfallDataItem::new("cart-db", "db.query carts", 96., 140.),
            WaterfallDataItem::new("prices", "pricing.quote", 240., 136.),
        ]),
        WaterfallDataItem::new("payment", "payments.charge", 400., 470.).children([
            WaterfallDataItem::new("fraud", "fraud.score", 410., 120.),
            WaterfallDataItem::new("gateway", "gateway.capture", 540., 310.),
        ]),
        WaterfallDataItem::new("email", "email.enqueue", 880., 30.),
    ])]
}

const FULL: Range = [0., 920.];

// The range scaled about its middle, kept inside the trace.
fn zoomed([start, end]: Range, factor: f64) -> Range {
    let center = (start + end) / 2.;
    let half = ((end - start) * factor / 2.).min(FULL[1] / 2.);
    let from = (center - half).max(FULL[0]).min(FULL[1] - half * 2.);
    [from, from + half * 2.]
}

fn trace_options(items: Rc<[WaterfallDataItem]>, range: Range) -> WaterfallOptions {
    WaterfallOptions::default()
        .items(items)
        .range(range)
        .default_expanded(["checkout"])
        .format(NumberFormat::Options(
            NumberFormatOptions::default()
                .style(NumberStyle::Unit)
                .unit("millisecond")
                .unit_display(UnitDisplay::Narrow),
        ))
}

struct Example {
    network: Entity<Waterfall>,
    sizes: HashMap<SharedString, SharedString>,
    trace: Entity<Waterfall>,
    trace_items: Rc<[WaterfallDataItem]>,
    range: Range,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sizes: HashMap<SharedString, SharedString> = SIZES
            .iter()
            .map(|(key, size)| (SharedString::from(*key), SharedString::from(*size)))
            .collect();
        let labels = sizes.clone();
        let network = cx.new(|cx| {
            Waterfall::new(
                WaterfallOptions::default()
                    .items(requests())
                    .get_bar_label(move |request, details| {
                        let size = labels.get(&request.value).cloned().unwrap_or_default();
                        format!("{size}, starts at {}, takes {}", details.start, details.duration)
                    }),
                window,
                cx,
            )
        });
        let trace_items: Rc<[WaterfallDataItem]> = checkout().into();
        let trace = cx.new(|cx| Waterfall::new(trace_options(trace_items.clone(), FULL), window, cx));
        Self {
            network,
            sizes,
            trace,
            trace_items,
            range: FULL,
        }
    }

    fn zoom(&mut self, range: Range, window: &mut Window, cx: &mut Context<Self>) {
        self.range = range;
        let options = trace_options(self.trace_items.clone(), range);
        self.trace
            .update(cx, |trace, cx| trace.set_options(options, window, cx));
    }
}

fn row(el: Stateful<Div>, state: &WaterfallRowState) -> Stateful<Div> {
    el.flex()
        .items_center()
        .gap_2()
        .h(px(28.))
        .px_2()
        .rounded_md()
        .border_1()
        .border_color(transparent_black())
        .focus_visible(|style| style.border_color(rgb(0x2563eb)))
        .when(state.selected, |el| el.bg(rgb(0xe5e7eb)))
        .when(state.level == 1, |el| el.font_weight(gpui::FontWeight::MEDIUM))
}

fn cell(el: Stateful<Div>) -> Stateful<Div> {
    el.h_full()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(transparent_black())
        .focus_visible(|style| style.border_color(rgb(0x2563eb)))
}

// The bar: the span's part of the axis, placed by the row's state.
fn bar(waterfall: &Entity<Waterfall>, value: &SharedString, cx: &App) -> impl IntoElement {
    Waterfall::bar(waterfall, value.clone(), cx).map_el(cell).flex_1().relative().with(|el, state| {
        el.child(
            div()
                .absolute()
                .top(px(8.))
                .bottom(px(8.))
                .left(relative(state.start as f32 / 100.))
                .w(relative(state.size as f32 / 100.))
                .min_w(px(2.))
                .rounded_sm()
                .bg(rgb(0x4682b4)),
        )
    })
}

fn row_header(waterfall: &Entity<Waterfall>, value: &SharedString, label: &SharedString, branch: bool, cx: &App) -> impl IntoElement {
    let indicator: AnyElement = if branch {
        Waterfall::indicator(waterfall, value.clone(), cx)
            .w(px(12.))
            .text_color(rgb(0x6b7280))
            .with(|el, state| el.child(if state.expanded { "▾" } else { "▸" }))
            .into_any_element()
    } else {
        div().w(px(12.)).into_any_element()
    };
    Waterfall::row_header(waterfall, value.clone(), cx)
        .map_el(cell)
        .w(px(180.))
        .gap_1()
        .with(|el, state| el.pl(px((state.level as f32 - 1.) * 14.)))
        .child(indicator)
        .child(label.clone())
}

fn axis(waterfall: &Entity<Waterfall>, max_ticks: usize, cx: &App) -> impl IntoElement {
    Waterfall::axis(waterfall, WaterfallAxisProps::default().max_ticks(max_ticks), cx)
        .relative()
        .size_full()
        .with(|el, state| {
            el.children(state.ticks.iter().map(|tick| {
                Waterfall::tick(waterfall, tick.time, cx).with(|el, tick| {
                    el.absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(tick.position as f32 / 100.))
                        .pl_1()
                        .border_l_1()
                        .border_color(rgb(0xe5e7eb))
                        .text_xs()
                        .text_color(rgb(0x6b7280))
                        .child(tick.label.clone())
                })
            }))
        })
}

fn header(label: &'static str, el: Part<()>) -> Part<()> {
    el.text_xs().text_color(rgb(0x6b7280)).child(label)
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
        let range = self.range;
        let controls = div()
            .flex()
            .gap_2()
            .child(button("expand", "Expand all").on_click(cx.listener(|this, _, window, cx| {
                this.trace
                    .update(cx, |trace, cx| trace.expand_all(ChangeOptions::default(), window, cx));
            })))
            .child(button("collapse", "Collapse all").on_click(cx.listener(|this, _, window, cx| {
                this.trace
                    .update(cx, |trace, cx| trace.collapse_all(ChangeOptions::default(), window, cx));
            })))
            .child(button("zoom-in", "Zoom in").on_click(cx.listener(move |this, _, window, cx| {
                this.zoom(zoomed(range, 0.5), window, cx)
            })))
            .child(button("zoom-out", "Zoom out").on_click(cx.listener(move |this, _, window, cx| {
                this.zoom(zoomed(range, 2.), window, cx)
            })))
            .child(button("reset", "Reset").on_click(cx.listener(|this, _, window, cx| this.zoom(FULL, window, cx))));
        let cx: &App = cx;

        let network = &self.network;
        let requests: Vec<WaterfallDataItem> = network.read(cx).items().to_vec();
        let network_rows = requests.iter().map(|request| {
            let value = request.value.clone();
            let size = self.sizes.get(&value).cloned().unwrap_or_default();
            Waterfall::row(network, value.clone(), cx)
                .with(row)
                .child(row_header(network, &value, &request.label, false, cx))
                .child(
                    Waterfall::cell(network, value.clone(), cx)
                        .map_el(cell)
                        .w(px(64.))
                        .text_color(rgb(0x6b7280))
                        .aria_label(size.clone())
                        .child(size),
                )
                .child(bar(network, &value, cx))
        });
        let network_view = Waterfall::root(network, cx)
            .aria_label("Network requests")
            .flex()
            .flex_col()
            .w(px(620.))
            .child(
                Waterfall::header_row(network, cx)
                    .flex()
                    .gap_2()
                    .h(px(24.))
                    .px_2()
                    .child(header("Name", Waterfall::column_header(network, cx).aria_label("Name").w(px(180.))))
                    .child(header("Size", Waterfall::column_header(network, cx).aria_label("Size").w(px(64.))))
                    .child(
                        Waterfall::column_header(network, cx)
                            .aria_label("Timeline")
                            .flex_1()
                            .h_full()
                            .relative()
                            .child(axis(network, 5, cx))
                            .child(Waterfall::mark(network, 620., cx).with(|el, mark| {
                                el.absolute()
                                    .top_0()
                                    .h(px(220.))
                                    .left(relative(mark.position as f32 / 100.))
                                    .border_l_1()
                                    .border_color(rgb(0xdc2626))
                                    .when(mark.outside, |el| el.invisible())
                            })),
                    ),
            )
            .children(network_rows);

        let trace = &self.trace;
        let expanded = trace.read(cx).get_expanded().clone();
        let trace_rows = visible_waterfall_rows(&self.trace_items, &expanded)
            .into_iter()
            .map(|shown| {
                let value = shown.item.value.clone();
                Waterfall::row(trace, value.clone(), cx)
                    .with(row)
                    .child(row_header(trace, &value, &shown.item.label, shown.item.children.is_some(), cx))
                    .child(bar(trace, &value, cx))
            });
        let trace_view = Waterfall::root(trace, cx)
            .aria_label("Checkout trace")
            .flex()
            .flex_col()
            .w(px(620.))
            .child(
                Waterfall::header_row(trace, cx)
                    .flex()
                    .gap_2()
                    .h(px(24.))
                    .px_2()
                    .child(header("Span", Waterfall::column_header(trace, cx).aria_label("Span").w(px(180.))))
                    .child(
                        Waterfall::column_header(trace, cx)
                            .aria_label("Timeline")
                            .flex_1()
                            .h_full()
                            .child(axis(trace, 6, cx)),
                    ),
            )
            .children(trace_rows);

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_6()
            .p_4()
            .bg(rgb(0xffffff))
            .text_sm()
            .text_color(rgb(0x111827))
            .child(network_view)
            .child(div().flex().flex_col().items_start().gap_2().child(controls).child(trace_view))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(700.), px(620.)), cx);
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
