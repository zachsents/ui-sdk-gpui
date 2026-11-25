//! Selectable text: two paragraphs and a line of Japanese sharing one
//! selection, copied with Command-C.

use gpui::{
    App, AppContext, Bounds, ClipboardItem, Context, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Point, Render, Styled, Window, WindowBounds, WindowOptions, div, px, rgb,
    size,
};
use ui_sdk_native::{Join, SelectableText, TextSelection};

const PARAGRAPHS: [&str; 3] = [
    "Double-click a word and drag to select by words; triple-click and drag to select by paragraphs.",
    "Shift-click extends by the same unit. Links like https://example.com/docs and words like don't stay whole.",
    "日本語のテキストも単語ごとに選択できます。",
];

struct Example {
    selection: TextSelection<u32>,
    focus: FocusHandle,
}

impl Example {
    fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            selection: TextSelection::new().joiner(|_, _| Join::Paragraph),
            focus: cx.focus_handle(),
        }
    }
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.selection.frame();
        let copy = self.selection.clone();
        let focus = self.focus.clone();
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                window.focus(&focus, cx);
            })
            .on_key_down(cx.listener(move |_, e: &KeyDownEvent, _, cx| {
                if e.keystroke.modifiers.platform && e.keystroke.key == "c" && !copy.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy.text()));
                }
            }))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .children(PARAGRAPHS.iter().enumerate().map(|(n, text)| {
                div().child(SelectableText::new(
                    ("paragraph", n),
                    n as u32,
                    &self.selection,
                    *text,
                ))
            }))
            .child(self.selection.drag_layer(
                |window| Bounds::new(Point::default(), window.viewport_size()),
                |_, _, _| {},
                |_, _| {},
            ))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(420.), px(320.)), cx);
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
