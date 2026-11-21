//! Port of `foundation/popups/sheet-driver.ts`: the boundary every sheet
//! driver implements. A driver owns the sheet's motion and its input, and
//! feeds the shared machine; [`attach_sheet`](crate::attach_sheet) owns
//! everything else (geometry, effects, modality, focus) and hears back
//! through these callbacks. Swapping drivers changes how the sheet moves,
//! nothing else.
//!
//! The original hands a settle to the compositor as a WAAPI animation, with
//! the spring sampled into `linear()`, or to native scrolling. gpui draws
//! each frame itself, so a settle is a [`SettleMotion`] the driver samples on
//! every frame while it runs, and the effects follow the same extent.

use std::rc::Rc;

use gpui::{App, Context, Pixels, Point, SharedString, Window};
use ui_sdk_kernel::InputEvent;
use ui_sdk_motion::{SheetConfig, SheetSnapReason, timestamp};
use ui_sdk_platform::{PhysicalSide, Region};

use crate::sheet_axis::ScrollArea;

/// How a sheet's motion is driven (`SheetDriverName`). `Auto` picks scroll
/// snap for a bottom sheet on a touch screen, and the pointer driver
/// otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SheetDriverName {
    Pointer,
    ScrollSnap,
    #[default]
    Auto,
}

impl SheetDriverName {
    /// The original's name for it, its `data-driver`.
    pub fn as_str(self) -> &'static str {
        match self {
            SheetDriverName::Pointer => "pointer",
            SheetDriverName::ScrollSnap => "scroll-snap",
            SheetDriverName::Auto => "auto",
        }
    }
}

/// An open starts on the sheet's first drawn frame, not at a time picked in
/// advance: the frame that builds the sheet can be long, and a start time
/// from before it skips the rise's first frames. So the open's motion is made
/// at once, held closed by a start time this far ahead, and moved to the time
/// of the sheet's first drawn frame by [`after_first_frame`] (`openHold`).
pub const OPEN_HOLD: f64 = 1000.0;

/// Runs `start` with the time of the frame after the sheet's first: the
/// first frame callback runs before that frame's layout. Callers check the
/// open is still the motion in progress, so a late frame after a newer motion
/// or a destroy does nothing (`afterFirstFrame`).
pub fn after_first_frame(window: &Window, start: impl FnOnce(f64, &mut Window, &mut App) + 'static) {
    window.on_next_frame(move |window, _cx| {
        window.on_next_frame(move |window, cx| start(timestamp(), window, cx));
    });
}

/// How far a closed bottom sheet goes past the window's bottom so it never
/// shows there, like under Safari's floating toolbar on iOS. Other sides only
/// need to clear a shadow (`toolbarBleed`).
pub const TOOLBAR_BLEED: f64 = 120.0;

/// Where a closed sheet sits, past its edge (`closedExtentFor`).
pub fn closed_extent_for(side: PhysicalSide) -> f64 {
    if side == PhysicalSide::Bottom {
        -TOOLBAR_BLEED
    } else {
        -32.0
    }
}

/// How long a sheet settles under reduced motion, without overshoot
/// (`reducedSettle`).
pub const REDUCED_SETTLE: f64 = 200.0;

/// How long a sheet fades in or out under reduced motion (`reducedFade`).
pub const REDUCED_FADE: f64 = 150.0;

/// A settle the sheet runs on its own: where it is at each ms after it
/// starts, with the fade that stands in for it under reduced motion.
#[derive(Clone)]
pub struct SettleMotion {
    /// Where the sheet is, `elapsed` ms after the start.
    pub extent_at: Rc<dyn Fn(f64) -> f64>,
    /// When it starts, on the input clock. An open moves it to the sheet's
    /// first frame.
    pub start_time: f64,
    pub duration: f64,
    /// Opacity from and to, when it fades instead of moving.
    pub fade: Option<(f64, f64)>,
}

impl std::fmt::Debug for SettleMotion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettleMotion")
            .field("start_time", &self.start_time)
            .field("duration", &self.duration)
            .field("fade", &self.fade)
            .finish_non_exhaustive()
    }
}

impl SettleMotion {
    fn elapsed(&self, now: f64) -> f64 {
        (now - self.start_time).clamp(0.0, self.duration)
    }

    /// Where the sheet is at `now`.
    pub fn extent(&self, now: f64) -> f64 {
        (self.extent_at)(self.elapsed(now))
    }

    /// Its opacity at `now`, while it fades.
    pub fn opacity(&self, now: f64) -> Option<f64> {
        let (from, to) = self.fade?;
        let progress = if self.duration > 0.0 {
            self.elapsed(now) / self.duration
        } else {
            1.0
        };
        Some(from + (to - from) * progress)
    }

    /// Whether it has run its course at `now`.
    pub fn finished(&self, now: f64) -> bool {
        now - self.start_time >= self.duration
    }
}

/// Runs inside the component's update.
pub type SheetHook<T, A> = Rc<dyn Fn(&mut T, A, &mut Window, &mut Context<T>)>;

/// What a driver reads from its sheet and tells it (`SheetDriverOptions`).
pub struct SheetDriverOptions<T: 'static> {
    /// The full-window element the sheet sits in.
    pub host: Region,
    pub side: Rc<dyn Fn() -> PhysicalSide>,
    /// The machine's geometry and physics, read on every event.
    pub config: Rc<dyn Fn() -> SheetConfig>,
    /// The sheet's own size along its axis, measured at the last layout.
    pub size: Rc<dyn Fn() -> f64>,
    /// Where a closed sheet sits: just past the edge.
    pub closed_extent: Rc<dyn Fn() -> f64>,
    pub reduced_motion: Rc<dyn Fn(&App) -> bool>,
    /// Whether this sheet, and not one stacked above it, owns a press at a
    /// point.
    pub owns: Rc<dyn Fn(Point<Pixels>, &Window) -> bool>,
    /// Whether a mouse press at a point may start a drag, like on the handle
    /// or empty space.
    pub can_mouse_drag: Rc<dyn Fn(Point<Pixels>, &Window, &App) -> bool>,
    /// The scroll areas inside the sheet.
    pub scroll_areas: Rc<dyn Fn() -> Vec<ScrollArea>>,
    /// The resting snap point changes, at release or when a programmatic
    /// move starts.
    pub on_snap: Rc<dyn Fn(&mut T, SharedString, SheetSnapReason, Option<&InputEvent>, &mut Window, &mut Context<T>)>,
    /// The sheet came to rest on a snap point, or closed (`None`).
    pub on_rest: SheetHook<T, Option<SharedString>>,
    /// A swipe dismissed the sheet; it's already settling closed.
    pub on_dismiss: Rc<dyn Fn(&mut T, Option<&InputEvent>, &mut Window, &mut Context<T>)>,
    /// A swipe tried to dismiss a sheet that can't be dismissed.
    pub on_dismiss_attempt: Rc<dyn Fn(&mut T, Option<&InputEvent>, &mut Window, &mut Context<T>)>,
    pub on_drag_change: SheetHook<T, bool>,
    /// Whether resting at a snap point leaves the window usable, so presses
    /// beside the sheet reach it.
    pub is_interactive: Rc<dyn Fn(&SharedString) -> bool>,
    /// A frame the sheet moved in. Effects follow it.
    pub on_frame: Rc<dyn Fn(f64, &mut App)>,
    /// The sheet started moving on its own.
    pub on_motion: Rc<dyn Fn(&SettleMotion)>,
}

impl<T: 'static> Clone for SheetDriverOptions<T> {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            side: self.side.clone(),
            config: self.config.clone(),
            size: self.size.clone(),
            closed_extent: self.closed_extent.clone(),
            reduced_motion: self.reduced_motion.clone(),
            owns: self.owns.clone(),
            can_mouse_drag: self.can_mouse_drag.clone(),
            scroll_areas: self.scroll_areas.clone(),
            on_snap: self.on_snap.clone(),
            on_rest: self.on_rest.clone(),
            on_dismiss: self.on_dismiss.clone(),
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            on_drag_change: self.on_drag_change.clone(),
            is_interactive: self.is_interactive.clone(),
            on_frame: self.on_frame.clone(),
            on_motion: self.on_motion.clone(),
        }
    }
}
