//! Port of `foundation/popups/safe-area.ts`. Lets the pointer cross other
//! triggers or items on its way to an open popup without switching to them.
//! Moves inside the triangle from where it left the trigger to the popup's
//! near edge are ignored until the pointer rests.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Context, Pixels, Point, Task, Window};
use ui_sdk_platform::ElementBounds;

/// The axis a popup sits on from its trigger: `X` beside it, `Y` above or
/// below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SafeAreaAxis {
    #[default]
    X,
    Y,
}

struct State {
    apex: Option<Point<Pixels>>,
    popup: Option<ElementBounds>,
    axis: SafeAreaAxis,
    last: Option<Point<Pixels>>,
    rest_timer: Option<Task<()>>,
    expired: bool,
    rest_timeout: Duration,
}

/// The triangle between where the pointer left a trigger and an open popup
/// (`createSafeArea`). Clones share it.
#[derive(Clone)]
pub struct SafeArea(Rc<RefCell<State>>);

/// A safe area whose pointer counts as resting after 300ms.
pub fn create_safe_area() -> SafeArea {
    create_safe_area_with(Duration::from_millis(300))
}

/// A safe area whose pointer counts as resting after `rest_timeout`.
pub fn create_safe_area_with(rest_timeout: Duration) -> SafeArea {
    SafeArea(Rc::new(RefCell::new(State {
        apex: None,
        popup: None,
        axis: SafeAreaAxis::X,
        last: None,
        rest_timer: None,
        expired: false,
        rest_timeout,
    })))
}

impl SafeArea {
    /// Call when the pointer leaves the trigger, with where it left and the
    /// popup's bounds. `along` is the axis the popup sits on from its trigger.
    pub fn start(&self, point: Point<Pixels>, popup: impl Into<ElementBounds>, along: SafeAreaAxis) {
        let mut state = self.0.borrow_mut();
        state.apex = Some(point);
        state.last = Some(point);
        state.popup = Some(popup.into());
        state.axis = along;
        state.expired = false;
    }

    pub fn stop(&self) {
        let mut state = self.0.borrow_mut();
        state.apex = None;
        state.popup = None;
        state.last = None;
        state.rest_timer = None;
    }

    /// Whether a crossing is being watched and hasn't rested yet.
    pub fn active(&self) -> bool {
        let state = self.0.borrow();
        state.apex.is_some() && !state.expired
    }

    /// Whether the move to `point` should be ignored. `on_rest` runs, inside
    /// the component's update, if the pointer stops inside the area.
    pub fn contains<T: 'static>(
        &self,
        point: Point<Pixels>,
        on_rest: impl FnOnce(&mut T, &mut Window, &mut Context<T>) + 'static,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let (inside, progressing, timeout) = {
            let mut state = self.0.borrow_mut();
            let (Some(apex), Some(popup)) = (state.apex, state.popup.clone()) else {
                return false;
            };
            if state.expired {
                return false;
            }
            let Some(rect) = popup.get() else {
                return false;
            };
            let (left, right, top, bottom) = (rect.left(), rect.right(), rect.top(), rect.bottom());
            let along = |p: Point<Pixels>| match state.axis {
                SafeAreaAxis::X => p.x,
                SafeAreaAxis::Y => p.y,
            };
            let near = match state.axis {
                SafeAreaAxis::X => {
                    if left >= apex.x {
                        left
                    } else {
                        right
                    }
                }
                SafeAreaAxis::Y => {
                    if top >= apex.y {
                        top
                    } else {
                        bottom
                    }
                }
            };
            let margin = gpui::px(8.);
            let inside = match state.axis {
                SafeAreaAxis::X => in_triangle(
                    point,
                    apex,
                    gpui::point(near, top - margin),
                    gpui::point(near, bottom + margin),
                ),
                SafeAreaAxis::Y => in_triangle(
                    point,
                    apex,
                    gpui::point(left - margin, near),
                    gpui::point(right + margin, near),
                ),
            };
            let progressing = state.last.is_none_or(|last| {
                f32::from((near - along(point)).abs()) <= f32::from((near - along(last)).abs()) + 1.0
            });
            state.last = Some(point);
            state.rest_timer = None;
            (inside, progressing, state.rest_timeout)
        };
        if !inside || !progressing {
            return false;
        }
        let area = Rc::downgrade(&self.0);
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(timeout).await;
            if let Some(area) = area.upgrade() {
                area.borrow_mut().expired = true;
            }
            this.update_in(cx, |this, window, cx| on_rest(this, window, cx)).ok();
        });
        self.0.borrow_mut().rest_timer = Some(task);
        true
    }
}

fn in_triangle(p: Point<Pixels>, a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>) -> bool {
    let sign = |p1: Point<Pixels>, p2: Point<Pixels>, p3: Point<Pixels>| {
        let (x1, y1) = (f32::from(p1.x), f32::from(p1.y));
        let (x2, y2) = (f32::from(p2.x), f32::from(p2.y));
        let (x3, y3) = (f32::from(p3.x), f32::from(p3.y));
        (x1 - x3) * (y2 - y3) - (x2 - x3) * (y1 - y3)
    };
    let d1 = sign(p, a, b);
    let d2 = sign(p, b, c);
    let d3 = sign(p, c, a);
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}
