//! Hover cards: one shared by two links, each handing it a profile, and one
//! of its own with an arrow.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled,
    Window, WindowBounds, WindowOptions, div, px, rgb, size, text,
};
use ui_sdk_hover_card::{
    HoverCard, HoverCardArrowState, HoverCardContentProps, HoverCardOptions, HoverCardTriggerProps,
};
use ui_sdk_kernel::roles::Role;
use ui_sdk_platform::PhysicalSide;

struct Example {
    people: Entity<HoverCard<SharedString>>,
    release: Entity<HoverCard>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let people = cx.new(|cx| HoverCard::new(HoverCardOptions::default(), window, cx));
        let release = cx.new(|cx| HoverCard::new(HoverCardOptions::default(), window, cx));
        Self { people, release }
    }
}

fn arrow(el: gpui::Stateful<gpui::Div>, state: &HoverCardArrowState) -> gpui::Stateful<gpui::Div> {
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

fn card<E: Styled>(content: E) -> E {
    content
        .flex()
        .flex_col()
        .gap_1()
        .w(px(200.))
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(0xd1d5db))
        .bg(rgb(0xffffff))
        .text_color(rgb(0x111827))
}

// A link: its press follows it, so only hover and focus open the card.
fn link<P: Clone + PartialEq + std::fmt::Debug + 'static>(
    entity: &Entity<HoverCard<P>>,
    key: &'static str,
    props: HoverCardTriggerProps<P>,
    cx: &App,
) -> impl IntoElement {
    HoverCard::trigger(entity, key, props, cx)
        .aria_role(Role::Link)
        .on_press(|_event, _window, _cx| {})
        .with(|el, state| el.text_color(if state.open { rgb(0x1d4ed8) } else { rgb(0x2563eb) }))
        .child(text!(key))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let people = HoverCard::content(&self.people, HoverCardContentProps::default(), cx).map(|content| {
            let name = content.state().payload.clone().unwrap_or_default();
            card(content).child(text!(name))
        });
        let release =
            HoverCard::content(&self.release, HoverCardContentProps::default(), cx).map(|content| {
                card(content)
                    .child(HoverCard::arrow(&self.release, cx).with(arrow))
                    .child(text!("v2.4.0"))
            });

        div()
            .size_full()
            .flex()
            .items_start()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(link(
                &self.people,
                "@kofi",
                HoverCardTriggerProps::default().payload(SharedString::from("Kofi Mensah")),
                cx,
            ))
            .child(link(
                &self.people,
                "@ines",
                HoverCardTriggerProps::default().payload(SharedString::from("Inès Moreau")),
                cx,
            ))
            .children(people)
            .child(link(
                &self.release,
                "Release",
                HoverCardTriggerProps::default(),
                cx,
            ))
            .children(release)
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(420.), px(260.)), cx);
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
