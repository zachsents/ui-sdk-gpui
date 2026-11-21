//! Port of `foundation/popups/bridge.ts`: the area a pointer may cross
//! between a trigger and its popup.

use gpui::{Bounds, Pixels, Point};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Corner {
    x: f32,
    y: f32,
}

/// The area a pointer may cross between a trigger and its popup: on either,
/// or anywhere between them, as the convex hull of both boxes grown by
/// `padding`. The pointer can then leave the trigger by any edge, even
/// sideways, on its way to the popup. Built once, so a pointer move only does
/// the arithmetic and allocates nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct Bridge {
    hull: Vec<Corner>,
}

impl Bridge {
    /// Whether a point is on the trigger, the popup or between them.
    pub fn contains(&self, point: Point<Pixels>) -> bool {
        let (x, y) = (f32::from(point.x), f32::from(point.y));
        let count = self.hull.len();
        for index in 0..count {
            let from = self.hull[index];
            let to = self.hull[(index + 1) % count];
            if (to.x - from.x) * (y - from.y) - (to.y - from.y) * (x - from.x) < 0.0 {
                return false;
            }
        }
        true
    }
}

/// The bridge between a trigger's and a popup's bounds (`bridgeBetween`),
/// with the original's padding of 4px.
pub fn bridge_between(trigger: Bounds<Pixels>, popup: Bounds<Pixels>) -> Bridge {
    bridge_between_with(trigger, popup, 4.0)
}

/// [`bridge_between`] with another padding.
pub fn bridge_between_with(trigger: Bounds<Pixels>, popup: Bounds<Pixels>, padding: f32) -> Bridge {
    let corners: Vec<Corner> = [trigger, popup]
        .iter()
        .flat_map(|bounds| {
            let left = f32::from(bounds.left());
            let right = f32::from(bounds.right());
            let top = f32::from(bounds.top());
            let bottom = f32::from(bounds.bottom());
            [
                Corner { x: left - padding, y: top - padding },
                Corner { x: right + padding, y: top - padding },
                Corner { x: right + padding, y: bottom + padding },
                Corner { x: left - padding, y: bottom + padding },
            ]
        })
        .collect();
    Bridge {
        hull: convex_hull(corners),
    }
}

fn cross(a: Corner, b: Corner, c: Corner) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn convex_hull(points: Vec<Corner>) -> Vec<Corner> {
    let mut sorted = points;
    sorted.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal))
    });
    let half = |list: &[Corner]| {
        let mut chain: Vec<Corner> = Vec::new();
        for &point in list {
            while chain.len() >= 2 && cross(chain[chain.len() - 2], chain[chain.len() - 1], point) <= 0.0 {
                chain.pop();
            }
            chain.push(point);
        }
        chain.pop();
        chain
    };
    let mut hull = half(&sorted);
    sorted.reverse();
    hull.extend(half(&sorted));
    hull
}
