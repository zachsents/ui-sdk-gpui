//! Trigger menus: @ for people, / for commands with arguments and # for
//! channels a loader finds in a message, and @ in a single-line task, where
//! Enter chooses only after an arrow key.

use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Rgba, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, prelude::FluentBuilder, px, rgb,
    size,
};
use ui_sdk_kernel::Part;
use ui_sdk_native::{TextField, TextFieldOptions};
use ui_sdk_platform::live_regions;
use ui_sdk_trigger_menu::{
    AsyncOptionsContext, ListNode, TriggerAfter, TriggerArgument, TriggerBoundary, TriggerDefinition, TriggerItemExtra,
    TriggerItems, TriggerMenu, TriggerMenuContentProps, TriggerMenuData, TriggerMenuDataItem, TriggerMenuInputState,
    TriggerMenuItemProps, TriggerMenuOptions,
};

struct Example {
    message: Entity<TextField>,
    compose: Entity<TriggerMenu>,
    task: Entity<TextField>,
    assign: Entity<TriggerMenu>,
}

fn person(value: &str, label: &str) -> TriggerMenuDataItem {
    TriggerMenuDataItem::new(SharedString::from(value.to_string()), label.to_string())
}

fn people() -> TriggerMenuData {
    TriggerMenuData::new([
        person("hollis", "Hollis Grant"),
        person("ines", "Inès Moreau"),
        person("kofi", "Kofi Mensah"),
        person("wren", "Wren Takahashi"),
        person("tomasz", "Tomasz Wójcik"),
    ])
}

fn commands() -> TriggerMenuData {
    let command = |name: &str| person(name, name);
    TriggerMenuData::new([
        command("status").extra(
            TriggerItemExtra::new().arguments([
                TriggerArgument::new("status")
                    .hint("<status>")
                    .required(true)
                    .items(["available", "busy", "away"]),
                TriggerArgument::new("until")
                    .hint("[until]")
                    .items(["30m", "1h", "4h", "today"])
                    .allow_custom_value(false),
            ]),
        ),
        command("search")
            .extra(TriggerItemExtra::new().arguments([TriggerArgument::new("query").hint("<query>").required(true)])),
        command("away").extra(TriggerItemExtra::new().after(TriggerAfter::Submit)),
        command("clear"),
    ])
}

const CHANNELS: [&str; 7] = [
    "announcements",
    "design",
    "design-reviews",
    "engineering",
    "general",
    "launches",
    "support",
];

fn channels() -> TriggerItems {
    TriggerItems::loader(|query: SharedString, _: AsyncOptionsContext, cx: &mut App| {
        let wait = cx.background_executor().timer(Duration::from_millis(400));
        Box::pin(async move {
            wait.await;
            let query = query.to_lowercase();
            Ok(TriggerMenuData::new(
                CHANNELS.into_iter().filter(|channel| channel.contains(&query)),
            ))
        })
    })
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| {
            TextField::new(
                TextFieldOptions::default()
                    .multi_line(true)
                    .rows(3)
                    .placeholder("Message")
                    .on_submit(|field, window, cx| field.set_value("", window, cx)),
                window,
                cx,
            )
        });
        let compose = cx.new(|cx| {
            TriggerMenu::new(
                TriggerMenuOptions::default().triggers([
                    TriggerDefinition::new("@").kind("mention").items(people()),
                    TriggerDefinition::new("/")
                        .boundary(TriggerBoundary::Start)
                        .kind("command")
                        .items(commands()),
                    TriggerDefinition::new("#").kind("channel").items(channels()),
                ]),
                window,
                cx,
            )
        });
        let task = cx.new(|cx| TextField::new(TextFieldOptions::default().placeholder("Task"), window, cx));
        let assign = cx.new(|cx| {
            TriggerMenu::new(
                TriggerMenuOptions::default()
                    .triggers([TriggerDefinition::new("@").items(people())])
                    .auto_highlight(false)
                    .r#loop(false),
                window,
                cx,
            )
        });
        Self {
            message,
            compose,
            task,
            assign,
        }
    }
}

const BORDER: u32 = 0xd1d5db;

fn muted() -> Rgba {
    rgb(0x6b7280)
}

fn field(label: &'static str, input: Part<TriggerMenuInputState>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().text_color(muted()).child(label))
        .child(
            input
                .aria_label(label)
                .w(px(320.))
                .border_1()
                .border_color(rgb(BORDER))
                .rounded_md()
                .px_2()
                .py_1(),
        )
}

fn suggestions(menu: &Entity<TriggerMenu>, cx: &App) -> Option<impl IntoElement + use<>> {
    let content = TriggerMenu::content(menu, TriggerMenuContentProps::default(), cx)?;
    let items = content.state().items.clone();
    let list = TriggerMenu::list(menu, cx)
        .flex()
        .flex_col()
        .children(items.iter().filter_map(|node| {
            match node {
                ListNode::Item(item) => Some(
                    TriggerMenu::item(
                        menu,
                        TriggerMenuItemProps::new(item.value.clone())
                            .label(item.label.clone())
                            .disabled(item.disabled),
                        cx,
                    )
                    .with(|el, state| el.when(state.highlighted, |el| el.bg(rgb(0xe5e7eb))))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .child(item.label.clone()),
                ),
                _ => None,
            }
        }));
    let empty = TriggerMenu::empty(menu, cx).with(|el, state| {
        el.px_2()
            .text_color(muted())
            .when(state.loading, |el| el.py_1().child("Loading"))
            .when(state.empty, |el| el.py_1().child("No matches"))
    });
    Some(
        content
            .w(px(220.))
            .max_h(px(200.))
            .overflow_y_scroll()
            .p_1()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded_md()
            .shadow_md()
            .child(empty)
            .child(list),
    )
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hidden = |menu: &Entity<TriggerMenu>, cx: &App| {
            TriggerMenu::status(menu, true, cx)
                .absolute()
                .size_0()
                .overflow_hidden()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(field("Message", TriggerMenu::input(&self.compose, &self.message, cx)))
            .children(TriggerMenu::hint(&self.compose, true, cx).map(|hint| hint.text_color(muted())))
            .children(suggestions(&self.compose, cx))
            .child(field("Task", TriggerMenu::input(&self.assign, &self.task, cx)))
            .children(suggestions(&self.assign, cx))
            .child(hidden(&self.compose, cx))
            .child(hidden(&self.assign, cx))
            .child(live_regions(window))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(420.), px(360.)), cx);
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
