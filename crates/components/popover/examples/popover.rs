//! Popovers: one with a title, a description and a Close part, one a
//! trigger's shortcut toggles, one that opens on hover, and a controlled
//! modal one.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size, text,
};
use ui_sdk_platform::PhysicalSide;
use ui_sdk_popover::{
    Popover, PopoverArrowState, PopoverContentProps, PopoverModal, PopoverOptions, PopoverShortcutProps,
    PopoverTriggerProps, PopoverTriggerState,
};

struct Example {
    share: Entity<Popover>,
    invite: Entity<Popover>,
    profile: Entity<Popover>,
    comment: Entity<Popover>,
    comment_open: bool,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let share = cx.new(|cx| Popover::new(PopoverOptions::default(), window, cx));
        let invite = cx.new(|cx| Popover::new(PopoverOptions::default(), window, cx));
        let profile = cx.new(|cx| Popover::new(PopoverOptions::default().open_on_hover(true), window, cx));
        let this = cx.entity().downgrade();
        let comment = cx.new(|cx| {
            Popover::new(
                PopoverOptions::default()
                    .open(false)
                    .modal(PopoverModal::True)
                    .on_open_change(move |open, _details, _window, cx| {
                        let open = *open;
                        this.update(cx, |example, cx| {
                            example.comment_open = open;
                            cx.notify();
                        })
                        .ok();
                    }),
                window,
                cx,
            )
        });
        Self {
            share,
            invite,
            profile,
            comment,
            comment_open: false,
        }
    }
}

fn trigger_color(state: &PopoverTriggerState) -> gpui::Rgba {
    if state.open { rgb(0xe5e7eb) } else { rgb(0xf3f4f6) }
}

fn trigger(
    entity: &Entity<Popover>,
    label: &'static str,
    props: PopoverTriggerProps,
    cx: &App,
) -> impl IntoElement {
    Popover::trigger(entity, label, props, cx)
        .with(|el, state| el.bg(trigger_color(state)))
        .flex()
        .gap_2()
        .px_3()
        .py_1()
        .rounded_md()
        .child(text!(label))
}

fn button(part: ui_sdk_kernel::Part<()>, label: &'static str) -> impl IntoElement {
    part.px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(0x1f2937))
        .text_color(rgb(0xffffff))
        .child(text!(label))
}

fn arrow(el: gpui::Stateful<gpui::Div>, state: &PopoverArrowState) -> gpui::Stateful<gpui::Div> {
    let el = el.absolute().size(px(8.)).bg(rgb(0xd1d5db));
    let offset = state.arrow.map(|arrow| arrow.offset).unwrap_or_default();
    match state.side {
        Some(PhysicalSide::Bottom) => el.top(px(-4.)).left(offset),
        Some(PhysicalSide::Top) => el.bottom(px(-4.)).left(offset),
        Some(PhysicalSide::Left) => el.right(px(-4.)).top(offset),
        Some(PhysicalSide::Right) => el.left(px(-4.)).top(offset),
        None => el.hidden(),
    }
}

fn panel<E: Styled>(content: E) -> E {
    content
        .flex()
        .flex_col()
        .gap_2()
        .w(px(220.))
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .bg(rgb(0xffffff))
        .text_color(rgb(0x111827))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let comment_open = self.comment_open;
        self.comment.update(cx, |popover, cx| {
            popover.set_controlled_open(Some(comment_open), cx)
        });

        let share = Popover::content(&self.share, PopoverContentProps::default(), cx).map(|content| {
            panel(content)
                .child(Popover::title(&self.share, cx).aria_label("Share").child("Share"))
                .child(Popover::description(&self.share, cx).aria_label("ui-sdk.dev/d/42").child("ui-sdk.dev/d/42"))
                .child(button(Popover::close_part(&self.share, None, cx), "Done"))
        });

        let invite_trigger = Popover::trigger(
            &self.invite,
            "invite",
            PopoverTriggerProps::default().shortcut("mod+i"),
            cx,
        )
        .with(|el, state| el.bg(trigger_color(state)))
        .flex()
        .gap_2()
        .px_3()
        .py_1()
        .rounded_md()
        .child(text!("Invite"))
        .child(
            Popover::shortcut(&self.invite, None, PopoverShortcutProps::default(), cx)
                .text_color(rgb(0x6b7280)),
        );
        let invite = Popover::content(&self.invite, PopoverContentProps::default(), cx).map(|content| {
            panel(content)
                .child(Popover::arrow(&self.invite, cx).with(arrow))
                .child(Popover::title(&self.invite, cx).aria_label("Invite").child("Invite"))
                .child(button(Popover::close_part(&self.invite, None, cx), "Send"))
        });

        let profile = Popover::content(&self.profile, PopoverContentProps::default(), cx).map(|content| {
            panel(content)
                .child(Popover::title(&self.profile, cx).aria_label("Kofi Mensah").child("Kofi Mensah"))
                .child(button(Popover::close_part(&self.profile, None, cx), "Follow"))
        });

        let comment = Popover::content(&self.comment, PopoverContentProps::default(), cx).map(|content| {
            panel(content)
                .child(Popover::title(&self.comment, cx).aria_label("Comment").child("Comment"))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(button(
                            Popover::close_part(&self.comment, Some(SharedString::from("post")), cx),
                            "Post",
                        ))
                        .child(button(
                            Popover::close_part(&self.comment, Some(SharedString::from("cancel")), cx),
                            "Cancel",
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
            .child(trigger(&self.share, "Share", PopoverTriggerProps::default(), cx))
            .children(share)
            .child(invite_trigger)
            .children(invite)
            .child(trigger(
                &self.profile,
                "@kofi",
                PopoverTriggerProps::default(),
                cx,
            ))
            .children(profile)
            .child(trigger(
                &self.comment,
                "Comment",
                PopoverTriggerProps::default(),
                cx,
            ))
            .children(comment)
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(560.), px(360.)), cx);
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
