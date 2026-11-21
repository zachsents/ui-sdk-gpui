//! Carousels: slides with Previous and Next, photos with indicators, a loop
//! that rotates with a play toggle and a progress bar, three slides to a
//! page, and a vertical one.

use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement, Orientation,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds,
    WindowOptions, div, px, relative, rgb, size,
};
use ui_sdk_carousel::{
    Carousel, CarouselContentState, CarouselItemState, CarouselOptions, CarouselStepState,
};
use ui_sdk_kernel::Part;

const SLIDES: [(&str, u32); 5] = [
    ("1", 0xdbeafe),
    ("2", 0xdcfce7),
    ("3", 0xfef3c7),
    ("4", 0xfce7f3),
    ("5", 0xede9fe),
];

const PHOTOS: [(&str, u32); 6] = [
    ("Dunes", 0xf5deb3),
    ("Forest", 0xa7c4a0),
    ("Glacier", 0xcfe8f3),
    ("Canyon", 0xe0a080),
    ("Meadow", 0xd4e8a0),
    ("Harbor", 0xa0b8d0),
];

struct Example {
    basic: Entity<Carousel>,
    photos: Entity<Carousel>,
    rotating: Entity<Carousel>,
    pages: Entity<Carousel>,
    vertical: Entity<Carousel>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut carousel = |options: CarouselOptions| cx.new(|cx| Carousel::new(options, window, cx));
        Self {
            basic: carousel(CarouselOptions::default()),
            photos: carousel(CarouselOptions::default()),
            rotating: carousel(
                CarouselOptions::default()
                    .r#loop(true)
                    .default_playing(true)
                    .interval(Duration::from_secs(3)),
            ),
            pages: carousel(CarouselOptions::default().slides_per_page(3)),
            vertical: carousel(CarouselOptions::default().orientation(Orientation::Vertical)),
        }
    }
}

fn tile(label: impl Into<SharedString>, color: u32) -> Div {
    div()
        .size_full()
        .rounded_md()
        .bg(rgb(color))
        .flex()
        .items_center()
        .justify_center()
        .text_lg()
        .child(label.into())
}

fn slides(
    carousel: &Entity<Carousel>,
    slides: &[(&'static str, u32)],
    sized: impl Fn(Part<CarouselItemState>) -> Part<CarouselItemState>,
    cx: &App,
) -> Part<CarouselContentState> {
    Carousel::content(carousel, cx).children(
        slides
            .iter()
            .map(|(name, color)| sized(Carousel::item(carousel, *name, cx)).child(tile(*name, *color))),
    )
}

fn step(part: Part<CarouselStepState>, label: &'static str, text: &'static str) -> Part<CarouselStepState> {
    part.aria_label(label)
        .flex_none()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .with(|el, state| {
            el.text_color(if state.disabled {
                rgb(0x9ca3af)
            } else {
                rgb(0x111827)
            })
        })
        .child(text)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cx: &App = cx;

        let basic = Carousel::root(&self.basic, cx)
            .aria_label("Featured")
            .w(px(360.))
            .flex()
            .items_center()
            .gap_2()
            .child(step(Carousel::previous_part(&self.basic, cx), "Previous slide", "‹"))
            .child(
                Carousel::viewport(&self.basic, cx)
                    .flex_1()
                    .rounded_md()
                    .child(slides(&self.basic, &SLIDES, |item| item.w_full().h(px(140.)), cx).gap_4()),
            )
            .child(step(Carousel::next_part(&self.basic, cx), "Next slide", "›"));

        let photos = Carousel::root(&self.photos, cx)
            .aria_label("Photos")
            .w(px(300.))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Carousel::viewport(&self.photos, cx)
                    .rounded_md()
                    .child(slides(&self.photos, &PHOTOS, |item| item.w_full().h(px(160.)), cx).gap_4()),
            )
            .child(
                Carousel::indicator_group(&self.photos, cx)
                    .aria_label("Photo")
                    .flex()
                    .justify_center()
                    .gap_2()
                    .children(PHOTOS.iter().enumerate().map(|(index, (name, _))| {
                        Carousel::indicator(&self.photos, *name, Some(index + 1), cx)
                            .aria_label(*name)
                            .h(px(8.))
                            .rounded_full()
                            .with(|el, state| {
                                el.w(px(if state.selected { 20. } else { 8. })).bg(if state.selected {
                                    rgb(0x1f2937)
                                } else {
                                    rgb(0xd1d5db)
                                })
                            })
                    })),
            );

        let root = Carousel::root(&self.rotating, cx);
        let progress = root.state().progress;
        let rotating = root
            .aria_label("Highlights")
            .w(px(300.))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Carousel::viewport(&self.rotating, cx)
                    .rounded_md()
                    .child(slides(&self.rotating, &SLIDES, |item| item.w_full().h(px(120.)), cx).gap_4()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Carousel::play_toggle(&self.rotating, cx)
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0xf3f4f6))
                            .with(|el, state| {
                                let label = if state.playing { "Pause" } else { "Play" };
                                el.aria_label(label).child(label)
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(4.))
                            .rounded_full()
                            .bg(rgb(0xe5e7eb))
                            .child(div().h_full().w(relative(progress)).rounded_full().bg(rgb(0x1f2937))),
                    ),
            );

        let pages = Carousel::root(&self.pages, cx)
            .aria_label("Products")
            .w(px(440.))
            .flex()
            .items_center()
            .gap_2()
            .child(step(Carousel::previous_part(&self.pages, cx), "Previous page", "‹"))
            .child(Carousel::viewport(&self.pages, cx).flex_1().child(slides(
                &self.pages,
                &PHOTOS,
                |item| item.w(relative(1. / 3.)).h(px(96.)).p_1(),
                cx,
            )))
            .child(step(Carousel::next_part(&self.pages, cx), "Next page", "›"));

        let vertical = Carousel::root(&self.vertical, cx)
            .aria_label("News")
            .w(px(200.))
            .flex()
            .flex_col()
            .gap_2()
            .child(step(Carousel::previous_part(&self.vertical, cx), "Previous slide", "▲"))
            .child(Carousel::viewport(&self.vertical, cx).h(px(208.)).child(slides(
                &self.vertical,
                &SLIDES,
                |item| item.w_full().h(relative(0.5)).p_1(),
                cx,
            )))
            .child(step(Carousel::next_part(&self.vertical, cx), "Next slide", "▼"));

        div()
            .id("carousels")
            .size_full()
            .flex()
            .flex_col()
            .gap_8()
            .p_6()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(div().flex().items_start().gap_8().child(basic).child(photos))
            .child(div().flex().items_start().gap_8().child(rotating).child(vertical))
            .child(div().flex().items_start().child(pages))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.), px(760.)), cx);
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
