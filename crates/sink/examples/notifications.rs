//! The sink's `forms-alerts` tile: two checkbox groups, each with a parent
//! box that checks its whole group, a quiet-hours switch, and a save button
//! that waits.
//!
//! The original's subtitle, "Sent to your phone and inbox", and the quiet
//! hours' "Hold alerts", narrate what the app does, so the heading stands
//! alone and quiet hours show their times.

use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, BoxShadow, Context, CursorStyle, Entity, FocusHandle, Focusable,
    FontFeatures, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent,
    MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, Render, Rgba, SharedString,
    StatefulInteractiveElement, Styled, Svg, TimedTransformation, Transformation, Window,
    WindowBounds, WindowOptions, div, hsla, px, relative, size, svg, text,
};
use ui_sdk_button::{Button, ButtonOptions};
use ui_sdk_color::{ColorMixOptions, ColorSpace, mix_color};
use ui_sdk_checkbox::{
    Checkbox, CheckboxGroup, CheckboxGroupOptions, CheckboxIndicatorProps, CheckboxOptions,
};
use ui_sdk_forms::{Label, LabelOptions};
use ui_sdk_kernel::{ActionOutcome, Key};
use ui_sdk_kernel::modality::focus_visible;
use ui_sdk_motion::{Presence, PresenceTiming, Timing};
use ui_sdk_platform::live_regions;
use ui_sdk_sink::{Palette, SERIF, Size, Tile, Tone, lifted, mix};
use ui_sdk_switch::{Switch, SwitchOptions};

const TILE: Tile = Tile {
    size: Size::Tall,
    tone: Tone::White,
    uses: &["Checkbox", "Switch", "Button"],
};

/// The demo's width (`.fm-alerts`' `max-width`), which the tile's stage
/// always leaves it.
const WIDTH: f32 = 262.;

/// One checkbox in a group: the value the group lists and its label.
struct Choice {
    value: &'static str,
    label: &'static str,
}

const TRIP: &[Choice] = &[
    Choice { value: "gate", label: "Gate changes" },
    Choice { value: "delay", label: "Delays over 30 min" },
    Choice { value: "boarding", label: "Boarding calls" },
    Choice { value: "belt", label: "Baggage belt" },
];

const PRICES: &[Choice] = &[
    Choice { value: "routes", label: "Watched routes" },
    Choice { value: "stays", label: "Hotels on your list" },
];

/// What the quiet-hours label reads as: both its lines.
const QUIET: &str = "Quiet hours 10 pm – 7 am";

// The original's Phosphor icons (`@phosphor-icons/react`), drawn in the
// color of their element's text.
const CHECK: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="currentColor"><path d="M232.49,80.49l-128,128a12,12,0,0,1-17,0l-56-56a12,12,0,1,1,17-17L96,183,215.51,63.51a12,12,0,0,1,17,17Z"/></svg>"#;
const MINUS: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="currentColor"><path d="M228,128a12,12,0,0,1-12,12H40a12,12,0,0,1,0-24H216A12,12,0,0,1,228,128Z"/></svg>"#;
const MOON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="currentColor"><path d="M235.54,150.21a104.84,104.84,0,0,1-37,52.91A104,104,0,0,1,32,120,103.09,103.09,0,0,1,52.88,57.48a104.84,104.84,0,0,1,52.91-37,8,8,0,0,1,10,10,88.08,88.08,0,0,0,109.8,109.8,8,8,0,0,1,10,10Z"/></svg>"#;
const NOTCH: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="currentColor"><path d="M236,128a108,108,0,0,1-216,0c0-42.52,24.73-81.34,63-98.9A12,12,0,1,1,93,50.91C63.24,64.57,44,94.83,44,128a84,84,0,0,0,168,0c0-33.17-19.24-63.43-49-77.09A12,12,0,1,1,173,29.1C211.27,46.66,236,85.48,236,128Z"/></svg>"#;

/// A CSS `cubic-bezier()` timing function at `t`.
fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let curve = |a: f32, b: f32, s: f32| {
        let r = 1. - s;
        3. * a * s * r * r + 3. * b * s * s * r + s * s * s
    };
    let (mut low, mut high) = (0f32, 1f32);
    for _ in 0..24 {
        let mid = (low + high) / 2.;
        if curve(x1, x2, mid) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    curve(y1, y2, (low + high) / 2.)
}

/// `--ease-out`.
fn ease_out(t: f32) -> f32 {
    cubic_bezier(0.22, 1., 0.36, 1., t)
}

/// CSS's `ease`, which a transition without a timing function runs.
fn ease(t: f32) -> f32 {
    cubic_bezier(0.25, 0.1, 0.25, 1., t)
}

/// Where a transition from one color to another is at `t`.
fn blend(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let a = from.a + (to.a - from.a) * t;
    if a <= 0. {
        return Rgba { a: 0., ..to };
    }
    let channel = |f: f32, g: f32| (f * from.a + (g * to.a - f * from.a) * t) / a;
    Rgba {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a,
    }
}

/// The primary button's hover, `color-mix(in oklch, accent 88%, ink)`.
fn accent_hover(palette: &Palette) -> Rgba {
    let oklch = ColorMixOptions::default().space(ColorSpace::Oklch);
    mix_color(&palette.accent.into(), &palette.ink.into(), 0.12, oklch).into()
}

fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// A 14px icon.
fn icon(data: &'static [u8], color: Rgba) -> Svg {
    svg().data(data).flex_none().size(px(14.)).text_color(color)
}

/// Whether `handle` shows its focus ring, `:focus-visible`.
fn ring_shown(handle: &FocusHandle, window: &Window) -> bool {
    handle.is_focused(window) && focus_visible(window)
}

/// The focus outline: 2px of the accent, 2px off the border box of a control
/// with `radius` corners and a `border` it sits inside of.
fn focus_ring(radius: f32, border: f32, palette: &Palette) -> impl IntoElement {
    let inset = px(-4. - border);
    div()
        .absolute()
        .top(inset)
        .left(inset)
        .right(inset)
        .bottom(inset)
        .border_2()
        .border_color(palette.accent)
        .rounded(px(radius + 4.))
}

/// A checkbox (`.fm-box`) with its mark: a check, or a minus while
/// indeterminate, which scales and fades in as the box fills.
fn check_box(checkbox: &Entity<Checkbox>, palette: &Palette, window: &Window, cx: &App) -> impl IntoElement {
    let palette = *palette;
    let mark = Checkbox::indicator(checkbox, CheckboxIndicatorProps::default(), cx);
    // The box's colors run the mark's transition.
    let filled = mark.as_ref().map_or(0., |mark| mark.state().presence.progress);
    let ring = ring_shown(&checkbox.read(cx).focus_handle(cx), window);
    Checkbox::root(checkbox, cx)
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(18.))
        .border(px(1.5))
        .rounded(px(5.))
        .border_color(blend(mix(palette.ink, 0.32), palette.accent, filled))
        .bg(blend(palette.surface, palette.accent, filled))
        .cursor_pointer()
        .children(mark.map(|mark| {
            mark.flex().items_center().justify_center().with(move |el, state| {
                let shown = state.presence.progress;
                let scale = 0.4 + 0.6 * shown;
                el.child(
                    icon(if state.indeterminate { MINUS } else { CHECK }, palette.accent_ink)
                        .with_transformation(Transformation::scale(size(scale, scale)).with_alpha(shown)),
                )
            })
        }))
        .when(ring, |el| el.child(focus_ring(5., 1.5, &palette)))
}

struct Row {
    checkbox: Entity<Checkbox>,
    label: Label,
    text: &'static str,
}

/// A titled group (`AlertGroup`): a parent box with the count, and its
/// checkboxes.
struct AlertGroup {
    title: &'static str,
    items: &'static [Choice],
    value: Vec<SharedString>,
    group: Entity<CheckboxGroup>,
    parent: Entity<Checkbox>,
    parent_label: Label,
    rows: Vec<Row>,
}

/// What the parent's label reads as: the title and the count.
fn parent_text(title: &str, count: usize, total: usize) -> SharedString {
    format!("{title} {count} of {total}").into()
}

impl AlertGroup {
    fn new(
        title: &'static str,
        items: &'static [Choice],
        initial: &[&'static str],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let value: Vec<SharedString> = initial.iter().map(|&value| value.into()).collect();
        let parent_label = Label::new(LabelOptions::default().text(parent_text(title, value.len(), items.len())));
        let label = parent_label.clone();
        let this = cx.weak_entity();
        let group = cx.new(|cx| {
            let options = CheckboxGroupOptions::default()
                .default_value(value.clone())
                .all_values(items.iter().map(|item| item.value))
                .on_value_change(move |value, _details, _window, cx| {
                    label.set_options(
                        LabelOptions::default().text(parent_text(title, value.len(), items.len())),
                        cx,
                    );
                    let value = value.clone();
                    this.update(cx, |group, cx| {
                        group.value = value;
                        cx.notify();
                    })
                    .ok();
                });
            CheckboxGroup::new(options, window, cx)
        });
        let timing = PresenceTiming::new(Timing::ms(140).easing(ease_out));
        let parent = cx.new(|cx| {
            let options = CheckboxOptions::default()
                .group(&group)
                .parent(true)
                .indicator_timing(timing.clone());
            Checkbox::new(options, window, cx)
        });
        let rows = items
            .iter()
            .map(|item| {
                let options = CheckboxOptions::default()
                    .group(&group)
                    .value(item.value)
                    .indicator_timing(timing.clone());
                Row {
                    checkbox: cx.new(|cx| Checkbox::new(options, window, cx)),
                    label: Label::new(LabelOptions::default().text(item.label)),
                    text: item.label,
                }
            })
            .collect();
        Self {
            title,
            items,
            value,
            group,
            parent,
            parent_label,
            rows,
        }
    }
}

impl Render for AlertGroup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = TILE.tone.palette();
        let cx: &App = cx;
        let (count, total) = (self.value.len(), self.items.len());
        let parent = self
            .parent_label
            .root()
            .aria_label(parent_text(self.title, count, total))
            .flex()
            .items_center()
            .gap(px(10.))
            .min_h(px(32.))
            .line_height(relative(1.3))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .child(check_box(&self.parent, &palette, window, cx))
            .child(div().flex_1().child(lifted(self.title)))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::NORMAL)
                    .font_features(tabular())
                    .text_color(palette.ink_3)
                    .child(lifted(format!("{count} of {total}"))),
            );
        let rows = self.rows.iter().map(|row| {
            row.label
                .root()
                .aria_label(row.text)
                .flex()
                .items_center()
                .gap(px(10.))
                .min_h(px(32.))
                .line_height(relative(1.3))
                .text_color(palette.ink_2)
                .cursor_pointer()
                .child(check_box(&row.checkbox, &palette, window, cx))
                .child(lifted(row.text))
        });
        CheckboxGroup::group(&self.group, cx)
            .aria_label(self.title)
            .flex()
            .flex_col()
            .pt(px(8.))
            .border_t_1()
            .border_color(palette.rule)
            .child(parent)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .ml(px(8.))
                    .pl(px(18.))
                    .border_l_1()
                    .border_color(palette.rule)
                    .children(rows),
            )
    }
}

/// The tile's demo (`Alerts`).
struct Example {
    trip: Entity<AlertGroup>,
    prices: Entity<AlertGroup>,
    quiet: Entity<Switch>,
    quiet_label: Label,
    save: Entity<Button>,
    saved: bool,
    // The switch's slide and colors, as their transitions run.
    slide: Presence,
    // The button's hover color.
    hover: Presence,
    // The button's press, `:active`.
    press: Presence,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Tab moves focus, as it does in a browser.
        ui_sdk_focus::bind_tab_keys(cx);
        let trip = cx.new(|cx| AlertGroup::new("Trip updates", TRIP, &["gate", "delay", "boarding"], window, cx));
        let prices = cx.new(|cx| AlertGroup::new("Price drops", PRICES, &[], window, cx));
        let this = cx.weak_entity();
        let quiet = cx.new(|cx| {
            let options = SwitchOptions::default()
                .default_checked(true)
                .on_checked_change(move |checked, _details, window, cx| {
                    let checked = *checked;
                    this.update(cx, |alerts, cx| {
                        alerts.saved = false;
                        alerts.slide_to(checked, window, cx);
                        cx.notify();
                    })
                    .ok();
                });
            Switch::new(options, window, cx)
        });
        let this = cx.weak_entity();
        let save = cx.new(|cx| {
            let options = ButtonOptions::default().action(move |_event, _details, _window, cx| {
                let wait = cx.background_executor().timer(Duration::from_millis(900));
                let this = this.clone();
                let mut cx = cx.to_async();
                ActionOutcome::wait(async move {
                    wait.await;
                    this.update(&mut cx, |alerts, cx| {
                        alerts.saved = true;
                        cx.notify();
                    })
                    .ok();
                    Ok::<(), String>(())
                })
            });
            Button::new(options, window, cx)
        });
        Self {
            trip,
            prices,
            quiet,
            quiet_label: Label::new(LabelOptions::default().text(QUIET)),
            save,
            saved: false,
            slide: Presence::new(true).with_timing(PresenceTiming::new(Timing::ms(200).easing(ease_out))),
            hover: Presence::new(false).with_timing(PresenceTiming::new(Timing::ms(120).easing(ease))),
            press: Presence::new(false).with_timing(PresenceTiming::new(Timing::ms(120).easing(ease_out))),
        }
    }

    /// Runs `presence` toward shown or hidden, as a CSS transition follows
    /// its state.
    fn transition(&mut self, presence: Presence, shown: bool, window: &mut Window, cx: &mut Context<Self>) {
        if shown == presence.is_open() {
            return;
        }
        if shown {
            presence.animate_in(window, cx);
        } else {
            presence.animate_out(self, |_, _, _| {}, window, cx);
        }
    }

    fn slide_to(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.transition(self.slide.clone(), on, window, cx);
    }

    fn hover_to(&mut self, hovered: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.transition(self.hover.clone(), hovered, window, cx);
    }

    fn press_to(&mut self, pressed: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.transition(self.press.clone(), pressed, window, cx);
    }

    /// The quiet-hours row (`.fm-quiet`): a label around its text and the
    /// switch, whose thumb carries a moon.
    fn quiet_row(&self, palette: &Palette, window: &Window, cx: &App) -> impl IntoElement {
        let on = self.slide.state().progress;
        let ring = ring_shown(&self.quiet.read(cx).focus_handle(cx), window);
        let thumb = Switch::thumb(&self.quiet, cx)
            .relative()
            .left(px(20. * on))
            .flex()
            .items_center()
            .justify_center()
            .size(px(22.))
            .rounded(px(6.))
            .bg(palette.surface)
            .shadow(vec![BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., 0.2)).blur_radius(px(2.))])
            .child(icon(MOON, blend(palette.ink_3, palette.ink, on)));
        let switch = Switch::root(&self.quiet, cx)
            .relative()
            .flex_none()
            .flex()
            .w(px(48.))
            .h(px(28.))
            .p(px(3.))
            .rounded(px(9.))
            .bg(blend(mix(palette.ink, 0.18), palette.ink, on))
            .cursor_pointer()
            .child(thumb)
            .when(ring, |el| el.child(focus_ring(9., 0., palette)));
        self.quiet_label
            .root()
            .aria_label(QUIET)
            .flex()
            .items_center()
            .gap(px(12.))
            .p(px(12.))
            .rounded(px(14.))
            .bg(mix(palette.ink, 0.05))
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .flex_col()
                    .gap(px(2.))
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(relative(1.3))
                    .child(lifted("Quiet hours"))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(palette.ink_3)
                            .child(lifted("10 pm – 7 am")),
                    ),
            )
            .child(switch)
    }

    /// The save button (`.b-btn.fm-alerts-save`, primary): a spinner while
    /// it waits, then a check.
    fn save_button(&self, palette: &Palette, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let button = self.save.read(cx);
        let pending = button.is_pending();
        let ring = ring_shown(&button.focus_handle(cx), window);
        // `scale(0.97)` while pressed. gpui can't scale a div, so the button
        // shrinks its box, text and icons by as much, about its center.
        let scale = 1. - 0.03 * self.press.state().progress;
        let (inset_x, inset_y) = (WIDTH * (1. - scale) / 2., 40. * (1. - scale) / 2.);
        let shrink = Transformation::scale(size(scale, scale));
        let content: Vec<AnyElement> = if pending {
            let spin = TimedTransformation::rotation(Duration::from_millis(700), shrink);
            vec![
                icon(NOTCH, palette.accent_ink)
                    .with_timed_transformation(spin)
                    .into_any_element(),
                lifted(text!("Saving")).into_any_element(),
            ]
        } else if self.saved {
            vec![
                icon(CHECK, palette.accent_ink)
                    .with_transformation(shrink)
                    .into_any_element(),
                lifted(text!("Preferences saved")).into_any_element(),
            ]
        } else {
            vec![lifted(text!("Save preferences")).into_any_element()]
        };
        let root = Button::root(&self.save, cx)
            .absolute()
            .top(px(inset_y))
            .bottom(px(inset_y))
            .left(px(inset_x))
            .right(px(inset_x))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .px(px(12.))
            .border_1()
            .border_color(palette.accent)
            .rounded(px(10. * scale))
            .bg(blend(palette.accent, accent_hover(palette), self.hover.state().progress))
            .text_color(palette.accent_ink)
            .text_size(px(14. * scale))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .cursor(if pending { CursorStyle::Arrow } else { CursorStyle::PointingHand })
            .on_hover(cx.listener(|alerts, hovered: &bool, window, cx| alerts.hover_to(*hovered, window, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|alerts, _: &MouseDownEvent, window, cx| alerts.press_to(true, window, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|alerts, _: &MouseUpEvent, window, cx| alerts.press_to(false, window, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|alerts, _: &MouseUpEvent, window, cx| alerts.press_to(false, window, cx)),
            )
            // Space holds a focused button pressed, as in a browser.
            .on_key_down(cx.listener(|alerts, event: &KeyDownEvent, window, cx| {
                if Key::of(&event.keystroke) == Key::Space {
                    alerts.press_to(true, window, cx);
                }
            }))
            .on_key_up(cx.listener(|alerts, event: &KeyUpEvent, window, cx| {
                if Key::of(&event.keystroke) == Key::Space {
                    alerts.press_to(false, window, cx);
                }
            }))
            .children(content)
            .when(ring, |el| el.child(focus_ring(10., 1., palette)));
        div().relative().h(px(40.)).child(root)
    }
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = TILE.tone.palette();
        let head = div()
            .font_family(SERIF)
            .font_weight(FontWeight::NORMAL)
            .text_size(px(30.))
            .line_height(relative(1.05))
            .child(lifted(text!("Notifications")));
        let demo = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .w_full()
            .max_w(px(WIDTH))
            .child(head)
            .child(self.trip.clone())
            .child(self.prices.clone())
            .child(self.quiet_row(&palette, window, cx))
            .child(self.save_button(&palette, window, cx))
            .child(live_regions(window));
        ui_sdk_sink::page(TILE.render(demo))
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
