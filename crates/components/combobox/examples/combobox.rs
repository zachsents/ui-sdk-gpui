//! Comboboxes: one that chooses a framework from items it renders, one that
//! chooses a country from items as data, and one that chooses several labels
//! as chips, with an option that creates one.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, prelude::FluentBuilder, px,
    rgb, size, text,
};
use ui_sdk_combobox::{
    Combobox, ComboboxClearProps, ComboboxContentProps, ComboboxCreateProps, ComboboxItemIndicatorProps,
    ComboboxItemProps, ComboboxItemState, ComboboxOptions, ComboboxValueProps, ListData, ListItem, ListNode,
    ListValue,
};
use ui_sdk_kernel::{ActionOutcome, ChangeOptions};

const FRAMEWORKS: &[(&str, &str)] = &[
    ("next", "Next.js"),
    ("sveltekit", "SvelteKit"),
    ("nuxt", "Nuxt"),
    ("remix", "Remix"),
    ("astro", "Astro"),
    ("solidstart", "SolidStart"),
    ("qwik", "Qwik City"),
];

const COUNTRIES: &[(&str, &str)] = &[
    ("fr", "France"),
    ("de", "Germany"),
    ("jp", "Japan"),
    ("se", "Sweden"),
    ("ch", "Switzerland"),
    ("us", "United States"),
];

const LABELS: &[&str] = &["Bug", "Feature", "Documentation", "Design", "Performance", "Security"];

type Picker = Entity<Combobox<SharedString>>;

struct Example {
    framework: Picker,
    country: Picker,
    labels: Picker,
    // The labels to choose from, with the ones created.
    label_names: Vec<SharedString>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let framework = cx.new(|cx| Combobox::new(ComboboxOptions::default().placeholder("Framework"), window, cx));
        let countries: ListData<SharedString> = COUNTRIES
            .iter()
            .map(|(value, label)| ListNode::Item(ListItem::new(SharedString::from(*value), *label)))
            .collect();
        let country = cx.new(|cx| {
            Combobox::new(
                ComboboxOptions::default()
                    .items(countries)
                    .auto_highlight(true)
                    .placeholder("Country"),
                window,
                cx,
            )
        });
        let labels = cx.new(|cx| {
            Combobox::new(
                ComboboxOptions::default()
                    .multiple(true)
                    .default_value(ListValue::Multiple(vec![SharedString::from("Bug")]))
                    .placeholder("Label"),
                window,
                cx,
            )
        });
        Self {
            framework,
            country,
            labels,
            label_names: LABELS.iter().map(|label| SharedString::from(*label)).collect(),
        }
    }

    // A new label joins the list, chosen.
    fn create_label(this: gpui::WeakEntity<Self>, labels: Picker) -> ComboboxCreateProps {
        ComboboxCreateProps::default()
            .min_length(2)
            .on_create(move |query, window, cx| {
                let query = query.clone();
                this.update(cx, |example, cx| {
                    example.label_names.push(query.clone());
                    cx.notify();
                })
                .ok();
                let labels = labels.clone();
                window.defer(cx, move |window, cx| {
                    labels.update(cx, |combobox, cx| {
                        let mut values = combobox.get_value().to_vec();
                        values.push(query);
                        combobox.set_value(ListValue::Multiple(values), ChangeOptions::default(), window, cx);
                    });
                });
                ActionOutcome::Done
            })
    }
}

fn field<E: Styled>(input: E) -> E {
    input
        .w(px(180.))
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .bg(rgb(0xffffff))
}

fn button<E: Styled>(button: E) -> E {
    button.px_2().py_1().rounded_md().bg(rgb(0xf3f4f6))
}

fn panel<E: Styled>(content: E) -> E {
    content
        .flex()
        .flex_col()
        .w(px(220.))
        .max_h(px(240.))
        .p_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .bg(rgb(0xffffff))
        .text_color(rgb(0x111827))
}

fn option_color(state: &ComboboxItemState) -> gpui::Rgba {
    match (state.highlighted, state.disabled) {
        (_, true) => rgb(0xffffff),
        (true, false) => rgb(0xe5e7eb),
        (false, false) => rgb(0xffffff),
    }
}

// An option, with a check while it's chosen.
fn item(picker: &Picker, value: &str, label: &str, cx: &App) -> impl IntoElement {
    let value = SharedString::from(value.to_string());
    let label = SharedString::from(label.to_string());
    let check = Combobox::item_indicator(picker, &value, ComboboxItemIndicatorProps::default(), cx);
    Combobox::item(picker, ComboboxItemProps::new(value).label(label.clone()), cx)
        .with(|el, state| el.bg(option_color(state)))
        .aria_label(label.clone())
        .flex()
        .justify_between()
        .px_2()
        .py_1()
        .rounded_md()
        .child(text!(label))
        .children(check.map(|check| check.child("✓")))
}

impl Example {
    fn framework(&self, cx: &App) -> impl IntoElement {
        let picker = &self.framework;
        let content = Combobox::content(picker, ComboboxContentProps::default(), cx).map(|content| {
            let empty = Combobox::empty(picker, cx);
            let nothing = empty.state().empty;
            panel(content)
                .overflow_y_scroll()
                .child(empty.when(nothing, |el| el.px_2().py_1().child(text!("No matches"))))
                .child(
                    Combobox::list(picker, cx)
                        .flex()
                        .flex_col()
                        .children(FRAMEWORKS.iter().map(|(value, label)| item(picker, value, label, cx))),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .items_start()
            .child(Combobox::label(picker, cx).aria_label("Framework").child(text!("Framework")))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_1()
                    .child(field(Combobox::input(picker, cx)))
                    .children(
                        Combobox::clear_part(picker, ComboboxClearProps::default(), cx)
                            .map(|clear| button(clear.aria_label("Clear")).child("×")),
                    )
                    .child(button(Combobox::trigger(picker, cx).aria_label("Show frameworks")).child("▾")),
            )
            .children(content)
    }

    fn country(&self, cx: &App) -> impl IntoElement {
        let picker = &self.country;
        let content = Combobox::content(picker, ComboboxContentProps::default(), cx).map(|content| {
            let items = content.state().items.clone();
            panel(content).overflow_y_scroll().child(
                Combobox::list(picker, cx).flex().flex_col().children(items.iter().filter_map(|node| match node {
                    ListNode::Item(country) => Some(item(picker, &country.value, &country.label, cx)),
                    _ => None,
                })),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .items_start()
            .child(Combobox::label(picker, cx).aria_label("Country").child(text!("Country")))
            .child(field(Combobox::input(picker, cx)))
            .children(content)
    }

    fn labels(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let picker = &self.labels;
        let value = Combobox::value(picker, ComboboxValueProps::default(), cx);
        let chips = value.value.as_slice().iter().zip(value.labels.iter()).map(|(chosen, label)| {
            Combobox::chip(picker, chosen.clone(), cx)
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .rounded_md()
                .bg(rgb(0xe5e7eb))
                .child(text!(label.clone()))
                .child(
                    Combobox::chip_remove(picker, chosen.clone(), cx)
                        .aria_label(SharedString::from(format!("Remove {label}")))
                        .child("×"),
                )
        });
        let create = Self::create_label(cx.weak_entity(), picker.clone());
        let names = self.label_names.clone();
        let content = Combobox::content(picker, ComboboxContentProps::default(), cx).map(|content| {
            let created = Combobox::create(picker, create, cx).map(|option| {
                let query = option.state().query.clone();
                option
                    .with(|el, state| el.bg(if state.highlighted { rgb(0xe5e7eb) } else { rgb(0xffffff) }))
                    .aria_label(SharedString::from(format!("Create {query}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .child(text!(format!("Create “{query}”")))
            });
            panel(content).overflow_y_scroll().child(
                Combobox::list(picker, cx)
                    .flex()
                    .flex_col()
                    .children(names.iter().map(|name| item(picker, name, name, cx)))
                    .children(created),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .items_start()
            .child(Combobox::label(picker, cx).aria_label("Labels").child(text!("Labels")))
            .child(
                Combobox::chips(picker, cx)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .w(px(280.))
                    .px_1()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0xd1d5db))
                    .children(chips)
                    .child(Combobox::input(picker, cx).min_w(px(60.)).flex_1()),
            )
            .children(content)
    }
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_start()
            .gap_6()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(self.framework(cx))
            .child(self.country(cx))
            .child(self.labels(cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(860.), px(420.)), cx);
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
