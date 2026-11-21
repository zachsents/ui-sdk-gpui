//! Port of `foundation/direct-manipulation/area.ts`: a value in two
//! dimensions picked on a surface: a color's saturation and brightness, a
//! point on an equalizer, a crop's focal point, an XY pad. A press jumps the
//! thumb under the pointer and a drag follows it; arrows step each axis,
//! Shift steps ten times as far, Page Up and Page Down, Home and End jump.
//! There is no WAI-ARIA pattern for two axes, so the thumb is a slider for
//! the x axis whose text names both.
//!
//! The surface records where it lays out ([`Area::measure`]), which a press
//! reads where the original reads the element's rectangle. The thumb takes
//! the area's focus handle ([`AreaThumbAttrs::apply`]), which a press on
//! the surface focuses, as the original finds `[data-area-thumb]` inside it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gpui::{
    App, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Pixels, Point, SharedString, Styled, Subscription, WeakEntity, Window, relative,
};
use ui_sdk_drag_and_drop::{BindDragOptions, DragSession, PointerEvent, ReadFn, on_pointer_down};
use ui_sdk_kernel::roles::{HandledKey, Role};
use ui_sdk_kernel::{Aria, ChangeOptions, FocusMode, Key, Measured, Part, Reason, is_destroying};
use ui_sdk_number::{StepRange, clamp, snap_to_step};
use ui_sdk_platform::is_rtl;

use crate::js;

/// A point in an area, in its own units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AreaValue {
    pub x: f64,
    pub y: f64,
}

impl AreaValue {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Why the value changed: a press on the surface (`TrackPress`), a drag
/// (`Drag`), a key (`StepKey`), or a call from code (`Imperative`), with the
/// names a slider reports.
pub type AreaReason = Reason;

/// One axis's range and step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AreaAxis {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    /// What screen readers call the axis in the default value text.
    pub label: Option<SharedString>,
}

impl AreaAxis {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
}

// An axis with its defaults filled in.
#[derive(Clone, Debug)]
struct Axis {
    min: f64,
    max: f64,
    step: f64,
    label: Option<SharedString>,
}

impl Axis {
    fn of(axis: Option<AreaAxis>) -> Self {
        let axis = axis.unwrap_or_default();
        Self {
            min: axis.min.unwrap_or(0.0),
            max: axis.max.unwrap_or(1.0),
            step: axis.step.unwrap_or(0.01),
            label: axis.label,
        }
    }

    fn range(&self) -> StepRange {
        StepRange::new(self.min, self.max, self.step)
    }

    fn fraction(&self, n: f64) -> f64 {
        if self.max == self.min {
            0.0
        } else {
            (n - self.min) / (self.max - self.min)
        }
    }
}

/// Asks the component for a new value, with why, and returns whether it
/// went through. It runs inside the component's update, with the component.
pub type SetAreaFn<T> =
    Rc<dyn Fn(&mut T, AreaValue, ChangeOptions, &mut Window, &mut Context<T>) -> bool>;

/// What runs when an interaction ends, with the value it ended on and why.
pub type AreaCommitFn<T> =
    Rc<dyn Fn(&mut T, &AreaValue, ChangeOptions, &mut Window, &mut Context<T>)>;

/// The text screen readers hear for a value.
pub type AreaTextFn<T> = Rc<dyn Fn(&T, &AreaValue) -> SharedString>;

/// How [`create_area`] reads and changes its value (`AreaOptions`). `T` is
/// the component.
pub struct AreaOptions<T: 'static> {
    /// Reads the point from the component, like its value, or one made from
    /// other state, like a color's saturation and brightness.
    pub value: ReadFn<T, AreaValue>,
    /// Asks for a new value and returns whether it went through. Pass the
    /// component's change, so `on_*_change`, `cancel()` and controlled values
    /// work: `|this, value, details, window, cx| this.value.change(&this.instance,
    /// value, details, window, cx)`. `details.reason` is `TrackPress`, `Drag`,
    /// `StepKey` or `Imperative`.
    pub set: SetAreaFn<T>,
    /// The x axis, left to right. Defaults to `{ min: 0, max: 1, step: 0.01 }`.
    pub x: Option<ReadFn<T, AreaAxis>>,
    /// The y axis, bottom to top, like a chart: a higher value sits higher.
    /// Defaults to `{ min: 0, max: 1, step: 0.01 }`.
    pub y: Option<ReadFn<T, AreaAxis>>,
    /// The text screen readers hear for the value. Defaults to each axis's
    /// label and percentage.
    pub value_text: Option<AreaTextFn<T>>,
    /// Runs when a press or a drag ends, or a key changed the value, with the
    /// value it ended on, like Slider's `on_value_commit`.
    pub on_commit: Option<AreaCommitFn<T>>,
    /// Whether it can't change.
    pub disabled: Option<ReadFn<T, bool>>,
}

impl<T: 'static> Clone for AreaOptions<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            set: self.set.clone(),
            x: self.x.clone(),
            y: self.y.clone(),
            value_text: self.value_text.clone(),
            on_commit: self.on_commit.clone(),
            disabled: self.disabled.clone(),
        }
    }
}

impl<T: 'static> AreaOptions<T> {
    /// Options with the value and how to change it.
    pub fn new(
        value: impl Fn(&T) -> AreaValue + 'static,
        set: impl Fn(&mut T, AreaValue, ChangeOptions, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        Self {
            value: Rc::new(value),
            set: Rc::new(set),
            x: None,
            y: None,
            value_text: None,
            on_commit: None,
            disabled: None,
        }
    }

    pub fn x(mut self, x: impl Fn(&T) -> AreaAxis + 'static) -> Self {
        self.x = Some(Rc::new(x));
        self
    }

    pub fn y(mut self, y: impl Fn(&T) -> AreaAxis + 'static) -> Self {
        self.y = Some(Rc::new(y));
        self
    }

    pub fn value_text(mut self, text: impl Fn(&T, &AreaValue) -> SharedString + 'static) -> Self {
        self.value_text = Some(Rc::new(text));
        self
    }

    pub fn on_commit(
        mut self,
        on_commit: impl Fn(&mut T, &AreaValue, ChangeOptions, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        self.on_commit = Some(Rc::new(on_commit));
        self
    }

    pub fn disabled(mut self, disabled: impl Fn(&T) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(disabled));
        self
    }
}

/// The keys [`Area::keydown`] handles, for the thumb's `on_keys`.
pub const AREA_KEYS: &[HandledKey] = &[
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "ArrowDown",
    "PageUp",
    "PageDown",
    "Home",
    "End",
];

struct AreaInner<T: 'static> {
    options: RefCell<AreaOptions<T>>,
    entity: WeakEntity<T>,
    drags: DragSession<T>,
    surface: Measured,
    thumb: FocusHandle,
    // While a pointer drags: what ends the drag if the area is disabled.
    watching: RefCell<Option<Subscription>>,
}

/// Lets users pick a point on a surface (`createArea`'s return). Cloning it
/// gives another handle to the same area.
pub struct Area<T: 'static> {
    inner: Rc<AreaInner<T>>,
}

impl<T: 'static> Clone for Area<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users pick a point on a surface (`createArea`). Make it in the
/// component's constructor, keep it in a field, and wire the surface and
/// its thumb:
///
/// ```ignore
/// // The surface: its bounds, its presses, and the listener drags hear the pointer through.
/// let pad = this.area.bind_surface(div().id("pad"));
/// // The thumb: a slider for x whose text names both axes.
/// let thumb = this.area.thumb_attrs(this).apply(Part::new(&this.instance, "thumb", None, ()))
///     .on_keys(AREA_KEYS, move |event, window, cx| {
///         entity.update(cx, |this, cx| { this.area.clone().keydown(this, event, window, cx); }).ok();
///     });
/// let thumb = this.area.thumb_style(this).apply(thumb, is_rtl(cx));
/// ```
///
/// The thumb's style places it absolutely in the surface, at its start in
/// the reading direction and from the top, with the fractions from 0 to 1 on
/// each axis for a gradient or a crosshair, the original's `--area-x` and
/// `--area-y`.
pub fn create_area<T: 'static>(options: AreaOptions<T>, cx: &Context<T>) -> Area<T> {
    Area::new(cx.weak_entity(), options, cx)
}

impl<T: 'static> Area<T> {
    /// An area of the component `entity`, made where there's no context for
    /// it, like one of several a color picker makes as its parts ask for
    /// them.
    pub fn new(entity: WeakEntity<T>, options: AreaOptions<T>, cx: &App) -> Self {
        Self {
            inner: Rc::new(AreaInner {
                options: RefCell::new(options),
                drags: DragSession::for_entity(entity.clone()),
                entity,
                surface: Measured::default(),
                thumb: cx.focus_handle(),
                watching: RefCell::new(None),
            }),
        }
    }

    fn upgrade(inner: &Weak<AreaInner<T>>) -> Option<Self> {
        inner.upgrade().map(|inner| Self { inner })
    }

    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: AreaOptions<T>) {
        *self.inner.options.borrow_mut() = options;
    }

    /// The thumb's focus handle, which a press on the surface focuses so the
    /// keyboard picks up where the pointer left off.
    pub fn focus_handle(&self) -> FocusHandle {
        self.inner.thumb.clone()
    }

    fn x(&self, this: &T) -> Axis {
        let x = self.inner.options.borrow().x.clone();
        Axis::of(x.map(|x| x(this)))
    }

    fn y(&self, this: &T) -> Axis {
        let y = self.inner.options.borrow().y.clone();
        Axis::of(y.map(|y| y(this)))
    }

    fn value(&self, this: &T) -> AreaValue {
        let value = self.inner.options.borrow().value.clone();
        value(this)
    }

    fn is_disabled(&self, this: &T) -> bool {
        let disabled = self.inner.options.borrow().disabled.clone();
        disabled.is_some_and(|disabled| disabled(this))
    }

    // Snaps `next` to each axis's step and asks for it, unless it's the
    // value already.
    fn change(
        &self,
        this: &mut T,
        next: AreaValue,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let snapped = AreaValue {
            x: snap_to_step(next.x, self.x(this).range()),
            y: snap_to_step(next.y, self.y(this).range()),
        };
        let current = self.value(this);
        if snapped.x == current.x && snapped.y == current.y {
            return false;
        }
        let set = self.inner.options.borrow().set.clone();
        set(this, snapped, options, window, cx)
    }

    fn commit(
        &self,
        this: &mut T,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let on_commit = self.inner.options.borrow().on_commit.clone();
        if let Some(on_commit) = on_commit {
            let value = self.value(this);
            on_commit(this, &value, options, window, cx);
        }
    }

    // The value under a point on the surface.
    fn at(&self, this: &T, point: Point<Pixels>, cx: &App) -> AreaValue {
        let bounds = self.inner.surface.get().unwrap_or_default();
        let length = |pixels: Pixels| f64::from(f32::from(pixels));
        let (width, height) = (length(bounds.size.width), length(bounds.size.height));
        let mut fx = if width != 0.0 {
            (length(point.x) - length(bounds.left())) / width
        } else {
            0.0
        };
        if is_rtl(cx) {
            fx = 1.0 - fx;
        }
        let fy = if height != 0.0 {
            1.0 - (length(point.y) - length(bounds.top())) / height
        } else {
            0.0
        };
        let (ax, ay) = (self.x(this), self.y(this));
        AreaValue {
            x: ax.min + clamp(fx, 0.0, 1.0) * (ax.max - ax.min),
            y: ay.min + clamp(fy, 0.0, 1.0) * (ay.max - ay.min),
        }
    }

    /// Sets the value, snapped to each axis's step. Reports `Imperative`.
    /// Returns whether it changed.
    pub fn set(
        &self,
        this: &mut T,
        next: AreaValue,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        self.change(
            this,
            next,
            ChangeOptions::reason(Reason::Imperative),
            window,
            cx,
        )
    }

    /// Jumps the thumb under the pointer and follows it until it lifts. Call
    /// it from the surface's press
    /// ([`on_pointer_down`]), or let
    /// [`bind_surface`](Self::bind_surface) do it.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drags = self.inner.drags.clone();
        if !drags.accepts(this, event, window, cx) || self.is_disabled(this) {
            return;
        }
        window.prevent_default();
        let at = self.at(this, event.position, cx);
        self.change(
            this,
            at,
            ChangeOptions::event_of(Reason::TrackPress, event.event.clone()),
            window,
            cx,
        );
        // Focus goes to the thumb, so the keyboard picks up where the pointer left off.
        if self.inner.thumb.is_rendered(window) {
            window.focus(&self.inner.thumb, cx);
        }
        // The commit names what changed the value last, as a slider's does.
        let reason = Rc::new(Cell::new(Reason::TrackPress));
        // Disabling it mid-drag ends the drag, like a slider's.
        let weak = Rc::downgrade(&self.inner);
        let entity = cx.entity();
        let watching = cx.observe_in(&entity, window, {
            let weak = weak.clone();
            move |this, _, window, cx| {
                if let Some(area) = Self::upgrade(&weak)
                    && area.is_disabled(this)
                {
                    area.inner.drags.clone().cancel(this, window, cx);
                }
            }
        });
        *self.inner.watching.borrow_mut() = Some(watching);
        let moved = (weak.clone(), reason.clone());
        let ended = (weak, reason);
        drags.start(
            event,
            BindDragOptions::new()
                .on_move(move |this: &mut T, sample, window, cx| {
                    let (weak, reason) = &moved;
                    let Some(area) = Self::upgrade(weak) else {
                        return;
                    };
                    let at = area.at(this, sample.position(), cx);
                    let options = ChangeOptions::event_of(Reason::Drag, sample.event.event.clone());
                    if area.change(this, at, options, window, cx) {
                        reason.set(Reason::Drag);
                    }
                })
                .on_end(move |this: &mut T, end, _canceled, window, cx| {
                    let (weak, reason) = &ended;
                    let Some(area) = Self::upgrade(weak) else {
                        return;
                    };
                    let watching = area.inner.watching.borrow_mut().take();
                    drop(watching);
                    // A teardown cut the drag short, so nobody ended it to commit.
                    if !is_destroying() {
                        area.commit(
                            this,
                            ChangeOptions::event_of(reason.get(), end.event.clone()),
                            window,
                            cx,
                        );
                    }
                }),
            window,
            cx,
        );
    }

    /// Handles a key on the thumb and returns whether it did. Arrows step an
    /// axis, Shift ten steps; Page Up and Page Down step y by a tenth of its
    /// range; Home and End go to x's ends. A key it handles calls
    /// `window.prevent_default()`.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented()
            || self.is_disabled(this)
            || modifiers.alt
            || modifiers.control
            || modifiers.platform
        {
            return false;
        }
        let (ax, ay) = (self.x(this), self.y(this));
        let current = self.value(this);
        let times = if modifiers.shift { 10.0 } else { 1.0 };
        let rtl = is_rtl(cx);
        let key = Key::of(&event.keystroke);
        let towards = |up: bool| if up { 1.0 } else { -1.0 };
        let next = match key {
            Key::ArrowLeft | Key::ArrowRight => {
                let sign = towards(key == Key::ArrowRight) * towards(!rtl);
                Some(AreaValue {
                    x: current.x + sign * ax.step * times,
                    ..current
                })
            }
            Key::ArrowUp | Key::ArrowDown => Some(AreaValue {
                y: current.y + towards(key == Key::ArrowUp) * ay.step * times,
                ..current
            }),
            Key::PageUp | Key::PageDown => Some(AreaValue {
                y: current.y + (towards(key == Key::PageUp) * (ay.max - ay.min)) / 10.0,
                ..current
            }),
            Key::Home => Some(AreaValue {
                x: ax.min,
                ..current
            }),
            Key::End => Some(AreaValue {
                x: ax.max,
                ..current
            }),
            _ => None,
        };
        let Some(next) = next else {
            return false;
        };
        window.prevent_default();
        if self.change(
            this,
            next,
            ChangeOptions::event(Reason::StepKey, event),
            window,
            cx,
        ) {
            self.commit(
                this,
                ChangeOptions::event(Reason::StepKey, event),
                window,
                cx,
            );
        }
        true
    }

    /// Where the value sits, from 0 to 1 on each axis, with y from the bottom.
    pub fn fractions(&self, this: &T) -> AreaValue {
        let current = self.value(this);
        AreaValue {
            x: self.x(this).fraction(current.x),
            y: self.y(this).fraction(current.y),
        }
    }

    /// The thumb's position (`thumbStyle`).
    pub fn thumb_style(&self, this: &T) -> AreaThumbStyle {
        let fractions = self.fractions(this);
        AreaThumbStyle {
            x: fractions.x,
            y: fractions.y,
        }
    }

    /// The thumb's role, focus and value for screen readers (`thumbAttrs`).
    pub fn thumb_attrs(&self, this: &T) -> AreaThumbAttrs {
        let current = self.value(this);
        let (ax, ay) = (self.x(this), self.y(this));
        let percent =
            |n: f64, axis: &Axis| format!("{}%", js::number(js::round(axis.fraction(n) * 100.0)));
        let value_text = self.inner.options.borrow().value_text.clone();
        let text = match value_text {
            Some(text) => text(this, &current),
            None => format!(
                "{} {}, {} {}",
                ax.label.as_deref().unwrap_or("X"),
                percent(current.x, &ax),
                ay.label.as_deref().unwrap_or("Y"),
                percent(current.y, &ay),
            )
            .into(),
        };
        AreaThumbAttrs {
            min: ax.min,
            max: ax.max,
            now: current.x,
            text,
            disabled: self.is_disabled(this),
            focus: self.inner.thumb.clone(),
        }
    }

    /// Adds a layer to the surface's element that records its bounds each
    /// layout, which a press reads.
    pub fn measure<E: ParentElement + Styled>(&self, el: E) -> E {
        self.inner.surface.measure(el)
    }

    /// The element that follows the pointer while a press on the surface
    /// lasts. Render it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T> {
        self.inner.drags.listener()
    }

    /// Wires the surface: records its bounds, hears its presses, which call
    /// [`pointer_down`](Self::pointer_down), and renders the
    /// [`listener`](Self::listener) inside it.
    pub fn bind_surface<E: InteractiveElement + ParentElement + Styled>(&self, el: E) -> E {
        let area = self.clone();
        let el = on_pointer_down(self.measure(el), move |event, window, cx| {
            let Some(entity) = area.inner.entity.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| area.pointer_down(this, event, window, cx));
        });
        el.child(self.listener())
    }
}

/// Where the thumb sits (`thumbStyle`): the value's fractions from 0 to 1 on
/// each axis, with y from the bottom, the original's `--area-x` and
/// `--area-y`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaThumbStyle {
    pub x: f64,
    pub y: f64,
}

impl AreaThumbStyle {
    /// Places the thumb absolutely in the surface: `x` from its start in the
    /// reading direction (the right side when `rtl`, `inset-inline-start`)
    /// and `1 - y` from its top. Center the thumb on that point yourself.
    pub fn apply<E: Styled>(self, el: E, rtl: bool) -> E {
        let start = relative(js::style_fraction(self.x));
        let el = el.absolute();
        let el = if rtl { el.right(start) } else { el.left(start) };
        el.top(relative(js::style_fraction(1.0 - self.y)))
    }
}

/// The thumb's role, focus and value for screen readers (`thumbAttrs`): a
/// slider for the x axis, named a 2D slider, whose value text names both.
#[derive(Clone, Debug)]
pub struct AreaThumbAttrs {
    /// `aria-valuemin`: x's minimum.
    pub min: f64,
    /// `aria-valuemax`: x's maximum.
    pub max: f64,
    /// `aria-valuenow`: x.
    pub now: f64,
    /// `aria-valuetext`: both axes, from `value_text` or each axis's label
    /// and percentage.
    pub text: SharedString,
    /// `aria-disabled`, which takes the thumb out of the tab order.
    pub disabled: bool,
    focus: FocusHandle,
}

impl AreaThumbAttrs {
    /// The area's focus handle, which the thumb tracks.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The ARIA gpui's setters leave out: the role description, the value
    /// text and `aria-disabled`.
    pub fn aria(&self) -> Aria {
        Aria::new()
            .role_description("2D slider")
            .value_text(self.text.clone())
            .disabled(self.disabled)
    }

    /// Puts the slider role, the area's focus handle (a tab stop unless
    /// disabled) and the value on the thumb part.
    pub fn apply<S>(&self, part: Part<S>) -> Part<S> {
        let part = part.aria_role(Role::Slider).focus_mode(
            FocusMode::Native,
            Some(&self.focus),
            self.disabled,
        );
        self.apply_values(part)
    }

    /// Puts only the value on a part that declares its own role and focus,
    /// like a color picker's thumb: gpui's minimum, maximum and value, and
    /// [`aria`](Self::aria).
    pub fn apply_values<S>(&self, part: Part<S>) -> Part<S> {
        use gpui::StatefulInteractiveElement as _;
        part.aria_min_numeric_value(self.min)
            .aria_max_numeric_value(self.max)
            .aria_numeric_value(self.now)
            .aria(self.aria())
    }
}
