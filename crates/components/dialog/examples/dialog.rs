//! Dialogs: a modal one with a backdrop, one a trigger's shortcut opens, a
//! non-modal one, and an alert dialog whose Delete waits for its action.

use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, IntoElement, ParentElement, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, WindowBounds,
    WindowOptions, div, px, rgb, rgba, size, text,
};
use ui_sdk_dialog::{
    AlertDialog, AlertDialogOptions, Dialog, DialogCloseProps, DialogCloseState,
    DialogContentProps, DialogOptions, DialogTriggerProps, DialogTriggerState, Modal,
    ShortcutProps,
};
use ui_sdk_kernel::{ActionOutcome, Part};

struct Example {
    share: Entity<Dialog>,
    contact: Entity<Dialog>,
    notes: Entity<Dialog>,
    delete: Entity<AlertDialog>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let share = cx.new(|cx| Dialog::new(DialogOptions::default(), window, cx));
        let contact = cx.new(|cx| Dialog::new(DialogOptions::default(), window, cx));
        let notes = cx.new(|cx| Dialog::new(DialogOptions::default().modal(Modal::False), window, cx));
        let delete = cx.new(|cx| AlertDialog::new(AlertDialogOptions::default(), window, cx));
        Self {
            share,
            contact,
            notes,
            delete,
        }
    }
}

fn trigger_color(state: &DialogTriggerState) -> gpui::Rgba {
    if state.open { rgb(0xe5e7eb) } else { rgb(0xf3f4f6) }
}

fn trigger(part: Part<DialogTriggerState>, label: &'static str) -> Part<DialogTriggerState> {
    part.with(|el, state| el.bg(trigger_color(state)))
        .flex()
        .gap_2()
        .px_3()
        .py_1()
        .rounded_md()
        .child(text!(label))
}

fn button(part: Part<DialogCloseState>, label: &'static str, busy: &'static str) -> impl IntoElement {
    let pending = part.state().pending;
    part.with(|el, state| {
        el.bg(if state.error.is_some() { rgb(0x991b1b) } else { rgb(0x1f2937) })
    })
    .px_3()
    .py_1()
    .rounded_md()
    .text_color(rgb(0xffffff))
    .child(text!(if pending { busy } else { label }))
}

fn panel(content: Stateful<Div>) -> Stateful<Div> {
    content
        .m_auto()
        .flex()
        .flex_col()
        .gap_2()
        .w(px(280.))
        .p_4()
        .rounded_md()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .bg(rgb(0xffffff))
        .text_color(rgb(0x111827))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let share_backdrop = Dialog::backdrop(&self.share, None, cx).map(|backdrop| backdrop.bg(rgba(0x0000004d)));
        let share = Dialog::content(&self.share, DialogContentProps::default(), cx).map(|content| {
            content.with(|el, _| panel(el))
                .child(Dialog::title(&self.share, cx).aria_label("Share link").child("Share link"))
                .child(Dialog::description(&self.share, cx).aria_label("ui-sdk.dev/d/42").child("ui-sdk.dev/d/42"))
                .child(button(
                    Dialog::close_part(&self.share, "done", DialogCloseProps::default(), cx),
                    "Done",
                    "Done",
                ))
        });

        let contact_trigger = trigger(
            Dialog::trigger(&self.contact, "new", DialogTriggerProps::default().shortcut("mod+j"), cx),
            "New contact",
        )
        .child(Dialog::shortcut(&self.contact, None, ShortcutProps::default(), cx).text_color(rgb(0x6b7280)));
        let contact_backdrop =
            Dialog::backdrop(&self.contact, None, cx).map(|backdrop| backdrop.bg(rgba(0x0000004d)));
        let contact = Dialog::content(&self.contact, DialogContentProps::default(), cx).map(|content| {
            content.with(|el, _| panel(el))
                .child(Dialog::title(&self.contact, cx).aria_label("New contact").child("New contact"))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(button(
                            Dialog::close_part(&self.contact, "save", DialogCloseProps::default().value("save"), cx),
                            "Save",
                            "Save",
                        ))
                        .child(button(
                            Dialog::close_part(&self.contact, "cancel", DialogCloseProps::default(), cx),
                            "Cancel",
                            "Cancel",
                        )),
                )
        });

        let notes = Dialog::content(&self.notes, DialogContentProps::default(), cx).map(|content| {
            content.with(|el, _| panel(el).mt(px(80.)))
                .child(Dialog::title(&self.notes, cx).aria_label("Notes").child("Notes"))
                .child(Dialog::description(&self.notes, cx).aria_label("3 drafts").child("3 drafts"))
                .child(button(
                    Dialog::close_part(&self.notes, "close", DialogCloseProps::default(), cx),
                    "Close",
                    "Close",
                ))
        });

        let delete_backdrop =
            AlertDialog::backdrop(&self.delete, None, cx).map(|backdrop| backdrop.bg(rgba(0x00000066)));
        let delete = AlertDialog::content(&self.delete, DialogContentProps::default(), cx).map(|content| {
            let erase = DialogCloseProps::default()
                .value("confirm")
                .action(|_event, _details, _window, cx| {
                    let wait = cx.background_executor().timer(Duration::from_millis(800));
                    ActionOutcome::wait(async move {
                        wait.await;
                        Ok::<(), SharedString>(())
                    })
                });
            content.with(|el, _| panel(el))
                .child(AlertDialog::title(&self.delete, cx).aria_label("Delete this project?").child("Delete this project?"))
                .child(AlertDialog::description(&self.delete, cx).aria_label("Acme Inc.").child("Acme Inc."))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(button(
                            AlertDialog::close_part(&self.delete, "cancel", DialogCloseProps::default(), cx),
                            "Cancel",
                            "Cancel",
                        ))
                        .child(button(
                            AlertDialog::close_part(&self.delete, "delete", erase, cx),
                            "Delete",
                            "Deleting",
                        )),
                )
        });

        div()
            .size_full()
            .flex()
            .items_start()
            .gap_2()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(trigger(
                Dialog::trigger(&self.share, "share", DialogTriggerProps::default(), cx),
                "Share",
            ))
            .children(share_backdrop)
            .children(share)
            .child(contact_trigger)
            .children(contact_backdrop)
            .children(contact)
            .child(trigger(
                Dialog::trigger(&self.notes, "notes", DialogTriggerProps::default(), cx),
                "Notes",
            ))
            .children(notes)
            .child(trigger(
                AlertDialog::trigger(&self.delete, "delete", DialogTriggerProps::default(), cx),
                "Delete project",
            ))
            .children(delete_backdrop)
            .children(delete)
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(640.), px(420.)), cx);
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
