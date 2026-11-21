//! One-time code inputs: six digits in two groups, in a form that Enter
//! submits; eight letters and digits; a custom hexadecimal pattern, shown in
//! capitals; and a disabled one.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_forms::{Form, FormOptions};
use ui_sdk_otp_input::{OtpInput, OtpInputOptions, OtpInputPattern, read_pattern};

struct Example {
    code: Entity<OtpInput>,
    recovery: Entity<OtpInput>,
    device: Entity<OtpInput>,
    disabled: Entity<OtpInput>,
    form: Form,
    error: Option<SharedString>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let code = cx.new(|cx| {
            OtpInput::new(
                OtpInputOptions::default()
                    .name("code")
                    .required(true)
                    .on_value_change(|code, details, _window, _cx| {
                        log::info!("code {code} ({})", details.reason.as_str());
                    })
                    .on_value_commit(|code, details, _window, _cx| {
                        log::info!("complete {code} ({})", details.reason.as_str());
                    }),
                window,
                cx,
            )
        });
        let recovery = cx.new(|cx| {
            OtpInput::new(
                OtpInputOptions::default()
                    .max_length(8)
                    .pattern(OtpInputPattern::Alphanumeric),
                window,
                cx,
            )
        });
        let device = cx.new(|cx| {
            OtpInput::new(
                OtpInputOptions::default().pattern(read_pattern("[0-9A-Fa-f]")),
                window,
                cx,
            )
        });
        let disabled = cx.new(|cx| {
            OtpInput::new(
                OtpInputOptions::default()
                    .default_value("123456")
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
        Self {
            code,
            recovery,
            device,
            disabled,
            form,
            error: None,
        }
    }
}

fn slot(otp: &Entity<OtpInput>, index: usize, capitals: bool, cx: &App) -> impl IntoElement {
    OtpInput::slot(otp, index, cx)
        .with(move |el, state| {
            let shown = state.char.map(|char| {
                if capitals {
                    char.to_ascii_uppercase()
                } else {
                    char
                }
            });
            el.border_color(if state.active { rgb(0x2563eb) } else { rgb(0xd1d5db) })
                .when(state.disabled, |el| el.text_color(rgb(0x9ca3af)))
                .when_some(shown, |el, char| el.child(char.to_string()))
                .when(state.has_fake_caret, |el| {
                    el.child(div().w(px(1.)).h(px(18.)).bg(rgb(0x111827)))
                })
        })
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .size(px(36.))
        .border_1()
        .rounded_md()
        .text_lg()
}

fn code(otp: &Entity<OtpInput>, label: &'static str, groups: &[usize], capitals: bool, cx: &App) -> impl IntoElement {
    let mut root = OtpInput::root(otp, cx).flex().items_center().gap_2();
    let mut index = 0;
    for (number, length) in groups.iter().enumerate() {
        if number > 0 {
            root = root.child(
                OtpInput::separator(otp, number - 1, cx)
                    .text_color(rgb(0x6b7280))
                    .child("-"),
            );
        }
        let mut group = OtpInput::group(otp, number, cx).flex().gap_1();
        for _ in 0..*length {
            group = group.child(slot(otp, index, capitals, cx));
            index += 1;
        }
        root = root.child(group);
    }
    root.child(OtpInput::control(otp, cx).aria_label(label))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let form = self
            .form
            .root()
            .flex()
            .flex_col()
            .gap_1()
            .child(code(&self.code, "Verification code", &[3, 3], false, cx))
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
            .child(code(&self.recovery, "Recovery code", &[4, 4], false, cx))
            .child(code(&self.device, "Device code", &[6], true, cx))
            .child(code(&self.disabled, "Disabled code", &[3, 3], false, cx))
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
