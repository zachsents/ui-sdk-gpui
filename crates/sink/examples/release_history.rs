//! The sink's release history (`collections-release-history`): the release
//! in focus over a timeline of every version. Arrow keys or a drag move
//! through them, and the card follows.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, AppContext, Bounds, BoxShadow, Context, Div,
    Entity, FocusHandle, FontFeatures, FontWeight, InteractiveElement, IntoElement, MouseMoveEvent,
    ParentElement, PathBuilder, Pixels, Render, Rgba, StatefulInteractiveElement, Styled,
    Subscription, Window, WindowBounds, WindowOptions, canvas, div, fill, point, px, relative,
    size, text,
};
use ui_sdk_button::{Button, ButtonOptions};
use ui_sdk_color::{Color, ColorMixOptions, mix_color};
use ui_sdk_kernel::ActionOutcome;
use ui_sdk_kernel::focus_events::on_focus_in;
use ui_sdk_number::format_number;
use ui_sdk_sink::{MONO, Palette, Size, Tile, Tone, lifted, mix, page};
use ui_sdk_timeline::{Timeline, TimelineOptions, TimelineOrientation, TimelineStatus};

const TILE: Tile = Tile {
    size: Size::Wide,
    tone: Tone::Paper,
    uses: &["Timeline", "createTimelineController"],
};

struct Release {
    version: &'static str,
    date: &'static str,
    name: &'static str,
    summary: &'static str,
    commits: u32,
    status: TimelineStatus,
}

static RELEASES: [Release; 7] = [
    Release {
        version: "3.0",
        date: "Mar 4",
        name: "The rewrite",
        summary: "A new sync engine and a sidebar you can collapse.",
        commits: 1204,
        status: TimelineStatus::Complete,
    },
    Release {
        version: "3.1",
        date: "Apr 15",
        name: "Shared links",
        summary: "Links remember the folder they were opened from.",
        commits: 318,
        status: TimelineStatus::Complete,
    },
    Release {
        version: "3.2",
        date: "Jun 2",
        name: "Search in files",
        summary: "Search matches what's inside a file, not only its name.",
        commits: 402,
        status: TimelineStatus::Complete,
    },
    Release {
        version: "3.3",
        date: "Jul 21",
        name: "Resumable uploads",
        summary: "Uploads pick up where they stopped after the network drops.",
        commits: 276,
        status: TimelineStatus::Complete,
    },
    Release {
        version: "3.4",
        date: "Sep 8",
        name: "Dark mode",
        summary: "Follows your system, with sharper avatars and icons.",
        commits: 189,
        status: TimelineStatus::Complete,
    },
    Release {
        version: "3.5",
        date: "Nov 12",
        name: "Offline drafts",
        summary: "Write without a connection. Drafts sync when you're back.",
        commits: 441,
        status: TimelineStatus::Current,
    },
    Release {
        version: "3.6",
        date: "Jan 20",
        name: "Comments",
        summary: "Threads on any line, with mentions and email replies.",
        commits: 97,
        status: TimelineStatus::Upcoming,
    },
];

const LATEST_RELEASE: &str = "3.5";

// The selected marker's `scale`, and how long it and the name take to move.
const SELECTED_SCALE: f32 = 1.35;
const SCALE_TRANSITION: Duration = Duration::from_millis(180);
const RISE: Duration = Duration::from_millis(240);

// Below this width the timeline drops its dates (`@container (width < 420px)`).
const NARROW: Pixels = px(420.);

struct Example {
    timeline: Entity<Timeline>,
    latest: Entity<Button>,
    selected: &'static str,
    // How often the selection has moved, which keys the name's rise as the
    // original's `key={release.version}` does. The first name shows at once:
    // gpui stops drawing a covered window, which would hold it half risen.
    moves: usize,
    // Each item's focus handle, from the timeline, so the tile hears it take
    // focus, the item's `onFocus`.
    items: Vec<FocusHandle>,
    // Each marker's scale, which eases as the selection moves.
    scales: Vec<Transition>,
    _focus: Vec<Subscription>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Tab moves focus, as it does in a browser.
        ui_sdk_focus::bind_tab_keys(cx);
        let timeline = cx.new(|cx| {
            Timeline::new(
                TimelineOptions::default()
                    .orientation(TimelineOrientation::Horizontal)
                    .r#loop(false),
                window,
                cx,
            )
        });
        let controller = timeline.downgrade();
        let latest = cx.new(|cx| {
            Button::new(
                ButtonOptions::default().action(move |_event, _details, window, cx| {
                    controller
                        .update(cx, |timeline, cx| {
                            timeline.focus(Some(LATEST_RELEASE), window, cx)
                        })
                        .ok();
                    ActionOutcome::Done
                }),
                window,
                cx,
            )
        });
        let items: Vec<FocusHandle> = RELEASES
            .iter()
            .map(|release| timeline.read(cx).item_focus_handle(release.version, cx))
            .collect();
        let focus = RELEASES
            .iter()
            .zip(&items)
            .map(|(release, item)| {
                let version = release.version;
                on_focus_in(item, window, cx, move |this: &mut Self, _window, cx| {
                    this.select(version, cx)
                })
            })
            .collect();
        let scales = RELEASES
            .iter()
            .map(|release| Transition::at(scale_of(release.version == LATEST_RELEASE)))
            .collect();
        Self {
            timeline,
            latest,
            selected: LATEST_RELEASE,
            moves: 0,
            items,
            scales,
            _focus: focus,
        }
    }

    fn select(&mut self, version: &'static str, cx: &mut Context<Self>) {
        if self.selected == version {
            return;
        }
        let now = Instant::now();
        let instant = cx.reduce_motion();
        for (release, scale) in RELEASES.iter().zip(&mut self.scales) {
            scale.retarget(scale_of(release.version == version), now, instant);
        }
        self.selected = version;
        self.moves += 1;
        cx.notify();
    }

    fn release(&self, release: &'static Release, palette: &Palette, cx: &App) -> Div {
        let (status, background, color) = match release.status {
            TimelineStatus::Current => (
                "Latest",
                mix(palette.accent, 0.16),
                mix_with(palette.accent, palette.ink, 0.25),
            ),
            TimelineStatus::Upcoming => ("Planned", mix(palette.ink, 0.07), palette.ink_2),
            TimelineStatus::Complete => ("Shipped", mix(palette.ink, 0.07), palette.ink_2),
        };
        let commits = format_number(Some(f64::from(release.commits)), Some("en-US"), None);
        let name = div()
            .mt(px(4.))
            .text_size(px(17.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(lifted(text!(release.name)));
        let name: AnyElement = if self.moves == 0 || cx.reduce_motion() {
            name.into_any_element()
        } else {
            name.relative()
                .with_animation(
                    ("release-name", self.moves),
                    Animation::new(RISE).with_easing(ease_out),
                    |name, delta| name.opacity(delta).top(px(4. * (1. - delta))),
                )
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(14.))
            .bg(palette.surface)
            .shadow(TILE.tone.shadow())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(24.))
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(12.))
                            .line_height(relative(1.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.ink_2)
                            .child(lifted(text!(format!("v{}", release.version)))),
                    )
                    .child(
                        div()
                            .px(px(7.))
                            .py(px(3.))
                            .rounded_full()
                            .bg(background)
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color)
                            .child(lifted(text!(status))),
                    )
                    .when(self.selected != LATEST_RELEASE, |head| {
                        head.child(self.jump_to_latest(palette, cx))
                    }),
            )
            .child(name)
            .child(
                div()
                    .truncate()
                    .text_color(palette.ink_2)
                    .child(lifted(text!(release.summary))),
            )
            .child(
                div()
                    .mt(px(2.))
                    .font_family(MONO)
                    .text_size(px(11.5))
                    .line_height(relative(1.3))
                    .font_weight(FontWeight::MEDIUM)
                    .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
                    .text_color(palette.ink_3)
                    .child(lifted(text!(format!("{} · {commits} commits", release.date)))),
            )
    }

    // A ghost button (`b-btn`), shown while another release is in focus.
    fn jump_to_latest(&self, palette: &Palette, cx: &App) -> impl IntoElement {
        let hover = mix(palette.ink, 0.07);
        let (accent, surface) = (palette.accent, palette.surface);
        Button::root(&self.latest, cx)
            .aria_label("Jump to latest")
            .ml_auto()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .h(px(24.))
            .px(px(8.))
            .border_1()
            .border_color(gpui::transparent_black())
            .rounded(px(10.))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(palette.ink)
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.shadow(outline(accent, surface)))
            .child(lifted("Jump to latest"))
    }

    fn timeline(&self, palette: &Palette, now: Instant, cx: &App) -> impl IntoElement {
        let narrow = self
            .timeline
            .read(cx)
            .instance()
            .bound_parts()
            .iter()
            .find(|part| part.slot == "root")
            .and_then(|part| part.bounds)
            .is_some_and(|bounds| bounds.size.width < NARROW);
        Timeline::root(&self.timeline, cx)
            .aria_label("Releases")
            .flex()
            .children(
                RELEASES
                    .iter()
                    .enumerate()
                    .map(|(index, release)| self.item(index, release, palette, narrow, now, cx)),
            )
    }

    fn item(
        &self,
        index: usize,
        release: &'static Release,
        palette: &Palette,
        narrow: bool,
        now: Instant,
        cx: &App,
    ) -> impl IntoElement {
        let version = release.version;
        let selected = version == self.selected;
        let accent = palette.accent;
        let ring = move |style: gpui::StyleRefinement| {
            style.shadow(vec![
                BoxShadow::new(px(0.), px(0.), accent.into())
                    .spread_radius(px(2.))
                    .inset(),
            ])
        };
        let item = Timeline::item(&self.timeline, version, Some(release.status), cx)
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(3.))
            .pt(px(2.))
            .pb(px(4.))
            .rounded(px(10.))
            .cursor_pointer()
            .focus_visible(ring);
        // Drawn before the marker, which sits over it.
        let connector = (index + 1 < RELEASES.len()).then(|| {
            let palette = *palette;
            Timeline::connector(&self.timeline, version, cx)
                .absolute()
                .top(px(13.))
                .left(relative(0.5))
                .w_full()
                .h(px(2.))
                .with(move |connector, state| match state.status {
                    Some(TimelineStatus::Complete) => connector.bg(palette.ink_2),
                    Some(TimelineStatus::Current) => connector.child(dashes(palette.ink_3)),
                    _ => connector.bg(mix(palette.ink, 0.22)),
                })
        });
        let scale = self.scales[index].value(now);
        let marker = {
            let palette = *palette;
            Timeline::marker(&self.timeline, version, cx)
                .relative()
                .flex_none()
                .size(px(12.))
                .mt(px(6.))
                .mb(px(4.))
                .with(move |marker, state| {
                    let (border, background) = match state.status {
                        Some(TimelineStatus::Complete) => (palette.ink_2, palette.ink_2),
                        Some(TimelineStatus::Current) => (palette.accent, palette.accent),
                        _ => (palette.ink_3, palette.bg),
                    };
                    let width = px(2. * scale);
                    let circle = dot(12. * scale).bg(background);
                    let circle = if state.status == Some(TimelineStatus::Upcoming) {
                        circle.child(dashed_ring(border, width))
                    } else {
                        circle.border(width).border_color(border)
                    };
                    // The halo is `box-shadow: 0 0 0 4px`, which gpui would
                    // draw with the dot's corners rather than round.
                    marker
                        .when(state.status == Some(TimelineStatus::Current), |marker| {
                            marker.child(dot(20. * scale).bg(mix(palette.accent, 0.22)))
                        })
                        .child(circle)
                })
        };
        let title = Timeline::title(&self.timeline, version, cx)
            .aria_label(version)
            .font_family(MONO)
            .text_size(px(12.))
            .line_height(relative(1.2))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if selected { palette.ink } else { palette.ink_2 })
            .child(lifted(version));
        let time = (!narrow).then(|| {
            Timeline::time(&self.timeline, version, cx)
                .aria_label(release.date)
                .text_size(px(11.))
                .line_height(relative(1.2))
                .text_color(palette.ink_3)
                .whitespace_nowrap()
                .child(lifted(release.date))
        });
        let pressed = self.timeline.downgrade();
        let moved = self.timeline.downgrade();
        let (down, over) = (self.items[index].clone(), self.items[index].clone());
        item.on_any_mouse_down(move |_event, window, cx| {
            scrub(&pressed, &down, version, window, cx)
        })
        .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
            if event.pressed_button.is_some() {
                scrub(&moved, &over, version, window, cx)
            }
        })
        .children(connector)
        .child(marker)
        .child(title)
        .children(time)
    }
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = TILE.tone.palette();
        let now = Instant::now();
        if self.scales.iter().any(|scale| scale.running(now)) {
            window.request_animation_frame();
        }
        let release = RELEASES
            .iter()
            .find(|candidate| candidate.version == self.selected)
            .unwrap_or(&RELEASES[0]);
        let demo = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .w_full()
            .min_w_0()
            .child(self.release(release, &palette, cx))
            .child(self.timeline(&palette, now, cx));
        page(TILE.render(demo))
    }
}

// A press or a drag over an item focuses it, as the original's `scrub`
// does with the item under the pointer.
fn scrub(
    timeline: &gpui::WeakEntity<Timeline>,
    item: &FocusHandle,
    version: &'static str,
    window: &mut Window,
    cx: &mut App,
) {
    if item.is_focused(window) {
        return;
    }
    timeline
        .update(cx, |timeline, cx| timeline.focus(Some(version), window, cx))
        .ok();
}

fn scale_of(selected: bool) -> f32 {
    if selected { SELECTED_SCALE } else { 1. }
}

// A circle of `diameter`, centered on the 12px marker it scales from.
fn dot(diameter: f32) -> Div {
    let offset = px((12. - diameter) / 2.);
    div()
        .absolute()
        .top(offset)
        .left(offset)
        .size(px(diameter))
        .rounded_full()
}

// The connector after the current release: 4px dashes every 8px, the
// original's `repeating-linear-gradient`.
fn dashes(color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let mut x = bounds.left();
            while x < bounds.right() {
                let width = (bounds.right() - x).min(px(4.));
                window.paint_quad(fill(
                    Bounds::new(point(x, bounds.top()), size(width, bounds.size.height)),
                    color,
                ));
                x += px(8.);
            }
        },
    )
    .size_full()
}

// The upcoming marker's dashed border, as Chrome strokes a dashed circle this
// size: three dashes three times the border's width, a gap centered on three
// o'clock. gpui's own dashes are twice the width, with half the gap.
fn dashed_ring(color: Rgba, width: Pixels) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let center = bounds.center();
            let radius = (bounds.size.width - width) / 2.;
            let half = 3. * f32::from(width) / f32::from(radius) / 2.;
            let at = |angle: f32| center + point(radius * angle.cos(), radius * angle.sin());
            let mut path = PathBuilder::stroke(width);
            for middle in [60f32, 180., 300.] {
                let middle = middle.to_radians();
                path.move_to(at(middle - half));
                path.arc_to(
                    point(radius, radius),
                    px(0.),
                    false,
                    true,
                    at(middle + half),
                );
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size_full()
}

// `outline: 2px solid; outline-offset: 2px` around a button on `surface`.
fn outline(color: Rgba, surface: Rgba) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(0.), color.into()).spread_radius(px(4.)),
        BoxShadow::new(px(0.), px(0.), surface.into()).spread_radius(px(2.)),
    ]
}

// `color-mix(in oklab, first, second amount)`.
fn mix_with(first: Rgba, second: Rgba, amount: f64) -> Rgba {
    mix_color(
        &Color::from(first),
        &Color::from(second),
        amount,
        ColorMixOptions::default(),
    )
    .into()
}

// `--ease-out`: `cubic-bezier(0.22, 1, 0.36, 1)`.
fn ease_out(t: f32) -> f32 {
    let curve = |a: f32, b: f32, s: f32| {
        3. * a * s * (1. - s) * (1. - s) + 3. * b * s * s * (1. - s) + s * s * s
    };
    let (mut low, mut high) = (0., 1.);
    for _ in 0..24 {
        let middle = (low + high) / 2.;
        if curve(0.22, 0.36, middle) < t {
            low = middle;
        } else {
            high = middle;
        }
    }
    curve(1., 1., (low + high) / 2.)
}

// A value easing toward another, as a CSS transition does: a change starts
// from wherever the last one had got to.
#[derive(Clone, Copy)]
struct Transition {
    from: f32,
    to: f32,
    start: Instant,
}

impl Transition {
    fn at(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start: Instant::now(),
        }
    }

    fn progress(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start);
        (elapsed.as_secs_f32() / SCALE_TRANSITION.as_secs_f32()).min(1.)
    }

    fn value(&self, now: Instant) -> f32 {
        self.from + (self.to - self.from) * ease_out(self.progress(now))
    }

    fn running(&self, now: Instant) -> bool {
        self.from != self.to && self.progress(now) < 1.
    }

    fn retarget(&mut self, to: f32, now: Instant, instant: bool) {
        if self.to == to {
            return;
        }
        let from = if instant { to } else { self.value(now) };
        *self = Self {
            from,
            to,
            start: now,
        };
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        // `UI_SDK_QUIET` opens it without taking focus, for screenshots.
        let quiet = std::env::var_os("UI_SDK_QUIET").is_some();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(TILE.window_bounds(quiet, cx))),
                focus: !quiet,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Example::new(window, cx)),
        )
        .unwrap();
        if !quiet {
            cx.activate(true);
        }
    });
}
