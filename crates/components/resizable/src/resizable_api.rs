//! Port of `components/resizable/resizable-api.ts`: a resizable group's
//! state and what it does: panels joining, drags, keys, collapsing, saving
//! and the controller. The parts and their state are in `resizable.rs`, the
//! pure layout rules in `resizable_layout.rs`, and finding panels and
//! handles, measuring and pressing in `resizable_dom.rs`.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{App, Context, KeyDownEvent, SharedString, Subscription, Task, Window};
use ui_sdk_kernel::focus_events::on_blur;
use ui_sdk_kernel::{ChangeOptions, InputEvent, Key, Reason};
use ui_sdk_platform::is_rtl;

use crate::resizable::{Resizable, ResizableOptions, ResizableOrientation, ResizablePanelRef};
use crate::resizable_handles;
use crate::resizable_layout::{
    HandleBounds, KeyDeltaOptions, PanelConstraints, PanelSizes, ResizableSize, all_handle_bounds,
    carry_layout, constraints_of, end_delta, fit_layout, home_delta, initial_layout, is_absolute,
    is_collapsed, key_delta, resize_at, resize_panel, same_layout, same_size, to_percent, toggle_target,
};
use crate::resizable_storage::{load_layout, save_layout};

/// What the options set besides the layout, its callbacks and saving
/// (`settings`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) orientation: ResizableOrientation,
    pub(crate) disabled: bool,
    pub(crate) step: f64,
    pub(crate) large_step: f64,
}

pub(crate) fn settings(options: &ResizableOptions) -> Settings {
    Settings {
        orientation: options.orientation,
        disabled: options.disabled,
        step: options.step,
        large_step: options.large_step,
    }
}

/// What a handle's separator says: the panel before it, its size and how
/// far it can go, and whether its own `disabled` stops it (`HandleValue`).
#[derive(Clone, Debug, PartialEq)]
pub struct HandleValue {
    pub panel: SharedString,
    pub now: f64,
    pub min: f64,
    pub max: f64,
    pub disabled: bool,
}

// What the handles' values are worked out from: the layout, the panels,
// which panel each handle follows, and which handles are locked.
pub(crate) type ShownInputs = (
    Vec<f64>,
    Vec<SharedString>,
    Vec<PanelConstraints>,
    Vec<(SharedString, isize)>,
    Vec<bool>,
);

// Every handle's value for a layout (`shownValues`).
pub(crate) fn shown_values(inputs: &ShownInputs) -> HashMap<SharedString, HandleValue> {
    let (layout, ids, constraints, handles, locked) = inputs;
    let bounds: Vec<HandleBounds> = if layout.len() == constraints.len() {
        all_handle_bounds(layout, constraints, locked)
    } else {
        Vec::new()
    };
    let mut values = HashMap::new();
    for (key, index) in handles {
        let Ok(index) = usize::try_from(*index) else {
            continue;
        };
        let (Some(range), Some(panel), Some(now)) = (bounds.get(index), ids.get(index), layout.get(index))
        else {
            continue;
        };
        values.insert(
            key.clone(),
            HandleValue {
                panel: panel.clone(),
                now: *now,
                min: range.min,
                max: range.max,
                disabled: locked.get(index).copied().unwrap_or(false),
            },
        );
    }
    values
}

fn same_constraints(a: &[PanelConstraints], b: &[PanelConstraints]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(panel, other)| {
            same_size(panel.min, other.min)
                && same_size(panel.max, other.max)
                && same_size(panel.collapsed, other.collapsed)
                && panel.collapsible == other.collapsible
                && panel.initial == other.initial
        })
}

/// A drag from a pointer (`Drag`).
pub(crate) struct Drag {
    pub(crate) handle: usize,
    pub(crate) key: SharedString,
    // Where the drag started. Each move is measured from here, so the result
    // never drifts, and a panel that snapped closed reopens on the way back.
    pub(crate) base: Vec<f64>,
}

/// Keys held down, which commit when they come up, so holding one commits
/// once.
pub(crate) struct Keys {
    pub(crate) start: Vec<f64>,
    pub(crate) event: Option<InputEvent>,
    // Focus leaving the handle, which stands for the keyup it won't get.
    pub(crate) _blur: Option<Subscription>,
}

/// An interaction that completed while a controlled group's owner hadn't
/// passed back the last layout it asked for (`waitForOwner`).
pub(crate) struct Waiting {
    pub(crate) before: Vec<f64>,
    pub(crate) start: Vec<f64>,
    pub(crate) reason: Reason,
    pub(crate) event: Option<InputEvent>,
    pub(crate) _timer: Task<()>,
}

/// Rem sizes follow the window's rem size.
pub(crate) fn rem_size(window: &Window) -> f64 {
    f64::from(window.rem_size())
}

impl Resizable {
    // The layout a change last asked for, or the layout (`current`).
    pub(crate) fn current(&self) -> Vec<f64> {
        self.requested
            .clone()
            .unwrap_or_else(|| self.layout.get().clone())
    }

    /// Whether the group is disabled, by itself or by a fieldset.
    pub fn is_disabled(&self) -> bool {
        self.settings.disabled || self.form_node.form_disabled()
    }

    // Whether a handle can't move: the group or its own `disabled` stops it.
    pub(crate) fn handle_disabled(&self, key: &SharedString) -> bool {
        self.is_disabled() || self.handle_props(key).is_some_and(|props| props.disabled)
    }

    // Which handles are disabled on their own, by index. Moves from the
    // others stop at them, so the panels past one keep their sizes.
    pub(crate) fn locked(&self) -> Vec<bool> {
        let mut locked = Vec::new();
        for (key, index) in &self.handles {
            let Ok(index) = usize::try_from(*index) else {
                continue;
            };
            if self.handle_props(key).is_some_and(|props| props.disabled) {
                if locked.len() <= index {
                    locked.resize(index + 1, false);
                }
                locked[index] = true;
            }
        }
        locked
    }

    pub(crate) fn vertical(&self) -> bool {
        self.settings.orientation == ResizableOrientation::Vertical
    }

    fn index_of(&self, panel: &ResizablePanelRef) -> Option<usize> {
        match panel {
            ResizablePanelRef::Index(index) => Some(*index),
            ResizablePanelRef::Id(id) => self.ids.iter().position(|each| each == id),
        }
    }

    // Writes the layout with no change, like panels joining: nobody moved
    // anything. A controlled group shows it until its owner passes one that
    // fits.
    pub(crate) fn show(&mut self, layout: Vec<f64>) {
        if self.layout.is_controlled() {
            self.layout.set_controlled(Some(layout));
        } else {
            self.layout.set_held(layout);
        }
    }

    // A controlled layout applies once it fits the panels there are, in the
    // same pass as the options that bring it. Once it applies, the layout a
    // change asked for is the owner's to keep.
    pub(crate) fn apply_controlled(&mut self) -> bool {
        let Some(controlled) = self.controlled_layout.clone() else {
            return false;
        };
        if controlled.len() != self.ids.len() || same_layout(&controlled, self.layout.get()) {
            return false;
        }
        self.show(controlled);
        self.requested = None;
        true
    }

    // Remembers the size of every panel this change collapses.
    fn remember(&mut self, previous: &[f64], next: &[f64]) {
        for (index, size) in next.iter().enumerate() {
            let Some(panel) = self.constraints.get(index) else {
                continue;
            };
            let was = previous.get(index).copied().unwrap_or(f64::NAN);
            if !is_collapsed(panel, *size) || is_collapsed(panel, was) {
                continue;
            }
            let before = match &self.drag {
                Some(drag)
                    if drag
                        .base
                        .get(index)
                        .is_some_and(|base| !is_collapsed(panel, *base)) =>
                {
                    drag.base[index]
                }
                _ => was,
            };
            if let Some(id) = self.ids.get(index) {
                self.expanded.insert(id.clone(), before);
            }
        }
    }

    // Layout functions return the layout as it was when nothing moved, so a
    // different layout is a change to ask for (`request`).
    pub(crate) fn request(
        &mut self,
        next: Vec<f64>,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let previous = self.current();
        if same_layout(&next, &previous) {
            return false;
        }
        // A new change drops a completion still waiting for its owner.
        self.waiting = None;
        let before = self.requested.replace(next.clone());
        let options = ChangeOptions {
            reason: Some(reason),
            event,
            source: None,
        };
        if !self
            .layout
            .change(&self.instance, next.clone(), options, window, cx)
        {
            self.requested = before;
            return false;
        }
        self.remember(&previous, &next);
        cx.notify();
        true
    }

    // The interaction is over (`complete`). A controlled group completes
    // with what its owner passed back; until the owner has passed back the
    // last layout asked for, it waits for the owner's next one. One that
    // ignored the change completes and saves nothing.
    pub(crate) fn complete(
        &mut self,
        start: Vec<f64>,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let layout = self.current();
        let asked = self.requested.take();
        if self.controlled_layout.is_none() {
            return self.finish(start, layout, reason, event, window, cx);
        }
        let applied = self.layout.get().clone();
        if asked.as_ref().is_none_or(|asked| same_layout(&applied, asked)) {
            return self.finish(start, applied, reason, event, window, cx);
        }
        self.wait_for_owner(start, reason, event, window, cx);
    }

    // Completes once the owner applies its next layout, or 100 ms without
    // one, so a change it ignored never completes with a later layout of
    // its own.
    fn wait_for_owner(
        &mut self,
        start: Vec<f64>,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let timer = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            this.update_in(cx, |group, window, cx| group.answer_owner(window, cx))
                .ok();
        });
        self.waiting = Some(Waiting {
            before: self.layout.get().clone(),
            start,
            reason,
            event,
            _timer: timer,
        });
    }

    fn answer_owner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(waiting) = self.waiting.take() else {
            return;
        };
        let layout = self.layout.get().clone();
        self.finish(waiting.start, layout, waiting.reason, waiting.event, window, cx);
    }

    // The layout may have changed under a completion waiting for the owner.
    pub(crate) fn heard_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .waiting
            .as_ref()
            .is_some_and(|waiting| &waiting.before != self.layout.get())
        {
            self.answer_owner(window, cx);
        }
    }

    fn finish(
        &mut self,
        start: Vec<f64>,
        layout: Vec<f64>,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if same_layout(&start, &layout) {
            return;
        }
        if let Some(id) = &self.auto_save_id {
            save_layout(id, &self.ids, &layout, self.storage.as_ref(), cx);
        }
        // The commit reports the layout the group has, which is `layout`.
        self.layout.commit(&self.instance, reason, event, window, cx);
    }

    fn commit_now(
        &mut self,
        next: Vec<f64>,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let start = self.current();
        if !self.request(next, reason, event.clone(), window, cx) {
            return false;
        }
        self.complete(start, reason, event, window, cx);
        true
    }

    // Where a new set of panels starts: the controlled layout, the saved one,
    // the sizes panels that stay had, `default_layout`, or each panel's
    // default (`startLayout`).
    fn start_layout(&self, ids: &[SharedString], constraints: &[PanelConstraints], cx: &App) -> Vec<f64> {
        if let Some(layout) = &self.controlled_layout
            && layout.len() == ids.len()
        {
            return layout.clone();
        }
        let saved = self
            .auto_save_id
            .as_ref()
            .and_then(|id| load_layout(id, ids, self.storage.as_ref(), cx));
        if let Some(saved) = saved {
            return fit_layout(&saved, constraints);
        }
        if !self.ids.is_empty() {
            return carry_layout(&self.ids, self.layout.get(), ids, constraints);
        }
        if let Some(layout) = &self.default_layout
            && layout.len() == ids.len()
        {
            return fit_layout(layout, constraints);
        }
        initial_layout(constraints)
    }

    /// Panels joined, left or changed their sizes. Not a change: nobody moved
    /// anything (`setPanels`).
    pub(crate) fn set_panels(
        &mut self,
        ids: Vec<SharedString>,
        panel_sizes: Vec<PanelSizes>,
        measured: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if measured > 0.0 {
            self.space = measured;
        }
        let rem = rem_size(window);
        let constraints: Vec<PanelConstraints> = panel_sizes
            .iter()
            .map(|panel| constraints_of(panel, self.space, rem))
            .collect();
        self.sizes = panel_sizes;
        let ids_changed = ids != self.ids;
        if !ids_changed && same_constraints(&constraints, &self.constraints) {
            return;
        }
        // New bounds on the same panels move them like new space does, with
        // the `Resize` reason. During a drag the drag's own moves fit them.
        if !ids_changed {
            self.constraints = constraints;
            cx.notify();
            if self.drag.is_some() {
                return;
            }
            let next = fit_layout(&self.current(), &self.constraints);
            self.request(next, Reason::Resize, None, window, cx);
            // No interaction completes this one, so a controlled owner that
            // ignores it leaves nothing asked.
            self.requested = None;
            return;
        }
        let layout = self.start_layout(&ids, &constraints, cx);
        self.requested = None;
        self.ids = ids;
        self.constraints = constraints;
        self.show(layout);
        self.apply_controlled();
        self.heard_layout(window, cx);
        cx.notify();
    }

    /// The pixels panels share changed. Panels sized in pixels keep their
    /// bounds, which moves them, with the `Resize` reason (`setSpace`).
    pub(crate) fn set_space(&mut self, next: f64, window: &mut Window, cx: &mut Context<Self>) {
        if same_size(next, self.space) || next <= 0.0 {
            return;
        }
        self.space = next;
        let absolute = self.sizes.iter().any(|panel| {
            is_absolute(panel.min_size) || is_absolute(panel.max_size) || is_absolute(panel.collapsed_size)
        });
        if !absolute {
            return;
        }
        let rem = rem_size(window);
        self.constraints = self
            .sizes
            .iter()
            .map(|panel| constraints_of(panel, self.space, rem))
            .collect();
        cx.notify();
        if self.drag.is_none() {
            let next = fit_layout(&self.current(), &self.constraints);
            self.request(next, Reason::Resize, None, window, cx);
        }
    }

    // The handle's place among the panels, or `None` before it follows one
    // that has a panel after it (`handleIndex`).
    pub(crate) fn handle_index(&self, key: &SharedString) -> Option<usize> {
        let index = usize::try_from(*self.handles.get(key)?).ok()?;
        (index + 1 < self.ids.len()).then_some(index)
    }

    /// Which panel each handle follows, by handle key (`setHandles`).
    pub(crate) fn set_handles(&mut self, handles: HashMap<SharedString, isize>, cx: &mut Context<Self>) {
        if handles != self.handles {
            self.handles = handles;
            cx.notify();
        }
    }

    // `locked` marks the disabled handles a move from a handle stops at. The
    // api passes none.
    fn collapse_panel(
        &mut self,
        index: usize,
        reason: Reason,
        event: Option<InputEvent>,
        locked: &[bool],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let layout = self.current();
        let Some(panel) = self.constraints.get(index).copied() else {
            return false;
        };
        if !panel.collapsible || is_collapsed(&panel, layout.get(index).copied().unwrap_or(f64::NAN)) {
            return false;
        }
        let next = resize_panel(&layout, &self.constraints, index, panel.collapsed, 1.0, locked);
        is_collapsed(&panel, next.get(index).copied().unwrap_or(f64::NAN))
            && self.commit_now(next, reason, event, window, cx)
    }

    fn expand_panel(
        &mut self,
        index: usize,
        reason: Reason,
        event: Option<InputEvent>,
        locked: &[bool],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let layout = self.current();
        let Some(panel) = self.constraints.get(index).copied() else {
            return false;
        };
        if !is_collapsed(&panel, layout.get(index).copied().unwrap_or(f64::NAN)) {
            return false;
        }
        let remembered = self.ids.get(index).and_then(|id| self.expanded.get(id)).copied();
        let target = remembered.or(panel.initial).unwrap_or(panel.min).max(panel.min);
        let next = resize_panel(&layout, &self.constraints, index, target, 1.0, locked);
        !is_collapsed(&panel, next.get(index).copied().unwrap_or(f64::NAN))
            && self.commit_now(next, reason, event, window, cx)
    }

    // Collapses or expands the panel beside a handle (`toggle`).
    fn toggle_handle(
        &mut self,
        handle: usize,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = toggle_target(&self.constraints, handle) else {
            return;
        };
        let panel = self.constraints[target];
        let locked = self.locked();
        if is_collapsed(&panel, self.current().get(target).copied().unwrap_or(f64::NAN)) {
            self.expand_panel(target, Reason::Collapse, event, &locked, window, cx);
        } else {
            self.collapse_panel(target, Reason::Collapse, event, &locked, window, cx);
        }
    }

    /// Each panel's size in percent, in order.
    pub fn get_layout(&self) -> Vec<f64> {
        self.current()
    }

    /// Sets every panel's size at once, in percent. It's fitted to the
    /// panels' bounds and adds up to 100. Returns whether anything changed.
    pub fn set_layout(
        &mut self,
        layout: Vec<f64>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.refresh(window, cx);
        if layout.len() != self.constraints.len() {
            return false;
        }
        let next = fit_layout(&layout, &self.constraints);
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.commit_now(next, reason, options.event, window, cx)
    }

    /// Collapses a collapsible panel, by its id or its index. Returns whether
    /// it collapsed.
    pub fn collapse(
        &mut self,
        panel: impl Into<ResizablePanelRef>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self.index_of(&panel.into()) else {
            return false;
        };
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.collapse_panel(index, reason, options.event, &[], window, cx)
    }

    /// Expands a collapsed panel to the size it had before it collapsed, or
    /// its minimum. Returns whether it expanded.
    pub fn expand(
        &mut self,
        panel: impl Into<ResizablePanelRef>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self.index_of(&panel.into()) else {
            return false;
        };
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.expand_panel(index, reason, options.event, &[], window, cx)
    }

    /// Resizes one panel, by its id or its index, like to 240 pixels. Its
    /// neighbours give or take the difference.
    pub fn resize(
        &mut self,
        panel: impl Into<ResizablePanelRef>,
        size: impl Into<ResizableSize>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.refresh(window, cx);
        let target = to_percent(Some(size.into()), self.space, rem_size(window));
        let current = self.current();
        let (Some(index), Some(target)) = (self.index_of(&panel.into()), target) else {
            return false;
        };
        if index >= current.len() {
            return false;
        }
        let next = resize_panel(&current, &self.constraints, index, target, 0.5, &[]);
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.commit_now(next, reason, options.event, window, cx)
    }

    /// Whether a panel, by its id or its index, is collapsed.
    pub fn is_collapsed(&self, panel: impl Into<ResizablePanelRef>) -> bool {
        let Some(index) = self.index_of(&panel.into()) else {
            return false;
        };
        let current = self.current();
        self.constraints
            .get(index)
            .is_some_and(|panel| is_collapsed(panel, current.get(index).copied().unwrap_or(f64::NAN)))
    }

    /// A drag from a pointer. Deltas are percent of the space panels share,
    /// from where the drag started (`startDrag`).
    pub(crate) fn start_drag(
        &mut self,
        handle: usize,
        key: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_disabled() || self.drag.is_some() {
            return false;
        }
        self.refresh(window, cx);
        let base = self.current();
        self.drag = Some(Drag {
            handle,
            key: key.clone(),
            base,
        });
        self.dragging = Some(key);
        cx.notify();
        true
    }

    pub(crate) fn drag_by(
        &mut self,
        delta: f64,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = &self.drag else {
            return;
        };
        let next = resize_at(
            &drag.base,
            &self.constraints,
            drag.handle,
            delta,
            0.5,
            &self.locked(),
        );
        self.request(next, Reason::Drag, event, window, cx);
    }

    /// The drag ended. `restore` puts back the layout from before the drag,
    /// like on Escape (`endDrag`).
    pub(crate) fn end_drag(
        &mut self,
        event: Option<InputEvent>,
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ended) = self.drag.take() else {
            return;
        };
        self.dragging = None;
        cx.notify();
        // Bounds or space that changed since the last move fit as part of the
        // drag, once it's over, so this write can't end it a second time.
        let from = if restore {
            ended.base.clone()
        } else {
            self.current()
        };
        let next = fit_layout(&from, &self.constraints);
        self.request(next, Reason::Drag, event.clone(), window, cx);
        self.complete(ended.base, Reason::Drag, event, window, cx);
    }

    /// A key on a handle (`internal.keydown`).
    pub(crate) fn keydown(
        &mut self,
        event: &KeyDownEvent,
        key: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.handle_index(key) else {
            return;
        };
        if self.handle_disabled(key) {
            return;
        }
        // Alt, Control and Command with an arrow belong to the app and the
        // system. Only Shift changes the step.
        let modifiers = event.keystroke.modifiers;
        if modifiers.alt || modifiers.control || modifiers.platform {
            return;
        }
        let pressed = Key::of(&event.keystroke);
        if pressed == Key::Enter {
            window.prevent_default();
            self.refresh(window, cx);
            self.toggle_handle(handle, Some(InputEvent::from(event)), window, cx);
            return;
        }
        let layout = self.current();
        let delta = match pressed {
            Key::Home => Some(home_delta(&layout, &self.constraints, handle)),
            Key::End => Some(end_delta(&layout, &self.constraints, handle)),
            _ => key_delta(
                &pressed,
                KeyDeltaOptions {
                    vertical: self.vertical(),
                    rtl: is_rtl(cx),
                    step: self.settings.step,
                    large_step: self.settings.large_step,
                    shift_key: modifiers.shift,
                },
            ),
        };
        let Some(delta) = delta else {
            return;
        };
        window.prevent_default();
        if self.keys.is_none() {
            self.refresh(window, cx);
            // Focus leaving while the key is down sends its keyup elsewhere,
            // so the last keydown stands for it.
            let blur = self.handle_focus.borrow().get(key).cloned().map(|focus| {
                on_blur(&focus, window, cx, |group: &mut Self, window, cx| {
                    group.commit_keys(None, window, cx)
                })
            });
            self.keys = Some(Keys {
                start: self.current(),
                event: None,
                _blur: blur,
            });
        }
        if let Some(keys) = &mut self.keys {
            keys.event = Some(InputEvent::from(event));
        }
        let next = resize_at(
            &self.current(),
            &self.constraints,
            handle,
            delta,
            1.0,
            &self.locked(),
        );
        self.request(next, Reason::StepKey, Some(InputEvent::from(event)), window, cx);
    }

    /// The key came up, or focus left while it was down, so its keyup goes
    /// elsewhere and the last keydown stands for it (`internal.commitKeys`).
    pub(crate) fn commit_keys(
        &mut self,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(keys) = self.keys.take() else {
            return;
        };
        let Keys {
            start, event: last, ..
        } = keys;
        self.complete(start, Reason::StepKey, event.or(last), window, cx);
    }

    /// A double-click on a handle (`internal.toggle`).
    pub(crate) fn toggle(
        &mut self,
        key: &SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.handle_index(key) else {
            return;
        };
        if self.handle_disabled(key) {
            return;
        }
        self.refresh(window, cx);
        self.toggle_handle(handle, event, window, cx);
    }

    /// What a handle's separator says (`internal.handleValue`), worked out
    /// once per change to what it depends on.
    pub fn handle_value(&self, key: &SharedString) -> Option<HandleValue> {
        let mut handles: Vec<(SharedString, isize)> = self
            .handles
            .iter()
            .map(|(key, index)| (key.clone(), *index))
            .collect();
        handles.sort();
        let inputs: ShownInputs = (
            self.layout.get().clone(),
            self.ids.clone(),
            self.constraints.clone(),
            handles,
            self.locked(),
        );
        let mut shown = self.shown.borrow_mut();
        shown.get(inputs, shown_values).get(key).cloned()
    }

    // Disabling mid-drag ends the drag.
    pub(crate) fn end_if_disabled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_disabled() {
            return;
        }
        let Some(key) = self.drag.as_ref().map(|drag| drag.key.clone()) else {
            return;
        };
        self.stop_drags(window, cx);
        self.end_drag(None, false, window, cx);
        resizable_handles::release(window, cx.entity_id(), &key);
    }
}
