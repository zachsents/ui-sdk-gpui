//! Port of `components/resizable/resizable-handles.ts`: where presses and
//! hovers find resize handles. One listener per window serves every group,
//! so a handle's hit area can be larger than the handle, and handles of
//! nested groups that meet drag together, one along each axis.
//!
//! The original listens on the document. gpui hears a window's pointer
//! through listeners an element registers as it paints, so each group's
//! root paints [`listener`], and the first to paint in a frame registers the
//! window's listeners for every group, and sets the cursor. They register
//! ahead of the panels' content, so they hear a press in its bubble phase
//! after the content's own handlers: one that stops it keeps the press, like
//! a button inside a panel, and so does a drag that follows it, like a
//! slider's.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Entity, EntityId, IntoElement, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, Pixels, Point, SharedString, Styled, TouchDragEvent, TouchPhase,
    WeakEntity, Window, WindowId, canvas, px,
};
use ui_sdk_drag_and_drop::{PointerEvent, is_drag_press, swallow_next_click};

use crate::resizable::Resizable;

// How wide a hit area is at least, in pixels, centered on the handle. A thin
// visual line stays easy to grab. Touch gets the WCAG minimum target size.
const HIT_SIZE_FINE: f32 = 10.0;
const HIT_SIZE_COARSE: f32 = 24.0;

/// The axis a handle moves along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    X,
    Y,
}

/// The cursor for handles of two axes that meet (`move`). gpui has none of
/// four arrows, so it's the open hand.
const MOVE: CursorStyle = CursorStyle::OpenHand;

// A handle as the window sees it: its group and its key.
#[derive(Clone)]
struct Target {
    group: WeakEntity<Resizable>,
    entity: EntityId,
    key: SharedString,
}

impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity && self.key == other.key
    }
}

struct Hit {
    target: Target,
    axis: Axis,
    distance: f32,
    // Whether the press landed on the handle itself.
    on_handle: bool,
    // Whether what the press landed on is in the handle's group, so a
    // dialog or a popover above a handle keeps its presses.
    owned: bool,
}

// One window's groups, and the handles hovered and dragged.
#[derive(Default)]
struct WindowHandles {
    groups: Vec<(EntityId, WeakEntity<Resizable>)>,
    hovered: Vec<Target>,
    active: Vec<Target>,
    active_touch: bool,
    // Whether the frame being drawn registered the window's listeners.
    drawing: bool,
    // The press last handled, so listeners a cached view repaints hear it
    // once.
    last_press: Option<PointerEvent>,
}

thread_local! {
    static WINDOWS: RefCell<HashMap<WindowId, WindowHandles>> = RefCell::new(HashMap::new());
}

fn with_window<R>(window: WindowId, f: impl FnOnce(&mut WindowHandles) -> R) -> R {
    WINDOWS.with(|windows| f(windows.borrow_mut().entry(window).or_default()))
}

fn window_id(window: &Window) -> WindowId {
    window.window_handle().window_id()
}

/// The element a group's root renders on every frame, first among its
/// children: it makes the group's handles findable by presses and hovers
/// near them (`registerHandle`, `listen`).
pub(crate) fn listener(this: &Entity<Resizable>) -> impl IntoElement + use<> {
    let group = this.downgrade();
    let entity = this.entity_id();
    canvas(
        |_, _, _| {},
        move |_, _, window, cx| painted(entity, group.clone(), window, cx),
    )
    .absolute()
    .size_0()
}

fn painted(entity: EntityId, group: WeakEntity<Resizable>, window: &mut Window, cx: &mut App) {
    let id = window_id(window);
    let first = with_window(id, |handles| {
        handles.groups.retain(|(_, group)| group.upgrade().is_some());
        if !handles.groups.iter().any(|(each, _)| *each == entity) {
            handles.groups.push((entity, group));
        }
        !std::mem::replace(&mut handles.drawing, true)
    });
    if !first {
        return;
    }
    window.defer(cx, move |_, _| with_window(id, |handles| handles.drawing = false));
    listen(window);
    if let Some(style) = cursor(id, cx) {
        window.set_window_cursor_style(style);
    }
}

// This frame's window listeners, in the bubble phase.
fn listen(window: &mut Window) {
    window.on_mouse_event(|event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            let press = PointerEvent::mouse_down(event, window, cx);
            pressed(press, false, window, cx);
        }
    });
    window.on_mouse_event(|event: &TouchDragEvent, phase, window, cx| {
        // A touch another drag claimed isn't offered here.
        if phase == DispatchPhase::Bubble && event.phase == TouchPhase::Started && !window.default_prevented()
        {
            let press = PointerEvent::touch_drag(event, window, cx);
            pressed(press, true, window, cx);
        }
    });
    window.on_mouse_event(|event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble {
            moved(event, window, cx);
        }
    });
    window.on_mouse_event(|_: &MouseExitEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble {
            set_hovered(window, Vec::new(), cx);
        }
    });
}

fn groups(window: WindowId) -> Vec<Entity<Resizable>> {
    with_window(window, |handles| {
        handles
            .groups
            .iter()
            .filter_map(|(_, group)| group.upgrade())
            .collect()
    })
}

// The handles at a point: the nearest, plus one of the other axis where two
// meet (`hitTest`).
fn hit_test(point: Point<Pixels>, coarse: bool, window: &Window, cx: &App) -> Vec<Hit> {
    let groups = groups(window_id(window));
    // Checked before measuring too, so a press outside every group reads no
    // layout.
    if !groups
        .iter()
        .any(|group| group.read(cx).root_contains(point, window))
    {
        return Vec::new();
    }
    let pad = px(if coarse { HIT_SIZE_COARSE } else { HIT_SIZE_FINE });
    let mut hits = Vec::new();
    for group in &groups {
        let resizable = group.read(cx);
        let owned = resizable.root_contains(point, window);
        for (key, axis, bounds) in resizable.hit_targets() {
            if bounds.size.width <= px(0.) && bounds.size.height <= px(0.) {
                continue;
            }
            let thickness = if axis == Axis::X {
                bounds.size.width
            } else {
                bounds.size.height
            };
            let reach = ((pad - thickness) / 2.).max(px(0.));
            let dx = (bounds.left() - point.x)
                .max(px(0.))
                .max(point.x - bounds.right());
            let dy = (bounds.top() - point.y)
                .max(px(0.))
                .max(point.y - bounds.bottom());
            if dx <= reach && dy <= reach {
                let (dx, dy) = (f32::from(dx), f32::from(dy));
                hits.push(Hit {
                    target: Target {
                        group: group.downgrade(),
                        entity: group.entity_id(),
                        key,
                    },
                    axis,
                    distance: dx.hypot(dy),
                    on_handle: contains(bounds, point),
                    owned: owned || contains(bounds, point),
                });
            }
        }
    }
    if !hits.iter().any(|hit| hit.owned) {
        return Vec::new();
    }
    hits.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut hits = hits.into_iter();
    let Some(nearest) = hits.next() else {
        return Vec::new();
    };
    match hits.find(|hit| hit.axis != nearest.axis) {
        Some(crossing) => vec![nearest, crossing],
        None => vec![nearest],
    }
}

fn contains(bounds: Bounds<Pixels>, point: Point<Pixels>) -> bool {
    point.x >= bounds.left()
        && point.x <= bounds.right()
        && point.y >= bounds.top()
        && point.y <= bounds.bottom()
}

// A press from the window: the handles near it start dragging.
fn pressed(press: PointerEvent, coarse: bool, window: &mut Window, cx: &mut App) {
    let id = window_id(window);
    let seen = with_window(id, |handles| {
        if handles.groups.is_empty()
            || handles
                .last_press
                .as_ref()
                .is_some_and(|last| last.is_same(&press))
        {
            return true;
        }
        handles.last_press = Some(press.clone());
        false
    });
    // A drag of the panels' content follows the press already, like a
    // slider's, which keeps it, as a handler that prevents its default does.
    if seen || is_drag_press(&press) {
        return;
    }
    let hits = hit_test(press.position, coarse, window, cx);
    if hits.is_empty() {
        return;
    }
    // No focus change: focus stays where the user was working.
    window.prevent_default();
    // A press beside a handle, over a panel's content, drags. Its release
    // would click whatever is under it, moved or not.
    if !hits.iter().any(|hit| hit.on_handle) {
        swallow_next_click(true, window, cx);
    }
    with_window(id, |handles| {
        handles.active = hits.iter().map(|hit| hit.target.clone()).collect();
        handles.active_touch = coarse;
    });
    for hit in hits {
        let key = hit.target.key.clone();
        hit.target
            .group
            .update(cx, |group, cx| group.press(&press, &key, window, cx))
            .ok();
    }
    window.refresh();
}

// Hovers near a handle highlight it and show its cursor.
fn moved(event: &MouseMoveEvent, window: &mut Window, cx: &mut App) {
    let id = window_id(window);
    let idle = with_window(id, |handles| {
        handles.active.is_empty() && !handles.groups.is_empty()
    });
    if event.pressed_button.is_some() || !idle {
        return;
    }
    let hits = hit_test(event.position, false, window, cx);
    let next = hits.into_iter().map(|hit| hit.target).collect();
    set_hovered(window, next, cx);
}

fn set_hovered(window: &mut Window, next: Vec<Target>, cx: &mut App) {
    let id = window_id(window);
    let previous = with_window(id, |handles| {
        std::mem::replace(&mut handles.hovered, next.clone())
    });
    if previous == next {
        return;
    }
    for target in &previous {
        if !next.contains(target) {
            let key = target.key.clone();
            target
                .group
                .update(cx, |group, cx| group.set_hovering(&key, false, cx))
                .ok();
        }
    }
    for target in &next {
        if !previous.contains(target) {
            let key = target.key.clone();
            target
                .group
                .update(cx, |group, cx| group.set_hovering(&key, true, cx))
                .ok();
        }
    }
    window.refresh();
}

/// Ends a handle's drag in the window's eyes: its cursor goes
/// (`releaseHandle`).
pub(crate) fn release(window: &mut Window, entity: EntityId, key: &SharedString) {
    let id = window_id(window);
    with_window(id, |handles| {
        handles
            .active
            .retain(|target| !(target.entity == entity && &target.key == key));
    });
    window.refresh();
}

/// Handles that left the page: they're no longer hovered or dragged in the
/// window's eyes (`registerHandle`'s cleanup). The group clears its own
/// hover.
pub(crate) fn forget(window: &mut Window, entity: EntityId, keys: &[SharedString]) {
    let id = window_id(window);
    with_window(id, |handles| {
        let gone = |target: &Target| target.entity == entity && keys.contains(&target.key);
        handles.hovered.retain(|target| !gone(target));
        handles.active.retain(|target| !gone(target));
    });
}

// The cursor shows everywhere while hovering or dragging, so it stays as the
// pointer passes over panel content. A touch drag shows none.
fn cursor(window: WindowId, cx: &App) -> Option<CursorStyle> {
    let (shown, touch) = with_window(window, |handles| {
        if handles.active.is_empty() {
            (handles.hovered.clone(), false)
        } else {
            (handles.active.clone(), handles.active_touch)
        }
    });
    if shown.is_empty() || touch {
        return None;
    }
    if shown.len() > 1 {
        return Some(MOVE);
    }
    let target = &shown[0];
    let group = target.group.upgrade()?;
    Some(group.read(cx).cursor(&target.key, cx))
}
