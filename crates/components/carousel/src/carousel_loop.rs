//! Port of `components/carousel/carousel-loop.ts`: a seamless loop. The
//! slides rotate, at rest and whenever a move would run out of slides, so
//! the current page always has slides on both sides, for fingers and wheels
//! as for buttons and springs. Nothing moves in the tree, so focus and the
//! app's own state are untouched. The geometry is pure, in
//! `carousel_layout`.

use gpui::{Context, SharedString, Window};

use crate::carousel::Carousel;
use crate::carousel_layout::{
    CarouselLoopView, CarouselPage, CarouselSlideBox, CarouselTrack, LoopAt, LoopHeading, can_loop,
    loop_pages, loop_rotation, measure_track, nearest_page,
};

/// What a looping track keeps between measures (`createCarouselLoop`'s closure).
#[derive(Debug, Default)]
pub struct CarouselLoop {
    track: Option<CarouselTrack>,
    view: Option<CarouselLoopView>,
    // The slide shown first, and whether the slides carry an order to clear.
    first_key: Option<SharedString>,
    ordered: bool,
    rotating: bool,
    // The content doesn't lay out as an even row, so the carousel rewinds.
    unsupported: bool,
}

impl CarouselLoop {
    fn shown_first(&self, slides: &[SharedString]) -> usize {
        self.first_key
            .as_ref()
            .and_then(|key| slides.iter().position(|slide| slide == key))
            .unwrap_or(0)
    }

    fn reset_order(&mut self) {
        self.first_key = None;
        self.ordered = false;
    }

    /// Whether the carousel loops seamlessly now.
    pub fn is_active(&self) -> bool {
        self.track.is_some()
    }

    /// Whether a rotation is measuring, which keeps the scroll position for it.
    pub fn is_rotating(&self) -> bool {
        self.rotating
    }

    /// Whether the slides carry a rotation, which shifts them from where the
    /// content lays them out.
    pub fn is_ordered(&self) -> bool {
        self.ordered
    }

    /// Writes the slides' order for the current rotation before a measure,
    /// and returns the slide shown first, or `None` when it doesn't loop.
    pub fn prepare(&mut self, slides: &[SharedString], loops: bool) -> Option<usize> {
        if !loops || self.unsupported {
            self.reset_order();
            return None;
        }
        let first = self.shown_first(slides);
        self.ordered = first > 0;
        Some(first)
    }

    /// A looping carousel's pages from measured slides, with `first` shown
    /// first, or `None` when it can't loop seamlessly and rewinds instead.
    pub fn layout(
        &mut self,
        boxes: &[CarouselSlideBox],
        next: CarouselLoopView,
        first: Option<usize>,
    ) -> Option<Vec<CarouselPage>> {
        self.view = Some(next);
        self.track = first.and_then(|first| measure_track(boxes, first));
        let rotated = first.is_some_and(|first| first > 0);
        if rotated && self.track.is_none() {
            self.unsupported = true;
        }
        if self.track.as_ref().is_some_and(|track| !can_loop(track, &next)) {
            self.track = None;
        }
        // A rotation that didn't hold goes back to the slides' own order on the next measure.
        if rotated && self.track.is_none() {
            self.first_key = None;
        }
        self.track
            .as_ref()
            .map(|track| loop_pages(track, &next, first.unwrap_or(0)))
    }

    /// Clears the rotation, when the viewport goes.
    pub fn reset(&mut self) {
        self.reset_order();
        self.unsupported = false;
    }
}

impl Carousel {
    fn rotate(&mut self, to: Option<LoopHeading>, lazy: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(track), Some(view)) = (self.seamless.track.clone(), self.seamless.view) else {
            return;
        };
        if self.motion.as_ref().is_none_or(|motion| motion.is_held()) {
            return;
        }
        // The slides as the last measure read them, which a rotation measures again.
        let slides = self.order.clone();
        let first = self.seamless.shown_first(&slides);
        let position = self.motion_position();
        let Some(pick) = loop_rotation(&track, &view, LoopAt { position, first }, to, lazy) else {
            return;
        };
        if pick.first == first {
            return;
        }
        let before = self.visual.clone();
        let Some(key) = slides.get(pick.first).cloned() else {
            return;
        };
        self.seamless.first_key = Some(key.clone());
        self.seamless.rotating = true;
        self.measure(window, cx);
        self.seamless.rotating = false;
        // The slides didn't follow the order, and the measure reset it.
        if self.seamless.first_key.as_ref() != Some(&key) {
            return;
        }
        let after = self.visual.clone();
        self.motion_shift(
            position,
            pick.delta,
            |index| {
                index
                    .and_then(|index| before.get(index))
                    .and_then(|page| after.iter().position(|other| other == page))
            },
            cx,
        );
    }

    /// Rotates, if it must, so the page at `index` lies ahead the way the
    /// carousel heads: `heading`, the step's way, or else the short way round.
    pub(crate) fn rotate_toward(
        &mut self,
        index: usize,
        heading: Option<i32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.seamless.track.is_none() || self.motion.is_none() {
            return;
        }
        let count = self.layout.len();
        let Some(page) = self.layout.get(index) else {
            return;
        };
        let at = self.motion_position();
        let nearest = nearest_page(self.layout.iter().map(|page| page.position), at).unwrap_or(0);
        let ahead = (index + count - nearest) % count;
        let way = if ahead == 0 {
            let difference = page.position - at;
            if difference > 0.0 {
                1
            } else if difference < 0.0 {
                -1
            } else {
                0
            }
        } else {
            heading.unwrap_or(if ahead as f64 <= count as f64 / 2.0 { 1 } else { -1 })
        };
        let heading = if way < 0 { -1 } else { 1 };
        self.rotate(Some(LoopHeading { page: index, heading }), true, window, cx);
    }

    /// Rotates at rest, so the current page sits mid-track.
    pub(crate) fn recenter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.seamless.track.is_some()
            && !self.seamless.rotating
            && !self.motion.as_ref().is_some_and(|motion| motion.is_moving())
        {
            self.rotate(None, false, window, cx);
        }
    }
}
