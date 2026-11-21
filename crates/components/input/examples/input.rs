//! Inputs: in a group with text around it and a clear button, in a form with
//! labels and native constraints, controlled, a textarea that grows and
//! submits with Command or Control+Enter, and disabled and read-only ones.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb,
    size,
};
use ui_sdk_forms::{Form, FormOptions, Label, LabelOptions};
use ui_sdk_input::{
    Input, InputGroup, InputGroupOptions, InputHintKind, InputHintOptions, InputOptions, Textarea,
    TextareaOptions, TextareaSubmitShortcut, TextFieldKind, input_hints,
};
use ui_sdk_kernel::Part;

struct Example {
    group: Entity<InputGroup>,
    website: Entity<Input>,
    email: Entity<Input>,
    code: Entity<Input>,
    message: Entity<Textarea>,
    name: Entity<Input>,
    name_value: SharedString,
    disabled: Entity<Input>,
    read_only: Entity<Input>,
    form: Form,
    labels: Vec<Label>,
    error: Option<SharedString>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let group = cx.new(|cx| InputGroup::new(InputGroupOptions::default(), window, cx));
        let website = cx.new(|cx| {
            Input::new(
                InputOptions::default()
                    .placeholder("example")
                    .on_value_change(|value, details, _window, _cx| {
                        log::info!("website {value} ({})", details.reason.as_str());
                    }),
                window,
                cx,
            )
        });
        let email = cx.new(|cx| {
            Input::new(
                InputOptions::default()
                    .kind(TextFieldKind::Email)
                    .name("email")
                    .required(true)
                    .placeholder("you@example.com")
                    .input_hints(input_hints(InputHintKind::Email, InputHintOptions::default())),
                window,
                cx,
            )
        });
        let code = cx.new(|cx| {
            Input::new(
                InputOptions::default()
                    .name("code")
                    .pattern("[A-Z]{3}-[0-9]{3}")
                    .placeholder("ABC-123"),
                window,
                cx,
            )
        });
        let message = cx.new(|cx| {
            Textarea::new(
                TextareaOptions::default()
                    .name("message")
                    .rows(2)
                    .auto_resize(true)
                    .submit_shortcut(TextareaSubmitShortcut::ModEnter),
                window,
                cx,
            )
        });
        let this = cx.entity().downgrade();
        let name = cx.new(|cx| {
            Input::new(
                InputOptions::default()
                    .value("Hollis Grant")
                    .on_value_change(move |value, _details, _window, cx| {
                        let value = value.clone();
                        this.update(cx, |example, cx| {
                            example.name_value = value;
                            cx.notify();
                        })
                        .ok();
                    }),
                window,
                cx,
            )
        });
        let disabled = cx.new(|cx| {
            Input::new(
                InputOptions::default()
                    .default_value("hollis@example.com")
                    .disabled(true),
                window,
                cx,
            )
        });
        let read_only = cx.new(|cx| {
            Input::new(
                InputOptions::default()
                    .default_value("example.com/hollis")
                    .read_only(true),
                window,
                cx,
            )
        });
        let reports = cx.entity().downgrade();
        let submits = cx.entity().downgrade();
        let form = Form::new(
            FormOptions::default()
                .on_submit(move |data, _details, _window, cx| {
                    log::info!("submitted {}", data.to_url_encoded());
                    submits
                        .update(cx, |example, cx| {
                            example.error = None;
                            cx.notify();
                        })
                        .ok();
                })
                .on_report(move |report, _window, cx| {
                    let message = report.message.clone();
                    reports
                        .update(cx, |example, cx| {
                            example.error = Some(message);
                            cx.notify();
                        })
                        .ok();
                }),
        );
        let labels = ["Email", "Code", "Message"]
            .into_iter()
            .map(|text| Label::new(LabelOptions::default().text(text)))
            .collect();
        Self {
            group,
            website,
            email,
            code,
            message,
            name,
            name_value: "Hollis Grant".into(),
            disabled,
            read_only,
            form,
            labels,
            error: None,
        }
    }
}

fn boxed<S: 'static>(part: Part<S>) -> Part<S> {
    part.w(px(260.))
        .px_2()
        .py_1()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .rounded_md()
        .focus(|style| style.border_color(rgb(0x2563eb)))
}

fn labeled(label: &Label, text: &'static str, field: impl IntoElement) -> impl IntoElement {
    label
        .root()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().text_color(rgb(0x4b5563)).child(text))
        .child(field)
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name_value = self.name_value.clone();
        self.name.update(cx, |input, cx| {
            input.set_controlled_value(Some(name_value.clone()), window, cx)
        });
        let clear = InputGroup::clear_part(&self.group, cx).map(|part| {
            part.aria_label("Clear website")
                .px_1()
                .rounded_sm()
                .text_color(rgb(0x6b7280))
                .child("×")
        });
        let group = InputGroup::group(&self.group, cx)
            .flex()
            .items_center()
            .gap_1()
            .w(px(260.))
            .px_2()
            .py_1()
            .border_1()
            .border_color(rgb(0xd1d5db))
            .rounded_md()
            .text_color(rgb(0x6b7280))
            .child("https://")
            .child(
                Input::root(&self.website, cx)
                    .aria_label("Website")
                    .flex_1()
                    .text_color(rgb(0x111827)),
            )
            .child(".com")
            .children(clear);
        let message = boxed(Textarea::root(&self.message, cx))
            .min_h(px(48.))
            .max_h(px(120.));
        let form = self
            .form
            .root()
            .flex()
            .flex_col()
            .gap_3()
            .child(labeled(&self.labels[0], "Email", boxed(Input::root(&self.email, cx))))
            .child(labeled(&self.labels[1], "Code", boxed(Input::root(&self.code, cx))))
            .child(labeled(&self.labels[2], "Message", message))
            .when_some(self.error.clone(), |form, error| {
                form.child(div().text_sm().text_color(rgb(0xb91c1c)).child(error))
            });
        let count = format!("{}/40", self.name_value.chars().count());
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(group)
            .child(form)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(boxed(Input::root(&self.name, cx)).aria_label("Name"))
                    .child(div().text_sm().text_color(rgb(0x6b7280)).child(count)),
            )
            .child(
                boxed(Input::root(&self.disabled, cx))
                    .aria_label("Email")
                    .with(|el, state| el.when(state.disabled, |el| el.text_color(rgb(0x9ca3af)))),
            )
            .child(boxed(Input::root(&self.read_only, cx)).aria_label("Profile"))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(360.), px(560.)), cx);
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
