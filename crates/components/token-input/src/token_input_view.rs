//! Port of `components/token-input/token-input-view.ts`: what the editor
//! shows, and where.
//!
//! The original renders the value into a contenteditable element, text
//! nodes and a host per token, `span[contenteditable=false]`, and reads the
//! browser's selection back as offsets. gpui has no editable element, so the
//! editor is an element of its own that lays the value out the way that DOM
//! is: text wraps as `white-space: pre-wrap` and `overflow-wrap: anywhere`
//! have it, and each token's host is an atomic box in the line, which gpui
//! lays out from what the app renders in it. Offsets map to places and back
//! through that layout, which the caret, the selection, clicks, the
//! platform's text input and assistive technology all read.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::accesskit::{self, NodeId, Rect, TextDirection, TextPosition, TextSelection};
use gpui::{
    A11ySubtreeBuilder, AnyElement, App, AvailableSpace, Bounds, ContentMask, DispatchPhase, Element,
    ElementId, ElementInputHandler, Entity, GlobalElementId, Hsla, InspectorElementId,
    InteractiveElement, IntoElement, LayoutId, MouseButton, MouseMoveEvent, ParentElement, Pixels,
    Point, Role, ShapedLine, SharedString, Size, Style, Styled, TextAlign, TextRun, TextStyle,
    UnderlineStyle, Window, div, fill, point, px, size,
};
use ui_sdk_kernel::Aria;
use ui_sdk_kernel::part::{with_aria, with_role};
use ui_sdk_native::{TextFieldAlign, TextFieldStyle};
use ui_sdk_text::{TextRange, Token};
use unicode_segmentation::UnicodeSegmentation;

use crate::token_input::{HostStyle, RenderToken, TokenInput, TokenInputTokenState};
use crate::token_input_editing::Composition;
use crate::token_input_model::{Segment, TOKEN_CHAR, TOKEN_LEN, slice_value, snap_range};

/// What the editor shows: the value's text, each token as U+FFFC, with an
/// input method's marked text in place of what it composes over.
#[derive(Clone, Debug)]
pub(crate) struct Shown {
    pub text: SharedString,
    /// The tokens, by where each starts in the shown text.
    pub tokens: Vec<(usize, Token)>,
    /// Where the marked text is.
    pub marked: Option<Range<usize>>,
    // The range of the value the marked text stands in for.
    composed: Option<TextRange>,
}

// A value's offset text from `base`, with where each token starts.
fn layout_of(value: &[Segment], base: usize, text: &mut String, tokens: &mut Vec<(usize, Token)>) {
    for segment in value {
        match segment {
            Segment::Text(part) => text.push_str(part),
            Segment::Token(token) => {
                tokens.push((base + text.len(), token.clone()));
                text.push(TOKEN_CHAR);
            }
        }
    }
}

impl Shown {
    pub fn of(value: &[Segment], composition: Option<&Composition>) -> Self {
        let mut text = String::new();
        let mut tokens = Vec::new();
        let Some(composition) = composition else {
            layout_of(value, 0, &mut text, &mut tokens);
            return Self {
                text: text.into(),
                tokens,
                marked: None,
                composed: None,
            };
        };
        let range = snap_range(value, composition.range);
        layout_of(&slice_value(value, 0, range.start), 0, &mut text, &mut tokens);
        let start = text.len();
        text.push_str(&composition.text);
        let marked = start..text.len();
        let mut after = String::new();
        layout_of(&slice_value(value, range.end, usize::MAX), text.len(), &mut after, &mut tokens);
        text.push_str(&after);
        Self {
            text: text.into(),
            tokens,
            marked: Some(marked),
            composed: Some(range),
        }
    }

    /// Plain text, like the placeholder.
    pub fn plain(text: &SharedString) -> Self {
        Self {
            text: text.clone(),
            tokens: Vec::new(),
            marked: None,
            composed: None,
        }
    }

    /// The shown offset of a value offset.
    pub fn to_shown(&self, offset: usize) -> usize {
        match (&self.composed, &self.marked) {
            (Some(range), Some(marked)) => {
                if offset <= range.start {
                    offset
                } else if offset >= range.end {
                    offset - (range.end - range.start) + marked.len()
                } else {
                    marked.start
                }
            }
            _ => offset.min(self.text.len()),
        }
    }

    /// The value offset of a shown offset.
    pub fn to_value(&self, offset: usize) -> usize {
        match (&self.composed, &self.marked) {
            (Some(range), Some(marked)) => {
                if offset <= marked.start {
                    offset
                } else if offset >= marked.end {
                    offset - marked.len() + (range.end - range.start)
                } else {
                    range.start
                }
            }
            _ => offset.min(self.text.len()),
        }
    }
}

/// The colors a frame paints with, once the inherited text color is known.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Colors {
    pub text: Hsla,
    pub placeholder: Hsla,
    pub caret: Hsla,
    pub caret_width: Pixels,
    pub selection: Hsla,
    pub selected_text: Hsla,
    pub composition_underline: UnderlineStyle,
}

// The native field's defaults: everything the text color, the placeholder
// at half its opacity and the selection at a fifth.
fn resolve(style: &TextFieldStyle, inherited: Hsla) -> Colors {
    let text = style.text_color.unwrap_or(inherited);
    Colors {
        text,
        placeholder: style.placeholder_color.unwrap_or(text.opacity(0.5)),
        caret: style.caret_color.unwrap_or(text),
        caret_width: style.caret_width,
        selection: style.selection_color.unwrap_or(text.opacity(0.2)),
        selected_text: style.selected_text_color.unwrap_or(text),
        composition_underline: UnderlineStyle {
            color: Some(style.composition_underline.color.unwrap_or(text)),
            ..style.composition_underline
        },
    }
}

// The style's alignment in the app's reading direction, else the part's.
fn align_of(style: &TextFieldStyle, inherited: TextAlign, rtl: bool) -> TextAlign {
    match (style.text_align, rtl) {
        (None, _) => inherited,
        (Some(TextFieldAlign::Center), _) => TextAlign::Center,
        (Some(TextFieldAlign::Start), false) | (Some(TextFieldAlign::End), true) => TextAlign::Left,
        (Some(TextFieldAlign::Start), true) | (Some(TextFieldAlign::End), false) => TextAlign::Right,
    }
}

/// How the shown text is drawn: its colors, the selected text's, and the
/// underline under what an input method composes.
pub(crate) struct Decorations {
    pub text: Hsla,
    pub selected_text: Hsla,
    pub underline: UnderlineStyle,
    pub selected: Option<Range<usize>>,
    pub marked: Option<Range<usize>>,
}

// The runs of the shown text from `start` to `end`.
fn runs(style: &TextStyle, start: usize, end: usize, decorations: Option<&Decorations>) -> Vec<TextRun> {
    let Some(decorations) = decorations else {
        return vec![style.to_run(end - start)];
    };
    let mut cuts = vec![start, end];
    for range in decorations.marked.iter().chain(decorations.selected.iter()) {
        cuts.push(range.start.clamp(start, end));
        cuts.push(range.end.clamp(start, end));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let within = |range: &Option<Range<usize>>, at: usize| {
        range.as_ref().is_some_and(|range| range.start <= at && at < range.end)
    };
    let base = style.to_run(0);
    cuts.windows(2)
        .map(|pair| TextRun {
            len: pair[1] - pair[0],
            color: if within(&decorations.selected, pair[0]) {
                decorations.selected_text
            } else {
                decorations.text
            },
            underline: within(&decorations.marked, pair[0]).then_some(decorations.underline),
            ..base.clone()
        })
        .collect()
}

/// One piece of a row: text, or a token's host.
pub(crate) enum Item {
    Text {
        start: usize,
        /// From the row's start.
        x: Pixels,
        line: ShapedLine,
    },
    Chip {
        start: usize,
        /// The token's place among the shown tokens.
        index: usize,
        x: Pixels,
        size: Size<Pixels>,
    },
}

impl Item {
    fn start(&self) -> usize {
        match self {
            Item::Text { start, .. } | Item::Chip { start, .. } => *start,
        }
    }

    fn x(&self) -> Pixels {
        match self {
            Item::Text { x, .. } | Item::Chip { x, .. } => *x,
        }
    }

    fn width(&self) -> Pixels {
        match self {
            Item::Text { line, .. } => line.width(),
            Item::Chip { size, .. } => size.width,
        }
    }
}

/// One visual row: a paragraph's text and tokens, or the part of them that
/// fits before it wraps.
pub(crate) struct Row {
    pub start: usize,
    /// Where it ends, before any line break.
    pub end: usize,
    pub y: Pixels,
    /// As tall as a line, or its tallest token.
    pub height: Pixels,
    /// Its content's width; spaces it wraps after hang past it.
    pub width: Pixels,
    /// Whether it ends because the paragraph wrapped, rather than at a line
    /// break or the end of the text.
    pub wraps: bool,
    pub items: Vec<Item>,
}

// A piece of a paragraph before it's broken into rows, with its x along the
// unwrapped paragraph.
enum Piece {
    Text {
        start: usize,
        end: usize,
        x: Pixels,
        line: ShapedLine,
    },
    Chip {
        start: usize,
        index: usize,
        x: Pixels,
        size: Size<Pixels>,
    },
}

// The x of a paragraph offset along its unwrapped line.
fn x_at(pieces: &[Piece], offset: usize) -> Pixels {
    let mut last = px(0.);
    for piece in pieces {
        match piece {
            Piece::Text { start, end, x, line } => {
                if offset <= *end && offset >= *start {
                    return *x + line.x_for_index(offset - start);
                }
                last = *x + line.width();
            }
            Piece::Chip { start, x, size, .. } => {
                if offset <= *start {
                    return *x;
                }
                if offset < start + TOKEN_LEN {
                    return *x;
                }
                last = *x + size.width;
            }
        }
    }
    last
}

// Where the next character after `at` ends: a grapheme, or a token.
fn next_unit(text: &str, pieces: &[Piece], at: usize) -> usize {
    for piece in pieces {
        match piece {
            Piece::Text { start, end, .. } if at >= *start && at < *end => {
                return text[at..*end]
                    .graphemes(true)
                    .next()
                    .map_or(*end, |grapheme| at + grapheme.len());
            }
            Piece::Chip { start, .. } if at == *start => return at + TOKEN_LEN,
            _ => {}
        }
    }
    at + text[at..].chars().next().map_or(1, char::len_utf8)
}

// Spaces a row wraps after hang past its edge, as `pre-wrap` keeps them.
fn hang(text: &str, start: usize, end: usize) -> usize {
    start + text[start..end].trim_end_matches([' ', '\t']).len()
}

// The furthest a row from `start` reaches toward `limit` within `wrap`, a
// character at a time: what `overflow-wrap: anywhere` breaks a run with no
// other break at. At least one character.
fn fit_units(text: &str, pieces: &[Piece], start: usize, limit: usize, wrap: Pixels) -> usize {
    let base = x_at(pieces, start);
    let mut fitted = None;
    let mut at = start;
    while at < limit {
        let next = next_unit(text, pieces, at).min(limit);
        if x_at(pieces, next) - base > wrap {
            break;
        }
        fitted = Some(next);
        at = next;
    }
    fitted.unwrap_or_else(|| next_unit(text, pieces, start).min(limit))
}

// Where a paragraph's rows after its first start: at the last break
// opportunity (UAX #14) that fits, and anywhere in a run that has none.
fn breaks(text: &str, start: usize, end: usize, pieces: &[Piece], wrap: Option<Pixels>) -> Vec<usize> {
    let Some(wrap) = wrap else {
        return Vec::new();
    };
    let mut breaks = Vec::new();
    let mut row_start = start;
    let mut fitting: Option<usize> = None;
    for (index, _) in unicode_linebreak::linebreaks(&text[start..end]) {
        let opportunity = start + index;
        if opportunity <= row_start {
            continue;
        }
        loop {
            let width = x_at(pieces, hang(text, row_start, opportunity)) - x_at(pieces, row_start);
            if width <= wrap {
                break;
            }
            if let Some(at) = fitting.take().filter(|at| *at > row_start) {
                breaks.push(at);
                row_start = at;
                continue;
            }
            let cut = fit_units(text, pieces, row_start, opportunity, wrap);
            if cut >= opportunity {
                break;
            }
            breaks.push(cut);
            row_start = cut;
        }
        fitting = Some(opportunity);
    }
    breaks
}

/// What shaping needs.
pub(crate) struct ShapeInput<'a> {
    pub shown: &'a Shown,
    /// Each token's host, by its place among the shown tokens.
    pub chips: &'a [Size<Pixels>],
    pub style: &'a TextStyle,
    pub decorations: Option<&'a Decorations>,
    pub wrap_width: Option<Pixels>,
    pub line_height: Pixels,
    /// The width rows align within.
    pub width: Pixels,
    pub align: TextAlign,
}

/// The shown text laid out: paragraphs between line breaks, wrapped into
/// rows, with each token's host in its row.
pub(crate) struct TokenLayout {
    pub rows: Vec<Row>,
    pub line_height: Pixels,
    pub width: Pixels,
    pub align: TextAlign,
    pub height: Pixels,
}

impl TokenLayout {
    pub fn shape(input: &ShapeInput, window: &mut Window) -> Self {
        let text = input.shown.text.as_ref();
        let font_size = input.style.font_size.to_pixels(window.rem_size());
        let mut rows = Vec::new();
        let mut y = px(0.);
        let mut token = 0;
        let mut paragraph = 0;
        loop {
            let end = text[paragraph..].find('\n').map_or(text.len(), |index| paragraph + index);
            let mut pieces = Vec::new();
            let mut at = paragraph;
            let mut x = px(0.);
            while at < end {
                let next = input
                    .shown
                    .tokens
                    .get(token)
                    .map(|(start, _)| *start)
                    .filter(|start| *start < end);
                match next {
                    Some(start) if start == at => {
                        let size = input.chips.get(token).copied().unwrap_or_default();
                        pieces.push(Piece::Chip {
                            start,
                            index: token,
                            x,
                            size,
                        });
                        x += size.width;
                        at += TOKEN_LEN;
                        token += 1;
                    }
                    _ => {
                        let stop = next.unwrap_or(end);
                        let runs = runs(input.style, at, stop, input.decorations);
                        let line = window.text_system().shape_line(
                            SharedString::new(&text[at..stop]),
                            font_size,
                            &runs,
                            None,
                        );
                        let width = line.width();
                        pieces.push(Piece::Text {
                            start: at,
                            end: stop,
                            x,
                            line,
                        });
                        x += width;
                        at = stop;
                    }
                }
            }
            let cuts = breaks(text, paragraph, end, &pieces, input.wrap_width);
            let mut starts = vec![paragraph];
            starts.extend(cuts.iter().copied());
            let mut row_items: Vec<Vec<Item>> = (0..starts.len()).map(|_| Vec::new()).collect();
            let row_of = |offset: usize| starts.partition_point(|start| *start <= offset).saturating_sub(1);
            for piece in &pieces {
                match piece {
                    Piece::Chip { start, index, size, .. } => {
                        let row = row_of(*start);
                        let item_x = x_at(&pieces, *start) - x_at(&pieces, starts[row]);
                        row_items[row].push(Item::Chip {
                            start: *start,
                            index: *index,
                            x: item_x,
                            size: *size,
                        });
                    }
                    Piece::Text { start, end, line, .. } => {
                        let mut cursor = line.cursor();
                        let mut from = *start;
                        let inside = cuts.iter().copied().filter(|cut| cut > start && cut < end);
                        for to in inside.chain(std::iter::once(*end)) {
                            let fragment = cursor.take_until(to - start);
                            let row = row_of(from);
                            let item_x = x_at(&pieces, from) - x_at(&pieces, starts[row]);
                            row_items[row].push(Item::Text {
                                start: from,
                                x: item_x,
                                line: fragment,
                            });
                            from = to;
                        }
                    }
                }
            }
            for (row, items) in row_items.into_iter().enumerate() {
                let row_start = starts[row];
                let wraps = row + 1 < starts.len();
                let row_end = starts.get(row + 1).copied().unwrap_or(end);
                let content_end = if wraps { hang(text, row_start, row_end) } else { row_end };
                let width = x_at(&pieces, content_end) - x_at(&pieces, row_start);
                let height = items
                    .iter()
                    .filter_map(|item| match item {
                        Item::Chip { size, .. } => Some(size.height),
                        Item::Text { .. } => None,
                    })
                    .fold(input.line_height, |tallest, height| tallest.max(height));
                rows.push(Row {
                    start: row_start,
                    end: row_end,
                    y,
                    height,
                    width,
                    wraps,
                    items,
                });
                y += height;
            }
            if end >= text.len() {
                break;
            }
            paragraph = end + 1;
        }
        Self {
            rows,
            line_height: input.line_height,
            width: input.width,
            align: input.align,
            height: y,
        }
    }

    /// The widest row.
    pub fn content_width(&self) -> Pixels {
        self.rows
            .iter()
            .map(|row| row.width)
            .fold(px(0.), |widest, width| widest.max(width))
    }

    // Where a row starts in the area, from the alignment.
    fn row_x(&self, row: &Row) -> Pixels {
        let space = (self.width - row.width).max(px(0.));
        match self.align {
            TextAlign::Left => px(0.),
            TextAlign::Center => space / 2.,
            TextAlign::Right => space,
        }
    }

    /// Where a row's line of text starts, from the area's top: centered in a
    /// row a token makes taller.
    pub fn text_top(&self, row: usize) -> Pixels {
        let row = &self.rows[row];
        row.y + (row.height - self.line_height) / 2.
    }

    /// The row an offset sits on. At a wrap, the caret goes to the start of
    /// the next row, unless `upstream` keeps it at the end of the first.
    pub fn row_for(&self, offset: usize, upstream: bool) -> usize {
        let last = self.rows.len() - 1;
        let index = self.rows.partition_point(|row| row.end < offset).min(last);
        let row = &self.rows[index];
        if !upstream && row.wraps && row.end == offset && index < last {
            index + 1
        } else {
            index
        }
    }

    /// The x of an offset on a row, from the area's left edge. Inside a
    /// token, its start.
    pub fn x_for(&self, row_index: usize, offset: usize) -> Pixels {
        let row = &self.rows[row_index];
        let mut last = px(0.);
        for item in &row.items {
            if offset < item.start() {
                return self.row_x(row) + item.x();
            }
            match item {
                Item::Text { start, x, line } => {
                    if offset <= start + line.len() {
                        return self.row_x(row) + *x + line.x_for_index(offset - start);
                    }
                }
                Item::Chip { start, x, size, .. } => {
                    if offset < start + TOKEN_LEN {
                        return self.row_x(row) + *x;
                    }
                    if offset == start + TOKEN_LEN {
                        return self.row_x(row) + *x + size.width;
                    }
                }
            }
            last = item.x() + item.width();
        }
        self.row_x(row) + last
    }

    /// Where the caret at an offset is drawn, from the area's top left.
    pub fn caret(&self, offset: usize, upstream: bool) -> Point<Pixels> {
        let row = self.row_for(offset, upstream);
        point(self.x_for(row, offset), self.text_top(row))
    }

    fn row_at_y(&self, y: Pixels) -> usize {
        let last = self.rows.len() - 1;
        if y < px(0.) {
            return 0;
        }
        self.rows
            .iter()
            .position(|row| y < row.y + row.height)
            .unwrap_or(last)
    }

    /// The offset nearest a point, from the area's top left, and whether it
    /// sits at the end of a wrapped row rather than the start of the next.
    pub fn offset_at(&self, position: Point<Pixels>) -> (usize, bool) {
        let index = self.row_at_y(position.y);
        let offset = self.offset_on_row(index, position.x);
        let row = &self.rows[index];
        (offset, row.wraps && offset == row.end)
    }

    /// The offset nearest an x on a row: a token's start or end, whichever
    /// is nearer.
    pub fn offset_on_row(&self, index: usize, x: Pixels) -> usize {
        let row = &self.rows[index];
        let local = x - self.row_x(row);
        for item in &row.items {
            match item {
                Item::Text { start, x, line } => {
                    if local < *x + line.width() {
                        return start + line.closest_index_for_x(local - *x);
                    }
                }
                Item::Chip { start, x, size, .. } => {
                    if local < *x + size.width {
                        return if local < *x + size.width / 2. {
                            *start
                        } else {
                            start + TOKEN_LEN
                        };
                    }
                }
            }
        }
        row.end
    }

    /// The offset of the character under a point, what a double click picks
    /// a word from. Past the end of a row, its last character.
    pub fn char_at(&self, position: Point<Pixels>) -> usize {
        let row = &self.rows[self.row_at_y(position.y)];
        let local = position.x - self.row_x(row);
        let mut last = row.start;
        for item in &row.items {
            match item {
                Item::Text { start, x, line } => {
                    if local < *x + line.width() {
                        return start + line.index_for_x((local - *x).max(px(0.))).unwrap_or(0);
                    }
                    last = start
                        + line
                            .text
                            .grapheme_indices(true)
                            .next_back()
                            .map_or(0, |(index, _)| index);
                }
                Item::Chip { start, x, size, .. } => {
                    if local < *x + size.width {
                        return *start;
                    }
                    last = *start;
                }
            }
        }
        last
    }

    /// The rectangles a range covers, row by row, from the area's top left.
    /// A selected line break shows as a sliver past its row.
    pub fn range_rects(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let mut rects = Vec::new();
        let last = self.rows.len() - 1;
        for (index, row) in self.rows.iter().enumerate() {
            if range.end < row.start || range.start > row.end {
                continue;
            }
            let start = range.start.max(row.start);
            let end = range.end.min(row.end);
            let line_break = !row.wraps && index < last && range.end > row.end;
            if start >= end && !line_break {
                continue;
            }
            // A range that only touches a wrap belongs to the row after it.
            if row.wraps && start == row.end && !line_break {
                continue;
            }
            let x0 = self.x_for(index, start);
            let mut x1 = self.x_for(index, end);
            if line_break {
                x1 += self.line_height * 0.25;
            }
            rects.push(Bounds::new(point(x0, row.y), size(x1 - x0, row.height)));
        }
        rects
    }

    /// The offsets where a row starts and ends.
    pub fn row_range(&self, index: usize) -> Range<usize> {
        let row = &self.rows[index];
        row.start..row.end
    }

    /// Where each token's host sits, from the area's top left, by its place
    /// among the shown tokens.
    pub fn chip_bounds(&self) -> Vec<(usize, usize, Bounds<Pixels>)> {
        let mut chips = Vec::new();
        for row in &self.rows {
            for item in &row.items {
                if let Item::Chip { start, index, x, size } = item {
                    let origin = point(self.row_x(row) + *x, row.y + (row.height - size.height) / 2.);
                    chips.push((*index, *start, Bounds::new(origin, *size)));
                }
            }
        }
        chips
    }

    /// Paints the text with the area's top left at `origin`.
    pub fn paint(&self, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
        for (index, row) in self.rows.iter().enumerate() {
            let top = origin.y + self.text_top(index);
            for item in &row.items {
                if let Item::Text { x, line, .. } = item {
                    let left = origin.x + self.row_x(row) + *x;
                    line.paint(point(left, top), self.line_height, TextAlign::Left, None, window, cx)
                        .ok();
                }
            }
        }
    }
}

/// Where the last frame laid the editor out, which clicks, vertical moves,
/// the platform's text input and a trigger menu read.
#[derive(Clone)]
pub(crate) struct Geometry {
    pub layout: Rc<TokenLayout>,
    pub shown: Rc<Shown>,
    /// The text area, in window coordinates.
    pub bounds: Bounds<Pixels>,
    pub scroll: Point<Pixels>,
    // Each token's host, its shown start, in window coordinates.
    chips: Rc<Vec<(usize, Bounds<Pixels>)>>,
}

impl Geometry {
    /// A window position, from the text area's top left with scrolling.
    pub fn local(&self, position: Point<Pixels>) -> Point<Pixels> {
        position - self.bounds.origin + self.scroll
    }

    /// A position from the text area's top left with scrolling, in the window.
    pub fn window(&self, local: Point<Pixels>) -> Point<Pixels> {
        local + self.bounds.origin - self.scroll
    }

    /// Where the caret at a shown offset is drawn, with no width.
    pub fn caret_bounds(&self, offset: usize) -> Bounds<Pixels> {
        let caret = self.layout.caret(offset.min(self.shown.text.len()), false);
        Bounds::new(self.window(caret), size(px(0.), self.layout.line_height))
    }

    /// The shown offset nearest a window position.
    pub fn hit_shown(&self, position: Point<Pixels>) -> Option<(usize, bool)> {
        Some(self.layout.offset_at(self.local(position)))
    }

    /// The value offset nearest a window position.
    pub fn hit(&self, position: Point<Pixels>) -> Option<(usize, bool)> {
        let (offset, upstream) = self.hit_shown(position)?;
        Some((self.shown.to_value(offset), upstream))
    }

    /// The value offset of the character under a window position.
    pub fn char_under(&self, position: Point<Pixels>) -> usize {
        self.shown.to_value(self.layout.char_at(self.local(position)))
    }

    /// Where the token under a window position starts in the value.
    pub fn token_under(&self, position: Point<Pixels>) -> Option<usize> {
        self.chips
            .iter()
            .find(|(_, bounds)| bounds.contains(&position))
            .map(|(start, _)| self.shown.to_value(*start))
    }
}

// A run's characters are indexed in bytes, so a long row splits into runs.
const RUN_CHARS: usize = 200;

/// One text run as the platform reads it.
struct A11yRun {
    row: usize,
    text: String,
    lengths: Vec<u8>,
    // The value offset each character starts at, and where the last ends.
    starts: Vec<usize>,
    positions: Vec<f32>,
    widths: Vec<f32>,
    word_starts: Vec<u8>,
    bounds: Rect,
}

/// What the editor's runs and selection report, written each frame the tree
/// is built, and read back by the actions. A token is one character, which
/// reads as its label and description.
#[derive(Default)]
pub(crate) struct A11ySnapshot {
    runs: Vec<A11yRun>,
    anchor: Option<(usize, usize)>,
    focus: Option<(usize, usize)>,
    ids: Vec<NodeId>,
}

/// What a snapshot reads besides the layout.
pub(crate) struct SnapshotInput<'a> {
    pub layout: &'a TokenLayout,
    pub shown: &'a Shown,
    /// What each shown token reads as.
    pub reads: &'a [String],
    pub origin: Point<Pixels>,
    pub scale: f32,
    pub anchor: usize,
    pub head: usize,
    pub upstream: bool,
}

// A character a run reads: its text, its shown offset, where it starts and
// how wide it is.
struct Character {
    text: String,
    start: usize,
    end: usize,
    x: Pixels,
    width: Pixels,
}

fn scaled(bounds: Bounds<Pixels>, scale: f32) -> Rect {
    let device = |pixels: Pixels| (pixels * scale).to_f64();
    Rect {
        x0: device(bounds.origin.x),
        y0: device(bounds.origin.y),
        x1: device(bounds.origin.x + bounds.size.width),
        y1: device(bounds.origin.y + bounds.size.height),
    }
}

// A token's text, cut to what a character's length can hold.
fn fitted(text: &str) -> String {
    let mut end = text.len().min(u8::MAX as usize);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

impl A11ySnapshot {
    /// Records the text as it was laid out this frame.
    pub fn update(&mut self, input: SnapshotInput) {
        let SnapshotInput {
            layout,
            shown,
            reads,
            origin,
            scale,
            anchor,
            head,
            upstream,
        } = input;
        let text = shown.text.as_ref();
        let last_row = layout.rows.len() - 1;
        self.runs.clear();
        for (index, row) in layout.rows.iter().enumerate() {
            let mut characters = Vec::new();
            for item in &row.items {
                match item {
                    Item::Text { start, line, .. } => {
                        let piece = &text[*start..start + line.len()];
                        for (at, grapheme) in piece.grapheme_indices(true) {
                            let from = start + at;
                            let to = from + grapheme.len();
                            let x = layout.x_for(index, from);
                            characters.push(Character {
                                text: grapheme.to_string(),
                                start: from,
                                end: to,
                                x,
                                width: layout.x_for(index, to) - x,
                            });
                        }
                    }
                    Item::Chip { start, index: token, size, .. } => {
                        let x = layout.x_for(index, *start);
                        characters.push(Character {
                            text: fitted(reads.get(*token).map_or("", String::as_str)),
                            start: *start,
                            end: start + TOKEN_LEN,
                            x,
                            width: size.width,
                        });
                    }
                }
            }
            if !row.wraps && index < last_row {
                characters.push(Character {
                    text: "\n".to_string(),
                    start: row.end,
                    end: row.end + 1,
                    x: layout.x_for(index, row.end),
                    width: px(0.),
                });
            }
            let words: Vec<usize> = text[row.start..row.end]
                .split_word_bound_indices()
                .map(|(at, _)| row.start + at)
                .collect();
            let top = origin.y + layout.text_top(index);
            let mut chunks = characters.chunks(RUN_CHARS).peekable();
            if chunks.peek().is_none() {
                let x = origin.x + layout.x_for(index, row.start);
                self.runs.push(A11yRun {
                    row: index,
                    text: String::new(),
                    lengths: Vec::new(),
                    starts: vec![shown.to_value(row.start)],
                    positions: Vec::new(),
                    widths: Vec::new(),
                    word_starts: Vec::new(),
                    bounds: scaled(Bounds::new(point(x, top), size(px(0.), layout.line_height)), scale),
                });
                continue;
            }
            for chunk in chunks {
                let first_x = chunk.first().map_or(px(0.), |character| character.x);
                let mut run = A11yRun {
                    row: index,
                    text: String::new(),
                    lengths: Vec::with_capacity(chunk.len()),
                    starts: Vec::with_capacity(chunk.len() + 1),
                    positions: Vec::with_capacity(chunk.len()),
                    widths: Vec::with_capacity(chunk.len()),
                    word_starts: Vec::new(),
                    bounds: Rect::default(),
                };
                let mut end_x = first_x;
                let mut end = shown.to_value(row.start);
                for (number, character) in chunk.iter().enumerate() {
                    run.text.push_str(&character.text);
                    run.lengths.push(character.text.len() as u8);
                    run.starts.push(shown.to_value(character.start));
                    run.positions.push(((character.x - first_x) * scale).as_f32());
                    run.widths.push((character.width * scale).as_f32());
                    if words.contains(&character.start) {
                        run.word_starts.push(number as u8);
                    }
                    end = shown.to_value(character.end);
                    end_x = character.x + character.width;
                }
                run.starts.push(end);
                run.bounds = scaled(
                    Bounds::from_corners(
                        point(origin.x + first_x, top),
                        point(origin.x + end_x, top + layout.line_height),
                    ),
                    scale,
                );
                self.runs.push(run);
            }
        }
        self.anchor = self.position(layout, shown, anchor, anchor == head && upstream);
        self.focus = self.position(layout, shown, head, upstream);
    }

    // The run and character a value offset is at.
    fn position(&self, layout: &TokenLayout, shown: &Shown, offset: usize, upstream: bool) -> Option<(usize, usize)> {
        let row = layout.row_for(shown.to_shown(offset), upstream);
        let mut found = None;
        for (index, run) in self.runs.iter().enumerate() {
            if run.row != row {
                continue;
            }
            let chars = run.lengths.len();
            let ends_line = run.text.ends_with('\n');
            let last = if ends_line { chars.saturating_sub(1) } else { chars };
            let character = run.starts[..=last].partition_point(|start| *start < offset);
            found = Some((index, character.min(last)));
            if offset < run.starts[last] {
                break;
            }
        }
        found
    }

    /// The value offset of a position the platform names.
    pub fn offset_of(&self, position: &TextPosition) -> Option<usize> {
        let index = self.ids.iter().position(|id| *id == position.node)?;
        let run = &self.runs[index];
        Some(run.starts[position.character_index.min(run.starts.len() - 1)])
    }
}

/// Adds the runs under the editor's node and its selection, and makes its
/// textbox a multi-line text input, `aria-multiline`.
pub(crate) fn build(snapshot: &Rc<RefCell<A11ySnapshot>>, builder: &mut A11ySubtreeBuilder) {
    let mut snapshot = snapshot.borrow_mut();
    let ids: Vec<NodeId> = (0..snapshot.runs.len())
        .map(|index| builder.synthetic_node_id(("text-run", index)))
        .collect();
    for (index, run) in snapshot.runs.iter().enumerate() {
        let mut node = accesskit::Node::new(Role::TextRun);
        node.set_value(run.text.clone());
        node.set_character_lengths(run.lengths.clone());
        node.set_character_positions(run.positions.clone());
        node.set_character_widths(run.widths.clone());
        node.set_word_starts(run.word_starts.clone());
        node.set_text_direction(TextDirection::LeftToRight);
        node.set_bounds(run.bounds);
        if index > 0 && snapshot.runs[index - 1].row == run.row {
            node.set_previous_on_line(ids[index - 1]);
        }
        if snapshot.runs.get(index + 1).is_some_and(|next| next.row == run.row) {
            node.set_next_on_line(ids[index + 1]);
        }
        builder.push_child(ids[index], node);
    }
    let parent = builder.parent_node();
    if parent.role() == Role::TextInput {
        parent.set_role(Role::MultilineTextInput);
    }
    if let (Some(anchor), Some(focus)) = (snapshot.anchor, snapshot.focus)
        && anchor.0 < ids.len()
        && focus.0 < ids.len()
    {
        parent.set_text_selection(TextSelection {
            anchor: TextPosition {
                node: ids[anchor.0],
                character_index: anchor.1,
            },
            focus: TextPosition {
                node: ids[focus.0],
                character_index: focus.1,
            },
        });
    }
    snapshot.ids = ids;
}

/// The element inside the editor part that lays the value out and paints
/// it: the text or the placeholder, each token's host, the selection, the
/// caret and the marked text, scrolled to keep the caret in view. It's as
/// wide as the part lets it be and as tall as its text, like an editable
/// block; past the part's `max_h`, it scrolls.
pub(crate) struct TokenEditorElement {
    pub input: Entity<TokenInput>,
    pub placeholder: Option<SharedString>,
    pub render_token: Option<RenderToken>,
    pub style_host: Option<HostStyle>,
}

/// Each token's host, laid out as it renders.
pub(crate) struct Hosts {
    elements: Vec<AnyElement>,
    sizes: Vec<Size<Pixels>>,
}

/// What a frame paints.
pub(crate) struct Frame {
    layout: Rc<TokenLayout>,
    placeholder: Option<TokenLayout>,
    scroll: Point<Pixels>,
    selection: Vec<Bounds<Pixels>>,
    caret: Option<Point<Pixels>>,
    colors: Colors,
    dragging: bool,
    blur: bool,
}

impl TokenEditorElement {
    // A token's host: what the app renders for it, or its label, hidden from
    // assistive technology, which reads the token in the editor's text.
    fn host(&self, token: &Token, state: &TokenInputTokenState, window: &mut Window, cx: &mut App) -> AnyElement {
        let id = token.id.clone().unwrap_or_default();
        let content = match &self.render_token {
            Some(render) => render(&self.input, token, window, cx),
            None => token.label.clone().into_any_element(),
        };
        let host = InteractiveElement::id(
            div(),
            ElementId::Name(format!("ui-sdk-token-host-{}", ui_sdk_kernel::id_part(&id)).into()),
        )
        .flex_none();
        let host = match &self.style_host {
            Some(style) => style(host, state),
            None => host,
        };
        with_aria(with_role(host.child(content), Role::Group), Aria::new().hidden(true)).into_any_element()
    }
}

impl Element for TokenEditorElement {
    type RequestLayoutState = Hosts;
    type PrepaintState = Option<Frame>;

    fn id(&self) -> Option<ElementId> {
        Some("ui-sdk-token-editor".into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (shown, states, empty) = {
            let input = self.input.read(cx);
            let shown = Shown::of(&input.committed, input.composition.as_ref());
            let states: Vec<TokenInputTokenState> =
                shown.tokens.iter().map(|(_, token)| input.token_state(token)).collect();
            let empty = input.committed.is_empty() && input.composition.is_none();
            (shown, states, empty)
        };
        let mut elements = Vec::with_capacity(shown.tokens.len());
        let mut sizes = Vec::with_capacity(shown.tokens.len());
        for ((_, token), state) in shown.tokens.iter().zip(&states) {
            let mut element = self.host(token, state, window, cx);
            let measured = element.layout_as_root(
                size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                window,
                cx,
            );
            elements.push(element);
            sizes.push(measured);
        }
        let text_style = window.text_style();
        let line_height = window.line_height();
        // An editor sized to its text sizes to its placeholder while it's empty.
        let placeholder = self.placeholder.clone().filter(|text| empty && !text.is_empty());
        let (measured, chips) = match &placeholder {
            Some(text) => (Shown::plain(text), Vec::new()),
            None => (shown, sizes.clone()),
        };
        let mut style = Style::default();
        style.flex_grow = 1.;
        style.flex_shrink = 1.;
        style.min_size.width = px(0.).into();
        style.min_size.height = px(0.).into();
        let layout_id = window.request_measured_layout(
            style,
            move |known: Size<Option<Pixels>>, available: Size<AvailableSpace>, window, _cx| {
                let wrap = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let layout = TokenLayout::shape(
                    &ShapeInput {
                        shown: &measured,
                        chips: &chips,
                        style: &text_style,
                        decorations: None,
                        wrap_width: wrap,
                        line_height,
                        width: wrap.unwrap_or_default(),
                        align: TextAlign::Left,
                    },
                    window,
                );
                let width = known.width.unwrap_or_else(|| wrap.unwrap_or_else(|| layout.content_width()));
                let height = known.height.unwrap_or(layout.height);
                size(width, height)
            },
        );
        (layout_id, Hosts { elements, sizes })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        hosts: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        let line_height = window.line_height();
        let input = self.input.read(cx);
        let focused = input.focus_handle.is_focused(window);
        let colors = resolve(&input.settings.style, text_style.color);
        let align = align_of(&input.settings.style, text_style.text_align, ui_sdk_platform::is_rtl(cx));
        let shown = Shown::of(&input.committed, input.composition.as_ref());
        let selection = match &input.composition {
            Some(composition) => {
                let start = shown.marked.as_ref().map_or(0, |marked| marked.start);
                (start + composition.selected.start)..(start + composition.selected.end)
            }
            None => input.start..input.end,
        };
        let selected = (focused && !selection.is_empty() && colors.selected_text != colors.text)
            .then(|| selection.clone());
        let decorations = Decorations {
            text: colors.text,
            selected_text: colors.selected_text,
            underline: colors.composition_underline,
            selected,
            marked: shown.marked.clone(),
        };
        let layout = TokenLayout::shape(
            &ShapeInput {
                shown: &shown,
                chips: &hosts.sizes,
                style: &text_style,
                decorations: Some(&decorations),
                wrap_width: Some(bounds.size.width),
                line_height,
                width: bounds.size.width,
                align,
            },
            window,
        );
        let empty = input.committed.is_empty() && input.composition.is_none();
        let placeholder = self
            .placeholder
            .clone()
            .filter(|text| empty && !text.is_empty())
            .map(|text| {
                let plain = Shown::plain(&text);
                let decorations = Decorations {
                    text: colors.placeholder,
                    selected_text: colors.placeholder,
                    underline: colors.composition_underline,
                    selected: None,
                    marked: None,
                };
                TokenLayout::shape(
                    &ShapeInput {
                        shown: &plain,
                        chips: &[],
                        style: &text_style,
                        decorations: Some(&decorations),
                        wrap_width: Some(bounds.size.width),
                        line_height,
                        width: bounds.size.width,
                        align,
                    },
                    window,
                )
            });
        let head = match &input.composition {
            Some(_) => selection.end,
            None => input.head(),
        };
        let upstream = input.upstream;
        let caret = layout.caret(head, upstream);
        let mut scroll = input.scroll;
        // Only a focused editor follows its caret.
        if input.autoscroll && focused {
            if caret.y < scroll.y {
                scroll.y = caret.y;
            } else if caret.y + line_height > scroll.y + bounds.size.height {
                scroll.y = caret.y + line_height - bounds.size.height;
            }
        }
        let max = (layout.height - bounds.size.height).max(px(0.));
        scroll.y = scroll.y.clamp(px(0.), max);
        scroll.x = px(0.);
        let selection_rects = if focused && !selection.is_empty() {
            layout.range_rects(selection.clone())
        } else {
            Vec::new()
        };
        // A read-only editor keeps its caret, as an editable element whose
        // edits are refused does.
        let caret_shown = focused && input.caret_visible && !input.is_disabled() && selection.is_empty();
        let caret = caret_shown.then(|| {
            // Kept inside the area, where alignment would put it on the edge.
            let x = caret.x.clamp(px(0.), (bounds.size.width - colors.caret_width).max(px(0.)));
            point(x, caret.y)
        });
        let dragging = input.is_dragging();
        let blur = focused && input.is_disabled();
        let anchor = input.anchor();
        let value_head = input.head();
        let reads: Vec<String> = shown
            .tokens
            .iter()
            .map(|(_, token)| format!("{} {}", token.label, input.describe(token)))
            .collect();
        let snapshot = input.a11y.clone();

        let origin = bounds.origin - scroll;
        let chips = layout.chip_bounds();
        let mut placed = vec![false; hosts.elements.len()];
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (index, _, chip) in &chips {
                if let Some(element) = hosts.elements.get_mut(*index) {
                    element.prepaint_at(origin + chip.origin, window, cx);
                    placed[*index] = true;
                }
            }
            for (index, element) in hosts.elements.iter_mut().enumerate() {
                if !placed[index] {
                    element.prepaint_at(origin, window, cx);
                }
            }
        });
        let layout = Rc::new(layout);
        if window.is_a11y_active() {
            snapshot.borrow_mut().update(SnapshotInput {
                layout: &layout,
                shown: &shown,
                reads: &reads,
                origin,
                scale: window.scale_factor(),
                anchor,
                head: value_head,
                upstream,
            });
        }
        let geometry = Geometry {
            layout: layout.clone(),
            shown: Rc::new(shown),
            bounds,
            scroll,
            chips: Rc::new(
                chips
                    .iter()
                    .map(|(_, start, chip)| (*start, Bounds::new(origin + chip.origin, chip.size)))
                    .collect(),
            ),
        };
        self.input.update(cx, |input, _| {
            input.geometry = Some(geometry);
            input.scroll = scroll;
            input.autoscroll = false;
            input.sync_mirror();
        });
        Some(Frame {
            layout,
            placeholder,
            scroll,
            selection: selection_rects,
            caret,
            colors,
            dragging,
            blur,
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        hosts: &mut Self::RequestLayoutState,
        frame: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(frame) = frame.take() else {
            return;
        };
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        let origin = bounds.origin - frame.scroll;
        let line_height = frame.layout.line_height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for rect in &frame.selection {
                window.paint_quad(fill(Bounds::new(rect.origin + origin, rect.size), frame.colors.selection));
            }
            match &frame.placeholder {
                Some(placeholder) => placeholder.paint(origin, window, cx),
                None => frame.layout.paint(origin, window, cx),
            }
            for element in &mut hosts.elements {
                element.paint(window, cx);
            }
            if let Some(caret) = frame.caret {
                window.paint_quad(fill(
                    Bounds::new(caret + origin, size(frame.colors.caret_width, line_height)),
                    frame.colors.caret,
                ));
            }
        });
        // A drag goes on past the editor's edges, and ends wherever the
        // button rises.
        if frame.dragging {
            let input = self.input.downgrade();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                input
                    .update(cx, |input, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            input.drag_to(event.position, window, cx);
                        } else {
                            input.end_drag();
                        }
                    })
                    .ok();
            });
        }
        // An editor disabled while focused lets focus go, as the browser does.
        if frame.blur {
            window.defer(cx, move |window, cx| {
                if focus_handle.is_focused(window) {
                    window.blur(cx);
                }
            });
        }
    }
}
