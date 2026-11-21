//! Buttons: plain, pending, failing, disabled, focusable while disabled, a
//! link, and a form's submit and reset buttons.

use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_button::{Button, ButtonElement, ButtonOptions, ButtonState, ButtonType};
use ui_sdk_forms::{Form, FormOptions};
use ui_sdk_kernel::{ActionOutcome, Part};
use ui_sdk_platform::live_regions;

struct Example {
    save: Entity<Button>,
    upload: Entity<Button>,
    retry: Entity<Button>,
    archive: Entity<Button>,
    publish: Entity<Button>,
    docs: Entity<Button>,
    submit: Entity<Button>,
    reset: Entity<Button>,
    form: Form,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut button = |options: ButtonOptions, cx: &mut Context<Self>| {
            cx.new(|cx| Button::new(options, window, cx))
        };
        let save = button(
            ButtonOptions::default().action(|_event, details, _window, _cx| {
                log::info!("saved ({})", details.source.as_str());
                ActionOutcome::Done
            }),
            cx,
        );
        let upload = button(
            ButtonOptions::default()
                .pending_announcement("Uploading")
                .action(|_event, _details, _window, cx| {
                    let wait = cx.background_executor().timer(Duration::from_millis(1500));
                    ActionOutcome::wait(async move {
                        wait.await;
                        Ok::<(), String>(())
                    })
                }),
            cx,
        );
        let retry = button(
            ButtonOptions::default().action(|_event, _details, _window, cx| {
                let wait = cx.background_executor().timer(Duration::from_millis(800));
                ActionOutcome::wait(async move {
                    wait.await;
                    Err::<(), _>("Can't reach the server")
                })
            }),
            cx,
        );
        let archive = button(ButtonOptions::default().disabled(true), cx);
        let publish = button(
            ButtonOptions::default()
                .disabled(true)
                .focusable_when_disabled(true),
            cx,
        );
        let docs = button(
            ButtonOptions::default()
                .element(ButtonElement::Link)
                .action(|_event, _details, _window, _cx| {
                    log::info!("opened docs");
                    ActionOutcome::Done
                }),
            cx,
        );
        let submit = button(ButtonOptions::default().r#type(ButtonType::Submit), cx);
        let reset = button(ButtonOptions::default().r#type(ButtonType::Reset), cx);
        let form = Form::new(FormOptions::default().on_submit(|data, _details, _window, _cx| {
            log::info!("submitted {}", data.to_url_encoded());
        }));
        Self {
            save,
            upload,
            retry,
            archive,
            publish,
            docs,
            submit,
            reset,
            form,
        }
    }
}

fn background(state: &ButtonState) -> gpui::Rgba {
    if state.disabled {
        rgb(0xe5e7eb)
    } else if state.pending {
        rgb(0x93c5fd)
    } else if state.error.is_some() {
        rgb(0xfecaca)
    } else {
        rgb(0xdbeafe)
    }
}

fn styled(entity: &Entity<Button>, label: &'static str, cx: &App) -> Part<ButtonState> {
    Button::root(entity, cx)
        .aria_label(SharedString::from(label))
        .with(|el, state| el.bg(background(state)))
        .px_3()
        .py_1()
        .rounded_md()
        .child(label)
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let retry_label = if self.retry.read(cx).error().is_some() {
            "Try again"
        } else {
            "Retry"
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(styled(&self.save, "Save", cx))
                    .child(styled(&self.upload, "Upload", cx))
                    .child(styled(&self.retry, retry_label, cx))
                    .child(styled(&self.archive, "Archive", cx))
                    .child(styled(&self.publish, "Publish", cx))
                    .child(styled(&self.docs, "Docs", cx).underline()),
            )
            .child(
                self.form
                    .root()
                    .flex()
                    .gap_2()
                    .child(styled(&self.submit, "Subscribe", cx))
                    .child(styled(&self.reset, "Reset", cx)),
            )
            .child(live_regions(window))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(560.), px(160.)), cx);
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
