//! A toggle, uncontrolled, controlled and disabled.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_toggle::{Toggle, ToggleOptions, ToggleState};

struct Example {
    bold: Entity<Toggle>,
    italic: Entity<Toggle>,
    pin: Entity<Toggle>,
    italic_pressed: bool,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let bold =
            cx.new(|cx| Toggle::new(ToggleOptions::default().default_pressed(true), window, cx));
        let this = cx.entity().downgrade();
        let italic = cx.new(|cx| {
            Toggle::new(
                ToggleOptions::default()
                    .pressed(false)
                    .on_pressed_change(move |pressed, _details, _window, cx| {
                        let pressed = *pressed;
                        this.update(cx, |example, cx| {
                            example.italic_pressed = pressed;
                            cx.notify();
                        })
                        .ok();
                    }),
                window,
                cx,
            )
        });
        let pin = cx.new(|cx| {
            Toggle::new(
                ToggleOptions::default()
                    .default_pressed(true)
                    .disabled(true)
                    .focusable_when_disabled(true),
                window,
                cx,
            )
        });
        Self {
            bold,
            italic,
            pin,
            italic_pressed: false,
        }
    }
}

fn button(state: &ToggleState) -> gpui::Rgba {
    match (state.pressed, state.disabled) {
        (_, true) => rgb(0xe5e5e5),
        (true, false) => rgb(0x1f2937),
        (false, false) => rgb(0xf3f4f6),
    }
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let italic_pressed = self.italic_pressed;
        self.italic.update(cx, |toggle, cx| {
            toggle.set_controlled_pressed(Some(italic_pressed), cx)
        });
        let toggle = |entity: &Entity<Toggle>, label: &'static str, cx: &App| {
            Toggle::root(entity, cx)
                .aria_label(SharedString::from(label))
                .with(|el, state| {
                    el.bg(button(state)).text_color(if state.pressed && !state.disabled {
                        rgb(0xffffff)
                    } else {
                        rgb(0x111827)
                    })
                })
                .px_3()
                .py_1()
                .rounded_md()
                .child(label)
        };
        div()
            .size_full()
            .flex()
            .items_start()
            .gap_2()
            .p_4()
            .bg(rgb(0xffffff))
            .child(toggle(&self.bold, "Bold", cx))
            .child(toggle(&self.italic, "Italic", cx))
            .child(toggle(&self.pin, "Pin", cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(320.), px(120.)), cx);
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
