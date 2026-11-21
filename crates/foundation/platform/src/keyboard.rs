//! Port of `foundation/platform/keyboard.ts`: the on-screen keyboard. Phones
//! shrink only the visual viewport when the keyboard opens, or lay it over the
//! window's bottom, so a centered dialog or a sheet would sit under it.
//!
//! The original infers the keyboard from how the visual viewport shrinks
//! while a field has focus. gpui reports it: the window's insets say what the
//! keyboard covers, and its visual viewport what's on screen. Both change only
//! as the window redraws, so a tracker measures as its element prepaints and
//! asks the element's view to render again when the keyboard moved, where the
//! original writes CSS variables for the page to restyle with.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, ParentElement, Pixels, Styled, Window, px};

use crate::layout::probe;

/// What the screen leaves for the window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportSize {
    /// The visual viewport's width.
    pub width: Pixels,
    /// The layout viewport's height: the window's whole drawable area.
    pub layout_height: Pixels,
    /// What's on screen of it, above the keyboard.
    pub height: Pixels,
    /// How far the visual viewport is scrolled down the layout viewport.
    pub offset_top: Pixels,
    /// The pinch-zoom scale. gpui never zooms a window's content, so it's 1.
    pub scale: f32,
}

/// The window's viewport now: the visual viewport, less what the keyboard
/// covers of it.
pub fn measure_viewport(window: &Window) -> ViewportSize {
    let visual = window.visual_viewport_bounds();
    let layout_height = window.viewport_size().height;
    let keyboard = window.insets().ime.bottom.max(px(0.));
    let bottom = visual.bottom().min(layout_height - keyboard);
    ViewportSize {
        width: visual.size.width,
        layout_height,
        height: (bottom - visual.origin.y).max(px(0.)),
        offset_top: visual.origin.y,
        scale: 1.,
    }
}

/// The keyboard's height: what the window's insets say it covers, or how
/// much less of the screen the window gets than its layout, when the platform
/// shrinks the visual viewport for it instead. 0 without one.
pub fn keyboard_height(window: &Window) -> Pixels {
    let size = measure_viewport(window);
    let covered = size.layout_height - size.offset_top - size.height;
    window
        .insets()
        .ime
        .bottom
        .max(covered)
        .max(px(0.))
        .round()
}

/// How much of the layout viewport's bottom the keyboard covers: what lies
/// below what's on screen, up to a measured `keyboard`. 0 when the platform
/// resizes the window for the keyboard instead.
pub fn keyboard_inset(size: &ViewportSize, keyboard: Pixels) -> Pixels {
    let covered = (size.layout_height - size.offset_top - size.height).max(px(0.));
    if keyboard > px(0.) {
        return keyboard.min(covered).round();
    }
    covered.round()
}

/// The keyboard as a tracked element reads it: the original's
/// `--keyboard-inset`, `--visual-viewport-height` and `data-keyboard`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KeyboardInset {
    /// How much of the layout viewport's bottom the keyboard covers.
    pub inset: Pixels,
    /// The visual viewport's height, above the keyboard.
    pub visual_viewport_height: Pixels,
}

impl KeyboardInset {
    /// Whether the keyboard covers part of the window (`data-keyboard`).
    pub fn keyboard(&self) -> bool {
        self.inset > px(0.)
    }
}

/// Hears an inset change around it: `before` runs first, so it can read the
/// layout the change is about to replace, the frame just drawn, and `after`
/// once the new inset is what [`TrackKeyboard::get`] reads. Both run once the
/// frame that measured the change has drawn, and the element's view renders
/// with the new inset next.
#[derive(Clone)]
pub struct KeyboardChange {
    pub before: Rc<dyn Fn(&mut Window, &mut App)>,
    pub after: Rc<dyn Fn(&mut Window, &mut App)>,
}

struct TrackState {
    current: Option<KeyboardInset>,
    on_change: Option<KeyboardChange>,
}

/// Follows the keyboard for an element. Clones share it.
#[derive(Clone)]
pub struct TrackKeyboard(Rc<RefCell<TrackState>>);

/// Follows the keyboard for the element [`TrackKeyboard::track`] adds it to,
/// for its parts to read from [`TrackKeyboard::get`]. `on_change` hears an
/// inset change around it. Dropping it stops.
pub fn track_keyboard(on_change: Option<KeyboardChange>) -> TrackKeyboard {
    TrackKeyboard(Rc::new(RefCell::new(TrackState {
        current: None,
        on_change,
    })))
}

impl TrackKeyboard {
    /// The keyboard as of the last frame. All 0 before the element first
    /// lays out.
    pub fn get(&self) -> KeyboardInset {
        self.0.borrow().current.unwrap_or_default()
    }

    /// Measures the keyboard each frame `el` lays out, and has its view
    /// render again when it moved.
    pub fn track<E: ParentElement + Styled>(&self, el: E) -> E {
        let state = Rc::downgrade(&self.0);
        probe(el, move |_, window, cx| {
            let Some(state) = state.upgrade() else {
                return;
            };
            let size = measure_viewport(window);
            let next = KeyboardInset {
                inset: keyboard_inset(&size, px(0.)),
                visual_viewport_height: size.height.round(),
            };
            let (previous, on_change) = {
                let state = state.borrow();
                (state.current, state.on_change.clone())
            };
            if previous == Some(next) {
                return;
            }
            let view = window.current_view();
            let changed = previous.is_some_and(|previous| previous.inset != next.inset);
            let on_change = on_change.filter(|_| changed);
            let state = Rc::downgrade(&state);
            window.defer(cx, move |window, cx| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                if let Some(on_change) = &on_change {
                    (on_change.before)(window, cx);
                }
                state.borrow_mut().current = Some(next);
                if let Some(on_change) = &on_change {
                    (on_change.after)(window, cx);
                }
                cx.notify(view);
            });
        })
    }
}
