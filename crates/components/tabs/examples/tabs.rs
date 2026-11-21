//! Tabs: a horizontal list with an indicator and a disabled tab, and a
//! vertical one with manual activation.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
    text,
};
use ui_sdk_tabs::{
    Tabs, TabsActivation, TabsOptions, TabsOrientation, TabsPanelProps, TabsTabProps, TabsTabState,
};

struct Example {
    settings: Entity<Tabs>,
    sections: Entity<Tabs>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = cx.new(|cx| Tabs::new(TabsOptions::default(), window, cx));
        let sections = cx.new(|cx| {
            Tabs::new(
                TabsOptions::default()
                    .orientation(TabsOrientation::Vertical)
                    .activation(TabsActivation::Manual)
                    .default_value("security"),
                window,
                cx,
            )
        });
        Self { settings, sections }
    }
}

fn tab_color(state: &TabsTabState) -> gpui::Rgba {
    match (state.selected, state.disabled) {
        (_, true) => rgb(0x9ca3af),
        (true, false) => rgb(0x111827),
        (false, false) => rgb(0x4b5563),
    }
}

fn tabs(
    entity: &Entity<Tabs>,
    label: &'static str,
    items: &[(&'static str, &'static str, bool, &'static str)],
    vertical: bool,
    cx: &App,
) -> impl IntoElement {
    let list = Tabs::list(entity, cx).aria_label(SharedString::from(label)).flex().gap_1();
    let list = if vertical { list.flex_col() } else { list.pb_1() };
    let list = list
        .children(items.iter().map(|&(value, name, disabled, _)| {
            Tabs::tab(entity, value, TabsTabProps::default().disabled(disabled), cx)
                .aria_label(SharedString::from(name))
                .with(|el, state| el.text_color(tab_color(state)))
                .px_3()
                .py_1()
                .child(name)
        }))
        .child(Tabs::indicator(entity, cx).with(move |el, state| match state.active_tab {
            Some(tab) if vertical => el.absolute().left(px(0.)).top(tab.top).w(px(2.)).h(tab.height).bg(rgb(0x2563eb)),
            Some(tab) => el.absolute().left(tab.left).bottom(px(0.)).w(tab.width).h(px(2.)).bg(rgb(0x2563eb)),
            None => el.hidden(),
        }));
    let root = Tabs::root(entity, cx).flex().gap_4();
    let root = if vertical { root } else { root.flex_col() };
    root.child(list).child(div().children(items.iter().filter_map(|&(value, _, _, body)| {
        Tabs::panel(entity, value, TabsPanelProps::default(), cx).map(|panel| panel.p_2().child(text!(body)))
    })))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_8()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(tabs(
                &self.settings,
                "Settings",
                &[
                    ("account", "Account", false, "Profile"),
                    ("password", "Password", false, "Change password"),
                    ("billing", "Billing", true, "Plans"),
                ],
                false,
                cx,
            ))
            .child(tabs(
                &self.sections,
                "Sections",
                &[
                    ("general", "General", false, "Name"),
                    ("security", "Security", false, "Two-factor"),
                    ("reports", "Reports", false, "Weekly"),
                ],
                true,
                cx,
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
