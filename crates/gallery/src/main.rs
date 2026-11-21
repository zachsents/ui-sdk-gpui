//! Every ported component's example in one window.

use gpui::{
    AnyView, App, AppContext, Bounds, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, StyleRefinement, Styled,
    Window,
    WindowBounds,
    WindowOptions, div, point, prelude::FluentBuilder, px, rgb, size,
};

mod sections {
    use gpui::{AnyView, App, Window};

    /// One component's example.
    pub struct Section {
        pub title: &'static str,
        pub make: fn(&mut Window, &mut App) -> AnyView,
    }

    include!(concat!(env!("OUT_DIR"), "/sections.rs"));
}

use sections::SECTIONS;

struct Gallery {
    selected: usize,
    views: Vec<AnyView>,
}

impl Gallery {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let views = SECTIONS
            .iter()
            .map(|section| (section.make)(window, cx))
            .collect();
        // `GALLERY_SECTION` opens on a section by its title, like `text-field`.
        let selected = std::env::var("GALLERY_SECTION")
            .ok()
            .and_then(|name| {
                SECTIONS.iter().position(|section| {
                    section.title.to_lowercase().replace(' ', "-") == name.to_lowercase()
                })
            })
            .unwrap_or(0);
        Self { selected, views }
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected;
        let nav = SECTIONS.iter().enumerate().map(|(ix, section)| {
            div()
                .id(SharedString::from(section.title))
                .px_3()
                .py_1p5()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(rgb(0x374151))
                .hover(|style| style.bg(rgb(0xf3f4f6)))
                .when(ix == selected, |el| {
                    el.bg(rgb(0xe5e7eb))
                        .text_color(rgb(0x111827))
                        .font_weight(FontWeight::MEDIUM)
                })
                .child(section.title)
                .on_click(cx.listener(move |gallery, _, _, cx| {
                    gallery.selected = ix;
                    cx.notify();
                }))
        });
        div()
            .size_full()
            .flex()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(
                div()
                    .id("nav")
                    .w(px(190.))
                    .flex_none()
                    .h_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .p_3()
                    .border_r_1()
                    .border_color(rgb(0xe5e7eb))
                    .bg(rgb(0xfafafa))
                    .child(
                        div()
                            .px_3()
                            .pb_3()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0x6b7280))
                            .child("UI SDK"),
                    )
                    .children(nav),
            )
            .child(
                div()
                    .id("section")
                    .flex_1()
                    .h_full()
                    .overflow_scroll()
                    .child(
                        self.views[selected]
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
            )
    }
}

fn main() {
    // `GALLERY_QUIET` opens it without taking focus, for screenshots.
    let quiet = std::env::var_os("GALLERY_QUIET").is_some();
    // Debug builds report what the kernel can check, like a control with no
    // name, as warnings. gpui's text system warns about duplicate system
    // fonts on every launch, so it's left out. `RUST_LOG` overrides both.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,gpui_apple::text_system=error"),
    )
    .init();
    gpui_platform::application().run(move |cx: &mut App| {
        ui_sdk_focus::bind_tab_keys(cx);
        ui_sdk_storage::init("ui-sdk-gallery", cx);
        match reqwest_client::ReqwestClient::user_agent("ui-sdk gallery") {
            Ok(client) => cx.set_http_client(std::sync::Arc::new(client)),
            Err(error) => log::warn!("no HTTP client, so remote images won't load: {error}"),
        }
        let window_size = size(px(1040.), px(680.));
        // Quiet windows open in the bottom-right corner, out of the way of
        // whatever the person at this machine is doing.
        let bounds = match cx.primary_display().filter(|_| quiet) {
            Some(display) => {
                let screen = display.bounds();
                Bounds::new(
                    point(
                        screen.origin.x + screen.size.width - window_size.width,
                        screen.origin.y + screen.size.height - window_size.height,
                    ),
                    window_size,
                )
            }
            None => Bounds::centered(None, window_size, cx),
        };
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                focus: !quiet,
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title("UI SDK");
                cx.new(|cx| Gallery::new(window, cx))
            },
        )
        .unwrap();
        if !quiet {
            cx.activate(true);
        }
    });
}
