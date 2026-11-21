//! Toasts of each type, one with an action and a cancel button, one that
//! stays until it's dismissed, a promise's, and the stack they make at the
//! bottom of the window.

use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, WindowBounds, WindowOptions, div, ease_in_out, px, rgb, size,
};
use ui_sdk_kernel::ActionOutcome;
use ui_sdk_motion::{PresenceTiming, Timing};
use ui_sdk_platform::live_regions;
use ui_sdk_toast::{
    PromiseMessage, ToastAction, ToastActionProps, ToastCancel, ToastCloseProps, ToastItem,
    ToastOptions, ToastPromise, ToastRegion, ToastRegionOptions, ToastType, Toaster,
    ToasterOptions, create_toaster,
};

struct Example {
    toaster: Toaster,
    region: Entity<ToastRegion>,
    uploads: usize,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        ui_sdk_focus::bind_tab_keys(cx);
        let toaster = create_toaster(ToasterOptions::default(), cx);
        let options = ToastRegionOptions::default()
            .toaster(toaster.clone())
            .max_visible(4)
            .timing(PresenceTiming::new(Timing::ms(200).easing(ease_in_out)));
        let region = cx.new(|cx| ToastRegion::new(options, window, cx));
        Self {
            toaster,
            region,
            uploads: 0,
        }
    }

    fn archive(&mut self, cx: &mut App) {
        let toaster = self.toaster.clone();
        let undo = ToastAction::new("Undo").on_action(move |_event, _details, _window, cx| {
            toaster.success("Message restored", cx);
            ActionOutcome::Done
        });
        let options = ToastOptions::from("Message archived")
            .action(undo)
            .cancel(ToastCancel::new("Dismiss"))
            .on_dismiss(|details, _cx| log::info!("closed ({})", details.reason.as_str()));
        self.toaster.show(options, cx);
    }

    fn remind(&mut self, cx: &mut App) {
        let options = ToastOptions::from("Session expires soon")
            .r#type(ToastType::Warning)
            .duration(Duration::MAX)
            .action(ToastAction::new("Got it"));
        self.toaster.show(options, cx);
    }

    fn upload(&mut self, cx: &mut App) {
        self.uploads += 1;
        let fails = self.uploads % 2 == 0;
        let wait = cx.background_executor().timer(Duration::from_millis(1500));
        let work = async move {
            wait.await;
            if fails {
                Err(SharedString::from("Can't reach the server"))
            } else {
                Ok(12)
            }
        };
        let messages = ToastPromise::new("Uploading photos")
            .success(PromiseMessage::with(|count: &usize| {
                ToastOptions::from("Photos uploaded").description(format!("{count} photos"))
            }))
            .error(PromiseMessage::with(|error: &SharedString| {
                ToastOptions::from("Upload failed").description(error.clone())
            }));
        self.toaster.promise(work, messages, cx);
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

fn accent(r#type: ToastType) -> Hsla {
    match r#type {
        ToastType::Success => rgb(0x16a34a).into(),
        ToastType::Error => rgb(0xdc2626).into(),
        ToastType::Warning => rgb(0xd97706).into(),
        ToastType::Info => rgb(0x2563eb).into(),
        ToastType::Loading => rgb(0x9ca3af).into(),
        ToastType::Default => rgb(0x111827).into(),
    }
}

// One toast: at the bottom of the stack, peeking out behind the newest while
// it's collapsed, spread out by the heights in front of it while expanded.
fn card(
    region: &Entity<ToastRegion>,
    item: &ToastItem,
    front: Pixels,
    cx: &App,
) -> impl IntoElement {
    let toast = &item.toast;
    let id = toast.id.clone();
    let gap = px(8.);
    let title = ToastRegion::title(region, id.clone(), cx)
        .text_sm()
        .child(toast.title.clone().unwrap_or_default());
    let description = toast.description.clone().map(|description| {
        ToastRegion::description(region, id.clone(), cx)
            .text_xs()
            .text_color(rgb(0x6b7280))
            .child(description)
    });
    let action = toast.action.as_ref().map(|action| {
        ToastRegion::action(region, id.clone(), ToastActionProps::default(), cx)
            .with(|el, state| el.opacity(if state.pending { 0.5 } else { 1. }))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x111827))
            .text_color(rgb(0xffffff))
            .text_xs()
            .child(action.label.clone())
    });
    let close = match &toast.cancel {
        Some(cancel) => ToastRegion::close(
            region,
            id.clone(),
            ToastCloseProps::default().cancel(cancel),
            cx,
        )
        .aria_label(cancel.label.clone())
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .text_xs()
        .child(cancel.label.clone()),
        None => ToastRegion::close(region, id.clone(), ToastCloseProps::default(), cx)
            .aria_label("Dismiss")
            .px_1()
            .text_color(rgb(0x6b7280))
            .child("×"),
    };
    ToastRegion::root(region, id, cx)
        .with(move |el, state| {
            let index = state.index as f32;
            let lift = if state.expanded {
                state.offset.unwrap_or(px(0.)) + gap * index
            } else {
                px(12.) * index
            };
            let inset = if state.expanded {
                px(0.)
            } else {
                px(8.) * index
            };
            let swipe = state.swipe.unwrap_or_default();
            let el = el
                .absolute()
                .left(inset + swipe.x)
                .right(inset - swipe.x)
                .bottom(lift - swipe.y)
                .opacity(state.presence.progress)
                .border_l_4()
                .border_color(accent(state.r#type));
            if state.front || state.expanded {
                el
            } else {
                el.h(front).overflow_hidden()
            }
        })
        .flex()
        .items_start()
        .gap_3()
        .p_3()
        .rounded_md()
        .bg(rgb(0xffffff))
        .shadow_md()
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(title)
                .children(description),
        )
        .children(action)
        .child(close)
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let event =
            button("event", "Create event").on_click(cx.listener(|this, _, _window, cx| {
                this.toaster.show(
                    ToastOptions::from("Event created").description("Friday at 10:00 AM"),
                    cx,
                );
            }));
        let save = button("save", "Save").on_click(cx.listener(|this, _, _window, cx| {
            this.toaster.success("Changes saved", cx);
        }));
        let send = button("send", "Send").on_click(cx.listener(|this, _, _window, cx| {
            this.toaster.error("Can't reach the server", cx);
        }));
        let archive = button("archive", "Archive")
            .on_click(cx.listener(|this, _, _window, cx| this.archive(cx)));
        let remind = button("remind", "Remind")
            .on_click(cx.listener(|this, _, _window, cx| this.remind(cx)));
        let upload = button("upload", "Upload")
            .on_click(cx.listener(|this, _, _window, cx| this.upload(cx)));
        let dismiss =
            button("dismiss", "Dismiss all").on_click(cx.listener(|this, _, _window, cx| {
                this.toaster.dismiss(None, cx);
            }));
        let cx: &App = cx;
        let region = ToastRegion::region(&self.region, cx);
        let state = region.state().clone();
        let front = state.front_height.unwrap_or(px(0.));
        let height = if state.expanded {
            state.stack_height + px(8.) * state.count.saturating_sub(1) as f32
        } else {
            front
        };
        // Back to front, so the newest paints over the ones peeking behind it.
        let mut toasts = state.toasts.clone();
        toasts.sort_by_key(|item| std::cmp::Reverse(item.index));
        let list = ToastRegion::list(&self.region, cx)
            .relative()
            .size_full()
            .children(
                toasts
                    .iter()
                    .map(|item| card(&self.region, item, front, cx)),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .gap_2()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(event)
                    .child(save)
                    .child(send)
                    .child(archive)
                    .child(remind)
                    .child(upload)
                    .child(dismiss),
            )
            .child(region.w(px(320.)).h(height).m_4().child(list))
            .child(live_regions(window))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(720.), px(480.)), cx);
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
