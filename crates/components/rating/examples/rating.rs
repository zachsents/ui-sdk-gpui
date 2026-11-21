//! Ratings: whole stars that clear on a second press, half stars, a
//! read-only average, and a disabled one.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, relative,
    rgb, size,
};
use ui_sdk_rating::{Rating, RatingOptions};

struct Example {
    stay: Entity<Rating>,
    food: Entity<Rating>,
    average: Entity<Rating>,
    service: Entity<Rating>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rating = |options: RatingOptions, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| Rating::new(options, window, cx))
        };
        Self {
            stay: rating(
                RatingOptions::default()
                    .name("stay")
                    .default_value(Some(3.0))
                    .deselectable(true),
                window,
                cx,
            ),
            food: rating(RatingOptions::default().step(0.5).default_value(Some(2.5)), window, cx),
            average: rating(
                RatingOptions::default()
                    .default_value(Some(4.2))
                    .read_only(true)
                    .get_value_label(|value, max| format!("Rated {value} out of {max}").into()),
                window,
                cx,
            ),
            service: rating(
                RatingOptions::default().default_value(Some(2.0)).disabled(true),
                window,
                cx,
            ),
        }
    }
}

fn star(fill: f64) -> impl IntoElement {
    div()
        .relative()
        .size_full()
        .text_color(rgb(0xd1d5db))
        .child("★")
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .h_full()
                .w(relative(fill as f32))
                .overflow_hidden()
                .text_color(rgb(0xf59e0b))
                .child("★"),
        )
}

fn stars(rating: &Entity<Rating>, title: &'static str, cx: &App) -> impl IntoElement {
    let max = rating.read(cx).max();
    let read_only = rating.read(cx).state().read_only;
    let root = Rating::root(rating, cx);
    // A read-only rating is named by its score.
    let root = if read_only { root } else { root.aria_label(SharedString::from(title)) };
    let root = root.flex().gap_1().children((1..=max).map(|index| {
        let radios = rating
            .read(cx)
            .values(index)
            .into_iter()
            .map(|score| Rating::radio(rating, score, cx));
        Rating::item(rating, index, cx)
            .size_6()
            .text_xl()
            .with(|el, state| {
                el.opacity(if state.disabled { 0.5 } else { 1.0 })
                    .child(star(state.fill))
            })
            .children(radios)
    }));
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().child(title))
        .child(root)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cx: &App = cx;
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(stars(&self.stay, "Stay", cx))
            .child(stars(&self.food, "Food", cx))
            .child(stars(&self.average, "Average", cx))
            .child(stars(&self.service, "Service", cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(320.), px(320.)), cx);
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
