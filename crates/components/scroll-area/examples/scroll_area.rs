//! Scroll areas: a list whose scrollbar shows on hover, a row that scrolls
//! sideways, a table that scrolls both ways with a corner, the four
//! scrollbar types side by side, and a chat that sticks to its end.

use gpui::{
    App, AppContext, Bounds, Context, Div, ElementId, Entity, InteractiveElement, IntoElement,
    Orientation, ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_kernel::Part;
use ui_sdk_kernel::roles::Role;
use ui_sdk_scroll_area::{
    ScrollArea, ScrollAreaOptions, ScrollAreaScrollbar, ScrollAreaState, ScrollAreaType,
};

const COLORS: [(&str, u32); 8] = [
    ("Sand", 0xe7d8b8),
    ("Clay", 0xc98b6b),
    ("Rust", 0xa3542f),
    ("Moss", 0x7a8b4f),
    ("Sage", 0xa8b89a),
    ("Sky", 0x8fb8d8),
    ("Slate", 0x5c6b7a),
    ("Ink", 0x1f2937),
];

const RELEASES: [[&str; 6]; 8] = [
    ["v1.12.0", "Oct 28, 2026", "Latest", "18,204", "4.3 MB", "20+"],
    ["v1.11.2", "Oct 14, 2026", "Stable", "26,931", "4.2 MB", "20+"],
    ["v1.11.1", "Oct 2, 2026", "Stable", "9,412", "4.2 MB", "20+"],
    ["v1.11.0", "Sep 23, 2026", "Stable", "31,077", "4.2 MB", "20+"],
    ["v1.10.3", "Sep 9, 2026", "Stable", "12,640", "4.1 MB", "18+"],
    ["v1.10.2", "Aug 27, 2026", "Stable", "8,315", "4.1 MB", "18+"],
    ["v1.10.1", "Aug 18, 2026", "Stable", "5,902", "4.1 MB", "18+"],
    ["v1.9.4", "Jul 21, 2026", "Deprecated", "3,118", "3.9 MB", "18+"],
];

struct Example {
    tags: Entity<ScrollArea>,
    palette: Entity<ScrollArea>,
    releases: Entity<ScrollArea>,
    types: Vec<(&'static str, Entity<ScrollArea>)>,
    chat: Entity<ScrollArea>,
    messages: Vec<SharedString>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut area = |options: ScrollAreaOptions| cx.new(|cx| ScrollArea::new(options, window, cx));
        let tags = area(ScrollAreaOptions::default());
        let palette = area(ScrollAreaOptions::default().r#type(ScrollAreaType::Auto));
        let releases = area(ScrollAreaOptions::default().r#type(ScrollAreaType::Auto));
        let types = [
            ("Auto", ScrollAreaType::Auto),
            ("Always", ScrollAreaType::Always),
            ("Scroll", ScrollAreaType::Scroll),
            ("Hover", ScrollAreaType::Hover),
        ]
        .map(|(label, r#type)| (label, area(ScrollAreaOptions::default().r#type(r#type))))
        .to_vec();
        let chat = area(ScrollAreaOptions::default().stick_to_end(true));
        Self {
            tags,
            palette,
            releases,
            types,
            chat,
            messages: (1..=12).map(|n| SharedString::from(format!("Message {n}"))).collect(),
        }
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let n = self.messages.len() + 1;
        self.messages.push(format!("Message {n}").into());
        cx.notify();
    }
}

fn scrollbar(area: &Entity<ScrollArea>, orientation: Orientation, cx: &App) -> ScrollAreaScrollbar {
    let thumb = ScrollArea::thumb(area, orientation, cx)
        .rounded_full()
        .with(|el, state| el.bg(if state.dragging { rgb(0x4b5563) } else { rgb(0x9ca3af) }));
    let bar = ScrollArea::scrollbar(area, orientation, cx)
        .p(px(2.))
        .with(|el, state| el.opacity(if state.visible { 1. } else { 0. }));
    match orientation {
        Orientation::Vertical => bar.w(px(10.)).child(thumb.w_full()),
        Orientation::Horizontal => bar.h(px(10.)).child(thumb.h_full()),
    }
}

fn frame(area: &Entity<ScrollArea>, width: f32, height: f32, cx: &App) -> Part<ScrollAreaState> {
    ScrollArea::root(area, cx)
        .w(px(width))
        .h(px(height))
        .rounded_md()
        .overflow_hidden()
        .bg(rgb(0xf9fafb))
        .border_1()
        .border_color(rgb(0xe5e7eb))
}

fn viewport(area: &Entity<ScrollArea>, label: &'static str, cx: &App) -> Part<ScrollAreaState> {
    ScrollArea::viewport(area, cx)
        .aria_role(Role::Region)
        .aria_label(label)
        .size_full()
}

fn button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .child(label)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let add = button("add", "Add").on_click(cx.listener(|this, _, _window, cx| this.add(cx)));
        let messages = self.messages.clone();
        let cx: &App = cx;

        let tags = frame(&self.tags, 200., 220., cx)
            .child(viewport(&self.tags, "Tags", cx).child(
                ScrollArea::content(&self.tags, cx)
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children((1..=50).map(|n| format!("v1.2.0-beta.{}", 51 - n))),
            ))
            .child(scrollbar(&self.tags, Orientation::Vertical, cx));

        let palette = frame(&self.palette, 320., 132., cx)
            .child(viewport(&self.palette, "Palette", cx).child(
                ScrollArea::content(&self.palette, cx).p_3().flex().gap_3().children(COLORS.map(
                    |(name, color)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().size(px(72.)).rounded_md().bg(rgb(color)))
                            .child(name)
                    },
                )),
            ))
            .child(scrollbar(&self.palette, Orientation::Horizontal, cx));

        let cell = |text: &'static str| div().w(px(112.)).flex_none().px_2().py_1().child(text);
        let releases = frame(&self.releases, 320., 200., cx)
            .child(viewport(&self.releases, "Releases", cx).child(
                ScrollArea::content(&self.releases, cx)
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .bg(rgb(0xf3f4f6))
                            .children(["Version", "Date", "Status", "Downloads", "Size", "Node"].map(cell)),
                    )
                    .children(RELEASES.map(|row| div().flex().children(row.map(cell)))),
            ))
            .child(scrollbar(&self.releases, Orientation::Vertical, cx))
            .child(scrollbar(&self.releases, Orientation::Horizontal, cx))
            .child(ScrollArea::corner(&self.releases, cx));

        let types = self.types.iter().map(|(label, area)| {
            div().flex().flex_col().gap_1().child(*label).child(
                frame(area, 120., 140., cx)
                    .child(viewport(area, label, cx).child(
                        ScrollArea::content(area, cx)
                            .p_2()
                            .flex()
                            .flex_col()
                            .children((1..=20).map(|n| format!("Item {n}"))),
                    ))
                    .child(scrollbar(area, Orientation::Vertical, cx)),
            )
        });

        let latest = ScrollArea::scroll_to_end_part(&self.chat, cx)
            .aria_label("Latest")
            .absolute()
            .bottom_2()
            .right_4()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x1f2937))
            .text_color(rgb(0xffffff))
            .with(|el, state| el.opacity(if state.visible { 1. } else { 0. }))
            .child("Latest");
        let chat = frame(&self.chat, 240., 220., cx)
            .child(viewport(&self.chat, "Chat", cx).child(
                ScrollArea::content(&self.chat, cx)
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(messages.into_iter().map(|message| {
                        div().px_2().py_1().rounded_md().bg(rgb(0xe5e7eb)).child(message)
                    })),
            ))
            .child(scrollbar(&self.chat, Orientation::Vertical, cx))
            .child(latest);

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_6()
            .p_6()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(div().flex().items_start().gap_6().child(tags).child(palette).child(releases))
            .child(div().flex().items_start().gap_4().children(types))
            .child(div().flex().items_start().gap_4().child(chat).child(add))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(980.), px(720.)), cx);
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
