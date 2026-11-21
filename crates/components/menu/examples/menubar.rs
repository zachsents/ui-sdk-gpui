//! Menubars: a row of menus with a submenu, shortcuts, checkboxes and a
//! radio group, and a vertical one whose menus open to the side.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, Orientation, ParentElement, Render, Rgba,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size, text,
};
use ui_sdk_kernel::{ActionOutcome, Part};
use ui_sdk_menu::{
    Menu, MenuCheckboxItemProps, MenuContent, MenuContentProps, MenuIndicatorItem, MenuItemIndicatorProps,
    MenuItemProps, MenuItemState, MenuOptions, MenuRadioGroupProps, MenuRadioItemProps, MenuShortcutProps,
    MenuSubTriggerProps, MenuTriggerProps, MenuTriggerState, Menubar, MenubarOptions, MenubarState,
};

struct Example {
    bar: Entity<Menubar>,
    file: Entity<Menu>,
    share: Entity<Menu>,
    edit: Entity<Menu>,
    view: Entity<Menu>,
    side: Entity<Menubar>,
    inbox: Entity<Menu>,
    drafts: Entity<Menu>,
}

fn logged() -> MenuOptions {
    MenuOptions::default().on_action(|value, _details, _window, _cx| {
        log::info!("{value}");
        ActionOutcome::Done
    })
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let bar = cx.new(|cx| Menubar::new(MenubarOptions::default(), window, cx));
        let file = cx.new(|cx| Menu::new(logged().menubar(&bar), window, cx));
        let share = cx.new(|cx| Menu::new(MenuOptions::default().parent(&file), window, cx));
        let edit = cx.new(|cx| Menu::new(logged().menubar(&bar), window, cx));
        let view = cx.new(|cx| {
            Menu::new(
                MenuOptions::default()
                    .menubar(&bar)
                    .on_checked_change(|checked, details, _window, _cx| log::info!("{} {checked}", details.value))
                    .on_value_change(|value, details, _window, _cx| log::info!("{} {value}", details.group)),
                window,
                cx,
            )
        });
        let side = cx.new(|cx| Menubar::new(MenubarOptions::default().orientation(Orientation::Vertical), window, cx));
        let inbox = cx.new(|cx| Menu::new(logged().menubar(&side), window, cx));
        let drafts = cx.new(|cx| Menu::new(logged().menubar(&side), window, cx));
        Self {
            bar,
            file,
            share,
            edit,
            view,
            side,
            inbox,
            drafts,
        }
    }
}

fn shade(highlighted: bool) -> Rgba {
    if highlighted { rgb(0xeff6ff) } else { rgb(0xffffff) }
}

fn bar(part: Part<MenubarState>) -> Part<MenubarState> {
    part.with(|el, state| {
        if state.orientation == Orientation::Vertical {
            el.flex_col().w(px(120.))
        } else {
            el
        }
    })
    .flex()
    .gap_1()
    .p_1()
    .rounded_md()
    .bg(rgb(0xf3f4f6))
}

fn trigger(menu: &Entity<Menu>, label: &'static str, cx: &App) -> Part<MenuTriggerState> {
    Menu::trigger(menu, label, MenuTriggerProps::default().label(label), cx)
        .with(|el, state| {
            let el = el.bg(if state.open { rgb(0xffffff) } else { rgb(0xf3f4f6) });
            if state.disabled {
                el.text_color(rgb(0x9ca3af))
            } else {
                el
            }
        })
        .px_3()
        .py_1()
        .rounded_md()
        .child(text!(label))
}

fn panel(content: MenuContent<()>) -> MenuContent<()> {
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

fn item(
    menu: &Entity<Menu>,
    value: &'static str,
    label: &'static str,
    shortcut: Option<&'static str>,
    cx: &App,
) -> Part<MenuItemState> {
    let mut props = MenuItemProps::new().value(value).label(label);
    if let Some(shortcut) = shortcut {
        props = props.shortcut(shortcut);
    }
    let part = Menu::item(menu, value, props, cx)
        .with(|el, state| el.bg(shade(state.highlighted)))
        .flex()
        .justify_between()
        .gap_4()
        .px_2()
        .py_1()
        .rounded_sm()
        .child(text!(label));
    match shortcut {
        Some(_) => part.child(
            Menu::shortcut(menu, Some(value.into()), MenuShortcutProps::default(), cx).text_color(rgb(0x6b7280)),
        ),
        None => part,
    }
}

fn separator(menu: &Entity<Menu>, key: &'static str, cx: &App) -> impl IntoElement {
    Menu::separator(menu, key, cx).h(px(1.)).my_1().bg(rgb(0xe5e7eb))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (file, share, edit, view) = (&self.file, &self.share, &self.edit, &self.view);

        let share_menu = Menu::content(share, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(item(share, "email", "Email", None, cx))
                .child(item(share, "link", "Copy link", None, cx))
        });
        let file_menu = Menu::content(file, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(item(file, "new", "New", Some("mod+alt+n"), cx))
                .child(item(file, "open", "Open", None, cx))
                .child(
                    Menu::trigger(share, "share", MenuSubTriggerProps::new().label("Share"), cx)
                        .with(|el, state| el.bg(shade(state.highlighted || state.open)))
                        .flex()
                        .justify_between()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .child(text!("Share"))
                        .child("›"),
                )
                .children(share_menu)
                .child(separator(file, "end", cx))
                .child(item(file, "quit", "Quit", Some("mod+q"), cx))
        });
        let edit_menu = Menu::content(edit, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(item(edit, "undo", "Undo", Some("mod+z"), cx))
                .child(item(edit, "redo", "Redo", Some("shift+mod+z"), cx))
                .child(separator(edit, "history", cx))
                .child(item(edit, "cut", "Cut", Some("mod+x"), cx))
                .child(item(edit, "copy", "Copy", Some("mod+c"), cx))
                .child(item(edit, "paste", "Paste", Some("mod+v"), cx))
        });
        let view_menu = Menu::content(view, MenuContentProps::default(), cx).map(|content| {
            let checkbox = |value: &'static str, label: &'static str, cx: &App| {
                let part = Menu::checkbox_item(view, value, MenuCheckboxItemProps::new().label(label), cx);
                let check = Menu::item_indicator(
                    view,
                    MenuIndicatorItem::checkbox(value),
                    MenuItemIndicatorProps::new(),
                    cx,
                )
                .map(|indicator| indicator.child("✓"));
                part.with(|el, state| el.bg(shade(state.highlighted)))
                    .flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .child(div().w(px(12.)).children(check))
                    .child(text!(label))
            };
            let radio = |value: &'static str, label: &'static str, cx: &App| {
                let part = Menu::radio_item(view, "appearance", value, MenuRadioItemProps::new().label(label), cx);
                let dot = Menu::item_indicator(
                    view,
                    MenuIndicatorItem::radio("appearance", value),
                    MenuItemIndicatorProps::new(),
                    cx,
                )
                .map(|indicator| indicator.child("•"));
                part.with(|el, state| el.bg(shade(state.highlighted)))
                    .flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .child(div().w(px(12.)).children(dot))
                    .child(text!(label))
            };
            panel(content)
                .child(checkbox("status-bar", "Status bar", cx))
                .child(checkbox("minimap", "Minimap", cx))
                .child(
                    Menu::separator(view, "appearance", cx)
                        .h(px(1.))
                        .my_1()
                        .bg(rgb(0xe5e7eb)),
                )
                .child(
                    Menu::radio_group(
                        view,
                        "appearance",
                        MenuRadioGroupProps::new().default_value("light"),
                        cx,
                    )
                    .child(
                        Menu::group_label(view, "appearance", cx)
                            .aria_label("Appearance")
                            .px_2()
                            .text_xs()
                            .text_color(rgb(0x6b7280))
                            .child("Appearance"),
                    )
                    .child(radio("light", "Light", cx))
                    .child(radio("dark", "Dark", cx)),
                )
        });

        let (inbox, drafts) = (&self.inbox, &self.drafts);
        let inbox_menu = Menu::content(inbox, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(item(inbox, "read", "Mark read", None, cx))
                .child(item(inbox, "archive", "Archive", None, cx))
        });
        let drafts_menu = Menu::content(drafts, MenuContentProps::default(), cx).map(|content| {
            panel(content)
                .child(item(drafts, "send", "Send", None, cx))
                .child(item(drafts, "discard", "Discard", None, cx))
        });

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
            .child(
                bar(Menubar::root(&self.bar, cx))
                    .aria_label("Application")
                    .child(trigger(file, "File", cx))
                    .children(file_menu)
                    .child(trigger(edit, "Edit", cx))
                    .children(edit_menu)
                    .child(trigger(view, "View", cx))
                    .children(view_menu),
            )
            .child(
                bar(Menubar::root(&self.side, cx))
                    .aria_label("Folders")
                    .child(trigger(inbox, "Inbox", cx))
                    .children(inbox_menu)
                    .child(trigger(drafts, "Drafts", cx))
                    .children(drafts_menu),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(560.), px(360.)), cx);
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
