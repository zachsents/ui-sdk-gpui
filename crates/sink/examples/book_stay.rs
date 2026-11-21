//! The sink's `language-stay` tile: a range over two months, with the
//! place, the night count and the dates beside it. From `Stay` in the
//! original's `tiles/language.tsx`, styled as `tiles/language.css` styles
//! it.
//!
//! The original's empty state, "Pick check-in, then check-out", narrates
//! how to use it, so it reads "No dates" here.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, BoxShadow, Context, Corners, Entity, FontFeatures, FontWeight,
    InteractiveElement, IntoElement, LineCap, LineJoin, ParentElement, PathBuilder, PathStyle,
    Pixels, Render, Rgba, Role, SharedString, StatefulInteractiveElement, StrokeOptions, Styled,
    WeakEntity, Window, WindowBounds, WindowOptions, canvas, deferred, div, font, point, px,
    relative,
};
use ui_sdk_calendar::{
    Calendar, CalendarDayState, CalendarMode, CalendarOptions, CalendarStepState, CalendarValue,
};
use ui_sdk_date::{ISODate, TextWidth, days_between, format_date_range};
use ui_sdk_kernel::modality::focus_visible;
use ui_sdk_kernel::part::{with_aria, with_role};
use ui_sdk_kernel::{Aria, Live, Part};
use ui_sdk_platform::is_rtl;
use ui_sdk_sink::{
    Palette, SANS, SERIF, Size, Tile, Tone, browser_baseline, lifted, metrics, mix, page,
};

const TILE: Tile = Tile {
    size: Size::Wide,
    tone: Tone::Sky,
    uses: &["Calendar", "daysBetween", "formatDateRange"],
};

const LOCALE: &str = "en-GB";

// `.lg-cal-day`'s `transition: background 120ms, color 120ms`.
const FADE: Duration = Duration::from_millis(120);

struct Example {
    calendar: Entity<Calendar>,
    range: Option<(ISODate, ISODate)>,
    // The day under the pointer, as `grid:date`, for `:hover`.
    hovered: Option<SharedString>,
    // Each day's colors on their way to its state's, by `grid:date`.
    fades: HashMap<SharedString, Fade>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Tab moves focus, as it does in a browser.
        ui_sdk_focus::bind_tab_keys(cx);
        let range = Some(("2026-11-12".into(), "2026-11-16".into()));
        let options = calendar_options(&range, narrow(window), cx.entity().downgrade());
        let calendar = cx.new(|cx| Calendar::new(options, window, cx));
        Self {
            calendar,
            range,
            hovered: None,
            fades: HashMap::new(),
        }
    }
}

// `useMedia("(max-width: 640px)")`.
fn narrow(window: &Window) -> bool {
    window.viewport_size().width <= px(640.)
}

// The calendar's props, as `Stay` renders them.
fn calendar_options(
    range: &Option<(ISODate, ISODate)>,
    narrow: bool,
    this: WeakEntity<Example>,
) -> CalendarOptions {
    CalendarOptions::default()
        .mode(CalendarMode::Range)
        .months(if narrow { 1 } else { 2 })
        .locale(LOCALE)
        .week_starts_on(0)
        .weekday_format(TextWidth::Narrow)
        .fixed_weeks(false)
        .default_month("2026-11")
        .value(match range {
            Some((start, end)) => CalendarValue::range(start.clone(), end.clone()),
            None => CalendarValue::Empty,
        })
        .on_value_change(move |value, _details, _window, cx| {
            let range = value
                .as_range()
                .map(|(start, end)| (start.clone(), end.clone()));
            this.update(cx, |example, cx| {
                example.range = range;
                cx.notify();
            })
            .ok();
        })
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let narrow = narrow(window);
        let options = calendar_options(&self.range, narrow, cx.entity().downgrade());
        self.calendar
            .update(cx, |calendar, cx| calendar.set_options(options, cx));
        let palette = TILE.tone.palette();
        let rtl = is_rtl(cx);
        let fades = Fades {
            last: std::mem::take(&mut self.fades),
            next: HashMap::new(),
            now: Instant::now(),
            instant: cx.reduce_motion(),
            running: false,
        };
        let mut days = Days {
            calendar: &self.calendar,
            hovered: self.hovered.as_ref(),
            palette,
            rtl,
            fades,
        };
        let calendar = days.calendar(window, cx);
        let Fades { next, running, .. } = days.fades;
        self.fades = next;
        // A day that left, as the months turn, hears no pointer leave.
        if self
            .hovered
            .as_ref()
            .is_some_and(|key| !self.fades.contains_key(key))
        {
            self.hovered = None;
        }
        if running {
            window.request_animation_frame();
        }
        let stay = div()
            .w_full()
            .flex()
            .when(rtl, |el| el.flex_row_reverse())
            .items_center()
            .justify_center()
            .gap(px(if narrow { 14. } else { 24. }))
            .child(calendar)
            .child(summary(self.range.as_ref(), narrow, &palette, window));
        page(TILE.render(stay))
    }
}

// What the calendar renders with this frame.
struct Days<'a> {
    calendar: &'a Entity<Calendar>,
    hovered: Option<&'a SharedString>,
    palette: Palette,
    rtl: bool,
    fades: Fades,
}

impl Days<'_> {
    fn calendar(&mut self, window: &Window, cx: &Context<Example>) -> AnyElement {
        let entity = self.calendar;
        let root = Calendar::root(entity, cx);
        let head = div()
            .flex()
            .when(self.rtl, |el| el.flex_row_reverse())
            .items_center()
            .justify_between()
            .gap(px(8.))
            .h(px(26.))
            .child(step(
                Calendar::previous_part(entity, cx),
                !self.rtl,
                &self.palette,
            ))
            .child(
                Calendar::heading(entity, cx)
                    .with(|el, state| el.child(lifted(state.label.clone())))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(step(
                Calendar::next_part(entity, cx),
                self.rtl,
                &self.palette,
            ));
        let mut grids = Vec::new();
        for offset in 0..entity.read(cx).model().months() {
            grids.push(self.grid(offset, window, cx));
        }
        root.flex()
            .flex_col()
            .gap(px(4.))
            .child(head)
            .child(
                div()
                    .flex()
                    .when(self.rtl, |el| el.flex_row_reverse())
                    .items_start()
                    .gap(px(18.))
                    .children(grids),
            )
            .into_any_element()
    }

    // A month's table: `border-spacing: 0 2px` around and between its rows.
    fn grid(&mut self, offset: usize, window: &Window, cx: &Context<Example>) -> AnyElement {
        let entity = self.calendar;
        let palette = self.palette;
        let part = Calendar::grid(entity, offset, cx);
        let weeks = part.state().weeks.clone();
        let weekdays = Calendar::weekdays(entity, offset, cx)
            .flex()
            .when(self.rtl, |el| el.flex_row_reverse())
            .children((0..7).map(|index| {
                Calendar::weekday(entity, offset, index, cx)
                    .with(|el, state| el.child(lifted(state.label.clone())))
                    .w(px(28.))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.ink_3)
            }));
        let mut rows = Vec::new();
        for (index, week) in weeks.into_iter().enumerate() {
            let mut cells = Vec::new();
            for cell in week {
                cells.push(if cell.hidden {
                    div().w(px(28.)).h(px(22.)).into_any_element()
                } else {
                    self.day(cell.date, offset, window, cx)
                });
            }
            rows.push(
                Calendar::week(entity, offset, index, cx)
                    .flex()
                    .when(self.rtl, |el| el.flex_row_reverse())
                    .children(cells),
            );
        }
        part.flex()
            .flex_col()
            .py(px(2.))
            .gap(px(2.))
            .child(weekdays)
            .children(rows)
            .into_any_element()
    }

    fn day(
        &mut self,
        date: ISODate,
        offset: usize,
        window: &Window,
        cx: &Context<Example>,
    ) -> AnyElement {
        let palette = self.palette;
        let key: SharedString = format!("{offset}:{date}").into();
        let ring = focus_visible(window)
            && self
                .calendar
                .read(cx)
                .model()
                .roving()
                .focus_handle(&date, cx)
                .is_focused(window);
        let part = Calendar::day(self.calendar, date, offset, cx);
        let state = part.state().clone();
        let hovered = self.hovered == Some(&key);
        let look = self
            .fades
            .look(key.clone(), day_look(&state, hovered, &palette));
        let corners = day_corners(&state);
        let hover = cx.listener(move |example, hovering: &bool, _window, cx| {
            let hovered = if *hovering {
                Some(key.clone())
            } else if example.hovered.as_ref() == Some(&key) {
                None
            } else {
                return;
            };
            if example.hovered != hovered {
                example.hovered = hovered;
                cx.notify();
            }
        });
        part.w(px(28.))
            .h(px(22.))
            .rounded_tl(corners.top_left)
            .rounded_tr(corners.top_right)
            .rounded_br(corners.bottom_right)
            .rounded_bl(corners.bottom_left)
            .bg(look.bg)
            .text_color(look.fg)
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
            .when(state.today, |el| {
                el.shadow(vec![
                    BoxShadow::new(px(0.), px(0.), mix(palette.ink, 0.3).into())
                        .spread_radius(px(1.))
                        .inset(),
                ])
            })
            .when(state.unavailable, |el| el.line_through())
            .when(state.disabled, |el| el.opacity(0.4))
            .flex()
            .items_center()
            .justify_center()
            .child(lifted(state.label.clone()))
            // `:hover`, since the day's own hover is the calendar's.
            .child(div().id("hover").absolute().size_full().on_hover(hover))
            .when(ring, |el| el.child(outline(corners, palette.ink)))
            .into_any_element()
    }
}

// A day's background and text color.
#[derive(Clone, Copy, PartialEq)]
struct Look {
    bg: Rgba,
    fg: Rgba,
}

// `.lg-cal-day` in its states, the later rules winning as in the CSS.
fn day_look(state: &CalendarDayState, hovered: bool, palette: &Palette) -> Look {
    let middle = state.range_middle();
    if state.selected && !middle {
        Look {
            bg: palette.accent,
            fg: palette.accent_ink,
        }
    } else if middle || state.preview {
        Look {
            bg: mix(palette.accent, 0.18),
            fg: palette.ink,
        }
    } else {
        Look {
            bg: mix(palette.ink, if hovered { 0.08 } else { 0. }),
            fg: if state.outside {
                palette.ink_3
            } else {
                palette.ink
            },
        }
    }
}

// Rounded, square through a range and its preview, rounded at a range's
// outer ends.
fn day_corners(state: &CalendarDayState) -> Corners<Pixels> {
    let (round, square) = (px(7.), px(0.));
    let (left, right) = if state.range_start && !state.range_end {
        (round, square)
    } else if state.range_end && !state.range_start {
        (square, round)
    } else if state.range_middle() || state.preview {
        (square, square)
    } else {
        (round, round)
    };
    Corners {
        top_left: left,
        top_right: right,
        bottom_right: right,
        bottom_left: left,
    }
}

// `outline: 2px solid; outline-offset: 1px`, over the days around it as its
// `z-index: 1` puts it.
fn outline(corners: Corners<Pixels>, color: Rgba) -> impl IntoElement {
    let reach = px(3.);
    let round = |radius: Pixels| {
        if radius > px(0.) {
            radius + reach
        } else {
            radius
        }
    };
    deferred(
        div()
            .absolute()
            .top(-reach)
            .left(-reach)
            .right(-reach)
            .bottom(-reach)
            .border_2()
            .border_color(color)
            .rounded_tl(round(corners.top_left))
            .rounded_tr(round(corners.top_right))
            .rounded_br(round(corners.bottom_right))
            .rounded_bl(round(corners.bottom_left)),
    )
}

// A day's colors fading from one look to another.
#[derive(Clone, Copy)]
struct Fade {
    from: Look,
    to: Look,
    start: Instant,
}

impl Fade {
    fn at(&self, now: Instant) -> Look {
        let t = now.saturating_duration_since(self.start).as_secs_f32() / FADE.as_secs_f32();
        if t >= 1. {
            return self.to;
        }
        let t = ease(t);
        Look {
            bg: blend(self.from.bg, self.to.bg, t),
            fg: blend(self.from.fg, self.to.fg, t),
        }
    }
}

// The days' fades: last frame's, and this frame's as its days render.
struct Fades {
    last: HashMap<SharedString, Fade>,
    next: HashMap<SharedString, Fade>,
    now: Instant,
    // Reduced motion, where changes land at once.
    instant: bool,
    running: bool,
}

impl Fades {
    // What a day shows now on its way to `target`. A day that just appeared
    // starts there, as a new element has nothing to transition from.
    fn look(&mut self, key: SharedString, target: Look) -> Look {
        let now = self.now;
        let fade = match self.last.get(&key) {
            Some(fade) if fade.to == target => *fade,
            Some(fade) if !self.instant => Fade {
                from: fade.at(now),
                to: target,
                start: now,
            },
            _ => Fade {
                from: target,
                to: target,
                start: now,
            },
        };
        let look = fade.at(now);
        self.running |= look != target;
        self.next.insert(key, fade);
        look
    }
}

// CSS `ease`, `cubic-bezier(0.25, 0.1, 0.25, 1)`.
fn ease(t: f32) -> f32 {
    let bezier = |a: f32, b: f32, s: f32| {
        let r = 1. - s;
        3. * r * r * s * a + 3. * r * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0., 1.);
    for _ in 0..20 {
        let mid = (low + high) / 2.;
        if bezier(0.25, 0.25, mid) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    bezier(0.1, 1., (low + high) / 2.)
}

// A color between two, premultiplied as CSS transitions mix them, so a
// transparent end takes on the other's hue.
fn blend(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let a = from.a + (to.a - from.a) * t;
    if a <= 0. {
        return to;
    }
    let channel = |x: f32, y: f32| (x * from.a + (y * to.a - x * from.a) * t) / a;
    Rgba {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a,
    }
}

// Previous or Next (`.lg-cal-nav`), with its caret.
fn step(part: Part<CalendarStepState>, back: bool, palette: &Palette) -> impl IntoElement {
    let disabled = part.state().disabled;
    let (ink, hover, accent, ground) = (
        palette.ink,
        mix(palette.ink, 0.08),
        palette.accent,
        palette.bg,
    );
    part.size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .text_color(palette.ink_2)
        .map(|el| {
            if disabled {
                el.opacity(0.4)
            } else {
                el.cursor_pointer()
                    .hover(move |style| style.bg(hover).text_color(ink))
            }
        })
        // `outline: 2px solid; outline-offset: 2px`, around a button that
        // sits on the tile.
        .focus_visible(move |style| {
            style.shadow(vec![
                BoxShadow::new(px(0.), px(0.), accent.into()).spread_radius(px(4.)),
                BoxShadow::new(px(0.), px(0.), ground.into()).spread_radius(px(2.)),
            ])
        })
        .child(caret(back))
}

// Phosphor's bold caret at 14px (`CaretLeftIcon`, `CaretRightIcon`): a
// 24-unit stroke with round ends on a 256-unit square, in the text color.
fn caret(back: bool) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let unit = f32::from(bounds.size.width) / 256.;
            let (tip, tail) = if back { (80., 160.) } else { (176., 96.) };
            let at = |x: f32, y: f32| bounds.origin + point(px(x * unit), px(y * unit));
            let mut path = PathBuilder::default().with_style(PathStyle::Stroke(StrokeOptions {
                line_width: 24. * unit,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..StrokeOptions::default()
            }));
            path.move_to(at(tail, 48.));
            path.line_to(at(tip, 128.));
            path.line_to(at(tail, 208.));
            let color = window.text_style().color;
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(14.))
}

// The place, the nights and the dates (`.lg-stay-sum`), heard as they change.
fn summary(
    range: Option<&(ISODate, ISODate)>,
    narrow: bool,
    palette: &Palette,
    window: &Window,
) -> impl IntoElement {
    let nights = range.map_or(0, |(start, end)| days_between(start, end));
    let dates: SharedString = match range {
        Some((start, end)) => format_date_range(start, end, Some(LOCALE), None).into(),
        None => "No dates".into(),
    };
    let size = if narrow { 38. } else { 54. };
    let line = size * 0.95;
    let unit = if nights == 1 { " night" } else { " nights" };
    // The unit sits on the count's baseline, as inline text does, in a line
    // as tall as its font (`line-height: normal`).
    let (ascent, descent) = metrics(&font(SANS), px(16.), window);
    let (count_ascent, count_descent) = metrics(&font(SERIF), px(size), window);
    let baseline = browser_baseline(count_ascent, count_descent, px(line));
    let column = div()
        .id("stay")
        .flex()
        .flex_col()
        .gap(px(6.))
        .when(!narrow, |el| el.min_w(px(128.)))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.ink_2)
                .child(lifted(gpui::text!("Alfama, Lisbon"))),
        )
        .child(
            div()
                .flex()
                .items_start()
                .whitespace_nowrap()
                .child(
                    div()
                        .font_family(SERIF)
                        .text_size(px(size))
                        .line_height(px(line))
                        // `letter-spacing: -0.02em`, after its last letter too.
                        .mr(px(-0.02 * size))
                        .child(lifted(gpui::text!(nights.to_string()))),
                )
                .child(
                    div()
                        .mt(baseline - ascent)
                        .font_family(SANS)
                        .text_size(px(16.))
                        .line_height(ascent + descent)
                        .text_color(palette.ink_2)
                        .child(gpui::text!(unit)),
                ),
        )
        .child(
            div()
                .text_size(px(13.))
                .line_height(relative(1.35))
                .text_color(palette.ink_2)
                .child(lifted(gpui::text!(dates))),
        );
    with_aria(
        with_role(column, Role::Group),
        Aria::new().live(Live::Polite),
    )
}

fn main() {
    // `UI_SDK_QUIET` opens it without taking focus, for screenshots.
    let quiet = std::env::var_os("UI_SDK_QUIET").is_some();
    gpui_platform::application().run(move |cx: &mut App| {
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
