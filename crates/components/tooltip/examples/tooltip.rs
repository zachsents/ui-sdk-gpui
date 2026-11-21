//! Tooltips: one that names an icon button and shows its shortcut, one that
//! describes a button with text, and a toggleable one.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, px, rgb, size, text,
};
use ui_sdk_platform::PhysicalSide;
use ui_sdk_tooltip::{
    Tooltip, TooltipArrowState, TooltipContentProps, TooltipOptions, TooltipShortcutProps,
    TooltipTriggerProps,
};

struct Example {
    bold: Entity<Tooltip>,
    share: Entity<Tooltip>,
    info: Entity<Tooltip>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let bold = cx.new(|cx| {
            Tooltip::new(
                TooltipOptions::default().text("Bold").shortcut("mod+b"),
                window,
                cx,
            )
        });
        let share = cx.new(|cx| Tooltip::new(TooltipOptions::default().text("Copy link"), window, cx));
        let info = cx.new(|cx| {
            Tooltip::new(
                TooltipOptions::default().text("Saved daily").toggleable(true),
                window,
                cx,
            )
        });
        Self { bold, share, info }
    }
}

fn arrow(el: gpui::Stateful<gpui::Div>, state: &TooltipArrowState) -> gpui::Stateful<gpui::Div> {
    let el = el.absolute().size(px(6.)).bg(rgb(0x111827));
    let offset = state.arrow.map(|arrow| arrow.offset).unwrap_or_default();
    match state.side {
        Some(PhysicalSide::Bottom) => el.top(px(-3.)).left(offset),
        Some(PhysicalSide::Top) => el.bottom(px(-3.)).left(offset),
        Some(PhysicalSide::Left) => el.right(px(-3.)).top(offset),
        Some(PhysicalSide::Right) => el.left(px(-3.)).top(offset),
        None => el.hidden(),
    }
}

fn tip<E: Styled>(content: E) -> E {
    content
        .flex()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0x111827))
        .text_color(rgb(0xffffff))
        .text_sm()
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // An icon button: the tooltip names it.
        let bold = Tooltip::trigger(&self.bold, "bold", TooltipTriggerProps::default(), cx)
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(rgb(0xf3f4f6))
            .child(div().size(px(12.)).bg(rgb(0x111827)));
        let bold_tip = tip(Tooltip::content(&self.bold, TooltipContentProps::default(), cx))
            .child(Tooltip::arrow(&self.bold, cx).with(arrow))
            .child(text!("Bold"))
            .child(
                Tooltip::shortcut(&self.bold, None, TooltipShortcutProps::default(), cx)
                    .text_color(rgb(0x9ca3af)),
            );

        // A button with text: the tooltip describes it.
        let share = Tooltip::trigger(&self.share, "share", TooltipTriggerProps::default(), cx)
            .px_3()
            .py_1()
            .rounded_md()
            .bg(rgb(0xf3f4f6))
            .child(text!("Share"));
        let share_tip = tip(Tooltip::content(&self.share, TooltipContentProps::default(), cx))
            .child(Tooltip::arrow(&self.share, cx).with(arrow))
            .child(text!("Copy link"));

        // An info button a press opens and closes, touch included.
        let info = Tooltip::trigger(&self.info, "info", TooltipTriggerProps::default(), cx)
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(rgb(0xf3f4f6))
            .child(div().size(px(4.)).rounded_full().bg(rgb(0x111827)));
        let info_tip = tip(Tooltip::content(&self.info, TooltipContentProps::default(), cx))
            .child(Tooltip::arrow(&self.info, cx).with(arrow))
            .child(text!("Saved daily"));

        div()
            .size_full()
            .flex()
            .items_start()
            .gap_2()
            .p_4()
            .pt_12()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(bold)
            .child(bold_tip)
            .child(share)
            .child(share_tip)
            .child(info)
            .child(info_tip)
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(360.), px(160.)), cx);
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
