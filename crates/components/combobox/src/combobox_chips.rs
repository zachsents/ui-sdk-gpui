//! Port of `components/combobox/combobox-chips.ts`: chips that collapse to
//! "+N more" when they don't fit on the chips' first line, once a
//! chips-overflow part is rendered.
//!
//! The original hears widths from a `ResizeObserver`, which reports them
//! after layout. Here each part records its bounds as it lays out, and the
//! chips-overflow part decides once the frame that drew it is over, from
//! those bounds. Hidden chips keep the width they last had. Decisions are a
//! pure function of those widths. What the original reads with
//! `getComputedStyle`, the chips' gap and padding and the input's minimum
//! width, is what the app styled those parts with.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{AbsoluteLength, DefiniteLength, Edges, Length, Pixels, SharedString, StyleRefinement, Window, px};
use ui_sdk_kernel::BoundPart;

/// What [`chips_that_fit`] decides from.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipsLayout<'a> {
    /// The chips' content width.
    pub available: Pixels,
    /// The gap between chips.
    pub gap: Pixels,
    /// Each chip's width, or `None` when it has never been measured.
    pub widths: &'a [Option<Pixels>],
    /// The overflow part's width.
    pub overflow: Pixels,
    /// The input's minimum width.
    pub reserve: Pixels,
}

/// How many chips fit on one line of `available` pixels, in order, leaving
/// room for the input and, when some don't fit, for the overflow part. A chip
/// never measured, like one just added while others were hidden, is shown
/// when everything before it fits, so it can be measured (`chipsThatFit`).
pub fn chips_that_fit(layout: &ChipsLayout) -> usize {
    let ChipsLayout {
        available,
        gap,
        widths,
        overflow,
        reserve,
    } = *layout;
    // Whether `count` chips, `used` pixels wide, fit beside `extra` and the input.
    let fit = |count: usize, used: Pixels, extra: Pixels| {
        (if count > 0 { used + gap } else { px(0.) }) + extra + reserve <= available
    };
    let unknown = widths.iter().position(Option::is_none);
    let measured = unknown.unwrap_or(widths.len());
    let mut used = px(0.);
    let mut fits = 0;
    // Every width before `measured` is known.
    for (index, width) in widths.iter().take(measured).enumerate() {
        used += if index > 0 { gap } else { px(0.) } + width.unwrap_or_default();
        if fits == index && fit(index + 1, used, overflow + gap) {
            fits = index + 1;
        }
    }
    match unknown {
        None if fit(widths.len(), used, px(0.)) => widths.len(),
        None => fits,
        Some(_) if fits == measured => measured + 1,
        Some(_) => fits,
    }
}

// What the parts around the input styled themselves with, as they rendered.
#[derive(Default)]
struct Styles {
    // The chips' padding and borders, which the content box leaves out.
    padding: Edges<Option<DefiniteLength>>,
    borders: Edges<Option<AbsoluteLength>>,
    // The chips' gap between columns, `columnGap`.
    gap: Option<DefiniteLength>,
    // The input's `min-width`.
    min_width: Option<Length>,
}

#[derive(Default)]
struct Measures {
    styles: Styles,
    // Each chip's width by key, kept while it's hidden, which measures nothing.
    widths: HashMap<SharedString, Pixels>,
    overflow: Option<Pixels>,
}

/// The chips around a combobox's input as the overflow part watches them
/// (`bindChipsOverflow`): what each measured and what the parts styled
/// themselves with. Clones share it.
#[derive(Clone, Default)]
pub struct ChipsOverflow(Rc<RefCell<Measures>>);

impl ChipsOverflow {
    pub fn new() -> Self {
        Self::default()
    }

    /// The chips part's style, read as it renders: its padding, borders and
    /// gap.
    pub fn chips_styled(&self, style: &StyleRefinement) {
        let styles = &mut self.0.borrow_mut().styles;
        styles.padding = Edges {
            top: style.padding.top,
            right: style.padding.right,
            bottom: style.padding.bottom,
            left: style.padding.left,
        };
        styles.borders = Edges {
            top: style.border_widths.top,
            right: style.border_widths.right,
            bottom: style.border_widths.bottom,
            left: style.border_widths.left,
        };
        styles.gap = style.gap.width;
    }

    /// The input part's style, read as it renders: its minimum width.
    pub fn input_styled(&self, style: &StyleRefinement) {
        self.0.borrow_mut().styles.min_width = style.min_size.width;
    }

    /// Forgets what was measured, as the overflow part leaves.
    pub fn clear(&self) {
        let mut measures = self.0.borrow_mut();
        measures.widths.clear();
        measures.overflow = None;
    }

    /// How many of `chips`, keys in value order, fit, or `None` when all of
    /// them do, from the bounds the parts in `bound` laid out with last frame
    /// (`decide`). A hidden chip, or one that laid out with no width, keeps
    /// the width it last had.
    pub fn decide(&self, chips: &[SharedString], bound: &[BoundPart], window: &Window) -> Option<usize> {
        let rem = window.rem_size();
        let mut measures = self.0.borrow_mut();
        let Some(container) = bound.iter().find(|part| part.slot == "chips").and_then(|part| part.bounds) else {
            return None;
        };
        let width = container.size.width;
        let resolve = |length: Option<DefiniteLength>, base: Pixels| {
            length.map_or(px(0.), |length| length.to_pixels(AbsoluteLength::Pixels(base), rem))
        };
        let border = |length: Option<AbsoluteLength>| length.map_or(px(0.), |length| length.to_pixels(rem));
        let (available, gap, reserve) = {
            let styles = &measures.styles;
            // The content box, which `contentBoxSize` reports.
            let available = (width
                - resolve(styles.padding.left, width)
                - resolve(styles.padding.right, width)
                - border(styles.borders.left)
                - border(styles.borders.right))
            .max(px(0.));
            let gap = resolve(styles.gap, available);
            let reserve = match styles.min_width {
                Some(Length::Definite(length)) => resolve(Some(length), available),
                _ => px(0.),
            };
            (available, gap, reserve)
        };
        for part in bound {
            let Some(bounds) = part.bounds else {
                continue;
            };
            // A hidden part reports nothing, and keeps the width it had.
            if bounds.size.width <= px(0.) {
                continue;
            }
            match (part.slot, &part.key) {
                ("chip", Some(key)) => {
                    measures.widths.insert(key.clone(), bounds.size.width);
                }
                ("chips-overflow", _) => measures.overflow = Some(bounds.size.width),
                _ => {}
            }
        }
        // Chips that left the value go with their widths.
        measures.widths.retain(|key, _| chips.contains(key));
        let widths: Vec<Option<Pixels>> = chips.iter().map(|key| measures.widths.get(key).copied()).collect();
        let visible = chips_that_fit(&ChipsLayout {
            available,
            gap,
            widths: &widths,
            overflow: measures.overflow.unwrap_or_default(),
            reserve,
        });
        (visible < chips.len()).then_some(visible)
    }
}
