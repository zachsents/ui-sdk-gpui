//! Port of `foundation/platform/layers.ts`: the stack of open layers.
//!
//! Every open popup is a layer. Layers nest: a submenu's layer has its menu's
//! layer as parent. Outside presses, Escape and window blur go to the right
//! layer.
//!
//! The original keeps one stack for the page and listens on each document
//! with a layer open. Here each window has its stack, reachable from any
//! component through the window, and hears it this way:
//!
//! - Presses, through mouse listeners, which gpui takes only while a frame
//!   paints: each layer's element paints [`listener`] (or calls [`listen`]
//!   while painting) on every frame it's shown. The first of them to hear a
//!   press decides for the whole stack, in the capture phase, before any
//!   element's handlers.
//! - Escape, through `cx.observe_keystrokes`, which hears a key once the
//!   focused element and the keymap had their turn, as a document's keydown
//!   listener does. A key an action took counts as handled, as one whose
//!   default was prevented does.
//! - Window blur, through the window's activation observer.
//!
//! A press a modal layer consumes stops in the capture phase, so no element
//! hears it: focus stays and no click forms, as the original's prevented
//! press and swallowed click have it. gpui sends a tap's press and release
//! together once the finger lifts, so a tap is decided as the original
//! decides a touch at its click, and a consumed tap moves no focus either.
//!
//! A layer's state is plain state its owner sets as it changes, so the stack
//! never reads an entity that may be mid-update: [`Layer::set_active`] as the
//! popup starts to close, [`Layer::set_exiting`] while it animates out.
//! Callbacks get the window and the app, and may update any entity but the
//! one running them. `contains` and `over_backdrop` are asked from inside
//! other components' updates too, so they read only what they hold, like
//! regions and focus handles.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use gpui::{
    App, AppContext, ClickEvent, DispatchPhase, ElementId, Entity, FocusHandle, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point, SharedString, Styled,
    Subscription, TouchClickEvent, Window, WindowId,
};
use ui_sdk_kernel::{InputEvent, Key, is_composing};
use web_time::Instant;

use crate::layout::placed_canvas;
use crate::platform::{Region, Target, region_at};
use crate::style::after_paint;

/// The `deferred` priority popups paint at.
pub const LAYER_PRIORITY: usize = 1;

/// The `deferred` priority persistent layers paint at, above every popup.
/// The original shows persistent popovers again whenever another opens, which
/// moves them back to the top of the top layer; gpui paints deferred draws by
/// priority, so a persistent layer painted at this one stays above.
pub const PERSISTENT_LAYER_PRIORITY: usize = 2;

/// When an outside press dismisses a layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OutsidePress {
    /// At the press, `"pointerdown"`.
    #[default]
    PointerDown,
    /// Once the press has also been released outside, `"click"`, like native
    /// light dismiss. A text selection dragged out of a field then never
    /// dismisses. Touch always waits for the release.
    Click,
}

/// Which step of a press dismissed a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PressKind {
    /// The press itself.
    PointerDown,
    /// Its release, for a layer that waits for one.
    PointerUp,
    /// A tap, which touch screens send whole.
    Click,
}

impl PressKind {
    /// The original's event type, like `"pointerdown"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PressKind::PointerDown => "pointerdown",
            PressKind::PointerUp => "pointerup",
            PressKind::Click => "click",
        }
    }
}

/// A press outside a layer, as its `on_outside_press` hears it.
#[derive(Clone, Debug)]
pub struct OutsidePressEvent {
    pub kind: PressKind,
    /// Where the press landed.
    pub position: Point<Pixels>,
    /// The press, for change details: the mouse down, its release, or a
    /// tap's touch click.
    pub event: InputEvent,
}

/// Whether a target counts as inside a layer, like its popup and its trigger.
/// The event is the press being decided, when there is one: the mouse down,
/// its release, or a tap's touch click.
pub type LayerContains = Rc<dyn Fn(&Target, Option<&InputEvent>, &Window, &App) -> bool>;

/// Whether a point is on a layer's backdrop, outside its popup.
pub type OverBackdrop = Rc<dyn Fn(Point<Pixels>, &Window, &App) -> bool>;

/// Keeps a region usable while a layer makes the rest of the window inert.
/// Returns a release.
pub type KeepRegion = Rc<dyn Fn(&Region) -> Box<dyn FnOnce()>>;

/// Where focus can go when a popup opened from a layer closes: the first
/// candidate `take` accepts, like one that took focus when tried. With
/// `strict`, what took the place of a trigger that left isn't one.
pub type ReturnTarget = Rc<
    dyn Fn(
        Option<&mut dyn FnMut(&FocusHandle, &mut Window, &mut App) -> bool>,
        bool,
        &mut Window,
        &mut App,
    ) -> Option<FocusHandle>,
>;

/// Hears Escape when a layer is the topmost.
pub type EscapeCallback = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>;

/// Hears a press outside a layer that dismisses it.
pub type OutsidePressCallback = Rc<dyn Fn(&OutsidePressEvent, &mut Window, &mut App)>;

/// Hears the window losing focus, like a click into another window.
pub type BlurCallback = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a layer is made with: the original's layer object.
#[derive(Clone, Default)]
pub struct LayerOptions {
    /// What counts as inside, such as the popup and its trigger, with the
    /// press being decided, when there is one. Without it, nothing does.
    pub contains: Option<LayerContains>,
    /// The layer this one nests under, like a submenu's menu.
    pub parent: Option<Layer>,
    /// Whether a point is on the layer's backdrop, outside its popup.
    pub over_backdrop: Option<OverBackdrop>,
    /// Whether an outside press only closes the layer, without reaching the
    /// window beneath.
    pub modal: bool,
    /// When an outside press dismisses.
    pub outside_press: OutsidePress,
    /// Stops an outside press here, so the layers beneath stay open, like a
    /// modal dialog whose backdrop was pressed.
    pub shield: bool,
    /// Keeps a region usable while this layer makes the rest of the window
    /// inert, like a child popup rendered elsewhere.
    pub keep: Option<KeepRegion>,
    /// Where focus can go when a popup opened from inside this one closes.
    pub return_target: Option<ReturnTarget>,
    /// Escape, when this is the topmost layer.
    pub on_escape: Option<EscapeCallback>,
    /// A press outside that dismisses it.
    pub on_outside_press: Option<OutsidePressCallback>,
    /// The window losing focus. Each layer decides what that means for it.
    pub on_blur: Option<BlurCallback>,
    /// Pauses keyboard shortcuts registered outside it, like a modal dialog
    /// does for the window's. Shortcuts scoped to it, or to layers above it,
    /// still run.
    pub blocking: bool,
    /// The component it belongs to, for the devtools, like `"menu"`.
    pub component: Option<SharedString>,
    /// The instance it belongs to, for the devtools.
    pub id: Option<SharedString>,
}

impl LayerOptions {
    pub fn contains(
        mut self,
        contains: impl Fn(&Target, Option<&InputEvent>, &Window, &App) -> bool + 'static,
    ) -> Self {
        self.contains = Some(Rc::new(contains));
        self
    }

    pub fn parent(mut self, parent: Option<Layer>) -> Self {
        self.parent = parent;
        self
    }

    pub fn over_backdrop(
        mut self,
        over_backdrop: impl Fn(Point<Pixels>, &Window, &App) -> bool + 'static,
    ) -> Self {
        self.over_backdrop = Some(Rc::new(over_backdrop));
        self
    }

    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    pub fn outside_press(mut self, outside_press: OutsidePress) -> Self {
        self.outside_press = outside_press;
        self
    }

    pub fn shield(mut self, shield: bool) -> Self {
        self.shield = shield;
        self
    }

    pub fn keep(mut self, keep: impl Fn(&Region) -> Box<dyn FnOnce()> + 'static) -> Self {
        self.keep = Some(Rc::new(keep));
        self
    }

    pub fn return_target(mut self, return_target: ReturnTarget) -> Self {
        self.return_target = Some(return_target);
        self
    }

    pub fn on_escape(
        mut self,
        on_escape: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_escape = Some(Rc::new(on_escape));
        self
    }

    pub fn on_outside_press(
        mut self,
        on_outside_press: impl Fn(&OutsidePressEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_outside_press = Some(Rc::new(on_outside_press));
        self
    }

    pub fn on_blur(mut self, on_blur: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_blur = Some(Rc::new(on_blur));
        self
    }

    pub fn blocking(mut self, blocking: bool) -> Self {
        self.blocking = blocking;
        self
    }

    pub fn component(mut self, component: impl Into<SharedString>) -> Self {
        self.component = Some(component.into());
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }
}

struct LayerState {
    contains: Option<LayerContains>,
    parent: Option<Layer>,
    active: Cell<bool>,
    exiting: Cell<bool>,
    over_backdrop: Option<OverBackdrop>,
    modal: Cell<bool>,
    outside_press: OutsidePress,
    shield: bool,
    keep: RefCell<Option<KeepRegion>>,
    return_target: RefCell<Option<ReturnTarget>>,
    on_escape: Option<EscapeCallback>,
    on_outside_press: Option<OutsidePressCallback>,
    on_blur: Option<BlurCallback>,
    blocking: Cell<bool>,
    order: Cell<u64>,
    component: Option<SharedString>,
    id: Option<SharedString>,
    window: Cell<Option<WindowId>>,
    // Where its element is in its window, as the listener it paints last
    // said.
    element: RefCell<Option<Rc<[ElementId]>>>,
}

/// An open popup in its window's stack. Clones are the same layer.
#[derive(Clone)]
pub struct Layer(Rc<LayerState>);

impl Layer {
    /// A layer, active until [`Layer::set_active`] says it started to close.
    pub fn new(options: LayerOptions) -> Self {
        Self(Rc::new(LayerState {
            contains: options.contains,
            parent: options.parent,
            active: Cell::new(true),
            exiting: Cell::new(false),
            over_backdrop: options.over_backdrop,
            modal: Cell::new(options.modal),
            outside_press: options.outside_press,
            shield: options.shield,
            keep: RefCell::new(options.keep),
            return_target: RefCell::new(options.return_target),
            on_escape: options.on_escape,
            on_outside_press: options.on_outside_press,
            on_blur: options.on_blur,
            blocking: Cell::new(options.blocking),
            order: Cell::new(0),
            component: options.component,
            id: options.id,
            window: Cell::new(None),
            element: RefCell::new(None),
        }))
    }

    /// Whether `target` counts as inside the layer itself, nested layers aside
    /// (see [`layer_contains`]). `event` is the press being decided, when
    /// there is one.
    pub fn contains(&self, target: &Target, event: Option<&InputEvent>, window: &Window, cx: &App) -> bool {
        self.0
            .contains
            .as_ref()
            .is_some_and(|contains| contains(target, event, window, cx))
    }

    pub fn parent(&self) -> Option<Layer> {
        self.0.parent.clone()
    }

    /// False once the layer starts to close. Closed layers ignore dismissal.
    pub fn is_active(&self) -> bool {
        self.0.active.get()
    }

    pub fn set_active(&self, active: bool) {
        self.0.active.set(active);
    }

    /// True while the layer animates out. It's inert then, so presses fall
    /// through it to the window. A modal one swallows those on its backdrop,
    /// so a double click that dismissed it can't land on the window, and
    /// shields the layers beneath it.
    pub fn is_exiting(&self) -> bool {
        self.0.exiting.get()
    }

    pub fn set_exiting(&self, exiting: bool) {
        self.0.exiting.set(exiting);
    }

    /// Whether a point is on the layer's backdrop, outside its popup.
    pub fn over_backdrop(&self, point: Point<Pixels>, window: &Window, cx: &App) -> bool {
        self.0
            .over_backdrop
            .as_ref()
            .is_some_and(|over| over(point, window, cx))
    }

    /// Whether an outside press only closes the layer.
    pub fn modal(&self) -> bool {
        self.0.modal.get()
    }

    pub fn set_modal(&self, modal: bool) {
        self.0.modal.set(modal);
    }

    pub fn outside_press(&self) -> OutsidePress {
        self.0.outside_press
    }

    pub fn shield(&self) -> bool {
        self.0.shield
    }

    pub fn keep(&self) -> Option<KeepRegion> {
        self.0.keep.borrow().clone()
    }

    /// Sets how the layer keeps regions usable, like once it makes the window
    /// inert.
    pub fn set_keep(&self, keep: Option<KeepRegion>) {
        *self.0.keep.borrow_mut() = keep;
    }

    pub fn return_target(&self) -> Option<ReturnTarget> {
        self.0.return_target.borrow().clone()
    }

    pub fn set_return_target(&self, return_target: Option<ReturnTarget>) {
        *self.0.return_target.borrow_mut() = return_target;
    }

    pub fn on_escape(&self) -> Option<EscapeCallback> {
        self.0.on_escape.clone()
    }

    pub fn on_outside_press(&self) -> Option<OutsidePressCallback> {
        self.0.on_outside_press.clone()
    }

    pub fn on_blur(&self) -> Option<BlurCallback> {
        self.0.on_blur.clone()
    }

    /// Whether it pauses keyboard shortcuts registered outside it.
    pub fn blocking(&self) -> bool {
        self.0.blocking.get()
    }

    pub fn set_blocking(&self, blocking: bool) {
        self.0.blocking.set(blocking);
    }

    /// When the layer opened, relative to the others. Set by [`push_layer`];
    /// 0 before.
    pub fn order(&self) -> u64 {
        self.0.order.get()
    }

    pub fn component(&self) -> Option<SharedString> {
        self.0.component.clone()
    }

    pub fn id(&self) -> Option<SharedString> {
        self.0.id.clone()
    }

    /// The window whose stack it's in, once pushed.
    pub fn window(&self) -> Option<WindowId> {
        self.0.window.get()
    }
}

impl PartialEq for Layer {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Layer {}

impl fmt::Debug for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Layer")
            .field("component", &self.0.component)
            .field("id", &self.0.id)
            .field("order", &self.order())
            .field("active", &self.is_active())
            .field("exiting", &self.is_exiting())
            .field("modal", &self.modal())
            .field("blocking", &self.blocking())
            .finish()
    }
}

struct PendingPress {
    layer: Layer,
    target: Target,
    point: Point<Pixels>,
}

// The window's observers while it has a layer open.
struct Listening {
    _keys: Subscription,
    _blur: Subscription,
    _watcher: Entity<BlurWatcher>,
}

struct BlurWatcher;

#[derive(Default)]
struct WindowLayers {
    layers: usize,
    listening: Option<Listening>,
    persistent: Vec<(u64, Region)>,
    escape_at: Option<Instant>,
    closed: Option<FocusHandle>,
    closed_mark: u64,
    pending: Option<PendingPress>,
    hearing_down: bool,
    hearing_up: bool,
}

#[derive(Default)]
struct Realm {
    stack: Vec<Layer>,
    pushed: u64,
    persistent_ids: u64,
    windows: HashMap<WindowId, WindowLayers>,
}

thread_local! {
    static REALM: RefCell<Realm> = RefCell::new(Realm::default());
}

// Never call a layer's callbacks inside: they may come back to the stack.
fn with_realm<R>(f: impl FnOnce(&mut Realm) -> R) -> R {
    REALM.with(|realm| f(&mut realm.borrow_mut()))
}

fn with_window<R>(window: WindowId, f: impl FnOnce(&mut WindowLayers) -> R) -> R {
    with_realm(|realm| f(realm.windows.entry(window).or_default()))
}

fn window_id(window: &Window) -> WindowId {
    window.window_handle().window_id()
}

fn stack_in(window: WindowId) -> Vec<Layer> {
    with_realm(|realm| {
        realm
            .stack
            .iter()
            .filter(|layer| layer.window() == Some(window))
            .cloned()
            .collect()
    })
}

fn active_in(window: WindowId) -> Vec<Layer> {
    stack_in(window)
        .into_iter()
        .filter(Layer::is_active)
        .collect()
}

/// A region kept live over every layer until released or dropped.
#[must_use = "dropping it releases the region"]
pub struct PersistentLayer {
    id: u64,
    window: WindowId,
    releases: RefCell<Vec<Box<dyn FnOnce()>>>,
    released: Cell<bool>,
}

impl PersistentLayer {
    /// Undoes [`bind_persistent_layer`].
    pub fn release(&self) {
        if self.released.replace(true) {
            return;
        }
        let id = self.id;
        with_window(self.window, |state| {
            state.persistent.retain(|(persistent, _)| *persistent != id)
        });
        for release in self.releases.take() {
            release();
        }
    }
}

impl Drop for PersistentLayer {
    fn drop(&mut self) {
        self.release();
    }
}

/// Keeps a region live over every layer, like a toast region over a modal
/// dialog: modal layers don't make it inert, whether they opened before or
/// after, and presses in it never dismiss a layer. Paint it at
/// [`PERSISTENT_LAYER_PRIORITY`] so it stays above popups that open later.
/// Releasing the returned guard, or dropping it, undoes it.
pub fn bind_persistent_layer(region: Region, window: &Window) -> PersistentLayer {
    let window = window_id(window);
    let id = with_realm(|realm| {
        realm.persistent_ids += 1;
        let id = realm.persistent_ids;
        realm
            .windows
            .entry(window)
            .or_default()
            .persistent
            .push((id, region.clone()));
        id
    });
    let releases = active_in(window)
        .iter()
        .filter_map(|layer| layer.keep().map(|keep| keep(&region)))
        .collect();
    PersistentLayer {
        id,
        window,
        releases: RefCell::new(releases),
        released: Cell::new(false),
    }
}

/// Whether a target is inside a persistent layer, like a toast region.
pub fn in_persistent_layer(target: &Target, window: &Window) -> bool {
    persistent_layers(window)
        .iter()
        .any(|region| region.contains(target, window))
}

/// The window's persistent layers, for modal side effects.
pub fn persistent_layers(window: &Window) -> Vec<Region> {
    with_window(window_id(window), |state| {
        state
            .persistent
            .iter()
            .map(|(_, region)| region.clone())
            .collect()
    })
}

/// How many layers have opened so far. A layer opened after a moment has a
/// higher order.
pub fn layer_count() -> u64 {
    with_realm(|realm| realm.pushed)
}

/// Whether a layer open in the window opened after `order`.
pub fn opened_since(order: u64, window: &Window) -> bool {
    active_in(window_id(window))
        .iter()
        .any(|layer| layer.order() > order)
}

/// The window's open layers, oldest first.
pub fn active_layers(window: &Window) -> Vec<Layer> {
    active_in(window_id(window))
}

/// When Escape last reached the window's layers, so a close request the
/// platform makes for the same key press, like a back gesture's, can tell it
/// was already handled.
pub fn last_escape(window: &Window) -> Option<Instant> {
    with_window(window_id(window), |state| state.escape_at)
}

// A target inside a layer nested under this one, like a popup opened from
// inside another popup and rendered elsewhere, is inside this one too. Each
// layer hears the press being decided, when there is one.
fn inside_tree(
    layer: &Layer,
    target: &Target,
    event: Option<&InputEvent>,
    active: &[Layer],
    window: &Window,
    cx: &App,
) -> bool {
    if layer.contains(target, event, window, cx) {
        return true;
    }
    active.iter().any(|other| {
        other.parent().as_ref() == Some(layer) && inside_tree(other, target, event, active, window, cx)
    })
}

/// Whether a target is inside a layer or any open layer nested under it,
/// such as a popup opened from inside another popup and rendered elsewhere.
pub fn layer_contains(layer: &Layer, target: &Target, window: &Window, cx: &App) -> bool {
    let active = active_in(layer.window().unwrap_or_else(|| window_id(window)));
    inside_tree(layer, target, None, &active, window, cx)
}

/// The topmost open layer that contains a target. A popup whose trigger sits
/// inside another popup nests under it, so pressing inside the child never
/// dismisses the parent.
pub fn find_layer(target: &Target, window: &Window, cx: &App) -> Option<Layer> {
    active_in(window_id(window))
        .into_iter()
        .rev()
        .find(|layer| layer.contains(target, None, window, cx))
}

/// Where focus would have gone from the popup that closed last, until the
/// frame that shows the close has painted. A popup that opens as another
/// closes, like a dialog whose `open` state a menu item set, returns focus
/// there once the item is gone. Only the focus handle is kept, so the closed
/// popup's content can go.
///
/// The original keeps it until the task ends, which a framework renders
/// within. gpui renders the next frame later, so it's kept through it.
pub fn mark_closed(target: Option<FocusHandle>, window: &Window) {
    let id = window_id(window);
    let mark = with_window(id, |state| {
        state.closed_mark += 1;
        state.closed = target;
        state.closed_mark
    });
    after_paint(window, move |_, _| {
        with_window(id, |state| {
            if state.closed_mark == mark {
                state.closed = None;
            }
        })
    });
}

pub fn last_closed(window: &Window) -> Option<FocusHandle> {
    with_window(window_id(window), |state| state.closed.clone())
}

fn report(window: WindowId) {
    if !cfg!(debug_assertions) || !log::log_enabled!(log::Level::Debug) {
        return;
    }
    let stack = stack_in(window)
        .iter()
        .map(|layer| {
            format!(
                "{}#{}{}{}",
                layer.component().as_deref().unwrap_or("popup"),
                layer.id().as_deref().unwrap_or(""),
                if layer.modal() { " modal" } else { "" },
                if layer.blocking() { " blocking" } else { "" },
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    log::debug!("[ui-sdk] layers: [{stack}]");
}

// Escape goes to the topmost layer of the window it was pressed in: one open
// in another window didn't hear the key. With `from`, only when that layer is
// the topmost, as its own content hears the key first.
fn escape_top(
    window_id: WindowId,
    event: &KeyDownEvent,
    from: Option<&Layer>,
    window: &mut Window,
    cx: &mut App,
) {
    // Escape ends an input method's composition first, like a Japanese candidate list.
    if Key::of(&event.keystroke) != Key::Escape
        || window.default_prevented()
        || is_composing(window, cx)
    {
        return;
    }
    let Some(top) = active_in(window_id).pop() else {
        return;
    };
    let Some(on_escape) = top.on_escape() else {
        return;
    };
    if from.is_some_and(|from| *from != top) {
        return;
    }
    let now = cx.background_executor().now();
    with_window(window_id, |state| state.escape_at = Some(now));
    on_escape(event, window, cx);
}

/// Escape on a layer's own content, before it bubbles to ancestors that may
/// act on it too: closes the layer when it's the topmost. Call it from the
/// content's key down handler. The window still hears what never reached a
/// layer.
pub fn escape_layer(layer: &Layer, event: &KeyDownEvent, window: &mut Window, cx: &mut App) {
    let id = layer.window().unwrap_or_else(|| window_id(window));
    escape_top(id, event, Some(layer), window, cx);
}

fn blur(window_id: WindowId, window: &mut Window, cx: &mut App) {
    for layer in active_in(window_id).iter().rev() {
        if let Some(on_blur) = layer.on_blur() {
            on_blur(window, cx);
        }
    }
}

// The window listens for keys and blur while it has a layer open.
fn listen_in(window: &mut Window, cx: &mut App) -> Listening {
    let id = window_id(window);
    let keys = cx.observe_keystrokes(move |event, window, cx| {
        if window_id(window) != id || event.action.is_some() {
            return;
        }
        let down = KeyDownEvent {
            keystroke: event.keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        };
        escape_top(id, &down, None, window, cx);
    });
    // A click into another window or app never reaches this one.
    let watcher = cx.new(|_| BlurWatcher);
    let blurs = watcher.update(cx, |_, cx| {
        cx.observe_window_activation(window, move |_, window, cx| {
            if !window.is_window_active() {
                blur(id, window, cx);
            }
        })
    });
    Listening {
        _keys: keys,
        _blur: blurs,
        _watcher: watcher,
    }
}

/// A layer in its window's stack until popped or dropped.
#[must_use = "dropping it pops the layer"]
pub struct PushedLayer {
    layer: Layer,
    popped: Cell<bool>,
}

impl PushedLayer {
    pub fn layer(&self) -> &Layer {
        &self.layer
    }

    /// Takes the layer off the stack, `pushLayer`'s cleanup.
    pub fn pop(&self) {
        if self.popped.replace(true) {
            return;
        }
        let layer = self.layer.clone();
        let Some(window) = layer.window() else {
            return;
        };
        let listening = with_realm(|realm| {
            if let Some(index) = realm.stack.iter().position(|other| *other == layer) {
                realm.stack.remove(index);
            }
            let state = realm.windows.entry(window).or_default();
            state.layers = state.layers.saturating_sub(1);
            if state.layers > 0 {
                return None;
            }
            state.pending = None;
            state.listening.take()
        });
        drop(listening);
        report(window);
    }
}

impl Drop for PushedLayer {
    fn drop(&mut self) {
        self.pop();
    }
}

/// Puts a layer on top of the window's stack and gives it its order. The
/// returned guard pops it, as does dropping it.
///
/// The layer's element must paint [`listener`] on every frame it's shown,
/// since that's how the window hears presses.
pub fn push_layer(layer: Layer, window: &mut Window, cx: &mut App) -> PushedLayer {
    let id = window_id(window);
    let first = with_realm(|realm| {
        realm.pushed += 1;
        layer.0.order.set(realm.pushed);
        layer.0.window.set(Some(id));
        realm.stack.push(layer.clone());
        let state = realm.windows.entry(id).or_default();
        state.layers += 1;
        state.listening.is_none()
    });
    if first {
        let listening = listen_in(window, cx);
        with_window(id, |state| state.listening = Some(listening));
    }
    report(id);
    PushedLayer {
        layer,
        popped: Cell::new(false),
    }
}

/// Registers this frame's press listeners for the window's layers. Call it
/// while painting, on every frame a layer is shown; [`listener`] does.
pub fn listen(window: &mut Window) {
    window.on_mouse_event(|event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            pointer_down(event, window, cx);
        }
    });
    window.on_mouse_event(|event: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            pointer_up(event, window, cx);
        }
    });
}

/// An element that takes no space and calls [`listen`] each frame it paints.
/// A layer's element adds it as a child, beside its region's
/// [`measure`](Region::measure), which tells the layer where its element is
/// ([`in_exiting_layer`]).
pub fn listener() -> impl IntoElement {
    placed_canvas(
        "layer-listener",
        |_, _, _, _| {},
        |_, _, place, window, cx| {
            listen(window);
            own(place, window, cx);
        },
    )
    .absolute()
}

// Tells the layer whose element is at `place` that it's there: the newest
// layer in the window that holds the focus handle of the region measured
// there, as its `contains` says. Its trigger counts as inside it too, but
// paints no listener.
fn own(place: &[ElementId], window: &Window, cx: &App) {
    let id = window_id(window);
    let Some(focus) = region_at(id, place).and_then(|region| region.focus_handle()) else {
        return;
    };
    let target = Target::Focus(focus);
    let owner = stack_in(id)
        .into_iter()
        .rev()
        .find(|layer| layer.contains(&target, None, window, cx));
    if let Some(owner) = owner {
        let mut element = owner.0.element.borrow_mut();
        if element.as_deref() != Some(place) {
            *element = Some(Rc::from(place));
        }
    }
}

/// Whether the element at `place` in `window`, its path of element ids as
/// the kernel records parts' places, is inside the element of a layer that's
/// animating out ([`Layer::set_exiting`]), like the items of a menu that's
/// closing: the original's `[data-ending-style]`. A layer's element is the
/// one that paints its [`listener`] and measures the region its `contains`
/// holds by focus.
pub fn in_exiting_layer(window: WindowId, place: &[ElementId]) -> bool {
    stack_in(window).iter().any(|layer| {
        layer.is_exiting()
            && layer
                .0
                .element
                .borrow()
                .as_deref()
                .is_some_and(|element| place.starts_with(element))
    })
}

// Every layer's element registers the listeners, so the first to hear an
// event decides for the stack, and the others let it be until the event's
// effects have run.
fn hear(window: &mut Window, cx: &mut App, up: bool) -> Option<WindowId> {
    let id = window_id(window);
    let first = with_window(id, |state| {
        let hearing = if up {
            &mut state.hearing_up
        } else {
            &mut state.hearing_down
        };
        state.layers > 0 && !std::mem::replace(hearing, true)
    });
    if !first {
        return None;
    }
    window.defer(cx, move |_, _| {
        with_window(id, |state| {
            if up {
                state.hearing_up = false;
            } else {
                state.hearing_down = false;
            }
        })
    });
    Some(id)
}

// Which layers a press outside dismisses, from the top, and whether it only
// closes them, reaching nothing else. Each layer hears the press `event`.
fn plan(
    window_id: WindowId,
    target: &Target,
    event: &InputEvent,
    point: Point<Pixels>,
    window: &Window,
    cx: &App,
) -> (Vec<Layer>, bool) {
    let mut dismissed = Vec::new();
    let mut consume = false;
    if in_persistent_layer(target, window) {
        return (dismissed, consume);
    }
    let stack = stack_in(window_id);
    let active: Vec<Layer> = stack.iter().filter(|layer| layer.is_active()).cloned().collect();
    for layer in stack.iter().rev() {
        if layer.is_active() {
            if inside_tree(layer, target, Some(event), &active, window, cx) {
                break;
            }
            if layer.modal() {
                consume = true;
            }
            dismissed.push(layer.clone());
            if layer.shield() {
                break;
            }
        } else if layer.is_exiting() {
            if inside_tree(layer, target, Some(event), &active, window, cx) {
                break;
            }
            if !layer.modal() {
                continue;
            }
            if layer.over_backdrop(point, window, cx) {
                consume = true;
            }
            break;
        }
    }
    (dismissed, consume)
}

fn dismiss(
    window_id: WindowId,
    target: &Target,
    press: OutsidePressEvent,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let (dismissed, consume) = plan(window_id, target, &press.event, press.position, window, cx);
    for layer in dismissed {
        if let Some(on_outside_press) = layer.on_outside_press() {
            on_outside_press(&press, window, cx);
        }
    }
    consume
}

// Consuming stops the press in the capture phase: no element hears it, so
// focus doesn't move and no click forms, where the original prevents the
// press's default and swallows its click.
fn consume(window: &mut Window, cx: &mut App) {
    window.prevent_default();
    cx.stop_propagation();
}

fn pointer_down(event: &MouseDownEvent, window: &mut Window, cx: &mut App) {
    let Some(id) = hear(window, cx, false) else {
        return;
    };
    if window.has_active_prompt() {
        return;
    }
    with_window(id, |state| state.pending = None);
    let target = Target::Point(event.position);
    // gpui sends a tap's press and release together once the finger lifts,
    // so a tap decides where the original's touch waits for its click.
    if window.last_input_was_touch() {
        let tap = OutsidePressEvent {
            kind: PressKind::Click,
            position: event.position,
            event: InputEvent::Click(ClickEvent::Touch(TouchClickEvent {
                position: event.position,
                tap_count: event.click_count,
                long_press: false,
            })),
        };
        if dismiss(id, &target, tap, window, cx) {
            consume(window, cx);
        }
        return;
    }
    let down = InputEvent::MouseDown(event.clone());
    let (dismissed, consumed) = plan(id, &target, &down, event.position, window, cx);
    if let Some(first) = dismissed.first()
        && first.outside_press() == OutsidePress::Click
    {
        if event.button != MouseButton::Left {
            return;
        }
        let pending = PendingPress {
            layer: first.clone(),
            target,
            point: event.position,
        };
        with_window(id, |state| state.pending = Some(pending));
        if consumed {
            consume(window, cx);
        }
        return;
    }
    // Before the layers hear it, so the press stops even when closing the
    // last one ends this listening.
    if consumed {
        consume(window, cx);
    }
    let press = OutsidePressEvent {
        kind: PressKind::PointerDown,
        position: event.position,
        event: down,
    };
    for layer in dismissed {
        if let Some(on_outside_press) = layer.on_outside_press() {
            on_outside_press(&press, window, cx);
        }
    }
}

fn pointer_up(event: &MouseUpEvent, window: &mut Window, cx: &mut App) {
    let Some(id) = hear(window, cx, true) else {
        return;
    };
    let Some(press) = with_window(id, |state| state.pending.take()) else {
        return;
    };
    let up = InputEvent::MouseUp(event.clone());
    if !press.layer.is_active()
        || inside_tree(
            &press.layer,
            &Target::Point(event.position),
            Some(&up),
            &active_in(id),
            window,
            cx,
        )
    {
        return;
    }
    let release = OutsidePressEvent {
        kind: PressKind::PointerUp,
        position: press.point,
        event: up,
    };
    dismiss(id, &press.target, release, window, cx);
}
