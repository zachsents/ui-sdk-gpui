//! Port of `foundation/platform/viewport.ts`: what a phone's screen leaves
//! for the window: the on-screen keyboard over the layout viewport's bottom,
//! the visual viewport a platform pans to keep a field in view, and the safe
//! areas around a notch or home indicator.
//!
//! The original reads safe areas through a hidden element padded with
//! `env()`, and follows the viewport with shared listeners. gpui reports
//! safe areas in the window's insets, and redraws whenever the viewport
//! changes, so a binding measures as its element prepaints, like the
//! keyboard tracker.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, Edges, ParentElement, Pixels, Styled, Window, px};

use crate::keyboard::{keyboard_height, keyboard_inset, measure_viewport};
use crate::layout::probe;

/// What [`bind_viewport`] reports.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportState {
    /// The on-screen keyboard's height while it's open. 0 without one.
    pub keyboard: Pixels,
    /// How much of the layout viewport's bottom the keyboard covers: less
    /// while the platform pans the window, and 0 when it resizes the window
    /// instead.
    pub keyboard_inset: Pixels,
    /// The visual viewport's width: what's on screen.
    pub width: Pixels,
    /// The visual viewport's height: what's on screen, above the keyboard.
    pub height: Pixels,
    /// How far the visual viewport is scrolled down the layout viewport, like
    /// when a platform pans a focused field into view.
    pub offset_top: Pixels,
    /// How far the visual viewport is scrolled across the layout viewport.
    pub offset_left: Pixels,
    /// The pinch-zoom scale. gpui never zooms a window's content, so it's 1.
    pub scale: f32,
    /// The safe-area insets around a notch, rounded corners or a home
    /// indicator. All 0 where the platform has none.
    pub safe_area: Edges<Pixels>,
}

impl ViewportState {
    /// Whether the keyboard is open (`data-keyboard`).
    pub fn keyboard_open(&self) -> bool {
        self.keyboard > px(0.)
    }
}

/// The window's viewport now.
pub fn viewport_state(window: &Window) -> ViewportState {
    let size = measure_viewport(window);
    let keyboard = keyboard_height(window);
    let safe_area = window.insets().safe_area;
    ViewportState {
        keyboard,
        keyboard_inset: keyboard_inset(&size, keyboard),
        width: size.width.round(),
        height: size.height.round(),
        offset_top: size.offset_top.round(),
        offset_left: window.visual_viewport_bounds().origin.x.round(),
        scale: size.scale,
        safe_area: Edges {
            top: safe_area.top.round(),
            right: safe_area.right.round(),
            bottom: safe_area.bottom.round(),
            left: safe_area.left.round(),
        },
    }
}

type ViewportChange = Rc<dyn Fn(&ViewportState, &mut Window, &mut App)>;

struct BindState {
    current: Option<ViewportState>,
    on_change: Option<ViewportChange>,
}

/// Follows the viewport for an element. Clones share it.
#[derive(Clone)]
pub struct BindViewport(Rc<RefCell<BindState>>);

/// Follows the viewport for the element [`BindViewport::track`] adds it to,
/// for its parts to read from [`BindViewport::get`]: the original's
/// `--keyboard-inset`, `--visual-viewport-width`, `--visual-viewport-height`,
/// `--visual-viewport-top` and `data-keyboard`. `on_change` hears every
/// change, and once at the start, after the frame that measured it has drawn.
/// Dropping it stops.
pub fn bind_viewport(on_change: Option<ViewportChange>) -> BindViewport {
    BindViewport(Rc::new(RefCell::new(BindState {
        current: None,
        on_change,
    })))
}

impl BindViewport {
    /// The viewport as of the last frame, once the element laid out.
    pub fn get(&self) -> Option<ViewportState> {
        self.0.borrow().current
    }

    /// Measures the viewport each frame `el` lays out, and has its view
    /// render again when it changed.
    pub fn track<E: ParentElement + Styled>(&self, el: E) -> E {
        let state = Rc::downgrade(&self.0);
        probe(el, move |_, window, cx| {
            let Some(state) = state.upgrade() else {
                return;
            };
            let next = viewport_state(window);
            let on_change = {
                let mut state = state.borrow_mut();
                if state.current == Some(next) {
                    return;
                }
                state.current = Some(next);
                state.on_change.clone()
            };
            let view = window.current_view();
            window.defer(cx, move |window, cx| {
                if let Some(on_change) = on_change {
                    on_change(&next, window, cx);
                }
                cx.notify(view);
            });
        })
    }
}

#[derive(Default)]
struct AvoidState {
    keyboard: Pixels,
    lifted: Pixels,
}

/// Keeps an element above the on-screen keyboard. Clones share it.
#[derive(Clone, Default)]
pub struct AvoidKeyboard(Rc<RefCell<AvoidState>>);

/// Keeps an element above the on-screen keyboard, like a chat composer or a
/// toolbar at the bottom of the screen: while the keyboard is open,
/// [`AvoidKeyboard::lift`] says how much of it lies below what's on screen,
/// for the element to move up by, like `.relative().bottom(lift)`, its own
/// offset aside. An element below the layout viewport stays put, and a scroll
/// around it measures it again with the frame it draws. Dropping it stops.
pub fn bind_avoid_keyboard() -> AvoidKeyboard {
    AvoidKeyboard::default()
}

impl AvoidKeyboard {
    /// How far to lift the element, as of the last frame.
    pub fn lift(&self) -> Pixels {
        self.0.borrow().lifted
    }

    /// Whether the keyboard is open (`data-keyboard`).
    pub fn keyboard(&self) -> bool {
        self.0.borrow().keyboard > px(0.)
    }

    /// Measures `el` each frame it lays out, lifted by what [`lift`] said,
    /// and has its view render again when the lift changed.
    ///
    /// [`lift`]: AvoidKeyboard::lift
    pub fn track<E: ParentElement + Styled>(&self, el: E) -> E {
        let state = Rc::downgrade(&self.0);
        probe(el, move |bounds, window, _cx| {
            let Some(state) = state.upgrade() else {
                return;
            };
            let keyboard = keyboard_height(window);
            let size = measure_viewport(window);
            let lifted = state.borrow().lifted;
            // Where the element would be without the lift, in the layout
            // viewport the keyboard covers, against where the visible part of
            // the window ends.
            let next = if keyboard > px(0.) {
                let top = bounds.top() + lifted;
                let bottom = (bounds.bottom() + lifted).min(size.layout_height);
                let below = if top < size.layout_height {
                    bottom - size.offset_top - size.height
                } else {
                    px(0.)
                };
                below.max(px(0.)).round()
            } else {
                px(0.)
            };
            let changed = {
                let mut state = state.borrow_mut();
                let changed = state.lifted != next || (state.keyboard > px(0.)) != (keyboard > px(0.));
                state.keyboard = keyboard;
                state.lifted = next;
                changed
            };
            if changed {
                let view = window.current_view();
                window.on_next_frame(move |_, cx| cx.notify(view));
            }
        })
    }
}
