//! Token inputs: a prompt with chips that send on Enter, tokens added from
//! buttons, one pending while it loads; one that selects a token before
//! removing it and sends with Command or Control+Enter; and a read-only one.

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px,
    rgb, size,
};
use ui_sdk_kernel::ChangeOptions;
use ui_sdk_platform::live_regions;
use ui_sdk_token_input::{
    InsertOptions, Token, TokenDeletion, TokenInput, TokenInputOptions, TokenInputSubmitShortcut,
    TokenStatus, TokenUpdate, default_codec, to_text,
};

struct Example {
    prompt: Entity<TokenInput>,
    note: Entity<TokenInput>,
    draft: Entity<TokenInput>,
    sent: SharedString,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let prompt = cx.new(|cx| {
            TokenInput::new(
                TokenInputOptions::default()
                    .default_value(vec![
                        "Explain ".into(),
                        Token::new("file", "src/routes.ts", "routes.ts").into(),
                    ])
                    .on_submit(move |value, _details, window, cx| {
                        let sent: SharedString = to_text(value, &default_codec()).into();
                        this.update(cx, |example, cx| {
                            example.sent = sent;
                            example.prompt.update(cx, |input, cx| {
                                input.clear(ChangeOptions::default(), window, cx);
                            });
                            cx.notify();
                        })
                        .ok();
                    }),
                window,
                cx,
            )
        });
        let this = cx.entity().downgrade();
        let note = cx.new(|cx| {
            TokenInput::new(
                TokenInputOptions::default()
                    .default_value(vec![
                        "Ask ".into(),
                        Token::new("mention", "ada", "Ada").into(),
                        " about ".into(),
                        Token::new("file", "src/app.ts", "app.ts").into(),
                    ])
                    .token_deletion(TokenDeletion::Select)
                    .submit_shortcut(TokenInputSubmitShortcut::ModEnter)
                    .on_submit(move |value, _details, _window, cx| {
                        let sent: SharedString = to_text(value, &default_codec()).into();
                        this.update(cx, |example, cx| {
                            example.sent = sent;
                            cx.notify();
                        })
                        .ok();
                    }),
                window,
                cx,
            )
        });
        let draft = cx.new(|cx| {
            TokenInput::new(
                TokenInputOptions::default()
                    .default_value(vec![
                        "Review ".into(),
                        Token::new("file", "README.md", "README.md").into(),
                    ])
                    .read_only(true),
                window,
                cx,
            )
        });
        Self {
            prompt,
            note,
            draft,
            sent: SharedString::default(),
        }
    }
}

// A chip with the token's label and a button that removes it.
fn chip(input: &Entity<TokenInput>, token: &Token, _window: &mut Window, cx: &mut App) -> AnyElement {
    let id = token.id.clone().unwrap_or_default();
    TokenInput::token(input, &id, cx)
        .with(|el, state| {
            el.bg(if state.selected { rgb(0xbfdbfe) } else { rgb(0xe5e7eb) })
                .when(state.pending, |el| el.opacity(0.5))
        })
        .flex()
        .items_center()
        .gap_1()
        .px_1()
        .rounded_sm()
        .child(token.label.clone())
        .child(
            TokenInput::token_remove(input, &id, cx)
                .with(|el, state| el.when(state.read_only, |el| el.invisible()))
                .px_0p5()
                .cursor_pointer()
                .child("×"),
        )
        .into_any_element()
}

fn button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .cursor_pointer()
        .child(label)
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = |input: &Entity<TokenInput>, label: &'static str, cx: &App| {
            TokenInput::editor(input, cx)
                .aria_label(SharedString::from(label))
                .w(px(360.))
                .min_h(px(56.))
                .max_h(px(140.))
                .p_2()
                .border_1()
                .border_color(rgb(0xd1d5db))
                .rounded_md()
                .with(|el, state| el.when(state.read_only, |el| el.bg(rgb(0xf9fafb))))
        };
        let prompt = self.prompt.clone();
        let attach = self.prompt.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_3()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(
                editor(&self.prompt, "Prompt", cx)
                    .placeholder("Ask")
                    .render_token(chip),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(button("mention", "Mention").on_click(move |_, window, cx| {
                        prompt.update(cx, |input, cx| {
                            input.insert(Token::new("mention", "ada", "Ada"), InsertOptions::default(), window, cx);
                        });
                    }))
                    .child(button("attach", "Attach").on_click(move |_, window, cx| {
                        let wait = cx.background_executor().timer(Duration::from_millis(1200));
                        attach.update(cx, |input, cx| {
                            let resolve = async move {
                                wait.await;
                                Ok(Some(TokenUpdate {
                                    label: Some("notes.md".into()),
                                    ..TokenUpdate::default()
                                }))
                            };
                            input.insert(
                                Token::new("file", "docs/notes.md", "notes.md"),
                                InsertOptions::default().resolve(resolve),
                                window,
                                cx,
                            );
                        });
                    })),
            )
            .child(
                editor(&self.note, "Note", cx)
                    .placeholder("Note")
                    .style_host(|host, state| {
                        host.px_1()
                            .rounded_sm()
                            .bg(if state.selected { rgb(0xfde68a) } else { rgb(0xfef3c7) })
                            .when(state.token.status == Some(TokenStatus::Unresolved), |host| {
                                host.border_1().border_color(rgb(0xf59e0b))
                            })
                    }),
            )
            .child(editor(&self.draft, "Draft", cx).render_token(chip))
            .child(div().text_sm().child(self.sent.clone()))
            .child(live_regions(window))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(480.), px(520.)), cx);
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
