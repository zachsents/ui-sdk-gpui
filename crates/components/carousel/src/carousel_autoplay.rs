//! Port of `components/carousel/carousel-autoplay.ts`: a carousel's rotation
//! timer. Each page gets the full interval once it arrives, and pauses keep
//! the time left, so a progress bar timed by the root's `interval` and
//! paused by its `paused` stays in step with it. Pure timing: the carousel
//! decides what pauses it and what a tick does.

use std::time::{Duration, Instant};

use gpui::{Context, Task, Window};

use crate::carousel::Carousel;

/// Why rotation is paused for now. None of these is a change to `playing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CarouselPause {
    /// A mouse is over the carousel.
    Hover,
    /// The window is hidden.
    Hidden,
    /// The carousel is scrolled out of view.
    Offscreen,
    /// A finger is down on the slides.
    Touch,
    /// A mouse is dragging the slides.
    Drag,
}

/// The rotation timer (`createAutoplay`).
pub struct Autoplay {
    pauses: Vec<CarouselPause>,
    running: bool,
    timer: Option<Task<()>>,
    // Time left on the current page, and when the timer started counting it down.
    remaining: Duration,
    started_at: Instant,
}

impl Default for Autoplay {
    fn default() -> Self {
        Self {
            pauses: Vec::new(),
            running: false,
            timer: None,
            remaining: Duration::ZERO,
            started_at: Instant::now(),
        }
    }
}

impl Autoplay {
    /// Whether rotation is paused for now.
    pub fn is_paused(&self) -> bool {
        self.running && !self.pauses.is_empty()
    }

    pub(crate) fn arm(&mut self, window: &mut Window, cx: &mut Context<Carousel>) {
        self.timer = None;
        if !self.running || !self.pauses.is_empty() {
            return;
        }
        self.started_at = Instant::now();
        let remaining = self.remaining;
        self.timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(remaining).await;
            this.update_in(cx, |this, window, cx| this.autoplay_tick(window, cx))
                .ok();
        }));
    }

    /// The timer fired: the next page gets a full interval.
    pub(crate) fn ticked(&mut self, interval: Duration) {
        self.timer = None;
        self.remaining = interval;
    }

    /// Pauses or resumes for `cause`. Returns whether it paused or resumed.
    pub fn set_paused(
        &mut self,
        cause: CarouselPause,
        paused: bool,
        window: &mut Window,
        cx: &mut Context<Carousel>,
    ) -> bool {
        let was = self.is_paused();
        if paused == self.pauses.contains(&cause) {
            return false;
        }
        if paused {
            // Keep what's left of this page's time.
            if self.pauses.is_empty() && self.running && self.timer.is_some() {
                self.remaining = self.remaining.saturating_sub(self.started_at.elapsed());
            }
            self.pauses.push(cause);
            self.timer = None;
        } else {
            self.pauses.retain(|pause| *pause != cause);
            self.arm(window, cx);
        }
        self.is_paused() != was
    }

    /// Starts or stops rotating. Starting gives the current page a full
    /// interval. Returns whether it paused or resumed.
    pub fn set_running(
        &mut self,
        value: bool,
        interval: Duration,
        window: &mut Window,
        cx: &mut Context<Carousel>,
    ) -> bool {
        if value == self.running {
            return false;
        }
        let was = self.is_paused();
        self.running = value;
        self.remaining = interval;
        self.arm(window, cx);
        self.is_paused() != was
    }

    /// The page changed: it gets a full interval from now.
    pub fn restart(&mut self, interval: Duration, window: &mut Window, cx: &mut Context<Carousel>) {
        self.remaining = interval;
        self.arm(window, cx);
    }

    pub fn destroy(&mut self) {
        self.running = false;
        self.timer = None;
    }
}
