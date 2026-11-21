//! Switches: uncontrolled, controlled, disabled and read-only, in a form that
//! Enter submits.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_forms::{Form, FormOptions, Label, LabelOptions};
use ui_sdk_switch::{Switch, SwitchOptions, SwitchState};

struct Example {
    airplane: Entity<Switch>,
    wifi: Entity<Switch>,
    alerts: Entity<Switch>,
    backups: Entity<Switch>,
    wifi_on: bool,
    form: Form,
    labels: Vec<Label>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let mut switch = |options: SwitchOptions, cx: &mut Context<Self>| {
            cx.new(|cx| Switch::new(options, window, cx))
        };
        let airplane = switch(SwitchOptions::default().name("airplane"), cx);
        let wifi = switch(
            SwitchOptions::default()
                .checked(true)
                .name("wifi")
                .on_checked_change(move |checked, details, _window, cx| {
                    log::info!("wifi {checked} ({})", details.reason.as_str());
                    let checked = *checked;
                    this.update(cx, |example, cx| {
                        example.wifi_on = checked;
                        cx.notify();
                    })
                    .ok();
                }),
            cx,
        );
        let alerts = switch(
            SwitchOptions::default()
                .default_checked(true)
                .disabled(true)
                .name("alerts"),
            cx,
        );
        let backups = switch(
            SwitchOptions::default()
                .default_checked(true)
                .read_only(true)
                .name("backups"),
            cx,
        );
        let form = Form::new(FormOptions::default().on_submit(|data, _details, _window, _cx| {
            log::info!("submitted {}", data.to_url_encoded());
        }));
        let labels = ["Airplane mode", "Wi-Fi", "Security alerts", "Encrypt backups"]
            .into_iter()
            .map(|text| Label::new(LabelOptions::default().text(text)))
            .collect();
        Self {
            airplane,
            wifi,
            alerts,
            backups,
            wifi_on: true,
            form,
            labels,
        }
    }
}

fn track(state: &SwitchState) -> gpui::Rgba {
    match (state.checked, state.disabled) {
        (_, true) => rgb(0xd1d5db),
        (true, false) => rgb(0x2563eb),
        (false, false) => rgb(0x9ca3af),
    }
}

fn row(label: &Label, switch: &Entity<Switch>, text: &'static str, cx: &App) -> impl IntoElement {
    let thumb = Switch::thumb(switch, cx)
        .size(px(16.))
        .rounded_full()
        .bg(rgb(0xffffff));
    label
        .root()
        .flex()
        .items_center()
        .gap_2()
        .child(
            Switch::root(switch, cx)
                .aria_label(SharedString::from(text))
                .with(|el, state| {
                    el.bg(track(state))
                        .when(state.checked, |el| el.justify_end())
                })
                .flex()
                .w(px(36.))
                .p(px(2.))
                .rounded_full()
                .child(thumb),
        )
        .child(text)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wifi_on = self.wifi_on;
        self.wifi
            .update(cx, |switch, cx| switch.set_controlled_checked(Some(wifi_on), cx));
        let switches = [&self.airplane, &self.wifi, &self.alerts, &self.backups];
        let texts = ["Airplane mode", "Wi-Fi", "Security alerts", "Encrypt backups"];
        let mut form = self
            .form
            .root()
            .flex()
            .flex_col()
            .gap_3();
        for ((label, switch), text) in self.labels.iter().zip(switches).zip(texts) {
            form = form.child(row(label, switch, text, cx));
        }
        div()
            .size_full()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(form)
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(320.), px(220.)), cx);
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
