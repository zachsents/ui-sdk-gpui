//! Command palettes: one composed of parts, with groups, shortcuts and a
//! page, and one from data, with a page and a shortcut that runs from any
//! page.

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, FontWeight, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_command::{
    Command, CommandData, CommandDataItem, CommandDataItemExt, CommandItemProps, CommandOptions, CommandState,
    command,
};
use ui_sdk_kernel::{ActionOutcome, Part};
use ui_sdk_lists::{ListGroup, ListItemTextProps, ListNode};
use ui_sdk_platform::live_regions;
use ui_sdk_shortcuts::ShortcutProps;

struct Example {
    composed: Entity<Command>,
    data: Entity<Command>,
    ran: SharedString,
}

fn commands() -> CommandData {
    CommandData::new([
        ListNode::Group(
            ListGroup::new([
                command("inbox", "Open inbox").shortcut("mod+i"),
                command("profile", "Profile").keywords(["account"]),
            ])
            .label("General"),
        ),
        ListNode::separator(),
        ListNode::Item(
            command("theme", "Change theme").items(CommandData::new([command("light", "Light"), command("dark", "Dark")])),
        ),
    ])
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let composed = cx.new(|cx| {
            Command::new(
                CommandOptions::default()
                    .placeholder("Search")
                    .on_action(move |value, _details, _window, cx| {
                        let ran = value.clone();
                        this.update(cx, |example, cx| {
                            example.ran = ran;
                            cx.notify();
                        })
                        .ok();
                        ActionOutcome::Done
                    }),
                window,
                cx,
            )
        });
        let this = cx.entity().downgrade();
        let data = cx.new(|cx| {
            Command::new(
                CommandOptions::default()
                    .placeholder("Search")
                    .items(commands())
                    .on_action(move |value, _details, _window, cx| {
                        let ran = value.clone();
                        this.update(cx, |example, cx| {
                            example.ran = ran;
                            cx.notify();
                        })
                        .ok();
                        ActionOutcome::Done
                    }),
                window,
                cx,
            )
        });
        Self {
            composed,
            data,
            ran: SharedString::default(),
        }
    }
}

// A command row: its text with what the search matched in bold, and its
// shortcut.
fn row(palette: &Entity<Command>, props: CommandItemProps, cx: &App) -> AnyElement {
    let key = props.value.clone();
    let label = props.label.clone().unwrap_or_else(|| key.clone());
    let has_shortcut = props.shortcut.is_some();
    Command::item(palette, props, cx)
        .with(|el, state| {
            el.when(state.highlighted, |el| el.bg(rgb(0xe5e7eb)))
                .when(state.disabled, |el| el.opacity(0.5))
        })
        .flex()
        .justify_between()
        .px_2()
        .py_1()
        .rounded_sm()
        .child(
            Command::item_text(palette, &key, ListItemTextProps::new(label), cx).with(|el, state| {
                el.flex().children(state.segments.iter().map(|segment| {
                    div()
                        .when(segment.matched, |el| el.font_weight(FontWeight::BOLD))
                        .child(segment.text.clone())
                }))
            }),
        )
        .when(has_shortcut, |el| {
            el.child(
                Command::shortcut(palette, Some(&key), ShortcutProps::default(), cx)
                    .text_color(rgb(0x6b7280)),
            )
        })
        .into_any_element()
}

fn data_rows(palette: &Entity<Command>, items: &CommandData, cx: &App) -> Vec<AnyElement> {
    let item = |item: &CommandDataItem| row(palette, CommandItemProps::from_data(item), cx);
    items
        .nodes()
        .iter()
        .enumerate()
        .map(|(index, node)| match node {
            ListNode::Item(command) => item(command),
            ListNode::Group(group) => {
                let key = group.label.clone().unwrap_or_else(|| SharedString::from(format!("group-{index}")));
                Command::group(palette, &key, cx)
                    .flex()
                    .flex_col()
                    .when_some(group.label.clone(), |el, label| {
                        el.child(
                            Command::group_label(palette, &key, cx)
                                .aria_label(label.clone())
                                .px_2()
                                .text_xs()
                                .text_color(rgb(0x6b7280))
                                .child(label),
                        )
                    })
                    .children(group.items.iter().map(item))
                    .into_any_element()
            }
            ListNode::Separator(_) => Command::separator(palette, Some(&format!("separator-{index}")), cx)
                .h(px(1.))
                .my_1()
                .bg(rgb(0xe5e7eb))
                .into_any_element(),
        })
        .collect()
}

fn frame(palette: &Entity<Command>, cx: &App) -> Part<CommandState> {
    Command::root(palette, cx)
        .flex()
        .flex_col()
        .gap_1()
        .w(px(300.))
        .p_1()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .rounded_md()
        .child(
            Command::input(palette, cx)
                .aria_label("Search commands")
                .w_full()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(rgb(0xe5e7eb)),
        )
        .child(Command::empty(palette, cx).with(|el, state| el.when(state.empty, |el| el.px_2().child("No results"))))
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let composed = &self.composed;
        let first = Command::page(composed, None, cx).map(|page| {
            page.flex()
                .flex_col()
                .child(
                    Command::group(composed, "files", cx)
                        .flex()
                        .flex_col()
                        .child(
                            Command::group_label(composed, "files", cx)
                                .aria_label("Files")
                                .px_2()
                                .text_xs()
                                .text_color(rgb(0x6b7280))
                                .child("Files"),
                        )
                        .child(row(
                            composed,
                            CommandItemProps::new("new-file").label("New file").group("files").shortcut("mod+shift+f"),
                            cx,
                        ))
                        .child(row(
                            composed,
                            CommandItemProps::new("new-folder").label("New folder").group("files"),
                            cx,
                        )),
                )
                .child(Command::separator(composed, Some("tools"), cx).h(px(1.)).my_1().bg(rgb(0xe5e7eb)))
                .child(row(composed, CommandItemProps::new("theme").label("Change theme").page("theme"), cx))
                .child(row(composed, CommandItemProps::new("archive").label("Archive").disabled(true), cx))
        });
        let theme = Command::page(composed, Some("theme"), cx).map(|page| {
            page.flex()
                .flex_col()
                .child(row(composed, CommandItemProps::new("light").label("Light"), cx))
                .child(row(composed, CommandItemProps::new("dark").label("Dark"), cx))
        });
        let composed_list = Command::list(composed, cx)
            .flex()
            .flex_col()
            .max_h(px(180.))
            .overflow_y_scroll()
            .children(first)
            .children(theme);
        let data_list = Command::list(&self.data, cx);
        let items = data_list.state().items.clone();
        let data_list = data_list
            .flex()
            .flex_col()
            .max_h(px(180.))
            .overflow_y_scroll()
            .children(data_rows(&self.data, &items, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_3()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .child(frame(composed, cx).child(composed_list))
                    .child(frame(&self.data, cx).child(data_list)),
            )
            .child(div().text_sm().child(self.ran.clone()))
            .child(live_regions(window))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(680.), px(420.)), cx);
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
