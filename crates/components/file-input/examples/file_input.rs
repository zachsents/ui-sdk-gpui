//! File inputs: attachments, up to five under 10 MB each, browsed, dropped
//! on their zone or pasted into its message, in a form that sends and
//! resets them; one PDF; a folder; and drops anywhere else in the window.

use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled, Window,
    WindowBounds, WindowOptions, div, px, rgb, rgba, size, transparent_black,
};
use ui_sdk_file_input::{
    File, FileInput, FileInputDropzoneProps, FileInputDropzoneTarget, FileInputOptions,
    format_file_size, relative_path,
};
use ui_sdk_forms::{Form, FormOptions};
use ui_sdk_kernel::ChangeOptions;
use ui_sdk_native::{TextField, TextFieldOptions};
use ui_sdk_platform::live_regions;

const MAX_SIZE: u64 = 10_000_000;

struct Example {
    attachments: Entity<FileInput>,
    rejected: Vec<String>,
    message: Entity<TextField>,
    form: Form,
    sent: Option<Vec<SharedString>>,
    resume: Entity<FileInput>,
    folder: Entity<FileInput>,
    anywhere: Entity<FileInput>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let rejecting = this.clone();
        let attachments = cx.new(|cx| {
            FileInput::new(
                FileInputOptions::default()
                    .name("attachments")
                    .multiple(true)
                    .max_files(5)
                    .max_size(MAX_SIZE)
                    .on_reject(move |rejected, _details, _window, cx| {
                        let lines = rejected
                            .iter()
                            .map(|rejection| {
                                let reasons: Vec<&str> =
                                    rejection.reasons.iter().map(|reason| reason.as_str()).collect();
                                format!("{} · {}", rejection.file.name, reasons.join(", "))
                            })
                            .collect();
                        rejecting
                            .update(cx, |example, cx| {
                                example.rejected = lines;
                                cx.notify();
                            })
                            .ok();
                    }),
                window,
                cx,
            )
        });
        let message = cx.new(|cx| {
            TextField::new(
                TextFieldOptions::default()
                    .placeholder("Message")
                    .multi_line(true)
                    .rows(3),
                window,
                cx,
            )
        });
        let sending = this;
        let form = Form::new(FormOptions::default().on_submit(move |data, _details, _window, cx| {
            let names = data
                .get_all("attachments")
                .into_iter()
                .filter_map(|entry| entry.as_file())
                .filter(|file| !file.name.is_empty())
                .map(|file| file.name.clone())
                .collect();
            sending
                .update(cx, |example, cx| {
                    example.sent = Some(names);
                    cx.notify();
                })
                .ok();
        }));
        let resume = cx.new(|cx| {
            FileInput::new(FileInputOptions::default().name("resume").accept(".pdf"), window, cx)
        });
        let folder = cx.new(|cx| FileInput::new(FileInputOptions::default().directory(true), window, cx));
        let anywhere = cx.new(|cx| FileInput::new(FileInputOptions::default().multiple(true), window, cx));
        Self {
            attachments,
            rejected: Vec::new(),
            message,
            form,
            sent: None,
            resume,
            folder,
            anywhere,
        }
    }
}

fn button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .child(label)
}

// One file: its name, or its path inside a folder, its size, and its remove
// button.
fn item(entity: &Entity<FileInput>, file: &File, path: bool, cx: &App) -> impl IntoElement {
    let label = if path {
        relative_path(file)
    } else {
        file.name.clone()
    };
    FileInput::item(entity, file, cx)
        .aria_label(label.clone())
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(transparent_black())
        .focus_visible(|style| style.border_color(rgb(0x2563eb)))
        .child(div().flex_1().child(label))
        .child(div().text_color(rgb(0x6b7280)).child(format_file_size(file.size, None)))
        .child(
            FileInput::item_remove(entity, file, cx)
                .px_1()
                .rounded_sm()
                .hover(|style| style.bg(rgb(0xe5e7eb)))
                .child("×"),
        )
}

fn list(entity: &Entity<FileInput>, label: &'static str, path: bool, cx: &App) -> impl IntoElement {
    let files = entity.read(cx).get_value().to_vec();
    FileInput::list(entity, cx)
        .aria_label(label)
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(160.))
        .overflow_y_scroll()
        .children(files.iter().map(|file| item(entity, file, path, cx)))
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let form = self.form.clone();
        let send = button("send", "Send").on_click(move |_, window, cx| {
            form.request_submit(None, None, window, cx);
        });
        let form = self.form.clone();
        let reset = button("reset", "Reset").on_click(move |_, window, cx| form.reset(window, cx));
        let attachments = self.attachments.clone();
        let clear = button("clear", "Clear").on_click(move |_, window, cx| {
            attachments.update(cx, |input, cx| {
                input.clear(ChangeOptions::default(), window, cx);
            });
        });
        let cx: &App = cx;
        let muted = rgb(0x6b7280);

        let zone = FileInput::dropzone(&self.attachments, "zone", FileInputDropzoneProps::default(), cx)
            .with(|el, state| {
                el.border_color(if state.drag_reject {
                    rgb(0xdc2626)
                } else if state.drag_accept {
                    rgb(0x16a34a)
                } else {
                    rgb(0xd1d5db)
                })
            })
            .border_2()
            .rounded_lg()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                TextField::root(&self.message, cx)
                    .aria_label("Message")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0xd1d5db)),
            )
            .child(
                div().flex().child(
                    FileInput::trigger(&self.attachments, cx)
                        .aria_label("Browse files")
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(0x1f2937))
                        .text_color(rgb(0xffffff))
                        .child("Browse files"),
                ),
            )
            .child(list(&self.attachments, "Attachments", false, cx));

        let sent = self.sent.as_ref().map(|names| {
            let names: Vec<&str> = names.iter().map(|name| name.as_ref()).collect();
            format!("Sent: {}", if names.is_empty() { "nothing".to_string() } else { names.join(", ") })
        });
        let attachments = self
            .form
            .root()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(360.))
            .child(FileInput::root(&self.attachments, cx).child(zone))
            .children(
                self.rejected
                    .iter()
                    .map(|line| div().text_color(rgb(0xdc2626)).child(line.clone())),
            )
            .child(div().flex().gap_2().child(send).child(reset).child(clear))
            .children(sent.map(|sent| div().text_color(muted).child(sent)));

        let resume_file = self.resume.read(cx).get_value().first().cloned();
        let resume = FileInput::root(&self.resume, cx)
            .flex()
            .items_center()
            .gap_2()
            .child(
                FileInput::trigger(&self.resume, cx)
                    .aria_label(if resume_file.is_some() { "Replace PDF" } else { "Choose a PDF" })
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0xf3f4f6))
                    .child(if resume_file.is_some() { "Replace PDF" } else { "Choose a PDF" }),
            )
            .child(
                div().text_color(muted).child(
                    resume_file
                        .map(|file| file.name.clone())
                        .unwrap_or_else(|| "No file".into()),
                ),
            );

        let folder = FileInput::root(&self.folder, cx)
            .flex()
            .flex_col()
            .gap_2()
            .w(px(320.))
            .child(
                div().flex().child(
                    FileInput::trigger(&self.folder, cx)
                        .aria_label("Choose a folder")
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(0xf3f4f6))
                        .child("Choose a folder"),
                ),
            )
            .child(list(&self.folder, "Files", true, cx));

        let dropped = FileInput::root(&self.anywhere, cx)
            .flex()
            .flex_col()
            .gap_2()
            .w(px(320.))
            .child(div().text_color(muted).child("Dropped"))
            .child(list(&self.anywhere, "Dropped", false, cx));

        // Over the whole window, shown while files are over it.
        let overlay = FileInput::dropzone(
            &self.anywhere,
            "window",
            FileInputDropzoneProps::default().target(FileInputDropzoneTarget::Document),
            cx,
        )
        .with(|el, state| el.opacity(if state.dragging { 1. } else { 0. }))
        .absolute()
        .inset_0()
        .border_4()
        .border_color(rgb(0x2563eb))
        .bg(rgba(0x2563eb14));

        div()
            .size_full()
            .relative()
            .flex()
            .gap_8()
            .p_6()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(attachments)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(resume)
                    .child(folder)
                    .child(dropped),
            )
            .child(overlay)
            .child(live_regions(window))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        ui_sdk_focus::bind_tab_keys(cx);
        let bounds = Bounds::centered(None, size(px(860.), px(620.)), cx);
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
