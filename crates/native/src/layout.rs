//! A field's text laid out with gpui's text system: paragraphs between line
//! breaks, wrapped into visual rows, and where each offset sits. Offsets
//! here are into the shown text; `Display` maps them to the value.

use std::ops::Range;

use gpui::{
    App, Bounds, Pixels, Point, SharedString, TextAlign, TextRun, Window, WrappedLine, point, px,
    size,
};

use crate::boundaries;

/// One paragraph: the text between two line breaks, shaped and wrapped.
pub(crate) struct Paragraph {
    pub line: WrappedLine,
    /// Where it starts in the shown text.
    pub start: usize,
}

/// One visual row of a paragraph.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub paragraph: usize,
    pub start: usize,
    /// Where it ends, before any line break.
    pub end: usize,
    /// Where it starts in its paragraph's unwrapped line.
    pub start_x: Pixels,
    pub width: Pixels,
    /// Whether it ends because the paragraph wrapped, rather than at a line
    /// break or the end of the text.
    pub wraps: bool,
}

/// Shown text, laid out in an area of some width.
pub(crate) struct TextLayout {
    pub paragraphs: Vec<Paragraph>,
    pub rows: Vec<Row>,
    pub line_height: Pixels,
    /// The width rows align within.
    pub width: Pixels,
    pub align: TextAlign,
    /// A single-line field's row aligns only while it fits, and scrolls past.
    pub single_line: bool,
}

impl TextLayout {
    /// Shapes `text`, wrapping it at `wrap_width` when there is one.
    #[allow(clippy::too_many_arguments)]
    pub fn shape(
        text: &SharedString,
        runs: &[TextRun],
        font_size: Pixels,
        wrap_width: Option<Pixels>,
        line_height: Pixels,
        width: Pixels,
        align: TextAlign,
        single_line: bool,
        window: &mut Window,
    ) -> Self {
        let lines = window
            .text_system()
            .shape_text(text.clone(), font_size, runs, wrap_width, None)
            .unwrap_or_default();
        let mut paragraphs = Vec::with_capacity(lines.len());
        let mut rows = Vec::new();
        let mut start = 0;
        for line in lines {
            let len = line.len();
            let unwrapped = line.unwrapped_layout.clone();
            let mut row_start = 0;
            let mut row_start_x = px(0.);
            for boundary in line.wrap_boundaries() {
                let glyph = &unwrapped.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
                rows.push(Row {
                    paragraph: paragraphs.len(),
                    start: start + row_start,
                    end: start + glyph.index,
                    start_x: row_start_x,
                    width: glyph.position.x - row_start_x,
                    wraps: true,
                });
                row_start = glyph.index;
                row_start_x = glyph.position.x;
            }
            rows.push(Row {
                paragraph: paragraphs.len(),
                start: start + row_start,
                end: start + len,
                start_x: row_start_x,
                width: unwrapped.width - row_start_x,
                wraps: false,
            });
            paragraphs.push(Paragraph { line, start });
            start += len + 1;
        }
        if rows.is_empty() {
            rows.push(Row {
                paragraph: 0,
                start: 0,
                end: 0,
                start_x: px(0.),
                width: px(0.),
                wraps: false,
            });
        }
        Self {
            paragraphs,
            rows,
            line_height,
            width,
            align,
            single_line,
        }
    }

    /// The widest row.
    pub fn content_width(&self) -> Pixels {
        self.rows
            .iter()
            .map(|row| row.width)
            .fold(px(0.), |widest, width| widest.max(width))
    }

    pub fn content_height(&self) -> Pixels {
        self.line_height * self.rows.len() as f32
    }

    // Where a row starts in the area, from the alignment.
    fn row_x(&self, row: &Row) -> Pixels {
        let space = self.width - row.width;
        if self.single_line && space < px(0.) {
            return px(0.);
        }
        match self.align {
            TextAlign::Left => px(0.),
            TextAlign::Center => space / 2.,
            TextAlign::Right => space,
        }
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

    /// The x of an offset on a row, from the area's left edge.
    pub fn x_for(&self, row_index: usize, offset: usize) -> Pixels {
        let row = &self.rows[row_index];
        let Some(paragraph) = self.paragraphs.get(row.paragraph) else {
            return self.row_x(row);
        };
        let local = offset.clamp(row.start, row.end) - paragraph.start;
        self.row_x(row) + paragraph.line.unwrapped_layout.x_for_index(local) - row.start_x
    }

    /// Where the caret at an offset is drawn, from the area's top left.
    pub fn caret(&self, offset: usize, upstream: bool) -> Point<Pixels> {
        let row = self.row_for(offset, upstream);
        point(self.x_for(row, offset), self.line_height * row as f32)
    }

    fn row_at_y(&self, y: Pixels) -> usize {
        let row = (y / self.line_height).floor();
        if row < 0. {
            0
        } else {
            (row as usize).min(self.rows.len() - 1)
        }
    }

    /// The offset nearest a point, from the area's top left, and whether it
    /// sits at the end of a wrapped row rather than the start of the next.
    pub fn offset_at(&self, position: Point<Pixels>) -> (usize, bool) {
        let index = self.row_at_y(position.y);
        let offset = self.offset_on_row(index, position.x);
        let row = &self.rows[index];
        (offset, row.wraps && offset == row.end)
    }

    /// The offset nearest an x on a row.
    pub fn offset_on_row(&self, index: usize, x: Pixels) -> usize {
        let row = &self.rows[index];
        let Some(paragraph) = self.paragraphs.get(row.paragraph) else {
            return row.start;
        };
        let local = x - self.row_x(row) + row.start_x;
        let offset = paragraph.start + paragraph.line.unwrapped_layout.closest_index_for_x(local);
        offset.clamp(row.start, row.end)
    }

    /// The offset of the character under a point, what a double click picks
    /// a word from. Past the end of a row, its last character.
    pub fn char_at(&self, position: Point<Pixels>) -> usize {
        let index = self.row_at_y(position.y);
        let row = &self.rows[index];
        let Some(paragraph) = self.paragraphs.get(row.paragraph) else {
            return row.start;
        };
        let unwrapped = &paragraph.line.unwrapped_layout;
        let local = position.x - self.row_x(row) + row.start_x;
        let end_x = row.start_x + row.width;
        if local >= end_x {
            let end = row.end - paragraph.start;
            let last = boundaries::previous_grapheme(&paragraph.line.text, end) + paragraph.start;
            return last.max(row.start);
        }
        let offset = paragraph.start + unwrapped.index_for_x(local.max(row.start_x)).unwrap_or(0);
        offset.clamp(row.start, row.end)
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
            let y = self.line_height * index as f32;
            rects.push(Bounds::new(point(x0, y), size(x1 - x0, self.line_height)));
        }
        rects
    }

    /// The offsets where a row starts and ends.
    pub fn row_range(&self, index: usize) -> Range<usize> {
        let row = &self.rows[index];
        row.start..row.end
    }

    /// Paints the text with its top left at `origin`.
    pub fn paint(&self, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let mut row_index = 0;
        for paragraph in &self.paragraphs {
            let first = row_index;
            row_index += paragraph.line.wrap_boundaries().len() + 1;
            if paragraph.line.len() == 0 {
                continue;
            }
            let y = origin.y + self.line_height * first as f32;
            if self.single_line {
                let x = origin.x + self.row_x(&self.rows[first]);
                paragraph
                    .line
                    .paint(point(x, y), self.line_height, TextAlign::Left, None, window, cx)
                    .ok();
            } else {
                let area = Bounds::new(point(origin.x, y), size(self.width, self.line_height));
                paragraph
                    .line
                    .paint(point(origin.x, y), self.line_height, self.align, Some(area), window, cx)
                    .ok();
            }
        }
    }
}
