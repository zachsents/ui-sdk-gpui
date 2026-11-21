//! Port of `foundation/lists/virtualizer.ts`: renders only the rows a
//! scroller shows, measuring each as it renders and estimating the rest.
//!
//! It keeps the original's math over a gpui scroll container rather than
//! gpui's own lists: `uniform_list` takes rows of one size only, and `list`
//! (`ListState`) keeps no estimates, no overscan, no sideways rows, no
//! scroll margin, no rows kept wherever the scroller is, and no end to load
//! more at. Render [`Virtualizer::items`] absolutely positioned, at their
//! `start`, inside a box [`Virtualizer::size`] long, in a scroller that
//! tracks the handle [`Virtualizer::connect`] took, and put
//! [`Virtualizer::measure`] on each row whose size isn't the estimate.
//! gpui renders the scroller's view again as it scrolls, which is when the
//! original hears `scroll`; a row whose measured size changed renders the
//! owner again.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::{Rc, Weak};

use gpui::{
    AnyWindowHandle, App, Bounds, Context, Element, ElementId, EntityId, GlobalElementId, InspectorElementId,
    LayoutId, ParentElement, Pixels, Refineable, ScrollHandle, SharedString, Style, StyleRefinement, Styled,
    WeakEntity, Window, point, px,
};

/// One row to render: where it starts along the scroller and its size
/// (`VirtualItem`).
#[derive(Clone, Debug, PartialEq)]
pub struct VirtualItem {
    pub index: usize,
    pub key: SharedString,
    pub start: Pixels,
    pub size: Pixels,
}

/// Where `scroll_to_index` puts a row (`VirtualAlign`). `Auto` scrolls only as
/// far as it takes to show it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VirtualAlign {
    #[default]
    Auto,
    Start,
    Center,
    End,
}

/// How [`create_virtualizer`] reads the list (`VirtualizerOptions`), from the
/// component or view that renders it.
pub struct VirtualizerOptions<C: 'static> {
    /// How many rows the list has. Read on every update, so it can grow,
    /// like a list that loads more.
    pub count: Rc<dyn Fn(&C) -> usize>,
    /// The size of a row before it's measured. A close guess keeps the
    /// scrollbar steady.
    pub estimate: Rc<dyn Fn(&C, usize) -> Pixels>,
    /// A stable key for each row, so a measured size follows its row when
    /// the list sorts or filters. Defaults to the index.
    pub key: Option<Rc<dyn Fn(&C, usize) -> SharedString>>,
    /// How many rows to render past each edge, so a fast scroll or Tab
    /// doesn't show a gap. Defaults to 4.
    pub overscan: usize,
    /// Whether the list scrolls sideways.
    pub horizontal: bool,
    /// Space in the scroller before the first row, like a sticky header.
    pub scroll_margin: Pixels,
    /// Rows rendered wherever the scroller is, like the one with focus, so
    /// scrolling it away doesn't drop focus.
    pub keep: Option<Rc<dyn Fn(&C) -> Vec<usize>>>,
    /// Runs when the rendered rows come within `end_threshold` rows of the
    /// last, to load more, once per `count`, so it doesn't ask again before
    /// the list grows. It runs inside the owner's update.
    pub on_end: Option<Rc<dyn Fn(&mut C, &mut Window, &mut Context<C>)>>,
    /// How close to the end `on_end` runs, in rows. Defaults to 10.
    pub end_threshold: usize,
}

impl<C: 'static> VirtualizerOptions<C> {
    pub fn new(count: impl Fn(&C) -> usize + 'static, estimate: impl Fn(&C, usize) -> Pixels + 'static) -> Self {
        Self {
            count: Rc::new(count),
            estimate: Rc::new(estimate),
            key: None,
            overscan: 4,
            horizontal: false,
            scroll_margin: px(0.),
            keep: None,
            on_end: None,
            end_threshold: 10,
        }
    }

    pub fn key(mut self, key: impl Fn(&C, usize) -> SharedString + 'static) -> Self {
        self.key = Some(Rc::new(key));
        self
    }

    pub fn overscan(mut self, overscan: usize) -> Self {
        self.overscan = overscan;
        self
    }

    pub fn horizontal(mut self, horizontal: bool) -> Self {
        self.horizontal = horizontal;
        self
    }

    pub fn scroll_margin(mut self, margin: impl Into<Pixels>) -> Self {
        self.scroll_margin = margin.into();
        self
    }

    pub fn keep(mut self, keep: impl Fn(&C) -> Vec<usize> + 'static) -> Self {
        self.keep = Some(Rc::new(keep));
        self
    }

    pub fn on_end(mut self, on_end: impl Fn(&mut C, &mut Window, &mut Context<C>) + 'static) -> Self {
        self.on_end = Some(Rc::new(on_end));
        self
    }

    pub fn end_threshold(mut self, threshold: usize) -> Self {
        self.end_threshold = threshold;
        self
    }
}

struct VirtState {
    sizes: HashMap<SharedString, Pixels>,
    scroller: Option<ScrollHandle>,
    offset: Pixels,
    viewport: Pixels,
    // Where each row starts, rebuilt when a count or size changes.
    starts: Vec<Pixels>,
    dirty: bool,
    snapshot: Rc<[VirtualItem]>,
    total: Pixels,
    ended_at: Option<usize>,
}

// Sizes rows reported as they laid out, until the frame is drawn.
struct Measured {
    pending: RefCell<Vec<(usize, Pixels)>>,
    queued: Cell<bool>,
}

struct VirtInner<C: 'static> {
    options: VirtualizerOptions<C>,
    entity: WeakEntity<C>,
    entity_id: EntityId,
    window: AnyWindowHandle,
    state: RefCell<VirtState>,
    measured: Rc<Measured>,
}

/// Renders a long list by rendering only what's in view
/// (`createVirtualizer`'s return). Cloning it gives another handle to the
/// same one.
pub struct Virtualizer<C: 'static> {
    inner: Rc<VirtInner<C>>,
}

impl<C: 'static> Clone for Virtualizer<C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Renders a long list by rendering only what's in view
/// (`createVirtualizer`), for the component or view of `cx`, which it
/// renders again when a row's measured size changes.
pub fn create_virtualizer<C: 'static>(options: VirtualizerOptions<C>, window: &Window, cx: &Context<C>) -> Virtualizer<C> {
    Virtualizer {
        inner: Rc::new(VirtInner {
            options,
            entity: cx.weak_entity(),
            entity_id: cx.entity_id(),
            window: window.window_handle(),
            state: RefCell::new(VirtState {
                sizes: HashMap::new(),
                scroller: None,
                offset: px(0.),
                viewport: px(0.),
                starts: Vec::new(),
                dirty: true,
                snapshot: Vec::new().into(),
                total: px(0.),
                ended_at: None,
            }),
            measured: Rc::new(Measured {
                pending: RefCell::new(Vec::new()),
                queued: Cell::new(false),
            }),
        }),
    }
}

impl<C: 'static> VirtInner<C> {
    fn key_of(&self, this: &C, index: usize) -> SharedString {
        match &self.options.key {
            Some(key) => key(this, index),
            None => SharedString::from(index.to_string()),
        }
    }

    fn size_of(&self, this: &C, state: &VirtState, index: usize) -> Pixels {
        state
            .sizes
            .get(&self.key_of(this, index))
            .copied()
            .unwrap_or_else(|| (self.options.estimate)(this, index))
    }

    fn layout(&self, this: &C, state: &mut VirtState) {
        let count = (self.options.count)(this);
        if !state.dirty && state.starts.len() == count {
            return;
        }
        state.dirty = false;
        let mut starts = Vec::with_capacity(count);
        let mut at = px(0.);
        for index in 0..count {
            starts.push(at);
            at += self.size_of(this, state, index);
        }
        state.starts = starts;
        state.total = at;
    }

    // The first row whose end is past `at`, by binary search.
    fn row_at(&self, this: &C, state: &VirtState, at: Pixels) -> usize {
        let mut low = 0;
        let mut high = state.starts.len().saturating_sub(1);
        while low < high {
            let middle = (low + high) / 2;
            if state.starts[middle] + self.size_of(this, state, middle) <= at {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low
    }

    // Reads where the scroller is, which the original hears on `scroll`.
    fn read(&self, state: &mut VirtState) {
        let Some(scroller) = &state.scroller else {
            return;
        };
        let offset = scroller.offset();
        let bounds = scroller.bounds();
        if self.options.horizontal {
            state.offset = -offset.x;
            state.viewport = bounds.size.width;
        } else {
            state.offset = -offset.y;
            state.viewport = bounds.size.height;
        }
    }

    fn compute(&self, this: &C, cx: &App) -> bool {
        let options = &self.options;
        let mut state = self.state.borrow_mut();
        self.read(&mut state);
        self.layout(this, &mut state);
        let count = state.starts.len();
        let margin = options.scroll_margin;
        let mut indexes = BTreeSet::new();
        if count > 0 {
            let from = self.row_at(this, &state, (state.offset - margin).max(px(0.)));
            let mut to = from;
            let end = state.offset - margin + state.viewport;
            while to < count - 1 && state.starts[to] + self.size_of(this, &state, to) < end {
                to += 1;
            }
            let window_from = from.saturating_sub(options.overscan);
            let window_to = (to + options.overscan).min(count - 1);
            indexes.extend(window_from..=window_to);
            if let Some(keep) = &options.keep {
                indexes.extend(keep(this).into_iter().filter(|index| *index < count));
            }
            if options.on_end.is_some()
                && to + 1 + options.end_threshold >= count
                && state.ended_at != Some(count)
            {
                state.ended_at = Some(count);
                self.queue_end(cx);
            }
        }
        let next: Vec<VirtualItem> = indexes
            .into_iter()
            .map(|index| VirtualItem {
                index,
                key: self.key_of(this, index),
                start: state.starts[index],
                size: self.size_of(this, &state, index),
            })
            .collect();
        if next.as_slice() == &state.snapshot[..] {
            return false;
        }
        state.snapshot = next.into();
        true
    }

    // `on_end` runs once the current render is over, inside the owner's
    // update.
    fn queue_end(&self, cx: &App) {
        let Some(on_end) = self.options.on_end.clone() else {
            return;
        };
        let entity = self.entity.clone();
        let window = self.window;
        cx.spawn(async move |cx| {
            window
                .update(cx, |_, window, cx| {
                    entity.update(cx, |this, cx| on_end(this, window, cx)).ok();
                })
                .ok();
        })
        .detach();
    }

    fn record(&self, this: &C, index: usize, size: Pixels) -> bool {
        let key = self.key_of(this, index);
        let options = &self.options;
        let mut state = self.state.borrow_mut();
        if size <= px(0.) || state.sizes.get(&key) == Some(&size) {
            return false;
        }
        self.layout(this, &mut state);
        if index >= state.starts.len() {
            return false;
        }
        // A row above the top that changes size would move what's in view,
        // so the scroller moves with it.
        let above = state.scroller.is_some() && state.starts[index] + options.scroll_margin < state.offset;
        let change = size - self.size_of(this, &state, index);
        state.sizes.insert(key, size);
        state.dirty = true;
        if above && let Some(scroller) = state.scroller.clone() {
            let offset = scroller.offset();
            if options.horizontal {
                scroller.set_offset(point(offset.x - change, offset.y));
            } else {
                scroller.set_offset(point(offset.x, offset.y - change));
            }
            self.read(&mut state);
        }
        true
    }
}

impl<C: 'static> Virtualizer<C> {
    /// The rows to render now, in order: the ones in view, the overscan, and
    /// the ones `keep` names. The same `Rc` until something changes.
    pub fn items(&self, this: &C, cx: &App) -> Rc<[VirtualItem]> {
        self.inner.compute(this, cx);
        self.inner.state.borrow().snapshot.clone()
    }

    /// The height, or the width sideways, of every row together, for the
    /// box the rows sit in.
    pub fn size(&self, this: &C) -> Pixels {
        let mut state = self.inner.state.borrow_mut();
        self.inner.layout(this, &mut state);
        state.total
    }

    /// Reads the scroller that tracks `handle` on every render, until
    /// [`disconnect`](Self::disconnect) (`connect`).
    pub fn connect(&self, handle: ScrollHandle) {
        let mut state = self.inner.state.borrow_mut();
        state.scroller = Some(handle);
        self.inner.read(&mut state);
    }

    /// Stops reading the scroller.
    pub fn disconnect(&self) {
        self.inner.state.borrow_mut().scroller = None;
    }

    /// Measures a rendered row each time it lays out (`measure`): put it on
    /// the row's element. Fixed-size rows can skip it. A row above the top
    /// that changes size moves the scroller with it, so what's in view stays
    /// where it is.
    pub fn measure<E: ParentElement + Styled>(&self, index: usize, el: E) -> E {
        el.relative().child(
            RowProbe {
                index,
                horizontal: self.inner.options.horizontal,
                measured: self.inner.measured.clone(),
                virtualizer: Rc::downgrade(&self.inner) as Weak<dyn Flush>,
                style: StyleRefinement::default(),
            }
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    /// Scrolls so a row shows, measuring as it goes (`scrollToIndex`). gpui
    /// scrolls at once; there's no smooth scrolling to ask for. It renders
    /// the owner again.
    pub fn scroll_to_index(&self, this: &C, index: usize, align: VirtualAlign, cx: &mut App) {
        let inner = &self.inner;
        let margin = inner.options.scroll_margin;
        {
            let mut state = inner.state.borrow_mut();
            inner.layout(this, &mut state);
            let Some(scroller) = state.scroller.clone() else {
                return;
            };
            if index >= state.starts.len() {
                return;
            }
            inner.read(&mut state);
            let start = state.starts[index] + margin;
            let size = inner.size_of(this, &state, index);
            let offset = state.offset;
            let viewport = state.viewport;
            // The rows show below `scroll_margin`, like under a sticky header.
            let to = match align {
                VirtualAlign::Start => start - margin,
                VirtualAlign::End => start + size - viewport,
                VirtualAlign::Center => start + size / 2. - (viewport + margin) / 2.,
                VirtualAlign::Auto if start < offset + margin => start - margin,
                VirtualAlign::Auto if start + size > offset + viewport => start + size - viewport,
                VirtualAlign::Auto => offset,
            };
            if to == offset {
                return;
            }
            let to = to.max(px(0.));
            let current = scroller.offset();
            if inner.options.horizontal {
                scroller.set_offset(point(-to, current.y));
            } else {
                scroller.set_offset(point(current.x, -to));
            }
            inner.read(&mut state);
        }
        cx.notify(inner.entity_id);
    }

    /// Reads the list again, like after its rows change without a scroll
    /// (`refresh`). It renders the owner again.
    pub fn refresh(&self, cx: &mut App) {
        {
            let mut state = self.inner.state.borrow_mut();
            state.dirty = true;
            self.inner.read(&mut state);
        }
        cx.notify(self.inner.entity_id);
    }

    /// Forgets every measured size, like after a change that resizes every
    /// row (`reset`).
    pub fn reset(&self, cx: &mut App) {
        self.inner.state.borrow_mut().sizes.clear();
        self.refresh(cx);
    }
}

// What a row's probe calls once the frame that measured it is drawn.
trait Flush {
    fn flush(&self, cx: &mut App);
}

impl<C: 'static> Flush for VirtInner<C> {
    fn flush(&self, cx: &mut App) {
        self.measured.queued.set(false);
        let pending: Vec<(usize, Pixels)> = std::mem::take(&mut *self.measured.pending.borrow_mut());
        let Some(entity) = self.entity.upgrade() else {
            return;
        };
        let changed = {
            let this = entity.read(cx);
            let mut changed = false;
            for (index, size) in pending {
                changed |= self.record(this, index, size);
            }
            changed
        };
        if changed {
            cx.notify(self.entity_id);
        }
    }
}

// What `measure` puts in a row's element: it takes no space and draws
// nothing, and reports the row's size as it lays out.
struct RowProbe {
    index: usize,
    horizontal: bool,
    measured: Rc<Measured>,
    virtualizer: Weak<dyn Flush>,
    style: StyleRefinement,
}

impl Styled for RowProbe {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Element for RowProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.refine(&self.style);
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let size = if self.horizontal {
            bounds.size.width
        } else {
            bounds.size.height
        };
        self.measured.pending.borrow_mut().push((self.index, size));
        if !self.measured.queued.replace(true) {
            let virtualizer = self.virtualizer.clone();
            window.defer(cx, move |_window, cx| {
                if let Some(virtualizer) = virtualizer.upgrade() {
                    virtualizer.flush(cx);
                }
            });
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}
