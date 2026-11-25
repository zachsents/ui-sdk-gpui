//! Port of `components/drawer/drawer.ts`: a dialog presented as a sheet.
//! Dialog's behavior and parts (focus, dismissal, nesting, Close actions,
//! open() that waits for a result) plus the sheet presentation from the
//! popups crate's `attach_sheet`: snap points, dragging with iOS physics, a
//! handle, modality that follows the snap point, the keyboard, and stacked
//! sheets pushing back the one beneath.
//!
//! The drawer renders through its viewport, a window-sized host above the
//! window that the content sits in: `Drawer::viewport(..)` with
//! `Drawer::content(..)` as its child, and the backdrop declared before
//! both.

use std::cell::{Cell, RefCell};
use std::fmt::Debug;
use std::panic;
use std::rc::Rc;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, Div, Element, ElementId, Entity,
    FocusHandle, GlobalElementId, InspectorElementId, InteractiveElement, Interactivity,
    IntoElement, LayoutId, ParentElement, Pixels, SharedString, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, deferred,
};
use ui_sdk_dialog::{
    DefinedDialog, DialogApi, DialogBackdrop, DialogCloseAction, DialogCloseProps,
    DialogCloseState, DialogDismissAttemptDetails, DialogFinalFocus, DialogHost,
    DialogInitialFocus, DialogOpenChangeCallback, DialogSettings,
    DialogShortcutProps, DialogTriggerProps, DialogTriggerState, PersistentElements, ShortcutPart,
    backdrop_part, content_keydown, define_dialog, dialog_close, dialog_description,
    dialog_shortcut, dialog_title, dialog_trigger,
};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentArgs, AgentError, AgentResult, AgentState, AgentValue, Aria,
    ChangeDetails, ChangeOptions, Controllable, FocusMode, InputEvent, Instance, Key, Part,
    Reason, json,
};
use ui_sdk_motion::{PresenceState, PresenceTiming, SheetPhysics, SnapPoint};
use ui_sdk_platform::{KeyboardInset, LAYER_PRIORITY, PhysicalSide, Region, TrackKeyboard};
use ui_sdk_popups::{
    DismissAttemptCallback, Modal, OpenChangeCompleteCallback, Popup, PopupContent,
    PopupContentProps, PopupHost, PopupOptions, ScrollArea, Sheet, SheetDriverName,
    SheetKeyframes, SheetLayer, SheetOutlet, SheetPartState, SheetSide, SheetThemeColor,
    SheetValues, bind_popup_content, window_box,
};

use crate::drawer_api::{
    DrawerSettings, DrawerShared, HandleAction, default_snap, interactive_by_order, shape_of,
};

/// The ways a user can dismiss a drawer, which `dismissible` can refuse
/// (`DrawerDismissReason`): `EscapeKey`, `OutsidePress`, `Back` and `Swipe`.
pub type DrawerDismissReason = ui_sdk_kernel::DismissAttemptReason;

/// What `on_dismiss_attempt` receives when a dismissal was refused
/// (`DrawerDismissAttemptDetails`).
pub type DrawerDismissAttemptDetails = DialogDismissAttemptDetails;

/// Hears the drawer move to another snap point: the snap point, and why:
/// `Swipe`, `HandlePress`, `HandleKey`, `ContentResize`, `Back` or
/// `Imperative` (`DrawerSnapPointChangeReason`). As `on_snap_point_change`,
/// it runs once the drawer's update is over, so it may update any entity,
/// the drawer too; as `on_before_snap_point_change`, it runs inside the
/// update, before the drawer moves, where `details.cancel()` keeps it where
/// it was.
pub type DrawerSnapPointChangeCallback = Rc<dyn Fn(&SnapPoint, &ChangeDetails, &mut Window, &mut App)>;

/// How the drawer's sheet moves (`DrawerDriver`).
pub type DrawerDriver = SheetDriverName;

/// Keyframes an outlet plays as the drawer expands (`DrawerKeyframes`).
pub type DrawerKeyframes = SheetKeyframes;

/// The edge a drawer comes from (`DrawerSide`). `Start` and `End` follow the
/// writing direction.
pub type DrawerSide = SheetSide;

/// The options `Drawer` takes (`DrawerOptions`).
pub struct DrawerOptions<P> {
    /// Whether the drawer is open. Pass it to control the drawer.
    pub open: Option<bool>,
    /// Whether it starts open, when uncontrolled.
    pub default_open: bool,
    /// Runs when the drawer opens or closes. `details.reason` says why,
    /// `Swipe` included, and the return value carries what closed it. It
    /// runs once the drawer's update is over, so it may update any entity,
    /// the drawer too.
    pub on_open_change: Option<DialogOpenChangeCallback>,
    /// Runs as the drawer is asked to open or close, before it does, inside
    /// the drawer's update: `details.cancel()` stops it. It hears the same
    /// reasons and return value as `on_open_change`. It may update any
    /// entity but the drawer.
    pub on_before_open_change: Option<DialogOpenChangeCallback>,
    /// Runs once the drawer has finished opening or closing, once the
    /// drawer's update is over.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Runs when the payload the content shows changes: a trigger with
    /// another payload opens the drawer, or takes a non-modal drawer over
    /// while it's open, which fires no `on_open_change`, and so does
    /// `open(payload)` on the open drawer. It runs once the drawer's update
    /// is over.
    pub on_payload_change: Option<ui_sdk_kernel::ChangeCallback<Option<P>>>,
    /// Runs as the payload is asked to change, before it does, inside the
    /// drawer's update: `details.cancel()` keeps the previous payload. It
    /// may update any entity but the drawer.
    pub on_before_payload_change: Option<ui_sdk_kernel::ChangeCallback<Option<P>>>,
    /// Whether Escape, an outside press, the back gesture and a swipe can
    /// close it. Close parts, triggers and the api always can. Defaults to
    /// true.
    pub dismissible: bool,
    /// Runs when a dismissal was refused because `dismissible` is false,
    /// including a swipe. The drawer resists and springs back. Ask "Discard
    /// changes?" here. It runs once the drawer's update is over.
    pub on_dismiss_attempt: Option<DismissAttemptCallback>,
    /// Regions that stay usable while the drawer blocks the window.
    pub persistent_elements: Option<PersistentElements>,
    /// What screen readers hear once a Close part's action has been running
    /// for a second. Defaults to "Loading".
    pub pending_announcement: Option<SharedString>,
    /// `True`: the rest of the window is blocked and doesn't scroll, focus is
    /// trapped, and an outside press only closes the drawer. `TrapFocus`:
    /// traps focus and leaves the window usable. `False`: a drawer beside the
    /// window. See `modal_above` to make it modal only above a snap point.
    /// Defaults to `True`.
    pub modal: Modal,
    /// Whether pressing outside the drawer, like on its backdrop, closes it.
    /// Defaults to true, or false while the drawer isn't modal.
    pub close_on_outside_press: Option<bool>,
    /// Where the drawer can rest, from the edge it comes from: a fraction of
    /// the window up to 1, pixels above 1, px, rem, a percentage, or
    /// `Content` for its own size. None can be larger than the drawer, which
    /// you size yourself. Defaults to `[Content]`.
    pub snap_points: Option<Vec<SnapPoint>>,
    /// The snap point it rests on. Pass it to control it, `None` inside for
    /// none yet.
    pub snap_point: Option<Option<SnapPoint>>,
    /// The snap point it opens at, when uncontrolled. It opens there again
    /// each time. Defaults to the first of `snap_points`.
    pub default_snap_point: Option<SnapPoint>,
    /// Runs when the drawer moves to another snap point, once the drawer's
    /// update is over.
    pub on_snap_point_change: Option<DrawerSnapPointChangeCallback>,
    /// Runs as the drawer is asked to move to another snap point, before it
    /// does, inside the drawer's update: `details.cancel()` keeps it where it
    /// was. It may update any entity but the drawer.
    pub on_before_snap_point_change: Option<DrawerSnapPointChangeCallback>,
    /// The edge it comes from. `Start` and `End` follow the writing
    /// direction. Defaults to `Bottom`.
    pub side: DrawerSide,
    /// At or below this snap point the window stays usable and undimmed,
    /// like Maps: nothing blocked, no `aria-modal`, and presses beside the
    /// drawer reach the window. Above it, `modal` applies. Changes once the
    /// drawer comes to rest.
    pub modal_above: Option<SnapPoint>,
    /// How it moves. `Pointer` follows touch, mouse and pen with a spring
    /// fitted to iOS. `ScrollSnap` uses the window's own scrolling for bottom
    /// drawers. `Auto` picks scroll snap for a bottom drawer on a touch
    /// screen, unless `modal_above` is set or `expand_on_scroll` is false,
    /// and the pointer driver otherwise. Defaults to `Auto`.
    pub driver: DrawerDriver,
    /// Whether dragging toward a larger snap point on scrollable content
    /// moves the drawer first, until it's at its largest. Defaults to true.
    pub expand_on_scroll: bool,
    /// Overrides the motion: spring, projection, rubber band and dim. Start
    /// from `SheetPhysics::IOS`.
    pub physics: Option<SheetPhysics>,
    /// The app's chrome colors, which dim with the backdrop, written where
    /// motion starts and ends, never per frame.
    pub theme_color: Option<SheetThemeColor>,
    /// Whether presses on the drawer near the window's left and right edges
    /// are kept from starting the platform's back gesture. Defaults to false.
    pub prevent_edge_swipe: bool,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl<P> Default for DrawerOptions<P> {
    fn default() -> Self {
        Self {
            open: None,
            default_open: false,
            on_open_change: None,
            on_before_open_change: None,
            on_open_change_complete: None,
            on_payload_change: None,
            on_before_payload_change: None,
            dismissible: true,
            on_dismiss_attempt: None,
            persistent_elements: None,
            pending_announcement: None,
            modal: Modal::True,
            close_on_outside_press: None,
            snap_points: None,
            snap_point: None,
            default_snap_point: None,
            on_snap_point_change: None,
            on_before_snap_point_change: None,
            side: SheetSide::Bottom,
            modal_above: None,
            driver: SheetDriverName::Auto,
            expand_on_scroll: true,
            physics: None,
            theme_color: None,
            prevent_edge_swipe: false,
            debug: false,
        }
    }
}

impl<P> Clone for DrawerOptions<P> {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_before_open_change: self.on_before_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            on_before_payload_change: self.on_before_payload_change.clone(),
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            persistent_elements: self.persistent_elements.clone(),
            pending_announcement: self.pending_announcement.clone(),
            modal: self.modal,
            close_on_outside_press: self.close_on_outside_press,
            snap_points: self.snap_points.clone(),
            snap_point: self.snap_point,
            default_snap_point: self.default_snap_point,
            on_snap_point_change: self.on_snap_point_change.clone(),
            on_before_snap_point_change: self.on_before_snap_point_change.clone(),
            side: self.side,
            modal_above: self.modal_above,
            driver: self.driver,
            expand_on_scroll: self.expand_on_scroll,
            physics: self.physics,
            theme_color: self.theme_color.clone(),
            prevent_edge_swipe: self.prevent_edge_swipe,
            debug: self.debug,
        }
    }
}

impl<P> DrawerOptions<P> {
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// `on_open_change`, for a callback that doesn't read what closed it.
    pub fn on_open_change(mut self, callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change = Some(Rc::new(move |open, details, _, window, cx| callback(open, details, window, cx)));
        self
    }

    /// `on_open_change` with what closed it: a Close part's `value`, or
    /// `close`'s own.
    pub fn on_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    /// `on_before_open_change`, for a callback that doesn't read what
    /// closes it.
    pub fn on_before_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change =
            Some(Rc::new(move |open, details, _, window, cx| callback(open, details, window, cx)));
        self
    }

    /// `on_before_open_change` with what closes it.
    pub fn on_before_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn on_payload_change(
        mut self,
        callback: impl Fn(&Option<P>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_payload_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_payload_change(
        mut self,
        callback: impl Fn(&Option<P>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_payload_change = Some(Rc::new(callback));
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(
        mut self,
        callback: impl Fn(&DrawerDismissAttemptDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
        self
    }

    pub fn persistent_elements(mut self, regions: Vec<Region>) -> Self {
        self.persistent_elements = Some(PersistentElements::List(regions));
        self
    }

    /// `persistent_elements` from a function, read each time it opens.
    pub fn persistent_elements_with(mut self, regions: impl Fn() -> Vec<Region> + 'static) -> Self {
        self.persistent_elements = Some(PersistentElements::Fn(Rc::new(regions)));
        self
    }

    pub fn pending_announcement(mut self, announcement: impl Into<SharedString>) -> Self {
        self.pending_announcement = Some(announcement.into());
        self
    }

    pub fn modal(mut self, modal: impl Into<Modal>) -> Self {
        self.modal = modal.into();
        self
    }

    pub fn close_on_outside_press(mut self, close: bool) -> Self {
        self.close_on_outside_press = Some(close);
        self
    }

    pub fn snap_points(mut self, points: impl IntoIterator<Item = SnapPoint>) -> Self {
        self.snap_points = Some(points.into_iter().collect());
        self
    }

    /// Controls the snap point, or with `None`, controls it as none yet.
    pub fn snap_point(mut self, point: impl Into<Option<SnapPoint>>) -> Self {
        self.snap_point = Some(point.into());
        self
    }

    pub fn default_snap_point(mut self, point: SnapPoint) -> Self {
        self.default_snap_point = Some(point);
        self
    }

    pub fn on_snap_point_change(
        mut self,
        callback: impl Fn(&SnapPoint, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_snap_point_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_snap_point_change(
        mut self,
        callback: impl Fn(&SnapPoint, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_snap_point_change = Some(Rc::new(callback));
        self
    }

    pub fn side(mut self, side: DrawerSide) -> Self {
        self.side = side;
        self
    }

    pub fn modal_above(mut self, point: SnapPoint) -> Self {
        self.modal_above = Some(point);
        self
    }

    pub fn driver(mut self, driver: DrawerDriver) -> Self {
        self.driver = driver;
        self
    }

    pub fn expand_on_scroll(mut self, expand: bool) -> Self {
        self.expand_on_scroll = expand;
        self
    }

    pub fn physics(mut self, physics: SheetPhysics) -> Self {
        self.physics = Some(physics);
        self
    }

    pub fn theme_color(mut self, theme_color: SheetThemeColor) -> Self {
        self.theme_color = Some(theme_color);
        self
    }

    pub fn prevent_edge_swipe(mut self, prevent: bool) -> Self {
        self.prevent_edge_swipe = prevent;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    pub(crate) fn settings(&self) -> DrawerSettings {
        DrawerSettings {
            snap_points: self.snap_points.clone(),
            default_snap_point: self.default_snap_point,
            side: self.side,
            modal_above: self.modal_above,
            driver: self.driver,
            expand_on_scroll: self.expand_on_scroll,
            physics: self.physics,
            theme_color: self.theme_color.clone(),
            prevent_edge_swipe: self.prevent_edge_swipe,
            modal: self.modal,
        }
    }

    // What the popup reads, and what Dialog's behavior reads.
    fn split(&self) -> (PopupOptions<P>, DialogSettings) {
        let settings = DialogSettings {
            dismissible: self.dismissible,
            persistent_elements: self.persistent_elements.clone(),
            pending_announcement: self.pending_announcement.clone(),
            close_on_outside_press: self.close_on_outside_press,
        };
        let popup = PopupOptions {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_before_open_change: self.on_before_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            on_before_payload_change: self.on_before_payload_change.clone(),
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            ..PopupOptions::default()
        };
        (popup, settings)
    }
}

/// The props the `content` slot takes (`DrawerContentProps`): Dialog's,
/// without `inline`, and what gpui can't find under a press.
#[derive(Clone, Default)]
pub struct DrawerContentProps {
    /// Where focus goes when the drawer opens. Opening with touch focuses the
    /// sheet itself, so no on-screen keyboard appears. Defaults to `Auto`.
    pub initial_focus: DialogInitialFocus,
    /// Where focus goes when the drawer closes while focus is inside it.
    pub final_focus: Option<DialogFinalFocus>,
    /// Whether the drawer stays in the page, hidden, while closed.
    pub keep_mounted: bool,
    /// How it animates in and out, beside the sheet's own motion.
    pub timing: Option<PresenceTiming>,
    /// The scroll areas inside the sheet: a drag on one scrolls it first,
    /// and once it's at its edge the same gesture moves the drawer.
    pub scroll_areas: Vec<ScrollArea>,
    /// What a mouse press doesn't drag the drawer from, like its fields and
    /// buttons, or a surface with drags of its own, like a canvas. Close
    /// parts never drag it; the handle always does.
    pub no_drag: Vec<Region>,
}

impl DrawerContentProps {
    pub fn initial_focus(mut self, focus: impl Into<DialogInitialFocus>) -> Self {
        self.initial_focus = focus.into();
        self
    }

    pub fn final_focus(mut self, focus: impl Into<DialogFinalFocus>) -> Self {
        self.final_focus = Some(focus.into());
        self
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = Some(timing);
        self
    }

    pub fn scroll_areas(mut self, areas: Vec<ScrollArea>) -> Self {
        self.scroll_areas = areas;
        self
    }

    pub fn no_drag(mut self, regions: Vec<Region>) -> Self {
        self.no_drag = regions;
        self
    }

    // The popup content props they amount to: the content renders in place,
    // inside its viewport, which goes above the window.
    fn popup_props(&self) -> PopupContentProps {
        PopupContentProps {
            initial_focus: self.initial_focus.clone(),
            final_focus: self.final_focus.clone(),
            keep_mounted: self.keep_mounted,
            inline: true,
            timing: self.timing.clone(),
            ..PopupContentProps::default()
        }
    }
}

/// The props the `outlet` slot takes (`DrawerOutletProps`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawerOutletProps {
    /// Keyframes it plays as the drawer expands, like `("scale", [1.0,
    /// 0.94])`: the first while collapsed, the last at the largest snap
    /// point.
    pub keyframes: Option<DrawerKeyframes>,
}

impl DrawerOutletProps {
    pub fn keyframes(mut self, keyframes: DrawerKeyframes) -> Self {
        self.keyframes = Some(keyframes);
        self
    }
}

/// The props the `trigger` slot takes (`DrawerTriggerProps`).
pub type DrawerTriggerProps<P> = DialogTriggerProps<P>;

/// The props the `close` slot takes.
pub type DrawerCloseProps = DialogCloseProps;

/// A Close part's `action`.
pub type DrawerCloseAction = DialogCloseAction;

/// What the `trigger` part styles from.
pub type DrawerTriggerState = DialogTriggerState;

/// What the `close` part styles from.
pub type DrawerCloseState = DialogCloseState;

/// What the `content` part styles from (`DrawerContentState`), with what the
/// original writes as attributes and variables.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawerContentState<P> {
    /// Whether the drawer is open. False while it animates out.
    pub open: bool,
    /// The `modal` in effect: none while the window behind it is usable.
    pub modal: Modal,
    /// Whether the drawer was opened from inside another drawer.
    pub nested: bool,
    /// Whether a drawer opened from inside this one is open.
    pub nested_open: bool,
    /// How many dialogs and drawers are open above this one, nested inside
    /// it.
    pub stack: usize,
    /// The payload of the last open.
    pub payload: Option<P>,
    /// Whether a Close part's action is running.
    pub pending: bool,
    /// The snap point it rests on or is moving to (`data-snap-point`).
    pub snap_point: Option<SnapPoint>,
    /// Whether it's at its largest snap point (`data-expanded`).
    pub expanded: bool,
    /// Whether a finger, mouse or pen is dragging it (`data-dragging`).
    pub dragging: bool,
    /// The edge it comes from, with `Start` and `End` resolved
    /// (`data-side`).
    pub side: PhysicalSide,
    /// Whether the window behind it is usable, because it rests at or below
    /// `modal_above` (`data-interactive`).
    pub interactive: bool,
    /// What moves it (`data-driver`).
    pub driver: DrawerDriver,
    /// Its enter and exit, the original's `data-starting-style` and
    /// `data-ending-style`. The drawer slides itself, so leave its position
    /// alone.
    pub presence: PresenceState,
    /// Kept in the page while closed, it renders hidden.
    pub hidden: bool,
    /// The sheet's dim, progress and expand values (`--drawer-dim`,
    /// `--drawer-progress`, `--drawer-expand`), opacity and drag, once it's
    /// attached.
    pub sheet: Option<SheetPartState>,
    /// How far the drawers opened from inside this one push it back,
    /// rising smoothly as each opens (`--drawer-stack`).
    pub sheet_stack: f64,
}

/// What the `backdrop` part styles from (`DrawerBackdropState`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawerBackdropState {
    /// Whether the drawer is open. False while it animates out.
    pub open: bool,
    /// Whether the drawer was opened from inside another drawer.
    pub nested: bool,
    /// Whether a drawer opened from inside this one is open.
    pub nested_open: bool,
    /// Whether the window behind is usable, so the backdrop lets presses
    /// through (`data-interactive`).
    pub interactive: bool,
    /// Whether a finger, mouse or pen is dragging the drawer
    /// (`data-dragging`).
    pub dragging: bool,
    /// How dark it is, from 0 to 1, which the drawer sets as its opacity.
    pub dim: f64,
    /// Its enter and exit.
    pub presence: PresenceState,
}

/// What the `viewport` part styles from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawerViewportState {
    /// Whether the drawer is open (`data-open`).
    pub open: bool,
    /// Kept in the page while closed, it renders hidden.
    pub hidden: bool,
    /// The edge the drawer comes from (`data-side`).
    pub side: PhysicalSide,
    /// How much the on-screen keyboard covers (`--keyboard-inset`,
    /// `--visual-viewport-height`, `data-keyboard`).
    pub keyboard: KeyboardInset,
}

/// What the `handle` part styles from (`DrawerHandleState`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawerHandleState {
    /// The snap point it rests on (`data-snap-point`).
    pub snap_point: Option<SnapPoint>,
    /// Whether there's a larger snap point to move to.
    pub can_expand: bool,
    /// Whether there's a smaller snap point to move to.
    pub can_collapse: bool,
    /// Whether it's at its largest snap point (`data-expanded`).
    pub expanded: bool,
}

/// What the `outlet` part styles from (`DrawerOutletState`), with the
/// values the original writes as variables.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawerOutletState {
    /// Whether the drawer is open. False as soon as it starts closing, while
    /// it still animates out.
    pub open: bool,
    /// The sheet's dim, progress and expand values as of its last frame
    /// (`--drawer-dim`, `--drawer-progress`, `--drawer-expand`).
    pub values: SheetValues,
    /// Each keyframed property's value now, by name.
    pub keyframes: Vec<(SharedString, f32)>,
}

impl DrawerOutletState {
    /// A keyframed property's value now, like `"scale"`.
    pub fn keyframe(&self, name: &str) -> Option<f32> {
        self.keyframes
            .iter()
            .find(|(own, _)| own.as_ref() == name)
            .map(|(_, value)| *value)
    }
}

/// A snap point from text (`snapPointOf`): numbers as numbers, like `"0.5"`,
/// and the rest as written: `"320px"`, `"20rem"`, `"50%"` or `"content"`.
pub fn snap_point_of(value: &str) -> Option<SnapPoint> {
    let trimmed = value.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return trimmed.parse().ok().map(SnapPoint::Number);
    }
    if trimmed == "content" {
        return Some(SnapPoint::Content);
    }
    let number = |unit: &str| trimmed.strip_suffix(unit).and_then(|amount| amount.trim().parse::<f64>().ok());
    if let Some(amount) = number("px") {
        return Some(SnapPoint::Px(amount));
    }
    if let Some(amount) = number("rem") {
        return Some(SnapPoint::Rem(amount));
    }
    number("%").map(SnapPoint::Percent)
}

/// A snap point given as text, like an attribute or an agent's step
/// (`snapPointAttribute`).
pub fn snap_point_attribute(attribute: Option<&str>) -> Option<SnapPoint> {
    attribute.and_then(snap_point_of)
}

/// Snap points given as comma-separated text, like `"0.5,1"`
/// (`snapPointsAttribute`).
pub fn snap_points_attribute(attribute: Option<&str>) -> Option<Vec<SnapPoint>> {
    attribute.map(|attribute| attribute.split(',').filter_map(snap_point_of).collect())
}

// A snap point as the agent bridge reads it: its number, or its text.
pub(crate) fn snap_point_value(point: Option<SnapPoint>) -> AgentValue {
    match point {
        None => AgentValue::Null,
        Some(SnapPoint::Number(value)) => json!(value),
        Some(point) => json!(point.id().to_string()),
    }
}

/// A drawer. Its entity is its controller: the app calls its methods to
/// open, close, move and inspect it, from anywhere. `P` is the type of the
/// data triggers and `open()` pass to the content.
pub struct Drawer<P: Clone + PartialEq + Debug + 'static = ()> {
    pub(crate) instance: Instance,
    pub(crate) popup: Popup<Self>,
    pub(crate) dialog: DialogApi<Self>,
    pub(crate) window: AnyWindowHandle,
    pub(crate) snap_point: Controllable<Option<SnapPoint>>,
    pub(crate) shared: Rc<DrawerShared>,
    pub(crate) sheet: Option<Sheet<Self>>,
    pub(crate) sheet_layer: Option<SheetLayer>,
    pub(crate) dragging: bool,
    pub(crate) interactive: bool,
    pub(crate) side: PhysicalSide,
    pub(crate) snap_index: Option<usize>,
    pub(crate) snap_count: usize,
    pub(crate) last_snap: Option<SnapPoint>,
    pub(crate) chosen_while_closed: bool,
    pub(crate) shape: crate::drawer_api::Shape,
    pub(crate) outlets: Rc<RefCell<Vec<OutletEntry>>>,
    pub(crate) keyboard: TrackKeyboard,
    pub(crate) handle_focus: FocusHandle,
    pub(crate) settling: Rc<Cell<bool>>,
}

// An outlet as it rendered: its key, what follows the sheet for it, and the
// keyframes it was made with.
pub(crate) struct OutletEntry {
    pub(crate) key: SharedString,
    pub(crate) outlet: SheetOutlet,
    pub(crate) keyframes: Option<DrawerKeyframes>,
}

impl<P: Clone + PartialEq + Debug + 'static> Drawer<P> {
    pub fn new(options: DrawerOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = options.settings();
        let open = options.open.unwrap_or(options.default_open);
        let initial = match options.snap_point {
            Some(controlled) => controlled,
            None => open.then(|| default_snap(&settings)),
        };
        let interactive = open && interactive_by_order(&settings, initial.as_ref());
        let mut snap_point = Controllable::new("snapPoint", initial);
        snap_point.set_controlled(options.snap_point);
        snap_point.set_on_change(snap_callback(options.on_snap_point_change.clone()));
        snap_point.set_on_before_change(snap_callback(options.on_before_snap_point_change.clone()));
        let (popup, dialog_settings) = options.split();
        let DefinedDialog { instance, popup, dialog } = define_dialog(
            "drawer",
            false,
            popup,
            dialog_settings,
            options.debug,
            crate::drawer_api::rules::<P>(),
            window,
            cx,
        );
        let shared = Rc::new(DrawerShared::new(settings.clone(), initial));
        let side = ui_sdk_popups::resolve_side(settings.side, ui_sdk_platform::is_rtl(cx));
        let mut this = Self {
            instance,
            popup,
            dialog,
            window: window.window_handle(),
            snap_point,
            shared,
            sheet: None,
            sheet_layer: None,
            dragging: false,
            interactive,
            side,
            snap_index: None,
            snap_count: settings.snap_points.as_ref().map_or(1, Vec::len),
            last_snap: initial,
            chosen_while_closed: false,
            shape: shape_of(&settings),
            outlets: Rc::new(RefCell::new(Vec::new())),
            keyboard: ui_sdk_platform::track_keyboard(None),
            handle_focus: cx.focus_handle(),
            settling: Rc::new(Cell::new(false)),
        };
        // The content renders in place, inside its viewport, from its first
        // open on.
        this.popup.set_content_props(DrawerContentProps::default().popup_props());
        // Its preset names no `modal`: its snap point decides.
        this.apply_modal(window, cx);
        this
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: DrawerOptions<P>, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        self.snap_point
            .set_on_change(snap_callback(options.on_snap_point_change.clone()));
        self.snap_point
            .set_on_before_change(snap_callback(options.on_before_snap_point_change.clone()));
        let settings = options.settings();
        let (popup, dialog_settings) = options.split();
        self.dialog.set_settings(dialog_settings);
        self.sync(settings, options.snap_point, cx);
        self.popup.set_options(popup, cx);
    }

    /// Passes the controlled `open`, or `None` to let the drawer keep its
    /// own.
    pub fn set_controlled_open(&mut self, open: Option<bool>, cx: &mut Context<Self>) {
        self.popup.set_controlled_open(open, cx);
    }

    /// Passes the controlled snap point, `Some(None)` for none yet, or `None`
    /// to let the drawer keep its own.
    pub fn set_controlled_snap_point(&mut self, point: Option<Option<SnapPoint>>, cx: &mut Context<Self>) {
        let settings = self.shared.settings.borrow().clone();
        self.sync(settings, point, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// Its Dialog behavior, which its parts call.
    pub fn api(&self) -> &DialogApi<Self> {
        &self.dialog
    }

    /// The `trigger` part: a button that opens the drawer, and presses
    /// itself with its `shortcut`.
    pub fn trigger(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: DrawerTriggerProps<P>,
        cx: &App,
    ) -> Part<DrawerTriggerState> {
        dialog_trigger(this, key, props, cx)
    }

    /// The `title` part, which names the drawer. Give it its text as an
    /// `aria_label`.
    pub fn title(this: &Entity<Self>, cx: &App) -> Part<()> {
        dialog_title(this, cx)
    }

    /// The `description` part, which describes it while it renders. Give it
    /// its text as an `aria_label`.
    pub fn description(this: &Entity<Self>, cx: &App) -> Part<()> {
        dialog_description(this, cx)
    }

    /// The `close` part: a button that closes the drawer with its `value`,
    /// after its `action`. Each has a key.
    pub fn close_part(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: DrawerCloseProps,
        cx: &App,
    ) -> Part<DrawerCloseState> {
        dialog_close(this, key, props, cx)
    }

    /// The `shortcut` part. Without keys of its own, it shows the shortcut
    /// of the trigger it sits in or beside.
    pub fn shortcut(
        this: &Entity<Self>,
        key: Option<SharedString>,
        props: DialogShortcutProps,
        cx: &App,
    ) -> ShortcutPart {
        dialog_shortcut(this, key, props, cx)
    }

    /// The `backdrop` part: it dims the window behind the drawer with the
    /// sheet, which sets its opacity, and lets presses through while the
    /// window behind is usable. A press on it is an outside press. `None`
    /// while it's out of the page. Declare it before the viewport.
    pub fn backdrop(
        this: &Entity<Self>,
        timing: Option<PresenceTiming>,
        cx: &App,
    ) -> Option<DialogBackdrop<Self, DrawerBackdropState>> {
        let drawer = this.read(cx);
        let dim = drawer.sheet.as_ref().map_or(0.0, |sheet| sheet.values().dim);
        let state = DrawerBackdropState {
            open: drawer.popup.is_open(),
            nested: drawer.dialog.nested(),
            nested_open: drawer.dialog.stack() > 0,
            interactive: drawer.interactive,
            dragging: drawer.dragging,
            dim,
            presence: drawer.popup.layer().backdrop_presence().state(),
        };
        // A usable window takes presses through the backdrop.
        let interactive = drawer.interactive;
        backdrop_part(this, state, timing, cx).map(|backdrop| {
            backdrop
                .in_place(false)
                .pass_through(interactive)
                .map_part(|part| part.opacity(dim as f32))
        })
    }

    /// The `viewport` part: the window-sized element the content sits in,
    /// above the window. Put the content inside it, and render it every
    /// frame: while the drawer is out of the page it renders hidden, and the
    /// content inside decides whether it's kept there.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> DrawerViewport<P> {
        let drawer = this.read(cx);
        let mounted = drawer.popup.is_mounted();
        let state = DrawerViewportState {
            open: drawer.popup.is_open(),
            hidden: !mounted,
            side: drawer.side,
            keyboard: drawer.keyboard.get(),
        };
        DrawerViewport {
            part: Part::new(&drawer.instance, "viewport", None, state),
            sheet: drawer.sheet.clone(),
            region: drawer.popup.viewport_region(),
            keyboard: drawer.keyboard.clone(),
            mounted,
        }
    }

    /// The `content` part: the sheet, a dialog named by its title, else its
    /// trigger, and described by its description. Put it in the viewport.
    /// `None` while it's out of the page; render it every frame, so it hears
    /// its props while closed.
    pub fn content(this: &Entity<Self>, props: DrawerContentProps, cx: &App) -> Option<DrawerContent<P>> {
        let drawer = this.read(cx);
        let popup = drawer.popup.clone();
        let popup_props = props.popup_props();
        drawer.shared.set_regions(props.scroll_areas, props.no_drag);
        if let Some(sheet) = &drawer.sheet {
            drawer.shared.hand_to(sheet, &drawer.dialog);
        }
        let popup_state = popup.state(drawer);
        let state = drawer.dialog.state(drawer);
        if !state.mounted && !props.keep_mounted {
            popup.set_content_props(popup_props);
            return None;
        }
        let instance = &drawer.instance;
        let sheet = drawer.sheet.clone();
        let content_state = DrawerContentState {
            open: state.open,
            modal: state.modal,
            nested: state.nested,
            nested_open: state.stack > 0,
            stack: state.stack,
            payload: state.payload.clone(),
            pending: state.pending.is_some(),
            snap_point: drawer.snap_point(),
            expanded: drawer.expanded(),
            dragging: drawer.dragging,
            side: drawer.side,
            interactive: drawer.interactive,
            driver: sheet
                .as_ref()
                .map_or(drawer.shared.settings.borrow().driver, Sheet::driver_name),
            presence: popup.layer().presence().state(),
            hidden: !state.mounted,
            sheet: sheet.as_ref().map(Sheet::part_state),
            sheet_stack: sheet.as_ref().map_or(0.0, |sheet| sheet.stack().stack()),
        };
        let handle = popup.content_focus_handle();
        let mut aria = Aria::new().hidden(!state.mounted);
        if let Some(name) = popup.trigger_name() {
            aria = aria.merge(name.content_aria(&popup, &popup_state, instance));
        }
        if state.described {
            aria = aria.described_by(instance.element_id("description", None));
        }
        let entity = this.downgrade();
        let part = Part::new(instance, popup.content_slot(), None, content_state)
            .aria_role(Role::Dialog)
            .map_el(|el| el.track_focus(&handle.tab_stop(false)))
            .aria(aria)
            .on_keys(&["Tab", "Escape"], move |event, window, cx| {
                content_keydown(&entity, event, window, cx)
            });
        let shown = popup.layer().is_shown();
        Some(DrawerContent {
            content: bind_popup_content(this, part, popup_props, cx),
            sheet: sheet.filter(|_| shown),
        })
    }

    /// The `handle` part: the grabber. A press goes to the next larger snap
    /// point, wrapping; arrow keys along the drawer's axis step without
    /// wrapping; Home and End go to the smallest and the largest. It's a
    /// button that drags the drawer; give it a name with `aria_label`.
    pub fn handle(this: &Entity<Self>, cx: &App) -> Part<DrawerHandleState> {
        let drawer = this.read(cx);
        let index = drawer.snap_index;
        let state = DrawerHandleState {
            snap_point: drawer.snap_point(),
            can_expand: index.is_some_and(|index| index + 1 < drawer.snap_count),
            can_collapse: index.is_some_and(|index| index > 0),
            expanded: drawer.expanded(),
        };
        let mut aria = Aria::new();
        if drawer.popup.is_mounted() {
            aria = aria.controls(drawer.instance.element_id(drawer.popup.content_slot(), None));
        }
        let pressed = this.downgrade();
        let keyed = this.downgrade();
        let part = Part::new(&drawer.instance, "handle", None, state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&drawer.handle_focus), false)
            .aria(aria)
            .on_press(move |event, window, cx| {
                pressed
                    .update(cx, |this, cx| {
                        this.step_handle(HandleAction::Cycle, Reason::HandlePress, Some(event), window, cx)
                    })
                    .ok();
            })
            .on_keys(
                &["Home", "End", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"],
                move |event, window, cx| {
                    let Some(drawer) = keyed.upgrade() else {
                        return;
                    };
                    let key = Key::of(&event.keystroke);
                    let side = drawer.read(cx).side;
                    let action = match key {
                        Key::Home => Some(HandleAction::First),
                        Key::End => Some(HandleAction::Last),
                        key => HandleAction::of_key(side, &key),
                    };
                    let Some(action) = action else {
                        return;
                    };
                    window.prevent_default();
                    let event = InputEvent::KeyDown(event.clone());
                    drawer.update(cx, |this, cx| {
                        this.step_handle(action, Reason::HandleKey, Some(event), window, cx)
                    });
                },
            );
        drawer.shared.handle.measure(part)
    }

    /// The `outlet` part: an element that follows the drawer, like the
    /// window's content behind it, through its keyframes and the sheet's
    /// values, from anywhere in the window. Each has a key.
    pub fn outlet(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: DrawerOutletProps,
        cx: &App,
    ) -> Part<DrawerOutletState> {
        let key = key.into();
        let drawer = this.read(cx);
        let outlet = drawer.register_outlet(&key, props.keyframes.clone());
        let keyframes = props
            .keyframes
            .iter()
            .flat_map(|keyframes| keyframes.0.iter())
            .filter_map(|(name, _)| outlet.keyframe(name).map(|value| (name.clone(), value)))
            .collect();
        let state = DrawerOutletState {
            open: drawer.popup.is_open(),
            values: outlet.values(),
            keyframes,
        };
        Part::new(&drawer.instance, "outlet", Some(&key), state)
    }
}

// `on_snap_point_change`, as the snap point's change callback: it changes
// only to a snap point.
fn snap_callback(
    callback: Option<DrawerSnapPointChangeCallback>,
) -> Option<ui_sdk_kernel::ChangeCallback<Option<SnapPoint>>> {
    let callback = callback?;
    Some(Rc::new(move |point: &Option<SnapPoint>, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
        if let Some(point) = point {
            callback(point, details, window, cx);
        }
    }))
}

/// A drawer's viewport part, the sheet's host, which renders above the
/// window. It's styled and filled like any part.
pub struct DrawerViewport<P: Clone + PartialEq + Debug + 'static> {
    part: Part<DrawerViewportState>,
    sheet: Option<Sheet<Drawer<P>>>,
    region: Region,
    keyboard: TrackKeyboard,
    mounted: bool,
}

impl<P: Clone + PartialEq + Debug + 'static> DrawerViewport<P> {
    pub fn state(&self) -> &DrawerViewportState {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &DrawerViewportState) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Styled for DrawerViewport<P> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl<P: Clone + PartialEq + Debug + 'static> InteractiveElement for DrawerViewport<P> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl<P: Clone + PartialEq + Debug + 'static> ParentElement for DrawerViewport<P> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl<P: Clone + PartialEq + Debug + 'static> IntoElement for DrawerViewport<P> {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        if !self.mounted {
            return self.region.measure(self.part.hidden()).into_any_element();
        }
        let part = self.keyboard.track(self.part);
        let host = match &self.sheet {
            Some(sheet) => sheet.host(part).into_any_element(),
            None => self.region.measure(part).into_any_element(),
        };
        deferred(window_box(host)).priority(LAYER_PRIORITY).into_any_element()
    }
}

/// A drawer's content part, the sheet itself, which moves along its axis
/// inside the viewport. It's styled and filled like any part.
pub struct DrawerContent<P: Clone + PartialEq + Debug + 'static> {
    content: PopupContent<Drawer<P>, DrawerContentState<P>>,
    sheet: Option<Sheet<Drawer<P>>>,
}

impl<P: Clone + PartialEq + Debug + 'static> DrawerContent<P> {
    pub fn state(&self) -> &DrawerContentState<P> {
        self.content.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &DrawerContentState<P>) -> Stateful<Div>) -> Self {
        self.content = self.content.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters, as on any part.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.content = self.content.aria(aria);
        self
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Styled for DrawerContent<P> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.content.style()
    }
}

impl<P: Clone + PartialEq + Debug + 'static> InteractiveElement for DrawerContent<P> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.content.interactivity()
    }
}

impl<P: Clone + PartialEq + Debug + 'static> StatefulInteractiveElement for DrawerContent<P> {}

impl<P: Clone + PartialEq + Debug + 'static> ParentElement for DrawerContent<P> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements)
    }
}

impl<P: Clone + PartialEq + Debug + 'static> IntoElement for DrawerContent<P> {
    type Element = DrawerContentElement;

    fn into_element(self) -> Self::Element {
        let child = match &self.sheet {
            Some(sheet) => sheet.body(self.content).into_any_element(),
            None => self.content.into_any_element(),
        };
        DrawerContentElement { child }
    }
}

/// What a [`DrawerContent`] renders as: the sheet, or the content before it
/// has one.
pub struct DrawerContentElement {
    child: AnyElement,
}

impl Element for DrawerContentElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

impl<P: Clone + PartialEq + Debug + 'static> PopupHost for Drawer<P> {
    type Payload = P;

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

impl<P: Clone + PartialEq + Debug + 'static> DialogHost for Drawer<P> {
    fn dialog(&self) -> &DialogApi<Self> {
        &self.dialog
    }
}

fn set_snap_point_action<P: Clone + PartialEq + Debug + 'static>(
    drawer: &mut Drawer<P>,
    args: &AgentArgs,
    window: &mut Window,
    cx: &mut Context<Drawer<P>>,
) -> AgentResult {
    let point = match args.value(0) {
        AgentValue::Number(number) => number.as_f64().map(SnapPoint::Number),
        AgentValue::String(text) => snap_point_of(text),
        _ => None,
    };
    let Some(point) = point else {
        return Err(AgentError::Takes {
            action: "setSnapPoint".into(),
            expected: "a snap point",
            given: args.value(0).clone(),
        });
    };
    Ok(drawer
        .set_snap_point(point, ChangeOptions::default(), window, cx)
        .into())
}

/// `agent: { actions: ["setOpen", "setSnapPoint"] }` and `controlled: ["open",
/// "snapPoint"]`, with `open`, `close` and `toggle`, which `setOpen` covers,
/// unlisted.
impl<P: Clone + PartialEq + Debug + 'static> Agent for Drawer<P> {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", ui_sdk_dialog::dialog_primitive::set_open_action::<Self>),
        AgentAction::new("setSnapPoint", set_snap_point_action::<P>),
        AgentAction::unlisted("open", ui_sdk_dialog::dialog_primitive::open_action::<Self>),
        AgentAction::unlisted("close", ui_sdk_dialog::dialog_primitive::close_action::<Self>),
        AgentAction::unlisted("toggle", ui_sdk_dialog::dialog_primitive::toggle_action::<Self>),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["open", "snapPoint"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        ui_sdk_dialog::dialog_primitive::dialog_agent_state(self).with("snapPoint", snap_point_value(self.snap_point()))
    }
}
