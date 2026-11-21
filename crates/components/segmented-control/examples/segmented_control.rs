//! Segmented controls: one with a default, one vertical, one controlled,
//! one with a disabled item, and one disabled.

use gpui::{
    App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb,
    size, transparent_black,
};
use ui_sdk_segmented_control::{
    SegmentedControl, SegmentedControlItemProps, SegmentedControlOptions, SegmentedControlOrientation,
};

struct Control {
    entity: Entity<SegmentedControl>,
    title: &'static str,
    items: Vec<(&'static str, &'static str, bool)>,
}

struct Example {
    view: Control,
    position: Control,
    range: Control,
    billing: Control,
    theme: Control,
    range_value: Option<SharedString>,
}

fn items(
    list: &[(&'static str, &'static str)],
    disabled: &[&str],
) -> Vec<(&'static str, &'static str, bool)> {
    list.iter()
        .map(|&(value, text)| (value, text, disabled.contains(&value)))
        .collect()
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = Control {
            entity: cx.new(|cx| {
                SegmentedControl::new(
                    SegmentedControlOptions::default().default_value(Some("list")),
                    window,
                    cx,
                )
            }),
            title: "View",
            items: items(&[("list", "List"), ("grid", "Grid"), ("board", "Board")], &[]),
        };
        let position = Control {
            entity: cx.new(|cx| {
                SegmentedControl::new(
                    SegmentedControlOptions::default()
                        .default_value(Some("top"))
                        .orientation(SegmentedControlOrientation::Vertical),
                    window,
                    cx,
                )
            }),
            title: "Position",
            items: items(&[("top", "Top"), ("middle", "Middle"), ("bottom", "Bottom")], &[]),
        };
        let this = cx.entity().downgrade();
        let range = Control {
            entity: cx.new(|cx| {
                SegmentedControl::new(
                    SegmentedControlOptions::default()
                        .value(Some("week"))
                        .on_value_change(move |value, _details, _window, cx| {
                            let value = value.clone();
                            this.update(cx, |example, cx| {
                                example.range_value = value;
                                cx.notify();
                            })
                            .ok();
                        }),
                    window,
                    cx,
                )
            }),
            title: "Range",
            items: items(&[("day", "Day"), ("week", "Week"), ("month", "Month")], &[]),
        };
        let billing = Control {
            entity: cx.new(|cx| {
                SegmentedControl::new(
                    SegmentedControlOptions::default()
                        .default_value(Some("monthly"))
                        .name("billing"),
                    window,
                    cx,
                )
            }),
            title: "Billing",
            items: items(
                &[
                    ("monthly", "Monthly"),
                    ("yearly", "Yearly"),
                    ("lifetime", "Lifetime"),
                ],
                &["lifetime"],
            ),
        };
        let theme = Control {
            entity: cx.new(|cx| {
                SegmentedControl::new(
                    SegmentedControlOptions::default()
                        .default_value(Some("system"))
                        .disabled(true),
                    window,
                    cx,
                )
            }),
            title: "Theme",
            items: items(&[("light", "Light"), ("dark", "Dark"), ("system", "System")], &[]),
        };
        Self {
            view,
            position,
            range,
            billing,
            theme,
            range_value: Some("week".into()),
        }
    }
}

fn control(control: &Control, cx: &App) -> impl IntoElement {
    let entity = &control.entity;
    let vertical = entity.read(cx).orientation() == SegmentedControlOrientation::Vertical;
    // Behind the items, over the checked one.
    let indicator = SegmentedControl::indicator(entity, cx)
        .absolute()
        .rounded_md()
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xd1d5db))
        .with(|el, state| match state.item_box {
            Some(item) => el.left(item.left).top(item.top).w(item.width).h(item.height),
            None => el.invisible(),
        });
    let items = control.items.iter().map(|&(value, text, disabled)| {
        SegmentedControl::item(
            entity,
            value,
            SegmentedControlItemProps::default().disabled(disabled),
            cx,
        )
        .aria_label(SharedString::from(text))
        .px_3()
        .py_1()
        .rounded_md()
        .border_2()
        .border_color(transparent_black())
        .text_sm()
        .with(|el, state| {
            el.text_color(if state.disabled {
                rgb(0x9ca3af)
            } else if state.checked {
                rgb(0x111827)
            } else {
                rgb(0x4b5563)
            })
        })
        .focus_visible(|style| style.border_color(rgb(0x2563eb)))
        .child(text)
    });
    let root = SegmentedControl::root(entity, cx)
        .aria_label(SharedString::from(control.title))
        .flex()
        .p_1()
        .rounded_lg()
        .bg(rgb(0xf3f4f6))
        .child(indicator)
        .children(items);
    let root = if vertical { root.flex_col() } else { root };
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap_2()
        .child(div().text_sm().child(control.title))
        .child(root)
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let range_value = self.range_value.clone();
        self.range.entity.update(cx, |control, cx| {
            control.set_controlled_value(Some(range_value), window, cx)
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
                    .child(control(&self.view, cx))
                    .child(control(&self.range, cx))
                    .child(control(&self.billing, cx))
                    .child(control(&self.theme, cx)),
            )
            .child(control(&self.position, cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(480.), px(360.)), cx);
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
