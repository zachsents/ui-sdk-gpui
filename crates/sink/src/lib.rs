//! Tiles from the original's sink (`ui-sdk.dev/sink`, `apps/docs/app/sink`),
//! rebuilt on the port: the shared tile frame and palette from
//! `bento/bento.css` and `bento/tile.tsx`. Each tile is an example in
//! `examples/`, which the gallery shows as a section.
//!
//! The frame is the original's at its widest page, where the grid's columns
//! are 345.5px wide and its rows 320px tall. Fonts follow the original's own
//! fallbacks where its fonts aren't installed: Geist for text, Times New
//! Roman for the serif (Geist Serif), Menlo for mono (Geist Mono).
//!
//! A browser places a line's text in its line box up to a pixel away from
//! where gpui does, most often higher. Text wrapped in [`lifted`] sits where
//! a browser puts it; the frame's own text does.
//!
//! # Drops
//!
//! - Copy that narrates what a tile shows or how it works, since an
//!   example's copy is labels, data and placeholders of a word or two: the
//!   bold lead and blurb under each demo (`title`, `blurb`), and the demos'
//!   own hints, subtitles and empty states that say how to use them.
//! - The tags under a tile are labels. The original's are toggles that
//!   filter the sink's grid, which a window of one tile doesn't have.
//! - The tags' fade at the end of their row (`mask-image`): gpui can't mask.
//! - The blueprint tone's faint grid (`background-image`) isn't drawn: no
//!   tile here has that tone.

use std::panic::Location;

use gpui::{
    AnyElement, App, Bounds, BoxShadow, Div, Element, ElementId, Font, FontWeight,
    GlobalElementId, InspectorElementId, InteractiveElement, IntoElement, LayoutId,
    ParentElement, Pixels, Rgba, Role, SharedString, StatefulInteractiveElement, Styled, Text,
    Window, div, point, px, relative, rgba, size,
};
use ui_sdk_color::parse_color;

pub const SANS: &str = "Geist";
pub const SERIF: &str = "Times New Roman";
pub const MONO: &str = "Menlo";

/// The page's padding around the grid (`.sink`).
const PAGE_PADDING: f32 = 28.;

/// An `oklch()` color, as the original's CSS writes it.
pub fn oklch(l: f32, c: f32, h: f32) -> Rgba {
    parse_color(&format!("oklch({l} {c} {h})"))
        .map(Rgba::from)
        .unwrap_or_else(|| rgba(0xff00ffff))
}

/// `color-mix(in oklab, color N%, transparent)`.
pub fn mix(color: Rgba, amount: f32) -> Rgba {
    Rgba {
        a: color.a * amount,
        ..color
    }
}

/// A CSS `box-shadow`, `x y blur spread color`, as gpui draws it. CSS blurs
/// with a Gaussian of half the blur radius; gpui's blur radius is that
/// Gaussian's.
pub fn css_shadow(x: f32, y: f32, blur: f32, spread: f32, color: Rgba) -> BoxShadow {
    BoxShadow::new(px(x), px(y), color.into())
        .blur_radius(px(blur / 2.))
        .spread_radius(px(spread))
}

/// The page's colors (`.sink`).
pub mod page {
    use super::oklch;
    use gpui::Rgba;

    pub fn paper() -> Rgba {
        oklch(0.968, 0.008, 85.)
    }
    pub fn paper_2() -> Rgba {
        oklch(0.944, 0.011, 83.)
    }
    pub fn raised() -> Rgba {
        oklch(0.995, 0.003, 85.)
    }
    pub fn ink() -> Rgba {
        oklch(0.22, 0.015, 60.)
    }
    pub fn ink_2() -> Rgba {
        oklch(0.45, 0.018, 62.)
    }
    pub fn ink_3() -> Rgba {
        oklch(0.62, 0.016, 70.)
    }
    pub fn rule() -> Rgba {
        oklch(0.89, 0.012, 80.)
    }
    pub fn signal() -> Rgba {
        oklch(0.64, 0.19, 38.)
    }
}

/// A tile's tone (`data-tone`), which sets its colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    White,
    Paper,
    Ink,
    Blueprint,
    Signal,
    Sage,
    Sky,
    Butter,
}

/// The colors a tone gives its tile, the `--tile-*` variables.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Rgba,
    pub ink: Rgba,
    pub ink_2: Rgba,
    pub ink_3: Rgba,
    pub rule: Rgba,
    pub surface: Rgba,
    pub surface_2: Rgba,
    pub accent: Rgba,
    pub accent_ink: Rgba,
}

impl Tone {
    pub fn palette(self) -> Palette {
        let base = Palette {
            bg: page::raised(),
            ink: page::ink(),
            ink_2: page::ink_2(),
            ink_3: page::ink_3(),
            rule: page::rule(),
            surface: oklch(1., 0., 0.),
            surface_2: page::paper(),
            accent: page::signal(),
            accent_ink: oklch(1., 0., 0.),
        };
        match self {
            Tone::White => base,
            Tone::Paper => Palette {
                bg: page::paper_2(),
                surface: page::raised(),
                surface_2: oklch(0.925, 0.012, 82.),
                ..base
            },
            Tone::Ink => Palette {
                bg: oklch(0.2, 0.012, 60.),
                ink: oklch(0.96, 0.006, 85.),
                ink_2: oklch(0.76, 0.01, 80.),
                ink_3: oklch(0.58, 0.012, 75.),
                rule: oklch(0.32, 0.012, 65.),
                surface: oklch(0.26, 0.012, 62.),
                surface_2: oklch(0.3, 0.012, 62.),
                accent: oklch(0.74, 0.16, 50.),
                accent_ink: oklch(0.18, 0.02, 60.),
            },
            Tone::Blueprint => Palette {
                bg: oklch(0.32, 0.075, 255.),
                ink: oklch(0.95, 0.025, 240.),
                ink_2: oklch(0.8, 0.05, 245.),
                ink_3: oklch(0.66, 0.06, 250.),
                rule: oklch(0.45, 0.08, 252.),
                surface: oklch(0.37, 0.08, 254.),
                surface_2: oklch(0.42, 0.08, 254.),
                accent: oklch(0.86, 0.12, 200.),
                accent_ink: oklch(0.25, 0.06, 255.),
            },
            Tone::Signal => Palette {
                bg: oklch(0.66, 0.18, 40.),
                ink: oklch(0.99, 0.01, 70.),
                ink_2: oklch(0.94, 0.04, 60.),
                ink_3: oklch(0.88, 0.06, 55.),
                rule: oklch(0.74, 0.15, 45.),
                surface: oklch(0.99, 0.01, 70.),
                surface_2: oklch(0.93, 0.05, 55.),
                accent: oklch(0.22, 0.04, 40.),
                accent_ink: oklch(0.99, 0.01, 70.),
            },
            Tone::Sage => Palette {
                bg: oklch(0.9, 0.035, 150.),
                ink: oklch(0.25, 0.04, 155.),
                ink_2: oklch(0.42, 0.045, 155.),
                ink_3: oklch(0.56, 0.04, 155.),
                rule: oklch(0.8, 0.045, 150.),
                surface: oklch(0.98, 0.012, 140.),
                surface_2: oklch(0.86, 0.04, 150.),
                accent: oklch(0.48, 0.11, 155.),
                accent_ink: oklch(0.98, 0.012, 140.),
            },
            Tone::Sky => Palette {
                bg: oklch(0.91, 0.04, 235.),
                ink: oklch(0.26, 0.05, 250.),
                ink_2: oklch(0.43, 0.06, 248.),
                ink_3: oklch(0.57, 0.05, 245.),
                rule: oklch(0.82, 0.05, 238.),
                surface: oklch(0.99, 0.01, 235.),
                surface_2: oklch(0.87, 0.045, 236.),
                accent: oklch(0.52, 0.15, 255.),
                accent_ink: oklch(0.99, 0.01, 235.),
            },
            Tone::Butter => Palette {
                bg: oklch(0.93, 0.07, 92.),
                ink: oklch(0.28, 0.04, 70.),
                ink_2: oklch(0.45, 0.05, 72.),
                ink_3: oklch(0.58, 0.05, 75.),
                rule: oklch(0.85, 0.07, 88.),
                surface: oklch(0.99, 0.02, 95.),
                surface_2: oklch(0.89, 0.08, 90.),
                accent: oklch(0.3, 0.04, 70.),
                accent_ink: oklch(0.97, 0.04, 95.),
            },
        }
    }

    /// `--tile-shadow`, which the cards and popups on a tile cast. The tile
    /// itself casts none.
    pub fn shadow(self) -> Vec<BoxShadow> {
        match self {
            Tone::Ink => vec![css_shadow(0., 10., 30., -10., mix(oklch(0., 0., 0.), 0.6))],
            Tone::Blueprint => vec![css_shadow(
                0.,
                10.,
                30.,
                -12.,
                mix(oklch(0.15, 0.08, 260.), 0.7),
            )],
            _ => {
                let shade = oklch(0.2, 0.02, 60.);
                // CSS paints its first shadow on top, gpui its last.
                vec![
                    css_shadow(0., 8., 24., -12., mix(shade, 0.18)),
                    css_shadow(0., 1., 2., 0., mix(shade, 0.06)),
                ]
            }
        }
    }
}

/// A tile's size in the bento grid (`data-size`): one cell, two across, two
/// down, or both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    #[default]
    Small,
    Wide,
    Tall,
    Large,
}

impl Size {
    /// How wide the tile is: a column, or two and the 14px gap between them.
    pub fn width(self) -> f32 {
        match self {
            Size::Small | Size::Tall => 345.5,
            Size::Wide | Size::Large => 705.,
        }
    }

    /// How tall: a row, or two and the gap.
    pub fn height(self) -> f32 {
        match self {
            Size::Small | Size::Wide => 320.,
            Size::Tall | Size::Large => 654.,
        }
    }
}

/// One tile (`<Tile>`): the demo on its stage, then the names it uses.
pub struct Tile {
    pub size: Size,
    pub tone: Tone,
    pub uses: &'static [&'static str],
}

impl Tile {
    /// The tile with `demo` on its stage, centered like the original's
    /// `place-items: center`.
    pub fn render(&self, demo: impl IntoElement) -> Div {
        let palette = self.tone.palette();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(self.size.width()))
            .h(px(self.size.height()))
            .overflow_hidden()
            .rounded(px(22.))
            .bg(palette.bg)
            // The edge, `inset 0 0 0 1px`, which takes no room.
            .shadow(vec![
                BoxShadow::new(px(0.), px(0.), mix(palette.ink, 0.06).into())
                    .spread_radius(px(1.))
                    .inset(),
            ])
            .text_color(palette.ink)
            .font_family(SANS)
            .text_size(px(14.))
            .line_height(relative(1.45))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .pt(px(22.))
                    .px(px(22.))
                    .pb(px(16.))
                    .overflow_hidden()
                    .child(demo),
            )
            .child(div().px(px(20.)).pb(px(18.)).child(self.uses(&palette)))
    }

    /// Where the tile's window opens, as big as the tile and the page's
    /// padding around it: centered, or for a quiet screenshot
    /// (`UI_SDK_QUIET`) in the primary display's bottom-right corner, out of
    /// the way.
    pub fn window_bounds(&self, quiet: bool, cx: &App) -> Bounds<Pixels> {
        let window = size(
            px(self.size.width() + 2. * PAGE_PADDING),
            px(self.size.height() + 2. * PAGE_PADDING),
        );
        match cx.primary_display().filter(|_| quiet) {
            Some(display) => {
                let screen = display.bounds();
                let corner = point(
                    screen.origin.x + screen.size.width - window.width,
                    screen.origin.y + screen.size.height - window.height,
                );
                Bounds::new(corner, window)
            }
            None => Bounds::centered(None, window, cx),
        }
    }

    // The names the tile uses, in one row that scrolls sideways.
    fn uses(&self, palette: &Palette) -> impl IntoElement {
        let border = mix(palette.ink, 0.16);
        div()
            .id("tile-uses")
            .role(Role::Group)
            .aria_label("Uses")
            .flex()
            .gap(px(4.))
            .mt(px(7.))
            .mx(px(-4.))
            .py(px(3.))
            .px(px(4.))
            .overflow_x_scroll()
            .children(self.uses.iter().enumerate().map(|(index, name)| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(22.))
                    .px(px(8.))
                    .border_1()
                    .border_color(border)
                    .font_family(MONO)
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(11.))
                    .line_height(relative(1.))
                    .text_color(palette.ink_2)
                    .child(lifted(Text::new(
                        ("tile-use", index).into(),
                        SharedString::new_static(name),
                    )))
            }))
    }
}

/// The page around a tile (`.sink`), as an example's root.
pub fn page(tile: impl IntoElement) -> Div {
    div()
        .size_full()
        .flex()
        .items_start()
        .p(px(PAGE_PADDING))
        .bg(page::paper())
        .child(tile)
}

/// A font's ascent and descent at a size, as gpui lays its lines out.
pub fn metrics(font: &Font, size: Pixels, window: &Window) -> (Pixels, Pixels) {
    let text = window.text_system();
    let id = text.resolve_font(font);
    (text.ascent(id, size), -text.descent(id, size))
}

/// How far below the top of a line `line` tall a browser puts its baseline,
/// for a font's ascent and descent. Blink rounds them to whole pixels and
/// floors the leading above them.
pub fn browser_baseline(ascent: Pixels, descent: Pixels, line: Pixels) -> Pixels {
    ((line - ascent.round() - descent.round()) / 2.).floor() + ascent.round()
}

/// How far above gpui's a browser draws the text of a line `line` tall, in
/// `font` at `size`, or below when it's negative: up to a pixel. gpui
/// centers the font's ascent and descent in the line, its height snapped to
/// device pixels; a browser puts the baseline at [`browser_baseline`].
pub fn lift(font: &Font, size: Pixels, line: Pixels, window: &Window) -> Pixels {
    let (ascent, descent) = metrics(font, size, window);
    let snapped = window.pixel_snap(line);
    (snapped - ascent - descent) / 2. + ascent - browser_baseline(ascent, descent, line)
}

/// `text`, raised by its [`lift`] to where a browser draws it in its lines.
/// It reads the text style the text inherits, so wrap the text itself, and
/// style an element around it.
pub fn lifted(text: impl IntoElement) -> Lifted {
    Lifted {
        text: text.into_any_element(),
    }
}

/// A text raised to where a browser draws it, made by [`lifted`].
pub struct Lifted {
    text: AnyElement,
}

impl Element for Lifted {
    type RequestLayoutState = Pixels;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Pixels) {
        let style = window.text_style();
        let rem = window.rem_size();
        let size = style.font_size.to_pixels(rem);
        let line = style.line_height.to_pixels(size.into(), rem);
        let lift = lift(&style.font(), size, line, window);
        (self.text.request_layout(window, cx), lift)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        lift: &mut Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_element_offset(point(px(0.), -*lift), |window| {
            self.text.prepaint(window, cx)
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _lift: &mut Pixels,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text.paint(window, cx);
    }
}
