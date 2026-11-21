//! Text fields: single-line, multi-line and password.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Rgba,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_kernel::Part;
use ui_sdk_native::{TextField, TextFieldKind, TextFieldOptions, TextFieldState};

struct Example {
    name: Entity<TextField>,
    notes: Entity<TextField>,
    password: Entity<TextField>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            TextField::new(TextFieldOptions::default().placeholder("Ada Lovelace"), window, cx)
        });
        let notes = cx.new(|cx| {
            TextField::new(
                TextFieldOptions::default()
                    .multi_line(true)
                    .rows(3)
                    .auto_resize(true),
                window,
                cx,
            )
        });
        let password = cx.new(|cx| {
            TextField::new(
                TextFieldOptions::default().kind(TextFieldKind::Password),
                window,
                cx,
            )
        });
        Self {
            name,
            notes,
            password,
        }
    }
}

fn border(state: &TextFieldState) -> Rgba {
    if state.focused {
        rgb(0x2563eb)
    } else {
        rgb(0xd1d5db)
    }
}

fn labeled(label: &'static str, field: Part<TextFieldState>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().text_color(rgb(0x4b5563)).child(label))
        .child(
            field
                .with(|el, state| el.border_color(border(state)))
                .aria_label(label)
                .w(px(280.))
                .border_1()
                .rounded_md()
                .px_2()
                .py_1(),
        )
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
            .child(labeled("Name", TextField::root(&self.name, cx)))
            .child(labeled(
                "Notes",
                TextField::root(&self.notes, cx).max_h(px(120.)),
            ))
            .child(labeled("Password", TextField::root(&self.password, cx)))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(360.), px(360.)), cx);
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
