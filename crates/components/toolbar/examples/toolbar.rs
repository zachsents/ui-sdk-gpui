//! Toolbars: a row of buttons, a group, a text field and a link, with a
//! disabled item that stays focusable and one that's skipped, and a
//! vertical one.

use std::rc::Rc;

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Rgba,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_kernel::Part;
use ui_sdk_native::{TextField, TextFieldOptions};
use ui_sdk_toolbar::{
    Toolbar, ToolbarButtonProps, ToolbarButtonState, ToolbarGroupProps, ToolbarInputProps,
    ToolbarLinkProps, ToolbarOptions, ToolbarOrientation,
};

struct Example {
    format: Entity<Toolbar>,
    find: Entity<TextField>,
    tools: Entity<Toolbar>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let format = cx.new(|cx| Toolbar::new(ToolbarOptions::default(), window, cx));
        let find = cx.new(|cx| TextField::new(TextFieldOptions::default().placeholder("Find"), window, cx));
        let tools = cx.new(|cx| {
            Toolbar::new(
                ToolbarOptions::default()
                    .orientation(ToolbarOrientation::Vertical)
                    .r#loop(false),
                window,
                cx,
            )
        });
        Self { format, find, tools }
    }
}

fn shade(state: &ToolbarButtonState) -> Rgba {
    if state.disabled {
        rgb(0x9ca3af)
    } else {
        rgb(0x111827)
    }
}

fn button(
    toolbar: &Entity<Toolbar>,
    key: &'static str,
    label: &'static str,
    props: ToolbarButtonProps,
    cx: &App,
) -> Part<ToolbarButtonState> {
    Toolbar::button(toolbar, key, props, cx)
        .aria_label(label)
        .with(|el, state| el.text_color(shade(state)))
        .on_click(move |_, _, _| log::info!("{label}"))
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .child(label)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let format = &self.format;
        let clipboard = ToolbarButtonProps::default().group("clipboard");
        let field: Rc<dyn ui_sdk_text::TextField> = Rc::new(self.find.read(cx).handle());
        let find = TextField::root(&self.find, cx)
            .aria_label("Find")
            .with(move |el, state| {
                el.border_color(if state.focused { rgb(0x2563eb) } else { rgb(0xd1d5db) })
            })
            .w(px(120.))
            .border_1()
            .rounded_md()
            .px_2()
            .py_1();
        let row = Toolbar::root(format, cx)
            .aria_label("Formatting")
            .flex()
            .items_center()
            .gap_1()
            .child(button(format, "undo", "Undo", ToolbarButtonProps::default(), cx))
            .child(button(
                format,
                "redo",
                "Redo",
                ToolbarButtonProps::default().disabled(true),
                cx,
            ))
            .child(Toolbar::separator(format, "history", cx).w(px(1.)).h(px(20.)).bg(rgb(0xd1d5db)))
            .child(
                Toolbar::group(format, "clipboard", ToolbarGroupProps::default(), cx)
                    .aria_label("Clipboard")
                    .flex()
                    .gap_1()
                    .child(button(format, "cut", "Cut", clipboard.clone(), cx))
                    .child(button(format, "copy", "Copy", clipboard.clone(), cx))
                    .child(button(
                        format,
                        "paste",
                        "Paste",
                        clipboard.disabled(true).focusable_when_disabled(false),
                        cx,
                    )),
            )
            .child(Toolbar::separator(format, "edit", cx).w(px(1.)).h(px(20.)).bg(rgb(0xd1d5db)))
            .child(Toolbar::input(format, "find", ToolbarInputProps::default(), field, find, cx))
            .child(
                Toolbar::link(format, "help", ToolbarLinkProps::default(), cx)
                    .aria_label("Help")
                    .on_click(|_, _, _| log::info!("opened help"))
                    .text_color(rgb(0x2563eb))
                    .px_2()
                    .child("Help"),
            );
        let tools = &self.tools;
        let column = Toolbar::root(tools, cx)
            .aria_label("Tools")
            .flex()
            .flex_col()
            .gap_1()
            .w(px(96.))
            .child(button(tools, "select", "Select", ToolbarButtonProps::default(), cx))
            .child(button(tools, "pen", "Pen", ToolbarButtonProps::default(), cx))
            .child(button(tools, "eraser", "Eraser", ToolbarButtonProps::default(), cx))
            .child(Toolbar::separator(tools, "actions", cx).h(px(1.)).w_full().bg(rgb(0xd1d5db)))
            .child(button(tools, "delete", "Delete", ToolbarButtonProps::default(), cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(row)
            .child(column)
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(560.), px(260.)), cx);
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
