//! Toggle groups: several pressed at once, one that always stays pressed,
//! and a vertical one.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_toggle::{
    Toggle, ToggleGroup, ToggleGroupOptions, ToggleGroupOrientation, ToggleOptions, ToggleState,
};

struct Example {
    formatting: Entity<ToggleGroup>,
    formats: Vec<(Entity<Toggle>, &'static str)>,
    alignment: Entity<ToggleGroup>,
    alignments: Vec<(Entity<Toggle>, &'static str)>,
    tool: Entity<ToggleGroup>,
    tools: Vec<(Entity<Toggle>, &'static str)>,
}

fn toggles(
    group: &Entity<ToggleGroup>,
    items: &[(&'static str, &'static str, bool)],
    window: &mut Window,
    cx: &mut Context<Example>,
) -> Vec<(Entity<Toggle>, &'static str)> {
    items
        .iter()
        .map(|&(value, label, disabled)| {
            let group = group.clone();
            let toggle = cx.new(|cx| {
                Toggle::new(
                    ToggleOptions::default().value(value).disabled(disabled).group(group),
                    window,
                    cx,
                )
            });
            (toggle, label)
        })
        .collect()
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let formatting = cx.new(|cx| {
            ToggleGroup::new(
                ToggleGroupOptions::default().multiple(true).default_value(["bold"]),
                window,
                cx,
            )
        });
        let formats = toggles(
            &formatting,
            &[("bold", "Bold", false), ("italic", "Italic", false), ("underline", "Underline", true)],
            window,
            cx,
        );
        let alignment = cx.new(|cx| {
            ToggleGroup::new(
                ToggleGroupOptions::default()
                    .deselectable(false)
                    .default_value(["start"]),
                window,
                cx,
            )
        });
        let alignments = toggles(
            &alignment,
            &[("start", "Start", false), ("center", "Center", false), ("end", "End", false)],
            window,
            cx,
        );
        let tool = cx.new(|cx| {
            ToggleGroup::new(
                ToggleGroupOptions::default()
                    .orientation(ToggleGroupOrientation::Vertical)
                    .r#loop(false),
                window,
                cx,
            )
        });
        let tools = toggles(
            &tool,
            &[("pen", "Pen", false), ("brush", "Brush", false), ("eraser", "Eraser", false)],
            window,
            cx,
        );
        Self {
            formatting,
            formats,
            alignment,
            alignments,
            tool,
            tools,
        }
    }
}

fn background(state: &ToggleState) -> gpui::Rgba {
    match (state.pressed, state.disabled) {
        (_, true) => rgb(0xe5e5e5),
        (true, false) => rgb(0x1f2937),
        (false, false) => rgb(0xf3f4f6),
    }
}

fn group(
    group: &Entity<ToggleGroup>,
    toggles: &[(Entity<Toggle>, &'static str)],
    label: &'static str,
    vertical: bool,
    cx: &App,
) -> impl IntoElement {
    let part = ToggleGroup::group(group, cx)
        .aria_label(SharedString::from(label))
        .flex()
        .gap_1();
    let part = if vertical { part.flex_col() } else { part };
    part.children(toggles.iter().map(|(toggle, label)| {
        Toggle::root(toggle, cx)
            .aria_label(SharedString::from(*label))
            .with(|el, state| {
                el.bg(background(state)).text_color(if state.pressed && !state.disabled {
                    rgb(0xffffff)
                } else {
                    rgb(0x111827)
                })
            })
            .px_3()
            .py_1()
            .rounded_md()
            .child(*label)
    }))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .child(group(&self.formatting, &self.formats, "Formatting", false, cx))
            .child(group(&self.alignment, &self.alignments, "Alignment", false, cx))
            .child(group(&self.tool, &self.tools, "Tool", true, cx))
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
