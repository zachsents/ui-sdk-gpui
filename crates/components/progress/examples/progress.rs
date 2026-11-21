//! Progress bars: an export at 60%, an indeterminate connection that sweeps
//! a highlight, an upload that buttons advance and reset, step 3 of 5, a
//! download in megabytes, and copied files with a value label.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, Stateful, StatefulInteractiveElement, Styled, Window, WindowBounds,
    WindowOptions, div, px, relative, rgb, size,
};
use ui_sdk_number::{NumberFormatOptions, NumberStyle};
use ui_sdk_progress::{NumberFormat, Progress, ProgressOptions, ProgressState};

const PHOTOS: f64 = 5.;

struct Example {
    export: Entity<Progress>,
    connecting: Entity<Progress>,
    upload: Entity<Progress>,
    uploaded: f64,
    steps: Entity<Progress>,
    download: Entity<Progress>,
    copying: Entity<Progress>,
}

fn upload(uploaded: f64) -> ProgressOptions {
    ProgressOptions::default().value(uploaded / PHOTOS * 100.)
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut progress =
            |options: ProgressOptions| cx.new(|cx| Progress::new(options, window, cx));
        Self {
            export: progress(ProgressOptions::default().value(60.)),
            connecting: progress(ProgressOptions::default()),
            upload: progress(upload(0.)),
            uploaded: 0.,
            steps: progress(
                ProgressOptions::default()
                    .min(1.)
                    .max(5.)
                    .value(3.)
                    .format(NumberFormat::function(|value| format!("Step {value} of 5"))),
            ),
            download: progress(
                ProgressOptions::default().max(120.).value(42.).format(
                    NumberFormatOptions::default()
                        .style(NumberStyle::Unit)
                        .unit("megabyte"),
                ),
            ),
            copying: progress(
                ProgressOptions::default()
                    .max(12.)
                    .value(3.)
                    .get_value_label(|value, details| match value {
                        Some(value) => format!("{value} of {} files, {}", details.max, details.formatted),
                        None => "Counting files".to_string(),
                    }),
            ),
        }
    }

    fn set_uploaded(&mut self, uploaded: f64, cx: &mut Context<Self>) {
        self.uploaded = uploaded.clamp(0., PHOTOS);
        let options = upload(self.uploaded);
        self.upload.update(cx, |progress, cx| progress.set_options(options, cx));
        cx.notify();
    }
}

fn fill(state: &ProgressState) -> gpui::Rgba {
    if state.status.is_complete() {
        rgb(0x16a34a)
    } else {
        rgb(0x2563eb)
    }
}

fn progress(entity: &Entity<Progress>, label: &'static str, cx: &App) -> impl IntoElement {
    let value = Progress::value(entity, cx);
    let text = value.state().text.clone();
    let indicator = Progress::indicator(entity, cx)
        .with(|el, state| el.bg(fill(state)))
        .absolute()
        .top_0()
        .h_full()
        .rounded_full();
    // Indeterminate, the indicator has no width: a short highlight sweeps the
    // track, or with reduced motion, a still fill covers it.
    let indicator = if !indicator.state().status.is_indeterminate() {
        indicator.into_any_element()
    } else if cx.reduce_motion() {
        indicator.w_full().opacity(0.4).into_any_element()
    } else {
        indicator
            .w(relative(0.3))
            .with_animation(
                "sweep",
                Animation::new(Duration::from_millis(1400)).repeat(),
                |el, delta| el.left(relative(delta * 1.3 - 0.3)),
            )
            .into_any_element()
    };
    Progress::root(entity, cx)
        .flex()
        .flex_col()
        .gap_1()
        .w(px(260.))
        .child(
            div()
                .flex()
                .justify_between()
                .child(Progress::label(entity, cx).aria_label(label).child(label))
                .child(value.text_color(rgb(0x4b5563)).child(text)),
        )
        .child(
            Progress::track(entity, cx)
                .relative()
                .w_full()
                .h(px(8.))
                .rounded_full()
                .overflow_hidden()
                .bg(rgb(0xe5e7eb))
                .child(indicator),
        )
}

fn button(id: &'static str, label: &'static str, disabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .text_color(if disabled { rgb(0x9ca3af) } else { rgb(0x111827) })
        .child(label)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let done = self.uploaded >= PHOTOS;
        let next = button("next", "Upload next", done).on_click(cx.listener(|this, _, _window, cx| {
            if this.uploaded < PHOTOS {
                this.set_uploaded(this.uploaded + 1., cx);
            }
        }));
        let reset = button("reset", "Reset", false).on_click(cx.listener(|this, _, _window, cx| {
            this.set_uploaded(0., cx);
        }));
        let cx: &App = cx;
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_5()
            .p_6()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(progress(&self.export, "Exporting", cx))
            .child(progress(&self.connecting, "Connecting", cx))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_4()
                    .child(progress(&self.upload, "Uploading photos", cx))
                    .child(div().flex().gap_2().child(next).child(reset)),
            )
            .child(progress(&self.steps, "Setup", cx))
            .child(progress(&self.download, "Downloading", cx))
            .child(progress(&self.copying, "Copying files", cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(560.), px(520.)), cx);
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
