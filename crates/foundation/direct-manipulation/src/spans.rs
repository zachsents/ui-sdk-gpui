//! Port of `foundation/direct-manipulation/spans.ts`: ranges laid on
//! tracks: events in a week calendar's days, bars in a Gantt chart's rows,
//! clips on a video editor's layers, bookings on a room's timeline. A drag
//! on an empty stretch of a track draws a range, a drag on a range moves it,
//! along its track and to other tracks, and a drag on one of its edges
//! resizes it. Every value snaps to a step. Like `create_transfer`, the
//! ranges live in a draft while a pointer drags, which renders as it
//! changes, and only the drop commits, so a drag is one change and one undo.
//! From the keyboard each arrow commits at once, as a slider's does.
//!
//! Tracks and spans join as they render ([`Spans::bind_track`],
//! [`Spans::bind_span`]), the original's slot `bind`, which records where
//! they lay out. An edge is a part inside its span that presses with its
//! edge ([`Spans::bind_edge`]), the original's `data-span-edge`. A press on a
//! span marks itself, so the track around it leaves it alone, as the
//! original asks `closest("[data-span-id]")`. The draft lives in the spans,
//! the original's `draft` state key, and follows a value the owner changed
//! mid-drag the next time it's read, which rendering and every change do.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gpui::{
    App, Axis, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Pixels, Point, SharedString, Styled, Task, WeakEntity, Window, relative,
};
use ui_sdk_drag_and_drop::{
    AutoScroll, AutoScrollOptions, BindDragOptions, DRAG_THRESHOLD, DragSession, MessageFn,
    PointerEvent, ReadFn, create_auto_scroll, on_pointer_down,
};
use ui_sdk_focus::keep_focus;
use ui_sdk_kernel::{ChangeOptions, Key, Reason};
use ui_sdk_number::{StepRange, clamp, on_grid, on_grid_with};
use ui_sdk_platform::{announce, is_rtl};

use crate::js;
use crate::target::{Bound, Mark, mark_press, marked_press};

/// A range on a track, from `start` to `end` in the track's own units, like
/// minutes from midnight (`Span`).
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub id: SharedString,
    pub track: SharedString,
    pub start: f64,
    pub end: f64,
}

impl Span {
    pub fn new(
        id: impl Into<SharedString>,
        track: impl Into<SharedString>,
        start: f64,
        end: f64,
    ) -> Self {
        Self {
            id: id.into(),
            track: track.into(),
            start,
            end,
        }
    }
}

/// What [`create_spans`] edits: a [`Span`], or a type that holds one with
/// what else it needs, like a booking with its title (the original's
/// `S extends Span`).
pub trait SpanLike: Clone + PartialEq + 'static {
    fn span(&self) -> &Span;
    fn span_mut(&mut self) -> &mut Span;
}

impl SpanLike for Span {
    fn span(&self) -> &Span {
        self
    }

    fn span_mut(&mut self) -> &mut Span {
        self
    }
}

/// What changed a span: drawn, moved, resized or deleted (`SpanAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpanAction {
    Draw,
    Move,
    Resize,
    Delete,
}

impl SpanAction {
    /// The original's name for it, like `"draw"`.
    pub fn as_str(self) -> &'static str {
        match self {
            SpanAction::Draw => "draw",
            SpanAction::Move => "move",
            SpanAction::Resize => "resize",
            SpanAction::Delete => "delete",
        }
    }
}

/// Why the spans changed: a pointer drag (`Drag`), an arrow key that moved
/// or resized a span (`MoveKey`), Delete or Backspace (`RemovePress`), or a
/// call from code (`Imperative`).
pub type SpansReason = Reason;

/// The spans as they show while a pointer drags (`SpansDraft`).
#[derive(Clone, Debug, PartialEq)]
pub struct SpansDraft<S = Span> {
    /// The span being drawn, moved or resized.
    pub id: SharedString,
    pub action: SpanAction,
    /// Every span as it shows now.
    pub spans: Vec<S>,
}

/// Which change a message is for (a message's `type`): a span's action, or
/// `Cancel` when a drag put the span back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpansMessageType {
    Draw,
    Move,
    Resize,
    Delete,
    Cancel,
}

impl SpansMessageType {
    /// The original's name for it, like `"cancel"`.
    pub fn as_str(self) -> &'static str {
        match self {
            SpansMessageType::Draw => "draw",
            SpansMessageType::Move => "move",
            SpansMessageType::Resize => "resize",
            SpansMessageType::Delete => "delete",
            SpansMessageType::Cancel => "cancel",
        }
    }
}

impl From<SpanAction> for SpansMessageType {
    fn from(action: SpanAction) -> Self {
        match action {
            SpanAction::Draw => SpansMessageType::Draw,
            SpanAction::Move => SpansMessageType::Move,
            SpanAction::Resize => SpansMessageType::Resize,
            SpanAction::Delete => SpansMessageType::Delete,
        }
    }
}

/// What happened, for the message screen readers hear (`SpansMessage`).
#[derive(Clone, Debug, PartialEq)]
pub struct SpansMessage<S = Span> {
    pub r#type: SpansMessageType,
    pub span: S,
    /// What the span is, from `label`.
    pub label: SharedString,
}

/// The English messages [`create_spans`] announces unless you pass
/// `message` (`spansMessage`).
pub fn spans_message<S>(message: &SpansMessage<S>) -> String {
    let label = &message.label;
    match message.r#type {
        SpansMessageType::Draw => format!("Added {label}."),
        SpansMessageType::Delete => format!("Deleted {label}."),
        SpansMessageType::Cancel => format!("Put back {label}."),
        SpansMessageType::Move | SpansMessageType::Resize => format!("{label}."),
    }
}

/// Where a span sits beside the spans it overlaps on its track: lane `lane`
/// of `lanes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpanLane {
    pub lane: usize,
    pub lanes: usize,
}

/// Where each span sits beside the spans it overlaps on its track, by id,
/// so overlapping events share their day's width (`stackSpans`). Spans that
/// only touch don't overlap.
pub fn stack_spans<S: SpanLike>(spans: &[S]) -> HashMap<SharedString, SpanLane> {
    let mut result: HashMap<SharedString, SpanLane> = HashMap::new();
    let mut by_track: Vec<(SharedString, Vec<&Span>)> = Vec::new();
    for span in spans {
        let span = span.span();
        match by_track.iter_mut().find(|(track, _)| *track == span.track) {
            Some((_, track)) => track.push(span),
            None => by_track.push((span.track.clone(), vec![span])),
        }
    }
    for (_, mut track) in by_track {
        track.sort_by(|a, b| {
            // `a.start - b.start || b.end - a.end`: NaN and 0 fall through.
            let by_start = a.start - b.start;
            let order = if by_start != 0.0 && !by_start.is_nan() {
                by_start
            } else {
                b.end - a.end
            };
            order.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
        });
        // Spans that overlap one another, directly or through others, share a lane count.
        let mut group: Vec<&Span> = Vec::new();
        let mut lanes: Vec<f64> = Vec::new();
        let mut group_end = f64::NEG_INFINITY;
        let close = |group: &mut Vec<&Span>,
                     lanes: &mut Vec<f64>,
                     result: &mut HashMap<SharedString, SpanLane>| {
            for span in group.iter() {
                if let Some(lane) = result.get_mut(&span.id) {
                    lane.lanes = lanes.len();
                }
            }
            group.clear();
            lanes.clear();
        };
        for span in track {
            if span.start >= group_end {
                close(&mut group, &mut lanes, &mut result);
            }
            let lane = match lanes.iter().position(|end| *end <= span.start) {
                Some(lane) => {
                    lanes[lane] = span.end;
                    lane
                }
                None => {
                    lanes.push(span.end);
                    lanes.len() - 1
                }
            };
            result.insert(span.id.clone(), SpanLane { lane, lanes: 0 });
            group.push(span);
            group_end = js::max(
                if group_end == f64::NEG_INFINITY {
                    span.end
                } else {
                    group_end
                },
                span.end,
            );
        }
        close(&mut group, &mut lanes, &mut result);
    }
    result
}

/// Whether `span` overlaps no other span on its track (`isSpanFree`).
pub fn is_span_free<S: SpanLike>(spans: &[S], span: &S) -> bool {
    let span = span.span();
    spans.iter().all(|other| {
        let other = other.span();
        other.id == span.id
            || other.track != span.track
            || other.end <= span.start
            || other.start >= span.end
    })
}

/// The edge of a span an element inside it resizes from, the original's
/// `data-span-edge`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpanEdge {
    Start,
    End,
}

/// Which span a change is for and what it did, which the original passes in
/// `set`'s details beside the reason and the event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanChange {
    pub id: SharedString,
    pub action: SpanAction,
}

/// Asks the component for new spans, with why and which span changed, and
/// returns whether they went through. It runs inside the component's
/// update, with the component.
pub type SetSpansFn<T, S> =
    Rc<dyn Fn(&mut T, Vec<S>, ChangeOptions, SpanChange, &mut Window, &mut Context<T>) -> bool>;

/// Makes the span a press on a track starts to draw.
pub type CreateSpanFn<T, S> = Rc<dyn Fn(&T, Span) -> Option<S>>;

/// Reads something about a span from the component.
pub type SpanFn<T, S, V> = Rc<dyn Fn(&T, &S) -> V>;

/// How [`create_spans`] reads, renders and changes the spans
/// (`SpansOptions`). `T` is the component, `S` its span.
pub struct SpansOptions<T: 'static, S = Span> {
    /// The spans, the component's value (`key`).
    pub value: ReadFn<T, Vec<S>>,
    /// Asks for the new spans and returns whether they went through. Pass
    /// the component's change: `|this, value, details, _span, window, cx|
    /// this.spans.change(&this.instance, value, details, window, cx)`.
    pub set: SetSpansFn<T, S>,
    /// Where tracks start, in their units. Defaults to 0.
    pub min: Option<ReadFn<T, f64>>,
    /// Where tracks end. Defaults to 100.
    pub max: Option<ReadFn<T, f64>>,
    /// What every start and end snaps to, and what arrows move by. Defaults
    /// to 1.
    pub step: Option<ReadFn<T, f64>>,
    /// The shortest a span can be. Defaults to the step.
    pub min_size: Option<ReadFn<T, f64>>,
    /// The longest a span can be.
    pub max_size: Option<ReadFn<T, f64>>,
    /// The direction tracks run: down (`Vertical`), like a calendar's days,
    /// or across (`Horizontal`), like a Gantt chart's rows. Defaults to
    /// `Vertical`, the original's `"y"`.
    pub axis: Axis,
    /// The tracks in the order the arrows across them go through. Defaults
    /// to the bound tracks in page order.
    pub tracks: Option<ReadFn<T, Vec<SharedString>>>,
    /// Whether spans on one track can overlap. When they can't, a drag stops
    /// where it would. Defaults to true.
    pub overlap: bool,
    /// Makes the span a press on a track starts to draw, with a new `id`
    /// and anything else it needs, like a title, or returns `None` to not
    /// draw. Runs on every press on an empty stretch of track. Without it,
    /// nothing is drawn.
    pub create: Option<CreateSpanFn<T, S>>,
    /// How long a span a click on an empty track makes. Without it, or when
    /// it returns `None`, a click makes none.
    pub size: Option<ReadFn<T, Option<f64>>>,
    /// Whether a span can't change.
    pub disabled: Option<SpanFn<T, S, bool>>,
    /// What screen readers call a span, like "Standup, Tuesday 9:00 to
    /// 9:30". Defaults to its track and range.
    pub label: Option<SpanFn<T, S, SharedString>>,
    /// The text screen readers hear for each change. Defaults to English
    /// ([`spans_message`]).
    pub message: Option<MessageFn<SpansMessage<S>>>,
    /// How a drag near a scroller's edge scrolls it, with the scrollers it
    /// may scroll, or `None` not to (`createAutoScroll: false`). Its
    /// `on_scroll` is the spans' own.
    pub auto_scroll: Option<AutoScrollOptions>,
}

impl<T: 'static, S> Clone for SpansOptions<T, S> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            set: self.set.clone(),
            min: self.min.clone(),
            max: self.max.clone(),
            step: self.step.clone(),
            min_size: self.min_size.clone(),
            max_size: self.max_size.clone(),
            axis: self.axis,
            tracks: self.tracks.clone(),
            overlap: self.overlap,
            create: self.create.clone(),
            size: self.size.clone(),
            disabled: self.disabled.clone(),
            label: self.label.clone(),
            message: self.message.clone(),
            auto_scroll: self.auto_scroll.clone(),
        }
    }
}

impl<T: 'static, S: SpanLike> SpansOptions<T, S> {
    /// Options with the spans and how to change them.
    pub fn new(
        value: impl Fn(&T) -> Vec<S> + 'static,
        set: impl Fn(&mut T, Vec<S>, ChangeOptions, SpanChange, &mut Window, &mut Context<T>) -> bool
        + 'static,
    ) -> Self {
        Self {
            value: Rc::new(value),
            set: Rc::new(set),
            min: None,
            max: None,
            step: None,
            min_size: None,
            max_size: None,
            axis: Axis::Vertical,
            tracks: None,
            overlap: true,
            create: None,
            size: None,
            disabled: None,
            label: None,
            message: None,
            auto_scroll: Some(AutoScrollOptions::default()),
        }
    }

    pub fn min(mut self, min: impl Fn(&T) -> f64 + 'static) -> Self {
        self.min = Some(Rc::new(min));
        self
    }

    pub fn max(mut self, max: impl Fn(&T) -> f64 + 'static) -> Self {
        self.max = Some(Rc::new(max));
        self
    }

    pub fn step(mut self, step: impl Fn(&T) -> f64 + 'static) -> Self {
        self.step = Some(Rc::new(step));
        self
    }

    pub fn min_size(mut self, min_size: impl Fn(&T) -> f64 + 'static) -> Self {
        self.min_size = Some(Rc::new(min_size));
        self
    }

    pub fn max_size(mut self, max_size: impl Fn(&T) -> f64 + 'static) -> Self {
        self.max_size = Some(Rc::new(max_size));
        self
    }

    pub fn axis(mut self, axis: Axis) -> Self {
        self.axis = axis;
        self
    }

    pub fn tracks(mut self, tracks: impl Fn(&T) -> Vec<SharedString> + 'static) -> Self {
        self.tracks = Some(Rc::new(tracks));
        self
    }

    pub fn overlap(mut self, overlap: bool) -> Self {
        self.overlap = overlap;
        self
    }

    pub fn create(mut self, create: impl Fn(&T, Span) -> Option<S> + 'static) -> Self {
        self.create = Some(Rc::new(create));
        self
    }

    pub fn size(mut self, size: impl Fn(&T) -> Option<f64> + 'static) -> Self {
        self.size = Some(Rc::new(size));
        self
    }

    pub fn disabled(mut self, disabled: impl Fn(&T, &S) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(disabled));
        self
    }

    pub fn label(mut self, label: impl Fn(&T, &S) -> SharedString + 'static) -> Self {
        self.label = Some(Rc::new(label));
        self
    }

    pub fn message(mut self, message: impl Fn(&SpansMessage<S>) -> String + 'static) -> Self {
        self.message = Some(Rc::new(message));
        self
    }

    pub fn auto_scroll(mut self, auto_scroll: Option<AutoScrollOptions>) -> Self {
        self.auto_scroll = auto_scroll;
        self
    }
}

/// Changes the rest of a span, in a [`SpanPatch`].
pub type SpanEditFn<S> = Rc<dyn Fn(&mut S)>;

/// A change to a span from code ([`Spans::update`]): a new track or range,
/// and anything else of yours on it (`Partial<Omit<S, "id">>`).
pub struct SpanPatch<S> {
    pub track: Option<SharedString>,
    pub start: Option<f64>,
    pub end: Option<f64>,
    /// Changes the rest of the span, like its title.
    pub with: Option<SpanEditFn<S>>,
}

impl<S> Default for SpanPatch<S> {
    fn default() -> Self {
        Self {
            track: None,
            start: None,
            end: None,
            with: None,
        }
    }
}

impl<S> Clone for SpanPatch<S> {
    fn clone(&self) -> Self {
        Self {
            track: self.track.clone(),
            start: self.start,
            end: self.end,
            with: self.with.clone(),
        }
    }
}

impl<S> SpanPatch<S> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn track(mut self, track: impl Into<SharedString>) -> Self {
        self.track = Some(track.into());
        self
    }

    pub fn start(mut self, start: f64) -> Self {
        self.start = Some(start);
        self
    }

    pub fn end(mut self, end: f64) -> Self {
        self.end = Some(end);
        self
    }

    pub fn with(mut self, with: impl Fn(&mut S) + 'static) -> Self {
        self.with = Some(Rc::new(with));
        self
    }
}

/// A span's place in its track (`spanStyle`): its start and length along
/// the axis, from 0 to 1 of the track (the original's `--span-start` and
/// `--span-size`), and its lane across it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanStyle {
    pub start: f64,
    pub size: f64,
    pub lane: Option<SpanLane>,
    pub axis: Axis,
}

impl SpanStyle {
    /// Places the span absolutely in its track: along the axis from its
    /// start, as long as it lasts, and across it in its lane, from the start
    /// in the reading direction (the right side when `rtl`).
    pub fn apply<E: Styled>(self, el: E, rtl: bool) -> E {
        let fraction = |n: f64| relative(js::style_fraction(n));
        let inline = |el: E, n: f64| {
            if rtl {
                el.right(fraction(n))
            } else {
                el.left(fraction(n))
            }
        };
        let lane = self.lane.map(|lane| {
            (
                lane.lane as f64 / lane.lanes as f64,
                1.0 / lane.lanes as f64,
            )
        });
        let el = el.absolute();
        match self.axis {
            Axis::Vertical => {
                let el = el.top(fraction(self.start)).h(fraction(self.size));
                match lane {
                    Some((across, breadth)) => inline(el, across).w(fraction(breadth)),
                    None => el,
                }
            }
            Axis::Horizontal => {
                let el = inline(el, self.start).w(fraction(self.size));
                match lane {
                    Some((across, breadth)) => el.top(fraction(across)).h(fraction(breadth)),
                    None => el,
                }
            }
        }
    }
}

/// A span's attributes (`spanAttrs`): its id, the original's
/// `data-span-id`, and what a pointer is doing to it, `data-dragging`,
/// `data-resizing` or `data-drawing`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanAttrs {
    pub id: SharedString,
    pub dragging: bool,
    pub resizing: bool,
    pub drawing: bool,
}

// Which edge of a span holds still as it's fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keep {
    Start,
    End,
    Size,
}

// What tracks and spans bind as.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SpansTarget {
    Track(SharedString),
    Span(SharedString),
}

// The spans with `span` in place of the one with its id, or added.
fn replaced<S: SpanLike>(spans: &[S], span: S) -> Vec<S> {
    if spans.iter().any(|other| other.span().id == span.span().id) {
        spans
            .iter()
            .map(|other| {
                if other.span().id == span.span().id {
                    span.clone()
                } else {
                    other.clone()
                }
            })
            .collect()
    } else {
        let mut spans = spans.to_vec();
        spans.push(span);
        spans
    }
}

struct SpansState<S> {
    draft: Option<SpansDraft<S>>,
    // Each draft set, so a settle clears only the draft it was for.
    generation: u64,
    held: bool,
    moved: bool,
    // The value the draft last followed.
    seen: Option<Vec<S>>,
    // A dropped draft waiting for the value it committed: the value from
    // before the commit, and the draft's generation.
    settling: Option<(Vec<S>, u64)>,
    _settle: Option<Task<()>>,
}

type NextFn<T, S> = Rc<dyn Fn(&Spans<T, S>, &T, Point<Pixels>, &App) -> Option<S>>;
type ClickFn<T, S> = Rc<dyn Fn(&Spans<T, S>, &mut T, &mut Window, &mut Context<T>)>;

// A pointer drag in progress (`drag`'s closure).
struct SpanDrag<T: 'static, S: SpanLike> {
    id: SharedString,
    action: SpanAction,
    before: Vec<S>,
    dragging: bool,
    last: Option<PointerEvent>,
    point: Point<Pixels>,
    scroller: Option<AutoScroll>,
    next: NextFn<T, S>,
    click: Option<ClickFn<T, S>>,
}

type DragRef<T, S> = Rc<RefCell<SpanDrag<T, S>>>;

struct SpansInner<T: 'static, S> {
    options: RefCell<SpansOptions<T, S>>,
    entity: WeakEntity<T>,
    drags: DragSession<T>,
    bound: Bound<SpansTarget>,
    state: RefCell<SpansState<S>>,
}

/// Lets users draw, move and resize spans on tracks (`createSpans`'s
/// return, `Spans`). Cloning it gives another handle to the same spans.
pub struct Spans<T: 'static, S = Span> {
    inner: Rc<SpansInner<T, S>>,
}

impl<T: 'static, S> Clone for Spans<T, S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users draw, move and resize spans on tracks, by dragging and with
/// the keyboard (`createSpans`). Make it in the component's constructor,
/// keep it in a field, render each track's spans from
/// [`spans`](Spans::spans), render [`listener`](Spans::listener) on every
/// frame, and wire the parts:
///
/// ```ignore
/// // A track runs the whole range, from `min` to `max`.
/// let track = spans.bind_track(day.clone(), Part::new(&instance, "track", Some(day.as_ref()), ()), cx);
/// // A span sits in it at `span_style`, and arrows move it.
/// let style = spans.span_style(this, &event, lanes.get(&event.span().id).copied());
/// let span = spans.bind_span(id.clone(), Some(&handle), style.apply(part, is_rtl(cx)), cx)
///     .on_keys(&["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Delete", "Backspace"], move |event, window, cx| {
///         entity.update(cx, |this, cx| { this.spans.clone().keydown(this, event, &id, window, cx); }).ok();
///     });
/// // Each edge it resizes from is an element inside it.
/// let end = spans.bind_edge(id.clone(), SpanEdge::End, div().h_1().w_full());
/// ```
pub fn create_spans<T: 'static, S: SpanLike>(
    options: SpansOptions<T, S>,
    cx: &Context<T>,
) -> Spans<T, S> {
    Spans {
        inner: Rc::new(SpansInner {
            options: RefCell::new(options),
            entity: cx.weak_entity(),
            drags: DragSession::new(cx),
            bound: Bound::new(),
            state: RefCell::new(SpansState {
                draft: None,
                generation: 0,
                held: false,
                moved: false,
                seen: None,
                settling: None,
                _settle: None,
            }),
        }),
    }
}

impl<T: 'static, S: SpanLike> Spans<T, S> {
    fn upgrade(inner: &Weak<SpansInner<T, S>>) -> Option<Self> {
        inner.upgrade().map(|inner| Self { inner })
    }

    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: SpansOptions<T, S>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn value(&self, this: &T) -> Vec<S> {
        let value = self.inner.options.borrow().value.clone();
        value(this)
    }

    fn find(&self, this: &T, id: &SharedString) -> Option<S> {
        self.value(this)
            .into_iter()
            .find(|span| span.span().id == *id)
    }

    fn min(&self, this: &T) -> f64 {
        let min = self.inner.options.borrow().min.clone();
        min.map_or(0.0, |min| min(this))
    }

    fn max(&self, this: &T) -> f64 {
        let max = self.inner.options.borrow().max.clone();
        max.map_or(100.0, |max| max(this))
    }

    fn step(&self, this: &T) -> f64 {
        let step = self.inner.options.borrow().step.clone();
        step.map_or(1.0, |step| step(this))
    }

    fn min_size(&self, this: &T) -> f64 {
        let step = self.step(this);
        let min_size = self.inner.options.borrow().min_size.clone();
        js::max(min_size.map_or(step, |min_size| min_size(this)), step)
    }

    fn axis(&self) -> Axis {
        self.inner.options.borrow().axis
    }

    fn range(&self, this: &T) -> StepRange {
        StepRange::new(self.min(this), self.max(this), self.step(this))
    }

    // A value on the step grid, rounded the way `Math.round` does.
    fn snap(&self, this: &T, n: f64) -> f64 {
        on_grid(n, self.range(this))
    }

    // A value on the step grid, rounded with `round`.
    fn snap_with(&self, this: &T, n: f64, round: fn(f64) -> f64) -> f64 {
        on_grid_with(n, self.range(this), round)
    }

    // The span's start and end held in range and size, keeping the edge
    // that didn't move.
    fn fit(&self, this: &T, span: &S, keep: Keep) -> S {
        let (lo, hi) = (self.min(this), self.max(this));
        let max_size = self.inner.options.borrow().max_size.clone();
        let longest = max_size.map_or(hi - lo, |max_size| max_size(this));
        let min_size = self.min_size(this);
        let mut next = span.clone();
        let range = next.span_mut();
        match keep {
            Keep::Size => {
                let size = range.end - range.start;
                range.start = js::min(js::max(range.start, lo), hi - size);
                range.end = range.start + size;
            }
            Keep::Start => {
                range.end = js::min(
                    js::min(hi, range.start + longest),
                    js::max(range.end, range.start + min_size),
                );
            }
            Keep::End => {
                range.start = js::max(
                    js::max(lo, range.end - longest),
                    js::min(range.start, range.end - min_size),
                );
            }
        }
        next
    }

    fn is_disabled(&self, this: &T, span: &S) -> bool {
        let disabled = self.inner.options.borrow().disabled.clone();
        disabled.is_some_and(|disabled| disabled(this, span))
    }

    fn allowed(&self, spans: &[S], span: &S) -> bool {
        self.inner.options.borrow().overlap || is_span_free(spans, span)
    }

    fn label(&self, this: &T, span: &S) -> SharedString {
        let label = self.inner.options.borrow().label.clone();
        match label {
            Some(label) => label(this, span),
            None => {
                let span = span.span();
                format!(
                    "{}, {} to {}",
                    span.track,
                    js::number(span.start),
                    js::number(span.end)
                )
                .into()
            }
        }
    }

    fn say(&self, this: &T, r#type: SpansMessageType, span: &S, window: &mut Window, cx: &mut App) {
        let message = SpansMessage {
            r#type,
            span: span.clone(),
            label: self.label(this, span),
        };
        let message_fn = self.inner.options.borrow().message.clone();
        let text = match message_fn {
            Some(message_fn) => message_fn(&message),
            None => spans_message(&message),
        };
        announce(&text, window, cx);
    }

    // The tracks in the order arrows across them go through.
    fn order(&self, this: &T) -> Vec<SharedString> {
        let tracks = self.inner.options.borrow().tracks.clone();
        if let Some(tracks) = tracks {
            return tracks(this);
        }
        self.inner
            .bound
            .keys()
            .into_iter()
            .filter_map(|target| match target {
                SpansTarget::Track(track) => Some(track),
                SpansTarget::Span(_) => None,
            })
            .collect()
    }

    // The value under a point on a track, along its axis, unsnapped.
    fn value_under(&self, this: &T, track: &SharedString, point: Point<Pixels>, cx: &App) -> f64 {
        let (min, max) = (self.min(this), self.max(this));
        let Some(region) = self.inner.bound.region(&SpansTarget::Track(track.clone())) else {
            return min;
        };
        let rect = region.bounds().unwrap_or_default();
        let length = |pixels: Pixels| f64::from(f32::from(pixels));
        let mut fraction = match self.axis() {
            Axis::Vertical => {
                let height = length(rect.size.height);
                if height != 0.0 {
                    (length(point.y) - length(rect.top())) / height
                } else {
                    0.0
                }
            }
            Axis::Horizontal => {
                let width = length(rect.size.width);
                if width != 0.0 {
                    (length(point.x) - length(rect.left())) / width
                } else {
                    0.0
                }
            }
        };
        if self.axis() == Axis::Horizontal && is_rtl(cx) {
            fraction = 1.0 - fraction;
        }
        min + clamp(fraction, 0.0, 1.0) * (max - min)
    }

    // The track under a point across the axis: the one containing it, else
    // the nearest.
    fn track_at(&self, point: Point<Pixels>) -> Option<SharedString> {
        let mut best: Option<(SharedString, Pixels)> = None;
        for target in self.inner.bound.keys() {
            let SpansTarget::Track(track) = target else {
                continue;
            };
            let Some(rect) = self
                .inner
                .bound
                .region(&SpansTarget::Track(track.clone()))
                .and_then(|region| region.bounds())
            else {
                continue;
            };
            let (at, from, to) = match self.axis() {
                Axis::Vertical => (point.x, rect.left(), rect.right()),
                Axis::Horizontal => (point.y, rect.top(), rect.bottom()),
            };
            let distance = if at < from {
                from - at
            } else if at > to {
                at - to
            } else {
                Pixels::ZERO
            };
            if best.as_ref().is_none_or(|(_, best)| distance < *best) {
                best = Some((track, distance));
            }
        }
        best.map(|(track, _)| track)
    }

    // A span that moves to another track renders as a new element there;
    // focus follows it.
    fn focus_later(&self, id: &SharedString, window: &Window, cx: &mut App) {
        let bound = self.inner.bound.clone();
        let target = SpansTarget::Span(id.clone());
        keep_focus(
            move |_window, _cx| {
                bound
                    .region(&target)
                    .and_then(|region| region.focus_handle())
            },
            window,
            cx,
        );
    }

    // Whether a key on the span is the span's: focus is on it, not on a
    // field or anything else inside it.
    fn is_target(&self, id: &SharedString, window: &Window) -> bool {
        self.inner
            .bound
            .region(&SpansTarget::Span(id.clone()))
            .and_then(|region| region.focus_handle())
            .is_none_or(|focus| focus.is_focused(window))
    }

    fn commit(
        &self,
        this: &mut T,
        spans: Vec<S>,
        options: ChangeOptions,
        change: SpanChange,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let span = spans
            .iter()
            .find(|other| other.span().id == change.id)
            .cloned()
            .or_else(|| self.find(this, &change.id));
        let action = change.action;
        let set = self.inner.options.borrow().set.clone();
        if !set(this, spans, options, change, window, cx) {
            return false;
        }
        if let Some(span) = span {
            self.say(this, action.into(), &span, window, cx);
        }
        cx.notify();
        true
    }

    fn set_draft(&self, next: Option<SpansDraft<S>>) {
        let mut state = self.inner.state.borrow_mut();
        if next.is_some() {
            state.generation += 1;
        }
        state.draft = next;
    }

    // The draft now, over the value now: a settling draft whose value
    // arrived clears, and a held one follows spans the owner changed
    // mid-drag: it takes the spans they have now, with the held one where the
    // drag put it, or ends when it's gone or would overlap one the owner put
    // there.
    fn current(&self, this: &T) -> Option<SpansDraft<S>> {
        let now = self.value(this);
        let mut state = self.inner.state.borrow_mut();
        if let Some((before, generation)) = &state.settling
            && *before != now
        {
            if state.generation == *generation {
                state.draft = None;
            }
            state.settling = None;
            state._settle = None;
        }
        let changed = state.seen.as_ref() != Some(&now);
        state.seen = Some(now.clone());
        if !changed || !state.held {
            return state.draft.clone();
        }
        let current = state.draft.clone()?;
        if current.action != SpanAction::Draw
            && !now.iter().any(|span| span.span().id == current.id)
        {
            state.draft = None;
            return None;
        }
        let span = if state.moved {
            current
                .spans
                .iter()
                .find(|other| other.span().id == current.id)
                .cloned()
        } else {
            None
        };
        let spans = match &span {
            Some(span) => replaced(&now, span.clone()),
            None => now,
        };
        if span
            .as_ref()
            .is_some_and(|span| !self.allowed(&spans, span))
        {
            state.draft = None;
            return None;
        }
        if spans != current.spans {
            state.draft = Some(SpansDraft { spans, ..current });
        }
        state.draft.clone()
    }

    /// The draft while a pointer drags, kept in the spans as the original
    /// keeps it in state.
    pub fn draft(&self, this: &T) -> Option<SpansDraft<S>> {
        self.current(this)
    }

    // Clears the dropped draft once its spans arrive, or a task later, as
    // `settleDraft` does: an uncontrolled value has already moved on, so it
    // clears now, and an app that kept the old value shows it again.
    fn settle(&self, this: &T, before: Vec<S>, generation: u64, cx: &mut Context<T>) {
        if self.value(this) != before {
            let mut state = self.inner.state.borrow_mut();
            if state.generation == generation {
                state.draft = None;
            }
            return;
        }
        let weak = Rc::downgrade(&self.inner);
        let task = cx.spawn(async move |entity, cx| {
            cx.background_executor().timer(Duration::ZERO).await;
            entity
                .update(cx, |_this, cx| {
                    let Some(spans) = Self::upgrade(&weak) else {
                        return;
                    };
                    let settled = {
                        let mut state = spans.inner.state.borrow_mut();
                        let ours = state
                            .settling
                            .take_if(|(_, settling)| *settling == generation)
                            .is_some();
                        if ours && state.generation == generation {
                            state.draft = None;
                        }
                        ours
                    };
                    if settled {
                        cx.notify();
                    }
                })
                .ok();
        });
        let mut state = self.inner.state.borrow_mut();
        state.settling = Some((before, generation));
        state._settle = Some(task);
    }

    // Follows a pointer that pressed, building the draft from `next` on each move.
    #[allow(clippy::too_many_arguments)]
    fn drag(
        &self,
        this: &mut T,
        event: &PointerEvent,
        id: SharedString,
        action: SpanAction,
        next: NextFn<T, S>,
        click: Option<ClickFn<T, S>>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drag: DragRef<T, S> = Rc::new(RefCell::new(SpanDrag {
            id,
            action,
            before: self.value(this),
            dragging: false,
            last: None,
            point: event.position,
            scroller: None,
            next,
            click,
        }));
        let auto_scroll = self.inner.options.borrow().auto_scroll.clone();
        if let Some(auto_scroll) = auto_scroll {
            let spans = Rc::downgrade(&self.inner);
            let scrolled = Rc::downgrade(&drag);
            let entity = self.inner.entity.clone();
            let scroller = create_auto_scroll(auto_scroll.on_scroll(move |_window, cx| {
                let (Some(spans), Some(drag)) = (Self::upgrade(&spans), scrolled.upgrade()) else {
                    return;
                };
                if drag.borrow().dragging {
                    entity
                        .update(cx, |this, cx| spans.place(&drag, this, cx))
                        .ok();
                }
            }));
            drag.borrow_mut().scroller = Some(scroller);
        }
        let weak = Rc::downgrade(&self.inner);
        let (started, moved, ended) = (drag.clone(), drag.clone(), drag);
        let (on_start, on_move, on_end) = (weak.clone(), weak.clone(), weak);
        self.inner.drags.start(
            event,
            BindDragOptions::new()
                .threshold(DRAG_THRESHOLD)
                .cancel_on_escape(true)
                .on_start(move |this: &mut T, _event, _window, cx| {
                    let Some(spans) = Self::upgrade(&on_start) else {
                        return false;
                    };
                    if spans.current(this).is_some() {
                        return false;
                    }
                    let (id, action) = {
                        let mut drag = started.borrow_mut();
                        drag.dragging = true;
                        (drag.id.clone(), drag.action)
                    };
                    let value = spans.value(this);
                    {
                        let mut state = spans.inner.state.borrow_mut();
                        state.held = true;
                        state.moved = false;
                        state.seen = Some(value.clone());
                    }
                    spans.set_draft(Some(SpansDraft {
                        id,
                        action,
                        spans: value,
                    }));
                    cx.notify();
                    true
                })
                .on_move(move |this: &mut T, sample, window, cx| {
                    let Some(spans) = Self::upgrade(&on_move) else {
                        return;
                    };
                    let scroller = {
                        let mut drag = moved.borrow_mut();
                        drag.last = Some(sample.event.clone());
                        drag.point = sample.position();
                        drag.scroller.clone()
                    };
                    if let Some(scroller) = scroller {
                        scroller.update(sample.x, sample.y, window);
                    }
                    spans.place(&moved, this, cx);
                })
                .on_end(move |this: &mut T, end, canceled, window, cx| {
                    if let Some(spans) = Self::upgrade(&on_end) {
                        spans.drag_ended(&ended, this, end, canceled, window, cx);
                    }
                }),
            window,
            cx,
        );
    }

    // Puts the dragged span where the pointer is now (the drag's `update`).
    fn place(&self, drag: &DragRef<T, S>, this: &T, cx: &mut Context<T>) {
        let (next, point, id, action) = {
            let drag = drag.borrow();
            (drag.next.clone(), drag.point, drag.id.clone(), drag.action)
        };
        let Some(span) = next(self, this, point, cx) else {
            return;
        };
        let Some(current) = self.current(this) else {
            return;
        };
        let spans = replaced(&current.spans, span.clone());
        if !self.allowed(&spans, &span) {
            return;
        }
        self.inner.state.borrow_mut().moved = true;
        self.set_draft(Some(SpansDraft { id, action, spans }));
        cx.notify();
    }

    fn drag_ended(
        &self,
        drag: &DragRef<T, S>,
        this: &mut T,
        end: &PointerEvent,
        canceled: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let (dragged, scroller, id, action, last, click, before) = {
            let mut drag = drag.borrow_mut();
            let dragged = std::mem::replace(&mut drag.dragging, false);
            (
                dragged,
                drag.scroller.clone(),
                drag.id.clone(),
                drag.action,
                drag.last.clone(),
                drag.click.clone(),
                drag.before.clone(),
            )
        };
        if let Some(scroller) = scroller {
            scroller.stop();
        }
        let current = if dragged { self.current(this) } else { None };
        self.inner.state.borrow_mut().held = false;
        if !dragged {
            if !canceled && let Some(click) = click {
                click(self, this, window, cx);
            }
            return;
        }
        if canceled
            && action != SpanAction::Draw
            && let Some(span) = before.iter().find(|span| span.span().id == id)
        {
            self.say(this, SpansMessageType::Cancel, span, window, cx);
        }
        let moved = self.inner.state.borrow().moved;
        let Some(current) = current.filter(|_| !canceled && moved) else {
            self.set_draft(None);
            cx.notify();
            return;
        };
        let base = self.value(this);
        let generation = self.inner.state.borrow().generation;
        let event = last.map_or_else(|| end.event.clone(), |last| last.event);
        let committed = self.commit(
            this,
            current.spans,
            ChangeOptions::event_of(Reason::Drag, event),
            SpanChange { id, action },
            window,
            cx,
        );
        if committed {
            // Keeps the draft until a controlled value's new spans arrive.
            self.settle(this, base, generation, cx);
        } else {
            self.set_draft(None);
        }
        cx.notify();
    }

    /// Starts a render of the tracks and spans, from the part that holds
    /// them, every frame it renders. Binding one starts one too; this is
    /// what tells a frame that renders none of them that they've left.
    pub fn begin(&self, cx: &App) {
        self.inner.bound.begin(cx);
    }

    /// Joins a track's element as it renders, so drags find it, and wires
    /// its presses, which draw a span ([`track_pointer_down`](Self::track_pointer_down)).
    pub fn bind_track<E: InteractiveElement + ParentElement + Styled>(
        &self,
        track: impl Into<SharedString>,
        el: E,
        cx: &App,
    ) -> E {
        let track = track.into();
        let el = self
            .inner
            .bound
            .bind(SpansTarget::Track(track.clone()), None, el, cx);
        let spans = self.clone();
        on_pointer_down(el, move |event, window, cx| {
            let Some(entity) = spans.inner.entity.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                spans.track_pointer_down(this, event, &track, window, cx)
            });
        })
    }

    /// Joins a span's element as it renders, with the focus handle it
    /// tracks, which focus follows to another track, and wires its presses,
    /// which move it ([`span_pointer_down`](Self::span_pointer_down)).
    pub fn bind_span<E: InteractiveElement + ParentElement + Styled>(
        &self,
        id: impl Into<SharedString>,
        focus: Option<&FocusHandle>,
        el: E,
        cx: &App,
    ) -> E {
        let id = id.into();
        let el = self
            .inner
            .bound
            .bind(SpansTarget::Span(id.clone()), focus, el, cx);
        self.bind_press(id, None, el)
    }

    /// Wires an element inside a span that resizes it from `edge`, the
    /// original's `data-span-edge`.
    pub fn bind_edge<E: InteractiveElement + ParentElement + Styled>(
        &self,
        id: impl Into<SharedString>,
        edge: SpanEdge,
        el: E,
    ) -> E {
        self.bind_press(id.into(), Some(edge), el)
    }

    fn bind_press<E: InteractiveElement + ParentElement + Styled>(
        &self,
        id: SharedString,
        edge: Option<SpanEdge>,
        el: E,
    ) -> E {
        let spans = self.clone();
        on_pointer_down(el, move |event, window, cx| {
            let Some(entity) = spans.inner.entity.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                spans.span_pointer_down(this, event, &id, edge, window, cx)
            });
        })
    }

    /// The element that follows the pointer while a span is drawn, moved or
    /// resized. Render it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T, S> {
        self.inner.drags.listener()
    }

    /// The spans to render: the draft's while a pointer drags, else the value.
    pub fn spans(&self, this: &T) -> Vec<S> {
        match self.current(this) {
            Some(draft) => draft.spans,
            None => self.value(this),
        }
    }

    /// One track's spans, from [`spans`](Self::spans).
    pub fn spans_on(&self, this: &T, track: &str) -> Vec<S> {
        self.spans(this)
            .into_iter()
            .filter(|span| span.span().track.as_ref() == track)
            .collect()
    }

    /// What a pointer is doing to a span, if anything.
    pub fn action(&self, this: &T, id: &str) -> Option<SpanAction> {
        self.current(this)
            .filter(|draft| draft.id.as_ref() == id)
            .map(|draft| draft.action)
    }

    /// A span's place in its track: its start and length along the axis, and
    /// its lane across it from [`stack_spans`], if passed (`spanStyle`).
    pub fn span_style(&self, this: &T, span: &S, lane: Option<SpanLane>) -> SpanStyle {
        let (min, max) = (self.min(this), self.max(this));
        let range = max - min;
        let range = if range == 0.0 || range.is_nan() {
            1.0
        } else {
            range
        };
        let span = span.span();
        SpanStyle {
            start: (span.start - min) / range,
            size: (span.end - span.start) / range,
            lane,
            axis: self.axis(),
        }
    }

    /// A span's attributes: its id, and whether a pointer is dragging,
    /// resizing or drawing it (`spanAttrs`).
    pub fn span_attrs(&self, this: &T, id: &str) -> SpanAttrs {
        let action = self.action(this, id);
        SpanAttrs {
            id: SharedString::from(id.to_string()),
            dragging: action == Some(SpanAction::Move),
            resizing: action == Some(SpanAction::Resize),
            drawing: action == Some(SpanAction::Draw),
        }
    }

    /// The value under a point on a track, snapped. For a hover line or a
    /// "new event at" menu.
    pub fn value_at(
        &self,
        this: &T,
        track: &SharedString,
        position: Point<Pixels>,
        cx: &App,
    ) -> f64 {
        let value = self.value_under(this, track, position, cx);
        self.snap(this, value)
    }

    /// Draws a span from where a track was pressed, once the pointer moves a
    /// few pixels; a click makes one `size` long, if `size` is set. Presses
    /// on a span are left to [`span_pointer_down`](Self::span_pointer_down).
    pub fn track_pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        track: &SharedString,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drags = self.inner.drags.clone();
        if !self
            .inner
            .bound
            .contains(&SpansTarget::Track(track.clone()))
            || !drags.accepts(this, event, window, cx)
        {
            return;
        }
        let create = self.inner.options.borrow().create.clone();
        let Some(create) = create else {
            return;
        };
        if marked_press(Mark::Span, event) {
            return;
        }
        let step = self.step(this);
        let under = self.value_under(this, track, event.position, cx);
        let at = self.snap_with(this, under, f64::floor);
        let Some(base) = create(this, Span::new("", track.clone(), at, at + step)) else {
            return;
        };
        let id = base.span().id.clone();
        let next: NextFn<T, S> = {
            let (base, track) = (base.clone(), track.clone());
            Rc::new(
                move |spans: &Spans<T, S>, this: &T, point: Point<Pixels>, cx: &App| {
                    let v = spans.value_under(this, &track, point, cx);
                    let mut made = base.clone();
                    if v >= at {
                        let end = spans.snap_with(this, v, f64::ceil);
                        let range = made.span_mut();
                        range.track = track.clone();
                        range.start = at;
                        range.end = end;
                        Some(spans.fit(this, &made, Keep::Start))
                    } else {
                        let start = spans.snap_with(this, v, f64::floor);
                        let step = spans.step(this);
                        let range = made.span_mut();
                        range.start = start;
                        range.end = at + step;
                        Some(spans.fit(this, &made, Keep::End))
                    }
                },
            )
        };
        let press = event.event.clone();
        let click: ClickFn<T, S> = Rc::new(
            move |spans: &Spans<T, S>, this: &mut T, window: &mut Window, cx: &mut Context<T>| {
                let size = spans.inner.options.borrow().size.clone();
                let Some(size) = size.and_then(|size| size(this)) else {
                    return;
                };
                let mut span = base.clone();
                {
                    let range = span.span_mut();
                    range.start = at;
                    range.end = at + size;
                }
                let span = spans.fit(this, &span, Keep::Size);
                let next = replaced(&spans.value(this), span.clone());
                if spans.allowed(&next, &span) {
                    let change = SpanChange {
                        id: span.span().id.clone(),
                        action: SpanAction::Draw,
                    };
                    spans.commit(
                        this,
                        next,
                        ChangeOptions::event_of(Reason::Drag, press.clone()),
                        change,
                        window,
                        cx,
                    );
                }
            },
        );
        self.drag(
            this,
            event,
            id,
            SpanAction::Draw,
            next,
            Some(click),
            window,
            cx,
        );
        window.prevent_default();
    }

    /// Moves a span, or resizes it from `edge` for a press on an element
    /// inside it that resizes ([`bind_edge`](Self::bind_edge)), once the
    /// pointer moves a few pixels, so a click stays a click. Escape puts it
    /// back.
    pub fn span_pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        id: &SharedString,
        edge: Option<SpanEdge>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        // Spans sit inside their track, so the track hears their presses too.
        mark_press(Mark::Span, event, window, cx);
        if !self.inner.bound.contains(&SpansTarget::Span(id.clone())) {
            return;
        }
        let Some(span) = self.find(this, id) else {
            return;
        };
        let drags = self.inner.drags.clone();
        if !drags.accepts(this, event, window, cx) || self.is_disabled(this, &span) {
            return;
        }
        let grabbed =
            self.value_under(this, &span.span().track, event.position, cx) - span.span().start;
        let (action, next): (SpanAction, NextFn<T, S>) = match edge {
            Some(edge) => (
                SpanAction::Resize,
                Rc::new(
                    move |spans: &Spans<T, S>, this: &T, point: Point<Pixels>, cx: &App| {
                        let under = spans.value_under(this, &span.span().track, point, cx);
                        let v = spans.snap(this, under);
                        let mut next = span.clone();
                        Some(match edge {
                            SpanEdge::Start => {
                                next.span_mut().start = v;
                                spans.fit(this, &next, Keep::End)
                            }
                            SpanEdge::End => {
                                next.span_mut().end = v;
                                spans.fit(this, &next, Keep::Start)
                            }
                        })
                    },
                ),
            ),
            None => (
                SpanAction::Move,
                Rc::new(
                    move |spans: &Spans<T, S>, this: &T, point: Point<Pixels>, cx: &App| {
                        let track = spans
                            .track_at(point)
                            .unwrap_or_else(|| span.span().track.clone());
                        let under = spans.value_under(this, &track, point, cx);
                        let start = spans.snap(this, under - grabbed);
                        let mut next = span.clone();
                        let range = next.span_mut();
                        range.end = start + range.end - range.start;
                        range.start = start;
                        range.track = track;
                        Some(spans.fit(this, &next, Keep::Size))
                    },
                ),
            ),
        };
        self.drag(this, event, id.clone(), action, next, None, window, cx);
    }

    /// Handles a key on a span and returns whether it did. Arrows along the
    /// axis move it a step, with Shift they move its end; arrows across move
    /// it to the next track; Delete and Backspace delete it. Each change
    /// commits, and screen readers hear the span's new place. Keys from
    /// something focused inside the span, like a field with its title, are
    /// its own. A key it handles calls `window.prevent_default()`.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        id: &SharedString,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let Some(span) = self.find(this, id) else {
            return false;
        };
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented()
            || self.current(this).is_some()
            || modifiers.alt
            || modifiers.control
            || modifiers.platform
        {
            return false;
        }
        if self.is_disabled(this, &span) || !self.is_target(id, window) {
            return false;
        }
        let rtl = is_rtl(cx);
        let (back, forward, before, after) = match self.axis() {
            Axis::Vertical => (
                Key::ArrowUp,
                Key::ArrowDown,
                if rtl { Key::ArrowRight } else { Key::ArrowLeft },
                if rtl { Key::ArrowLeft } else { Key::ArrowRight },
            ),
            Axis::Horizontal => (
                if rtl { Key::ArrowRight } else { Key::ArrowLeft },
                if rtl { Key::ArrowLeft } else { Key::ArrowRight },
                Key::ArrowUp,
                Key::ArrowDown,
            ),
        };
        let key = Key::of(&event.keystroke);
        let mut action = SpanAction::Move;
        let mut next: Option<S> = None;
        if key == Key::Delete || key == Key::Backspace {
            window.prevent_default();
            let rest = self
                .value(this)
                .into_iter()
                .filter(|other| other.span().id != *id)
                .collect();
            let change = SpanChange {
                id: id.clone(),
                action: SpanAction::Delete,
            };
            return self.commit(
                this,
                rest,
                ChangeOptions::event(Reason::RemovePress, event),
                change,
                window,
                cx,
            );
        } else if key == back || key == forward {
            let step = self.step(this);
            let by = if key == forward { step } else { -step };
            let mut moved = span.clone();
            if modifiers.shift {
                action = SpanAction::Resize;
                moved.span_mut().end += by;
                next = Some(self.fit(this, &moved, Keep::Start));
            } else {
                let range = moved.span_mut();
                range.start += by;
                range.end += by;
                next = Some(self.fit(this, &moved, Keep::Size));
            }
        } else if (key == before || key == after) && !modifiers.shift {
            let names = self.order(this);
            let index = names
                .iter()
                .position(|track| *track == span.span().track)
                .map_or(-1, |index| index as isize)
                + if key == after { 1 } else { -1 };
            if index >= 0
                && let Some(track) = names.get(index as usize)
            {
                let mut moved = span.clone();
                moved.span_mut().track = track.clone();
                next = Some(moved);
            }
        }
        let Some(next) = next else {
            return false;
        };
        window.prevent_default();
        let result = replaced(&self.value(this), next.clone());
        let (to, from) = (next.span(), span.span());
        if to.start == from.start && to.end == from.end && to.track == from.track {
            return true;
        }
        let moved_track = to.track != from.track;
        let change = SpanChange {
            id: id.clone(),
            action,
        };
        if self.allowed(&result, &next)
            && self.commit(
                this,
                result,
                ChangeOptions::event(Reason::MoveKey, event),
                change,
                window,
                cx,
            )
            && moved_track
        {
            self.focus_later(id, window, cx);
        }
        true
    }

    /// Changes a span at once, like from a form, within range and size, and
    /// the overlap rule. Reports `Imperative`; `disabled` doesn't apply.
    /// Returns whether it went through.
    pub fn update(
        &self,
        this: &mut T,
        id: &SharedString,
        patch: SpanPatch<S>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let Some(span) = self.find(this, id) else {
            return false;
        };
        let keep = match (patch.start, patch.end) {
            (None, _) => Keep::Start,
            (Some(_), None) => Keep::End,
            (Some(_), Some(_)) => Keep::Size,
        };
        let mut patched = span;
        if let Some(with) = &patch.with {
            with(&mut patched);
        }
        {
            let range = patched.span_mut();
            range.id = id.clone();
            if let Some(track) = patch.track {
                range.track = track;
            }
            if let Some(start) = patch.start {
                range.start = start;
            }
            if let Some(end) = patch.end {
                range.end = end;
            }
        }
        let next = self.fit(this, &patched, keep);
        let result = replaced(&self.value(this), next.clone());
        if !self.allowed(&result, &next) {
            return false;
        }
        let change = SpanChange {
            id: id.clone(),
            action: SpanAction::Move,
        };
        self.commit(
            this,
            result,
            ChangeOptions::reason(Reason::Imperative),
            change,
            window,
            cx,
        )
    }

    /// Deletes a span. Reports `Imperative`. Returns whether it went through.
    pub fn remove(
        &self,
        this: &mut T,
        id: &SharedString,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let value = self.value(this);
        if !value.iter().any(|span| span.span().id == *id) {
            return false;
        }
        let rest = value
            .into_iter()
            .filter(|span| span.span().id != *id)
            .collect();
        let change = SpanChange {
            id: id.clone(),
            action: SpanAction::Delete,
        };
        self.commit(
            this,
            rest,
            ChangeOptions::reason(Reason::Imperative),
            change,
            window,
            cx,
        )
    }

    /// Ends a pointer drag and puts its span back.
    pub fn cancel(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        self.inner.drags.clone().cancel(this, window, cx);
        self.set_draft(None);
        cx.notify();
    }
}
