//! Port of `components/pagination/pagination-range.ts`: which page numbers a
//! pagination shows, and where the gaps go. Pure, so it runs anywhere, like
//! in an app's own markup.
//!
//! Pages and counts are whole and never negative here, so the original's
//! clamping of fractions and negative counts is the type's.

/// One entry of a pagination's range: a page number, or a gap before or
/// after the current page's neighbours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaginationRangeItem {
    Page(usize),
    /// `"ellipsis-start"`.
    EllipsisStart,
    /// `"ellipsis-end"`.
    EllipsisEnd,
}

impl PaginationRangeItem {
    /// The page, for an entry that's one.
    pub fn page(self) -> Option<usize> {
        match self {
            PaginationRangeItem::Page(page) => Some(page),
            _ => None,
        }
    }

    pub fn is_ellipsis(self) -> bool {
        self.page().is_none()
    }

    /// The entry as the original writes it, a page number or
    /// `"ellipsis-start"` and `"ellipsis-end"`: a key that keeps a page's
    /// part as the range moves.
    pub fn key(self) -> String {
        match self {
            PaginationRangeItem::Page(page) => page.to_string(),
            PaginationRangeItem::EllipsisStart => "ellipsis-start".into(),
            PaginationRangeItem::EllipsisEnd => "ellipsis-end".into(),
        }
    }
}

/// What [`pagination_range`] takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaginationRangeOptions {
    /// The current page, from 1. Clamped to the range.
    pub page: usize,
    /// How many pages there are.
    pub page_count: usize,
    /// How many pages show on each side of the current one. 1 by default.
    pub siblings: Option<usize>,
    /// How many pages always show at each end. 1 by default.
    pub boundaries: Option<usize>,
}

fn span(start: usize, end: usize) -> impl Iterator<Item = PaginationRangeItem> {
    (start..=end).map(PaginationRangeItem::Page)
}

/// The pages to show, with an ellipsis for each gap. The list has the same
/// length on every page of a given count, so links never shift under the
/// pointer while paging, and a gap of one page shows that page instead.
///
/// `pagination_range(page 5, page_count 10)` is
/// `[1, EllipsisStart, 4, 5, 6, EllipsisEnd, 10]`.
pub fn pagination_range(options: PaginationRangeOptions) -> Vec<PaginationRangeItem> {
    let total = options.page_count;
    let around = options.siblings.unwrap_or(1);
    let ends = options.boundaries.unwrap_or(1);
    if total == 0 {
        return Vec::new();
    }
    let current = clamp_page(options.page, total);
    // Both ends, the current page with its siblings, and two gaps.
    let slots = 2 * ends + 2 * around + 3;
    if total <= slots {
        return span(1, total).collect();
    }
    // The window around the current page slides to stay clear of the ends,
    // so the length never changes.
    let start = current
        .saturating_sub(around)
        .max(ends + 2)
        .min(total - ends - 1 - 2 * around);
    let end = start + 2 * around;
    let mut items: Vec<PaginationRangeItem> = span(1, ends).collect();
    items.push(if start > ends + 2 {
        PaginationRangeItem::EllipsisStart
    } else {
        PaginationRangeItem::Page(ends + 1)
    });
    items.extend(span(start, end));
    items.push(if end < total - ends - 1 {
        PaginationRangeItem::EllipsisEnd
    } else {
        PaginationRangeItem::Page(total - ends)
    });
    items.extend(span(total - ends + 1, total));
    items
}

/// A page number brought within 1 and `page_count`.
pub fn clamp_page(page: usize, page_count: usize) -> usize {
    page.clamp(1, page_count.max(1))
}

/// A page number given as any number, like an agent's, brought within 1
/// and `page_count`: rounded, and page 1 when it isn't a number.
pub fn clamp_page_number(page: f64, page_count: usize) -> usize {
    if !page.is_finite() {
        return 1;
    }
    // `Math.round`, which rounds halves up.
    let rounded = (page + 0.5).floor();
    if rounded < 1.0 {
        return 1;
    }
    clamp_page(rounded.min(usize::MAX as f64) as usize, page_count)
}

/// How many pages `total` items fill, `page_size` to a page. Never less
/// than 1.
pub fn page_count_of(total: usize, page_size: usize) -> usize {
    if page_size == 0 {
        return 1;
    }
    total.div_ceil(page_size).max(1)
}
