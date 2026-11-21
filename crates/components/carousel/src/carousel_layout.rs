//! Port of `components/carousel/carousel-layout.ts`: where a carousel's pages
//! are, from its slides' measured layout. Pure. Positions are scroll
//! positions along the carousel's axis, from the start edge: right to left
//! counts from the right.

use ui_sdk_number::clamp;

/// Where a page lines up in the viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CarouselAlign {
    #[default]
    Start,
    Center,
    End,
}

impl CarouselAlign {
    /// The original's name for it, like `"center"`.
    pub fn as_str(self) -> &'static str {
        match self {
            CarouselAlign::Start => "start",
            CarouselAlign::Center => "center",
            CarouselAlign::End => "end",
        }
    }
}

/// How many slides one page holds, or `Auto` for as many as fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CarouselSlidesPerPage {
    Count(usize),
    Auto,
}

impl Default for CarouselSlidesPerPage {
    fn default() -> Self {
        CarouselSlidesPerPage::Count(1)
    }
}

impl From<usize> for CarouselSlidesPerPage {
    fn from(count: usize) -> Self {
        CarouselSlidesPerPage::Count(count)
    }
}

/// A slide's place along the axis, from the start of the scrollable area.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CarouselSlideBox {
    pub start: f64,
    pub size: f64,
}

/// What the layout is measured from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarouselGeometry<'a> {
    pub slides: &'a [CarouselSlideBox],
    /// The viewport's size along the axis, less its scroll padding: where pages line up.
    pub snapport: f64,
    /// The viewport's scroll padding at the start edge.
    pub padding_start: f64,
    /// The largest scroll position.
    pub max_scroll: f64,
}

/// One page: the slides that scroll in together, and where it rests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CarouselPage {
    /// Indexes of the slides it holds, in order.
    pub slides: Vec<usize>,
    /// The scroll position it rests at.
    pub position: f64,
    /// The slide the page snaps to.
    pub leader: usize,
    /// The leader's distance from the snapport's start, which lines the page up by `align`.
    pub margin: f64,
}

// Pages closer than this share one position, like the last few slides that
// can't scroll to the start because the content ends.
const MERGE_DISTANCE: f64 = 1.0;

/// Groups slides into pages: `count` at a time, or with `Auto`, as many as fit in the snapport.
pub fn group_slides(slides: &[CarouselSlideBox], snapport: f64, count: CarouselSlidesPerPage) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    if let CarouselSlidesPerPage::Count(count) = count {
        let size = count.max(1);
        let mut index = 0;
        while index < slides.len() {
            groups.push((index..(index + size).min(slides.len())).collect());
            index += size;
        }
        return groups;
    }
    for index in 0..slides.len() {
        let end = slides[index].start + slides[index].size;
        // Half a pixel of slack, since measured sizes carry fractions.
        match groups.last_mut() {
            Some(group) if end - slides[group[0]].start <= snapport + 0.5 => group.push(index),
            _ => groups.push(vec![index]),
        }
    }
    groups
}

// How far before the snapport's start a page's first slide sits, so the page lines up by `align`.
fn align_offset(align: CarouselAlign, snapport: f64, size: f64) -> f64 {
    match align {
        CarouselAlign::Start => 0.0,
        CarouselAlign::Center => (snapport - size) / 2.0,
        CarouselAlign::End => snapport - size,
    }
}

/// The carousel's pages. Each rests where its leader lines up, clamped to
/// the scroll range, and pages that would rest at the same place merge into
/// one, so five slides three to a view make three pages.
pub fn carousel_pages(
    geometry: CarouselGeometry,
    align: CarouselAlign,
    slides_per_page: CarouselSlidesPerPage,
) -> Vec<CarouselPage> {
    let CarouselGeometry {
        slides,
        snapport,
        padding_start,
        max_scroll,
    } = geometry;
    let mut pages: Vec<CarouselPage> = Vec::new();
    for group in group_slides(slides, snapport, slides_per_page) {
        let first = slides[group[0]];
        let last = slides[group[group.len() - 1]];
        let margin = align_offset(align, snapport, last.start + last.size - first.start);
        let position = clamp(first.start - margin - padding_start, 0.0, max_scroll);
        if let Some(previous) = pages.last_mut()
            && (position - previous.position).abs() < MERGE_DISTANCE
        {
            previous.slides.extend(group);
            continue;
        }
        pages.push(CarouselPage {
            leader: group[0],
            slides: group,
            position,
            margin,
        });
    }
    pages
}

/// A looping carousel's slides, wherever its rotation puts them: their sizes
/// in their own order, the gap between neighbors, and where the slide shown
/// first starts. The track wraps, so the last slide's neighbor is the first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CarouselTrack {
    pub sizes: Vec<f64>,
    pub gap: f64,
    pub start: f64,
}

/// Where a looping carousel puts its pages, besides its track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarouselLoopView {
    pub snapport: f64,
    pub padding_start: f64,
    pub max_scroll: f64,
    pub align: CarouselAlign,
    pub slides_per_page: CarouselSlidesPerPage,
}

/// Reads a looping track from slides measured with slide `first` shown
/// first. `None` unless the slides sit in that rotation with even gaps, as
/// when the content doesn't lay out as a row; the carousel rewinds instead.
pub fn measure_track(slides: &[CarouselSlideBox], first: usize) -> Option<CarouselTrack> {
    if slides.len() < 2 || first >= slides.len() {
        return None;
    }
    let lead = slides[first];
    let next = slides[(first + 1) % slides.len()];
    let track = CarouselTrack {
        sizes: slides.iter().map(|slide| slide.size).collect(),
        gap: next.start - lead.start - lead.size,
        start: lead.start,
    };
    let placed = place_slides(&track, first);
    placed
        .iter()
        .zip(slides)
        .all(|(placed, slide)| (placed.start - slide.start).abs() < 1.0)
        .then_some(track)
}

/// Where each slide sits when the track shows slide `first` first.
pub fn place_slides(track: &CarouselTrack, first: usize) -> Vec<CarouselSlideBox> {
    let count = track.sizes.len();
    let mut boxes = vec![CarouselSlideBox::default(); count];
    let mut at = track.start;
    for step in 0..count {
        let index = (first + step) % count;
        boxes[index] = CarouselSlideBox {
            start: at,
            size: track.sizes[index],
        };
        at += track.sizes[index] + track.gap;
    }
    boxes
}

// A looping carousel's pages, grouped in the slides' own order.
fn loop_groups(track: &CarouselTrack, view: &CarouselLoopView) -> Vec<Vec<usize>> {
    group_slides(&place_slides(track, 0), view.snapport, view.slides_per_page)
}

fn loop_page(boxes: &[CarouselSlideBox], group: &[usize], view: &CarouselLoopView) -> CarouselPage {
    let first = boxes[group[0]];
    let last = boxes[group[group.len() - 1]];
    let margin = align_offset(view.align, view.snapport, last.start + last.size - first.start);
    CarouselPage {
        slides: group.to_vec(),
        position: first.start - margin - view.padding_start,
        leader: group[0],
        margin,
    }
}

/// Whether a track is long enough to loop seamlessly: the scroll range holds
/// a page's step to either side of the current page. Two slides one to a
/// view don't, and the carousel rewinds instead.
pub fn can_loop(track: &CarouselTrack, view: &CarouselLoopView) -> bool {
    let groups = loop_groups(track, view);
    if groups.len() < 2 {
        return false;
    }
    let boxes = place_slides(track, 0);
    let length: f64 = track.sizes.iter().map(|size| size + track.gap).sum();
    let mut step: f64 = 0.0;
    for (index, group) in groups.iter().enumerate() {
        let next = &groups[(index + 1) % groups.len()];
        step = step.max((boxes[next[0]].start - boxes[group[0]].start + length) % length);
    }
    view.max_scroll >= 2.0 * step - 1.0
}

/// A looping carousel's pages with slide `first` shown first, which leads a
/// page, so no page splits where the track wraps. Positions aren't clamped
/// or merged: a loop has no ends, and pages far from the current one may lie
/// past the scroll range until the track rotates again.
pub fn loop_pages(track: &CarouselTrack, view: &CarouselLoopView, first: usize) -> Vec<CarouselPage> {
    let boxes = place_slides(track, first);
    loop_groups(track, view)
        .iter()
        .map(|group| loop_page(&boxes, group, view))
        .collect()
}

/// Where a looping carousel's scroll position is and which slide it shows
/// first, for [`loop_rotation`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopAt {
    pub position: f64,
    pub first: usize,
}

/// The page a rotation heads for, and which way, 1 or -1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopHeading {
    pub page: usize,
    pub heading: i32,
}

/// The rotation [`loop_rotation`] picks: the slide shown first, and how far
/// the scroll position moves to keep the same slides in view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopRotation {
    pub first: usize,
    pub delta: f64,
}

/// Picks the slide a looping carousel shows first, among the pages'
/// leaders, so the slides showing at `at.position` keep the most room on
/// both sides, and with `to`, the page it heads for rests within the scroll
/// range the way it's heading. `None` when no rotation fits. The current
/// rotation wins ties, and with `lazy`, wins whenever it fits.
pub fn loop_rotation(
    track: &CarouselTrack,
    view: &CarouselLoopView,
    at: LoopAt,
    to: Option<LoopHeading>,
    lazy: bool,
) -> Option<LoopRotation> {
    let groups = loop_groups(track, view);
    let now = place_slides(track, at.first);
    // The slide at the snapport's start, which stays in place.
    let edge = at.position + view.padding_start;
    let mut anchor = at.first;
    for (index, slide) in now.iter().enumerate() {
        if slide.start <= edge + 0.5 && slide.start > now[anchor].start {
            anchor = index;
        }
    }
    let mut leaders: Vec<usize> = groups.iter().map(|group| group[0]).collect();
    if leaders.contains(&at.first) {
        leaders.insert(0, at.first);
    }
    let mut best: Option<(LoopRotation, f64)> = None;
    for first in leaders {
        let placed;
        let boxes = if first == at.first {
            &now
        } else {
            placed = place_slides(track, first);
            &placed
        };
        let delta = boxes[anchor].start - now[anchor].start;
        let position = at.position + delta;
        let mut room = position.min(view.max_scroll - position);
        if let Some(to) = to {
            let Some(group) = groups.get(to.page) else {
                continue;
            };
            let target = loop_page(boxes, group, view).position;
            // It must lie the way the carousel heads, or right where it is.
            if (target - position) * f64::from(to.heading) < -0.5 {
                continue;
            }
            room = room.min(target).min(view.max_scroll - target);
        }
        if room < -0.5 {
            continue;
        }
        if best.is_none_or(|(_, best_room)| room > best_room + 0.5) {
            best = Some((LoopRotation { first, delta }, room));
        }
        if lazy && first == at.first {
            break;
        }
    }
    best.map(|(rotation, _)| rotation)
}

/// The index of the page resting nearest `position`, or `None` without pages.
pub fn nearest_page(pages: impl IntoIterator<Item = f64>, position: f64) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (index, at) in pages.into_iter().enumerate() {
        let distance = (at - position).abs();
        if best.is_none_or(|(_, nearest)| distance < nearest) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

/// Whether a slide counts as in view at `position`: at least half of it
/// shows, or, for a slide larger than the viewport, it fills at least half
/// of it.
pub fn is_slide_in_view(slide: &CarouselSlideBox, position: f64, viewport: f64) -> bool {
    let shown = (slide.start + slide.size).min(position + viewport) - slide.start.max(position);
    shown >= slide.size.min(viewport) / 2.0 - 0.5
}

/// The page after or before `page` (from 1), wrapping with `loop`. Returns
/// `page` at an end.
pub fn step_page(page: usize, by: i32, count: usize, r#loop: bool) -> usize {
    let next = page as i64 + i64::from(by);
    if next >= 1 && next <= count as i64 {
        return next as usize;
    }
    if !r#loop || count < 2 {
        return page;
    }
    if next < 1 { count } else { 1 }
}
