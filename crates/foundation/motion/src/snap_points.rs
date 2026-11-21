//! Port of `foundation/motion/snap-points.ts`.
//!
//! Where a sheet can rest. Snap points are written as the app thinks of them
//! (a fraction of the screen, pixels, rem, a percentage, or the content's own
//! size) and resolve to extents in px, which is all the machine knows.

use std::borrow::Cow;

use gpui::SharedString;

use crate::js;

/// A place the sheet can rest, measured from the edge it comes from. A
/// [`Number`](SnapPoint::Number) up to 1 is a fraction of the screen along
/// the sheet's axis, and a larger one is pixels. Pixels, rem and percentages
/// (of the screen) work too. [`Content`](SnapPoint::Content) is the sheet's
/// own size. None can be larger than the sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapPoint {
    /// A fraction of the screen up to 1, pixels past it, like `0.5` or `320`.
    Number(f64),
    /// `"…px"`.
    Px(f64),
    /// `"…rem"`, in the root font size.
    Rem(f64),
    /// `"…%"` of the screen.
    Percent(f64),
    /// `"content"`: the sheet's own size.
    Content,
}

impl SnapPoint {
    /// The id the machine and state use: the snap point as a string, like
    /// `"0.5"`, `"320px"` or `"content"`.
    pub fn id(&self) -> SharedString {
        snap_point_id(self).into()
    }
}

impl From<f64> for SnapPoint {
    fn from(value: f64) -> Self {
        SnapPoint::Number(value)
    }
}

/// A snap point resolved to an extent. Points within 1px of each other share
/// one entry.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSnapPoint {
    /// The id the machine and state use: the snap point as a string.
    pub id: SharedString,
    /// The snap point as written, which callbacks report.
    pub value: SnapPoint,
    /// px from the attached edge.
    pub extent: f64,
    /// Every snap point merged into this one, as ids.
    pub ids: Vec<SharedString>,
}

/// What snap points resolve against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapPointContext {
    /// The space along the axis, in px: the screen, less the keyboard.
    pub container: f64,
    /// The sheet's own size, in px, which caps every point.
    pub content: f64,
    /// The root font size, in px, for rem.
    pub root_font_size: f64,
}

fn snap_point_id(point: &SnapPoint) -> String {
    match *point {
        SnapPoint::Number(value) => js::number_string(value),
        SnapPoint::Px(value) => format!("{}px", js::number_string(value)),
        SnapPoint::Rem(value) => format!("{}rem", js::number_string(value)),
        SnapPoint::Percent(value) => format!("{}%", js::number_string(value)),
        SnapPoint::Content => "content".into(),
    }
}

// The extent of one snap point, or NaN when it can't be read.
fn snap_point_extent(point: &SnapPoint, context: &SnapPointContext) -> f64 {
    match *point {
        SnapPoint::Number(value) => {
            if value <= 1.0 {
                value * context.container
            } else {
                value
            }
        }
        SnapPoint::Content => context.content,
        SnapPoint::Rem(amount) => amount * context.root_font_size,
        SnapPoint::Percent(amount) => (amount / 100.0) * context.container,
        SnapPoint::Px(amount) => amount,
    }
}

struct Measured {
    value: SnapPoint,
    index: usize,
    extent: f64,
}

/// Resolves snap points to ascending extents. `container` is the space along
/// the axis (the screen, less the keyboard) and `content` is the sheet's own
/// size, which caps every point. Points within 1px of each other merge, and
/// the one written later names the entry, so `Content` equal to `1` on a
/// short screen doesn't leave a dead snap point.
pub fn resolve_snap_points(
    points: &[SnapPoint],
    context: SnapPointContext,
) -> Vec<ResolvedSnapPoint> {
    let mut measured: Vec<Measured> = points
        .iter()
        .enumerate()
        .map(|(index, value)| Measured {
            value: *value,
            index,
            extent: js::min(snap_point_extent(value, &context), context.content),
        })
        .filter(|point| point.extent > 0.0)
        .collect();
    measured.sort_by(|a, b| {
        a.extent
            .partial_cmp(&b.extent)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.index.cmp(&b.index))
    });
    let mut resolved: Vec<(ResolvedSnapPoint, usize)> = Vec::new();
    for point in measured {
        let id: SharedString = snap_point_id(&point.value).into();
        if let Some((previous, previous_index)) = resolved.last_mut()
            && point.extent - previous.extent < 1.0
        {
            previous.ids.push(id.clone());
            if point.index > *previous_index {
                previous.id = id;
                previous.value = point.value;
                *previous_index = point.index;
            }
            previous.extent = js::max(previous.extent, point.extent);
            continue;
        }
        resolved.push((
            ResolvedSnapPoint {
                id: id.clone(),
                value: point.value,
                extent: js::round(point.extent * 100.0) / 100.0,
                ids: vec![id],
            },
            point.index,
        ));
    }
    resolved.into_iter().map(|(point, _)| point).collect()
}

/// What names a snap point where one is looked up: the point itself, or its
/// id as a string, the way the original takes either.
pub trait SnapPointKey {
    /// The id, as `String(point)` reads it.
    fn snap_point_id(&self) -> Cow<'_, str>;
}

impl SnapPointKey for SnapPoint {
    fn snap_point_id(&self) -> Cow<'_, str> {
        Cow::Owned(snap_point_id(self))
    }
}

impl SnapPointKey for str {
    fn snap_point_id(&self) -> Cow<'_, str> {
        Cow::Borrowed(self)
    }
}

impl SnapPointKey for String {
    fn snap_point_id(&self) -> Cow<'_, str> {
        Cow::Borrowed(self)
    }
}

impl SnapPointKey for SharedString {
    fn snap_point_id(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.as_ref())
    }
}

/// The index of the resolved entry a snap point merged into, if any.
pub fn find_snap_index<K: SnapPointKey + ?Sized>(
    resolved: &[ResolvedSnapPoint],
    point: Option<&K>,
) -> Option<usize> {
    let id = point?.snap_point_id();
    resolved
        .iter()
        .position(|entry| entry.ids.iter().any(|merged| merged.as_ref() == id.as_ref()))
}

/// Whether `point` is at or below `limit`, like a sheet resting where the
/// page stays usable.
pub fn is_at_or_below<P: SnapPointKey + ?Sized, L: SnapPointKey + ?Sized>(
    resolved: &[ResolvedSnapPoint],
    point: Option<&P>,
    limit: Option<&L>,
) -> bool {
    let below = find_snap_index(resolved, limit);
    let index = find_snap_index(resolved, point);
    matches!((below, index), (Some(below), Some(index)) if index <= below)
}
