//! Avatars: an image that loads, one that fails, a remote one whose fallback
//! waits, initials alone, and a button whose press swaps its image.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, FocusHandle, ImageSource, InteractiveElement,
    IntoElement, ObjectFit, ParentElement, Render, StatefulInteractiveElement, Styled, StyledImage,
    WeakEntity, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_avatar::{Avatar, AvatarFallbackProps, AvatarLoadingStatus, AvatarOptions};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, FocusMode};

// A portrait the example writes to a file, so one avatar loads from disk.
const PORTRAIT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><rect width="64" height="64" fill="#c7d2fe"/><circle cx="32" cy="25" r="12" fill="#4338ca"/><rect x="12" y="41" width="40" height="30" rx="15" fill="#4338ca"/></svg>"##;

struct Person {
    name: &'static str,
    initials: &'static str,
    image: Option<ImageSource>,
    delay: Duration,
    // The button's focus and the image a press swaps in, for an avatar that
    // acts on a press.
    button: Option<(FocusHandle, Option<ImageSource>)>,
    avatar: Entity<Avatar>,
    reason: Option<&'static str>,
}

struct Example {
    people: Vec<Person>,
}

impl Example {
    fn new(portrait: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let portrait = ImageSource::from(portrait);
        let missing = ImageSource::from(
            std::env::temp_dir()
                .join("ui-sdk-avatar")
                .join("missing.png"),
        );
        let remote = ImageSource::from("https://github.com/shadcn.png");
        let people = [
            ("Ada Lovelace", "AL", Some(portrait.clone()), Duration::ZERO, None),
            ("Grace Hopper", "GH", Some(missing.clone()), Duration::ZERO, None),
            ("shadcn", "CN", Some(remote), Duration::from_millis(600), None),
            ("Alan Turing", "AT", None, Duration::ZERO, None),
            ("Hedy Lamarr", "HL", Some(portrait), Duration::ZERO, Some(missing)),
        ];
        let people = people
            .into_iter()
            .enumerate()
            .map(|(index, (name, initials, image, delay, swap))| {
                let this = this.clone();
                let options = AvatarOptions::default().on_loading_status_change(
                    move |_status, details, _window, cx| {
                        let reason = details.reason.as_str();
                        this.update(cx, |example, cx| {
                            example.people[index].reason = Some(reason);
                            cx.notify();
                        })
                        .ok();
                    },
                );
                Person {
                    name,
                    initials,
                    image,
                    delay,
                    button: swap.map(|swap| (cx.focus_handle(), Some(swap))),
                    avatar: cx.new(|cx| Avatar::new(options, window, cx)),
                    reason: None,
                }
            })
            .collect();
        Self { people }
    }
}

impl Person {
    fn render(&self, index: usize, this: &WeakEntity<Example>, cx: &App) -> Div {
        let avatar = &self.avatar;
        let fallback =
            Avatar::fallback(avatar, AvatarFallbackProps::default().delay(self.delay), cx);
        let initials = match fallback.state().loading_status {
            AvatarLoadingStatus::Error => "?",
            _ => self.initials,
        };
        let fallback = fallback.text_color(rgb(0x4b5563)).child(initials);
        let image = self.image.clone().map(|image| {
            Avatar::image(avatar, image, cx)
                .size_full()
                .rounded_full()
                .object_fit(ObjectFit::Cover)
        });
        let root = Avatar::root(avatar, cx)
            .size(px(48.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .with(|el, state| {
                el.bg(match state.loading_status {
                    AvatarLoadingStatus::Error => rgb(0xfee2e2),
                    _ => rgb(0xf3f4f6),
                })
            });
        let root = match &self.button {
            // A button carries the name, which stays while the image loads,
            // so the image is left out. Initials as a plain string aren't read.
            Some((focus, _)) => {
                let this = this.clone();
                root.aria_role(Role::Button)
                    .focus_mode(FocusMode::TabStop, Some(focus), false)
                    .aria_label(self.name)
                    .cursor_pointer()
                    .focus_visible(|style| style.border_2().border_color(rgb(0x2563eb)))
                    .on_press(move |_event, _window, cx| {
                        this.update(cx, |example, cx| {
                            let person = &mut example.people[index];
                            if let Some((_, swap)) = &mut person.button {
                                std::mem::swap(&mut person.image, swap);
                            }
                            cx.notify();
                        })
                        .ok();
                    })
                    .children(image.map(|image| image.aria(Aria::new().hidden(true))))
                    .child(fallback)
            }
            None => root
                .children(image.map(|image| image.aria_label(self.name)))
                .child(fallback.aria_role(Role::Img).aria_label(self.name)),
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .text_xs()
            .child(root)
            .child(avatar.read(cx).loading_status().as_str())
            .child(div().text_color(rgb(0x9ca3af)).children(self.reason))
    }
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        div()
            .size_full()
            .flex()
            .gap_6()
            .p_6()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .children(
                self.people
                    .iter()
                    .enumerate()
                    .map(|(index, person)| person.render(index, &this, cx)),
            )
    }
}

fn main() {
    let portrait = std::env::temp_dir().join("ui-sdk-avatar-portrait.svg");
    std::fs::write(&portrait, PORTRAIT).ok();
    gpui_platform::application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.), px(160.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Example::new(portrait, window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
