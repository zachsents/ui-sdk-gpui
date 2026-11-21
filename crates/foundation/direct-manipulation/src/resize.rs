//! Port of `foundation/direct-manipulation/resize.ts`: resizing boxes by
//! their edges and corners: a crop on an image, a window on a desktop, a
//! widget on a dashboard. A pointer drags a handle, or the box itself to
//! move it, through a draft box the resize keeps; the drop commits once.
//! Arrows on a focused handle move its edges, and screen readers hear the
//! new size.
//!
//! The original writes the draft to the element as custom properties, so a
//! drag renders nothing. gpui renders the component again as the draft
//! changes, and [`Resize::box_style`] places the box as it shows. A handle
//! inside its box marks the events it sees ([`Resize::pointer_down`] and
//! [`Resize::keydown`] with its handle), which the box's own `Move` then
//! leaves alone, as the original asks `closest("[data-resize-handle]")`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;
use std::rc::{Rc, Weak};

use gpui::{
    Context, CursorStyle, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    SharedString, Styled, WeakEntity, Window, px,
};
use ui_sdk_drag_and_drop::{
    BindDragOptions, DRAG_THRESHOLD, DragSession, KeyFn, MessageFn, PointerEvent, ReadFn,
    on_pointer_down,
};
use ui_sdk_kernel::{ChangeOptions, Key, Reason};
use ui_sdk_number::clamp;
use ui_sdk_platform::{AnnouncePriority, announce_with};

use crate::js;
use crate::target::{Mark, mark_key, mark_press, marked_key, marked_press};

/// Where a box sits and how big it is, in the units your boxes use, usually
/// pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ResizeBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ResizeBox {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// The edge or corner a handle drags, by compass point, or `Move` for the
/// whole box (`ResizeHandle`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ResizeHandle {
    N,
    E,
    S,
    W,
    Ne,
    Nw,
    #[default]
    Se,
    Sw,
    Move,
}

impl ResizeHandle {
    /// Every handle, the eight edges and corners and `Move`.
    pub const ALL: [ResizeHandle; 9] = [
        ResizeHandle::N,
        ResizeHandle::E,
        ResizeHandle::S,
        ResizeHandle::W,
        ResizeHandle::Ne,
        ResizeHandle::Nw,
        ResizeHandle::Se,
        ResizeHandle::Sw,
        ResizeHandle::Move,
    ];

    /// The original's name for it, like `"ne"`, its `data-resize-handle`.
    pub fn as_str(self) -> &'static str {
        match self {
            ResizeHandle::N => "n",
            ResizeHandle::E => "e",
            ResizeHandle::S => "s",
            ResizeHandle::W => "w",
            ResizeHandle::Ne => "ne",
            ResizeHandle::Nw => "nw",
            ResizeHandle::Se => "se",
            ResizeHandle::Sw => "sw",
            ResizeHandle::Move => "move",
        }
    }

    /// The cursor that shows what the handle does ([`resize_cursors`]).
    pub fn cursor(self) -> CursorStyle {
        resize_cursors(self)
    }

    fn west(self) -> bool {
        matches!(self, ResizeHandle::W | ResizeHandle::Nw | ResizeHandle::Sw)
    }

    fn east(self) -> bool {
        matches!(self, ResizeHandle::E | ResizeHandle::Ne | ResizeHandle::Se)
    }

    fn north(self) -> bool {
        matches!(self, ResizeHandle::N | ResizeHandle::Ne | ResizeHandle::Nw)
    }

    fn south(self) -> bool {
        matches!(self, ResizeHandle::S | ResizeHandle::Se | ResizeHandle::Sw)
    }
}

/// The box a pointer is changing (`ResizeDraft`).
#[derive(Clone, Debug, PartialEq)]
pub struct ResizeDraft<K> {
    pub key: K,
    pub handle: ResizeHandle,
    pub r#box: ResizeBox,
}

/// Why a box changed: a pointer drag (`Drag`), a key on a handle
/// (`ResizeKey`), or a call from code (`Imperative`).
pub type ResizeReason = Reason;

/// What happened, for the message screen readers hear (`ResizeMessage`).
#[derive(Clone, Debug, PartialEq)]
pub struct ResizeMessage<K> {
    pub key: K,
    /// What the box is called, from `label`, else its key.
    pub label: SharedString,
    pub handle: ResizeHandle,
    pub r#box: ResizeBox,
}

/// The English messages [`create_resize`] announces unless you pass
/// `message` (`resizeMessage`).
pub fn resize_message<K>(message: &ResizeMessage<K>) -> String {
    let ResizeMessage {
        label,
        handle,
        r#box,
        ..
    } = message;
    if *handle == ResizeHandle::Move {
        format!(
            "{label} moved to {}, {}.",
            js::number(r#box.x),
            js::number(r#box.y)
        )
    } else {
        format!(
            "{label} resized to {} by {}.",
            js::number(r#box.width),
            js::number(r#box.height)
        )
    }
}

/// The cursor that shows what each handle does (`resizeCursors`): the
/// two-way arrow along its edges, and gpui's `move`, a closed hand, for
/// `Move`.
pub fn resize_cursors(handle: ResizeHandle) -> CursorStyle {
    match handle {
        ResizeHandle::N | ResizeHandle::S => CursorStyle::ResizeUpDown,
        ResizeHandle::E | ResizeHandle::W => CursorStyle::ResizeLeftRight,
        ResizeHandle::Ne | ResizeHandle::Sw => CursorStyle::ResizeUpRightDownLeft,
        ResizeHandle::Nw | ResizeHandle::Se => CursorStyle::ResizeUpLeftDownRight,
        ResizeHandle::Move => CursorStyle::ClosedHand,
    }
}

// How far each arrow moves, on screen.
fn arrow(key: &Key) -> Option<(f64, f64)> {
    match key {
        Key::ArrowUp => Some((0.0, -1.0)),
        Key::ArrowDown => Some((0.0, 1.0)),
        Key::ArrowLeft => Some((-1.0, 0.0)),
        Key::ArrowRight => Some((1.0, 0.0)),
        _ => None,
    }
}

/// A size: the smallest or largest a box gets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeSize {
    pub width: f64,
    pub height: f64,
}

impl ResizeSize {
    pub fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// An area a box stays inside, like the image a crop sits on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeBounds {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl ResizeBounds {
    pub fn new(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

/// The limits [`resize_box`] keeps a box within (`ResizeLimits`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ResizeLimits {
    /// The area the box stays inside.
    pub bounds: Option<ResizeBounds>,
    /// The smallest it gets. Defaults to 1 by 1.
    pub min: Option<ResizeSize>,
    /// The largest it gets.
    pub max: Option<ResizeSize>,
    /// Width over height, kept while it resizes.
    pub aspect: Option<f64>,
}

impl ResizeLimits {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bounds(mut self, bounds: ResizeBounds) -> Self {
        self.bounds = Some(bounds);
        self
    }

    pub fn min(mut self, min: ResizeSize) -> Self {
        self.min = Some(min);
        self
    }

    pub fn max(mut self, max: ResizeSize) -> Self {
        self.max = Some(max);
        self
    }

    pub fn aspect(mut self, aspect: f64) -> Self {
        self.aspect = Some(aspect);
        self
    }
}

/// The box a handle makes when its edges move by `dx` and `dy` from `from`,
/// within the limits, as a drag of that handle makes it (`resizeBox`). Pure.
pub fn resize_box(
    from: ResizeBox,
    handle: ResizeHandle,
    dx: f64,
    dy: f64,
    limits: &ResizeLimits,
) -> ResizeBox {
    let area = limits.bounds;
    let left = area.map_or(f64::NEG_INFINITY, |area| area.left);
    let top = area.map_or(f64::NEG_INFINITY, |area| area.top);
    let right = area.map_or(f64::INFINITY, |area| area.right);
    let bottom = area.map_or(f64::INFINITY, |area| area.bottom);
    if handle == ResizeHandle::Move {
        return ResizeBox {
            x: clamp(from.x + dx, left, js::max(left, right - from.width)),
            y: clamp(from.y + dy, top, js::max(top, bottom - from.height)),
            ..from
        };
    }
    let min = limits.min.unwrap_or(ResizeSize::new(1.0, 1.0));
    let max = limits
        .max
        .unwrap_or(ResizeSize::new(f64::INFINITY, f64::INFINITY));
    let (west, east, north, south) = (handle.west(), handle.east(), handle.north(), handle.south());
    // The edges that stay put, and how far the moving ones may go.
    let anchor_x = if west { from.x + from.width } else { from.x };
    let anchor_y = if north { from.y + from.height } else { from.y };
    let room_x = js::min(
        max.width,
        if west {
            anchor_x - left
        } else {
            right - anchor_x
        },
    );
    let room_y = js::min(
        max.height,
        if north {
            anchor_y - top
        } else {
            bottom - anchor_y
        },
    );
    let mut width = if west {
        from.width - dx
    } else if east {
        from.width + dx
    } else {
        from.width
    };
    let mut height = if north {
        from.height - dy
    } else if south {
        from.height + dy
    } else {
        from.height
    };
    match limits
        .aspect
        .filter(|ratio| *ratio != 0.0 && !ratio.is_nan())
    {
        Some(ratio) => {
            // A corner follows whichever edge moved further; an edge sets the
            // other side. The height follows the width once it's fit, below.
            let height_leads = if (west || east) && (north || south) {
                // False for NaN too, which the original leaves to the height.
                let width_leads =
                    (width / from.width - 1.0).abs() >= (height / from.height - 1.0).abs();
                !width_leads
            } else {
                !(west || east)
            };
            if height_leads {
                width = height * ratio;
            }
            // An edge grows the other side about the box's middle, so it has room on both sides of it.
            let middle_y = from.y + from.height / 2.0;
            let middle_x = from.x + from.width / 2.0;
            let fit_y = if north || south {
                room_y
            } else {
                2.0 * js::min(js::min(middle_y - top, bottom - middle_y), max.height / 2.0)
            };
            let fit_x = if west || east {
                room_x
            } else {
                2.0 * js::min(js::min(middle_x - left, right - middle_x), max.width / 2.0)
            };
            width = clamp(
                width,
                js::max(min.width, min.height * ratio),
                js::min(fit_x, fit_y * ratio),
            );
            height = width / ratio;
        }
        None => {
            width = clamp(width, min.width, js::max(min.width, room_x));
            height = clamp(height, min.height, js::max(min.height, room_y));
        }
    }
    let x = if west {
        anchor_x - width
    } else if east {
        anchor_x
    } else {
        from.x + (from.width - width) / 2.0
    };
    let y = if north {
        anchor_y - height
    } else if south {
        anchor_y
    } else {
        from.y + (from.height - height) / 2.0
    };
    ResizeBox {
        x,
        y,
        width,
        height,
    }
}

// Whether an event reached the box from a handle inside it, which a handle
// marks, since handles sit inside their box and the box hears their events too.
fn from_handle(handle: ResizeHandle, marked: bool) -> bool {
    handle == ResizeHandle::Move && marked
}

// The box on a grid of `size`.
fn round(r#box: ResizeBox, size: f64) -> ResizeBox {
    let on = |n: f64| js::round(n / size) * size;
    ResizeBox {
        x: on(r#box.x),
        y: on(r#box.y),
        width: on(r#box.width),
        height: on(r#box.height),
    }
}

/// Asks the component for new boxes, with why, and returns whether they went
/// through. It runs inside the component's update, with the component.
pub type SetBoxesFn<T, K> =
    Rc<dyn Fn(&mut T, HashMap<K, ResizeBox>, ChangeOptions, &mut Window, &mut Context<T>) -> bool>;

/// How [`create_resize`] reads and changes boxes (`ResizeOptions`). `T` is
/// the component, `K` a box's key.
pub struct ResizeOptions<T: 'static, K> {
    /// Every box, by key, like the component's boxes, or a map of its one
    /// crop.
    pub boxes: ReadFn<T, HashMap<K, ResizeBox>>,
    /// Asks for new boxes and returns whether they went through. Pass the
    /// component's change: `|this, value, details, window, cx|
    /// this.boxes.change(&this.instance, value, details, window, cx)`, or
    /// its crop's change with `value[&key]` for a component with one box.
    pub set: SetBoxesFn<T, K>,
    /// The smallest a box gets. Defaults to 1 by 1.
    pub min: Option<KeyFn<T, K, ResizeSize>>,
    /// The largest a box gets.
    pub max: Option<KeyFn<T, K, Option<ResizeSize>>>,
    /// Keeps a box inside an area, like the image a crop sits on.
    pub bounds: Option<KeyFn<T, K, Option<ResizeBounds>>>,
    /// Keeps width over height at this ratio, like `16. / 9.`. Without one,
    /// Shift on a corner keeps the ratio the box had when the drag started.
    pub aspect: Option<KeyFn<T, K, Option<f64>>>,
    /// The grid a dropped box lands on, and how far an arrow moves an edge.
    /// Defaults to 1.
    pub step: Option<ReadFn<T, f64>>,
    /// How far Shift with an arrow moves an edge. Defaults to ten steps.
    pub large_step: Option<ReadFn<T, f64>>,
    /// How many screen pixels one unit takes, like a canvas's zoom.
    /// Defaults to 1.
    pub scale: Option<ReadFn<T, f64>>,
    /// Whether a box can't be changed.
    pub disabled: Option<KeyFn<T, K, bool>>,
    /// The name screen readers hear for a box. Return `None` for its key.
    pub label: Option<KeyFn<T, K, Option<SharedString>>>,
    /// The text screen readers hear after a change. Defaults to English
    /// ([`resize_message`]).
    pub message: Option<MessageFn<ResizeMessage<K>>>,
}

impl<T: 'static, K> Clone for ResizeOptions<T, K> {
    fn clone(&self) -> Self {
        Self {
            boxes: self.boxes.clone(),
            set: self.set.clone(),
            min: self.min.clone(),
            max: self.max.clone(),
            bounds: self.bounds.clone(),
            aspect: self.aspect.clone(),
            step: self.step.clone(),
            large_step: self.large_step.clone(),
            scale: self.scale.clone(),
            disabled: self.disabled.clone(),
            label: self.label.clone(),
            message: self.message.clone(),
        }
    }
}

impl<T: 'static, K: 'static> ResizeOptions<T, K> {
    /// Options with the boxes and how to change them.
    pub fn new(
        boxes: impl Fn(&T) -> HashMap<K, ResizeBox> + 'static,
        set: impl Fn(&mut T, HashMap<K, ResizeBox>, ChangeOptions, &mut Window, &mut Context<T>) -> bool
        + 'static,
    ) -> Self {
        Self {
            boxes: Rc::new(boxes),
            set: Rc::new(set),
            min: None,
            max: None,
            bounds: None,
            aspect: None,
            step: None,
            large_step: None,
            scale: None,
            disabled: None,
            label: None,
            message: None,
        }
    }

    pub fn min(mut self, min: impl Fn(&T, &K) -> ResizeSize + 'static) -> Self {
        self.min = Some(Rc::new(min));
        self
    }

    pub fn max(mut self, max: impl Fn(&T, &K) -> Option<ResizeSize> + 'static) -> Self {
        self.max = Some(Rc::new(max));
        self
    }

    pub fn bounds(mut self, bounds: impl Fn(&T, &K) -> Option<ResizeBounds> + 'static) -> Self {
        self.bounds = Some(Rc::new(bounds));
        self
    }

    pub fn aspect(mut self, aspect: impl Fn(&T, &K) -> Option<f64> + 'static) -> Self {
        self.aspect = Some(Rc::new(aspect));
        self
    }

    pub fn step(mut self, step: impl Fn(&T) -> f64 + 'static) -> Self {
        self.step = Some(Rc::new(step));
        self
    }

    pub fn large_step(mut self, large_step: impl Fn(&T) -> f64 + 'static) -> Self {
        self.large_step = Some(Rc::new(large_step));
        self
    }

    pub fn scale(mut self, scale: impl Fn(&T) -> f64 + 'static) -> Self {
        self.scale = Some(Rc::new(scale));
        self
    }

    pub fn disabled(mut self, disabled: impl Fn(&T, &K) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(disabled));
        self
    }

    pub fn label(mut self, label: impl Fn(&T, &K) -> Option<SharedString> + 'static) -> Self {
        self.label = Some(Rc::new(label));
        self
    }

    pub fn message(mut self, message: impl Fn(&ResizeMessage<K>) -> String + 'static) -> Self {
        self.message = Some(Rc::new(message));
        self
    }
}

struct ResizeInner<T: 'static, K> {
    options: RefCell<ResizeOptions<T, K>>,
    entity: WeakEntity<T>,
    drags: DragSession<T>,
    draft: RefCell<Option<ResizeDraft<K>>>,
}

/// Lets users resize and move boxes (`createResize`'s return). Cloning it
/// gives another handle to the same resize.
pub struct Resize<T: 'static, K> {
    inner: Rc<ResizeInner<T, K>>,
}

impl<T: 'static, K> Clone for Resize<T, K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users resize a component's boxes by handles on their edges and
/// corners, and move them by dragging the box (`createResize`). Make it in
/// the component's constructor, keep it in a field, render its
/// [`listener`](Resize::listener) on every frame, and wire the box and its
/// handles, each its own part:
///
/// ```ignore
/// // The box, placed as it shows and moved by a drag.
/// let frame = match resize.box_style(this, &key) {
///     Some(style) => style.apply(part),
///     None => part,
/// };
/// let frame = resize.bind_box(key.clone(), frame);
/// // A handle: its cursor, its drags and its arrows.
/// let handle = resize.bind_handle(key.clone(), ResizeHandle::Se, handle_part)
///     .on_keys(&["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"], move |event, window, cx| {
///         entity.update(cx, |this, cx| { this.resize.clone().keydown(this, event, &key, ResizeHandle::Se, window, cx); }).ok();
///     });
/// ```
pub fn create_resize<T: 'static, K: Clone + Eq + Hash + Display + 'static>(
    options: ResizeOptions<T, K>,
    cx: &Context<T>,
) -> Resize<T, K> {
    Resize {
        inner: Rc::new(ResizeInner {
            options: RefCell::new(options),
            entity: cx.weak_entity(),
            drags: DragSession::new(cx),
            draft: RefCell::new(None),
        }),
    }
}

impl<T: 'static, K: Clone + Eq + Hash + Display + 'static> Resize<T, K> {
    fn upgrade(inner: &Weak<ResizeInner<T, K>>) -> Option<Self> {
        inner.upgrade().map(|inner| Self { inner })
    }

    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: ResizeOptions<T, K>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn options(&self) -> ResizeOptions<T, K> {
        self.inner.options.borrow().clone()
    }

    fn boxes(&self, this: &T) -> HashMap<K, ResizeBox> {
        let boxes = self.inner.options.borrow().boxes.clone();
        boxes(this)
    }

    fn is_disabled(&self, this: &T, key: &K) -> bool {
        let disabled = self.inner.options.borrow().disabled.clone();
        disabled.is_some_and(|disabled| disabled(this, key))
    }

    fn step(&self, this: &T) -> f64 {
        let step = self.inner.options.borrow().step.clone();
        step.map_or(1.0, |step| step(this))
    }

    #[allow(clippy::too_many_arguments)]
    fn fit(
        &self,
        this: &T,
        key: &K,
        from: ResizeBox,
        handle: ResizeHandle,
        dx: f64,
        dy: f64,
        keep_ratio: bool,
    ) -> ResizeBox {
        let options = self.options();
        let aspect = options
            .aspect
            .as_ref()
            .and_then(|aspect| aspect(this, key))
            .or_else(|| keep_ratio.then(|| from.width / from.height));
        let limits = ResizeLimits {
            bounds: options.bounds.as_ref().and_then(|bounds| bounds(this, key)),
            min: options.min.as_ref().map(|min| min(this, key)),
            max: options.max.as_ref().and_then(|max| max(this, key)),
            aspect,
        };
        resize_box(from, handle, dx, dy, &limits)
    }

    /// The box as it shows now: the draft's while a pointer drags it, else
    /// the component's (`box`).
    pub fn r#box(&self, this: &T, key: &K) -> Option<ResizeBox> {
        if let Some(draft) = self.inner.draft.borrow().as_ref()
            && draft.key == *key
        {
            return Some(draft.r#box);
        }
        self.boxes(this).get(key).copied()
    }

    /// Sets a box, in one change, and tells screen readers its new size, or
    /// where it moved for `Move`. `ResizeHandle::Se` is the original's
    /// default handle. Returns whether it went through.
    #[allow(clippy::too_many_arguments)]
    pub fn resize_to(
        &self,
        this: &mut T,
        key: &K,
        r#box: ResizeBox,
        options: ChangeOptions,
        handle: ResizeHandle,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let mut boxes = self.boxes(this);
        if boxes.get(key) == Some(&r#box) {
            return false;
        }
        boxes.insert(key.clone(), r#box);
        let set = self.inner.options.borrow().set.clone();
        let done = set(this, boxes, options, window, cx);
        if done {
            let resize = self.options();
            let label = resize
                .label
                .as_ref()
                .and_then(|label| label(this, key))
                .unwrap_or_else(|| key.to_string().into());
            let message = ResizeMessage {
                key: key.clone(),
                label,
                handle,
                r#box,
            };
            let text = match &resize.message {
                Some(message_fn) => message_fn(&message),
                None => resize_message(&message),
            };
            announce_with(&text, AnnouncePriority::Assertive, window, cx);
            cx.notify();
        }
        done
    }

    /// Moves a handle's edges by an offset, within the limits, like an arrow
    /// key does. A disabled box ignores it. Returns whether it went through.
    #[allow(clippy::too_many_arguments)]
    pub fn resize_by(
        &self,
        this: &mut T,
        key: &K,
        handle: ResizeHandle,
        dx: f64,
        dy: f64,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let Some(from) = self.boxes(this).get(key).copied() else {
            return false;
        };
        if self.is_disabled(this, key) {
            return false;
        }
        let r#box = self.fit(this, key, from, handle, dx, dy, false);
        self.resize_to(this, key, r#box, options, handle, window, cx)
    }

    /// Handles a key on a handle, or on the box for `Move`, and returns
    /// whether it did: arrows move the handle's edges, or the box for
    /// `Move`, a step, or `large_step` with Shift; an edge handle moves along
    /// its own axis only. Reports `ResizeKey`. A key it handles calls
    /// `window.prevent_default()`.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        key: &K,
        handle: ResizeHandle,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        // Handles sit inside their box, so the box hears their keys too.
        if handle != ResizeHandle::Move {
            mark_key(Mark::ResizeHandle, event, window, cx);
        }
        let modifiers = event.keystroke.modifiers;
        let direction = arrow(&Key::of(&event.keystroke));
        let Some((x, y)) = direction else {
            return false;
        };
        if modifiers.alt
            || modifiers.platform
            || modifiers.control
            || from_handle(handle, marked_key(Mark::ResizeHandle, event, window))
        {
            return false;
        }
        // An edge handle moves along its own axis only.
        if (x != 0.0 && matches!(handle, ResizeHandle::N | ResizeHandle::S))
            || (y != 0.0 && matches!(handle, ResizeHandle::E | ResizeHandle::W))
        {
            return false;
        }
        window.prevent_default();
        let step = self.step(this);
        let large_step = self.inner.options.borrow().large_step.clone();
        let by = if modifiers.shift {
            large_step.map_or(step * 10.0, |large_step| large_step(this))
        } else {
            step
        };
        self.resize_by(
            this,
            key,
            handle,
            x * by,
            y * by,
            ChangeOptions::event(Reason::ResizeKey, event),
            window,
            cx,
        );
        true
    }

    /// Starts a drag of a handle, or of the box with `Move`, once the pointer
    /// moves past [`DRAG_THRESHOLD`]. Shift on a corner keeps the box's
    /// ratio; Escape puts it back. The drop lands on the step's grid and
    /// reports `Drag`.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        key: &K,
        handle: ResizeHandle,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        // Handles sit inside their box, so the box hears their presses too.
        if handle != ResizeHandle::Move {
            mark_press(Mark::ResizeHandle, event, window, cx);
        }
        let Some(from) = self.boxes(this).get(key).copied() else {
            return;
        };
        let drags = self.inner.drags.clone();
        if !drags.accepts(this, event, window, cx)
            || self.is_disabled(this, key)
            || from_handle(handle, marked_press(Mark::ResizeHandle, event))
        {
            return;
        }
        // Offsets count from the press, so the edge keeps up with the pointer past the threshold.
        let start = event.position;
        let weak = Rc::downgrade(&self.inner);
        let (started, moved, ended) = (weak.clone(), weak.clone(), weak);
        let (start_key, move_key, end_key) = (key.clone(), key.clone(), key.clone());
        drags.start(
            event,
            BindDragOptions::new()
                .threshold(DRAG_THRESHOLD)
                .cancel_on_escape(true)
                .on_start(move |_this: &mut T, _event, _window, cx| {
                    if let Some(resize) = Self::upgrade(&started) {
                        *resize.inner.draft.borrow_mut() = Some(ResizeDraft {
                            key: start_key.clone(),
                            handle,
                            r#box: from,
                        });
                        cx.notify();
                    }
                    true
                })
                .on_move(move |this: &mut T, sample, _window, cx| {
                    let Some(resize) = Self::upgrade(&moved) else {
                        return;
                    };
                    let scale = resize.inner.options.borrow().scale.clone();
                    let scale = scale.map_or(1.0, |scale| scale(this));
                    let dx = f64::from(f32::from(sample.x - start.x)) / scale;
                    let dy = f64::from(f32::from(sample.y - start.y)) / scale;
                    let shift = sample.event.modifiers.shift;
                    let r#box = resize.fit(this, &move_key, from, handle, dx, dy, shift);
                    *resize.inner.draft.borrow_mut() = Some(ResizeDraft {
                        key: move_key.clone(),
                        handle,
                        r#box,
                    });
                    cx.notify();
                })
                .on_end(move |this: &mut T, end, canceled, window, cx| {
                    let Some(resize) = Self::upgrade(&ended) else {
                        return;
                    };
                    let draft = resize.inner.draft.borrow_mut().take();
                    cx.notify();
                    if let Some(draft) = draft
                        && !canceled
                    {
                        let step = resize.step(this);
                        resize.resize_to(
                            this,
                            &end_key,
                            round(draft.r#box, step),
                            ChangeOptions::event_of(Reason::Drag, end.event.clone()),
                            handle,
                            window,
                            cx,
                        );
                    }
                }),
            window,
            cx,
        );
    }

    /// The handle being dragged on a box, if any.
    pub fn dragging(&self, key: &K) -> Option<ResizeHandle> {
        self.inner
            .draft
            .borrow()
            .as_ref()
            .filter(|draft| draft.key == *key)
            .map(|draft| draft.handle)
    }

    /// The draft while a pointer drags a box.
    pub fn draft(&self) -> Option<ResizeDraft<K>> {
        self.inner.draft.borrow().clone()
    }

    /// Places the box as it shows, as `r#box` reads it, absolutely, in
    /// pixels of its units (`boxStyle`).
    pub fn box_style(&self, this: &T, key: &K) -> Option<ResizeBoxStyle> {
        self.r#box(this, key).map(ResizeBoxStyle)
    }

    /// The dragged box, the original's `--resize-x`, `--resize-y`,
    /// `--resize-width` and `--resize-height` (`style`).
    pub fn style(&self, key: &K) -> Option<ResizeBox> {
        self.inner
            .draft
            .borrow()
            .as_ref()
            .filter(|draft| draft.key == *key)
            .map(|draft| draft.r#box)
    }

    /// A handle's attributes: which handle it is, its
    /// `data-resize-handle`, and its cursor (`handleAttrs`). A drag claims a
    /// touch as it starts, which is the original's `touch-action: none`.
    pub fn handle_attrs(&self, handle: ResizeHandle) -> ResizeHandleAttrs {
        ResizeHandleAttrs::new(handle)
    }

    /// The element that follows the pointer while a box is dragged. Render
    /// it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T, K> {
        self.inner.drags.listener()
    }

    /// Wires a box's presses, which drag it with `Move`.
    pub fn bind_box<E: InteractiveElement + ParentElement + Styled>(&self, key: K, el: E) -> E {
        self.bind_press(key, ResizeHandle::Move, el)
    }

    /// Wires a handle: its cursor and its presses, which drag its edges.
    pub fn bind_handle<E: InteractiveElement + ParentElement + Styled>(
        &self,
        key: K,
        handle: ResizeHandle,
        el: E,
    ) -> E {
        let el = self.handle_attrs(handle).apply(el);
        self.bind_press(key, handle, el)
    }

    fn bind_press<E: InteractiveElement + ParentElement + Styled>(
        &self,
        key: K,
        handle: ResizeHandle,
        el: E,
    ) -> E {
        let resize = self.clone();
        on_pointer_down(el, move |event, window, cx| {
            let Some(entity) = resize.inner.entity.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                resize.pointer_down(this, event, &key, handle, window, cx)
            });
        })
    }
}

/// Where a box sits as it shows (`boxStyle`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeBoxStyle(pub ResizeBox);

impl ResizeBoxStyle {
    /// Places the box absolutely: its `x` and `y` as `left` and `top`, and
    /// its size, in pixels. A zoomed canvas scales them itself.
    pub fn apply<E: Styled>(self, el: E) -> E {
        let ResizeBox {
            x,
            y,
            width,
            height,
        } = self.0;
        el.absolute()
            .left(px(x as f32))
            .top(px(y as f32))
            .w(px(width as f32))
            .h(px(height as f32))
    }
}

/// A handle's attributes (`handleAttrs`): which handle it is, the
/// original's `data-resize-handle`, and the cursor that shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeHandleAttrs {
    pub handle: ResizeHandle,
    pub cursor: CursorStyle,
}

impl ResizeHandleAttrs {
    pub fn new(handle: ResizeHandle) -> Self {
        Self {
            handle,
            cursor: resize_cursors(handle),
        }
    }

    /// Puts the cursor on the handle's element.
    pub fn apply<E: Styled>(self, el: E) -> E {
        el.cursor(self.cursor)
    }
}
