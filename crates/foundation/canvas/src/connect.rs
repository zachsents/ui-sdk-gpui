//! Port of `foundation/canvas/connect.ts`: drawing a connection from one
//! port to another: wiring nodes in an editor, linking cards on a board,
//! joining steps of a flow. A pointer drags a loose line out of a port and
//! it snaps to the nearest port that can take it; let go there to connect.
//! The keyboard starts from a port, arrows go to the nearest port that can
//! take it in their direction, Enter connects and Escape lets go. The loose
//! line lives in the behavior, so it renders like any part.
//!
//! The ports are the parts of one slot, which the component keeps in a
//! kernel [`Parts`] and measures with `Parts::measure`: what the original
//! reads with `elements(context, slot)`. Port boxes are read when a
//! connection starts, so dragging measures nothing.

use std::cell::RefCell;
use std::fmt::Display;
use std::hash::Hash;
use std::rc::Rc;

use gpui::{
    Bounds, Context, IntoElement, KeyDownEvent, Pixels, Point, SharedString, Window, point, px,
};
use ui_sdk_drag_and_drop::{BindDragOptions, DragSession, PointerEvent};
use ui_sdk_focus::{SpatialRect, nearest_in_direction};
use ui_sdk_kernel::{ChangeOptions, InputEvent, Key, PartInfo, Parts, Reason};
use ui_sdk_platform::{AnnouncePriority, announce_with};

/// The connection being drawn: where it starts, the port it would land on,
/// and its loose end in your units (`ConnectDraft`).
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectDraft<K> {
    pub from: K,
    pub to: Option<K>,
    pub x: f32,
    pub y: f32,
    pub keyboard: bool,
}

/// Why a connection was made (`ConnectReason`): a pointer let go on a port,
/// or a key. The kernel's reasons have no `connect-key`, so a change
/// reports the nearest of its own ([`reason`](Self::reason)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnectReason {
    Drag,
    ConnectKey,
}

impl ConnectReason {
    /// The original's name for it, like `"connect-key"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ConnectReason::Drag => "drag",
            ConnectReason::ConnectKey => "connect-key",
        }
    }

    /// The kernel's reason for it.
    pub fn reason(self) -> Reason {
        match self {
            ConnectReason::Drag => Reason::Drag,
            ConnectReason::ConnectKey => Reason::ConnectKey,
        }
    }
}

/// Why and how a connection was made, what `connect` receives.
#[derive(Clone, Debug)]
pub struct ConnectDetails {
    pub reason: ConnectReason,
    pub event: Option<InputEvent>,
}

impl ConnectDetails {
    pub fn new(reason: ConnectReason) -> Self {
        Self {
            reason,
            event: None,
        }
    }

    /// The details as a kernel change reports them, for
    /// `Controllable::change`.
    pub fn change_options(&self) -> ChangeOptions {
        ChangeOptions {
            reason: Some(self.reason.reason()),
            source: None,
            event: self.event.clone(),
        }
    }
}

impl Default for ConnectDetails {
    fn default() -> Self {
        Self::new(ConnectReason::ConnectKey)
    }
}

/// What screen readers hear, by step (`ConnectMessage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectMessage<K> {
    Start { from: K },
    Target { from: K, to: K },
    Connect { from: K, to: K },
    Cancel { from: K },
}

/// The English messages [`create_connect`] announces unless you pass
/// `message` (`connectMessage`).
pub fn connect_message<K>(message: &ConnectMessage<K>, label: &dyn Fn(&K) -> SharedString) -> String {
    match message {
        ConnectMessage::Start { from } => format!(
            "Connecting from {}. Arrows choose a port, Enter connects, Escape cancels.",
            label(from)
        ),
        ConnectMessage::Target { to, .. } => format!("To {}.", label(to)),
        ConnectMessage::Connect { from, to } => {
            format!("Connected {} to {}.", label(from), label(to))
        }
        ConnectMessage::Cancel { .. } => "Canceled.".to_string(),
    }
}

/// A port's part in the connection being drawn (`role`), for its styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnectRole {
    Source,
    Target,
    Candidate,
}

impl ConnectRole {
    /// The original's name for it, like `"candidate"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ConnectRole::Source => "source",
            ConnectRole::Target => "target",
            ConnectRole::Candidate => "candidate",
        }
    }
}

/// Makes a connection and returns whether it went through.
pub type ConnectFn<T, K> =
    Rc<dyn Fn(&mut T, &K, &K, ConnectDetails, &mut Window, &mut Context<T>) -> bool>;

/// Whether `from` can connect to `to`.
pub type CanConnectFn<T, K> = Rc<dyn Fn(&T, &K, &K) -> bool>;

/// Turns a point on screen into your units.
pub type PointFn<T> = Rc<dyn Fn(&T, Point<Pixels>) -> Point<f32>>;

/// How near a pointer must come to a port to snap to it.
pub type RadiusFn<T> = Rc<dyn Fn(&T) -> Pixels>;

/// A port's name for screen readers.
pub type LabelFn<K> = Rc<dyn Fn(&K) -> SharedString>;

/// What screen readers hear, with the port names.
pub type ConnectMessageFn<K> = Rc<dyn Fn(&ConnectMessage<K>, &dyn Fn(&K) -> SharedString) -> String>;

/// How [`create_connect`] finds ports and makes connections
/// (`ConnectOptions`). `T` is the component, `K` a port's key, `P` what its
/// parts register with.
pub struct ConnectOptions<T: 'static, K, P = PartInfo> {
    /// The ports (`slot`), measured with `Parts::measure`.
    pub parts: Parts<K, P>,
    /// Makes the connection and returns whether it went through.
    pub connect: ConnectFn<T, K>,
    /// Whether `from` can connect to `to`. Defaults to any other port.
    pub can_connect: Option<CanConnectFn<T, K>>,
    /// Turns a point on screen into your units, like a canvas viewport's
    /// `to_canvas`. Defaults to window pixels.
    pub point: Option<PointFn<T>>,
    /// How near, in screen pixels, a pointer must come to a port to snap to
    /// it. Defaults to 28.
    pub radius: Option<RadiusFn<T>>,
    /// A port's name for screen readers. Defaults to its key.
    pub label: Option<LabelFn<K>>,
    /// What screen readers hear. Defaults to English
    /// ([`connect_message`]).
    pub message: Option<ConnectMessageFn<K>>,
}

impl<T: 'static, K, P> Clone for ConnectOptions<T, K, P> {
    fn clone(&self) -> Self {
        Self {
            parts: self.parts.clone(),
            connect: self.connect.clone(),
            can_connect: self.can_connect.clone(),
            point: self.point.clone(),
            radius: self.radius.clone(),
            label: self.label.clone(),
            message: self.message.clone(),
        }
    }
}

impl<T: 'static, K: 'static, P> ConnectOptions<T, K, P> {
    /// Options with the ports and how to connect them.
    pub fn new(
        parts: Parts<K, P>,
        connect: impl Fn(&mut T, &K, &K, ConnectDetails, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        Self {
            parts,
            connect: Rc::new(connect),
            can_connect: None,
            point: None,
            radius: None,
            label: None,
            message: None,
        }
    }

    pub fn can_connect(mut self, can_connect: impl Fn(&T, &K, &K) -> bool + 'static) -> Self {
        self.can_connect = Some(Rc::new(can_connect));
        self
    }

    pub fn point(mut self, point: impl Fn(&T, Point<Pixels>) -> Point<f32> + 'static) -> Self {
        self.point = Some(Rc::new(point));
        self
    }

    pub fn radius(mut self, radius: impl Fn(&T) -> Pixels + 'static) -> Self {
        self.radius = Some(Rc::new(radius));
        self
    }

    pub fn label(mut self, label: impl Fn(&K) -> SharedString + 'static) -> Self {
        self.label = Some(Rc::new(label));
        self
    }

    pub fn message(
        mut self,
        message: impl Fn(&ConnectMessage<K>, &dyn Fn(&K) -> SharedString) -> String + 'static,
    ) -> Self {
        self.message = Some(Rc::new(message));
        self
    }
}

fn center(bounds: &Bounds<Pixels>) -> Point<Pixels> {
    bounds.center()
}

struct Inner<T: 'static, K, P> {
    options: RefCell<ConnectOptions<T, K, P>>,
    draft: RefCell<Option<ConnectDraft<K>>>,
    // The ports a pointer drag can land on, with their centers, read as it
    // starts.
    targets: RefCell<Vec<(K, Point<Pixels>)>>,
    drags: DragSession<T>,
}

/// Connects a slot's parts, ports (`createConnect`'s return). Cloning it
/// gives another handle to the same behavior.
pub struct Connect<T: 'static, K, P = PartInfo> {
    inner: Rc<Inner<T, K, P>>,
}

impl<T: 'static, K, P> Clone for Connect<T, K, P> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users connect a slot's parts, ports, by dragging from one to
/// another or with the keyboard (`createConnect`). Make it in the
/// component's constructor, keep it in a field, render
/// [`listener`](Connect::listener) on every frame, and hand each port's
/// presses to [`pointer_down`](Connect::pointer_down) through
/// `on_pointer_down`; style a port from [`role`](Connect::role).
///
/// [`start`](Connect::start) begins from the keyboard; send keys to
/// [`keydown`](Connect::keydown) while [`active`](Connect::active). Draw the
/// loose line from the `from` port to the draft's `x` and `y`.
pub fn create_connect<T: 'static, K: Clone + Eq + Hash + Display + 'static, P: Clone + 'static>(
    options: ConnectOptions<T, K, P>,
    cx: &Context<T>,
) -> Connect<T, K, P> {
    Connect {
        inner: Rc::new(Inner {
            options: RefCell::new(options),
            draft: RefCell::new(None),
            targets: RefCell::new(Vec::new()),
            drags: DragSession::new(cx),
        }),
    }
}

impl<T: 'static, K: Clone + Eq + Hash + Display + 'static, P: Clone + 'static> Connect<T, K, P> {
    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: ConnectOptions<T, K, P>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn options(&self) -> ConnectOptions<T, K, P> {
        self.inner.options.borrow().clone()
    }

    /// The element that follows the pointer while a line is drawn. Render
    /// it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T, K, P> {
        self.inner.drags.listener()
    }

    /// The connection being drawn, if one is.
    pub fn draft(&self) -> Option<ConnectDraft<K>> {
        self.inner.draft.borrow().clone()
    }

    /// Whether a connection is being drawn.
    pub fn active(&self) -> bool {
        self.inner.draft.borrow().is_some()
    }

    fn set_draft(&self, draft: Option<ConnectDraft<K>>, cx: &mut Context<T>) {
        *self.inner.draft.borrow_mut() = draft;
        cx.notify();
    }

    fn at(&self, this: &T, screen: Point<Pixels>) -> Point<f32> {
        let point_of = self.inner.options.borrow().point.clone();
        match point_of {
            Some(point_of) => point_of(this, screen),
            None => point(f32::from(screen.x), f32::from(screen.y)),
        }
    }

    fn label(&self, key: &K) -> SharedString {
        let label = self.inner.options.borrow().label.clone();
        match label {
            Some(label) => label(key),
            None => key.to_string().into(),
        }
    }

    fn say(&self, message: ConnectMessage<K>, window: &mut Window, cx: &mut Context<T>) {
        let options = self.options();
        let label = |key: &K| self.label(key);
        let text = match &options.message {
            Some(message_fn) => message_fn(&message, &label),
            None => connect_message(&message, &label),
        };
        announce_with(&text, AnnouncePriority::Assertive, window, cx);
    }

    fn can(&self, this: &T, from: &K, to: &K) -> bool {
        let can_connect = self.inner.options.borrow().can_connect.clone();
        from != to && can_connect.is_none_or(|can_connect| can_connect(this, from, to))
    }

    // The ports in the page, with their bounds as of their last layout.
    fn ports(&self) -> Vec<(K, Bounds<Pixels>)> {
        let parts = self.inner.options.borrow().parts.clone();
        parts
            .keys()
            .into_iter()
            .filter_map(|key| parts.bounds(&key).map(|bounds| (key, bounds)))
            .collect()
    }

    /// Starts a connection from a port with the keyboard. Returns whether
    /// it started.
    pub fn start(&self, this: &mut T, from: &K, window: &mut Window, cx: &mut Context<T>) -> bool {
        let ports = self.ports();
        let Some((_, bounds)) = ports.iter().find(|(key, _)| key == from) else {
            return false;
        };
        if !ports.iter().any(|(key, _)| self.can(this, from, key)) {
            return false;
        }
        let at = self.at(this, center(bounds));
        self.set_draft(
            Some(ConnectDraft {
                from: from.clone(),
                to: None,
                x: at.x,
                y: at.y,
                keyboard: true,
            }),
            cx,
        );
        self.say(ConnectMessage::Start { from: from.clone() }, window, cx);
        true
    }

    /// Points the connection at a port, or at none. Returns whether it did.
    pub fn target(
        &self,
        this: &mut T,
        to: Option<&K>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let Some(current) = self.draft() else {
            return false;
        };
        if to.is_some_and(|to| !self.can(this, &current.from, to)) {
            return false;
        }
        let at = to
            .and_then(|to| self.ports().into_iter().find(|(key, _)| key == to))
            .map(|(_, bounds)| self.at(this, center(&bounds)));
        let (x, y) = at.map_or((current.x, current.y), |at| (at.x, at.y));
        let from = current.from.clone();
        self.set_draft(
            Some(ConnectDraft {
                to: to.cloned(),
                x,
                y,
                ..current
            }),
            cx,
        );
        if let Some(to) = to {
            self.say(
                ConnectMessage::Target {
                    from,
                    to: to.clone(),
                },
                window,
                cx,
            );
        }
        true
    }

    /// Makes the connection the draft points at, and ends it. Returns
    /// whether it went through.
    pub fn commit(
        &self,
        this: &mut T,
        details: ConnectDetails,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let current = self.inner.draft.borrow_mut().take();
        cx.notify();
        let Some(ConnectDraft {
            from, to: Some(to), ..
        }) = current
        else {
            return false;
        };
        let connect = self.inner.options.borrow().connect.clone();
        let done = connect(this, &from, &to, details, window, cx);
        if done {
            self.say(ConnectMessage::Connect { from, to }, window, cx);
        }
        done
    }

    /// Lets go of the connection without making it.
    pub fn cancel(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let current = self.draft();
        self.inner.drags.clone().cancel(this, window, cx);
        self.set_draft(None, cx);
        if let Some(current) = current
            && current.keyboard
        {
            self.say(ConnectMessage::Cancel { from: current.from }, window, cx);
        }
    }

    /// While connecting from the keyboard: arrows go to the nearest port
    /// that can take it in their direction, Enter or Space connects, Escape
    /// and Tab let go. Returns whether it handled the key.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let Some(current) = self.draft() else {
            return false;
        };
        let key = Key::of(&event.keystroke);
        let direction = match key {
            Key::ArrowUp => Some([0., -1.]),
            Key::ArrowDown => Some([0., 1.]),
            Key::ArrowLeft => Some([-1., 0.]),
            Key::ArrowRight => Some([1., 0.]),
            _ => None,
        };
        if let Some(direction) = direction {
            window.prevent_default();
            let all = self.ports();
            let on = current.to.as_ref().unwrap_or(&current.from);
            let Some((_, from)) = all.iter().find(|(key, _)| key == on) else {
                return true;
            };
            let candidates: Vec<(K, SpatialRect)> = all
                .iter()
                .filter(|(key, _)| Some(key) != current.to.as_ref() && self.can(this, &current.from, key))
                .map(|(key, bounds)| (key.clone(), SpatialRect::from(*bounds)))
                .collect();
            if let Some(next) = nearest_in_direction(&SpatialRect::from(*from), &candidates, direction) {
                self.target(this, Some(&next), window, cx);
            }
            return true;
        }
        if key == Key::Enter || key == Key::Space {
            window.prevent_default();
            if current.to.is_some() {
                let details = ConnectDetails {
                    reason: ConnectReason::ConnectKey,
                    event: Some(InputEvent::from(event)),
                };
                self.commit(this, details, window, cx);
            }
            return true;
        }
        if key == Key::Escape || key == Key::Tab {
            if key == Key::Escape {
                window.prevent_default();
            }
            self.cancel(this, window, cx);
            return true;
        }
        false
    }

    // The port nearest a point within the radius, with its center.
    fn nearest(&self, at: Point<Pixels>, radius: Pixels) -> Option<(K, Point<Pixels>)> {
        let targets = self.inner.targets.borrow();
        let mut best: Option<(&K, Point<Pixels>, f32)> = None;
        for (key, center) in targets.iter() {
            let distance = f32::from(center.x - at.x).hypot(f32::from(center.y - at.y));
            if distance <= f32::from(radius) && best.as_ref().is_none_or(|(_, _, best)| distance < *best) {
                best = Some((key, *center, distance));
            }
        }
        best.map(|(key, center, _)| (key.clone(), center))
    }

    /// Drags a loose line out of a port; letting go near a port that can
    /// take it connects (`pointerdown`). The press stops propagating, so a
    /// surface around it doesn't pan.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        from: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drags = self.inner.drags.clone();
        if !drags.accepts(this, event, window, cx) {
            return;
        }
        cx.stop_propagation();
        let radius = self.inner.options.borrow().radius.clone();
        let radius = radius.map_or(px(28.), |radius| radius(this));
        let targets: Vec<(K, Point<Pixels>)> = self
            .ports()
            .into_iter()
            .filter(|(key, _)| self.can(this, from, key))
            .map(|(key, bounds)| (key, center(&bounds)))
            .collect();
        *self.inner.targets.borrow_mut() = targets;
        let at = self.at(this, event.position);
        self.set_draft(
            Some(ConnectDraft {
                from: from.clone(),
                to: None,
                x: at.x,
                y: at.y,
                keyboard: false,
            }),
            cx,
        );
        let moved = Rc::downgrade(&self.inner);
        let ended = moved.clone();
        let source = from.clone();
        drags.start(
            event,
            BindDragOptions::new()
                .cancel_on_escape(true)
                .on_move(move |this: &mut T, sample, _window, cx| {
                    let Some(inner) = moved.upgrade() else {
                        return;
                    };
                    let connect = Connect { inner };
                    let near = connect.nearest(sample.position(), radius);
                    // Near a port, the loose end snaps onto it.
                    let at = connect.at(this, near.as_ref().map_or(sample.position(), |(_, at)| *at));
                    connect.set_draft(
                        Some(ConnectDraft {
                            from: source.clone(),
                            to: near.map(|(key, _)| key),
                            x: at.x,
                            y: at.y,
                            keyboard: false,
                        }),
                        cx,
                    );
                })
                .on_end(move |this: &mut T, end, canceled, window, cx| {
                    let Some(inner) = ended.upgrade() else {
                        return;
                    };
                    let connect = Connect { inner };
                    connect.inner.targets.borrow_mut().clear();
                    if canceled {
                        connect.cancel(this, window, cx);
                    } else {
                        let details = ConnectDetails {
                            reason: ConnectReason::Drag,
                            event: Some(end.event.clone()),
                        };
                        connect.commit(this, details, window, cx);
                    }
                }),
            window,
            cx,
        );
    }

    /// A port's part in the connection being drawn: `Source`, `Target`,
    /// `Candidate`, or `None` (`role`).
    pub fn role(&self, this: &T, key: &K) -> Option<ConnectRole> {
        let current = self.draft()?;
        if current.from == *key {
            return Some(ConnectRole::Source);
        }
        if current.to.as_ref() == Some(key) {
            return Some(ConnectRole::Target);
        }
        self.can(this, &current.from, key).then_some(ConnectRole::Candidate)
    }
}
