//! Port of `foundation/drag-and-drop/auto-scroll.ts`: scrolling while
//! dragging near an edge, so a drag can reach what's out of view: a card to
//! a column off screen, a row past the bottom of a long list. The closer the
//! pointer is to the edge, the faster it scrolls.
//!
//! The original finds the scrollers under the pointer in the DOM, the
//! innermost first, then scrolls the window. gpui can't list the elements at
//! a point, and its windows don't scroll, so a drag names what can scroll:
//! the [`ScrollHandle`]s of its scrolling elements, innermost first. Each
//! animation frame, those whose bounds hold the pointer scroll, each axis by
//! the innermost one that moves. The original's `ignore`, the dragged
//! element it looks past to find the scrollers under it, has nothing to
//! look past here.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use gpui::{App, Pixels, Point, ScrollHandle, Window, px};

/// What [`AutoScrollOptions::on_scroll`] runs.
pub type AutoScrollCallback = Rc<dyn Fn(&mut Window, &mut App)>;

/// How [`create_auto_scroll`] scrolls (`AutoScrollOptions`).
#[derive(Clone)]
pub struct AutoScrollOptions {
    /// How far in from an edge scrolling starts. Defaults to 48.
    pub edge: Pixels,
    /// The fastest it scrolls, per frame, with the pointer at the edge.
    /// Defaults to 18.
    pub speed: Pixels,
    /// Runs after each frame that scrolled, so a drag can find its target
    /// again under a pointer that didn't move. It runs once gpui has drawn
    /// that frame, so the elements it laid out are measured where the scroll
    /// put them. It runs outside any update, so it may update any entity.
    pub on_scroll: Option<AutoScrollCallback>,
    /// The scroll handles of the elements a drag scrolls, innermost first,
    /// each tracked by its element with `track_scroll`.
    pub scrollers: Vec<ScrollHandle>,
}

impl Default for AutoScrollOptions {
    fn default() -> Self {
        Self {
            edge: px(48.),
            speed: px(18.),
            on_scroll: None,
            scrollers: Vec::new(),
        }
    }
}

impl fmt::Debug for AutoScrollOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AutoScrollOptions")
            .field("edge", &self.edge)
            .field("speed", &self.speed)
            .field("scrollers", &self.scrollers.len())
            .finish()
    }
}

impl AutoScrollOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn edge(mut self, edge: Pixels) -> Self {
        self.edge = edge;
        self
    }

    pub fn speed(mut self, speed: Pixels) -> Self {
        self.speed = speed;
        self
    }

    pub fn on_scroll(mut self, on_scroll: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_scroll = Some(Rc::new(on_scroll));
        self
    }

    /// Adds a scroller, inside the ones added before.
    pub fn scroller(mut self, handle: &ScrollHandle) -> Self {
        self.scrollers.push(handle.clone());
        self
    }

    pub fn scrollers(mut self, scrollers: impl IntoIterator<Item = ScrollHandle>) -> Self {
        self.scrollers = scrollers.into_iter().collect();
        self
    }
}

// How fast to go along one axis: negative toward the start, by how deep into
// the edge band the pointer is.
fn velocity(at: Pixels, start: Pixels, end: Pixels, edge: Pixels, speed: Pixels) -> Pixels {
    let band = edge.min((end - start) / 3.);
    if band <= px(0.) {
        return px(0.);
    }
    if at < start + band {
        return -speed * (((start + band - at) / band).min(1.));
    }
    if at > end - band {
        return speed * (((at - (end - band)) / band).min(1.));
    }
    px(0.)
}

struct Inner {
    options: AutoScrollOptions,
    point: Option<Point<Pixels>>,
    // The generation of the tick the next frame runs; 0 when none is asked for.
    frame: u64,
    frames: u64,
}

/// What [`create_auto_scroll`] returns. Cloning it gives another handle to
/// the same scrolling.
#[derive(Clone)]
pub struct AutoScroll {
    inner: Rc<RefCell<Inner>>,
}

/// Scrolls the scrollers under a dragging pointer when it nears their edges,
/// the innermost first (`createAutoScroll`). Call
/// [`update`](AutoScroll::update) from every pointer move and
/// [`stop`](AutoScroll::stop) when the drag ends:
///
/// ```ignore
/// let scroll = create_auto_scroll(AutoScrollOptions::new().scroller(&list_scroll).on_scroll(find_target));
/// // on_move: scroll.update(sample.x, sample.y, window)
/// // on_end: scroll.stop()
/// ```
pub fn create_auto_scroll(options: AutoScrollOptions) -> AutoScroll {
    AutoScroll {
        inner: Rc::new(RefCell::new(Inner {
            options,
            point: None,
            frame: 0,
            frames: 0,
        })),
    }
}

impl AutoScroll {
    /// The pointer moved to `x`, `y` in window coordinates. Starts scrolling
    /// if it's near an edge.
    pub fn update(&self, x: Pixels, y: Pixels, window: &Window) {
        let frame = {
            let mut inner = self.inner.borrow_mut();
            inner.point = Some(Point::new(x, y));
            if inner.frame != 0 {
                return;
            }
            inner.frames += 1;
            inner.frame = inner.frames;
            inner.frame
        };
        self.schedule(frame, window);
    }

    /// Stops scrolling, like when the drag ends.
    pub fn stop(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.point = None;
        inner.frame = 0;
    }

    /// The scrollers it scrolls, innermost first, like after a list rendered
    /// another one.
    pub fn set_scrollers(&self, scrollers: Vec<ScrollHandle>) {
        self.inner.borrow_mut().options.scrollers = scrollers;
    }

    fn schedule(&self, frame: u64, window: &Window) {
        let inner = Rc::downgrade(&self.inner);
        window.on_next_frame(move |window, _cx| {
            if let Some(inner) = inner.upgrade() {
                AutoScroll { inner }.tick(frame, window);
            }
        });
    }

    fn tick(&self, frame: u64, window: &mut Window) {
        let (point, options) = {
            let mut inner = self.inner.borrow_mut();
            if inner.frame != frame {
                return;
            }
            inner.frame = 0;
            let Some(point) = inner.point else {
                return;
            };
            (point, inner.options.clone())
        };
        let mut moved = false;
        let (mut done_x, mut done_y) = (false, false);
        for handle in &options.scrollers {
            if done_x && done_y {
                break;
            }
            let bounds = handle.bounds();
            if !bounds.contains(&point) {
                continue;
            }
            let max = handle.max_offset();
            // A scroll handle's offset is the content's, so it grows more
            // negative as the element scrolls further.
            let before = handle.offset();
            let mut offset = before;
            if !done_x && max.x > px(0.) {
                let v = velocity(point.x, bounds.left(), bounds.right(), options.edge, options.speed);
                if v != px(0.) {
                    offset.x = (offset.x - v).clamp(-max.x, px(0.));
                    if offset.x != before.x {
                        done_x = true;
                        moved = true;
                    }
                }
            }
            if !done_y && max.y > px(0.) {
                let v = velocity(point.y, bounds.top(), bounds.bottom(), options.edge, options.speed);
                if v != px(0.) {
                    offset.y = (offset.y - v).clamp(-max.y, px(0.));
                    if offset.y != before.y {
                        done_y = true;
                        moved = true;
                    }
                }
            }
            if offset != before {
                handle.set_offset(offset);
            }
        }
        if !moved {
            return;
        }
        window.refresh();
        let next = {
            let mut inner = self.inner.borrow_mut();
            if inner.point.is_none() || inner.frame != 0 {
                return;
            }
            inner.frames += 1;
            inner.frame = inner.frames;
            inner.frame
        };
        // The frame this one draws lays the scroll out, which is where what's
        // under the pointer is measured: `on_scroll` runs once it's drawn,
        // then the next step.
        let inner = Rc::downgrade(&self.inner);
        window.on_next_frame(move |window, cx| {
            let Some(inner) = inner.upgrade() else {
                return;
            };
            if inner.borrow().frame != next {
                return;
            }
            if let Some(on_scroll) = &options.on_scroll {
                on_scroll(window, cx);
            }
            AutoScroll { inner }.tick(next, window);
        });
    }
}
