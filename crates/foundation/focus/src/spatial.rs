//! Port of `foundation/focus/spatial.ts`. Spatial focus: arrow keys go to the
//! nearest item on screen in their direction, not the next one in page
//! order. For layouts that aren't a line or a regular grid, like a board of
//! cards placed anywhere, a ring of actions, a dashboard or a grid whose rows
//! wrap at different widths.

use std::rc::Rc;

use gpui::{App, Bounds, KeyDownEvent, Pixels};
use ui_sdk_kernel::Key;

use crate::roving_focus::{RovingFocusItem, RovingFocusMoveContext, RovingMoveFn};

/// A box on screen, like an element's bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpatialRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl SpatialRect {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

impl From<Bounds<Pixels>> for SpatialRect {
    fn from(bounds: Bounds<Pixels>) -> Self {
        Self {
            left: bounds.left().as_f32(),
            top: bounds.top().as_f32(),
            right: bounds.right().as_f32(),
            bottom: bounds.bottom().as_f32(),
        }
    }
}

// The direction each arrow points, on screen.
fn arrow(key: &Key) -> Option<[f32; 2]> {
    match key {
        Key::ArrowUp => Some([0., -1.]),
        Key::ArrowDown => Some([0., 1.]),
        Key::ArrowLeft => Some([-1., 0.]),
        Key::ArrowRight => Some([1., 0.]),
        _ => None,
    }
}

/// The key of the box nearest `from` in a direction, `[1., 0.]` for right,
/// or `None` when none lies that way. Boxes inside a 45° cone come first,
/// and distance to the side counts double, so a box straight ahead beats a
/// closer one off to the side. Pure, so it needs no window.
pub fn nearest_in_direction<K: Clone>(
    from: &SpatialRect,
    candidates: &[(K, SpatialRect)],
    direction: [f32; 2],
) -> Option<K> {
    let [dx, dy] = direction;
    let cx = (from.left + from.right) / 2.;
    let cy = (from.top + from.bottom) / 2.;
    let mut best: Option<(&K, f32, bool)> = None;
    for (key, rect) in candidates {
        let x = (rect.left + rect.right) / 2. - cx;
        let y = (rect.top + rect.bottom) / 2. - cy;
        let ahead = x * dx + y * dy;
        if ahead <= 0. {
            continue;
        }
        let aside = if dx != 0. { y.abs() } else { x.abs() };
        let cone = aside <= ahead;
        let score = ahead + aside * 2.;
        let better = match best {
            None => true,
            Some((_, best_score, best_cone)) => {
                (cone && !best_cone) || (cone == best_cone && score < best_score)
            }
        };
        if better {
            best = Some((key, score, cone));
        }
    }
    best.map(|(key, _, _)| key.clone())
}

/// Where an item is on screen, for [`spatial_move`].
pub type SpatialRectFn<K> = Rc<dyn Fn(&RovingFocusItem<K>, &App) -> Option<SpatialRect>>;

/// How [`spatial_move`] finds items on screen.
pub struct SpatialMoveOptions<K> {
    /// Where an item is on screen. Defaults to the bounds its element last
    /// laid out at, which the group records for items it
    /// [`measure`](crate::RovingFocus::measure)s. Pass it when the app
    /// already knows, like for a zoomed canvas or a layout from angles: the
    /// same units for every item, with larger `top` farther down.
    pub rect: Option<SpatialRectFn<K>>,
}

impl<K> Default for SpatialMoveOptions<K> {
    fn default() -> Self {
        Self { rect: None }
    }
}

impl<K> SpatialMoveOptions<K> {
    pub fn rect(
        mut self,
        rect: impl Fn(&RovingFocusItem<K>, &App) -> Option<SpatialRect> + 'static,
    ) -> Self {
        self.rect = Some(Rc::new(rect));
        self
    }
}

/// A `move` for [`RovingFocus`](crate::RovingFocus) that sends each arrow to
/// the nearest item on screen in its direction. Home and End go to the first
/// and last item in page order. Directions are physical: Left is left in
/// either reading direction, since the items sit where they're drawn. Arrows
/// with Alt, Ctrl or ⌘ are left alone, for a component's own shortcuts,
/// like nudging. Measure each item with `RovingFocus::measure`, or pass
/// `rect`.
pub fn spatial_move<K: Clone + PartialEq + 'static>(options: SpatialMoveOptions<K>) -> RovingMoveFn<K> {
    let rect_of = options.rect;
    Rc::new(
        move |event: &KeyDownEvent, context: &RovingFocusMoveContext<K>, cx: &App| {
            let modifiers = event.keystroke.modifiers;
            if modifiers.alt || modifiers.control || modifiers.platform {
                return None;
            }
            let key = Key::of(&event.keystroke);
            match key {
                Key::Home => return context.items.first().map(|item| item.key.clone()),
                Key::End => return context.items.last().map(|item| item.key.clone()),
                _ => {}
            }
            let direction = arrow(&key)?;
            let rect = |item: &RovingFocusItem<K>| match &rect_of {
                Some(rect_of) => rect_of(item, cx),
                None => item.bounds.map(SpatialRect::from),
            };
            let candidates: Vec<(K, SpatialRect)> = context
                .items
                .iter()
                .filter(|item| item.key != context.from.key)
                .filter_map(|item| rect(item).map(|rect| (item.key.clone(), rect)))
                .collect();
            nearest_in_direction(&rect(&context.from)?, &candidates, direction)
        },
    )
}
