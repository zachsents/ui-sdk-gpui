//! A switch with a label: the README's quick start.

use gpui::prelude::*;
use gpui::{App, Bounds, Context, Entity, Window, WindowBounds, WindowOptions, div, px, rgb, size};
use ui_sdk::components::label::{Label, LabelOptions};
use ui_sdk::components::switch::{Switch, SwitchOptions};
use ui_sdk::foundation::platform::live_regions;

struct Settings {
    wifi: Entity<Switch>,
    wifi_label: Entity<Label>,
}

impl Settings {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let wifi = cx.new(|cx| {
            Switch::new(
                SwitchOptions::default()
                    .id("wifi")
                    .default_checked(true)
                    .on_checked_change(|checked, details, _window, _cx| {
                        println!("Wi-Fi {checked} ({})", details.reason.as_str());
                    }),
                window,
                cx,
            )
        });
        let wifi_label = cx.new(|cx| Label::new(LabelOptions::default().for_id("wifi").text("Wi-Fi"), window, cx));
        Self { wifi, wifi_label }
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let thumb = Switch::thumb(&self.wifi, cx)
            .size(px(16.))
            .rounded_full()
            .bg(rgb(0xffffff));
        let switch = Switch::root(&self.wifi, cx)
            .with(|el, state| {
                el.bg(if state.checked { rgb(0x2563eb) } else { rgb(0x9ca3af) })
                    .when(state.checked, |el| el.justify_end())
            })
            .flex()
            .w(px(36.))
            .p(px(2.))
            .rounded_full()
            .child(thumb);
        div()
            .size_full()
            .flex()
            .items_center()
            .gap_2()
            .p_4()
            .bg(rgb(0xffffff))
            .child(switch)
            .child(Label::root(&self.wifi_label, cx).child("Wi-Fi"))
            .child(live_regions(window))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        ui_sdk::init("ui-sdk-quickstart", cx);
        let bounds = Bounds::centered(None, size(px(320.), px(120.)), cx);
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
