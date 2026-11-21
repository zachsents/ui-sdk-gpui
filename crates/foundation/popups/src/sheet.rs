//! Port of `foundation/popups/sheet.ts`: the sheet presentation. It makes an
//! element a sheet that rests on snap points, follows a finger, and moves
//! the window's effects with it. Drawer uses it for its content, next to its
//! popup layer, and a popup presented as a sheet through
//! [`present_sheet`](crate::present_sheet).
//!
//! It owns the geometry (snap points from the window, the content and the
//! keyboard), picks the driver, wires the effects, follows the keyboard, and
//! stacks sheets opened from inside each other. The component keeps the
//! state: it hears about snaps, rests and dismissals through callbacks and
//! tells the sheet where to go.
//!
//! The sheet renders through two elements: its host, a box the size of the
//! window it sits in ([`Sheet::host`]), and its body, the sheet itself
//! ([`Sheet::body`]), which the sheet moves along its axis as it paints, as
//! the original's `translate`. Its values, the dim and its progress, are
//! state its parts style from ([`Sheet::part_state`]).

use std::cell::{Cell, RefCell};
use std::panic;
use std::rc::{Rc, Weak};
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, ContentMask, Context, Element, ElementId, GlobalElementId,
    InspectorElementId, InteractiveElement, IntoElement, LayoutId, ParentElement, Pixels, Point,
    Rgba, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, point, px,
};
use ui_sdk_drag_and_drop::on_pointer_down;
use ui_sdk_kernel::InputEvent;
use ui_sdk_motion::{
    ContentScroll, ResolvedSnapPoint, SheetConfig as MachineConfig, SheetPhysics, SheetSnapPoint,
    SheetSnapReason, SnapPoint, SnapPointContext, find_snap_index, is_at_or_below,
    resolve_snap_points,
};
use ui_sdk_platform::{
    KeyboardChange, Layer, PhysicalSide, Region, TrackKeyboard, is_ios, is_rtl, track_keyboard,
};

use crate::popup::RegionFn;
use crate::sheet_axis::{ScrollArea, SheetSide, is_vertical, resolve_side, size_along, translate_value};
use crate::sheet_driver::{SheetDriverName, SheetDriverOptions, closed_extent_for};
use crate::sheet_effects::{
    SheetEffects, SheetEffectsTargets, SheetOutlet, SheetStack, SheetValues, create_sheet_effects,
};
use crate::sheet_pointer::{PointerDriver, create_pointer_driver};
use crate::sheet_scroll_snap::{ScrollSnapDriver, create_scroll_snap_driver};
use crate::theme_color::{ThemeColor, dim_theme_color};

/// The resolved geometry, which `on_layout` hears.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetLayout {
    pub side: PhysicalSide,
    pub snap_points: Vec<ResolvedSnapPoint>,
}

/// The chrome colors a sheet dims with its backdrop, and how the app paints
/// them.
#[derive(Clone)]
pub struct SheetThemeColor {
    pub colors: Vec<Rgba>,
    pub apply: ThemeSink,
}

/// Paints the chrome colors a sheet writes.
pub type ThemeSink = Rc<dyn Fn(Vec<Rgba>, &mut App)>;

/// Runs inside the component's update.
pub type SheetCallback<T, A> = Rc<dyn Fn(&mut T, A, &mut Window, &mut Context<T>)>;

/// What a sheet reads from its component (`SheetOptions`). Only `host` and
/// `on_dismiss` are required; the rest default to a bottom sheet at its own
/// size that can be dismissed. Getters are read when they matter, so option
/// changes apply while it's open.
pub struct SheetOptions<T: 'static> {
    /// The window-sized element the sheet sits in.
    pub host: Region,
    /// The edge it comes from. Defaults to the bottom.
    pub side: Option<Rc<dyn Fn() -> SheetSide>>,
    /// Where it can rest. Defaults to its own size, `Content`.
    pub snap_points: Option<Rc<dyn Fn() -> Vec<SnapPoint>>>,
    /// The snap point it should rest on, from the component's state.
    /// Defaults to the first.
    pub snap_point: Option<Rc<dyn Fn() -> Option<SnapPoint>>>,
    /// Whether a swipe can dismiss it. Defaults to true.
    pub dismissible: Option<Rc<dyn Fn() -> bool>>,
    /// At or below this snap point the window stays usable and undimmed.
    pub modal_above: Option<Rc<dyn Fn() -> Option<SnapPoint>>>,
    /// Defaults to `Auto`.
    pub driver: Option<Rc<dyn Fn() -> SheetDriverName>>,
    /// Whether a drag on scrollable content grows the sheet first. Defaults
    /// to true.
    pub expand_on_scroll: Option<Rc<dyn Fn() -> bool>>,
    /// How it moves. Defaults to the iOS preset.
    pub physics: Option<Rc<dyn Fn() -> Option<SheetPhysics>>>,
    pub backdrop: Option<RegionFn>,
    /// Dims with the host itself instead of a backdrop part, like a popup
    /// presented as a sheet.
    pub host_backdrop: bool,
    pub outlets: Option<Rc<dyn Fn() -> Vec<SheetOutlet>>>,
    /// The sheet this one opened from, which it pushes back while open.
    pub parent: Option<Rc<dyn Fn() -> Option<SheetStack>>>,
    /// The app's chrome colors, which dim with the backdrop, read as the
    /// sheet opens. `None` leaves them alone.
    pub theme_color: Option<Rc<dyn Fn() -> Option<SheetThemeColor>>>,
    /// Keeps the back gesture from starting on the sheet or its backdrop.
    pub prevent_edge_swipe: Option<Rc<dyn Fn() -> bool>>,
    /// Whether a mouse press at a point may drag the sheet. Defaults to
    /// everywhere but the regions [`Sheet::set_no_drag`] names.
    pub can_mouse_drag: Option<Rc<dyn Fn(Point<Pixels>, &Window, &App) -> bool>>,
    /// A swipe dismissed it; it's already settling closed. Close the
    /// component here.
    pub on_dismiss: Rc<dyn Fn(&mut T, Option<&InputEvent>, &mut Window, &mut Context<T>)>,
    /// The resting snap point changed, at release or when a programmatic move
    /// starts.
    pub on_snap: Option<Rc<dyn Fn(&mut T, SnapPoint, SheetSnapReason, Option<&InputEvent>, &mut Window, &mut Context<T>)>>,
    /// It came to rest, on a snap point or closed (`None`).
    pub on_rest: Option<SheetCallback<T, Option<SnapPoint>>>,
    /// A swipe tried to dismiss it while `dismissible` is false.
    pub on_dismiss_attempt: Option<Rc<dyn Fn(&mut T, Option<&InputEvent>, &mut Window, &mut Context<T>)>>,
    pub on_drag_change: Option<SheetCallback<T, bool>>,
    /// The geometry changed: the resolved side and snap points.
    pub on_layout: Option<SheetCallback<T, SheetLayout>>,
}

impl<T: 'static> SheetOptions<T> {
    pub fn new(
        host: Region,
        on_dismiss: impl Fn(&mut T, Option<&InputEvent>, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        Self {
            host,
            side: None,
            snap_points: None,
            snap_point: None,
            dismissible: None,
            modal_above: None,
            driver: None,
            expand_on_scroll: None,
            physics: None,
            backdrop: None,
            host_backdrop: false,
            outlets: None,
            parent: None,
            theme_color: None,
            prevent_edge_swipe: None,
            can_mouse_drag: None,
            on_dismiss: Rc::new(on_dismiss),
            on_snap: None,
            on_rest: None,
            on_dismiss_attempt: None,
            on_drag_change: None,
            on_layout: None,
        }
    }
}

/// What a sheet's parts style from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetPartState {
    /// The dim, progress and expand values, as of its last frame.
    pub values: SheetValues,
    /// How opaque the sheet is: below 1 while it fades in or out under
    /// reduced motion.
    pub opacity: f64,
    /// Whether a finger or the mouse drags it now.
    pub dragging: bool,
    /// The edge it comes from.
    pub side: PhysicalSide,
    /// Whether its scroll areas stay still, so a swipe on them moves the
    /// sheet, with the scroll-snap driver.
    pub scroll_locked: bool,
}

/// Whether a mouse press at `point` may drag a sheet: not on one of the
/// regions that don't drag it, its controls (`isDragSurface`).
pub fn is_drag_surface(point: Point<Pixels>, no_drag: &[Region], window: &Window) -> bool {
    !no_drag.iter().any(|region| region.contains_point(point, window))
}

thread_local! {
    static SHEETS: RefCell<Vec<(u64, Region)>> = const { RefCell::new(Vec::new()) };
    static NEXT_SHEET: Cell<u64> = const { Cell::new(1) };
    static SHEET_LAYERS: RefCell<Vec<(Layer, SheetStack)>> = const { RefCell::new(Vec::new()) };
    // The hosts of sheets that keep the back gesture from starting on them.
    static EDGE_SWIPE: RefCell<Vec<(u64, Region)>> = const { RefCell::new(Vec::new()) };
}

/// Whether a back gesture starting at `point` starts on a sheet that keeps
/// it from starting there (`preventEdgeSwipe`), so it closes nothing.
pub fn prevents_edge_swipe(point: Point<Pixels>, window: &Window) -> bool {
    EDGE_SWIPE.with(|hosts| {
        hosts
            .borrow()
            .iter()
            .any(|(_, host)| host.bounds().is_some_and(|bounds| bounds.contains(&point)) && host.focus_handle().is_none_or(|handle| handle.is_rendered(window)))
    })
}

// The innermost sheet at a point, so a press on a sheet stacked above
// another is that one's.
fn sheet_at(point: Point<Pixels>, window: &Window) -> Option<Region> {
    SHEETS.with(|sheets| {
        sheets
            .borrow()
            .iter()
            .rev()
            .find(|(_, region)| region.contains_point(point, window))
            .map(|(_, region)| region.clone())
    })
}

/// Open sheets by their popup layer, so a sheet opened from inside another
/// stacks on it (`sheetLayers`). Dropping the guard unregisters it.
#[must_use = "dropping it unregisters the sheet's layer"]
pub struct SheetLayer {
    layer: Layer,
}

impl Drop for SheetLayer {
    fn drop(&mut self) {
        let layer = self.layer.clone();
        SHEET_LAYERS.with(|layers| {
            if let Ok(mut layers) = layers.try_borrow_mut() {
                layers.retain(|(own, _)| *own != layer);
            }
        });
    }
}

/// Registers the sheet a layer shows, until the guard drops.
pub fn register_sheet_layer(layer: &Layer, sheet: SheetStack) -> SheetLayer {
    SHEET_LAYERS.with(|layers| layers.borrow_mut().push((layer.clone(), sheet)));
    SheetLayer { layer: layer.clone() }
}

/// The sheet of the nearest layer above `layer` that has one (`parentSheet`).
pub fn parent_sheet(layer: &Layer) -> Option<SheetStack> {
    let mut up = layer.parent();
    while let Some(current) = up {
        let found = SHEET_LAYERS.with(|layers| {
            layers
                .borrow()
                .iter()
                .find(|(own, _)| *own == current)
                .map(|(_, sheet)| sheet.clone())
        });
        if found.is_some() {
            return found;
        }
        up = current.parent();
    }
    None
}

/// How the iOS keyboard slides: 250ms on UIKit's keyboard curve, as commonly
/// measured. A sheet riding on it moves the same way.
const KEYBOARD_DURATION: f64 = 250.0;

fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let sample = |a: f64, b: f64, s: f64| {
        let inverse = 1.0 - s;
        3.0 * inverse * inverse * s * a + 3.0 * inverse * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0.0, 1.0);
    let mut s = t;
    for _ in 0..24 {
        let x = sample(x1, x2, s);
        if (x - t).abs() < 1e-5 {
            break;
        }
        if x < t {
            low = s;
        } else {
            high = s;
        }
        s = (low + high) / 2.0;
    }
    sample(y1, y2, s)
}

pub(crate) enum SheetDriverKind<T: 'static> {
    Pointer(PointerDriver<T>),
    ScrollSnap(ScrollSnapDriver<T>),
}

impl<T: 'static> Clone for SheetDriverKind<T> {
    fn clone(&self) -> Self {
        match self {
            SheetDriverKind::Pointer(driver) => SheetDriverKind::Pointer(driver.clone()),
            SheetDriverKind::ScrollSnap(driver) => SheetDriverKind::ScrollSnap(driver.clone()),
        }
    }
}

struct SheetData {
    side: PhysicalSide,
    resolved: Vec<ResolvedSnapPoint>,
    size: f64,
    host_size: f64,
    config: MachineConfig,
    measured: bool,
    destroyed: bool,
    opened: bool,
    keyboard_from: f64,
    keyboard_slide: Option<(Instant, f64)>,
    viewport_height: Option<Pixels>,
}

struct SheetInner<T: 'static> {
    id: u64,
    options: SheetOptions<T>,
    el: Region,
    entity: WeakEntity<T>,
    data: RefCell<SheetData>,
    effects: SheetEffects,
    driver: RefCell<Option<SheetDriverKind<T>>>,
    stack: SheetStack,
    ancestors: RefCell<Vec<SheetStack>>,
    // The chrome colors it dims, and how the app paints them, once it has
    // opened with some.
    theme: RefCell<Option<(ThemeColor, ThemeSink)>>,
    keyboard: TrackKeyboard,
    scroll_areas: RefCell<Vec<ScrollArea>>,
    no_drag: RefCell<Vec<Region>>,
    me: Weak<SheetInner<T>>,
}

impl<T: 'static> Drop for SheetInner<T> {
    fn drop(&mut self) {
        let id = self.id;
        SHEETS.with(|sheets| {
            if let Ok(mut sheets) = sheets.try_borrow_mut() {
                sheets.retain(|(own, _)| *own != id);
            }
        });
        EDGE_SWIPE.with(|hosts| {
            if let Ok(mut hosts) = hosts.try_borrow_mut() {
                hosts.retain(|(own, _)| *own != id);
            }
        });
    }
}

/// A sheet attached to an element, for the component that owns it
/// (`Sheet`). Clones share it.
pub struct Sheet<T: 'static> {
    inner: Rc<SheetInner<T>>,
}

impl<T: 'static> Clone for Sheet<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Makes `el`, the sheet's region, a sheet of the component being updated,
/// sitting in `options.host` (`attachSheet`). Render the host with
/// [`Sheet::host`] and the sheet with [`Sheet::body`], every frame, and
/// [`Sheet::destroy`] it once it's gone.
pub fn attach_sheet<T: 'static>(el: Region, options: SheetOptions<T>, cx: &mut Context<T>) -> Sheet<T> {
    let side = resolve_side(options.side.as_ref().map_or(SheetSide::Bottom, |side| side()), is_rtl(cx));
    let id = NEXT_SHEET.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    SHEETS.with(|sheets| sheets.borrow_mut().push((id, el.clone())));
    let entity = cx.weak_entity();
    let owner = Some(cx.entity_id());
    let sheet = Sheet {
        inner: Rc::new_cyclic(|me: &Weak<SheetInner<T>>| {
            let ancestors_of = me.clone();
            let outlets = options.outlets.clone();
            let theme_of = me.clone();
            let effects = create_sheet_effects(SheetEffectsTargets {
                outlets: Rc::new(move || outlets.as_ref().map(|outlets| outlets()).unwrap_or_default()),
                ancestors: Rc::new(move || {
                    ancestors_of
                        .upgrade()
                        .map(|inner| inner.ancestors.borrow().clone())
                        .unwrap_or_default()
                }),
                theme_color: options.theme_color.as_ref().map(|_| {
                    Rc::new(move |dim: f64, cx: &mut App| {
                        let Some(inner) = theme_of.upgrade() else {
                            return;
                        };
                        let written = inner
                            .theme
                            .borrow()
                            .as_ref()
                            .and_then(|(theme, apply)| theme.write(dim).map(|colors| (colors, apply.clone())));
                        if let Some((colors, apply)) = written {
                            apply(colors, cx);
                        }
                    }) as Rc<dyn Fn(f64, &mut App)>
                }),
            });
            let keyboard_before = me.clone();
            let keyboard_after = me.clone();
            let keyboard_entity = entity.clone();
            let keyboard = track_keyboard(Some(KeyboardChange {
                before: Rc::new(move |_window, _cx| {
                    if let Some(inner) = keyboard_before.upgrade() {
                        let top = inner.el.bounds().map_or(0.0, |bounds| f64::from(f32::from(bounds.top())));
                        inner.data.borrow_mut().keyboard_from = top;
                    }
                }),
                after: Rc::new(move |window, cx| {
                    let Some(inner) = keyboard_after.upgrade() else {
                        return;
                    };
                    let sheet = Sheet { inner };
                    keyboard_entity
                        .update(cx, |this, cx| sheet.follow_keyboard(this, window, cx))
                        .ok();
                }),
            }));
            SheetInner {
                id,
                stack: SheetStack::new(el.clone(), owner),
                el,
                entity,
                data: RefCell::new(SheetData {
                    side,
                    resolved: Vec::new(),
                    size: 0.0,
                    host_size: 0.0,
                    config: MachineConfig {
                        snap_points: Vec::new(),
                        dimension: 0.0,
                        dismissible: true,
                        expand_on_scroll: true,
                        physics: SheetPhysics::IOS,
                        content_scroll: Some(ContentScroll),
                    },
                    measured: false,
                    destroyed: false,
                    opened: false,
                    keyboard_from: 0.0,
                    keyboard_slide: None,
                    viewport_height: None,
                }),
                effects,
                driver: RefCell::new(None),
                ancestors: RefCell::new(Vec::new()),
                theme: RefCell::new(None),
                keyboard,
                scroll_areas: RefCell::new(Vec::new()),
                no_drag: RefCell::new(Vec::new()),
                options,
                me: me.clone(),
            }
        }),
    };
    sheet.read_config();
    let driver_options = sheet.driver_options();
    let driver = if side == PhysicalSide::Bottom && sheet.uses_scroll_snap() {
        SheetDriverKind::ScrollSnap(create_scroll_snap_driver(driver_options, cx))
    } else {
        SheetDriverKind::Pointer(create_pointer_driver(driver_options, cx))
    };
    *sheet.inner.driver.borrow_mut() = Some(driver);
    sheet
}

impl<T: 'static> Sheet<T> {
    fn from_weak(weak: &Weak<SheetInner<T>>) -> Option<Self> {
        weak.upgrade().map(|inner| Sheet { inner })
    }

    fn driver(&self) -> SheetDriverKind<T> {
        self.inner
            .driver
            .borrow()
            .clone()
            .expect("the sheet picks its driver as it attaches")
    }

    // `Auto` picks scroll snap on touch screens without hover, where the
    // platform moves the sheet with native scrolling and hands off to content
    // scrolling itself. Mouse and pen keep the pointer driver, as does a sheet
    // the window stays usable beside, and one whose content scrolls before it
    // grows, which only the pointer driver can hand off.
    fn uses_scroll_snap(&self) -> bool {
        let options = &self.inner.options;
        let driver = options.driver.as_ref().map_or(SheetDriverName::Auto, |driver| driver());
        if driver != SheetDriverName::Auto {
            return driver == SheetDriverName::ScrollSnap;
        }
        options.modal_above.as_ref().is_none_or(|modal_above| modal_above().is_none())
            && options.expand_on_scroll.as_ref().is_none_or(|expand| expand())
            && is_ios()
    }

    fn read_config(&self) -> MachineConfig {
        let options = &self.inner.options;
        let physics = options
            .physics
            .as_ref()
            .and_then(|physics| physics())
            .unwrap_or(SheetPhysics::IOS);
        let dismissible = options.dismissible.as_ref().is_none_or(|dismissible| dismissible());
        let expand_on_scroll = options.expand_on_scroll.as_ref().is_none_or(|expand| expand());
        let mut data = self.inner.data.borrow_mut();
        data.config.physics = physics;
        data.config.dismissible = dismissible;
        data.config.expand_on_scroll = expand_on_scroll;
        data.config.clone()
    }

    fn snap_points_option(&self) -> Vec<SnapPoint> {
        self.inner
            .options
            .snap_points
            .as_ref()
            .map(|points| points())
            .unwrap_or_else(|| vec![SnapPoint::Content])
    }

    fn snap_point_option(&self) -> Option<SnapPoint> {
        self.inner.options.snap_point.as_ref().and_then(|point| point())
    }

    fn modal_above(&self) -> Option<SnapPoint> {
        self.inner.options.modal_above.as_ref().and_then(|modal_above| modal_above())
    }

    fn value_of(&self, id: Option<&SharedString>) -> Option<SnapPoint> {
        let id = id?;
        self.inner
            .data
            .borrow()
            .resolved
            .iter()
            .find(|point| &point.id == id)
            .map(|point| point.value)
    }

    fn driver_options(&self) -> SheetDriverOptions<T> {
        let me = || self.inner.me.clone();
        let side = me();
        let config = me();
        let size = me();
        let closed = me();
        let owns = me();
        let drag = me();
        let areas = me();
        let snap = me();
        let rest = me();
        let interactive = me();
        let frame = me();
        let dismiss = self.inner.options.on_dismiss.clone();
        let attempt = self.inner.options.on_dismiss_attempt.clone();
        let drag_change = self.inner.options.on_drag_change.clone();
        SheetDriverOptions {
            host: self.inner.options.host.clone(),
            side: Rc::new(move || side.upgrade().map_or(PhysicalSide::Bottom, |inner| inner.data.borrow().side)),
            config: Rc::new(move || {
                Self::from_weak(&config).map_or_else(
                    || MachineConfig {
                        snap_points: Vec::new(),
                        dimension: 0.0,
                        dismissible: true,
                        expand_on_scroll: true,
                        physics: SheetPhysics::IOS,
                        content_scroll: Some(ContentScroll),
                    },
                    |sheet| sheet.read_config(),
                )
            }),
            size: Rc::new(move || size.upgrade().map_or(0.0, |inner| inner.data.borrow().size)),
            closed_extent: Rc::new(move || {
                closed
                    .upgrade()
                    .map_or(-32.0, |inner| closed_extent_for(inner.data.borrow().side))
            }),
            reduced_motion: Rc::new(|cx: &App| cx.reduce_motion()),
            owns: Rc::new(move |point: Point<Pixels>, window: &Window| {
                owns.upgrade()
                    .is_some_and(|inner| sheet_at(point, window).is_none_or(|found| found == inner.el))
            }),
            can_mouse_drag: Rc::new(move |point: Point<Pixels>, window: &Window, cx: &App| {
                let Some(inner) = drag.upgrade() else {
                    return false;
                };
                match &inner.options.can_mouse_drag {
                    Some(can) => can(point, window, cx),
                    None => is_drag_surface(point, &inner.no_drag.borrow(), window),
                }
            }),
            scroll_areas: Rc::new(move || {
                areas
                    .upgrade()
                    .map(|inner| inner.scroll_areas.borrow().clone())
                    .unwrap_or_default()
            }),
            on_snap: Rc::new(move |this: &mut T, id: SharedString, reason, event, window, cx| {
                let Some(sheet) = Self::from_weak(&snap) else {
                    return;
                };
                if let Some(value) = sheet.value_of(Some(&id))
                    && let Some(on_snap) = sheet.inner.options.on_snap.clone()
                {
                    on_snap(this, value, reason, event, window, cx);
                }
            }),
            on_rest: Rc::new(move |this: &mut T, id: Option<SharedString>, window, cx| {
                let Some(sheet) = Self::from_weak(&rest) else {
                    return;
                };
                if sheet.inner.data.borrow().destroyed {
                    return;
                }
                let extent = {
                    let data = sheet.inner.data.borrow();
                    id.as_ref()
                        .and_then(|id| data.resolved.iter().find(|point| &point.id == id))
                        .map_or(closed_extent_for(data.side), |point| point.extent)
                };
                sheet.inner.effects.apply(extent, true, cx);
                if let Some(on_rest) = sheet.inner.options.on_rest.clone() {
                    on_rest(this, sheet.value_of(id.as_ref()), window, cx);
                }
            }),
            on_dismiss: Rc::new(move |this: &mut T, event, window, cx| dismiss(this, event, window, cx)),
            on_dismiss_attempt: Rc::new(move |this: &mut T, event, window, cx| {
                if let Some(attempt) = &attempt {
                    attempt(this, event, window, cx);
                }
            }),
            on_drag_change: Rc::new(move |this: &mut T, dragging, window, cx| {
                if let Some(drag_change) = &drag_change {
                    drag_change(this, dragging, window, cx);
                }
            }),
            is_interactive: Rc::new(move |id: &SharedString| {
                Self::from_weak(&interactive).is_some_and(|sheet| {
                    let value = sheet.value_of(Some(id));
                    sheet.is_interactive_at(value)
                })
            }),
            on_frame: Rc::new(move |extent: f64, cx: &mut App| {
                if let Some(sheet) = Self::from_weak(&frame)
                    && !sheet.inner.data.borrow().destroyed
                {
                    sheet.inner.effects.apply(extent, false, cx);
                }
            }),
            on_motion: Rc::new(|_| {}),
        }
    }

    // Measures the geometry: the side, the host's and the sheet's sizes, and
    // the snap points, and reports a change to the component. Returns
    // whether it changed.
    fn measure(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) -> bool {
        let side = resolve_side(
            self.inner.options.side.as_ref().map_or(SheetSide::Bottom, |side| side()),
            is_rtl(cx),
        );
        let host = self.inner.options.host.bounds();
        let el = self.inner.el.bounds();
        let container = host.map_or(0.0, |bounds| size_along(bounds, side));
        let size = el.map_or(0.0, |bounds| size_along(bounds, side));
        let root_font_size = f64::from(f32::from(window.rem_size()));
        let mut next = resolve_snap_points(
            &self.snap_points_option(),
            SnapPointContext {
                container,
                content: size,
                root_font_size,
            },
        );
        if next.is_empty() {
            next.push(ResolvedSnapPoint {
                id: "content".into(),
                value: SnapPoint::Content,
                extent: size,
                ids: vec!["content".into()],
            });
        }
        let modal_above = self.modal_above();
        let (changed, layout) = {
            let mut data = self.inner.data.borrow_mut();
            data.measured = true;
            data.side = side;
            data.size = size;
            data.host_size = container;
            data.viewport_height.get_or_insert(window.viewport_size().height);
            let changed = container != data.config.dimension
                || next.len() != data.resolved.len()
                || next.iter().zip(&data.resolved).any(|(point, before)| {
                    point.id != before.id || (point.extent - before.extent).abs() >= 0.5
                });
            data.resolved = next;
            data.config.dimension = container;
            data.config.snap_points = data.resolved.iter().map(SheetSnapPoint::from).collect();
            let extents = data.config.snap_points.iter().map(|point| point.extent).collect();
            let undimmed = find_snap_index(&data.resolved, modal_above.as_ref());
            self.inner.effects.configure(extents, undimmed);
            (
                changed,
                SheetLayout {
                    side,
                    snap_points: data.resolved.clone(),
                },
            )
        };
        if changed && let Some(on_layout) = self.inner.options.on_layout.clone() {
            on_layout(this, layout, window, cx);
        }
        changed
    }

    // The keyboard changed the room above it. The sheet takes its new place
    // at once, so it never passes through the old one, and slides there from
    // where it was drawn with the keyboard's own curve.
    fn follow_keyboard(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        if self.inner.data.borrow().destroyed {
            return;
        }
        self.relayout(this, true, window, cx);
        let height = window.viewport_size().height;
        let (shift, changed) = {
            let mut data = self.inner.data.borrow_mut();
            let changed = data.viewport_height != Some(height);
            data.viewport_height = Some(height);
            let top = self
                .inner
                .el
                .bounds()
                .map_or(data.keyboard_from, |bounds| f64::from(f32::from(bounds.top())));
            let inset = f64::from(f32::from(self.inner.keyboard.get().inset));
            (data.keyboard_from - top + inset.min(0.0), changed)
        };
        if !changed {
            return;
        }
        let shift = if shift.abs() < 1.0 {
            f64::from(f32::from(self.inner.keyboard.get().inset))
        } else {
            shift
        };
        if shift.abs() < 1.0 || cx.reduce_motion() {
            self.inner.data.borrow_mut().keyboard_slide = None;
            return;
        }
        self.inner.data.borrow_mut().keyboard_slide = Some((Instant::now(), shift));
        self.animate_slide(window);
    }

    fn animate_slide(&self, window: &Window) {
        let me = self.inner.me.clone();
        window.on_next_frame(move |window, cx| {
            let Some(sheet) = Self::from_weak(&me) else {
                return;
            };
            let running = sheet
                .inner
                .data
                .borrow()
                .keyboard_slide
                .is_some_and(|(start, _)| start.elapsed().as_secs_f64() * 1000.0 < KEYBOARD_DURATION);
            if !running {
                sheet.inner.data.borrow_mut().keyboard_slide = None;
            } else {
                sheet.animate_slide(window);
            }
            if let Some(entity) = sheet.inner.entity.upgrade() {
                cx.notify(entity.entity_id());
            }
        });
    }

    // The host's offset while it slides with the keyboard.
    fn keyboard_offset(&self) -> f64 {
        let data = self.inner.data.borrow();
        let Some((start, shift)) = data.keyboard_slide else {
            return 0.0;
        };
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        if elapsed >= KEYBOARD_DURATION {
            return 0.0;
        }
        let progress = cubic_bezier(0.38, 0.7, 0.125, 1.0, elapsed / KEYBOARD_DURATION);
        shift * (1.0 - progress)
    }

    // A snap point the owner moved to as the resting one left, like a drawer
    // whose new snap points drop it, wins over the nearest.
    fn relayout(&self, this: &mut T, immediate: bool, window: &mut Window, cx: &mut Context<T>) {
        let (first, destroyed) = {
            let data = self.inner.data.borrow();
            (!data.measured, data.destroyed)
        };
        if destroyed {
            return;
        }
        if self.measure(this, window, cx) && !first {
            let fallback = {
                let point = self.snap_point_option();
                let data = self.inner.data.borrow();
                find_snap_index(&data.resolved, point.as_ref())
                    .and_then(|index| data.resolved.get(index))
                    .map(|point| point.id.clone())
            };
            match self.driver() {
                SheetDriverKind::Pointer(driver) => driver.relayout(this, immediate, fallback, window, cx),
                SheetDriverKind::ScrollSnap(driver) => driver.relayout(this, immediate, fallback, window, cx),
            }
        }
    }

    fn id_of(&self, point: Option<&SnapPoint>) -> Option<SharedString> {
        let data = self.inner.data.borrow();
        let index = find_snap_index(&data.resolved, point).unwrap_or(0);
        data.resolved.get(index).map(|point| point.id.clone())
    }

    /// The sheet's element, its region.
    pub fn element(&self) -> Region {
        self.inner.el.clone()
    }

    /// The sheet as sheets stacked on it see it.
    pub fn stack(&self) -> SheetStack {
        self.inner.stack.clone()
    }

    /// The sheet this one opened from.
    pub fn parent(&self) -> Option<SheetStack> {
        self.inner.options.parent.as_ref().and_then(|parent| parent())
    }

    /// The scroll areas inside the sheet, which the hand-off reads.
    pub fn set_scroll_areas(&self, areas: Vec<ScrollArea>) {
        *self.inner.scroll_areas.borrow_mut() = areas;
    }

    /// The regions a mouse press doesn't drag the sheet from, like its
    /// buttons and fields.
    pub fn set_no_drag(&self, regions: Vec<Region>) {
        *self.inner.no_drag.borrow_mut() = regions;
    }

    /// When the settle in progress started and how long it runs, in ms on
    /// the input clock.
    pub fn motion_window(&self) -> Option<(f64, f64)> {
        match self.driver() {
            SheetDriverKind::Pointer(driver) => driver.motion_window(),
            SheetDriverKind::ScrollSnap(driver) => driver.motion_window(),
        }
    }

    /// Whether it has opened, so it shows.
    pub fn is_opened(&self) -> bool {
        self.inner.data.borrow().opened
    }

    /// Shows the sheet at its snap point, rising from closed unless `animate`
    /// is false. It measures itself as it opens, so a sheet whose host and
    /// body haven't rendered yet opens once the frame that renders them is
    /// drawn, out of sight until then.
    pub fn open(&self, this: &mut T, animate: bool, window: &mut Window, cx: &mut Context<T>) {
        if self.inner.data.borrow().destroyed {
            return;
        }
        if self.inner.el.bounds().is_none() || self.inner.options.host.bounds().is_none() {
            let me = self.inner.me.clone();
            crate::sheet_driver::after_first_frame(window, move |_, window, cx| {
                let Some(sheet) = Self::from_weak(&me) else {
                    return;
                };
                if sheet.inner.el.bounds().is_none() {
                    return;
                }
                let entity = sheet.inner.entity.clone();
                entity
                    .update(cx, |this, cx| sheet.open(this, animate, window, cx))
                    .ok();
            });
            return;
        }
        self.inner.data.borrow_mut().opened = true;
        let mut ancestors = Vec::new();
        let mut parent = self.parent();
        self.inner.stack.set_parent(parent.clone());
        while let Some(current) = parent {
            ancestors.push(current.clone());
            parent = current.parent();
        }
        *self.inner.ancestors.borrow_mut() = ancestors;
        self.measure(this, window, cx);
        let config = self.read_config();
        if let Some(theme) = self.inner.options.theme_color.as_ref().and_then(|theme| theme()) {
            let mut current = self.inner.theme.borrow_mut();
            if current.is_none() {
                *current = Some((dim_theme_color(theme.colors, config.physics.dim), theme.apply));
            }
        }
        let side = self.inner.data.borrow().side;
        if animate {
            self.inner.effects.apply(closed_extent_for(side), false, cx);
        }
        let Some(id) = self.id_of(self.snap_point_option().as_ref()) else {
            return;
        };
        match self.driver() {
            SheetDriverKind::Pointer(driver) => driver.open(this, id, animate, window, cx),
            SheetDriverKind::ScrollSnap(driver) => driver.open(this, id, animate, window, cx),
        }
    }

    /// Settles closed. Its popup's presence waits for the settle, which the
    /// sheet tracks on it.
    pub fn close(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        if self.inner.data.borrow().destroyed {
            return;
        }
        match self.driver() {
            SheetDriverKind::Pointer(driver) => driver.close(this, window, cx),
            SheetDriverKind::ScrollSnap(driver) => driver.close(this, window, cx),
        }
    }

    /// Moves to a snap point. `reason` comes back through `on_snap` if it's
    /// a change.
    pub fn snap_to(&self, this: &mut T, point: &SnapPoint, reason: SheetSnapReason, window: &mut Window, cx: &mut Context<T>) {
        let id = {
            let data = self.inner.data.borrow();
            find_snap_index(&data.resolved, Some(point)).map(|index| data.resolved[index].id.clone())
        };
        let Some(id) = id else {
            return;
        };
        let target = match self.driver() {
            SheetDriverKind::Pointer(driver) => driver.target(),
            SheetDriverKind::ScrollSnap(driver) => driver.target(),
        };
        if target.as_ref() == Some(&id) {
            return;
        }
        match self.driver() {
            SheetDriverKind::Pointer(driver) => driver.snap_to(this, id, reason, window, cx),
            SheetDriverKind::ScrollSnap(driver) => driver.snap_to(this, id, reason, window, cx),
        }
    }

    /// A refused dismissal: a small push toward closing that springs back.
    pub fn nudge(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        match self.driver() {
            SheetDriverKind::Pointer(driver) => driver.nudge(this, window, cx),
            SheetDriverKind::ScrollSnap(driver) => driver.nudge(this, window, cx),
        }
    }

    /// The resolved snap points, ascending.
    pub fn snap_points(&self) -> Vec<ResolvedSnapPoint> {
        self.inner.data.borrow().resolved.clone()
    }

    /// The snap point it rests on or is settling to.
    pub fn target(&self) -> Option<SnapPoint> {
        let target = match self.driver() {
            SheetDriverKind::Pointer(driver) => driver.target(),
            SheetDriverKind::ScrollSnap(driver) => driver.target(),
        };
        self.value_of(target.as_ref())
    }

    /// Whether resting at `point` leaves the window usable, because it's at
    /// or below `modal_above`.
    pub fn is_interactive_at(&self, point: Option<SnapPoint>) -> bool {
        let modal_above = self.modal_above();
        let data = self.inner.data.borrow();
        is_at_or_below(&data.resolved, point.as_ref(), modal_above.as_ref())
    }

    /// Measures again, like after an option that changes the geometry.
    pub fn refresh(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        self.relayout(this, false, window, cx);
    }

    /// Lets the sheet go: no more frames or callbacks, and what follows it
    /// goes back as it was.
    pub fn destroy(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        {
            let mut data = self.inner.data.borrow_mut();
            if data.destroyed {
                return;
            }
            data.destroyed = true;
        }
        match self.driver() {
            SheetDriverKind::Pointer(driver) => driver.destroy(this, window, cx),
            SheetDriverKind::ScrollSnap(driver) => driver.destroy(this, window, cx),
        }
        self.inner.effects.destroy(cx);
        let theme = self.inner.theme.borrow_mut().take();
        if let Some((theme, apply)) = theme {
            apply(theme.restore(), cx);
        }
        let id = self.inner.id;
        SHEETS.with(|sheets| sheets.borrow_mut().retain(|(own, _)| *own != id));
        EDGE_SWIPE.with(|hosts| hosts.borrow_mut().retain(|(own, _)| *own != id));
    }

    /// Whether it's been let go.
    pub fn is_destroyed(&self) -> bool {
        self.inner.data.borrow().destroyed
    }

    /// Whether a gesture or a settle is in progress.
    pub fn is_moving(&self) -> bool {
        match self.driver() {
            SheetDriverKind::Pointer(driver) => driver.is_moving(),
            SheetDriverKind::ScrollSnap(driver) => driver.is_moving(),
        }
    }

    /// Which driver moves it.
    pub fn driver_name(&self) -> SheetDriverName {
        match self.driver() {
            SheetDriverKind::Pointer(_) => SheetDriverName::Pointer,
            SheetDriverKind::ScrollSnap(_) => SheetDriverName::ScrollSnap,
        }
    }

    /// The extent it shows now, mid-settle too.
    pub fn extent(&self) -> f64 {
        match self.driver() {
            SheetDriverKind::Pointer(driver) => driver.rendered(),
            SheetDriverKind::ScrollSnap(driver) => driver.rendered(),
        }
    }

    /// The dim, progress and expand values, as of its last frame.
    pub fn values(&self) -> SheetValues {
        self.inner.effects.values()
    }

    /// What its parts style from.
    pub fn part_state(&self) -> SheetPartState {
        let (opacity, dragging, scroll_locked) = match self.driver() {
            SheetDriverKind::Pointer(driver) => (driver.opacity(), driver.is_dragging(), false),
            SheetDriverKind::ScrollSnap(driver) => (1.0, driver.is_dragging(), driver.scroll_areas_locked()),
        };
        SheetPartState {
            values: self.values(),
            opacity,
            dragging,
            side: self.inner.data.borrow().side,
            scroll_locked,
        }
    }

    /// The host the sheet sits in, as it renders: measured, following the
    /// keyboard, laid out toward the sheet's edge, and with the scroll-snap
    /// driver, the scroller the sheet moves in, which hears the pointer only
    /// over the sheet while it rests where the window stays usable. Render it
    /// in a box the size of the window, above the window, like
    /// `deferred(window_box(sheet.host(part)))`.
    pub fn host<E>(&self, el: E) -> SheetHost<T>
    where
        E: IntoElement + ParentElement + Styled + InteractiveElement + StatefulInteractiveElement + 'static,
    {
        let side = self.inner.data.borrow().side;
        let vertical = is_vertical(side);
        let inset = self.inner.keyboard.get().inset;
        let at_end = matches!(side, PhysicalSide::Bottom | PhysicalSide::Right);
        let mut el = el.size_full().flex();
        el = if vertical { el.flex_col() } else { el.flex_row() };
        el = if at_end { el.justify_end() } else { el.justify_start() };
        if inset > px(0.) {
            el = el.pb(inset);
        }
        if let SheetDriverKind::ScrollSnap(driver) = self.driver() {
            let handle = driver.scroll_handle().clone();
            let entity = self.inner.entity.clone();
            let touched = driver.clone();
            // The scroller takes the pointer from what's under it, as a
            // full-window element on top does.
            el = el
                .block()
                .overflow_y_scroll()
                .occlude()
                .track_scroll(&handle)
                .on_scroll(move |event, window, cx| {
                    let phase = event.touch_phase;
                    let driver = touched.clone();
                    entity
                        .update(cx, |this, cx| driver.touch(this, phase, window, cx))
                        .ok();
                })
                .child(gpui::div().w_full().h(px(driver.content_height() as f32)));
        }
        let prevents = self
            .inner
            .options
            .prevent_edge_swipe
            .as_ref()
            .is_some_and(|prevent| prevent());
        let id = self.inner.id;
        let host = self.inner.options.host.clone();
        EDGE_SWIPE.with(|hosts| {
            let mut hosts = hosts.borrow_mut();
            hosts.retain(|(own, _)| *own != id);
            if prevents {
                hosts.push((id, host));
            }
        });
        if prevents {
            el = el.on_back_gesture(|_, _, _| {});
        }
        let el = self.inner.keyboard.track(self.inner.options.host.measure(el));
        let listener = match self.driver() {
            SheetDriverKind::Pointer(driver) => Some(driver.listener().into_any_element()),
            SheetDriverKind::ScrollSnap(_) => None,
        };
        let el = match listener {
            Some(listener) => el.child(listener),
            None => el,
        };
        // Resting where the window stays usable, the scroller lets the pointer
        // through beside the sheet, so presses, hovers and scrolls there reach
        // the window, and the sheet's own still move it (`pointer-events:
        // none` on the host, `auto` on the sheet).
        let passes = match self.driver() {
            SheetDriverKind::ScrollSnap(driver) if !driver.host_blocks() => {
                Some(self.inner.el.bounds().unwrap_or_default())
            }
            _ => None,
        };
        SheetHost {
            sheet: self.clone(),
            child: el.into_any_element(),
            offset: self.keyboard_offset(),
            passes,
        }
    }

    /// The sheet itself, as it renders: measured, pressed to drag, moved
    /// along its axis by where it is, and taking presses and hovers from
    /// what's under it. Scrolls go through to its host with the scroll-snap
    /// driver, which moves it with the host's own scrolling; the pointer
    /// driver's sheet keeps them.
    pub fn body<E>(&self, el: E) -> SheetBody<T>
    where
        E: IntoElement + ParentElement + Styled + InteractiveElement + 'static,
    {
        let el = match self.driver() {
            SheetDriverKind::ScrollSnap(_) => el.block_mouse_except_scroll(),
            SheetDriverKind::Pointer(_) => el.occlude(),
        };
        let entity = self.inner.entity.clone();
        let sheet = self.clone();
        let el = on_pointer_down(self.inner.el.measure(el), move |event, window, cx| {
            let sheet = sheet.clone();
            entity
                .update(cx, |this, cx| {
                    if let SheetDriverKind::Pointer(driver) = sheet.driver() {
                        driver.press(this, event, window, cx);
                    }
                })
                .ok();
        });
        let side = self.inner.data.borrow().side;
        // Out of sight until it opens, once it has laid out.
        let el = if self.is_opened() { el } else { el.opacity(0.) };
        let el = match self.driver() {
            SheetDriverKind::ScrollSnap(driver) => {
                let top = driver.sheet_top() as f32;
                el.absolute().left_0().right_0().top(px(top))
            }
            SheetDriverKind::Pointer(_) => el,
        };
        let offset = match self.driver() {
            SheetDriverKind::Pointer(driver) => {
                let size = self.inner.data.borrow().size;
                translate_value(side, size - driver.rendered())
            }
            SheetDriverKind::ScrollSnap(driver) => point(px(0.), px(driver.flip_offset() as f32)),
        };
        SheetBody {
            child: el.into_any_element(),
            offset,
            _owner: std::marker::PhantomData,
        }
    }

    // After a frame laid the sheet out: a new size moves it to its snap
    // point's new extent, and the scroll-snap driver hears its scroll.
    fn laid_out(&self, window: &mut Window, cx: &mut App) {
        let (size, host) = {
            let data = self.inner.data.borrow();
            if data.destroyed || !data.measured {
                return;
            }
            (data.size, data.host_size)
        };
        let side = self.inner.data.borrow().side;
        let now_size = self.inner.el.bounds().map_or(size, |bounds| size_along(bounds, side));
        let now_host = self
            .inner
            .options
            .host
            .bounds()
            .map_or(host, |bounds| size_along(bounds, side));
        let resized = (now_size - size).abs() >= 0.5 || (now_host - host).abs() >= 0.5;
        let snap = matches!(self.driver(), SheetDriverKind::ScrollSnap(_));
        if !resized && !snap {
            return;
        }
        let me = self.inner.me.clone();
        window.defer(cx, move |window, cx| {
            let Some(sheet) = Self::from_weak(&me) else {
                return;
            };
            let entity = sheet.inner.entity.clone();
            entity
                .update(cx, |this, cx| {
                    if resized {
                        sheet.relayout(this, false, window, cx);
                    }
                    if let SheetDriverKind::ScrollSnap(driver) = sheet.driver() {
                        driver.scrolled(this, window, cx);
                    }
                })
                .ok();
        });
    }
}

/// What [`Sheet::host`] returns: the host, offset while it slides with the
/// keyboard.
pub struct SheetHost<T: 'static> {
    sheet: Sheet<T>,
    child: AnyElement,
    offset: f64,
    // The sheet's bounds, outside which the host and what's in it don't hear
    // the pointer, while it lets the pointer through.
    passes: Option<Bounds<Pixels>>,
}

/// What [`Sheet::body`] returns: the sheet, moved along its axis.
pub struct SheetBody<T: 'static> {
    child: AnyElement,
    offset: Point<Pixels>,
    _owner: std::marker::PhantomData<T>,
}

impl<T: 'static> Element for SheetHost<T> {
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
        let offset = point(px(0.), px(self.offset as f32));
        // Hitboxes keep the content mask they're made in, which clips where
        // they hear the pointer and nothing they paint.
        let mask = self.passes.map(|bounds| ContentMask { bounds });
        window.with_content_mask(mask, |window| {
            window.with_element_offset(offset, |window| self.child.prepaint(window, cx))
        });
        self.sheet.laid_out(window, cx);
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

impl<T: 'static> Element for SheetBody<T> {
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
        window.with_element_offset(self.offset, |window| self.child.prepaint(window, cx));
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
