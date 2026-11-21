//! Autocompletes: one that suggests a fruit from suggestions it renders,
//! with a clear button and a trigger; one that completes a berry inline
//! from suggestions as data, marking what matched; and a search that opens
//! on focus with recent searches and searches as you choose.

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, FontWeight, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div,
    prelude::FluentBuilder, px, rgb, size, text,
};
use ui_sdk_autocomplete::{
    Autocomplete, AutocompleteClearProps, AutocompleteContentProps, AutocompleteItemProps, AutocompleteItemState,
    AutocompleteOptions, ListData, ListGroup, ListItem, ListNode,
};

const FRUITS: &[&str] = &[
    "Apple",
    "Banana",
    "Cherry",
    "Grape",
    "Mango",
    "Orange",
    "Peach",
    "Pear",
    "Pineapple",
];

const BERRIES: &[&str] = &[
    "Blackberry",
    "Blueberry",
    "Boysenberry",
    "Cranberry",
    "Elderberry",
    "Gooseberry",
    "Raspberry",
    "Strawberry",
];

const TOPICS: &[&str] = &["Go", "Kotlin", "React", "React Native", "Ruby", "Rust", "Swift", "TypeScript"];

const RECENT: &[&str] = &["Rust", "Kotlin"];

type Field = Entity<Autocomplete>;

struct Example {
    fruit: Field,
    berry: Field,
    search: Field,
    // What the search searched for last.
    searched: Option<SharedString>,
}

fn data(values: &[&str]) -> ListData<SharedString> {
    values
        .iter()
        .map(|value| ListNode::Item(ListItem::new(SharedString::from(*value), *value)))
        .collect()
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fruit = cx.new(|cx| Autocomplete::new(AutocompleteOptions::default().placeholder("Fruit"), window, cx));
        let berry = cx.new(|cx| {
            Autocomplete::new(
                AutocompleteOptions::default()
                    .items(data(BERRIES))
                    .inline_complete(true)
                    .placeholder("Berry"),
                window,
                cx,
            )
        });
        let recent: ListData<SharedString> = [ListNode::Group(
            ListGroup::new(
                RECENT
                    .iter()
                    .map(|value| ListItem::new(SharedString::from(*value), *value)),
            )
            .label("Recent"),
        )]
        .into_iter()
        .collect();
        let example = cx.weak_entity();
        let search = cx.new(|cx| {
            Autocomplete::new(
                AutocompleteOptions::default()
                    .items(data(TOPICS))
                    .empty_items(recent)
                    .open_on_focus(true)
                    .auto_highlight(true)
                    .submit_on_select(true)
                    .on_submit(move |value, details, _window, cx| {
                        details.prevent_default();
                        let value = value.clone();
                        example
                            .update(cx, |example, cx| {
                                example.searched = Some(value);
                                cx.notify();
                            })
                            .ok();
                    })
                    .placeholder("Search"),
                window,
                cx,
            )
        });
        Self {
            fruit,
            berry,
            search,
            searched: None,
        }
    }
}

fn input<E: Styled>(input: E) -> E {
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

fn option_color(state: &AutocompleteItemState) -> gpui::Rgba {
    if state.highlighted && !state.disabled {
        rgb(0xe5e7eb)
    } else {
        rgb(0xffffff)
    }
}

// A suggestion, showing `shown`.
fn suggestion(field: &Field, props: AutocompleteItemProps, shown: impl IntoElement, cx: &App) -> AnyElement {
    let name = props.value.clone();
    Autocomplete::item(field, props, cx)
        .with(|el, state| el.bg(option_color(state)))
        .aria_label(name)
        .px_2()
        .py_1()
        .rounded_md()
        .child(shown)
        .into_any_element()
}

// A suggestion's label, with what the text matched in bold.
fn marked(field: &Field, value: &SharedString, label: &SharedString, cx: &App) -> impl IntoElement {
    let part = Autocomplete::item_text(field, value.clone(), label.clone(), cx);
    let segments = part.state().segments.clone();
    part.flex().children(segments.into_iter().map(|segment| {
        div()
            .when(segment.matched, |piece| piece.font_weight(FontWeight::BOLD))
            .child(text!(segment.text))
    }))
}

impl Example {
    fn fruit(&self, cx: &App) -> impl IntoElement {
        let field = &self.fruit;
        let content = Autocomplete::content(field, AutocompleteContentProps::default(), cx).map(|content| {
            let empty = Autocomplete::empty(field, cx);
            let nothing = empty.state().empty;
            panel(content)
                .overflow_y_scroll()
                .child(empty.when(nothing, |el| el.px_2().py_1().child(text!("No matches"))))
                .child(Autocomplete::list(field, cx).flex().flex_col().children(FRUITS.iter().map(|fruit| {
                    suggestion(field, AutocompleteItemProps::new(*fruit), text!(*fruit), cx)
                })))
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .items_start()
            .child(Autocomplete::label(field, cx).aria_label("Fruit").child(text!("Fruit")))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_1()
                    .child(input(Autocomplete::input(field, cx)))
                    .children(
                        Autocomplete::clear_part(field, AutocompleteClearProps::default(), cx)
                            .map(|clear| button(clear.aria_label("Clear")).child("×")),
                    )
                    .child(button(Autocomplete::trigger(field, cx).aria_label("Show fruits")).child("▾")),
            )
            .children(content)
    }

    fn berry(&self, cx: &App) -> impl IntoElement {
        let field = &self.berry;
        let content = Autocomplete::content(field, AutocompleteContentProps::default(), cx).map(|content| {
            let items = content.state().items.clone();
            panel(content).overflow_y_scroll().child(
                Autocomplete::list(field, cx)
                    .flex()
                    .flex_col()
                    .children(items.iter().filter_map(|node| match node {
                        ListNode::Item(berry) => Some(suggestion(
                            field,
                            AutocompleteItemProps::new(berry.value.clone()).label(berry.label.clone()),
                            marked(field, &berry.value, &berry.label, cx),
                            cx,
                        )),
                        _ => None,
                    })),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .items_start()
            .child(Autocomplete::label(field, cx).aria_label("Berry").child(text!("Berry")))
            .child(input(Autocomplete::input(field, cx)))
            .children(content)
    }

    fn search(&self, cx: &App) -> impl IntoElement {
        let field = &self.search;
        let content = Autocomplete::content(field, AutocompleteContentProps::default(), cx).map(|content| {
            let items = content.state().items.clone();
            let topic = |item: &ListItem<SharedString>, group: Option<&SharedString>| {
                let mut props = AutocompleteItemProps::new(item.value.clone()).label(item.label.clone());
                if let Some(group) = group {
                    props = props.group(group.clone());
                }
                suggestion(field, props, text!(item.label.clone()), cx)
            };
            panel(content).overflow_y_scroll().child(
                Autocomplete::list(field, cx)
                    .flex()
                    .flex_col()
                    .children(items.iter().filter_map(|node| match node {
                        ListNode::Item(item) => Some(topic(item, None)),
                        ListNode::Group(group) => {
                            let label = group.label.clone().unwrap_or_default();
                            Some(
                                Autocomplete::group(field, &label, cx)
                                    .flex()
                                    .flex_col()
                                    .child(
                                        Autocomplete::group_label(field, &label, cx)
                                            .aria_label(label.clone())
                                            .px_2()
                                            .text_xs()
                                            .text_color(rgb(0x6b7280))
                                            .child(text!(label.clone())),
                                    )
                                    .children(group.items.iter().map(|item| topic(item, Some(&label))))
                                    .into_any_element(),
                            )
                        }
                        _ => None,
                    })),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .items_start()
            .child(Autocomplete::label(field, cx).aria_label("Search").child(text!("Search")))
            .child(input(Autocomplete::input(field, cx)))
            .children(content)
            .children(self.searched.clone().map(|searched| {
                div()
                    .flex()
                    .gap_1()
                    .child(text!("Last search"))
                    .child(text!(searched))
            }))
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
            .child(self.fruit(cx))
            .child(self.berry(cx))
            .child(self.search(cx))
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
