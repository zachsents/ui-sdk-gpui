//! Password inputs: in a form that Enter submits, which hides the password
//! again, with a Caps Lock warning; shown at first; with controlled
//! visibility; and disabled.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb,
    size,
};
use ui_sdk_forms::{Form, FormOptions, Label, LabelOptions};
use ui_sdk_password_input::{PasswordInput, PasswordInputOptions, PasswordInputState};

struct Example {
    password: Entity<PasswordInput>,
    wifi: Entity<PasswordInput>,
    controlled: Entity<PasswordInput>,
    controlled_visible: bool,
    disabled: Entity<PasswordInput>,
    form: Form,
    label: Label,
    error: Option<SharedString>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let password = cx.new(|cx| {
            PasswordInput::new(
                PasswordInputOptions::default()
                    .name("password")
                    .required(true)
                    .min_length(8)
                    .on_visible_change(|visible, details, _window, _cx| {
                        log::info!("visible {visible} ({})", details.reason.as_str());
                    }),
                window,
                cx,
            )
        });
        let wifi = cx.new(|cx| {
            PasswordInput::new(
                PasswordInputOptions::default()
                    .default_value("correct horse battery")
                    .default_visible(true),
                window,
                cx,
            )
        });
        let this = cx.entity().downgrade();
        let controlled = cx.new(|cx| {
            PasswordInput::new(
                PasswordInputOptions::default()
                    .visible(false)
                    .on_visible_change(move |visible, details, _window, cx| {
                        log::info!("visible {visible} ({})", details.reason.as_str());
                        let visible = *visible;
                        this.update(cx, |example, cx| {
                            example.controlled_visible = visible;
                            cx.notify();
                        })
                        .ok();
                    }),
                window,
                cx,
            )
        });
        let disabled = cx.new(|cx| {
            PasswordInput::new(
                PasswordInputOptions::default()
                    .default_value("hunter22")
                    .disabled(true),
                window,
                cx,
            )
        });
        let reports = cx.entity().downgrade();
        let submits = cx.entity().downgrade();
        let form = Form::new(
            FormOptions::default()
                .on_submit(move |data, _details, _window, cx| {
                    log::info!("submitted {} fields", data.len());
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
        Self {
            password,
            wifi,
            controlled,
            controlled_visible: false,
            disabled,
            form,
            label: Label::new(LabelOptions::default().text("Password")),
            error: None,
        }
    }
}

fn field(password: &Entity<PasswordInput>, name: &'static str, cx: &App) -> impl IntoElement {
    let toggle = PasswordInput::toggle(password, cx)
        .aria_label("Show password")
        .with(|el, state: &PasswordInputState| {
            el.text_color(if state.disabled { rgb(0x9ca3af) } else { rgb(0x2563eb) })
                .child(if state.visible { "Hide" } else { "Show" })
        })
        .px_1()
        .rounded_sm()
        .text_sm();
    PasswordInput::root(password, cx)
        .with(|el, state| {
            el.border_color(if state.disabled { rgb(0xe5e7eb) } else { rgb(0xd1d5db) })
        })
        .flex()
        .items_center()
        .gap_2()
        .w(px(280.))
        .px_2()
        .py_1()
        .border_1()
        .rounded_md()
        .child(
            PasswordInput::control(password, cx)
                .aria_label(name)
                .flex_1()
                .with(|el, state| el.when(state.disabled, |el| el.text_color(rgb(0x9ca3af)))),
        )
        .child(toggle)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.controlled_visible;
        self.controlled
            .update(cx, |password, cx| password.set_controlled_visible(Some(visible), cx));
        let caps_lock = self.password.read(cx).state().caps_lock;
        let form = self
            .form
            .root()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                self.label
                    .root()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().text_color(rgb(0x4b5563)).child("Password"))
                    .child(field(&self.password, "Password", cx)),
            )
            .when(caps_lock, |form| {
                form.child(div().text_sm().text_color(rgb(0xb45309)).child("Caps Lock"))
            })
            .when_some(self.error.clone(), |form, error| {
                form.child(div().text_sm().text_color(rgb(0xb91c1c)).child(error))
            });
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(form)
            .child(field(&self.wifi, "Wi-Fi password", cx))
            .child(field(&self.controlled, "PIN", cx))
            .child(field(&self.disabled, "Old password", cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(340.), px(320.)), cx);
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
