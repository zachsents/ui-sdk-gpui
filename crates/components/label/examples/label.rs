//! Labels: one that names a switch beside it by its id, and one that holds
//! its switch.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_label::{Label, LabelOptions};
use ui_sdk_switch::{Switch, SwitchOptions, SwitchState};

struct Example {
    updates_label: Entity<Label>,
    updates: Entity<Switch>,
    terms_label: Entity<Label>,
    terms: Entity<Switch>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let updates_label = cx.new(|cx| {
            Label::new(
                LabelOptions::default()
                    .for_id("updates")
                    .text("Email me product updates"),
                window,
                cx,
            )
        });
        let updates = cx.new(|cx| Switch::new(SwitchOptions::default().id("updates"), window, cx));
        let terms_label = cx.new(|cx| {
            Label::new(
                LabelOptions::default().text("Accept terms and conditions"),
                window,
                cx,
            )
        });
        let terms = cx.new(|cx| Switch::new(SwitchOptions::default(), window, cx));
        Self {
            updates_label,
            updates,
            terms_label,
            terms,
        }
    }
}

fn switch(entity: &Entity<Switch>, cx: &App) -> impl IntoElement + use<> {
    let thumb = Switch::thumb(entity, cx)
        .size(px(16.))
        .rounded_full()
        .bg(rgb(0xffffff));
    Switch::root(entity, cx)
        .with(|el, state: &SwitchState| {
            let el = el.bg(if state.checked { rgb(0x2563eb) } else { rgb(0x9ca3af) });
            if state.checked { el.justify_end() } else { el }
        })
        .flex()
        .w(px(36.))
        .p(px(2.))
        .rounded_full()
        .child(thumb)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Label::root(&self.updates_label, cx).child("Email me product updates"))
                    .child(switch(&self.updates, cx)),
            )
            .child(
                Label::root(&self.terms_label, cx)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(switch(&self.terms, cx))
                    .child("Accept terms and conditions"),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(360.), px(140.)), cx);
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
