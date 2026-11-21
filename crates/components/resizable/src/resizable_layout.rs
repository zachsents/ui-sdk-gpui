//! Port of `components/resizable/resizable-layout.ts`: the pure decisions
//! behind resizable panels: sizes and their units, the constraints each
//! panel puts on the layout, and how a layout moves when a handle does.

use std::collections::HashMap;
use std::str::FromStr;

use gpui::{Length, SharedString, px, relative, rems};
use ui_sdk_kernel::Key;
use ui_sdk_number::clamp;

/// A panel size: a percentage of the group, like `30.0` (`From<f64>`), or
/// one with a unit, in pixels or rems. Pixel and rem sizes follow the
/// group's size as it changes. [`ResizableSize::parse`] reads the
/// original's strings, like `"30%"`, `"240px"` or `"16rem"`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResizableSize {
    /// A percentage of the space panels share.
    Percent(f64),
    /// Pixels.
    Px(f64),
    /// Rems, which follow the window's rem size.
    Rem(f64),
}

impl ResizableSize {
    pub fn percent(value: f64) -> Self {
        Self::Percent(value)
    }

    pub fn px(value: f64) -> Self {
        Self::Px(value)
    }

    pub fn rem(value: f64) -> Self {
        Self::Rem(value)
    }

    /// Reads a size as the original writes it: `"30"` and `"30%"` are 30
    /// percent, and `"240px"` and `"16rem"` keep their unit. `None` for text
    /// that isn't a size.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (number, unit): (&str, fn(f64) -> Self) = if let Some(number) = text.strip_suffix("px") {
            (number, Self::Px)
        } else if let Some(number) = text.strip_suffix("rem") {
            (number, Self::Rem)
        } else if let Some(number) = text.strip_suffix('%') {
            (number, Self::Percent)
        } else {
            (text, Self::Percent)
        };
        number
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| !value.is_nan())
            .map(unit)
    }
}

impl From<f64> for ResizableSize {
    fn from(percent: f64) -> Self {
        Self::Percent(percent)
    }
}

impl FromStr for ResizableSize {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        Self::parse(text).ok_or(())
    }
}

/// What a panel allows, in percentages of the space panels share.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelConstraints {
    pub min: f64,
    pub max: f64,
    pub collapsible: bool,
    /// The size it collapses to, at most `min`.
    pub collapsed: f64,
    /// Where it starts, if it says.
    pub initial: Option<f64>,
}

/// The size props a panel passes (`PanelSizes`), which are the `panel`
/// part's props.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelSizes {
    /// Where the panel starts. Without one, panels share what's left evenly.
    pub default_size: Option<ResizableSize>,
    /// The smallest the panel gets while open. Defaults to 0.
    pub min_size: Option<ResizableSize>,
    /// The largest the panel gets. Defaults to 100 percent.
    pub max_size: Option<ResizableSize>,
    /// Lets the panel collapse below its minimum to `collapsed_size`: by
    /// dragging past half the way there, with Enter or a double-click on its
    /// handle, or through `collapse`.
    pub collapsible: bool,
    /// The size a collapsible panel collapses to. At 0, its content leaves
    /// the accessibility tree. Defaults to 0.
    pub collapsed_size: Option<ResizableSize>,
}

impl PanelSizes {
    pub fn default_size(mut self, size: impl Into<ResizableSize>) -> Self {
        self.default_size = Some(size.into());
        self
    }

    pub fn min_size(mut self, size: impl Into<ResizableSize>) -> Self {
        self.min_size = Some(size.into());
        self
    }

    pub fn max_size(mut self, size: impl Into<ResizableSize>) -> Self {
        self.max_size = Some(size.into());
        self
    }

    pub fn collapsible(mut self, collapsible: bool) -> Self {
        self.collapsible = collapsible;
        self
    }

    pub fn collapsed_size(mut self, size: impl Into<ResizableSize>) -> Self {
        self.collapsed_size = Some(size.into());
        self
    }
}

// Sizes closer than this are the same. Drags divide pixels by the group's
// size, so floats rarely land exactly.
pub(crate) const EPSILON: f64 = 1e-6;

/// Whether two sizes are the same, within float noise.
pub fn same_size(a: f64, b: f64) -> bool {
    (a - b).abs() < EPSILON
}

/// Whether two layouts are the same, panel by panel.
pub fn same_layout(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_size(*a, *b))
}

fn sum(sizes: &[f64]) -> f64 {
    sizes.iter().sum()
}

// `Math.min` and `Math.max`: NaN wins.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// Whether a size needs the group's pixels to resolve: pixels and rems do.
pub fn is_absolute(size: Option<ResizableSize>) -> bool {
    matches!(size, Some(ResizableSize::Px(_) | ResizableSize::Rem(_)))
}

/// A size in percent of `space`, the pixels panels share. Pixel and rem
/// sizes can't be resolved before the group is measured, so they're `None`
/// then.
pub fn to_percent(size: Option<ResizableSize>, space: f64, rem_size: f64) -> Option<f64> {
    let (value, pixels) = match size? {
        ResizableSize::Percent(value) => return (!value.is_nan()).then_some(value),
        ResizableSize::Px(value) => (value, value),
        ResizableSize::Rem(value) => (value, value * rem_size),
    };
    if value.is_nan() || space <= 0.0 {
        return None;
    }
    Some(pixels / space * 100.0)
}

/// A size as a flex basis, for the first frame, before the group lays its
/// panels out (`toCSS`).
pub fn to_length(size: ResizableSize) -> Length {
    match size {
        ResizableSize::Percent(value) => relative((value / 100.0) as f32).into(),
        ResizableSize::Px(value) => px(value as f32).into(),
        ResizableSize::Rem(value) => rems(value as f32).into(),
    }
}

/// What a panel's sizes allow, with `space` pixels to share.
pub fn constraints_of(sizes: &PanelSizes, space: f64, rem_size: f64) -> PanelConstraints {
    let min = clamp(
        to_percent(sizes.min_size, space, rem_size).unwrap_or(0.0),
        0.0,
        100.0,
    );
    let max = clamp(
        to_percent(sizes.max_size, space, rem_size).unwrap_or(100.0),
        min,
        100.0,
    );
    let initial = to_percent(sizes.default_size, space, rem_size);
    PanelConstraints {
        min,
        max,
        collapsible: sizes.collapsible,
        collapsed: clamp(
            to_percent(sizes.collapsed_size, space, rem_size).unwrap_or(0.0),
            0.0,
            min,
        ),
        initial: initial.map(|initial| clamp(initial, 0.0, 100.0)),
    }
}

// The smallest a panel gets: collapsed if it can, else its minimum.
fn lowest(panel: &PanelConstraints) -> f64 {
    if panel.collapsible {
        panel.collapsed
    } else {
        panel.min
    }
}

/// Whether a panel at `size` is down at its collapsed size, below its
/// minimum.
pub fn is_collapsed(panel: &PanelConstraints, size: f64) -> bool {
    panel.collapsible && size <= panel.collapsed + EPSILON && panel.collapsed < panel.min
}

// Whether a panel may be this size: within its bounds, or collapsed.
fn allowed(panel: &PanelConstraints, size: f64) -> bool {
    if size >= panel.min - EPSILON && size <= panel.max + EPSILON {
        return true;
    }
    panel.collapsible && same_size(size, panel.collapsed)
}

/// The size a panel takes when asked for `target`, coming from `current`.
/// Between collapsed and the minimum it snaps to one or the other: an open
/// panel collapses once the target falls below `threshold` of that span,
/// and a collapsed one opens once the target passes the same share from the
/// other end. Pointers use half the span. Keys use all of it, so one step
/// past the minimum collapses, and one step out of collapsed opens.
pub fn settle(panel: &PanelConstraints, target: f64, current: f64, threshold: f64) -> f64 {
    if target >= panel.min - EPSILON {
        return clamp(target, panel.min, panel.max);
    }
    if !panel.collapsible {
        return panel.min;
    }
    let span = panel.min - panel.collapsed;
    if current <= panel.collapsed + EPSILON {
        return if target > panel.collapsed + span * (1.0 - threshold) + EPSILON {
            panel.min
        } else {
            panel.collapsed
        };
    }
    if target < panel.collapsed + span * threshold - EPSILON {
        panel.collapsed
    } else {
        panel.min
    }
}

fn locked_at(locked: &[bool], index: usize) -> bool {
    locked.get(index).copied().unwrap_or(false)
}

/// The layout after moving handle `handle`, which sits between panels
/// `handle` and `handle + 1`, by `delta` percent. Positive moves it toward
/// the end. The panel on the side it moves away from grows; the other side
/// shrinks from the handle outward, so a panel at its minimum passes the
/// rest on to the next one, up to a handle `locked` marks, which keeps the
/// panels past it as they are. Returns the layout as it was when nothing
/// can move.
pub fn resize_at(
    layout: &[f64],
    panels: &[PanelConstraints],
    handle: usize,
    delta: f64,
    threshold: f64,
    locked: &[bool],
) -> Vec<f64> {
    if delta.abs() < EPSILON || handle + 1 >= layout.len() || panels.len() < layout.len() {
        return layout.to_vec();
    }
    let forward = delta > 0.0;
    let pivot = if forward { handle } else { handle + 1 };
    let shrinking = shrinkable(layout.len(), handle, forward, locked);
    let start = layout[pivot];
    let room = panels[pivot].max - start;
    let capacity: f64 = shrinking
        .iter()
        .map(|index| js_max(0.0, layout[*index] - lowest(&panels[*index])))
        .sum();
    let asked = js_min(js_min(delta.abs(), room), capacity);
    let wanted = settle(&panels[pivot], start + asked, start, threshold) - start;
    if wanted <= EPSILON {
        return layout.to_vec();
    }

    let mut next = layout.to_vec();
    let mut given = 0.0;
    for index in shrinking {
        let remaining = wanted - given;
        if remaining <= EPSILON {
            break;
        }
        let size = layout[index];
        let panel = &panels[index];
        let mut settled = js_min(size, settle(panel, size - remaining, size, threshold));
        // A collapse that frees more than the growing panel can take stops at
        // the minimum instead.
        if given + size - settled > room + EPSILON {
            settled = js_min(size, js_max(panel.min, size - remaining));
        }
        next[index] = settled;
        given += size - settled;
    }
    // Too little freed to open a collapsed panel, or to reach a size it
    // allows.
    if given <= EPSILON || !allowed(&panels[pivot], start + given) {
        return layout.to_vec();
    }
    next[pivot] = start + given;
    next
}

// The panels a move shrinks, from the handle outward: the one next to it,
// then each one past it up to the first locked handle. Handle `i` sits
// between panels `i` and `i + 1`.
fn shrinkable(count: usize, handle: usize, forward: bool, locked: &[bool]) -> Vec<usize> {
    let mut indices = Vec::new();
    if forward {
        let mut index = handle + 1;
        while index < count && (index == handle + 1 || !locked_at(locked, index - 1)) {
            indices.push(index);
            index += 1;
        }
    } else {
        let mut index = handle as isize;
        while index >= 0 && (index == handle as isize || !locked_at(locked, index as usize)) {
            indices.push(index as usize);
            index -= 1;
        }
    }
    indices
}

/// How far a handle moves for Home: its panel down to its smallest, which
/// may be collapsed.
pub fn home_delta(layout: &[f64], panels: &[PanelConstraints], handle: usize) -> f64 {
    lowest(&panels[handle]) - layout[handle]
}

/// How far a handle moves for End: its panel up to its largest.
pub fn end_delta(layout: &[f64], panels: &[PanelConstraints], handle: usize) -> f64 {
    panels[handle].max - layout[handle]
}

/// The range a handle's panel can take from here, for the separator's
/// value bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandleBounds {
    pub min: f64,
    pub max: f64,
}

/// The range a handle's panel can take from here.
pub fn handle_bounds(
    layout: &[f64],
    panels: &[PanelConstraints],
    handle: usize,
    locked: &[bool],
) -> HandleBounds {
    let home = home_delta(layout, panels, handle);
    let end = end_delta(layout, panels, handle);
    HandleBounds {
        min: resize_at(layout, panels, handle, home, 1.0, locked)[handle],
        max: resize_at(layout, panels, handle, end, 1.0, locked)[handle],
    }
}

/// [`handle_bounds`] for every handle, in one pass when no panel can
/// collapse and each is within its bounds. Then a handle's panel shrinks to
/// its minimum or to what the next panel can take, and grows to its maximum
/// or to what the panels after it can give, up to a locked handle.
pub fn all_handle_bounds(layout: &[f64], panels: &[PanelConstraints], locked: &[bool]) -> Vec<HandleBounds> {
    let handles = 0..layout.len().saturating_sub(1);
    let plain = panels.len() == layout.len()
        && panels
            .iter()
            .zip(layout)
            .all(|(panel, size)| !panel.collapsible && allowed(panel, *size));
    if !plain {
        return handles
            .map(|handle| handle_bounds(layout, panels, handle, locked))
            .collect();
    }
    // What the panels from each one to the next locked handle can give,
    // summed from the end, so a change near the start leaves the later sums
    // exactly as they were.
    let mut spare = vec![0.0; layout.len() + 1];
    for index in (0..layout.len()).rev() {
        let after = if locked_at(locked, index) {
            0.0
        } else {
            spare[index + 1]
        };
        spare[index] = js_max(0.0, layout[index] - panels[index].min) + after;
    }
    handles
        .map(|handle| {
            let size = layout[handle];
            let shrink = js_min(
                size - panels[handle].min,
                panels[handle + 1].max - layout[handle + 1],
            );
            let grow = js_min(panels[handle].max - size, spare[handle + 1]);
            HandleBounds {
                min: size - js_max(0.0, shrink),
                max: size + js_max(0.0, grow),
            }
        })
        .collect()
}

/// A layout every panel allows that adds up to 100: scaled to 100 when it
/// doesn't, each size settled into its panel's bounds, and what that leaves
/// over or short shared out in order. Collapsed panels stay collapsed.
pub fn fit_layout(layout: &[f64], panels: &[PanelConstraints]) -> Vec<f64> {
    let total = sum(layout);
    let scale = if total > EPSILON && !same_size(total, 100.0) {
        100.0 / total
    } else {
        1.0
    };
    let mut next: Vec<f64> = layout
        .iter()
        .enumerate()
        .map(|(index, size)| match panels.get(index) {
            Some(panel) => settle(panel, size * scale, size * scale, 0.5),
            None => size * scale,
        })
        .collect();
    let mut left = 100.0 - sum(&next);
    let mut index = 0;
    while index < next.len() && left.abs() > EPSILON {
        if let Some(panel) = panels.get(index)
            && !is_collapsed(panel, next[index])
        {
            let change = if left > 0.0 {
                js_min(left, panel.max - next[index])
            } else {
                js_max(left, panel.min - next[index])
            };
            next[index] += change;
            left -= change;
        }
        index += 1;
    }
    // Constraints that can't all hold: the last panel takes the rest, so it
    // still fills the group.
    if left.abs() > EPSILON
        && let Some(last) = next.last_mut()
    {
        *last += left;
    }
    next
}

/// Where panels start: their `default_size`, and what's left shared by the
/// panels without one.
pub fn initial_layout(panels: &[PanelConstraints]) -> Vec<f64> {
    let given: Vec<f64> = panels.iter().filter_map(|panel| panel.initial).collect();
    let left = js_max(0.0, 100.0 - sum(&given));
    let share = if panels.len() > given.len() {
        left / (panels.len() - given.len()) as f64
    } else {
        0.0
    };
    let sizes: Vec<f64> = panels
        .iter()
        .map(|panel| panel.initial.unwrap_or(share))
        .collect();
    fit_layout(&sizes, panels)
}

/// The layout after panels came or went: panels that stay keep their sizes
/// relative to each other, new panels start at their `default_size` or an
/// even share, and the result fits.
pub fn carry_layout(
    previous_ids: &[SharedString],
    previous_layout: &[f64],
    ids: &[SharedString],
    panels: &[PanelConstraints],
) -> Vec<f64> {
    let kept: HashMap<&SharedString, Option<f64>> = previous_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id, previous_layout.get(index).copied()))
        .collect();
    if !ids.iter().any(|id| kept.contains_key(id)) {
        return initial_layout(panels);
    }
    let fresh: Vec<Option<f64>> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            if kept.contains_key(id) {
                None
            } else {
                Some(
                    panels
                        .get(index)
                        .and_then(|panel| panel.initial)
                        .unwrap_or(100.0 / ids.len() as f64),
                )
            }
        })
        .collect();
    let kept_size = |id: &SharedString| kept.get(id).copied().flatten().unwrap_or(0.0);
    let kept_total: f64 = ids.iter().map(kept_size).sum();
    let room = js_max(
        0.0,
        100.0 - fresh.iter().map(|size| size.unwrap_or(0.0)).sum::<f64>(),
    );
    let divisor = if kept_total == 0.0 { 1.0 } else { kept_total };
    let sizes: Vec<f64> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| fresh[index].unwrap_or_else(|| kept_size(id) / divisor * room))
        .collect();
    fit_layout(&sizes, panels)
}

/// The layout with one panel at `target`, through the handle after it, or
/// the one before it when that gets closer, like for the last panel.
pub fn resize_panel(
    layout: &[f64],
    panels: &[PanelConstraints],
    index: usize,
    target: f64,
    threshold: f64,
    locked: &[bool],
) -> Vec<f64> {
    let Some(current) = layout.get(index) else {
        return layout.to_vec();
    };
    let delta = target - current;
    let mut candidates = Vec::new();
    if index + 1 < layout.len() && !locked_at(locked, index) {
        candidates.push(resize_at(layout, panels, index, delta, threshold, locked));
    }
    if index > 0 && !locked_at(locked, index - 1) {
        candidates.push(resize_at(layout, panels, index - 1, -delta, threshold, locked));
    }
    let mut best = layout.to_vec();
    for candidate in candidates {
        if (candidate[index] - target).abs() < (best[index] - target).abs() - EPSILON {
            best = candidate;
        }
    }
    best
}

/// Which panel Enter and a double-click on a handle collapse or expand: the
/// panel before it, as the APG's primary pane, or the one after when only
/// that one is collapsible.
pub fn toggle_target(panels: &[PanelConstraints], handle: usize) -> Option<usize> {
    if panels.get(handle).is_some_and(|panel| panel.collapsible) {
        return Some(handle);
    }
    if panels.get(handle + 1).is_some_and(|panel| panel.collapsible) {
        return Some(handle + 1);
    }
    None
}

/// How a handle's arrow keys move it, for [`key_delta`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyDeltaOptions {
    pub vertical: bool,
    pub rtl: bool,
    pub step: f64,
    pub large_step: f64,
    pub shift_key: bool,
}

/// Where a handle's arrow keys move it, in percent: one step, or the large
/// step with Shift, along the group's axis. In right-to-left layouts the
/// start of a horizontal group is on the right, so the arrows swap.
pub fn key_delta(key: &Key, options: KeyDeltaOptions) -> Option<f64> {
    let size = if options.shift_key {
        options.large_step
    } else {
        options.step
    };
    let (back, forward) = if options.vertical {
        (Key::ArrowUp, Key::ArrowDown)
    } else if options.rtl {
        (Key::ArrowRight, Key::ArrowLeft)
    } else {
        (Key::ArrowLeft, Key::ArrowRight)
    };
    if *key == forward {
        return Some(size);
    }
    if *key == back {
        return Some(-size);
    }
    None
}
