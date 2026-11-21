//! Port of `components/carousel/carousel.ts`: slides shown a page at a
//! time, with Previous and Next, indicators, keys, swipes and drags, and
//! optional rotation, as the WAI-ARIA carousel pattern describes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, Context, DispatchPhase, ElementId, Entity, FocusHandle, HitboxBehavior,
    InteractiveElement, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, Orientation,
    ParentElement, Pixels, Point, Role, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, canvas, point, px,
};
use ui_sdk_drag_and_drop::{DragSession, PointerEvent};
use ui_sdk_focus::{RovingFocus, RovingMember, TabStop, bind_tab_stop};
use ui_sdk_kernel::part::{FocusMode, with_role};
use ui_sdk_kernel::roles::Role as AriaRole;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, Aria, ChangeCallback, ChangeDetails, ChangeOptions,
    Controllable, Instance, Live, Part, Reason, dev, flag_while_bound, name_of, track_input,
};
use ui_sdk_text::{NameFrom, NameSources, check_name};

use crate::carousel_autoplay::{Autoplay, CarouselPause};
use crate::carousel_dom::{CarouselProbes, LaidOut, label_slide};
use crate::carousel_indicators::{INDICATOR_KEYS, indicator_roving};
use crate::carousel_keyboard::VIEWPORT_KEYS;
use crate::carousel_layout::{CarouselAlign, CarouselPage, CarouselSlideBox, CarouselSlidesPerPage};
use crate::carousel_loop::CarouselLoop;
use crate::carousel_motion::CarouselMotion;

/// Why the page changed: `PreviousPress` or `NextPress` for those parts,
/// `ItemPress` for an indicator, `ListNavigation` for arrow keys, `Swipe`
/// for a finger or a mouse drag, `Scroll` for another kind of scroll (a
/// wheel or a trackpad), `Timeout` for rotation, `ContentResize` for a
/// layout change that removed the page, or `Imperative` for an api call.
pub type CarouselPageChangeReason = Reason;

/// Why rotation started or stopped: `TogglePress` for the play toggle,
/// `FocusIn` for keyboard focus moving into the carousel, `PreviousPress`,
/// `NextPress`, `ItemPress`, `ListNavigation`, `Swipe` or `Scroll` for the
/// user moving it some other way, or `Imperative` for an api call.
pub type CarouselPlayingChangeReason = Reason;

/// Options for the page methods: the reason and event `on_page_change` receives.
pub type CarouselPageOptions = ChangeOptions;

/// Options for the rotation methods: the reason and event `on_playing_change` receives.
pub type CarouselPlayingOptions = ChangeOptions;

/// Which way the slides run, which picks the arrow keys and the scroll axis.
pub type CarouselOrientation = Orientation;

/// What `on_page_change_complete` runs, with the page the slides rest on.
pub type PageCompleteCallback = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The options `Carousel` takes.
#[derive(Clone, Default)]
pub struct CarouselOptions {
    /// The current page, from 1. Pass it to control the carousel.
    pub page: Option<usize>,
    /// The page it starts on, from 1, when uncontrolled. 1 by default.
    pub default_page: Option<usize>,
    /// Runs when the page wants to change. `details.reason` says why, and
    /// `details.cancel()` stays on the page: a swipe or drag springs back. It
    /// runs inside the carousel's update, so it may update any entity but
    /// the carousel.
    pub on_page_change: Option<ChangeCallback<usize>>,
    /// Runs once the slides come to rest on a page, after a spring, a drag
    /// or a scroll, with that page. Start work there that would stutter the
    /// motion.
    pub on_page_change_complete: Option<PageCompleteCallback>,
    /// Whether it rotates through the pages on its own. Pass it to control
    /// rotation.
    pub playing: Option<bool>,
    /// Whether it starts rotating, when uncontrolled. It doesn't when the
    /// user prefers reduced motion; the play toggle still starts it.
    pub default_playing: bool,
    /// Runs when rotation wants to start or stop. Keyboard focus moving in
    /// and the user paging stop it, as the carousel pattern asks.
    /// `details.cancel()` keeps it as it is. It runs inside the carousel's
    /// update, so it may update any entity but the carousel.
    pub on_playing_change: Option<ChangeCallback<bool>>,
    /// How long each page shows while rotating. Hovering, a hidden window and
    /// scrolling the carousel out of view pause it. 5 seconds by default.
    pub interval: Option<Duration>,
    /// Which way the slides scroll, and which arrow keys page. Horizontal by default.
    pub orientation: Option<CarouselOrientation>,
    /// Where a page lines up in the viewport. `Start` by default.
    pub align: Option<CarouselAlign>,
    /// How many slides one page holds: Next moves this many, and each
    /// indicator stands for this many. `Auto` fits as many as the viewport
    /// shows. 1 by default.
    pub slides_per_page: Option<CarouselSlidesPerPage>,
    /// Whether the slides go round: Next on the last page goes to the first,
    /// which comes in after the last, and a swipe back from the first page
    /// reaches the last. With fewer than a page of slides to either side,
    /// Next on the last page scrolls back to the first instead.
    pub r#loop: bool,
    /// Whether a fast swipe can pass over pages. Without it, one swipe moves one page.
    pub skip_snaps: bool,
    /// Whether swipes and drags coast to wherever they stop instead of a
    /// page. The nearest page becomes the current one.
    pub drag_free: bool,
    /// Whether swiping, dragging and scrolling move the slides. Without it,
    /// only the parts, keys and api calls do. True by default.
    pub drag: Option<bool>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl CarouselOptions {
    pub fn page(mut self, page: usize) -> Self {
        self.page = Some(page);
        self
    }

    pub fn default_page(mut self, page: usize) -> Self {
        self.default_page = Some(page);
        self
    }

    pub fn on_page_change(
        mut self,
        callback: impl Fn(&usize, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_page_change = Some(Rc::new(callback));
        self
    }

    pub fn on_page_change_complete(mut self, callback: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_page_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn playing(mut self, playing: bool) -> Self {
        self.playing = Some(playing);
        self
    }

    pub fn default_playing(mut self, playing: bool) -> Self {
        self.default_playing = playing;
        self
    }

    pub fn on_playing_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_playing_change = Some(Rc::new(callback));
        self
    }

    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = Some(interval);
        self
    }

    pub fn orientation(mut self, orientation: CarouselOrientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    pub fn align(mut self, align: CarouselAlign) -> Self {
        self.align = Some(align);
        self
    }

    pub fn slides_per_page(mut self, slides_per_page: impl Into<CarouselSlidesPerPage>) -> Self {
        self.slides_per_page = Some(slides_per_page.into());
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
        self
    }

    pub fn skip_snaps(mut self, skip_snaps: bool) -> Self {
        self.skip_snaps = skip_snaps;
        self
    }

    pub fn drag_free(mut self, drag_free: bool) -> Self {
        self.drag_free = drag_free;
        self
    }

    pub fn drag(mut self, drag: bool) -> Self {
        self.drag = Some(drag);
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root part's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarouselState {
    /// The current page, from 1.
    pub page: usize,
    /// How many pages there are, or `None` before the slides are measured.
    pub page_count: Option<usize>,
    /// Whether it rotates on its own (`data-playing`).
    pub playing: bool,
    /// Which way the slides scroll (`data-orientation`).
    pub orientation: CarouselOrientation,
    /// Whether rotation is paused for now (`data-paused`): hovered, the
    /// window hidden, scrolled out of view or held.
    pub paused: bool,
    /// How far the slides have scrolled, from 0 at the first page to 1 at
    /// the last, or round the loop (`--carousel-progress`), on every frame of
    /// a scroll or a spring.
    pub progress: f32,
    /// How long each page shows while rotating (`--carousel-interval`), for a
    /// progress bar timed by it.
    pub interval: Duration,
}

/// What the viewport part's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarouselViewportState {
    /// Which way the slides scroll.
    pub orientation: CarouselOrientation,
    /// Whether a mouse is dragging the slides (`data-dragging`).
    pub dragging: bool,
}

/// What the content part's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarouselContentState {
    /// Which way the slides scroll.
    pub orientation: CarouselOrientation,
}

/// What an item part's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarouselItemState {
    /// The slide's position among the slides, from 0, once it's measured.
    pub index: Option<usize>,
    /// Whether the slide belongs to the current page (`data-active`).
    pub active: bool,
    /// Whether at least half of the slide shows (`data-in-view`).
    pub in_view: bool,
    /// Whether it's out of view and holds no focus, which the original makes
    /// `inert`: it's hidden from assistive technology then. gpui can't take
    /// what's inside out of the tab order, so leave its controls out of it
    /// while this is set.
    pub inert: bool,
    /// Which way the slides scroll.
    pub orientation: CarouselOrientation,
}

/// What the Previous and Next parts' `with` read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarouselStepState {
    /// Whether there's no page that way, so pressing does nothing
    /// (`data-disabled`). It stays focusable.
    pub disabled: bool,
}

/// What the Previous part's `with` reads.
pub type CarouselPreviousState = CarouselStepState;

/// What the Next part's `with` reads.
pub type CarouselNextState = CarouselStepState;

/// What the indicator group's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarouselIndicatorGroupState {
    /// The current page, from 1.
    pub page: usize,
    /// How many pages there are, or `None` before the slides are measured.
    pub page_count: Option<usize>,
}

/// What an indicator's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarouselIndicatorState {
    /// The page it stands for, from 1, or 0 before it's known.
    pub page: usize,
    /// Whether its page is the current one (`data-selected`).
    pub selected: bool,
    /// Whether its page is past the page count, so it hides itself.
    pub hidden: bool,
}

/// What the play toggle's `with` reads. Label it from `playing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarouselPlayToggleState {
    /// Whether it rotates on its own (`data-playing`).
    pub playing: bool,
    /// Whether rotation is paused for now (`data-paused`).
    pub paused: bool,
}

// What the store subscription compares between changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reacted {
    pub page: usize,
    pub playing: bool,
    pub interval: Duration,
    pub orientation: CarouselOrientation,
    pub align: CarouselAlign,
    pub slides_per_page: CarouselSlidesPerPage,
    pub r#loop: bool,
}

/// A carousel. Its entity is its controller: `set_page`, `next`,
/// `previous`, `set_playing`, `play` and `pause` drive it from anywhere, and
/// `page`, `page_count`, `can_next`, `can_previous`, `progress` and
/// `is_playing` read it.
pub struct Carousel {
    pub(crate) instance: Instance,
    pub(crate) window: AnyWindowHandle,
    pub(crate) page: Controllable<usize>,
    pub(crate) playing: Controllable<bool>,
    pub(crate) on_page_change_complete: Option<PageCompleteCallback>,
    pub(crate) orientation: CarouselOrientation,
    pub(crate) align: CarouselAlign,
    pub(crate) slides_per_page: CarouselSlidesPerPage,
    pub(crate) r#loop: bool,
    pub(crate) skip_snaps: bool,
    pub(crate) drag_free: bool,
    pub(crate) drag: bool,
    pub(crate) interval: Duration,
    pub(crate) page_count: Option<usize>,
    // Off while the carousel moves itself, and with `drag_free`.
    pub(crate) snapping: bool,
    // Indicator keys in page order, for indicators without a `page`.
    pub(crate) indicators: Vec<SharedString>,
    // Slide keys per page, slide keys in order, slides in view, the slide
    // holding focus, and a drag.
    pub(crate) pages: Vec<Vec<SharedString>>,
    pub(crate) order: Vec<SharedString>,
    pub(crate) in_view: Vec<SharedString>,
    pub(crate) focused: Option<SharedString>,
    pub(crate) dragging: bool,
    pub(crate) layout: Vec<CarouselPage>,
    // Page indexes in the order they rest along the axis, which a looping
    // track rotates. The motion's snap points go in this order.
    pub(crate) visual: Vec<usize>,
    pub(crate) boxes: Vec<CarouselSlideBox>,
    pub(crate) max_scroll: f64,
    pub(crate) viewport_size: f64,
    pub(crate) measured: bool,
    pub(crate) measure_queued: bool,
    // Direction is read when measuring and on input, never on each frame of
    // a spring. Turning it changes the layout, so input that finds it turned
    // measures again.
    pub(crate) is_rtl: bool,
    // What moves the slides while the viewport is on the page.
    pub(crate) motion: Option<CarouselMotion>,
    // Which way a step heads, 1 or -1, for the loop to rotate toward it.
    pub(crate) heading: Option<i32>,
    // A controlled page asked for, which the app passes back as it renders.
    pub(crate) awaited: Option<usize>,
    pub(crate) seamless: CarouselLoop,
    pub(crate) autoplay: Autoplay,
    pub(crate) reacted: Reacted,
    // The slides on the page, in page order, and what the last frame and
    // the last measure laid out.
    pub(crate) slides: Vec<SharedString>,
    pub(crate) laid_out: Option<LaidOut>,
    pub(crate) measured_from: Option<LaidOut>,
    // How far a looping track's rotation shifts each slide along the axis.
    pub(crate) shifts: HashMap<SharedString, f64>,
    pub(crate) probes: Rc<RefCell<CarouselProbes>>,
    pub(crate) indicator_pages: RefCell<HashMap<SharedString, Option<usize>>>,
    pub(crate) slide_handles: RefCell<HashMap<SharedString, FocusHandle>>,
    pub(crate) roving: RovingFocus<SharedString>,
    pub(crate) drags: DragSession<Carousel>,
    pub(crate) tab_stop: TabStop,
    pub(crate) previous_focus: FocusHandle,
    pub(crate) next_focus: FocusHandle,
    pub(crate) play_focus: FocusHandle,
    pub(crate) focus_inside: bool,
    pub(crate) rotating: bool,
    // The parts development builds checked for a name, the root and each
    // indicator, once each as it first renders.
    names_checked: HashSet<ElementId>,
    _subscriptions: Vec<Subscription>,
}

impl Carousel {
    pub fn new(options: CarouselOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        track_input(window, cx);
        let mut page = Controllable::new("page", options.default_page.unwrap_or(1).max(1));
        page.set_controlled(options.page);
        page.set_on_change(options.on_page_change.clone());
        let mut playing = Controllable::new("playing", options.default_playing);
        playing.set_controlled(options.playing);
        playing.set_on_change(options.on_playing_change.clone());
        let mut instance = Instance::new("carousel").with_agent(cx);
        instance.debug = options.debug;
        // The root and the viewport leaving the page stop what they started.
        flag_while_bound(&instance, "root");
        flag_while_bound(&instance, "viewport");
        let this = cx.entity();
        let bound = cx.observe_in(&this, window, |this: &mut Self, _, window, cx| this.check_bound(window, cx));
        let visibility = cx.observe_window_visibility(window, |this: &mut Self, visibility, window, cx| {
            this.set_paused(CarouselPause::Hidden, !visibility.is_visible(), window, cx);
        });
        let orientation = options.orientation.unwrap_or(Orientation::Horizontal);
        let align = options.align.unwrap_or_default();
        let slides_per_page = options.slides_per_page.unwrap_or_default();
        let interval = options.interval.unwrap_or(Duration::from_secs(5));
        let reacted = Reacted {
            page: *page.get(),
            playing: *playing.get(),
            interval,
            orientation,
            align,
            slides_per_page,
            r#loop: options.r#loop,
        };
        let mut carousel = Self {
            instance,
            window: window.window_handle(),
            page,
            playing,
            on_page_change_complete: options.on_page_change_complete,
            orientation,
            align,
            slides_per_page,
            r#loop: options.r#loop,
            skip_snaps: options.skip_snaps,
            drag_free: options.drag_free,
            drag: options.drag.unwrap_or(true),
            interval,
            page_count: None,
            snapping: true,
            indicators: Vec::new(),
            pages: Vec::new(),
            order: Vec::new(),
            in_view: Vec::new(),
            focused: None,
            dragging: false,
            layout: Vec::new(),
            visual: Vec::new(),
            boxes: Vec::new(),
            max_scroll: 0.0,
            viewport_size: 0.0,
            measured: false,
            measure_queued: false,
            is_rtl: false,
            motion: None,
            heading: None,
            awaited: None,
            seamless: CarouselLoop::default(),
            autoplay: Autoplay::default(),
            reacted,
            slides: Vec::new(),
            laid_out: None,
            measured_from: None,
            shifts: HashMap::new(),
            probes: Rc::new(RefCell::new(CarouselProbes::default())),
            indicator_pages: RefCell::new(HashMap::new()),
            slide_handles: RefCell::new(HashMap::new()),
            roving: indicator_roving(window, cx),
            drags: DragSession::new(cx),
            tab_stop: bind_tab_stop(cx.focus_handle()),
            previous_focus: cx.focus_handle(),
            next_focus: cx.focus_handle(),
            play_focus: cx.focus_handle(),
            focus_inside: false,
            rotating: false,
            names_checked: HashSet::new(),
            _subscriptions: vec![bound, visibility],
        };
        if !window.visibility().is_visible() {
            carousel.set_paused(CarouselPause::Hidden, true, window, cx);
        }
        carousel
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). What a change sets off, like following a controlled page,
    /// runs right after.
    pub fn set_options(&mut self, options: CarouselOptions, cx: &mut Context<Self>) {
        let mut changed = self.page.set_controlled(options.page);
        changed |= self.playing.set_controlled(options.playing);
        self.page.set_on_change(options.on_page_change);
        self.playing.set_on_change(options.on_playing_change);
        self.on_page_change_complete = options.on_page_change_complete;
        let orientation = options.orientation.unwrap_or(Orientation::Horizontal);
        let align = options.align.unwrap_or_default();
        let slides_per_page = options.slides_per_page.unwrap_or_default();
        let interval = options.interval.unwrap_or(Duration::from_secs(5));
        let drag = options.drag.unwrap_or(true);
        changed |= self.orientation != orientation
            || self.align != align
            || self.slides_per_page != slides_per_page
            || self.r#loop != options.r#loop
            || self.skip_snaps != options.skip_snaps
            || self.drag_free != options.drag_free
            || self.drag != drag
            || self.interval != interval;
        self.orientation = orientation;
        self.align = align;
        self.slides_per_page = slides_per_page;
        self.r#loop = options.r#loop;
        self.skip_snaps = options.skip_snaps;
        self.drag_free = options.drag_free;
        self.drag = drag;
        self.interval = interval;
        self.instance.debug = options.debug;
        if !changed {
            return;
        }
        cx.notify();
        let window = self.window;
        cx.spawn(async move |this, cx| {
            window
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| this.react(window, cx)).ok();
                })
                .ok();
        })
        .detach();
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub(crate) fn set_paused(&mut self, cause: CarouselPause, paused: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.autoplay.set_paused(cause, paused, window, cx) {
            cx.notify();
        }
    }

    // A slide's focus handle, made the first time it's asked for.
    pub(crate) fn slide_handle(&self, key: &SharedString, cx: &App) -> FocusHandle {
        self.slide_handles
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// A slide's focus handle, if it has one.
    pub(crate) fn slide_focus(&self, key: &SharedString) -> Option<FocusHandle> {
        self.slide_handles.borrow().get(key).cloned()
    }

    /// The slide that holds focus, or something inside it.
    pub(crate) fn focused_slide(&self, window: &Window, cx: &App) -> Option<SharedString> {
        let handles = self.slide_handles.borrow();
        self.slides
            .iter()
            .find(|key| handles.get(*key).is_some_and(|handle| handle.contains_focused(window, cx)))
            .cloned()
    }

    // How far the content shifts for the scroll position and a drag's stretch.
    fn content_shift(&self) -> Point<Pixels> {
        let Some(motion) = &self.motion else {
            return Point::default();
        };
        let along = px((motion.position + motion.overshoot) as f32);
        if self.orientation == Orientation::Vertical {
            point(px(0.), -along)
        } else if self.is_rtl {
            point(along, px(0.))
        } else {
            point(-along, px(0.))
        }
    }

    // How far a slide shifts for a looping track's rotation.
    fn slide_shift(&self, key: &SharedString) -> Point<Pixels> {
        let along = px(self.shifts.get(key).copied().unwrap_or(0.0) as f32);
        if self.orientation == Orientation::Vertical {
            point(px(0.), along)
        } else if self.is_rtl {
            point(-along, px(0.))
        } else {
            point(along, px(0.))
        }
    }

    fn content_id(&self) -> gpui::ElementId {
        self.instance.element_id("content", None)
    }

    // Whether development builds still check that something names the part
    // of `slot`, with `key`, which they do once, as it first renders.
    fn checks_name(&self, slot: &'static str, key: Option<&str>) -> bool {
        dev::DEV && !self.names_checked.contains(&self.instance.element_id(slot, key))
    }

    // A part rendered: development builds check, once, that something names
    // it (`checkName`), from the name it rendered with. A labelled-by names
    // it, since its targets' text can't be read.
    fn check_name(&mut self, slot: &'static str, key: Option<&str>, role: AriaRole, part: &str, window: &Window) {
        if !self.checks_name(slot, key) {
            return;
        }
        let Some(name) = name_of(&self.instance, slot, key) else {
            return;
        };
        let id = self.instance.element_id(slot, key);
        self.names_checked.insert(id.clone());
        if !name.labelled_by.is_empty() {
            return;
        }
        let mut sources = NameSources::new(role);
        if let Some(label) = name.aria_label {
            sources = sources.aria_label(label);
        }
        let from = NameFrom {
            component: "carousel",
            id: Some(self.instance.id().clone()),
        };
        check_name(&id, &sources, part, from, window);
    }

    /// The `root` part: a region described as a carousel. Name it with
    /// `aria_label`, like "Featured products".
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<CarouselState> {
        let carousel = this.read(cx);
        let state = CarouselState {
            page: *carousel.page.get(),
            page_count: carousel.page_count,
            playing: *carousel.playing.get(),
            orientation: carousel.orientation,
            paused: *carousel.playing.get() && carousel.autoplay.is_paused(),
            progress: carousel.progress() as f32,
            interval: carousel.interval,
        };
        let probes = carousel.probes.clone();
        let entity = this.downgrade();
        let hover = this.downgrade();
        let probe = canvas(
            move |bounds, window, _cx| {
                let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                let shown = window.content_mask().bounds.intersect(&bounds);
                let on_screen = shown.size.width > px(0.) && shown.size.height > px(0.);
                let mut probes = probes.borrow_mut();
                probes.hitbox = Some(hitbox.clone());
                probes.on_screen = Some(on_screen);
                hitbox
            },
            move |_bounds, hitbox, window, _cx| {
                let moved = hover.clone();
                window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || window.last_input_was_touch() {
                        return;
                    }
                    let hovering = hitbox.is_hovered_at(window.mouse_position(), window);
                    moved
                        .update(cx, |this, cx| this.set_paused(CarouselPause::Hover, hovering, window, cx))
                        .ok();
                });
                let left = hover;
                window.on_mouse_event(move |_: &MouseExitEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        left.update(cx, |this, cx| this.set_paused(CarouselPause::Hover, false, window, cx))
                            .ok();
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let mut part = Part::new(&carousel.instance, "root", None, state)
            .aria_role(AriaRole::Region)
            .aria(Aria::new().role_description("carousel"))
            .relative()
            .child(probe)
            .child(carousel.drags.listener());
        // Development builds check that something names it.
        if carousel.checks_name("root", None) {
            part = part.keep_name();
        }
        part.on_props(move |window, cx| {
            entity
                .update(cx, |this, cx| {
                    this.check_name("root", None, AriaRole::Region, "Carousel", window);
                    this.after_frame(window, cx);
                })
                .ok();
        })
    }

    /// The `viewport` part: the window the slides scroll in, which clips them.
    /// It's a tab stop for the arrow keys, which Tab passes over while
    /// something inside takes focus.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> Part<CarouselViewportState> {
        let carousel = this.read(cx);
        let state = CarouselViewportState {
            orientation: carousel.orientation,
            dragging: carousel.dragging,
        };
        let probes = carousel.probes.clone();
        let probe = canvas(
            move |bounds, _window, _cx| probes.borrow_mut().viewport = Some(bounds),
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let tab_stop = carousel.tab_stop.clone();
        let keyed = this.downgrade();
        let pressed = this.downgrade();
        let scrolled = this.downgrade();
        Part::new(&carousel.instance, "viewport", None, state)
            .map_el(|el| tab_stop.bind(el, true))
            .relative()
            .overflow_hidden()
            .child(probe)
            .on_keys(VIEWPORT_KEYS, move |event, window, cx| {
                keyed
                    .update(cx, |this, cx| this.on_viewport_key_down(event, window, cx))
                    .ok();
            })
            .on_any_mouse_down(move |event: &MouseDownEvent, window, cx| {
                let event = PointerEvent::mouse_down(event, window, cx);
                pressed
                    .update(cx, |this, cx| {
                        this.follow_direction(window, cx);
                        this.motion_pointer_down(&event, window, cx);
                    })
                    .ok();
            })
            .on_scroll(move |event, window, cx| {
                scrolled
                    .update(cx, |this, cx| this.viewport_scroll(event, window, cx))
                    .ok();
            })
    }

    /// The `content` part: the row of slides, a column when vertical, which
    /// moves as the slides scroll. Slides coming into view are read out,
    /// except while rotating, as the carousel pattern asks.
    pub fn content(this: &Entity<Self>, cx: &App) -> Part<CarouselContentState> {
        let carousel = this.read(cx);
        let state = CarouselContentState {
            orientation: carousel.orientation,
        };
        let shift = carousel.content_shift();
        let probes = carousel.probes.clone();
        let probe = canvas(
            move |bounds, _window, _cx| {
                let laid = gpui::Bounds::new(bounds.origin - shift, bounds.size);
                probes.borrow_mut().content = Some(laid);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let live = if *carousel.playing.get() { Live::Off } else { Live::Polite };
        let part = Part::new(&carousel.instance, "content", None, state)
            .map_el(|el| with_role(el, Role::Group))
            .aria(Aria::new().live(live))
            .size_full()
            .flex()
            .relative()
            .left(shift.x)
            .top(shift.y)
            .child(probe);
        match (carousel.orientation, carousel.is_rtl) {
            (Orientation::Vertical, _) => part.flex_col(),
            // Right to left, the row starts at the right.
            (Orientation::Horizontal, true) => part.flex_row_reverse(),
            (Orientation::Horizontal, false) => part,
        }
    }

    /// An `item` part: one slide, by its key, a group described as a slide
    /// and named by its position, like "2 of 5", unless the app gives it an
    /// `aria_label` of its own. Size it along the axis, like a full width for
    /// one to a view. Slides less than half in view are hidden from
    /// assistive technology, except one that holds focus.
    pub fn item(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part<CarouselItemState> {
        let carousel = this.read(cx);
        let key: SharedString = key.into();
        CarouselProbes::rendered(&mut carousel.probes.borrow_mut().item_order, &key);
        let index = carousel.order.iter().position(|slide| *slide == key);
        let active = carousel
            .pages
            .get(carousel.page.get().wrapping_sub(1))
            .is_some_and(|slides| slides.contains(&key));
        let in_view = carousel.in_view.contains(&key);
        let inert = carousel.page_count.is_some() && !in_view && carousel.focused.as_ref() != Some(&key);
        let state = CarouselItemState {
            index,
            active,
            in_view,
            inert,
            orientation: carousel.orientation,
        };
        let handle = carousel.slide_handle(&key, cx);
        let viewport = carousel.tab_stop.handle().clone();
        let shift = carousel.content_shift() + carousel.slide_shift(&key);
        let own = carousel.slide_shift(&key);
        let probes = carousel.probes.clone();
        let measured = key.clone();
        let probe = canvas(
            move |bounds, window, _cx| {
                let laid = gpui::Bounds::new(bounds.origin - shift, bounds.size);
                probes.borrow_mut().items.insert(measured, laid);
                window.insert_hitbox(bounds, HitboxBehavior::Normal)
            },
            move |_bounds, hitbox, window, _cx| {
                // A press on a slide focuses the viewport, the focusable
                // element around it, as a browser does, rather than the slide,
                // which only the keys focus.
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble
                        && event.button == MouseButton::Left
                        && hitbox.is_hovered(window)
                        && !window.default_prevented()
                    {
                        window.focus(&viewport, cx);
                        window.prevent_default();
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let mut part = Part::new(&carousel.instance, "item", Some(&key), state)
            .aria_role(AriaRole::Group)
            .aria(Aria::new().role_description("slide").hidden(inert))
            .map_el(|el| el.track_focus(&handle.tab_stop(false)))
            .flex_shrink_0()
            .relative()
            .left(own.x)
            .top(own.y)
            .child(probe);
        if let Some(index) = index {
            part = part.aria_label(label_slide(index, carousel.order.len()));
        }
        part
    }

    fn step_part(this: &Entity<Self>, by: i32, cx: &App) -> Part<CarouselStepState> {
        let carousel = this.read(cx);
        let disabled = !carousel.can_step(by);
        let (slot, handle, reason) = if by < 0 {
            ("previous", &carousel.previous_focus, Reason::PreviousPress)
        } else {
            ("next", &carousel.next_focus, Reason::NextPress)
        };
        let entity = this.downgrade();
        Part::new(&carousel.instance, slot, None, CarouselStepState { disabled })
            .aria_role(AriaRole::Button)
            .focus_mode(FocusMode::Native, Some(handle), false)
            .aria(Aria::new().controls(carousel.content_id()).disabled(disabled))
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |this, cx| {
                        let options = ChangeOptions::event_of(reason, event);
                        if by < 0 {
                            this.previous(options, window, cx);
                        } else {
                            this.next(options, window, cx);
                        }
                    })
                    .ok();
            })
    }

    /// The `previous` part: a button that shows the previous page. It stays
    /// focusable at the start, reporting itself disabled, so focus never
    /// drops to the page. Give it text, or an `aria_label` when it holds an icon.
    pub fn previous_part(this: &Entity<Self>, cx: &App) -> Part<CarouselPreviousState> {
        Self::step_part(this, -1, cx)
    }

    /// The `next` part: a button that shows the next page, like Previous.
    pub fn next_part(this: &Entity<Self>, cx: &App) -> Part<CarouselNextState> {
        Self::step_part(this, 1, cx)
    }

    /// The `indicator-group` part: a tablist with one tab stop on the current
    /// page's indicator. Arrows move between the indicators and show their
    /// page, Home and End go to the ends. Name it with `aria_label`.
    pub fn indicator_group(this: &Entity<Self>, cx: &App) -> Part<CarouselIndicatorGroupState> {
        let carousel = this.read(cx);
        carousel.roving.begin(cx);
        let state = CarouselIndicatorGroupState {
            page: *carousel.page.get(),
            page_count: carousel.page_count,
        };
        let entity = this.downgrade();
        Part::new(&carousel.instance, "indicator-group", None, state)
            .aria_role(AriaRole::TabList)
            .on_keys(INDICATOR_KEYS, move |event, window, cx| {
                entity
                    .update(cx, |this, cx| this.on_indicator_key_down(event, window, cx))
                    .ok();
            })
    }

    /// An `indicator` part, by its key: a tab that shows its `page`, from 1,
    /// or, without one, the page its place among the indicators stands for.
    /// It controls its page's slides. One past the page count hides itself,
    /// so one per slide works however many slides fit. Give it an
    /// `aria_label`, like the slide's name.
    pub fn indicator(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        page: Option<usize>,
        cx: &App,
    ) -> Part<CarouselIndicatorState> {
        let carousel = this.read(cx);
        let key: SharedString = key.into();
        CarouselProbes::rendered(&mut carousel.probes.borrow_mut().indicator_order, &key);
        carousel.indicator_pages.borrow_mut().insert(key.clone(), page);
        let current = *carousel.page.get();
        let indicator_page = carousel.indicator_page(&key, page);
        let selected = indicator_page > 0 && indicator_page == current;
        let hidden = carousel.page_count.is_some_and(|count| indicator_page > count);
        let handle = carousel.roving.focus_handle(&key, cx);
        let tab_stop = carousel.roving.bind(
            key.clone(),
            &handle,
            RovingMember::new()
                .focusable(!hidden)
                .preferred(indicator_page == current)
                .claims(indicator_page == 0 || indicator_page == current),
            cx,
        );
        let mut aria = Aria::new().hidden(hidden);
        if let Some(slides) = carousel.pages.get(indicator_page.wrapping_sub(1)) {
            for slide in slides {
                aria = aria.controls(carousel.instance.element_id("item", Some(slide)));
            }
        }
        let state = CarouselIndicatorState {
            page: indicator_page,
            selected,
            hidden,
        };
        let entity = this.downgrade();
        let pressed = key.clone();
        let mut part = Part::new(&carousel.instance, "indicator", Some(&key), state)
            .aria_role(AriaRole::Tab)
            .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
            .aria_selected(selected)
            .aria(aria)
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |this, cx| {
                        let page = this.indicator_pages.borrow().get(&pressed).copied().flatten();
                        this.press_indicator(&pressed, page, event, window, cx);
                    })
                    .ok();
            });
        // Development builds check that something names it.
        if carousel.checks_name("indicator", Some(&key)) {
            let entity = this.downgrade();
            part = part.keep_name().on_props(move |window, cx| {
                entity
                    .update(cx, |this, _| {
                        this.check_name("indicator", Some(&key), AriaRole::Tab, "CarouselIndicator", window);
                    })
                    .ok();
            });
        }
        if hidden { part.hidden() } else { part }
    }

    /// The `play-toggle` part: a button that starts and stops rotation. Label
    /// it from `playing` in its state.
    pub fn play_toggle(this: &Entity<Self>, cx: &App) -> Part<CarouselPlayToggleState> {
        let carousel = this.read(cx);
        let playing = *carousel.playing.get();
        let state = CarouselPlayToggleState {
            playing,
            paused: carousel.autoplay.is_paused(),
        };
        let entity = this.downgrade();
        Part::new(&carousel.instance, "play-toggle", None, state)
            .aria_role(AriaRole::Button)
            .focus_mode(FocusMode::Native, Some(&carousel.play_focus), false)
            .aria(Aria::new().controls(carousel.content_id()))
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |this, cx| {
                        let playing = !*this.playing.get();
                        this.set_playing(playing, ChangeOptions::event_of(Reason::TogglePress, event), window, cx);
                    })
                    .ok();
            })
    }
}

/// `agent: { actions: ["setPage", "next", "previous", "setPlaying", "play",
/// "pause"] }` and `controlled: ["page", "playing"]`.
impl Agent for Carousel {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setPage", |carousel, args, window, cx| {
            let page = args.number(0)?;
            let page = if page.is_finite() && page >= 1.0 { page as usize } else { 1 };
            Ok(carousel.set_page(page, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("next", |carousel, _args, window, cx| {
            Ok(carousel.next(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("previous", |carousel, _args, window, cx| {
            Ok(carousel.previous(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setPlaying", |carousel, args, window, cx| {
            let playing = args.boolean(0)?;
            Ok(carousel.set_playing(playing, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("play", |carousel, _args, window, cx| {
            Ok(carousel.play(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("pause", |carousel, _args, window, cx| {
            Ok(carousel.pause(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["page", "playing"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("page", *self.page.get())
            .with("playing", *self.playing.get())
    }
}
