//! Separators: between a profile's header and its bio, between links in a
//! row, between stats a line spans, and decorative ones around "or".

use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, FontWeight, IntoElement, ParentElement, Render,
    Stateful, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_kernel::Part;
use ui_sdk_separator::{Separator, SeparatorOptions, SeparatorOrientation, SeparatorState};

struct Example {
    profile: Entity<Separator>,
    links: [Entity<Separator>; 2],
    stats: [Entity<Separator>; 2],
    or: [Entity<Separator>; 2],
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut separator =
            |options: SeparatorOptions| cx.new(|cx| Separator::new(options, window, cx));
        let vertical = SeparatorOptions::default().orientation(SeparatorOrientation::Vertical);
        let decorative = SeparatorOptions::default().decorative(true);
        Self {
            profile: separator(SeparatorOptions::default()),
            links: [separator(vertical.clone()), separator(vertical.clone())],
            stats: [separator(vertical.clone()), separator(vertical)],
            or: [separator(decorative.clone()), separator(decorative)],
        }
    }
}

// A number over its label, which a vertical line beside it spans.
fn stat(number: &'static str, label: &'static str) -> Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(number),
        )
        .child(div().text_color(rgb(0x6b7280)).child(label))
}

fn line(el: Stateful<Div>, state: &SeparatorState) -> Stateful<Div> {
    match state.orientation {
        SeparatorOrientation::Horizontal => el.w_full().h(px(1.)),
        SeparatorOrientation::Vertical => el.w(px(1.)).self_stretch(),
    }
}

fn separator(entity: &Entity<Separator>, cx: &App) -> Part<SeparatorState> {
    Separator::root(entity, cx)
        .with(line)
        .flex_shrink_0()
        .bg(rgb(0xe5e7eb))
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = rgb(0x6b7280);
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_8()
            .p_6()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(320.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child("Hollis Grant")
                            .child(div().text_color(muted).child("hollis@example.com")),
                    )
                    .child(separator(&self.profile, cx))
                    .child("Designs the billing and onboarding flows."),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child("Privacy")
                    .child(separator(&self.links[0], cx))
                    .child("Terms")
                    .child(separator(&self.links[1], cx))
                    .child("Cookies"),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .w(px(320.))
                    .child(stat("24", "Projects"))
                    .child(separator(&self.stats[0], cx))
                    .child(stat("1,280", "Followers"))
                    .child(separator(&self.stats[1], cx))
                    .child(stat("312", "Following")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(320.))
                    .child("Sign In with Passkey")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_color(muted)
                            .child(separator(&self.or[0], cx).flex_1())
                            .child("or")
                            .child(separator(&self.or[1], cx).flex_1()),
                    )
                    .child("Email Me a Link"),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(380.), px(420.)), cx);
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
