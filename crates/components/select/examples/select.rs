//! Selects: one value with a label, several values, groups, a search
//! field, and one that presents as a sheet.

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb,
    size,
};
use ui_sdk_native::TextFieldOptions;
use ui_sdk_platform::live_regions;
use ui_sdk_popups::PresentationSetting;
use ui_sdk_select::{
    Select, SelectContentProps, SelectItemProps, SelectItemState, SelectOptions, SelectTriggerProps,
    SelectValueProps,
};

const ROLES: &[(&str, &str)] = &[
    ("viewer", "Viewer"),
    ("commenter", "Commenter"),
    ("editor", "Editor"),
    ("admin", "Admin"),
];

const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("fr", "French"),
    ("de", "German"),
    ("ja", "Japanese"),
    ("pt", "Portuguese"),
    ("es", "Spanish"),
];

const FONTS: &[(&str, &str, &[(&str, &str)])] = &[
    ("sans", "Sans serif", &[("inter", "Inter"), ("geist", "Geist")]),
    ("serif", "Serif", &[("source-serif", "Source Serif"), ("lora", "Lora")]),
    ("mono", "Monospace", &[("geist-mono", "Geist Mono"), ("jetbrains-mono", "JetBrains Mono")]),
];

const COUNTRIES: &[(&str, &str)] = &[
    ("ar", "Argentina"),
    ("au", "Australia"),
    ("br", "Brazil"),
    ("ca", "Canada"),
    ("de", "Germany"),
    ("in", "India"),
    ("jp", "Japan"),
    ("kr", "South Korea"),
    ("mx", "Mexico"),
    ("se", "Sweden"),
];

const STATUSES: &[(&str, &str)] = &[
    ("todo", "To do"),
    ("doing", "In progress"),
    ("review", "In review"),
    ("done", "Done"),
];

struct Example {
    role: Entity<Select>,
    languages: Entity<Select>,
    font: Entity<Select>,
    country: Entity<Select>,
    status: Entity<Select>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Tab moves focus, as in a browser.
        ui_sdk_focus::bind_tab_keys(cx);
        let role = cx.new(|cx| Select::new(SelectOptions::default().placeholder("Choose one"), window, cx));
        let languages = cx.new(|cx| {
            Select::new(
                SelectOptions::default()
                    .multiple(true)
                    .placeholder("None")
                    .default_value(vec![SharedString::from("en"), SharedString::from("ja")]),
                window,
                cx,
            )
        });
        let font = cx.new(|cx| Select::new(SelectOptions::default().placeholder("Choose one"), window, cx));
        let country = cx.new(|cx| {
            Select::new(
                SelectOptions::default()
                    .placeholder("Choose one")
                    .search(TextFieldOptions::default().placeholder("Search")),
                window,
                cx,
            )
        });
        let status = cx.new(|cx| {
            Select::new(
                SelectOptions::default()
                    .presentation(PresentationSetting::Sheet)
                    .default_value(Some(SharedString::from("todo"))),
                window,
                cx,
            )
        });
        Self {
            role,
            languages,
            font,
            country,
            status,
        }
    }
}

fn item_look(el: Stateful<Div>, state: &SelectItemState) -> Stateful<Div> {
    el.when(state.highlighted, |el| el.bg(rgb(0xeef2ff)))
        .when(state.selected, |el| el.text_color(rgb(0x3730a3)))
        .when(state.disabled, |el| el.text_color(rgb(0x9ca3af)))
}

fn item(select: &Entity<Select>, value: &'static str, label: &'static str, group: Option<&'static str>, cx: &App) -> impl IntoElement {
    let indicator = Select::item_indicator(select, &SharedString::from(value), Default::default(), cx)
        .map(|indicator| indicator.child("✓"));
    let mut props = SelectItemProps::new(value).text(label);
    if let Some(group) = group {
        props = props.group(group);
    }
    Select::item(select, props, cx)
        .with(item_look)
        .flex()
        .justify_between()
        .gap_4()
        .px_2()
        .py_1()
        .rounded_md()
        .child(label)
        .children(indicator)
}

// The trigger with the value in it.
fn trigger(select: &Entity<Select>, value: SelectValueProps, cx: &App) -> impl IntoElement {
    Select::trigger(select, SelectTriggerProps::default(), cx)
        .with(|el, state| el.when(state.placeholder, |el| el.text_color(rgb(0x6b7280))))
        .flex()
        .justify_between()
        .gap_2()
        .w(px(200.))
        .px_3()
        .py_1()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .rounded_md()
        .focus(|style| style.border_color(rgb(0x6366f1)))
        .child(Select::value_part(select, value, cx).with(|el, state| el.child(state.text.clone())))
        .child("▾")
}

// The popup's own look.
fn popup<E: Styled>(content: E) -> E {
    content
        .flex()
        .flex_col()
        .min_w(px(200.))
        .max_h(px(240.))
        .p_1()
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xd1d5db))
        .rounded_md()
        .shadow_md()
}

fn field(label: Option<AnyElement>, trigger: impl IntoElement, content: Option<AnyElement>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(label)
        .child(trigger)
        .children(content)
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let role = field(
            Some(Select::label(&self.role, cx).aria_label("Role").child("Role").into_any_element()),
            trigger(&self.role, SelectValueProps::default(), cx),
            Select::content(&self.role, SelectContentProps::default(), cx).map(|content| {
                popup(content)
                    .overflow_y_scroll()
                    .children(ROLES.iter().map(|&(value, label)| item(&self.role, value, label, None, cx)))
                    .into_any_element()
            }),
        );
        let languages = field(
            Some(Select::label(&self.languages, cx).aria_label("Languages").child("Languages").into_any_element()),
            trigger(&self.languages, SelectValueProps::default().max_labels(2), cx),
            Select::content(&self.languages, SelectContentProps::default(), cx).map(|content| {
                popup(content)
                    .children(LANGUAGES.iter().map(|&(value, label)| item(&self.languages, value, label, None, cx)))
                    .into_any_element()
            }),
        );
        let font = field(
            Some(Select::label(&self.font, cx).aria_label("Font").child("Font").into_any_element()),
            trigger(&self.font, SelectValueProps::default(), cx),
            Select::content(&self.font, SelectContentProps::default().align_item_with_trigger(true), cx).map(|content| {
                popup(content)
                    .children(FONTS.iter().enumerate().map(|(index, &(key, label, fonts))| {
                        div()
                            .when(index > 0, |el| {
                                el.child(Select::separator(&self.font, key, cx).h(px(1.)).my_1().bg(rgb(0xe5e7eb)))
                            })
                            .child(
                                Select::group(&self.font, key, cx)
                                    .flex()
                                    .flex_col()
                                    .child(
                                        Select::group_label(&self.font, key, cx)
                                            .aria_label(label)
                                            .px_2()
                                            .py_1()
                                            .text_xs()
                                            .text_color(rgb(0x6b7280))
                                            .child(label),
                                    )
                                    .children(fonts.iter().map(|&(value, text)| item(&self.font, value, text, Some(key), cx))),
                            )
                    }))
                    .into_any_element()
            }),
        );
        let country = field(
            Some(Select::label(&self.country, cx).aria_label("Country").child("Country").into_any_element()),
            trigger(&self.country, SelectValueProps::default(), cx),
            Select::content(&self.country, SelectContentProps::default(), cx).map(|content| {
                popup(content)
                    .gap_1()
                    .child(
                        Select::input(&self.country, cx)
                            .aria_label("Search countries")
                            .px_2()
                            .py_1()
                            .border_1()
                            .border_color(rgb(0xe5e7eb))
                            .rounded_md(),
                    )
                    .child(
                        Select::empty(&self.country, cx)
                            .with(|el, state| el.when(state.empty, |el| el.px_2().py_1().child("No results"))),
                    )
                    .child(
                        Select::list(&self.country, cx)
                            .flex()
                            .flex_col()
                            .max_h(px(160.))
                            .overflow_y_scroll()
                            .children(COUNTRIES.iter().map(|&(value, label)| item(&self.country, value, label, None, cx))),
                    )
                    .into_any_element()
            }),
        );
        let sheet = Select::content(&self.status, SelectContentProps::default(), cx).map(|content| {
            content
                .flex()
                .flex_col()
                .p_2()
                .bg(rgb(0xffffff))
                .rounded_t_lg()
                .shadow_md()
                .children(STATUSES.iter().map(|&(value, label)| item(&self.status, value, label, None, cx)))
        });
        let status = field(
            Some(Select::label(&self.status, cx).aria_label("Status").child("Status").into_any_element()),
            trigger(&self.status, SelectValueProps::default(), cx),
            sheet.map(|sheet| match Select::viewport(&self.status, cx) {
                Some(viewport) => viewport.flex().flex_col().justify_end().child(sheet).into_any_element(),
                None => sheet.into_any_element(),
            }),
        );
        div()
            .size_full()
            .flex()
            .flex_wrap()
            .items_start()
            .gap_6()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(role)
            .child(languages)
            .child(font)
            .child(country)
            .child(status)
            .child(live_regions(window))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(940.), px(560.)), cx);
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
