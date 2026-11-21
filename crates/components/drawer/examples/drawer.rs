//! Drawers: one at its own size, one with snap points and a handle, and one
//! from the right edge.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, relative,
    rgb, rgba, size, text,
};
use ui_sdk_dialog::{DialogCloseProps, DialogCloseState, DialogTriggerProps, DialogTriggerState};
use ui_sdk_drawer::{Drawer, DrawerContentProps, DrawerOptions, DrawerSide, SnapPoint};
use ui_sdk_kernel::Part;

struct Example {
    filters: Entity<Drawer>,
    directions: Entity<Drawer>,
    settings: Entity<Drawer>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filters = cx.new(|cx| Drawer::new(DrawerOptions::default(), window, cx));
        let directions = cx.new(|cx| {
            Drawer::new(
                DrawerOptions::default().snap_points([SnapPoint::Number(0.4), SnapPoint::Number(0.9)]),
                window,
                cx,
            )
        });
        let settings = cx.new(|cx| Drawer::new(DrawerOptions::default().side(DrawerSide::Right), window, cx));
        Self {
            filters,
            directions,
            settings,
        }
    }
}

fn trigger(part: Part<DialogTriggerState>, label: &'static str) -> impl IntoElement {
    part.with(|el, state| el.bg(if state.open { rgb(0xe5e7eb) } else { rgb(0xf3f4f6) }))
        .px_3()
        .py_1()
        .rounded_md()
        .child(text!(label))
}

fn button(part: Part<DialogCloseState>, label: &'static str) -> impl IntoElement {
    part.px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(0x1f2937))
        .text_color(rgb(0xffffff))
        .child(text!(label))
}

fn handle(drawer: &Entity<Drawer>, cx: &App) -> impl IntoElement {
    Drawer::handle(drawer, cx)
        .aria_label("Resize")
        .self_center()
        .w(px(40.))
        .h(px(5.))
        .rounded_full()
        .bg(rgb(0xd1d5db))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filters_backdrop = Drawer::backdrop(&self.filters, None, cx).map(|backdrop| backdrop.bg(rgba(0x0000004d)));
        let filters = Drawer::viewport(&self.filters, cx).children(
            Drawer::content(&self.filters, DrawerContentProps::default(), cx).map(|content| {
                content
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .rounded_t_lg()
                    .bg(rgb(0xffffff))
                    .text_color(rgb(0x111827))
                    .child(handle(&self.filters, cx))
                    .child(Drawer::title(&self.filters, cx).aria_label("Filters").child("Filters"))
                    .child(Drawer::description(&self.filters, cx).aria_label("128 results").child("128 results"))
                    .child(button(
                        Drawer::close_part(&self.filters, "done", DialogCloseProps::default(), cx),
                        "Done",
                    ))
            }),
        );

        let directions_backdrop =
            Drawer::backdrop(&self.directions, None, cx).map(|backdrop| backdrop.bg(rgba(0x0000004d)));
        let directions = Drawer::viewport(&self.directions, cx).children(
            Drawer::content(&self.directions, DrawerContentProps::default(), cx).map(|content| {
                content
                    .w_full()
                    .h(relative(0.9))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .rounded_t_lg()
                    .bg(rgb(0xffffff))
                    .text_color(rgb(0x111827))
                    .child(handle(&self.directions, cx))
                    .child(Drawer::title(&self.directions, cx).aria_label("Directions").child("Directions"))
                    .child(Drawer::description(&self.directions, cx).aria_label("12 min").child("12 min"))
                    .child(button(
                        Drawer::close_part(&self.directions, "end", DialogCloseProps::default(), cx),
                        "End",
                    ))
            }),
        );

        let settings_backdrop =
            Drawer::backdrop(&self.settings, None, cx).map(|backdrop| backdrop.bg(rgba(0x0000004d)));
        let settings = Drawer::viewport(&self.settings, cx).children(
            Drawer::content(&self.settings, DrawerContentProps::default(), cx).map(|content| {
                content
                    .h_full()
                    .w(px(280.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .bg(rgb(0xffffff))
                    .text_color(rgb(0x111827))
                    .child(Drawer::title(&self.settings, cx).aria_label("Settings").child("Settings"))
                    .child(button(
                        Drawer::close_part(&self.settings, "close", DialogCloseProps::default(), cx),
                        "Close",
                    ))
            }),
        );

        div()
            .size_full()
            .flex()
            .items_start()
            .gap_2()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(trigger(
                Drawer::trigger(&self.filters, "filters", DialogTriggerProps::default(), cx),
                "Filters",
            ))
            .children(filters_backdrop)
            .child(filters)
            .child(trigger(
                Drawer::trigger(&self.directions, "directions", DialogTriggerProps::default(), cx),
                "Directions",
            ))
            .children(directions_backdrop)
            .child(directions)
            .child(trigger(
                Drawer::trigger(&self.settings, "settings", DialogTriggerProps::default(), cx),
                "Settings",
            ))
            .children(settings_backdrop)
            .child(settings)
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(640.), px(480.)), cx);
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
