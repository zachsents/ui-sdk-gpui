//! Collapsibles: animated, kept mounted, controlled and disabled.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, ease_in_out, px,
    rgb, size,
};
use ui_sdk_collapsible::{
    Collapsible, CollapsibleOptions, CollapsiblePanelProps, CollapsiblePanelState,
};
use ui_sdk_motion::{PresenceTiming, Timing};

struct Example {
    order: Entity<Collapsible>,
    note: Entity<Collapsible>,
    codes: Entity<Collapsible>,
    gift: Entity<Collapsible>,
    codes_open: bool,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let timing = PresenceTiming::new(Timing::ms(200).easing(ease_in_out));
        let order = cx.new(|cx| {
            Collapsible::new(CollapsibleOptions::default().panel_timing(timing.clone()), window, cx)
        });
        let note = cx.new(|cx| {
            Collapsible::new(
                CollapsibleOptions::default().default_open(true).panel_timing(timing.clone()),
                window,
                cx,
            )
        });
        let this = cx.entity().downgrade();
        let codes = cx.new(|cx| {
            Collapsible::new(
                CollapsibleOptions::default()
                    .open(false)
                    .on_open_change(move |open, _details, _window, cx| {
                        let open = *open;
                        this.update(cx, |example, cx| {
                            example.codes_open = open;
                            cx.notify();
                        })
                        .ok();
                    }),
                window,
                cx,
            )
        });
        let gift = cx.new(|cx| Collapsible::new(CollapsibleOptions::default().disabled(true), window, cx));
        Self {
            order,
            note,
            codes,
            gift,
            codes_open: false,
        }
    }
}

fn panel_style(el: gpui::Stateful<gpui::Div>, state: &CollapsiblePanelState) -> gpui::Stateful<gpui::Div> {
    let el = el.px_2().opacity(state.panel.presence.progress);
    match state.panel.height {
        Some(height) => el.h(height).overflow_hidden(),
        None => el,
    }
}

fn section(
    entity: &Entity<Collapsible>,
    title: &'static str,
    body: &'static str,
    props: CollapsiblePanelProps,
    cx: &App,
) -> impl IntoElement {
    Collapsible::root(entity, cx)
        .flex()
        .flex_col()
        .gap_1()
        .child(
            Collapsible::trigger(entity, cx)
                .aria_label(SharedString::from(title))
                .with(|el, state| {
                    el.bg(if state.open { rgb(0xe5e7eb) } else { rgb(0xf3f4f6) })
                        .text_color(if state.disabled { rgb(0x9ca3af) } else { rgb(0x111827) })
                })
                .px_2()
                .py_1()
                .rounded_md()
                .child(title),
        )
        .children(Collapsible::panel(entity, props, cx).map(|panel| panel.with(panel_style).child(body)))
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let codes_open = self.codes_open;
        self.codes.update(cx, |codes, cx| codes.set_controlled_open(Some(codes_open), window, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(section(&self.order, "Order #4189", "2 × Linen shirt", CollapsiblePanelProps::default(), cx))
            .child(section(
                &self.note,
                "Delivery note",
                "Leave at the door",
                CollapsiblePanelProps::default().keep_mounted(true),
                cx,
            ))
            .child(section(&self.codes, "Recovery codes", "4F9K-22QX", CollapsiblePanelProps::default(), cx))
            .child(section(
                &self.gift,
                "Gift options",
                "Gift message and wrapping",
                CollapsiblePanelProps::default(),
                cx,
            ))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(360.), px(320.)), cx);
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
