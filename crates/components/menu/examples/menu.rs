//! Menus: actions with shortcuts, a pending and a destructive item, a
//! submenu, checkbox and radio items in groups, a context menu on rows, and
//! a menu that presents as a sheet.

use std::fmt::Debug;
use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Rgba, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size, text,
};
use ui_sdk_kernel::{ActionOutcome, Part};
use ui_sdk_menu::{
    Menu, MenuCheckboxItemProps, MenuCheckboxItemState, MenuContent, MenuContentProps, MenuContextTriggerProps,
    MenuIndicatorItem, MenuItemIndicatorProps, MenuItemProps, MenuItemState, MenuOptions, MenuRadioGroupProps,
    MenuRadioItemProps, MenuRadioItemState, MenuShortcutProps, MenuSubTriggerProps, MenuTriggerProps, MenuTriggerState,
    PresentationSetting,
};
use ui_sdk_platform::live_regions;

struct Example {
    file: Entity<Menu>,
    share: Entity<Menu>,
    view: Entity<Menu>,
    rows: Entity<Menu<SharedString>>,
    options: Entity<Menu>,
    move_to: Entity<Menu>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let file = cx.new(|cx| {
            Menu::new(
                MenuOptions::default().on_action(|value, _details, _window, cx| {
                    log::info!("{value}");
                    if value.as_ref() != "sync" {
                        return ActionOutcome::Done;
                    }
                    let timer = cx.background_executor().timer(Duration::from_millis(1500));
                    ActionOutcome::wait(async move {
                        timer.await;
                        Ok::<(), SharedString>(())
                    })
                }),
                window,
                cx,
            )
        });
        let share = cx.new(|cx| Menu::new(MenuOptions::default().parent(&file), window, cx));
        let view = cx.new(|cx| {
            Menu::new(
                MenuOptions::default()
                    .on_checked_change(|checked, details, _window, _cx| log::info!("{} {checked}", details.value))
                    .on_value_change(|value, details, _window, _cx| log::info!("{} {value}", details.group)),
                window,
                cx,
            )
        });
        let rows = cx.new(|cx| {
            Menu::new(
                MenuOptions::default().on_action(|value, _details, _window, _cx| {
                    log::info!("{value}");
                    ActionOutcome::Done
                }),
                window,
                cx,
            )
        });
        let options = cx.new(|cx| {
            Menu::new(
                MenuOptions::default()
                    .presentation(PresentationSetting::Sheet)
                    .on_action(|value, _details, _window, _cx| {
                        log::info!("{value}");
                        ActionOutcome::Done
                    }),
                window,
                cx,
            )
        });
        let move_to = cx.new(|cx| Menu::new(MenuOptions::default().parent(&options), window, cx));
        Self {
            file,
            share,
            view,
            rows,
            options,
            move_to,
        }
    }
}

fn shade(highlighted: bool) -> Rgba {
    if highlighted { rgb(0xeff6ff) } else { rgb(0xffffff) }
}

fn trigger(part: Part<MenuTriggerState>, label: &'static str) -> Part<MenuTriggerState> {
    part.with(|el, state| el.bg(if state.open { rgb(0xe5e7eb) } else { rgb(0xf3f4f6) }))
        .px_3()
        .py_1()
        .rounded_md()
        .child(text!(label))
}

fn panel<P: Clone + PartialEq + Debug + 'static>(content: MenuContent<P>) -> MenuContent<P> {
    content
        .flex()
        .flex_col()
        .min_w(px(200.))
        .p_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .bg(rgb(0xffffff))
        .shadow_md()
}

fn item(part: Part<MenuItemState>, label: &'static str) -> Part<MenuItemState> {
    part.with(|el, state| {
        let color = if state.disabled {
            rgb(0x9ca3af)
        } else if state.destructive {
            rgb(0xb91c1c)
        } else {
            rgb(0x111827)
        };
        el.bg(shade(state.highlighted))
            .text_color(color)
            .opacity(if state.pending { 0.6 } else { 1. })
    })
    .flex()
    .justify_between()
    .gap_4()
    .px_2()
    .py_1()
    .rounded_sm()
    .child(text!(label))
}

fn sub_trigger(part: Part<MenuTriggerState>, label: &'static str) -> Part<MenuTriggerState> {
    part.with(|el, state| el.bg(shade(state.highlighted || state.open)))
        .flex()
        .justify_between()
        .gap_4()
        .px_2()
        .py_1()
        .rounded_sm()
        .child(text!(label))
        .child("›")
}

fn separator<P: Clone + PartialEq + Debug + 'static>(
    menu: &Entity<Menu<P>>,
    key: &'static str,
    cx: &App,
) -> impl IntoElement {
    Menu::separator(menu, key, cx).h(px(1.)).my_1().bg(rgb(0xe5e7eb))
}

fn checkbox(
    view: &Entity<Menu>,
    value: &'static str,
    label: &'static str,
    checked: bool,
    cx: &App,
) -> Part<MenuCheckboxItemState> {
    let item = Menu::checkbox_item(
        view,
        value,
        MenuCheckboxItemProps::new().label(label).default_checked(checked),
        cx,
    );
    let indicator = Menu::item_indicator(
        view,
        MenuIndicatorItem::checkbox(value),
        MenuItemIndicatorProps::new(),
        cx,
    )
    .map(|indicator| indicator.child("✓"));
    item.with(|el, state| el.bg(shade(state.highlighted)))
        .flex()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_sm()
        .child(div().w(px(12.)).children(indicator))
        .child(text!(label))
}

fn radio(view: &Entity<Menu>, value: &'static str, label: &'static str, cx: &App) -> Part<MenuRadioItemState> {
    let item = Menu::radio_item(view, "panel", value, MenuRadioItemProps::new().label(label), cx);
    let indicator = Menu::item_indicator(
        view,
        MenuIndicatorItem::radio("panel", value),
        MenuItemIndicatorProps::new(),
        cx,
    )
    .map(|indicator| indicator.child("•"));
    item.with(|el, state| el.bg(shade(state.highlighted)))
        .flex()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_sm()
        .child(div().w(px(12.)).children(indicator))
        .child(text!(label))
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let file = &self.file;
        let share = &self.share;
        let share_menu = Menu::content(share, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(item(
                    Menu::item(share, "email", MenuItemProps::new().value("email").label("Email"), cx),
                    "Email",
                ))
                .child(item(
                    Menu::item(share, "link", MenuItemProps::new().value("link").label("Copy link"), cx),
                    "Copy link",
                ))
        });
        let file_menu = Menu::content(file, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(
                    item(
                        Menu::item(
                            file,
                            "new",
                            MenuItemProps::new().value("new").label("New").shortcut("mod+n"),
                            cx,
                        ),
                        "New",
                    )
                    .child(
                        Menu::shortcut(file, Some("new".into()), MenuShortcutProps::default(), cx)
                            .text_color(rgb(0x6b7280)),
                    ),
                )
                .child(
                    item(
                        Menu::item(
                            file,
                            "open",
                            MenuItemProps::new().value("open").label("Open").shortcut("mod+o"),
                            cx,
                        ),
                        "Open",
                    )
                    .child(
                        Menu::shortcut(file, Some("open".into()), MenuShortcutProps::default(), cx)
                            .text_color(rgb(0x6b7280)),
                    ),
                )
                .child(item(
                    Menu::item(
                        file,
                        "sync",
                        MenuItemProps::new().value("sync").label("Sync").close_on_select(false),
                        cx,
                    ),
                    "Sync",
                ))
                .child(sub_trigger(
                    Menu::trigger(share, "share", MenuSubTriggerProps::new().label("Share"), cx),
                    "Share",
                ))
                .children(share_menu)
                .child(separator(file, "end", cx))
                .child(item(
                    Menu::item(file, "print", MenuItemProps::new().label("Print").disabled(true), cx),
                    "Print",
                ))
                .child(item(
                    Menu::item(
                        file,
                        "delete",
                        MenuItemProps::new().value("delete").label("Delete").destructive(true),
                        cx,
                    ),
                    "Delete",
                ))
        });

        let view = &self.view;
        let view_menu = Menu::content(view, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(
                    Menu::group(view, "show", cx)
                        .child(
                            Menu::group_label(view, "show", cx)
                                .aria_label("Show")
                                .px_2()
                                .text_xs()
                                .text_color(rgb(0x6b7280))
                                .child("Show"),
                        )
                        .child(checkbox(view, "status-bar", "Status bar", true, cx))
                        .child(checkbox(view, "minimap", "Minimap", false, cx)),
                )
                .child(separator(view, "groups", cx))
                .child(
                    Menu::radio_group(view, "panel", MenuRadioGroupProps::new().default_value("bottom"), cx)
                        .child(
                            Menu::group_label(view, "panel", cx)
                                .aria_label("Panel")
                                .px_2()
                                .text_xs()
                                .text_color(rgb(0x6b7280))
                                .child("Panel"),
                        )
                        .child(radio(view, "bottom", "Bottom", cx))
                        .child(radio(view, "right", "Right", cx)),
                )
        });

        let rows = &self.rows;
        let rows_menu = Menu::content(rows, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(item(
                    Menu::item(rows, "open", MenuItemProps::new().value("open").label("Open"), cx),
                    "Open",
                ))
                .child(item(
                    Menu::item(
                        rows,
                        "archive",
                        MenuItemProps::new().value("archive").label("Archive"),
                        cx,
                    ),
                    "Archive",
                ))
        });
        let row = |name: &'static str, cx: &App| {
            Menu::context_trigger(
                rows,
                name,
                MenuContextTriggerProps::default().payload(SharedString::from(name)),
                cx,
            )
            .with(|el, state| el.bg(if state.open { rgb(0xeff6ff) } else { rgb(0xffffff) }))
            .w(px(240.))
            .px_3()
            .py_2()
            .border_1()
            .border_color(rgb(0xe5e7eb))
            .child(text!(name))
        };

        let options = &self.options;
        let move_to = &self.move_to;
        let move_menu = Menu::content(move_to, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(item(
                    Menu::item(move_to, "inbox", MenuItemProps::new().value("inbox").label("Inbox"), cx),
                    "Inbox",
                ))
                .child(item(
                    Menu::item(move_to, "trash", MenuItemProps::new().value("trash").label("Trash"), cx),
                    "Trash",
                ))
        });
        let options_menu = Menu::content(options, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(item(
                    Menu::item(options, "pin", MenuItemProps::new().value("pin").label("Pin"), cx),
                    "Pin",
                ))
                .child(sub_trigger(
                    Menu::trigger(move_to, "move", MenuSubTriggerProps::new().label("Move to"), cx),
                    "Move to",
                ))
                .children(move_menu)
        });

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(trigger(
                        Menu::trigger(file, "file", MenuTriggerProps::default(), cx),
                        "File",
                    ))
                    .children(file_menu)
                    .child(trigger(
                        Menu::trigger(view, "view", MenuTriggerProps::default(), cx),
                        "View",
                    ))
                    .children(view_menu)
                    .child(trigger(
                        Menu::trigger(options, "options", MenuTriggerProps::default(), cx),
                        "Options",
                    ))
                    .children(options_menu),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .child(row("Inbox", cx))
                    .child(row("Drafts", cx))
                    .child(row("Archive", cx))
                    .children(rows_menu),
            )
            .child(live_regions(window))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(640.), px(420.)), cx);
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
