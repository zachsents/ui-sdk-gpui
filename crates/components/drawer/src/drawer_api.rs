//! Port of `components/drawer/drawer-api.ts`: the drawer's state and api:
//! Dialog's api plus the sheet. It keeps the snap point, modality and drag
//! state, tells the sheet where to go, and hears back through the callbacks
//! it hands `attach_sheet`.
//!
//! The original keeps these in its store and follows them with a
//! subscription; here the drawer's fields hold them, and each change calls
//! what that subscription did.

use std::cell::{Cell, RefCell};
use std::fmt::Debug;
use std::rc::Rc;

use gpui::{App, Context, SharedString, Window};
use ui_sdk_dialog::{DialogHost, DialogRules, DialogState};
use ui_sdk_kernel::dev::{self, Origin};
use ui_sdk_kernel::{ChangeOptions, InputEvent, Key, Reason};
use ui_sdk_motion::{ResolvedSnapPoint, SheetPhysics, SheetSnapReason, SnapPoint, find_snap_index, is_at_or_below};
use ui_sdk_platform::{Layer, PhysicalSide, Region};
use ui_sdk_popups::{
    Dismissal, HideRequest, Modal, PopupCloseOptions, PopupOpenOptions, PopupOpener,
    PopupPending, Presentation, ScrollArea, Sheet, SheetDriverName, SheetLayout, SheetOptions,
    SheetOutlet, SheetSide, SheetStack, SheetThemeColor, ShowCleanup, attach_sheet, drawer_preset,
    parent_sheet, register_sheet_layer,
};

use crate::drawer::{Drawer, DrawerKeyframes, OutletEntry};

/// The drawer's state: Dialog's, and its own (`DrawerState`).
#[derive(Clone, Debug, PartialEq)]
pub struct DrawerState<P> {
    /// Dialog's state.
    pub dialog: DialogState<P>,
    /// The snap point it rests on or is moving to, `None` before it first
    /// opens.
    pub snap_point: Option<SnapPoint>,
    /// Whether a finger, mouse or pen is dragging it.
    pub dragging: bool,
    /// Whether the window behind it is usable, because it rests at or below
    /// `modal_above`.
    pub interactive: bool,
    /// The `modal` option, which applies above `modal_above`.
    pub modal_setting: Modal,
    /// The edge it comes from, with `Start` and `End` resolved.
    pub side: PhysicalSide,
    /// Where its snap point is among the resolved ones, once measured.
    pub snap_index: Option<usize>,
    /// How many snap points it resolved to.
    pub snap_count: usize,
}

// What the drawer reads from its options, which the sheet reads through
// getters.
#[derive(Clone)]
pub(crate) struct DrawerSettings {
    pub(crate) snap_points: Option<Vec<SnapPoint>>,
    pub(crate) default_snap_point: Option<SnapPoint>,
    pub(crate) side: SheetSide,
    pub(crate) modal_above: Option<SnapPoint>,
    pub(crate) driver: SheetDriverName,
    pub(crate) expand_on_scroll: bool,
    pub(crate) physics: Option<SheetPhysics>,
    pub(crate) theme_color: Option<SheetThemeColor>,
    pub(crate) prevent_edge_swipe: bool,
    pub(crate) modal: Modal,
}

// What the drawer shares with its sheet, which reads it outside the
// drawer's update.
pub(crate) struct DrawerShared {
    pub(crate) settings: RefCell<DrawerSettings>,
    // The snap point, as the sheet's getter reads it.
    pub(crate) snap: Cell<Option<SnapPoint>>,
    // The snap points the sheet resolved, ascending, for the handle and the
    // back gesture.
    pub(crate) resolved: RefCell<Vec<ResolvedSnapPoint>>,
    // The sheet this drawer opened from, which it pushes back while open.
    pub(crate) stacked_on: RefCell<Option<SheetStack>>,
    pub(crate) scroll_areas: RefCell<Vec<ScrollArea>>,
    pub(crate) no_drag: RefCell<Vec<Region>>,
    pub(crate) handle: Region,
    // What new options asked of the sheet, done once they're handed over.
    pub(crate) reshape: Cell<bool>,
    pub(crate) refresh: Cell<bool>,
    pub(crate) snap_moved: Cell<bool>,
}

impl DrawerShared {
    pub(crate) fn new(settings: DrawerSettings, snap: Option<SnapPoint>) -> Self {
        Self {
            settings: RefCell::new(settings),
            snap: Cell::new(snap),
            resolved: RefCell::new(Vec::new()),
            stacked_on: RefCell::new(None),
            scroll_areas: RefCell::new(Vec::new()),
            no_drag: RefCell::new(Vec::new()),
            handle: Region::new(),
            reshape: Cell::new(false),
            refresh: Cell::new(false),
            snap_moved: Cell::new(false),
        }
    }

    // The content's scroll areas and what doesn't drag, as it renders.
    pub(crate) fn set_regions(&self, scroll_areas: Vec<ScrollArea>, no_drag: Vec<Region>) {
        *self.scroll_areas.borrow_mut() = scroll_areas;
        *self.no_drag.borrow_mut() = no_drag;
    }

    // Hands them to the sheet. Close parts never drag it, and the handle, a
    // button that drags, always does.
    pub(crate) fn hand_to<T: DialogHost>(&self, sheet: &Sheet<T>, dialog: &ui_sdk_dialog::DialogApi<T>) {
        sheet.set_scroll_areas(self.scroll_areas.borrow().clone());
        let mut no_drag = self.no_drag.borrow().clone();
        no_drag.extend(dialog.close_regions());
        no_drag.retain(|region| *region != self.handle);
        sheet.set_no_drag(no_drag);
    }
}

/// The first snap point, which a drawer opens at unless told otherwise
/// (`defaultSnap`).
pub(crate) fn default_snap(settings: &DrawerSettings) -> SnapPoint {
    settings
        .default_snap_point
        .or_else(|| settings.snap_points.as_ref().and_then(|points| points.first().copied()))
        .unwrap_or(SnapPoint::Content)
}

/// Before the sheet is measured, whether a snap point is at or below
/// `modal_above`, by their order in `snap_points`, which is ascending in
/// practice (`interactiveByOrder`).
pub(crate) fn interactive_by_order(settings: &DrawerSettings, point: Option<&SnapPoint>) -> bool {
    let (Some(modal_above), Some(point)) = (settings.modal_above, point) else {
        return false;
    };
    let points = settings
        .snap_points
        .clone()
        .unwrap_or_else(|| vec![SnapPoint::Content]);
    let above = points.iter().position(|entry| *entry == modal_above);
    let index = points.iter().position(|entry| entry == point);
    matches!((above, index), (Some(above), Some(index)) if index <= above)
}

/// What a sheet reads only as it attaches: `Auto` picks its driver from the
/// rest (`shapeOf`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shape {
    side: SheetSide,
    driver: SheetDriverName,
    prevent_edge_swipe: bool,
    auto: Option<(bool, bool)>,
}

pub(crate) fn shape_of(settings: &DrawerSettings) -> Shape {
    Shape {
        side: settings.side,
        driver: settings.driver,
        prevent_edge_swipe: settings.prevent_edge_swipe,
        auto: (settings.driver == SheetDriverName::Auto)
            .then(|| (settings.modal_above.is_none(), settings.expand_on_scroll)),
    }
}

/// Where a handle moves the drawer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HandleAction {
    /// The next larger snap point, wrapping to the smallest, like iOS's
    /// grabber.
    Cycle,
    /// One larger, without wrapping.
    Expand,
    /// One smaller, without wrapping.
    Collapse,
    /// The smallest.
    First,
    /// The largest.
    Last,
}

impl HandleAction {
    /// What an arrow key along the drawer's axis does on its handle
    /// (`handleKeys`): toward the window's middle expands it.
    pub fn of_key(side: PhysicalSide, key: &Key) -> Option<Self> {
        use HandleAction::{Collapse, Expand};
        use PhysicalSide::{Bottom, Left, Right, Top};
        match (side, key) {
            (Bottom, Key::ArrowUp) | (Top, Key::ArrowDown) | (Left, Key::ArrowRight) | (Right, Key::ArrowLeft) => {
                Some(Expand)
            }
            (Bottom, Key::ArrowDown) | (Top, Key::ArrowUp) | (Left, Key::ArrowLeft) | (Right, Key::ArrowRight) => {
                Some(Collapse)
            }
            _ => None,
        }
    }
}

/// The snap point a handle moves to, by index among `count`, from `index`,
/// `-1` when it rests on none. `Cycle` goes to the next larger one and wraps
/// to the smallest; `Expand` and `Collapse` step once without wrapping;
/// `First` and `Last` jump to the ends. `-1` without snap points
/// (`stepSnapIndex`).
pub fn step_snap_index(index: isize, count: usize, action: HandleAction) -> isize {
    if count == 0 {
        return -1;
    }
    let count = count as isize;
    match action {
        HandleAction::First => 0,
        HandleAction::Last => count - 1,
        HandleAction::Cycle => (index + 1).rem_euclid(count),
        HandleAction::Expand => (count - 1).min(index + 1),
        HandleAction::Collapse => (index - 1).max(0),
    }
}

/// What the drawer adds to Dialog's behavior: Drawer's preset, the sheet
/// attaching as the content binds and letting go once it leaves the page,
/// rising as the popup layer shows it, settling closed as it closes, the
/// back gesture stepping it down, and a refused dismissal nudging it.
pub(crate) fn rules<P: Clone + PartialEq + Debug + 'static>() -> DialogRules<Drawer<P>> {
    DialogRules {
        preset: Some(drawer_preset()),
        on_bind: Some(Rc::new(|this: &mut Drawer<P>, window: &mut Window, cx: &mut Context<Drawer<P>>| {
            this.bound(window, cx)
        })),
        on_unbind: Some(Rc::new(|this: &mut Drawer<P>, window: &mut Window, cx: &mut Context<Drawer<P>>| {
            this.detach(window, cx)
        })),
        on_show: Some(Rc::new(|this: &mut Drawer<P>, layer: &Layer, window: &mut Window, cx: &mut Context<Drawer<P>>| {
            this.shown(layer, window, cx)
        })),
        on_open_change: Some(Rc::new(|this: &mut Drawer<P>, open, window, cx| this.open_change(open, window, cx))),
        on_open_change_complete: Some(Rc::new(|this: &mut Drawer<P>, open, window, cx| {
            this.open_change_complete(open, window, cx)
        })),
        intercept: Some(Rc::new(|this: &mut Drawer<P>, reason, event, window, cx| {
            this.intercept(reason, event, window, cx)
        })),
        on_refuse: Some(Rc::new(|this: &mut Drawer<P>, _reason, window, cx| {
            if let Some(sheet) = this.sheet.clone() {
                sheet.nudge(this, window, cx);
            }
        })),
        presentation: Presentation::Sheet,
        bind: None,
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Drawer<P> {
    /// Its state, as its parts and the app read it.
    pub fn state(&self) -> DrawerState<P> {
        DrawerState {
            dialog: self.dialog.state(self),
            snap_point: self.snap_point(),
            dragging: self.dragging,
            interactive: self.interactive,
            modal_setting: self.shared.settings.borrow().modal,
            side: self.side,
            snap_index: self.snap_index,
            snap_count: self.snap_count,
        }
    }

    /// Whether it's open. False as soon as it starts closing.
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// Opens or closes it. Returns false when the change was canceled or it
    /// was already open or closed. Closing this way always works, even when
    /// it isn't dismissible or an action is running.
    pub fn set_open(&mut self, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.set_open(self, open, options, window, cx)
    }

    /// Opens it, and resolves once it closes, with why, how and any return
    /// value.
    pub fn open(&mut self, options: PopupOpenOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        let popup = self.popup.clone();
        popup.open(self, options, window, cx)
    }

    /// Closes it, with a return value for the future `open()` returned.
    pub fn close(&mut self, options: PopupCloseOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.close(self, options, window, cx)
    }

    /// Opens it when it's closed, and closes it when it's open.
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.toggle(self, options, window, cx)
    }

    /// The payload of the last open.
    pub fn payload(&self) -> Option<P> {
        self.popup.payload()
    }

    /// Moves focus to the trigger that opened it last, or the first trigger.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.popup.focus(window, cx);
    }

    /// The snap point it rests on, or `None` before it first opens
    /// (`getSnapPoint`).
    pub fn snap_point(&self) -> Option<SnapPoint> {
        *self.snap_point.get()
    }

    /// Moves it to a snap point; while closed, it opens there next. Returns
    /// false when the change was canceled or it was already there
    /// (`setSnapPoint`). The reason defaults to `Imperative`.
    pub fn set_snap_point(&mut self, point: SnapPoint, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        self.snap_to(point, options, window, cx)
    }

    /// Whether it rests at its largest snap point.
    pub fn expanded(&self) -> bool {
        self.snap_count > 0 && self.snap_index == Some(self.snap_count - 1)
    }

    // The snap points the sheet resolved, ascending, for the handle and the
    // back gesture.
    fn resolved(&self) -> Vec<ResolvedSnapPoint> {
        self.shared.resolved.borrow().clone()
    }

    // A change to the snap point, through `on_snap_point_change`. A
    // controlled one moves once the app passes it back.
    fn snap_to(&mut self, point: SnapPoint, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let before = self.snap_point();
        let changed = self
            .snap_point
            .change(&self.instance, Some(point), options, window, cx);
        if self.snap_point() != before {
            self.snap_point_changed(window, cx);
        }
        changed
    }

    // What an open starts with: the default snap point, unless it's
    // controlled or was chosen while closed, and whether the window behind is
    // usable there.
    fn opened(&self) -> (Option<SnapPoint>, bool) {
        let settings = self.shared.settings.borrow();
        let current = *self.snap_point.get();
        let snap = if !self.snap_point.is_controlled() && current.is_none() {
            Some(default_snap(&settings))
        } else {
            current
        };
        (snap, interactive_by_order(&settings, snap.as_ref()))
    }

    // Uncontrolled drawers open at their default snap point again, from the
    // moment they unmount.
    fn forgets_snap(&self) -> bool {
        !self.snap_point.is_controlled() && self.snap_point.get().is_some()
    }

    // The snap point changed, from any source: the sheet moves there while
    // open, and while closed, it's where the drawer opens next.
    pub(crate) fn snap_point_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = *self.snap_point.get();
        self.shared.snap.set(current);
        if current == self.last_snap {
            return;
        }
        self.last_snap = current;
        if !self.popup.is_open() {
            self.chosen_while_closed = current.is_some();
        } else if let Some(point) = current
            && let Some(sheet) = self.sheet.clone()
        {
            sheet.snap_to(self, &point, SheetSnapReason::Imperative, window, cx);
        }
        cx.notify();
    }

    // The modal setting in effect: none while the window behind is usable.
    // Whatever changes it applies to the window at once.
    pub(crate) fn apply_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let modal = if self.interactive {
            Modal::False
        } else {
            self.shared.settings.borrow().modal
        };
        let popup = self.popup.clone();
        popup.set_modal(self, modal, window, cx);
    }

    // New options and a controlled snap point arrive.
    pub(crate) fn sync(&mut self, settings: DrawerSettings, snap_point: Option<Option<SnapPoint>>, cx: &mut Context<Self>) {
        let before = self.shared.settings.replace(settings.clone());
        let mut changed = self.snap_point.set_controlled(snap_point);
        // An uncontrolled snap point the new snap points leave out moves to
        // the default, or to their first when they leave that out too.
        if !self.snap_point.is_controlled()
            && let Some(point) = *self.snap_point.get()
        {
            let points = settings
                .snap_points
                .clone()
                .unwrap_or_else(|| vec![SnapPoint::Content]);
            if !points.contains(&point) {
                let fallback = default_snap(&settings);
                let next = if points.contains(&fallback) || points.is_empty() {
                    fallback
                } else {
                    points[0]
                };
                changed |= self.snap_point.set_held(Some(next));
            }
        }
        // A sheet picks its driver, side and touch guard as it attaches, so
        // a change to them attaches it again where it rests. New snap points
        // it measures again.
        let shape = shape_of(&settings);
        let reshaped = shape != self.shape;
        let moved = before.snap_points != settings.snap_points;
        self.shape = shape;
        self.shared.reshape.set(self.shared.reshape.get() || reshaped);
        self.shared.refresh.set(self.shared.refresh.get() || moved);
        self.shared.snap_moved.set(self.shared.snap_moved.get() || changed);
        if changed || reshaped || moved || before.modal != settings.modal || before.modal_above != settings.modal_above {
            self.settle(cx);
            cx.notify();
        }
    }

    // Once the options are handed over, so the sheet's state isn't written
    // beside them.
    fn settle(&mut self, cx: &mut Context<Self>) {
        if self.settling.replace(true) {
            return;
        }
        let window = self.window;
        let entity = cx.weak_entity();
        let app: &mut App = cx;
        app.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    entity
                        .update(cx, |this, cx| {
                            this.settling.set(false);
                            this.apply_modal(window, cx);
                            if this.shared.snap_moved.replace(false) {
                                this.snap_point_changed(window, cx);
                            }
                            let reshaped = this.shared.reshape.replace(false);
                            let moved = this.shared.refresh.replace(false);
                            if reshaped || moved {
                                this.reshape(reshaped, window, cx);
                            }
                        })
                        .ok();
                })
                .ok();
        });
    }

    fn reshape(&mut self, reshaped: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.sheet.clone() else {
            return;
        };
        if !reshaped {
            sheet.refresh(self, window, cx);
            return;
        }
        self.attach(window, cx);
        let Some(sheet) = self.sheet.clone() else {
            return;
        };
        if let Some(layer) = self.popup.layer().layer() {
            self.sheet_layer.take();
            self.sheet_layer = Some(register_sheet_layer(&layer, sheet.stack()));
        }
        if self.popup.is_open() {
            sheet.open(self, false, window, cx);
        }
    }

    // The content binds: the sheet attaches to it before the popup layer
    // shows it. A drawer kept in the page binds again as it opens, and keeps
    // the sheet it has.
    pub(crate) fn bound(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet.is_none() {
            self.attach(window, cx);
        }
    }

    // The popup layer shows the content: the sheet rises, unless it's open
    // from the start.
    pub(crate) fn shown(&mut self, layer: &Layer, window: &mut Window, cx: &mut Context<Self>) -> Option<ShowCleanup<Self>> {
        let sheet = self.sheet.clone()?;
        self.sheet_layer.take();
        self.sheet_layer = Some(register_sheet_layer(layer, sheet.stack()));
        *self.shared.stacked_on.borrow_mut() = parent_sheet(layer);
        // A closed drawer stays closed.
        if self.popup.is_open() {
            let animate = self.popup.layer().opener() != PopupOpener::Initial;
            sheet.open(self, animate, window, cx);
        }
        Some(Box::new(|this: &mut Self, _window: &mut Window, _cx: &mut Context<Self>| {
            this.sheet_layer = None;
        }))
    }

    // It starts opening or closing. An open starts where it starts; a close
    // settles the sheet closed, and the exit waits for it.
    pub(crate) fn open_change(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open {
            self.chosen_while_closed = false;
            let (snap, interactive) = self.opened();
            self.snap_point.set_held(snap);
            self.interactive = interactive;
            self.apply_modal(window, cx);
            self.snap_point_changed(window, cx);
            return;
        }
        let Some(sheet) = self.sheet.clone() else {
            return;
        };
        sheet.close(self, window, cx);
        if let Some(remaining) = sheet.motion_remaining() {
            self.popup.layer().presence().track_animation(remaining);
        }
    }

    // An open or close finished. A snap point set while closed, even during
    // the close animation, is where it opens next; otherwise, out of the
    // page, it forgets it.
    pub(crate) fn open_change_complete(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open || self.popup.is_mounted() || self.popup.is_open() {
            return;
        }
        if !self.chosen_while_closed && self.forgets_snap() {
            self.snap_point.set_held(None);
            self.snap_point_changed(window, cx);
        }
    }

    // The back gesture steps the drawer down a snap point before it closes
    // it.
    pub(crate) fn intercept(
        &mut self,
        reason: Reason,
        event: Option<&InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if reason != Reason::Back {
            return false;
        }
        let resolved = self.resolved();
        let index = find_snap_index(&resolved, self.snap_point().as_ref());
        let Some(index) = index.filter(|index| *index > 0) else {
            return false;
        };
        let options = ChangeOptions {
            reason: Some(Reason::Back),
            event: event.cloned(),
            source: None,
        };
        self.snap_to(resolved[index - 1].value, options, window, cx);
        true
    }

    // The resting snap point changes, at release or when a move starts.
    fn sheet_snapped(
        &mut self,
        point: SnapPoint,
        reason: Reason,
        event: Option<&InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snap_point() == Some(point) {
            return;
        }
        self.last_snap = Some(point);
        let options = ChangeOptions {
            reason: Some(reason),
            event: event.cloned(),
            source: None,
        };
        let changed = self.snap_to(point, options, window, cx);
        if changed || !self.popup.is_open() {
            return;
        }
        // Refused: back to where it was.
        self.last_snap = self.snap_point();
        if let Some(last) = self.last_snap
            && let Some(sheet) = self.sheet.clone()
        {
            sheet.snap_to(self, &last, SheetSnapReason::Imperative, window, cx);
        }
    }

    // It came to rest. Modality follows the snap point it rests on.
    fn sheet_rested(&mut self, point: Option<SnapPoint>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(point) = point else {
            return;
        };
        let resolved = self.resolved();
        let modal_above = self.shared.settings.borrow().modal_above;
        self.interactive = is_at_or_below(&resolved, Some(&point), modal_above.as_ref());
        self.snap_index = find_snap_index(&resolved, Some(&point));
        self.snap_count = resolved.len();
        self.apply_modal(window, cx);
        cx.notify();
    }

    // A swipe dismissed it; it's already settling closed.
    fn sheet_dismissed(&mut self, event: Option<&InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = self.dialog.clone();
        dialog.hide(
            self,
            HideRequest::new().reason(Reason::Swipe).event(event.cloned()),
            window,
            cx,
        );
        // A close the app refused, or a controlled one it hasn't confirmed by
        // the next frame, springs back.
        let entity = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            entity
                .update(cx, |this, cx| {
                    let Some(snap) = this.snap_point() else {
                        return;
                    };
                    if this.popup.is_open()
                        && let Some(sheet) = this.sheet.clone()
                    {
                        sheet.snap_to(this, &snap, SheetSnapReason::Imperative, window, cx);
                    }
                })
                .ok();
        });
    }

    fn sheet_dragged(&mut self, dragging: bool, cx: &mut Context<Self>) {
        if self.dragging != dragging {
            self.dragging = dragging;
            cx.notify();
        }
    }

    // The geometry changed: the resolved side and snap points.
    fn sheet_laid_out(&mut self, layout: SheetLayout, cx: &mut Context<Self>) {
        self.snap_index = find_snap_index(&layout.snap_points, self.snap_point().as_ref());
        self.snap_count = layout.snap_points.len();
        self.side = layout.side;
        *self.shared.resolved.borrow_mut() = layout.snap_points;
        cx.notify();
    }

    // One sheet per drawer: attaching again replaces the last one.
    pub(crate) fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sheet) = self.sheet.take() {
            sheet.destroy(self, window, cx);
        }
        let shared = self.shared.clone();
        let mut options = SheetOptions::new(
            self.popup.viewport_region(),
            |this: &mut Self, event: Option<&InputEvent>, window: &mut Window, cx: &mut Context<Self>| {
                this.sheet_dismissed(event, window, cx)
            },
        );
        {
            let shared = shared.clone();
            options.side = Some(Rc::new(move || shared.settings.borrow().side));
        }
        {
            let shared = shared.clone();
            options.snap_points = Some(Rc::new(move || {
                shared
                    .settings
                    .borrow()
                    .snap_points
                    .clone()
                    .unwrap_or_else(|| vec![SnapPoint::Content])
            }));
        }
        {
            let shared = shared.clone();
            options.snap_point = Some(Rc::new(move || shared.snap.get()));
        }
        // A swipe is refused like any dismissal.
        if let Some(dismissal) = self.popup.dismissal().map(Dismissal::sheet) {
            options.dismissible = Some(dismissal.dismissible.clone());
            let attempt = dismissal.on_dismiss_attempt.clone();
            options.on_dismiss_attempt = Some(Rc::new(move |_this: &mut Self, event, window, cx| {
                attempt(event, window, cx)
            }));
        }
        {
            let shared = shared.clone();
            options.modal_above = Some(Rc::new(move || shared.settings.borrow().modal_above));
        }
        {
            let shared = shared.clone();
            options.driver = Some(Rc::new(move || shared.settings.borrow().driver));
        }
        {
            let shared = shared.clone();
            options.expand_on_scroll = Some(Rc::new(move || shared.settings.borrow().expand_on_scroll));
        }
        {
            let shared = shared.clone();
            options.physics = Some(Rc::new(move || shared.settings.borrow().physics));
        }
        {
            let instance = self.instance.handle();
            let region = self.popup.backdrop_region();
            options.backdrop = Some(Rc::new(move || {
                instance
                    .bound_parts()
                    .iter()
                    .any(|part| part.slot == "backdrop")
                    .then(|| region.clone())
            }));
        }
        {
            // Outlets in the page, each with its latest keyframes.
            let outlets = self.outlets.clone();
            let instance = self.instance.handle();
            options.outlets = Some(Rc::new(move || {
                let bound: Vec<SharedString> = instance
                    .bound_parts()
                    .into_iter()
                    .filter(|part| part.slot == "outlet")
                    .filter_map(|part| part.key)
                    .collect();
                outlets
                    .borrow()
                    .iter()
                    .filter(|entry| bound.contains(&entry.key))
                    .map(|entry| entry.outlet.clone())
                    .collect()
            }));
        }
        {
            let shared = shared.clone();
            options.parent = Some(Rc::new(move || shared.stacked_on.borrow().clone()));
        }
        {
            let shared = shared.clone();
            options.theme_color = Some(Rc::new(move || shared.settings.borrow().theme_color.clone()));
        }
        {
            let shared = shared.clone();
            options.prevent_edge_swipe = Some(Rc::new(move || shared.settings.borrow().prevent_edge_swipe));
        }
        options.on_snap = Some(Rc::new(|this: &mut Self, point, reason: SheetSnapReason, event, window, cx| {
            this.sheet_snapped(point, reason.reason(), event, window, cx)
        }));
        options.on_rest = Some(Rc::new(|this: &mut Self, point, window, cx| this.sheet_rested(point, window, cx)));
        options.on_drag_change = Some(Rc::new(|this: &mut Self, dragging, _window, cx| this.sheet_dragged(dragging, cx)));
        options.on_layout = Some(Rc::new(|this: &mut Self, layout, _window, cx| this.sheet_laid_out(layout, cx)));
        let sheet = attach_sheet(self.popup.content_region(), options, cx);
        self.shared.hand_to(&sheet, &self.dialog);
        self.sheet = Some(sheet);
        if dev::DEV
            && self
                .shared
                .settings
                .borrow()
                .snap_points
                .as_ref()
                .is_some_and(Vec::is_empty)
        {
            dev::warn(
                "`snap_points` is empty, so it rests at its own size.",
                Some(Origin::new("drawer", self.instance.id())),
            );
        }
    }

    // The sheet lets go once the content has left the page.
    pub(crate) fn detach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sheet) = self.sheet.take() {
            sheet.destroy(self, window, cx);
        }
    }

    // The handle moves it to another snap point.
    pub(crate) fn step_handle(
        &mut self,
        action: HandleAction,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let resolved = self.resolved();
        let current = find_snap_index(&resolved, self.snap_point().as_ref()).map_or(-1, |index| index as isize);
        let index = step_snap_index(current, resolved.len(), action);
        if index < 0 {
            return;
        }
        let options = ChangeOptions {
            reason: Some(reason),
            event,
            source: None,
        };
        self.snap_to(resolved[index as usize].value, options, window, cx);
    }

    // An outlet in the page, which follows the sheet with its keyframes. One
    // that renders with other keyframes follows anew.
    pub(crate) fn register_outlet(&self, key: &SharedString, keyframes: Option<DrawerKeyframes>) -> SheetOutlet {
        let mut outlets = self.outlets.borrow_mut();
        if let Some(entry) = outlets
            .iter()
            .find(|entry| entry.key == *key && entry.keyframes == keyframes)
        {
            return entry.outlet.clone();
        }
        outlets.retain(|entry| entry.key != *key);
        let view = self.instance.entity_id();
        let outlet = match &keyframes {
            Some(keyframes) => SheetOutlet::with_keyframes(view, keyframes.clone()),
            None => SheetOutlet::new(view),
        };
        outlets.push(OutletEntry {
            key: key.clone(),
            outlet: outlet.clone(),
            keyframes,
        });
        outlet
    }
}
