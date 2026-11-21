//! Port of `components/carousel`: slides shown a page at a time, with
//! Previous and Next, indicators, keys, swipes and drags, and optional
//! rotation, as the WAI-ARIA carousel pattern describes.
//!
//! | Original | Here |
//! | --- | --- |
//! | `carousel.ts` | [`carousel`]: [`Carousel`], its options, reasons, parts and states |
//! | `carousel-api.ts` | [`carousel_api`]: paging, rotation, measuring and moving the slides |
//! | `carousel-motion.ts` | [`carousel_motion`]: drags, springs and the user's own scrolling |
//! | `carousel-loop.ts` | [`carousel_loop`]: the seamless loop |
//! | `carousel-layout.ts` | [`carousel_layout`]: the pure geometry |
//! | `carousel-dom.ts` | [`carousel_dom`]: layout reads, slide labels, a rotation's shifts |
//! | `carousel-indicators.ts` | [`carousel_indicators`]: the indicators' tablist |
//! | `carousel-keyboard.ts` | [`carousel_keyboard`]: the viewport's keys |
//! | `carousel-autoplay.ts` | [`carousel_autoplay`]: the rotation timer |
//!
//! # In gpui
//!
//! The original's viewport is a native scroller with CSS scroll snap doing
//! the paging. gpui has no scroll snapping, so the carousel scrolls the
//! slides itself: the viewport clips them, the content shifts by the scroll
//! position, and wheels, trackpads and touch pans move it as a browser
//! scrolls one, contained along the axis. When they rest, the slides spring
//! to where mandatory scroll snap would put them: the next page the way a
//! wheel's lines head, or the page nearest where a precise scroll rests, one
//! page on at most without `skip_snaps`. Mouse drags follow the pointer
//! through a drag session and spring from the release velocity, with the
//! motion crate's machine, velocity tracker, projection and springs, and
//! programmatic moves spring from wherever the slides are. Every step runs on
//! gpui's animation frames, and reduced motion (`cx.reduce_motion()`) lands
//! at once.
//!
//! The content fills the viewport and lays its slides out in a row (a
//! column when vertical), so a slide sized to the full width is one page.
//! Right to left is the app's direction (`ui_sdk_platform::is_rtl`): the row
//! starts at the right, and moves the other way. A looping track rotates
//! its slides by shifting each from where the row lays it out, since gpui
//! has no CSS `order`; they stay in their own order in the tree.
//!
//! Each frame that draws the carousel reports where its parts laid out, and
//! the carousel measures when that changed, as the original's resize
//! observers do; it also follows hover, the window's visibility, whether the
//! root shows inside the clips around it, and focus, which gpui tells after
//! the frame.
//!
//! Two timings differ. A drag's moves land as the drag session reports them,
//! once a frame, rather than a frame later. A controlled page that stayed
//! moves back after the app's next render, where it passes the page, rather
//! than a microtask after the change.
//!
//! # Dropped, and why
//!
//! - `createCarouselController`, the `controller` option and the parts'
//!   `controller` prop: the entity is the controller, and parts outside the
//!   carousel take it like the ones inside.
//! - CSS scroll snap, `scroll-snap-type`, `scroll-snap-align`,
//!   `scroll-snap-stop`, the snap areas written for several slides to a page
//!   (`writeSnapAreas`, `clearSnapAreas`), scroll padding and
//!   `overscroll-behavior`: gpui has none. The carousel settles the user's
//!   scroll itself, and contains it along the axis.
//! - `ignoredTargets` and `keepsArrows`, which read an event's target: gpui
//!   can't. A text field stops the press that would start a drag itself, and
//!   keeps its keys (`ui_sdk_native::is_typing`); widgets that move with
//!   arrows prevent the keys they use, which the viewport checks first. A
//!   `data-carousel-no-drag` element has no gpui counterpart.
//! - Pointer capture, `getCoalescedEvents`, clearing the selection and
//!   `user-select` during a press, and `dragstart` prevention: the drag
//!   session follows the pointer anywhere, once a frame, and gpui selects no
//!   text in parts or drags images natively.
//! - `scrollend`: a scroll rests after 120ms without one, and not while a
//!   finger or a trackpad's fingers are down.
//! - Focus or find in page bringing a slide into view: gpui doesn't scroll
//!   focused elements into view, and has no find in page.
//! - The `inert` attribute on slides out of view: gpui can't make a subtree
//!   inert. The slide is hidden from assistive technology, its state says
//!   `inert`, and the app leaves its controls out of the tab order.
//! - `checkPartName`, the development warning for a Previous, Next or play
//!   toggle nobody named: a button also takes its name from a `text!`
//!   child, which the carousel can't read, so it would warn about every
//!   button named by its text.
//! - Indicators that are links (`a[href]`, `opensElsewhere`): gpui has no
//!   links; an indicator is a tab.
//! - The `IntersectionObserver` and `visibilitychange`: the root's probe asks
//!   whether it shows inside every clip around it, and the window's
//!   visibility observer whether the window does.
//! - `labelSlide`'s bookkeeping of the labels it wrote: the app's own
//!   `aria_label` comes after the slide's and wins.
//! - A press on a slide focuses the viewport, the focusable element around
//!   it, as a browser does; the slide itself takes focus only from the keys.

pub mod carousel;
pub mod carousel_api;
pub mod carousel_autoplay;
pub mod carousel_dom;
pub mod carousel_indicators;
pub mod carousel_keyboard;
pub mod carousel_layout;
pub mod carousel_loop;
pub mod carousel_motion;

pub use carousel::{
    Carousel, CarouselContentState, CarouselIndicatorGroupState, CarouselIndicatorState,
    CarouselItemState, CarouselNextState, CarouselOptions, CarouselOrientation,
    CarouselPageChangeReason, CarouselPageOptions, CarouselPlayToggleState,
    CarouselPlayingChangeReason, CarouselPlayingOptions, CarouselPreviousState, CarouselState,
    CarouselStepState, CarouselViewportState, PageCompleteCallback,
};
pub use carousel_api::CarouselMeasure;
pub use carousel_autoplay::CarouselPause;
pub use carousel_layout::{CarouselAlign, CarouselSlidesPerPage};
