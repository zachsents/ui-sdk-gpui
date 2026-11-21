//! Listboxes: one value, several with ranges, groups, horizontal, focus on
//! the list, and a long list that scrolls.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb,
    size,
};
use ui_sdk_listbox::{
    Listbox, ListboxActivation, ListboxItemIndicatorProps, ListboxItemProps, ListboxItemState, ListboxOptions,
    ListboxOrientation,
};

const TOPPINGS: &[(&str, &str)] = &[
    ("basil", "Basil"),
    ("cheese", "Extra cheese"),
    ("mushroom", "Mushroom"),
    ("olive", "Olive"),
    ("onion", "Onion"),
    ("pepper", "Pepper"),
];

const FRUITS: &[(&str, &str)] = &[("apple", "Apple"), ("banana", "Banana"), ("orange", "Orange")];

const VEGETABLES: &[(&str, &str)] = &[("carrot", "Carrot"), ("broccoli", "Broccoli"), ("spinach", "Spinach")];

const SIZES: &[(&str, &str)] = &[("xs", "XS"), ("s", "S"), ("m", "M"), ("l", "L"), ("xl", "XL")];

const FRAMEWORKS: &[(&str, &str)] = &[
    ("next", "Next.js"),
    ("remix", "Remix"),
    ("astro", "Astro"),
    ("nuxt", "Nuxt"),
    ("sveltekit", "SvelteKit"),
];

const ZONES: &[(&str, &str)] = &[
    ("honolulu", "Honolulu"),
    ("anchorage", "Anchorage"),
    ("los-angeles", "Los Angeles"),
    ("denver", "Denver"),
    ("chicago", "Chicago"),
    ("new-york", "New York"),
    ("sao-paulo", "São Paulo"),
    ("london", "London"),
    ("paris", "Paris"),
    ("cairo", "Cairo"),
    ("dubai", "Dubai"),
    ("kolkata", "Kolkata"),
    ("tokyo", "Tokyo"),
    ("sydney", "Sydney"),
];

struct Example {
    toppings: Entity<Listbox>,
    extras: Entity<Listbox>,
    produce: Entity<Listbox>,
    sizes: Entity<Listbox>,
    frameworks: Entity<Listbox>,
    zones: Entity<Listbox>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Tab moves focus, as in a browser.
        ui_sdk_focus::bind_tab_keys(cx);
        let toppings = cx.new(|cx| {
            Listbox::new(
                ListboxOptions::default().default_value(Some(SharedString::from("basil"))),
                window,
                cx,
            )
        });
        let extras = cx.new(|cx| {
            Listbox::new(
                ListboxOptions::default()
                    .multiple(true)
                    .default_value(vec![SharedString::from("cheese"), SharedString::from("onion")]),
                window,
                cx,
            )
        });
        let produce = cx.new(|cx| {
            Listbox::new(
                ListboxOptions::default()
                    .multiple(true)
                    .default_value(vec![SharedString::from("banana")]),
                window,
                cx,
            )
        });
        let sizes = cx.new(|cx| {
            Listbox::new(
                ListboxOptions::default()
                    .orientation(ListboxOrientation::Horizontal)
                    .r#loop(false)
                    .default_value(Some(SharedString::from("m"))),
                window,
                cx,
            )
        });
        let frameworks = cx.new(|cx| {
            Listbox::new(
                ListboxOptions::default()
                    .activation(ListboxActivation::Manual)
                    .virtual_focus(true),
                window,
                cx,
            )
        });
        let zones = cx.new(|cx| {
            Listbox::new(
                ListboxOptions::default().default_value(Some(SharedString::from("tokyo"))),
                window,
                cx,
            )
        });
        Self {
            toppings,
            extras,
            produce,
            sizes,
            frameworks,
            zones,
        }
    }
}

fn item_look(el: Stateful<Div>, state: &ListboxItemState) -> Stateful<Div> {
    el.when(state.highlighted, |el| el.bg(rgb(0xeef2ff)))
        .when(state.selected, |el| el.text_color(rgb(0x3730a3)))
        .when(state.disabled, |el| el.text_color(rgb(0x9ca3af)))
}

fn option(listbox: &Entity<Listbox>, value: &'static str, label: &'static str, group: Option<&'static str>, cx: &App) -> impl IntoElement {
    let indicator = Listbox::item_indicator(listbox, &SharedString::from(value), ListboxItemIndicatorProps::default(), cx)
        .map(|indicator| indicator.child("✓"));
    let mut props = ListboxItemProps::new(value).text(label);
    if let Some(group) = group {
        props = props.group(group);
    }
    Listbox::item(listbox, props, cx)
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

fn frame(el: impl IntoElement) -> Div {
    div().flex().flex_col().gap_1().child(el)
}

fn boxed<S: 'static>(part: ui_sdk_kernel::Part<S>) -> ui_sdk_kernel::Part<S> {
    part.flex()
        .flex_col()
        .w(px(180.))
        .p_1()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .rounded_md()
        .focus(|style| style.border_color(rgb(0x6366f1)))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toppings = boxed(Listbox::root(&self.toppings, cx))
            .aria_label("Toppings")
            .children(TOPPINGS.iter().map(|&(value, label)| option(&self.toppings, value, label, None, cx)));
        let extras = boxed(Listbox::root(&self.extras, cx))
            .aria_label("Extras")
            .children(TOPPINGS.iter().map(|&(value, label)| option(&self.extras, value, label, None, cx)));
        let group = |key: &'static str, label: &'static str, items: &[(&'static str, &'static str)], cx: &App| {
            Listbox::group(&self.produce, key, cx)
                .flex()
                .flex_col()
                .child(
                    Listbox::group_label(&self.produce, key, cx)
                        .aria_label(label)
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(rgb(0x6b7280))
                        .child(label),
                )
                .children(items.iter().map(|&(value, text)| option(&self.produce, value, text, Some(key), cx)))
        };
        let produce = boxed(Listbox::root(&self.produce, cx))
            .aria_label("Produce")
            .child(group("fruits", "Fruits", FRUITS, cx))
            .child(
                Listbox::separator(&self.produce, "between", cx)
                    .h(px(1.))
                    .my_1()
                    .bg(rgb(0xe5e7eb)),
            )
            .child(group("vegetables", "Vegetables", VEGETABLES, cx));
        let sizes = Listbox::root(&self.sizes, cx)
            .aria_label("Size")
            .flex()
            .gap_1()
            .p_1()
            .border_1()
            .border_color(rgb(0xd1d5db))
            .rounded_md()
            .children(SIZES.iter().map(|&(value, label)| {
                Listbox::item(&self.sizes, ListboxItemProps::new(value).text(label).disabled(value == "xl"), cx)
                    .with(|el, state| item_look(el, state).when(state.selected, |el| el.bg(rgb(0xe0e7ff))))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .child(label)
            }));
        let frameworks = boxed(Listbox::root(&self.frameworks, cx))
            .aria_label("Framework")
            .children(FRAMEWORKS.iter().map(|&(value, label)| option(&self.frameworks, value, label, None, cx)));
        let zones = boxed(Listbox::root(&self.zones, cx))
            .aria_label("Time zone")
            .max_h(px(140.))
            .overflow_y_scroll()
            .children(ZONES.iter().map(|&(value, label)| option(&self.zones, value, label, None, cx)));
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
            .child(frame(toppings))
            .child(frame(extras))
            .child(frame(produce))
            .child(frame(frameworks))
            .child(frame(zones))
            .child(frame(sizes))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(880.), px(560.)), cx);
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
