//! Text the reader selects and copies but doesn't edit, the selection a
//! browser gives a page's words: a press and drag selects, across every run
//! of words that shares a [`TextSelection`]; a double press selects a word
//! and a triple press its paragraph, and a drag after them extends by whole
//! words or paragraphs, keeping what they selected whichever way it goes;
//! Shift extends, by the last press's unit while its selection stands; and
//! [`TextSelection::text`] is what a copy takes.
//!
//! gpui draws text with no selection of its own, so each run of words is a
//! [`SelectableText`], which wraps gpui's `StyledText`, notes where it was
//! laid out in its selection, and paints its part of the selection under its
//! glyphs. Runs are ordered by their key (`K`), which is the order a drag
//! crosses them in and a copy joins them in: a message's place in a
//! conversation and the block's place in the message, say.
//!
//! The view keeps a selection's drag going past the runs' edges with
//! [`TextSelection::drag_layer`], drawn over them, which follows the pointer
//! anywhere in the window, settles the selection when the button rises
//! anywhere, and scrolls the view while the pointer is held past its top or
//! bottom. It calls [`TextSelection::frame`] each time it renders, so runs
//! scrolled out of it aren't hit.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ops::{Range, RangeInclusive};
use std::rc::Rc;

use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, Styled, StyledText, TextLayout,
    TextRun, Window, canvas, fill, hsla, point, px,
};

use crate::boundaries::{floor_char, paragraph_at, word_at};

/// A place in the words: a run, and a byte offset into its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPosition<K> {
    pub key: K,
    pub offset: usize,
}

/// What a press selects by, and a drag after it extends by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Granularity {
    #[default]
    Character,
    Word,
    Paragraph,
}

/// What a copy puts between one run's words and the next's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Join {
    /// A line break: lines of one block.
    #[default]
    Line,
    /// A tab: a table's cells along a row.
    Tab,
    /// A blank line: blocks with room around them.
    Paragraph,
}

/// What a run's link looks like under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkLook {
    pub cursor: CursorStyle,
    /// A wash painted under the link the pointer's on.
    pub hover: Option<Hsla>,
}

impl Default for LinkLook {
    fn default() -> Self {
        Self {
            cursor: CursorStyle::PointingHand,
            hover: None,
        }
    }
}

struct Run {
    text: SharedString,
    /// Ranges that copy as other text, with it.
    copy_as: Vec<(Range<usize>, String)>,
    layout: TextLayout,
    bounds: Bounds<Pixels>,
    /// The frame it was last laid out in.
    drawn: u64,
    /// Its text was cut short where it was laid out (a line clamp).
    clamped: bool,
}

// What the last double or triple press selected, which a drag or a Shift
// press after it extends from by its unit.
#[derive(Clone, Copy)]
struct Unit<K> {
    granularity: Granularity,
    start: TextPosition<K>,
    end: TextPosition<K>,
}

type Joiner<K> = Rc<dyn Fn(&K, &K) -> Join>;

struct State<K> {
    anchor: Option<TextPosition<K>>,
    head: Option<TextPosition<K>>,
    dragging: bool,
    // A press on words is down: its release, anywhere, settles the selection.
    pressed: bool,
    unit: Option<Unit<K>>,
    runs: BTreeMap<K, Run>,
    frame: u64,
    // A press on a link, kept to follow it if the button rises there without
    // a drag.
    pressed_link: Option<(K, usize)>,
    joins: HashMap<K, Join>,
    joiner: Option<Joiner<K>>,
    multi_click_off_glyphs: bool,
    wash: Hsla,
}

/// One selection over many runs of words, and every run it has drawn.
/// Cloning it clones a handle to the same selection.
pub struct TextSelection<K>(Rc<RefCell<State<K>>>);

impl<K> Clone for TextSelection<K> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<K: Ord + Copy + std::hash::Hash + 'static> Default for TextSelection<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// The system's selection blue, see-through so it reads on any background.
pub fn default_selection_wash() -> Hsla {
    hsla(211. / 360., 1., 0.5, 0.32)
}

impl<K: Ord + Copy + std::hash::Hash + 'static> TextSelection<K> {
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(State {
            anchor: None,
            head: None,
            dragging: false,
            pressed: false,
            unit: None,
            runs: BTreeMap::new(),
            frame: 0,
            pressed_link: None,
            joins: HashMap::new(),
            joiner: None,
            multi_click_off_glyphs: true,
            wash: default_selection_wash(),
        })))
    }

    /// What a copy puts between two runs that follow each other, by their
    /// keys; the larger of it and the later run's own [`Self::join`] wins.
    /// A line break when not given.
    pub fn joiner(self, joiner: impl Fn(&K, &K) -> Join + 'static) -> Self {
        self.0.borrow_mut().joiner = Some(Rc::new(joiner));
        self
    }

    /// Whether a double or triple press past the end of a run's glyphs (in
    /// the room left on its last line) selects, or is left to the element
    /// under the run. Selects by default.
    pub fn multi_click_off_glyphs(self, selects: bool) -> Self {
        self.0.borrow_mut().multi_click_off_glyphs = selects;
        self
    }

    /// The color painted under selected words.
    pub fn wash(self, color: Hsla) -> Self {
        self.0.borrow_mut().wash = color;
        self
    }

    /// A new frame of the view: what's laid out from here is what's on screen.
    pub fn frame(&self) {
        self.0.borrow_mut().frame += 1;
    }

    /// The selection's ends in order, when anything is selected.
    pub fn range(&self) -> Option<(TextPosition<K>, TextPosition<K>)> {
        let s = self.0.borrow();
        let (a, h) = (s.anchor?, s.head?);
        if a == h {
            return None;
        }
        Some(if a <= h { (a, h) } else { (h, a) })
    }

    pub fn is_empty(&self) -> bool {
        self.range().is_none()
    }

    /// Whether a drag is selecting.
    pub fn is_dragging(&self) -> bool {
        self.0.borrow().dragging
    }

    /// The unit the last press selected by, which a drag or Shift press
    /// after it extends by.
    pub fn granularity(&self) -> Granularity {
        self.0
            .borrow()
            .unit
            .map_or(Granularity::Character, |u| u.granularity)
    }

    pub fn clear(&self) {
        let mut s = self.0.borrow_mut();
        s.anchor = None;
        s.head = None;
        s.dragging = false;
        s.unit = None;
    }

    /// Selects every run it holds.
    pub fn select_all(&self) {
        let mut s = self.0.borrow_mut();
        let first = s.runs.keys().next().copied();
        let last = s.runs.iter().next_back().map(|(k, r)| (*k, r.text.len()));
        if let (Some(a), Some((b, len))) = (first, last) {
            s.anchor = Some(TextPosition { key: a, offset: 0 });
            s.head = Some(TextPosition { key: b, offset: len });
            s.unit = None;
        }
    }

    /// Selects `range` of a run's words.
    pub fn select(&self, key: K, range: Range<usize>) {
        let mut s = self.0.borrow_mut();
        s.anchor = Some(TextPosition {
            key,
            offset: range.start,
        });
        s.head = Some(TextPosition {
            key,
            offset: range.end,
        });
        s.unit = None;
    }

    /// Run `key` follows the one before it with `join` in a copy.
    pub fn join(&self, key: K, join: Join) {
        self.0.borrow_mut().joins.insert(key, join);
    }

    /// The words selected, as a copy takes them.
    pub fn text(&self) -> String {
        let Some((from, to)) = self.range() else {
            return String::new();
        };
        let s = self.0.borrow();
        let mut out = String::new();
        let mut last: Option<K> = None;
        for (key, run) in s.runs.range(from.key..=to.key) {
            let len = run.text.len();
            let start = if *key == from.key { from.offset } else { 0 };
            let end = if *key == to.key { to.offset } else { len };
            if let Some(prev) = last {
                let between = s.joiner.as_ref().map_or(Join::Line, |j| j(&prev, key));
                let own = s.joins.get(key).copied().unwrap_or_default();
                out.push_str(match between.max(own) {
                    Join::Line => "\n",
                    Join::Tab => "\t",
                    Join::Paragraph => "\n\n",
                });
            }
            out.push_str(&copied(run, start.min(len)..end.min(len)));
            last = Some(*key);
        }
        out
    }

    /// Where the selection starts on screen: the first line of it.
    pub fn first_rect(&self) -> Option<Bounds<Pixels>> {
        let (from, to) = self.range()?;
        let s = self.0.borrow();
        let run = s.runs.get(&from.key)?;
        let len = run.text.len();
        let end = if to.key == from.key { to.offset } else { len };
        rects(
            &run.layout,
            run.bounds,
            from.offset.min(len)..end.min(len),
        )
        .into_iter()
        .next()
    }

    /// The lines a range of a run's words cover on screen, where it was last
    /// laid out.
    pub fn rects(&self, key: K, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let s = self.0.borrow();
        let Some(run) = s.runs.get(&key) else {
            return vec![];
        };
        let len = run.text.len();
        rects(&run.layout, run.bounds, range.start.min(len)..range.end.min(len))
    }

    /// The runs it holds with keys in `keys`, and their words, in order.
    pub fn runs(&self, keys: RangeInclusive<K>) -> Vec<(K, SharedString)> {
        self.0
            .borrow()
            .runs
            .range(keys)
            .map(|(k, r)| (*k, r.text.clone()))
            .collect()
    }

    /// Whether a run's words were cut short where they were laid out.
    pub fn is_clamped(&self, key: K) -> bool {
        self.0.borrow().runs.get(&key).is_some_and(|r| r.clamped)
    }

    /// Forgets the runs whose keys `keep` turns down, as their words leave
    /// the view.
    pub fn retain(&self, keep: impl Fn(&K) -> bool) {
        let mut s = self.0.borrow_mut();
        s.runs.retain(|k, _| keep(k));
        s.joins.retain(|k, _| keep(k));
    }

    /// Where a window position is in the words: in the run under it, else at
    /// the nearest run laid out before it.
    pub fn hit(&self, at: Point<Pixels>) -> Option<TextPosition<K>> {
        let s = self.0.borrow();
        let frame = s.frame;
        let mut first = None;
        // The last run whose top is above the pointer, in the runs' order.
        let mut best: Option<(&K, &Run)> = None;
        for (k, r) in s.runs.iter().filter(|(_, r)| r.drawn == frame) {
            first.get_or_insert(*k);
            if r.bounds.contains(&at) {
                let ix = r.layout.index_for_position(at).unwrap_or_else(|e| e);
                return Some(TextPosition {
                    key: *k,
                    offset: floor_char(&r.text, ix),
                });
            }
            if r.bounds.top() <= at.y {
                best = Some((k, r));
            }
        }
        match best {
            Some((k, r)) => {
                let ix = if at.y > r.bounds.bottom() {
                    r.text.len()
                } else {
                    r.layout.index_for_position(at).unwrap_or_else(|e| e)
                };
                Some(TextPosition {
                    key: *k,
                    offset: floor_char(&r.text, ix),
                })
            }
            None => first.map(|key| TextPosition { key, offset: 0 }),
        }
    }

    /// A drag goes on to a window position. Answers whether the selection
    /// changed.
    pub fn drag_to(&self, at: Point<Pixels>) -> bool {
        if !self.0.borrow().dragging {
            return false;
        }
        let Some(pos) = self.hit(at) else {
            return false;
        };
        let before = {
            let s = self.0.borrow();
            (s.anchor, s.head)
        };
        self.extend_to(pos);
        let mut s = self.0.borrow_mut();
        if (s.anchor, s.head) != before {
            s.pressed_link = None;
            return true;
        }
        false
    }

    /// The button rises: a drag ends, and a press and release in one place
    /// selects nothing, though it stays where a Shift press extends from.
    /// Answers the link pressed, by its run and its number there, when the
    /// press didn't drag.
    pub fn release(&self) -> Option<(K, usize)> {
        let mut s = self.0.borrow_mut();
        s.dragging = false;
        if s.anchor.is_some() && s.anchor == s.head {
            return s.pressed_link.take();
        }
        s.pressed_link = None;
        None
    }

    // Moves the selection's head to `pos`, by the held unit: what a double
    // or triple press selected stays selected, and the moving end snaps to
    // the far edge of the word or paragraph at `pos`.
    fn extend_to(&self, pos: TextPosition<K>) {
        let mut s = self.0.borrow_mut();
        let Some(unit) = s.unit else {
            s.head = Some(pos);
            return;
        };
        let Some(text) = s.runs.get(&pos.key).map(|r| r.text.clone()) else {
            return;
        };
        let at = unit_at(unit.granularity, &text, pos.offset);
        let (start, end) = (
            TextPosition {
                key: pos.key,
                offset: at.start,
            },
            TextPosition {
                key: pos.key,
                offset: at.end,
            },
        );
        if start < unit.start {
            s.anchor = Some(unit.end);
            s.head = Some(start);
        } else {
            s.anchor = Some(unit.start);
            s.head = Some(end.max(unit.end));
        }
    }

    // A press on a run's words, `clicks` times in a row.
    fn press(&self, key: K, text: &str, offset: usize, clicks: usize, shift: bool) {
        let offset = floor_char(text, offset);
        let pos = TextPosition { key, offset };
        let granularity = match clicks {
            0 | 1 => Granularity::Character,
            2 => Granularity::Word,
            _ => Granularity::Paragraph,
        };
        {
            let mut s = self.0.borrow_mut();
            s.pressed = true;
            s.dragging = true;
            s.pressed_link = None;
        }
        if granularity == Granularity::Character {
            let held = {
                let s = self.0.borrow();
                // The unit still holds while its selection stands.
                s.unit.filter(|u| {
                    let (a, h) = (s.anchor, s.head);
                    let (lo, hi) = match (a, h) {
                        (Some(a), Some(h)) => (a.min(h), a.max(h)),
                        _ => return false,
                    };
                    lo <= u.start && u.end <= hi
                })
            };
            let mut s = self.0.borrow_mut();
            if shift && s.anchor.is_some() {
                s.unit = held;
                drop(s);
                self.extend_to(pos);
            } else {
                s.unit = None;
                s.anchor = Some(pos);
                s.head = Some(pos);
            }
            return;
        }
        let at = unit_at(granularity, text, offset);
        let unit = Unit {
            granularity,
            start: TextPosition {
                key,
                offset: at.start,
            },
            end: TextPosition {
                key,
                offset: at.end,
            },
        };
        let mut s = self.0.borrow_mut();
        s.unit = Some(unit);
        s.anchor = Some(unit.start);
        s.head = Some(unit.end);
    }

    // Notes where a run was laid out; answers whether its clamp changed.
    fn put(
        &self,
        key: K,
        text: SharedString,
        copy_as: Vec<(Range<usize>, String)>,
        layout: TextLayout,
        bounds: Bounds<Pixels>,
    ) -> bool {
        let clamped = layout.text() != text.as_ref();
        let mut s = self.0.borrow_mut();
        let frame = s.frame;
        let was = s.runs.get(&key).map(|r| r.clamped);
        s.runs.insert(
            key,
            Run {
                text,
                copy_as,
                layout,
                bounds,
                drawn: frame,
                clamped,
            },
        );
        was.is_some_and(|w| w != clamped) || (was.is_none() && clamped)
    }

    // A run's part of the selection, as a byte range of its words.
    fn part(&self, key: K, len: usize) -> Option<Range<usize>> {
        let (from, to) = self.range()?;
        if key < from.key || key > to.key {
            return None;
        }
        let start = if key == from.key { from.offset } else { 0 };
        let end = if key == to.key { to.offset } else { len };
        (end > start).then_some(start.min(len)..end.min(len))
    }

    /// Keeps a drag going outside the runs, drawn over the view that holds
    /// them: the pointer is followed anywhere in the window and the button's
    /// release anywhere settles the selection (`on_release` after it), and
    /// held past the top or bottom of `viewport` (in window coordinates) the view is moved by
    /// `scroll` each frame, faster the further past it is, as a browser's
    /// autoscroll does.
    pub fn drag_layer(
        &self,
        viewport: impl Fn(&Window) -> Bounds<Pixels> + 'static,
        scroll: impl Fn(Pixels, &mut Window, &mut App) + 'static,
        on_release: impl Fn(&mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let (sel, on_release) = (self.clone(), Rc::new(on_release));
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                if sel.is_dragging() {
                    let b = viewport(window);
                    let at = window.mouse_position();
                    let past = if at.y < b.top() {
                        at.y - b.top()
                    } else if at.y > b.bottom() {
                        at.y - b.bottom()
                    } else {
                        Pixels::ZERO
                    };
                    if past != Pixels::ZERO {
                        let by = if past < Pixels::ZERO {
                            (past / 4.).min(px(-2.))
                        } else {
                            (past / 4.).max(px(2.))
                        };
                        scroll(by, window, cx);
                        sel.drag_to(at);
                        window.request_animation_frame();
                    }
                }
                let moved = sel.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, _| {
                    if phase == DispatchPhase::Bubble && moved.drag_to(e.position) {
                        window.refresh();
                    }
                });
                let (up, on_release) = (sel.clone(), on_release.clone());
                window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble
                        || e.button != MouseButton::Left
                        || !std::mem::take(&mut up.0.borrow_mut().pressed)
                    {
                        return;
                    }
                    // After the words under it have heard it, so a link's
                    // press and release follows it.
                    let (up, on_release) = (up.clone(), on_release.clone());
                    window.defer(cx, move |window, cx| {
                        up.release();
                        on_release(window, cx);
                        window.refresh();
                    });
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

fn unit_at(granularity: Granularity, text: &str, offset: usize) -> Range<usize> {
    match granularity {
        Granularity::Character => offset..offset,
        Granularity::Word => word_at(text, offset),
        Granularity::Paragraph => paragraph_at(text, offset),
    }
}

// A run's words in `part`, with what copies as other text taken as it.
fn copied(run: &Run, part: Range<usize>) -> String {
    let mut out = String::new();
    let mut at = part.start;
    for (range, written) in &run.copy_as {
        if range.end <= at || range.start >= part.end {
            continue;
        }
        out.push_str(run.text.get(at..range.start.max(at)).unwrap_or(""));
        out.push_str(written);
        at = range.end;
    }
    if at < part.end {
        out.push_str(run.text.get(at..part.end).unwrap_or(""));
    }
    out
}

// The rectangles a range of a laid out run covers, line by line.
fn rects(layout: &TextLayout, bounds: Bounds<Pixels>, part: Range<usize>) -> Vec<Bounds<Pixels>> {
    let (Some(a), Some(b)) = (
        layout.position_for_index(part.start),
        layout.position_for_index(part.end),
    ) else {
        return vec![];
    };
    let h = layout.line_height();
    if a.y == b.y {
        return vec![Bounds::from_corners(a, point(b.x, b.y + h))];
    }
    let mut out = vec![Bounds::from_corners(a, point(bounds.right(), a.y + h))];
    let mut y = a.y + h;
    while y < b.y {
        out.push(Bounds::from_corners(
            point(bounds.left(), y),
            point(bounds.right(), y + h),
        ));
        y += h;
    }
    out.push(Bounds::from_corners(point(bounds.left(), b.y), point(b.x, b.y + h)));
    out
}

type OnLink = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A run of selectable words: gpui's `StyledText`, with its part of its
/// [`TextSelection`] painted under it, and links followed on a press and
/// release that didn't drag.
pub struct SelectableText<K> {
    id: ElementId,
    key: K,
    selection: TextSelection<K>,
    text: StyledText,
    words: SharedString,
    copy_as: Vec<(Range<usize>, String)>,
    links: Vec<Range<usize>>,
    link_looks: Vec<LinkLook>,
    on_link: Option<OnLink>,
    cursor: CursorStyle,
}

impl<K: Ord + Copy + std::hash::Hash + 'static> SelectableText<K> {
    /// The run `key` of `selection`, showing `words`.
    pub fn new(
        id: impl Into<ElementId>,
        key: K,
        selection: &TextSelection<K>,
        words: impl Into<SharedString>,
    ) -> Self {
        let words = words.into();
        Self {
            id: id.into(),
            key,
            selection: selection.clone(),
            text: StyledText::new(words.clone()),
            words,
            copy_as: vec![],
            links: vec![],
            link_looks: vec![],
            on_link: None,
            cursor: CursorStyle::IBeam,
        }
    }

    /// Styles the words by runs, as `StyledText::with_runs` does.
    pub fn with_runs(mut self, runs: Vec<TextRun>) -> Self {
        self.text = StyledText::new(self.words.clone()).with_runs(runs);
        self
    }

    /// Ranges of the words a copy takes as other text: a stand-in drawn
    /// over, copied as what it stands for.
    pub fn copy_as(mut self, ranges: Vec<(Range<usize>, String)>) -> Self {
        self.copy_as = ranges;
        self
    }

    /// The cursor over the words outside links: the I-beam by default.
    pub fn cursor(mut self, cursor: CursorStyle) -> Self {
        self.cursor = cursor;
        self
    }

    /// Ranges that are links; `on_link` gets the link's number in `links`
    /// on a press and release on it that didn't drag.
    pub fn links(
        mut self,
        links: Vec<Range<usize>>,
        on_link: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.links = links;
        self.on_link = Some(Rc::new(on_link));
        self
    }

    /// How each link looks under the pointer, by its number; a hand with no
    /// wash for those not given.
    pub fn link_looks(mut self, looks: Vec<LinkLook>) -> Self {
        self.link_looks = looks;
        self
    }

    /// The words as laid out, for drawing over them once painted.
    pub fn layout(&self) -> &TextLayout {
        self.text.layout()
    }
}

impl<K: Ord + Copy + std::hash::Hash + 'static> Element for SelectableText<K> {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.text.request_layout(None, inspector, window, cx)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.text.prepaint(None, inspector, bounds, state, window, cx);
        if self.selection.put(
            self.key,
            self.words.clone(),
            self.copy_as.clone(),
            self.text.layout().clone(),
            bounds,
        ) {
            window.refresh();
        }
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.text.layout().clone();
        let wash = self.selection.0.borrow().wash;
        if let Some(part) = self.selection.part(self.key, self.words.len()) {
            for r in rects(&layout, bounds, part) {
                window.paint_quad(fill(r, wash));
            }
        }
        let under = if hitbox.is_hovered(window) {
            layout
                .index_for_position(window.mouse_position())
                .ok()
                .and_then(|ix| self.links.iter().position(|r| r.contains(&ix)))
        } else {
            None
        };
        let look = under.map(|n| self.link_looks.get(n).copied().unwrap_or_default());
        if let (Some(n), Some(hover)) = (under, look.and_then(|l| l.hover)) {
            for r in rects(&layout, bounds, self.links[n].clone()) {
                window.paint_quad(fill(r, hover));
            }
        }
        window.set_cursor_style(look.map_or(self.cursor, |l| l.cursor), hitbox);

        let (sel, key, words, links) = (
            self.selection.clone(),
            self.key,
            self.words.clone(),
            self.links.clone(),
        );
        let (hit, layout_down) = (hitbox.clone(), layout.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || e.button != MouseButton::Left
                || !hit.is_hovered(window)
            {
                return;
            }
            let exact = layout_down.index_for_position(e.position);
            let ix = exact.unwrap_or_else(|x| x).min(words.len());
            let off_glyphs = exact.is_err() && !sel.0.borrow().multi_click_off_glyphs;
            if e.click_count >= 2 && off_glyphs {
                return;
            }
            sel.press(key, &words, ix, e.click_count, e.modifiers.shift);
            if e.click_count <= 1 {
                sel.0.borrow_mut().pressed_link =
                    links.iter().position(|r| r.contains(&ix)).map(|n| (key, n));
            } else {
                cx.stop_propagation();
            }
            window.refresh();
        });
        if let Some(on_link) = self.on_link.clone() {
            let (sel, hit) = (self.selection.clone(), hitbox.clone());
            window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || e.button != MouseButton::Left
                    || !hit.is_hovered(window)
                {
                    return;
                }
                let pressed = sel.0.borrow().pressed_link;
                if let Some((k, n)) = pressed
                    && k == key
                    && sel.is_empty()
                {
                    sel.0.borrow_mut().pressed_link = None;
                    on_link(n, window, cx);
                }
            });
        }
        self.text
            .paint(None, inspector, bounds, state, &mut (), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_follow_unicode_and_dictionaries() {
        let text = "Visit https://example.com/path, don't wait: 3.14 日本語を話す";
        let word = |at: &str| {
            let i = text.find(at).unwrap();
            &text[word_at(text, i)]
        };
        assert_eq!(word("Visit"), "Visit");
        assert_eq!(word("example"), "example.com");
        assert_eq!(word("path"), "path");
        assert_eq!(word("don't"), "don't");
        assert_eq!(word("14"), "3.14");
        assert_eq!(word(","), ",");
        assert_eq!(word("日本"), "日本語");
    }

    #[test]
    fn paragraphs_end_at_line_breaks() {
        let text = "one two\nthree four";
        assert_eq!(&text[paragraph_at(text, 9)], "three four");
        assert_eq!(&text[paragraph_at(text, 2)], "one two");
    }

    fn two_runs() -> TextSelection<u32> {
        let sel = TextSelection::new();
        for (k, t) in [(0, "alpha beta gamma"), (1, "delta epsilon\nzeta eta")] {
            sel.0.borrow_mut().runs.insert(
                k,
                Run {
                    text: SharedString::from(t),
                    copy_as: vec![],
                    layout: TextLayout::default(),
                    bounds: Bounds::default(),
                    drawn: 0,
                    clamped: false,
                },
            );
        }
        sel
    }

    fn pos(key: u32, offset: usize) -> TextPosition<u32> {
        TextPosition { key, offset }
    }

    #[test]
    fn a_drag_after_a_double_press_extends_by_words() {
        let sel = two_runs();
        sel.press(0, "alpha beta gamma", 7, 2, false);
        assert_eq!(sel.text(), "beta");
        sel.extend_to(pos(0, 12));
        assert_eq!(sel.text(), "beta gamma");
        sel.extend_to(pos(1, 2));
        assert_eq!(sel.text(), "beta gamma\ndelta");
        // Back past where it began: the word pressed stays selected.
        sel.extend_to(pos(0, 1));
        assert_eq!(sel.text(), "alpha beta");
    }

    #[test]
    fn a_drag_after_a_triple_press_extends_by_paragraphs() {
        let sel = two_runs();
        sel.press(1, "delta epsilon\nzeta eta", 2, 3, false);
        assert_eq!(sel.text(), "delta epsilon");
        sel.extend_to(pos(1, 16));
        assert_eq!(sel.text(), "delta epsilon\nzeta eta");
        sel.extend_to(pos(0, 3));
        assert_eq!(sel.text(), "alpha beta gamma\ndelta epsilon");
    }

    #[test]
    fn shift_extends_by_the_held_unit() {
        let sel = two_runs();
        sel.press(0, "alpha beta gamma", 7, 2, false);
        sel.release();
        sel.press(1, "delta epsilon\nzeta eta", 8, 1, true);
        assert_eq!(sel.text(), "beta gamma\ndelta epsilon");
        sel.release();
        // A plain press lets the unit go; Shift then extends by characters.
        sel.press(0, "alpha beta gamma", 2, 1, false);
        sel.release();
        sel.press(0, "alpha beta gamma", 8, 1, true);
        assert_eq!(sel.text(), "pha be");
    }

    #[test]
    fn copies_join_runs_and_take_stand_ins_as_written() {
        let sel = two_runs().joiner(|_, _| Join::Paragraph);
        sel.0.borrow_mut().runs.get_mut(&0).unwrap().copy_as = vec![(6..10, "β".into())];
        sel.select_all();
        assert_eq!(sel.text(), "alpha β gamma\n\ndelta epsilon\nzeta eta");
    }
}
