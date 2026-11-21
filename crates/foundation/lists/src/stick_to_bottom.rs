//! Port of `foundation/lists/stick-to-bottom.ts`: keeping a growing log
//! scrolled to its end: a chat streaming a reply, a build log, a terminal.
//! While the reader is at the bottom it follows new content; once they scroll
//! up it stops, so they can read, and says how far behind they are;
//! scrolling back down, or `scroll_to_bottom`, follows again. Older content
//! loaded above keeps what's in view still.
//!
//! The original watches the scroller's size and children with observers and
//! hears its scrolls. Here the scroller's element ([`StickToBottom::scroller`])
//! looks once each frame it draws is done, at where it scrolled and how far
//! its content reaches, and while following it asks gpui to keep the end in
//! view in the very frame new content lands (`ScrollHandle::scroll_to_bottom`).
//! Its rows, measured with [`StickToBottom::row`], keep the first one in view
//! where it is while content above it changes size; that correction shows a
//! frame later, since gpui lays out before anything can read the new size.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gpui::{
    App, Bounds, Context, Element, ElementId, EntityId, GlobalElementId, InspectorElementId, InteractiveElement,
    KeyDownEvent, LayoutId, ParentElement, Pixels, Refineable, ScrollHandle, ScrollEvent, SharedString, Style,
    StyleRefinement, Styled, Window, point, px,
};
use ui_sdk_kernel::Key;

/// Where a log stands (`StickToBottomState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StickToBottomState {
    /// Whether new content scrolls into view.
    pub following: bool,
    /// Whether the end is in view now.
    pub at_bottom: bool,
    /// Whether content arrived below while not following. Clears at the
    /// bottom.
    pub unseen: bool,
}

/// How [`create_stick_to_bottom`] follows (`StickToBottomOptions`).
#[derive(Clone)]
pub struct StickToBottomOptions {
    /// How close to the end still counts as at the bottom. Defaults to 24px.
    pub threshold: Pixels,
    /// Whether it follows from the start. Defaults to true.
    pub initial: bool,
    /// Runs when [`StickToBottom::get`] changes.
    pub on_change: Option<Rc<dyn Fn(&StickToBottomState, &mut App)>>,
}

impl Default for StickToBottomOptions {
    fn default() -> Self {
        Self {
            threshold: px(24.),
            initial: true,
            on_change: None,
        }
    }
}

impl StickToBottomOptions {
    pub fn threshold(mut self, threshold: impl Into<Pixels>) -> Self {
        self.threshold = threshold.into();
        self
    }

    pub fn initial(mut self, initial: bool) -> Self {
        self.initial = initial;
        self
    }

    pub fn on_change(mut self, on_change: impl Fn(&StickToBottomState, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(on_change));
        self
    }
}

struct Data {
    state: StickToBottomState,
    scroller: Option<ScrollHandle>,
    // The scroll position this module set last, so its own scrolls don't
    // read as the reader's.
    ours: Option<Pixels>,
    last_top: Pixels,
    last_height: Pixels,
    last_viewport: Pixels,
    // The distance from the end saved by `prepend`, kept until content grows.
    anchor: Option<Pixels>,
    // The first row in view while not following, and where it sits in the
    // content.
    pinned: Option<(SharedString, Pixels)>,
    // The rows as the last frame laid them out, in order, and this frame's.
    rows: Vec<(SharedString, Bounds<Pixels>)>,
    rows_next: Vec<(SharedString, Bounds<Pixels>)>,
    checking: bool,
}

struct Inner {
    options: StickToBottomOptions,
    entity: EntityId,
    data: RefCell<Data>,
}

/// Follows a scroller's end as its content grows (`createStickToBottom`'s
/// return). Cloning it gives another handle to the same one.
#[derive(Clone)]
pub struct StickToBottom {
    inner: Rc<Inner>,
}

/// Follows a scroller's end as its content grows (`createStickToBottom`),
/// for the component or view of `cx`, which it renders again whenever
/// [`StickToBottom::get`] changes (`subscribe`).
pub fn create_stick_to_bottom<C: 'static>(options: StickToBottomOptions, cx: &Context<C>) -> StickToBottom {
    let following = options.initial;
    StickToBottom {
        inner: Rc::new(Inner {
            options,
            entity: cx.entity_id(),
            data: RefCell::new(Data {
                state: StickToBottomState {
                    following,
                    at_bottom: true,
                    unseen: false,
                },
                scroller: None,
                ours: None,
                last_top: px(0.),
                last_height: px(0.),
                last_viewport: px(0.),
                anchor: None,
                pinned: None,
                rows: Vec::new(),
                rows_next: Vec::new(),
                checking: false,
            }),
        }),
    }
}

// Where a scroller stands: how far it scrolled, how far it can, and its
// size along the list.
#[derive(Clone, Copy)]
struct Measure {
    top: Pixels,
    max: Pixels,
    viewport: Pixels,
    // Its top edge in the window.
    origin: Pixels,
}

impl Measure {
    fn of(handle: &ScrollHandle) -> Self {
        let bounds = handle.bounds();
        Self {
            top: -handle.offset().y,
            max: handle.max_offset().y,
            viewport: bounds.size.height,
            origin: bounds.top(),
        }
    }

    fn height(&self) -> Pixels {
        self.max + self.viewport
    }

    fn distance(&self) -> Pixels {
        self.max - self.top
    }
}

fn scroll_to(handle: &ScrollHandle, top: Pixels) {
    let offset = handle.offset();
    handle.set_offset(point(offset.x, -top.max(px(0.))));
}

impl StickToBottom {
    /// Where the log stands now (`get`).
    pub fn get(&self) -> StickToBottomState {
        self.inner.data.borrow().state
    }

    fn set(&self, data: &mut Data, next: impl FnOnce(&mut StickToBottomState), changed: &mut bool) {
        let mut merged = data.state;
        next(&mut merged);
        if merged.at_bottom {
            merged.unseen = false;
        }
        if merged == data.state {
            return;
        }
        data.state = merged;
        *changed = true;
    }

    fn report(&self, changed: bool, cx: &mut App) {
        if !changed {
            return;
        }
        let state = self.get();
        if let Some(on_change) = self.inner.options.on_change.clone() {
            on_change(&state, cx);
        }
        cx.notify(self.inner.entity);
    }

    // The first row in view while not following, and where it sits in the
    // content, so a size change above it can keep it there.
    fn pin(&self, data: &mut Data, measure: &Measure) {
        data.pinned = None;
        if data.state.following {
            return;
        }
        let rows = &data.rows;
        let top = measure.origin;
        let bottom = measure.origin + measure.viewport;
        let mut low = 0isize;
        let mut high = rows.len() as isize - 1;
        let mut found: Option<usize> = None;
        while low <= high {
            let middle = ((low + high) / 2) as usize;
            let rect = rows[middle].1;
            if rect.bottom() <= top {
                low = middle as isize + 1;
            } else {
                found = Some(middle);
                high = middle as isize - 1;
            }
        }
        if let Some(index) = found
            && rows[index].1.top() < bottom
        {
            let (key, rect) = &rows[index];
            data.pinned = Some((key.clone(), rect.top() - top + measure.top));
        }
    }

    fn to_end(&self, data: &mut Data, measure: &Measure) {
        data.ours = Some(measure.max.max(px(0.)).round());
        if let Some(handle) = &data.scroller {
            handle.scroll_to_bottom();
            scroll_to(handle, measure.max);
        }
    }

    // The scroller or its content changed size.
    fn resized(&self, data: &mut Data, measure: &mut Measure, changed: &mut bool) {
        let Some(handle) = data.scroller.clone() else {
            return;
        };
        let mut unseen = false;
        if let Some(anchor) = data.anchor
            && measure.height() != data.last_height
        {
            // Older content went in above: keep the same distance from the end.
            let ours = (measure.max - anchor).round();
            data.ours = Some(ours);
            scroll_to(&handle, ours);
            data.anchor = None;
        } else if data.state.following {
            self.to_end(data, measure);
        } else {
            // gpui keeps no scroll anchoring of its own.
            let moved = data
                .pinned
                .as_ref()
                .and_then(|(key, top)| {
                    data.rows
                        .iter()
                        .find(|(row, _)| row == key)
                        .map(|(_, rect)| rect.top() - measure.origin + measure.top - *top)
                })
                .unwrap_or(px(0.));
            if moved.abs() >= px(1.) {
                // Content that shrank below the scroll position pulled it
                // back already.
                let clamped = measure.top < data.last_top && measure.distance() < px(1.);
                let ours = ((if clamped { data.last_top } else { measure.top }) + moved).round();
                data.ours = Some(ours);
                scroll_to(&handle, ours);
            }
            unseen = measure.height() - moved > data.last_height;
        }
        *measure = Measure {
            top: -handle.offset().y,
            ..*measure
        };
        data.last_height = measure.height();
        data.last_viewport = measure.viewport;
        data.last_top = measure.top;
        self.pin(data, measure);
        let threshold = self.inner.options.threshold;
        // One change, so content that pushes the end out of view stays unseen.
        let at_bottom = measure.distance() <= threshold;
        self.set(
            data,
            |state| {
                state.at_bottom = at_bottom;
                if unseen {
                    state.unseen = true;
                }
            },
            changed,
        );
    }

    // The scroller scrolled.
    fn scrolled(&self, data: &mut Data, measure: &Measure, changed: &mut bool) {
        let top = measure.top;
        let at_bottom = measure.distance() <= self.inner.options.threshold;
        let mine = data.ours.is_some_and(|ours| (top - ours).abs() <= px(1.));
        if mine {
            // Our own scroll.
            if at_bottom {
                data.ours = None;
            }
        } else if top < data.last_top - px(1.) && !at_bottom {
            self.set(data, |state| state.following = false, changed);
        } else if at_bottom {
            self.set(data, |state| state.following = true, changed);
        }
        data.last_top = top;
        self.set(data, |state| state.at_bottom = at_bottom, changed);
        self.pin(data, measure);
    }

    // Once a frame the scroller drew is done: what scrolled and what grew.
    fn check(&self, window: &mut Window, cx: &mut App) {
        let mut changed = false;
        let mut moved = false;
        {
            let mut data = self.inner.data.borrow_mut();
            data.checking = false;
            data.rows = std::mem::take(&mut data.rows_next);
            let Some(handle) = data.scroller.clone() else {
                return;
            };
            let mut measure = Measure::of(&handle);
            let before = -handle.offset().y;
            if (measure.top - data.last_top).abs() > px(0.5) {
                self.scrolled(&mut data, &measure, &mut changed);
            }
            if measure.height() != data.last_height || measure.viewport != data.last_viewport {
                self.resized(&mut data, &mut measure, &mut changed);
            }
            moved |= -handle.offset().y != before;
        }
        if moved {
            window.refresh();
        }
        self.report(changed, cx);
    }

    /// Follows the scroller that tracks `handle`'s end (`connect`), until
    /// [`disconnect`](Self::disconnect). Put [`scroller`](Self::scroller) on
    /// its element.
    pub fn connect(&self, handle: ScrollHandle) {
        let mut data = self.inner.data.borrow_mut();
        data.scroller = Some(handle.clone());
        let measure = Measure::of(&handle);
        data.last_height = measure.height();
        data.last_viewport = measure.viewport;
        if data.state.following {
            self.to_end(&mut data, &measure);
        }
        data.last_top = -handle.offset().y;
    }

    /// Stops following the scroller.
    pub fn disconnect(&self) {
        let mut data = self.inner.data.borrow_mut();
        data.scroller = None;
        data.pinned = None;
    }

    /// The scroller's element, the one that tracks the connected handle: it
    /// keeps the end in view while following, stops following at once on a
    /// wheel or a key going up, before any frame lands, and looks at what
    /// scrolled and grew once each frame is drawn.
    pub fn scroller<E: ParentElement + Styled + InteractiveElement>(&self, el: E) -> E {
        {
            let data = self.inner.data.borrow();
            if data.state.following
                && let Some(handle) = &data.scroller
            {
                handle.scroll_to_bottom();
            }
        }
        let wheel = self.clone();
        let keys = self.clone();
        el.on_scroll(move |event: &ScrollEvent, _window, cx| {
            let up = event.delta.pixel_delta(px(16.)).y > px(0.);
            wheel.intent(up, cx);
        })
        .on_key_down(move |event: &KeyDownEvent, _window, cx| {
            let up = matches!(Key::of(&event.keystroke), Key::ArrowUp | Key::PageUp | Key::Home);
            keys.intent(up, cx);
        })
        .child(
            FrameProbe {
                stick: Rc::downgrade(&self.inner),
                style: StyleRefinement::default(),
            }
            .absolute()
            .size_0(),
        )
    }

    // A wheel or key going up stops following at once.
    fn intent(&self, up: bool, cx: &mut App) {
        let mut changed = false;
        {
            let mut data = self.inner.data.borrow_mut();
            let scrollable = data.scroller.as_ref().is_some_and(|handle| handle.max_offset().y > px(0.));
            if !up || !scrollable {
                return;
            }
            data.ours = None;
            self.set(&mut data, |state| state.following = false, &mut changed);
        }
        self.report(changed, cx);
    }

    /// Measures a row of the log, by a key of its own, so the first row in
    /// view stays where it is when content above it changes size. Rows
    /// report in the order they render.
    pub fn row<E: ParentElement + Styled>(&self, key: impl Into<SharedString>, el: E) -> E {
        el.relative().child(
            RowProbe {
                key: key.into(),
                stick: Rc::downgrade(&self.inner),
                style: StyleRefinement::default(),
            }
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    /// Scrolls to the end and follows again (`scrollToBottom`). gpui has no
    /// smooth scrolling, so it goes at once.
    pub fn scroll_to_bottom(&self, cx: &mut App) {
        let mut changed = false;
        {
            let mut data = self.inner.data.borrow_mut();
            self.set(&mut data, |state| state.following = true, &mut changed);
            if let Some(handle) = data.scroller.clone() {
                let measure = Measure::of(&handle);
                self.to_end(&mut data, &measure);
            }
        }
        self.report(changed, cx);
        cx.notify(self.inner.entity);
    }

    /// Stops following, as if the reader scrolled up (`release`).
    pub fn release(&self, cx: &mut App) {
        let mut changed = false;
        {
            let mut data = self.inner.data.borrow_mut();
            self.set(&mut data, |state| state.following = false, &mut changed);
            if let Some(handle) = data.scroller.clone() {
                let measure = Measure::of(&handle);
                self.pin(&mut data, &measure);
            }
        }
        self.report(changed, cx);
    }

    /// Call right before adding content above, like older messages: the next
    /// growth keeps what's in view where it is instead of pushing it down
    /// (`prepend`).
    pub fn prepend(&self) {
        let mut data = self.inner.data.borrow_mut();
        if let Some(handle) = &data.scroller {
            let measure = Measure::of(handle);
            data.anchor = Some(measure.distance());
        }
    }
}

// What `scroller` puts in the scroller's element: it takes no space and
// draws nothing, and looks once each frame it's drawn in is done.
struct FrameProbe {
    stick: Weak<Inner>,
    style: StyleRefinement,
}

impl Styled for FrameProbe {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Element for FrameProbe {
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
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let Some(inner) = self.stick.upgrade() else {
            return;
        };
        let queued = std::mem::replace(&mut inner.data.borrow_mut().checking, true);
        if queued {
            return;
        }
        let stick = StickToBottom { inner };
        window.defer(cx, move |window, cx| stick.check(window, cx));
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

// What `row` puts in a row's element: it reports where the row laid out.
struct RowProbe {
    key: SharedString,
    stick: Weak<Inner>,
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
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some(inner) = self.stick.upgrade()
            && let Ok(mut data) = inner.data.try_borrow_mut()
        {
            data.rows_next.push((self.key.clone(), bounds));
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
