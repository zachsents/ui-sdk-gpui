//! Port of `components/resizable/resizable-dom.ts`: the page side of a
//! resizable group: finding its panels and handles, measuring the space they
//! share, pointer presses and the cursor.
//!
//! The original finds panels and handles in document order a microtask
//! after any of them binds. Here they register as they render, which is page
//! order, and once a frame has drawn the group, it reconciles them when they
//! or their size props changed, and measures the space panels share, which
//! is what its `ResizeObserver` watched.

use std::collections::HashMap;

use gpui::{App, Bounds, Context, CursorStyle, Pixels, Point, SharedString, Window};
use ui_sdk_drag_and_drop::{BindDragOptions, PointerEvent, PointerPhase};
use ui_sdk_platform::is_rtl;

use crate::resizable::{Resizable, ResizableHandleProps};
use crate::resizable_handles::{self, Axis};
use crate::resizable_layout::PanelSizes;

/// A panel or a handle as it rendered, in page order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Member {
    Panel(SharedString, PanelSizes),
    Handle(SharedString, ResizableHandleProps),
}

impl Resizable {
    // The props a panel rendered with last.
    pub(crate) fn panel_props(&self, id: &SharedString) -> Option<PanelSizes> {
        self.members.borrow().iter().find_map(|member| match member {
            Member::Panel(each, props) if each == id => Some(*props),
            _ => None,
        })
    }

    // The props a handle rendered with last.
    pub(crate) fn handle_props(&self, key: &SharedString) -> Option<ResizableHandleProps> {
        self.members.borrow().iter().find_map(|member| match member {
            Member::Handle(each, props) if each == key => Some(*props),
            _ => None,
        })
    }

    // The handles in the page, in order.
    pub(crate) fn handle_keys(&self) -> Vec<SharedString> {
        self.members
            .borrow()
            .iter()
            .filter_map(|member| match member {
                Member::Handle(key, _) => Some(key.clone()),
                Member::Panel(..) => None,
            })
            .collect()
    }

    // A part's bounds as of its last layout.
    fn bounds_of(&self, slot: &str, key: &SharedString) -> Option<Bounds<Pixels>> {
        self.instance
            .bound_parts()
            .into_iter()
            .find(|part| part.slot == slot && part.key.as_ref() == Some(key))
            .and_then(|part| part.bounds)
    }

    // The sum of the panels' sizes along the axis, whatever handles and gaps
    // take (`spaceOf`).
    fn space_of(&self) -> f64 {
        let vertical = self.vertical();
        self.instance
            .bound_parts()
            .into_iter()
            .filter(|part| part.slot == "panel")
            .filter_map(|part| part.bounds)
            .map(|bounds| {
                f64::from(if vertical {
                    bounds.size.height
                } else {
                    bounds.size.width
                })
            })
            .sum()
    }

    // The pixels panels share, measured again (`measure`).
    pub(crate) fn measure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.measured = self.space_of();
        self.set_space(self.measured, window, cx);
    }

    /// Panels' current props, like a new `min_size`, read again before every
    /// interaction. The panels are in the order the last reconcile found
    /// (`refresh`).
    pub(crate) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rendered = self
            .members
            .borrow()
            .iter()
            .any(|member| matches!(member, Member::Panel(..)));
        if !rendered {
            return;
        }
        let ids = self.ids.clone();
        let sizes = ids
            .iter()
            .map(|id| self.panel_props(id).unwrap_or_default())
            .collect();
        self.set_panels(ids, sizes, self.measured, window, cx);
    }

    // Panels and handles in page order: one pass pairs each handle with the
    // last panel before it. New bounds and new space are fitted together, so
    // a pixel bound the same render changed never moves a panel by its old
    // value (`reconcile`).
    fn reconcile(&mut self, members: &[Member], window: &mut Window, cx: &mut Context<Self>) {
        let mut ids = Vec::new();
        let mut sizes = Vec::new();
        let mut handles = HashMap::new();
        let mut before: isize = -1;
        for member in members {
            match member {
                Member::Panel(id, props) => {
                    ids.push(id.clone());
                    sizes.push(*props);
                    before += 1;
                }
                Member::Handle(key, _) => {
                    handles.insert(key.clone(), before);
                }
            }
        }
        self.measured = self.space_of();
        self.set_panels(ids, sizes, self.measured, window, cx);
        self.set_handles(handles, cx);
    }

    /// Once a frame has drawn the group: reconciles its panels and handles
    /// when they changed, and measures the space they share. A handle that
    /// left, or that its own `disabled` now stops, ends its drag.
    pub(crate) fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let members = self.members.borrow().clone();
        if members != self.reconciled {
            let left: Vec<SharedString> = self
                .reconciled
                .iter()
                .filter_map(|member| match member {
                    Member::Handle(key, _) => Some(key.clone()),
                    Member::Panel(..) => None,
                })
                .filter(|key| {
                    !members
                        .iter()
                        .any(|member| matches!(member, Member::Handle(each, _) if each == key))
                })
                .collect();
            self.reconcile(&members, window, cx);
            self.reconciled = members;
            if !left.is_empty() {
                resizable_handles::forget(window, cx.entity_id(), &left);
                if self.hovering.as_ref().is_some_and(|key| left.contains(key)) {
                    self.hovering = None;
                    cx.notify();
                }
            }
            if let Some(key) = self.dragging.clone() {
                let stopped =
                    left.contains(&key) || self.handle_props(&key).is_some_and(|props| props.disabled);
                if stopped {
                    self.stop_drags(window, cx);
                    self.end_drag(None, false, window, cx);
                    resizable_handles::release(window, cx.entity_id(), &key);
                }
            }
        }
        self.measure(window, cx);
    }

    // Stops the pointer tracking of a drag that ends some other way, like the
    // group being disabled or its handle leaving the page (`stopAll`).
    pub(crate) fn stop_drags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stopping = true;
        let drags = self.drags.clone();
        drags.cancel(self, window, cx);
        self.stopping = false;
    }

    /// A press from the window's handle hit areas. The space is measured
    /// once, so moves never read layout (`press`).
    pub(crate) fn press(
        &mut self,
        event: &PointerEvent,
        key: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.handle_index(key) else {
            return;
        };
        self.measure(window, cx);
        if self.measured <= 0.0 || !self.start_drag(handle, key.clone(), window, cx) {
            resizable_handles::release(window, cx.entity_id(), key);
            return;
        }
        let rtl = is_rtl(cx);
        let vertical = self.vertical();
        let origin = if vertical { event.y() } else { event.x() };
        let key = key.clone();
        let drags = self.drags.clone();
        drags.start(
            event,
            BindDragOptions::new()
                .cancel_on_escape(true)
                .on_move(move |group: &mut Self, sample, window, cx| {
                    let moved = f64::from(if vertical { sample.y } else { sample.x } - origin);
                    let moved = if rtl && !vertical { -moved } else { moved };
                    group.drag_by(
                        moved / group.measured * 100.0,
                        Some(sample.event.event.clone()),
                        window,
                        cx,
                    );
                })
                .on_end(move |group: &mut Self, end, canceled, window, cx| {
                    // Stopped some other way: whatever stopped it ends it.
                    if group.stopping {
                        return;
                    }
                    // Escape cancels with the last move. The system taking
                    // the pointer keeps the layout.
                    let restore = canceled && end.phase != PointerPhase::Cancel;
                    group.end_drag(Some(end.event.clone()), restore, window, cx);
                    resizable_handles::release(window, cx.entity_id(), &key);
                }),
            window,
            cx,
        );
    }

    /// The cursor over a handle: the axis, or at a limit the one way the
    /// handle can go.
    pub(crate) fn cursor(&self, key: &SharedString, cx: &App) -> CursorStyle {
        let vertical = self.vertical();
        let both = if vertical {
            CursorStyle::ResizeUpDown
        } else {
            CursorStyle::ResizeLeftRight
        };
        let Some(value) = self.handle_value(key) else {
            return both;
        };
        let at_min = value.now <= value.min + 1e-6;
        let at_max = value.now >= value.max - 1e-6;
        if at_min == at_max {
            return both;
        }
        if vertical {
            return if at_min {
                CursorStyle::ResizeDown
            } else {
                CursorStyle::ResizeUp
            };
        }
        if at_min != is_rtl(cx) {
            CursorStyle::ResizeRight
        } else {
            CursorStyle::ResizeLeft
        }
    }

    // The axis a handle moves along, or `None` while it can't move.
    fn handle_axis(&self, key: &SharedString) -> Option<Axis> {
        if self.handle_disabled(key) || self.handle_index(key).is_none() {
            return None;
        }
        Some(if self.vertical() { Axis::Y } else { Axis::X })
    }

    /// The handles that can move now, with their axis and bounds, for the
    /// window's hit areas.
    pub(crate) fn hit_targets(&self) -> Vec<(SharedString, Axis, Bounds<Pixels>)> {
        self.handle_keys()
            .into_iter()
            .filter_map(|key| {
                let axis = self.handle_axis(&key)?;
                let bounds = self.bounds_of("handle", &key)?;
                Some((key, axis, bounds))
            })
            .collect()
    }

    /// Whether what a press at `point` lands on is in the group's element.
    pub(crate) fn root_contains(&self, point: Point<Pixels>, window: &Window) -> bool {
        self.root_region.contains_point(point, window)
    }

    /// A handle hovered or no longer.
    pub(crate) fn set_hovering(&mut self, key: &SharedString, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            if self.hovering.as_ref() != Some(key) {
                self.hovering = Some(key.clone());
                cx.notify();
            }
        } else if self.hovering.as_ref() == Some(key) {
            self.hovering = None;
            cx.notify();
        }
    }
}
