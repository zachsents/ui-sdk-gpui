//! The sink's `language-sheet` tile: a trip budget sheet whose cells select
//! as a block and copy and paste as tab-separated text.
//!
//! The original's hint under the sheet, "Shift + arrows select, then copy
//! or paste", and its note before anything is copied, "Tab-separated, like
//! any sheet", narrate how it works, so neither shows here.

use std::sync::Arc;

use gpui::{
    App, AppContext, ClipboardItem, Context, Div, FocusHandle, FontFeatures, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, PathBuilder, Render, Rgba, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, canvas,
    div, point, prelude::FluentBuilder, px, relative, text,
};
use ui_sdk_color::{Color, ColorMixOptions, ColorSpace, convert_color, mix_color};
use ui_sdk_drag_and_drop::on_pointer_down;
use ui_sdk_focus::{GridCell, GridFocus, GridFocusOptions};
use ui_sdk_kernel::modality::focus_visible;
use ui_sdk_kernel::roles::Role as AriaRole;
use ui_sdk_kernel::{Aria, ChangeOptions, FocusMode, Instance, Live, Need, Part, Reason, with_aria};
use ui_sdk_number::format_number;
use ui_sdk_platform::{is_apple, live_regions, matches_shortcut};
use ui_sdk_selection::{
    CellBounds, CellRange, CellRangeOptions, CellRangeSelection, RangeEdges, cells_to_text,
    create_cell_range, text_to_cells,
};
use ui_sdk_sink::{MONO, Palette, SANS, SERIF, Size, Tile, Tone, lifted};

const TILE: Tile = Tile {
    size: Size::Wide,
    tone: Tone::White,
    uses: &["cellsToText", "createCellRange", "textToCells", "createGridFocus", "formatNumber"],
};

const SHEET_COLS: [&str; 4] = ["A", "B", "C", "D"];
const SHEET_START: [[&str; 4]; 5] = [
    ["Stop", "Nights", "Hotel €", "Meals €"],
    ["Porto", "3", "276", "144"],
    ["Bologna", "2", "236", "122"],
    ["Ljubljana", "2", "172", "78"],
    ["Split", "4", "416", "220"],
];

/// The keys a cell hands to the cell range.
const CELL_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Home",
    "End",
    "mod+Home",
    "mod+End",
    "PageUp",
    "PageDown",
    "shift+ArrowUp",
    "shift+ArrowDown",
    "shift+ArrowLeft",
    "shift+ArrowRight",
    "shift+Home",
    "shift+End",
    "shift+ ",
    "mod+ ",
    "mod+a",
    "Escape",
];

/// Geist's `line-height: normal`.
const NORMAL: f32 = 1.3;

/// The board's corner radius inside its border, which clips its corner cells.
const INNER_RADIUS: f32 = 9.;

fn sheet_start() -> Vec<Vec<String>> {
    SHEET_START
        .iter()
        .map(|row| row.iter().map(|cell| cell.to_string()).collect())
        .collect()
}

fn sheet_bounds(range: Option<CellRange>, active: Option<GridCell>) -> Option<CellBounds> {
    if let Some(range) = range {
        return Some(CellBounds {
            top: range.anchor.row.min(range.end.row),
            bottom: range.anchor.row.max(range.end.row),
            left: range.anchor.col.min(range.end.col),
            right: range.anchor.col.max(range.end.col),
        });
    }
    active.map(|active| CellBounds {
        top: active.row,
        bottom: active.row,
        left: active.col,
        right: active.col,
    })
}

/// `Number(text)`, as the original sums cells: trimmed, empty reads as 0,
/// and hex, octal and binary read too.
fn js_number(text: &str) -> f64 {
    let text =
        text.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}');
    if text.is_empty() {
        return 0.;
    }
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if text.get(..2).is_some_and(|head| head.eq_ignore_ascii_case(prefix)) {
            let digits = &text[2..];
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return digits.chars().fold(0., |value, c| {
                value * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
            });
        }
    }
    text.parse().unwrap_or(f64::NAN)
}

/// `/^-?\d/`, which right-aligns a cell as a number.
fn is_number(text: &str) -> bool {
    text.strip_prefix('-')
        .unwrap_or(text)
        .starts_with(|c: char| c.is_ascii_digit())
}

/// `color-mix(in oklch, color N%, base)`. An achromatic base's hue is
/// powerless, so the mix keeps the color's, as browsers do.
fn mix_oklch(color: Rgba, amount: f64, base: Rgba) -> Rgba {
    let mut base = convert_color(&Color::from(base), ColorSpace::Oklch);
    if base.coords[1].is_some_and(|chroma| chroma.abs() < 1e-4) {
        base.coords[2] = None;
    }
    let color = convert_color(&Color::from(color), ColorSpace::Oklch);
    let options = ColorMixOptions::default().space(ColorSpace::Oklch);
    Rgba::from(mix_color(&base, &color, amount, options))
}

fn tabular_nums() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// One of the grid's tracks, `24px minmax(0, 1.35fr) repeat(3, minmax(0, 1fr))`
/// after the row numbers.
fn track(col: usize) -> Div {
    div()
        .flex()
        .min_w_0()
        .flex_basis(px(0.))
        .flex_grow(if col == 0 { 1.35 } else { 1. })
}

#[derive(Clone, Debug, PartialEq)]
struct SheetRootState {
    label: SharedString,
    sum: f64,
    count: usize,
    note: SharedString,
}

#[derive(Clone, Debug, PartialEq)]
struct SheetCellState {
    text: SharedString,
    /// `aria-selected` and `data-selected`.
    selected: bool,
    /// `data-range-edge`.
    edges: RangeEdges,
    /// `data-head`.
    head: bool,
    /// `data-number`.
    number: bool,
    /// `data-active`.
    active: bool,
}

struct Example {
    instance: Instance,
    grid: GridFocus,
    cell_range: CellRangeSelection<Self>,
    range: Option<CellRange>,
    cells: Vec<Vec<String>>,
    note: String,
    copy_focus: FocusHandle,
    reset_focus: FocusHandle,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Tab moves focus, as it does in a browser.
        ui_sdk_focus::bind_tab_keys(cx);
        let grid = GridFocus::new(
            GridFocusOptions::default()
                .rows(SHEET_START.len())
                .cols(SHEET_COLS.len()),
            window,
            cx,
        );
        let cell_range = create_cell_range(
            CellRangeOptions::new(&grid, |this: &Self| this.cells.len(), |_| SHEET_COLS.len())
                .controlled(
                    |this: &Self| this.range,
                    |this, range, _details, _window, cx| {
                        this.range = range;
                        cx.notify();
                        true
                    },
                ),
            cx,
        );
        Self {
            instance: Instance::new("lg-sheet"),
            grid,
            cell_range,
            range: Some(CellRange::new(GridCell::new(1, 2), GridCell::new(3, 3))),
            cells: sheet_start(),
            note: String::new(),
            copy_focus: cx.focus_handle(),
            reset_focus: cx.focus_handle(),
        }
    }

    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.cell_range.text(self, |cell| {
            self.cells
                .get(cell.row)
                .and_then(|row| row.get(cell.col))
                .cloned()
                .unwrap_or_default()
        });
        if text.is_empty() {
            return;
        }
        window.prevent_default();
        self.note = format!("Copied {} rows", text.split('\n').count());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.notify();
    }

    fn paste(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let rows_in = text_to_cells(text);
        let Some(bounds) = self.cell_range.bounds(self) else {
            return;
        };
        if rows_in.is_empty() {
            return;
        }
        window.prevent_default();
        let mut cells = self.cells.clone();
        let mut bottom = bounds.top;
        let mut right = bounds.left;
        for (i, row) in rows_in.into_iter().enumerate() {
            for (j, value) in row.into_iter().enumerate() {
                let (r, c) = (bounds.top + i, bounds.left + j);
                if r >= cells.len() || c >= SHEET_COLS.len() {
                    continue;
                }
                cells[r][c] = value;
                bottom = bottom.max(r);
                right = right.max(c);
            }
        }
        self.cells = cells;
        self.note = "Pasted into the sheet".into();
        cx.notify();
        let pasted = CellRange::new(
            GridCell::new(bounds.top, bounds.left),
            GridCell::new(bottom, right),
        );
        self.cell_range.clone().select(
            self,
            Some(pasted),
            ChangeOptions::reason(Reason::Imperative),
            window,
            cx,
        );
    }

    fn copy_sheet(&mut self, cx: &mut Context<Self>) {
        let text = cells_to_text(&self.cells);
        // JavaScript counts a string's length in UTF-16 units.
        self.note = format!("Sheet copied, {} characters", text.encode_utf16().count());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.cells = sheet_start();
        self.note.clear();
        cx.notify();
    }

    fn root_state(&self) -> SheetRootState {
        let note = SharedString::from(self.note.clone());
        let Some(b) = sheet_bounds(self.range, self.grid.active()) else {
            return SheetRootState {
                label: "–".into(),
                sum: 0.,
                count: 0,
                note,
            };
        };
        let mut sum = 0.;
        let mut count = 0;
        for r in b.top..=b.bottom {
            for c in b.left..=b.right {
                let text = &self.cells[r][c];
                let value = js_number(text);
                if r > 0 && !text.is_empty() && value.is_finite() {
                    sum += value;
                    count += 1;
                }
            }
        }
        let from = format!("{}{}", SHEET_COLS[b.left], b.top + 1);
        let to = format!("{}{}", SHEET_COLS[b.right], b.bottom + 1);
        SheetRootState {
            label: if from == to { from } else { format!("{from}:{to}") }.into(),
            sum,
            count,
            note,
        }
    }

    fn cell_state(&self, cell: GridCell) -> SheetCellState {
        let text = self
            .cells
            .get(cell.row)
            .and_then(|row| row.get(cell.col))
            .cloned()
            .unwrap_or_default();
        let place = self.cell_range.cell(self, &cell);
        SheetCellState {
            selected: place.selected,
            edges: place.edges,
            head: cell.row == 0,
            number: cell.row > 0 && is_number(&text),
            active: self.grid.is_active(&cell),
            text: text.into(),
        }
    }

    fn cell(&self, cell: GridCell, palette: &Palette, cx: &Context<Self>) -> impl IntoElement + use<> {
        let state = self.cell_state(cell);
        let (text, selected, active) = (state.text.clone(), state.selected, state.active);
        let edges = selected.then_some(state.edges);
        let handle = self.grid.focus_handle(cell, cx);
        let tab_stop = self.cell_range.bind(cell, &handle, cx);
        let key = format!("lg-cell-{}-{}", cell.row, cell.col);
        let keyed = cx.weak_entity();
        let pressed = cx.weak_entity();
        let part = Part::new(&self.instance, "cell", Some(&key), state)
            .aria_role(AriaRole::GridCell)
            .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
            .aria_selected(selected)
            .on_keys(CELL_KEYS, move |event, window, cx| {
                keyed
                    .update(cx, |this, cx| {
                        this.cell_range.clone().keydown(this, event, window, cx);
                    })
                    .ok();
            });
        let part = on_pointer_down(part, move |event, window, cx| {
            pressed
                .update(cx, |this, cx| {
                    this.cell_range
                        .clone()
                        .pointer_down(this, event, cell, window, cx);
                })
                .ok();
        });
        let part = if text.is_empty() {
            part
        } else {
            part.aria_label(text.clone())
        };
        let last_row = self.cells.len().saturating_sub(1);
        let last_col = SHEET_COLS.len() - 1;
        let top_right = cell.row == 0 && cell.col == last_col;
        let bottom_right = cell.row == last_row && cell.col == last_col;
        let corners = move |el: Div| {
            el.when(top_right, |el| el.rounded_tr(px(INNER_RADIUS)))
                .when(bottom_right, |el| el.rounded_br(px(INNER_RADIUS)))
        };
        let accent = palette.accent;
        let surface = palette.surface;
        let surface_2 = palette.surface_2;
        let ink_2 = palette.ink_2;
        let rule = palette.rule;
        self.cell_range
            .measure(cell, part)
            .with(move |el, state| {
                el.flex_1()
                    .flex()
                    .items_center()
                    .h(px(28.))
                    .min_w_0()
                    .px(px(8.))
                    .border_l_1()
                    .border_color(rule)
                    .font_family(MONO)
                    .font_weight(FontWeight::NORMAL)
                    .text_size(px(13.))
                    .line_height(px(13. * NORMAL))
                    .whitespace_nowrap()
                    .cursor_crosshair()
                    .when(top_right, |el| el.rounded_tr(px(INNER_RADIUS)))
                    .when(bottom_right, |el| el.rounded_br(px(INNER_RADIUS)))
                    .when(state.head, |el| {
                        el.bg(surface_2)
                            .font_family(SANS)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(12.))
                            .line_height(px(12. * NORMAL))
                            .text_color(ink_2)
                    })
                    .when(state.number, |el| el.justify_end().font_features(tabular_nums()))
                    .when(state.selected, |el| el.bg(mix_oklch(accent, 0.14, surface)))
            })
            .when_some(edges, |el, edges| {
                el.child(corners(
                    div()
                        .absolute()
                        .inset_0()
                        .border_color(accent)
                        .when(edges.top, |el| el.border_t_2())
                        .when(edges.right, |el| el.border_r_2())
                        .when(edges.bottom, |el| el.border_b_2())
                        .when(edges.left, |el| el.border_l_2()),
                ))
            })
            .child(text)
            // `outline: 2px` at `outline-offset: -2px` covers the left rule too.
            .when(active, |el| {
                el.child(corners(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom_0()
                        .left(px(-1.))
                        .border_2()
                        .border_color(accent),
                ))
            })
    }

    fn board(&self, palette: &Palette, cx: &Context<Self>) -> impl IntoElement {
        self.grid.begin(cx);
        let last_row = self.cells.len().saturating_sub(1);
        let clipboard = cx.weak_entity();
        Part::new(&self.instance, "board", None, ())
            .aria_role(AriaRole::Grid)
            .unneeded(Need::Keys, "Focus stays on a cell, whose keydown handles them.")
            .aria_label("Trip budget")
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if window.default_prevented() {
                    return;
                }
                let apple = is_apple(cx);
                if matches_shortcut(&event.keystroke, "mod+c", apple) {
                    clipboard.update(cx, |this, cx| this.copy(window, cx)).ok();
                } else if matches_shortcut(&event.keystroke, "mod+v", apple) {
                    let text = cx
                        .read_from_clipboard()
                        .and_then(|item| item.text())
                        .unwrap_or_default();
                    clipboard
                        .update(cx, |this, cx| this.paste(&text, window, cx))
                        .ok();
                }
            })
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_1()
            .border_color(palette.rule)
            .rounded(px(10.))
            .bg(palette.surface)
            .children((0..self.cells.len()).map(|row| {
                div()
                    .id(("lg-sheet-row", row))
                    .role(Role::Row)
                    .flex()
                    .when(row > 0, |el| el.border_t_1().border_color(palette.rule))
                    .child(
                        div()
                            .w(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(palette.surface_2)
                            .font_family(MONO)
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(11.))
                            .line_height(px(11. * NORMAL))
                            .text_color(palette.ink_3)
                            .when(row == 0, |el| el.rounded_tl(px(INNER_RADIUS)))
                            .when(row == last_row, |el| el.rounded_bl(px(INNER_RADIUS)))
                            .child(SharedString::from((row + 1).to_string())),
                    )
                    .children(
                        (0..SHEET_COLS.len())
                            .map(|col| track(col).child(self.cell(GridCell::new(row, col), palette, cx))),
                    )
            }))
            .child(self.cell_range.listener())
    }

    fn main(&self, palette: &Palette, cx: &Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .max_w(px(390.))
            .child(
                div()
                    .flex()
                    .h(px(18.))
                    .child(div().w(px(24.)).flex_none())
                    .children(SHEET_COLS.iter().enumerate().map(|(col, letter)| {
                        track(col)
                            .items_center()
                            .justify_center()
                            .font_family(MONO)
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(11.))
                            .line_height(px(11. * NORMAL))
                            .text_color(palette.ink_3)
                            .child(*letter)
                    })),
            )
            .child(self.board(palette, cx))
    }

    fn side(
        &self,
        state: &SheetRootState,
        palette: &Palette,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let copied = cx.weak_entity();
        let reset = cx.weak_entity();
        let note = (!state.note.is_empty()).then(|| lifted(text!(state.note.clone())));
        let aside = div()
            .id("lg-sheet-side")
            .role(Role::Complementary)
            .flex()
            .flex_col()
            .gap(px(10.))
            .w(px(150.))
            .flex_none()
            .child(
                div()
                    .font_family(SERIF)
                    .text_size(px(38.))
                    .line_height(relative(1.))
                    .child(lifted(text!(state.label.clone()))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(18.))
                    .child(
                        div()
                            .child(dt(text!("Sum"), palette))
                            .child(dd(text!(format_number(Some(state.sum), Some("en-GB"), None)))),
                    )
                    .child(
                        div()
                            .child(dt(text!("Count"), palette))
                            .child(dd(text!(state.count.to_string()))),
                    ),
            )
            .child(
                div()
                    .min_h(px(32.))
                    .text_size(px(12.))
                    .line_height(relative(1.35))
                    .text_color(palette.ink_3)
                    .children(note),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        button(
                            Part::new(&self.instance, "copy", None, ()),
                            &self.copy_focus,
                            palette,
                            window,
                        )
                        .on_press(move |_event, _window, cx| {
                            copied.update(cx, |this, cx| this.copy_sheet(cx)).ok();
                        })
                        .bg(palette.surface)
                        .border_color(palette.rule)
                        .hover(|style| style.border_color(palette.ink_3))
                        .child(icon(copy_icon, palette.ink))
                        .child(lifted(text!("Copy all"))),
                    )
                    .child(
                        button(
                            Part::new(&self.instance, "reset", None, ()),
                            &self.reset_focus,
                            palette,
                            window,
                        )
                        .on_press(move |_event, _window, cx| {
                            reset.update(cx, |this, cx| this.reset(cx)).ok();
                        })
                        .aria_label("Reset sheet")
                        .hover(|style| style.bg(ui_sdk_sink::mix(palette.ink, 0.07)))
                        .child(icon(reset_icon, palette.ink)),
                    ),
            );
        with_aria(aside, Aria::new().live(Live::Polite))
    }
}

fn dt(label: impl IntoElement, palette: &Palette) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .text_color(palette.ink_3)
        .child(lifted(label))
}

fn dd(value: impl IntoElement) -> impl IntoElement {
    div()
        .font_family(MONO)
        .font_weight(FontWeight::MEDIUM)
        .text_size(px(15.))
        .line_height(px(15. * NORMAL))
        .font_features(tabular_nums())
        .child(value)
}

/// A `.b-btn.lg-sheet-btn`: transparent, as the ghost variant, until styled.
fn button(part: Part, focus: &FocusHandle, palette: &Palette, window: &Window) -> Part {
    let ring = (focus.is_focused(window) && focus_visible(window)).then(|| {
        // `outline: 2px solid var(--signal)` at `outline-offset: 2px`, from
        // inside the 1px border.
        div()
            .absolute()
            .top(px(-5.))
            .right(px(-5.))
            .bottom(px(-5.))
            .left(px(-5.))
            .border_2()
            .border_color(ui_sdk_sink::page::signal())
            .rounded(px(14.))
    });
    part.aria_role(AriaRole::Button)
        .focus_mode(FocusMode::Native, Some(focus), false)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(10.))
        .border_1()
        .border_color(gpui::transparent_black())
        .rounded(px(10.))
        .text_color(palette.ink)
        .font_weight(FontWeight::MEDIUM)
        .text_size(px(12.))
        .whitespace_nowrap()
        .cursor_pointer()
        .children(ring)
}

/// A 14px Phosphor icon, drawn from its bold path on a 256 grid.
fn icon(draw: fn(&mut PathBuilder), color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let mut path = PathBuilder::fill();
            draw(&mut path);
            path.scale(14. / 256.);
            path.translate(bounds.origin);
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(14.))
    .flex_none()
}

fn at(x: f32, y: f32) -> gpui::Point<gpui::Pixels> {
    point(px(x), px(y))
}

/// Phosphor's `CopyIcon`, bold.
fn copy_icon(path: &mut PathBuilder) {
    let r = at(12., 12.);
    path.move_to(at(216., 28.));
    path.line_to(at(88., 28.));
    path.arc_to(r, px(0.), false, false, at(76., 40.));
    path.line_to(at(76., 76.));
    path.line_to(at(40., 76.));
    path.arc_to(r, px(0.), false, false, at(28., 88.));
    path.line_to(at(28., 216.));
    path.arc_to(r, px(0.), false, false, at(40., 228.));
    path.line_to(at(168., 228.));
    path.arc_to(r, px(0.), false, false, at(180., 216.));
    path.line_to(at(180., 180.));
    path.line_to(at(216., 180.));
    path.arc_to(r, px(0.), false, false, at(228., 168.));
    path.line_to(at(228., 40.));
    path.arc_to(r, px(0.), false, false, at(216., 28.));
    path.close();
    path.move_to(at(156., 204.));
    path.line_to(at(52., 204.));
    path.line_to(at(52., 100.));
    path.line_to(at(156., 100.));
    path.close();
    path.move_to(at(204., 156.));
    path.line_to(at(180., 156.));
    path.line_to(at(180., 88.));
    path.arc_to(r, px(0.), false, false, at(168., 76.));
    path.line_to(at(100., 76.));
    path.line_to(at(100., 52.));
    path.line_to(at(204., 52.));
    path.close();
}

/// Phosphor's `ArrowCounterClockwiseIcon`, bold.
fn reset_icon(path: &mut PathBuilder) {
    let r = at(12., 12.);
    path.move_to(at(228., 128.));
    path.arc_to(at(100., 100.), px(0.), false, true, at(129.34, 228.));
    path.line_to(at(128., 228.));
    path.arc_to(at(99.39, 99.39), px(0.), false, true, at(59.38, 200.71));
    path.arc_to(r, px(0.), false, true, at(75.86, 183.26));
    path.arc_to(at(76., 76.), px(0.), true, false, at(74.29, 74.26));
    path.cubic_bezier_to(at(73.9, 74.63), at(74.16, 74.39), at(74.04, 74.51));
    path.line_to(at(54.89, 92.));
    path.line_to(at(72., 92.));
    path.arc_to(r, px(0.), false, true, at(72., 116.));
    path.line_to(at(24., 116.));
    path.arc_to(r, px(0.), false, true, at(12., 104.));
    path.line_to(at(12., 56.));
    path.arc_to(r, px(0.), false, true, at(36., 56.));
    path.line_to(at(36., 76.72));
    path.line_to(at(57.48, 57.06));
    path.arc_to(at(100., 100.), px(0.), false, true, at(228., 128.));
    path.close();
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = TILE.tone.palette();
        let state = self.root_state();
        let side = self.side(&state, &palette, window, cx);
        let sheet = Part::new(&self.instance, "root", None, state)
            .flex()
            .items_center()
            .gap(px(20.))
            .w_full()
            .child(self.main(&palette, cx))
            .child(side);
        ui_sdk_sink::page(TILE.render(sheet)).child(live_regions(window))
    }
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
