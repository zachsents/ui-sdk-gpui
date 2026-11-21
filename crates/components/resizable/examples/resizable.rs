//! Resizable groups: a collapsible sidebar beside a group nested in a panel,
//! three panels with a fixed handle, and one kept between pixel bounds and
//! saved.

use gpui::{
    App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb,
    size,
};
use ui_sdk_kernel::Part;
use ui_sdk_resizable::{
    PanelSizes, Resizable, ResizableHandleProps, ResizableHandleState, ResizableOptions,
    ResizableOrientation, ResizablePanelState, ResizableSize,
};

struct Example {
    editor: Entity<Resizable>,
    code: Entity<Resizable>,
    mail: Entity<Resizable>,
    docs: Entity<Resizable>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| Resizable::new(ResizableOptions::default(), window, cx));
        let code = cx.new(|cx| {
            Resizable::new(
                ResizableOptions::default().orientation(ResizableOrientation::Vertical),
                window,
                cx,
            )
        });
        let mail = cx.new(|cx| {
            Resizable::new(
                ResizableOptions::default().default_layout(vec![25.0, 35.0, 40.0]),
                window,
                cx,
            )
        });
        let docs = cx.new(|cx| Resizable::new(ResizableOptions::default().auto_save_id("docs"), window, cx));
        Self {
            editor,
            code,
            mail,
            docs,
        }
    }
}

fn panel(
    group: &Entity<Resizable>,
    id: &'static str,
    props: PanelSizes,
    text: &'static str,
    cx: &App,
) -> Part<ResizablePanelState> {
    Resizable::panel(group, id, props, cx)
        .overflow_hidden()
        .with(|el, state| el.p(if state.collapsed { px(0.) } else { px(8.) }))
        .child(text)
}

fn handle(
    group: &Entity<Resizable>,
    key: &'static str,
    name: &'static str,
    props: ResizableHandleProps,
    cx: &App,
) -> Part<ResizableHandleState> {
    Resizable::handle(group, key, props, cx)
        .aria_label(SharedString::from(name))
        .with(|el, state| {
            let el = if state.orientation == ResizableOrientation::Horizontal {
                el.w(px(1.))
            } else {
                el.h(px(1.))
            };
            el.bg(if state.disabled {
                rgb(0xe5e7eb)
            } else if state.dragging || state.hovering {
                rgb(0x2563eb)
            } else {
                rgb(0xd1d5db)
            })
        })
        .focus_visible(|style| style.bg(rgb(0x2563eb)))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cx: &App = cx;
        let code = Resizable::root(&self.code, cx)
            .size_full()
            .child(panel(
                &self.code,
                "page",
                PanelSizes::default().default_size(70.0),
                "page.tsx",
                cx,
            ))
            .child(handle(
                &self.code,
                "terminal",
                "Resize terminal",
                ResizableHandleProps::default(),
                cx,
            ))
            .child(panel(
                &self.code,
                "terminal",
                PanelSizes::default().min_size(15.0),
                "Terminal",
                cx,
            ));
        let editor = Resizable::root(&self.editor, cx)
            .w(px(560.))
            .h(px(200.))
            .border_1()
            .border_color(rgb(0xd1d5db))
            .child(panel(
                &self.editor,
                "files",
                PanelSizes::default()
                    .default_size(25.0)
                    .min_size(15.0)
                    .collapsible(true),
                "Files",
                cx,
            ))
            .child(handle(
                &self.editor,
                "files",
                "Resize files",
                ResizableHandleProps::default(),
                cx,
            ))
            .child(Resizable::panel(&self.editor, "main", PanelSizes::default(), cx).child(code));
        let mail = Resizable::root(&self.mail, cx)
            .w(px(560.))
            .h(px(80.))
            .border_1()
            .border_color(rgb(0xd1d5db))
            .child(panel(&self.mail, "pinned", PanelSizes::default(), "Pinned", cx))
            .child(handle(
                &self.mail,
                "pinned",
                "Resize pinned",
                ResizableHandleProps::default().disabled(true),
                cx,
            ))
            .child(panel(
                &self.mail,
                "inbox",
                PanelSizes::default().min_size(10.0),
                "Inbox",
                cx,
            ))
            .child(handle(
                &self.mail,
                "inbox",
                "Resize inbox",
                ResizableHandleProps::default(),
                cx,
            ))
            .child(panel(
                &self.mail,
                "message",
                PanelSizes::default().min_size(10.0),
                "Message",
                cx,
            ));
        let docs = Resizable::root(&self.docs, cx)
            .w(px(560.))
            .h(px(80.))
            .border_1()
            .border_color(rgb(0xd1d5db))
            .child(panel(
                &self.docs,
                "docs",
                PanelSizes::default()
                    .default_size(ResizableSize::px(200.0))
                    .min_size(ResizableSize::px(160.0))
                    .max_size(ResizableSize::px(320.0)),
                "Docs",
                cx,
            ))
            .child(handle(
                &self.docs,
                "docs",
                "Resize docs",
                ResizableHandleProps::default(),
                cx,
            ))
            .child(panel(&self.docs, "page", PanelSizes::default(), "Page", cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(editor)
            .child(mail)
            .child(docs)
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(600.), px(480.)), cx);
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
