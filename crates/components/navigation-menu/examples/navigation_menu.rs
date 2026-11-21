//! Navigation menus: one with a viewport and an indicator, and a vertical
//! one that opens on click, each panel its own popup.

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds,
    WindowOptions, div, px, rgb, size, text,
};
use ui_sdk_navigation_menu::{
    NavigationMenu, NavigationMenuContentProps, NavigationMenuLinkProps, NavigationMenuOptions,
    NavigationMenuOrientation, NavigationMenuTriggerProps, NavigationMenuViewportProps,
};

// An item: its value, its trigger's label, and its panel's links.
type Item = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

const MAIN: &[Item] = &[
    ("guides", "Guides", &[("intro", "Introduction"), ("install", "Installation"), ("styling", "Styling")]),
    ("components", "Components", &[("tabs", "Tabs"), ("toggle", "Toggle")]),
];

const SIDEBAR: &[Item] = &[
    ("account", "Account", &[("profile", "Profile"), ("billing", "Billing")]),
    ("team", "Team", &[("members", "Members"), ("roles", "Roles")]),
];

struct Example {
    main: Entity<NavigationMenu>,
    sidebar: Entity<NavigationMenu>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let main = cx.new(|cx| NavigationMenu::new(NavigationMenuOptions::default(), window, cx));
        let sidebar = cx.new(|cx| {
            NavigationMenu::new(
                NavigationMenuOptions::default()
                    .orientation(NavigationMenuOrientation::Vertical)
                    .open_on_hover(false),
                window,
                cx,
            )
        });
        Self { main, sidebar }
    }
}

fn link(menu: &Entity<NavigationMenu>, key: &'static str, label: &'static str, current: bool, cx: &App) -> impl IntoElement {
    NavigationMenu::link(menu, key, NavigationMenuLinkProps::default().current(current), cx)
        .aria_label(SharedString::from(label))
        .px_2()
        .py_1()
        .rounded_md()
        .hover(|style| style.bg(rgb(0xf3f4f6)))
        .child(text!(label))
}

fn items(menu: &Entity<NavigationMenu>, items: &[Item], in_viewport: bool, cx: &App) -> Vec<AnyElement> {
    items
        .iter()
        .map(|&(value, label, links)| {
            let trigger = NavigationMenu::trigger(menu, value, NavigationMenuTriggerProps::default(), cx)
                .aria_label(SharedString::from(label))
                .px_3()
                .py_1()
                .rounded_md()
                .with(|el, state| el.bg(if state.open { rgb(0xe5e7eb) } else { rgb(0xffffff) }))
                .child(text!(label));
            let content = NavigationMenu::content(menu, value, NavigationMenuContentProps::default(), cx).map(|content| {
                let content = content
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .w(px(200.))
                    .children(links.iter().map(|&(key, label)| link(menu, key, label, false, cx)));
                if in_viewport {
                    content.absolute().top_0().left_0()
                } else {
                    content.bg(rgb(0xffffff)).border_1().border_color(rgb(0xd1d5db)).rounded_md()
                }
            });
            NavigationMenu::item(menu, value, cx)
                .child(trigger)
                .children(content)
                .into_any_element()
        })
        .collect()
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = NavigationMenu::root(&self.main, cx).aria_label(SharedString::from("Main"));
        let list = NavigationMenu::list(&self.main, cx)
            .flex()
            .gap_1()
            .children(items(&self.main, MAIN, true, cx))
            .child(NavigationMenu::item(&self.main, "blog", cx).child(link(&self.main, "blog", "Blog", true, cx)));
        let indicator = NavigationMenu::indicator(&self.main, cx).with(|el, state| match state.item_box {
            Some(item) => el.absolute().left(item.left).bottom(item.bottom).w(item.width).h(px(2.)).bg(rgb(0x2563eb)),
            None => el.hidden(),
        });
        let viewport = NavigationMenu::viewport(&self.main, NavigationMenuViewportProps::default(), cx)
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0xd1d5db))
            .rounded_md()
            .overflow_hidden()
            .with(|el, state| match state.size {
                Some(content) => el.w(content.width + px(2.)).h(content.height + px(2.)),
                None => el,
            });
        let sidebar = NavigationMenu::root(&self.sidebar, cx).aria_label(SharedString::from("Settings"));
        let sidebar_list = NavigationMenu::list(&self.sidebar, cx)
            .flex()
            .flex_col()
            .gap_1()
            .children(items(&self.sidebar, SIDEBAR, false, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_8()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(main.child(list).child(indicator).child(viewport))
            .child(sidebar.child(sidebar_list))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(560.), px(400.)), cx);
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
