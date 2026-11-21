//! Port of `foundation/direct-manipulation/preview.ts`: trying an option
//! before choosing it: a theme, a font, a layout, a generated variant. The
//! preview is its own state beside the value, so the page renders
//! `preview.or(value)` and choosing is a separate step. A pointer can press
//! an option and slide across the others, each previewing in turn like a
//! scrub; letting go on one chooses it, and off them puts the page back.
//! Arrows preview from the keyboard, Enter keeps, Escape puts back.
//!
//! The options join as they render ([`Preview::bind_option`]), which
//! records where they lay out, where the original reads its slot's parts
//! from the page. The preview lives in the behavior, as the original's
//! state key does in its store, unless the component keeps it (`current`).

use std::cell::{Cell, RefCell};
use std::fmt::Display;
use std::hash::Hash;
use std::rc::{Rc, Weak};

use gpui::{
    App, Bounds, Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels,
    Point, SharedString, Styled, WeakEntity, Window,
};
use ui_sdk_drag_and_drop::{
    BindDragOptions, DragSession, KeyFn, MessageFn, PointerEvent, ReadFn, on_pointer_down,
};
use ui_sdk_kernel::{ChangeOptions, InputEvent, Key, Reason};
use ui_sdk_platform::announce;

use crate::target::Bound;

/// Why the preview changed: a pointer over an option (`ItemHover`), the
/// keyboard (`ListNavigation`), Escape (`EscapeKey`), a slide that let go
/// off the options or focus leaving (`FocusOut`), choosing (`ItemPress`), or
/// a call from code (`Imperative`).
pub type PreviewReason = Reason;

/// Asks the component for a new preview, with why, and returns whether it
/// went through. It runs inside the component's update, with the component.
pub type SetPreviewFn<T, K> =
    Rc<dyn Fn(&mut T, Option<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool>;

/// Chooses an option, with why, and returns whether it went through.
pub type ChooseFn<T, K> =
    Rc<dyn Fn(&mut T, K, ChangeOptions, &mut Window, &mut Context<T>) -> bool>;

/// How [`create_preview`] keeps the preview and chooses (`PreviewOptions`).
/// `T` is the component, `K` an option's key.
pub struct PreviewOptions<T: 'static, K> {
    /// Reads the previewed key, when the component keeps it, like in a
    /// value with an `on_preview_change` that `set` changes. Without it, the
    /// preview keeps its own (the original's state key).
    pub current: Option<ReadFn<T, Option<K>>>,
    /// Asks for a new preview. Defaults to setting the preview's own; pass
    /// the component's change to fire its callback. With `current`, pass it
    /// too, since nothing else writes the component's preview.
    pub set: Option<SetPreviewFn<T, K>>,
    /// Chooses an option. Pass the component's change for its value;
    /// `details.reason` is `ItemPress`.
    pub choose: ChooseFn<T, K>,
    /// The chosen option now, which previews as nothing.
    pub value: Option<ReadFn<T, Option<K>>>,
    /// Whether an option can't be previewed or chosen.
    pub disabled: Option<KeyFn<T, K, bool>>,
    /// An option's name for screen readers. Defaults to its key.
    pub label: Option<KeyFn<T, K, SharedString>>,
    /// What screen readers hear when the keyboard previews one, from its
    /// label. Defaults to English.
    pub message: Option<MessageFn<SharedString>>,
}

impl<T: 'static, K> Clone for PreviewOptions<T, K> {
    fn clone(&self) -> Self {
        Self {
            current: self.current.clone(),
            set: self.set.clone(),
            choose: self.choose.clone(),
            value: self.value.clone(),
            disabled: self.disabled.clone(),
            label: self.label.clone(),
            message: self.message.clone(),
        }
    }
}

impl<T: 'static, K: 'static> PreviewOptions<T, K> {
    /// Options with how to choose an option.
    pub fn new(
        choose: impl Fn(&mut T, K, ChangeOptions, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        Self {
            current: None,
            set: None,
            choose: Rc::new(choose),
            value: None,
            disabled: None,
            label: None,
            message: None,
        }
    }

    pub fn current(mut self, current: impl Fn(&T) -> Option<K> + 'static) -> Self {
        self.current = Some(Rc::new(current));
        self
    }

    pub fn set(
        mut self,
        set: impl Fn(&mut T, Option<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        self.set = Some(Rc::new(set));
        self
    }

    pub fn value(mut self, value: impl Fn(&T) -> Option<K> + 'static) -> Self {
        self.value = Some(Rc::new(value));
        self
    }

    pub fn disabled(mut self, disabled: impl Fn(&T, &K) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(disabled));
        self
    }

    pub fn label(mut self, label: impl Fn(&T, &K) -> SharedString + 'static) -> Self {
        self.label = Some(Rc::new(label));
        self
    }

    pub fn message(mut self, message: impl Fn(&SharedString) -> String + 'static) -> Self {
        self.message = Some(Rc::new(message));
        self
    }
}

struct PreviewInner<T: 'static, K> {
    options: RefCell<PreviewOptions<T, K>>,
    entity: WeakEntity<T>,
    drags: DragSession<T>,
    bound: Bound<K>,
    preview: RefCell<Option<K>>,
    sliding: Cell<bool>,
}

/// Lets users preview options before choosing one (`createPreview`'s
/// return). Cloning it gives another handle to the same preview.
pub struct Preview<T: 'static, K> {
    inner: Rc<PreviewInner<T, K>>,
}

impl<T: 'static, K> Clone for Preview<T, K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users preview a component's options before choosing one
/// (`createPreview`). Make it in the component's constructor, keep it in a
/// field, render its [`listener`](Preview::listener) on every frame, and
/// wire the option part:
///
/// ```ignore
/// let option = preview.bind_option(key.clone(), part, cx)
///     .capture_key_down(move |event, window, cx| {
///         entity.update(cx, |this, cx| {
///             if this.preview.clone().keydown(this, event, &key, window, cx) {
///                 return;
///             }
///             if let Some(moved) = this.roving.keydown(event, &key, window, cx) {
///                 this.preview.clone().show(this, Some(moved.key), ChangeOptions::event(Reason::ListNavigation, event), window, cx);
///             }
///         }).ok();
///     });
/// ```
///
/// Call [`show`](Preview::show) as roving focus moves for arrow previews, and
/// [`clear`](Preview::clear) with `FocusOut` when focus leaves the group,
/// unless [`dragging`](Preview::dragging). Wire its keys in the capture
/// phase: gpui clicks a focused option as Enter or Space comes up unless
/// the key went down prevented, as `preventDefault` keeps a browser from
/// clicking.
pub fn create_preview<T: 'static, K: Clone + Eq + Hash + Display + 'static>(
    options: PreviewOptions<T, K>,
    cx: &Context<T>,
) -> Preview<T, K> {
    Preview {
        inner: Rc::new(PreviewInner {
            options: RefCell::new(options),
            entity: cx.weak_entity(),
            drags: DragSession::new(cx),
            bound: Bound::new(),
            preview: RefCell::new(None),
            sliding: Cell::new(false),
        }),
    }
}

impl<T: 'static, K: Clone + Eq + Hash + Display + 'static> Preview<T, K> {
    fn upgrade(inner: &Weak<PreviewInner<T, K>>) -> Option<Self> {
        inner.upgrade().map(|inner| Self { inner })
    }

    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: PreviewOptions<T, K>) {
        *self.inner.options.borrow_mut() = options;
    }

    /// The previewed option now, if any.
    pub fn current(&self, this: &T) -> Option<K> {
        let current = self.inner.options.borrow().current.clone();
        match current {
            Some(current) => current(this),
            None => self.inner.preview.borrow().clone(),
        }
    }

    fn value(&self, this: &T) -> Option<K> {
        let value = self.inner.options.borrow().value.clone();
        value.and_then(|value| value(this))
    }

    fn is_disabled(&self, this: &T, key: &K) -> bool {
        let disabled = self.inner.options.borrow().disabled.clone();
        disabled.is_some_and(|disabled| disabled(this, key))
    }

    fn label(&self, this: &T, key: &K) -> SharedString {
        let label = self.inner.options.borrow().label.clone();
        match label {
            Some(label) => label(this, key),
            None => key.to_string().into(),
        }
    }

    fn set(
        &self,
        this: &mut T,
        value: Option<K>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let (set, kept) = {
            let preview = self.inner.options.borrow();
            (preview.set.clone(), preview.current.is_some())
        };
        let done = match set {
            Some(set) => set(this, value.clone(), options, window, cx),
            None => true,
        };
        if done && !kept {
            *self.inner.preview.borrow_mut() = value;
        }
        if done {
            cx.notify();
        }
        done
    }

    /// Shows an option without choosing it. The chosen option and disabled
    /// ones show as nothing. A keyboard preview (`ListNavigation`) tells
    /// screen readers. Returns whether it changed.
    pub fn show(
        &self,
        this: &mut T,
        key: Option<K>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let chosen = self.value(this);
        let next = key.filter(|key| !self.is_disabled(this, key) && chosen.as_ref() != Some(key));
        if next == self.current(this) {
            return false;
        }
        let navigating = options.reason == Some(Reason::ListNavigation);
        let done = self.set(this, next.clone(), options, window, cx);
        if done
            && navigating
            && let Some(next) = next
        {
            let label = self.label(this, &next);
            let message = self.inner.options.borrow().message.clone();
            let text = match message {
                Some(message) => message(&label),
                None => format!("Previewing {label}. Enter to keep it."),
            };
            announce(&text, window, cx);
        }
        done
    }

    /// Puts the page back. Returns whether there was a preview.
    pub fn clear(
        &self,
        this: &mut T,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        self.current(this).is_some() && self.set(this, None, options, window, cx)
    }

    /// Chooses the previewed option, or `key`, and ends the preview. Reports
    /// `ItemPress`. Returns whether the choice went through.
    pub fn choose(
        &self,
        this: &mut T,
        key: Option<K>,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let Some(target) = key.or_else(|| self.current(this)) else {
            return false;
        };
        if self.is_disabled(this, &target) {
            return false;
        }
        let details = || ChangeOptions {
            reason: Some(Reason::ItemPress),
            source: None,
            event: event.clone(),
        };
        let choose = self.inner.options.borrow().choose.clone();
        let done = choose(this, target, details(), window, cx);
        self.clear(this, details(), window, cx);
        done
    }

    /// Whether a pointer is pressed and sliding across the options.
    pub fn dragging(&self) -> bool {
        self.inner.sliding.get()
    }

    // The option under a point, by the boxes read on the press.
    fn under(boxes: &[(K, Bounds<Pixels>)], at: Point<Pixels>) -> Option<K> {
        boxes
            .iter()
            .find(|(_, bounds)| {
                at.x >= bounds.left()
                    && at.x <= bounds.right()
                    && at.y >= bounds.top()
                    && at.y <= bounds.bottom()
            })
            .map(|(key, _)| key.clone())
    }

    /// Starts a press and slide from an option: each option the pointer
    /// slides over previews, letting go on one chooses it, and letting go
    /// off them, or Escape, puts the page back.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drags = self.inner.drags.clone();
        if !drags.accepts(this, event, window, cx) {
            return;
        }
        window.prevent_default();
        // Boxes are read once, on press, so sliding measures nothing.
        let boxes: Rc<Vec<(K, Bounds<Pixels>)>> = Rc::new(
            self.inner
                .bound
                .keys()
                .into_iter()
                .filter_map(|key| {
                    let bounds = self.inner.bound.region(&key)?.bounds()?;
                    Some((key, bounds))
                })
                .collect(),
        );
        self.inner.sliding.set(true);
        self.show(
            this,
            Some(key.clone()),
            ChangeOptions::event_of(Reason::ItemHover, event.event.clone()),
            window,
            cx,
        );
        let weak = Rc::downgrade(&self.inner);
        let (moved, ended) = (weak.clone(), weak);
        let over = boxes.clone();
        drags.start(
            event,
            BindDragOptions::new()
                .cancel_on_escape(true)
                .on_move(move |this: &mut T, sample, window, cx| {
                    let Some(preview) = Self::upgrade(&moved) else {
                        return;
                    };
                    let next = Self::under(&over, sample.position());
                    if next != preview.current(this) {
                        let options =
                            ChangeOptions::event_of(Reason::ItemHover, sample.event.event.clone());
                        preview.show(this, next, options, window, cx);
                    }
                })
                .on_end(move |this: &mut T, end, canceled, window, cx| {
                    let Some(preview) = Self::upgrade(&ended) else {
                        return;
                    };
                    preview.inner.sliding.set(false);
                    let landed = if canceled {
                        None
                    } else {
                        Self::under(&boxes, end.position)
                    };
                    match landed {
                        Some(landed) if Some(&landed) != preview.value(this).as_ref() => {
                            preview.choose(this, Some(landed), Some(end.event.clone()), window, cx);
                        }
                        landed => {
                            let reason = if landed.is_some() {
                                Reason::ItemPress
                            } else {
                                Reason::FocusOut
                            };
                            preview.clear(
                                this,
                                ChangeOptions::event_of(reason, end.event.clone()),
                                window,
                                cx,
                            );
                        }
                    }
                    cx.notify();
                }),
            window,
            cx,
        );
    }

    /// Enter or Space keeps the preview, or chooses `key`; Escape puts the
    /// page back. Returns whether it handled the key, which calls
    /// `window.prevent_default()`.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let pressed = Key::of(&event.keystroke);
        if pressed == Key::Enter || pressed == Key::Space {
            window.prevent_default();
            let target = self.current(this).unwrap_or_else(|| key.clone());
            self.choose(
                this,
                Some(target),
                Some(InputEvent::from(event)),
                window,
                cx,
            );
            return true;
        }
        if pressed == Key::Escape && self.current(this).is_some() {
            window.prevent_default();
            self.clear(
                this,
                ChangeOptions::event(Reason::EscapeKey, event),
                window,
                cx,
            );
            return true;
        }
        false
    }

    /// Joins an option as it renders, so a slide finds it under the pointer,
    /// and wires its presses ([`pointer_down`](Self::pointer_down)).
    pub fn bind_option<E: InteractiveElement + ParentElement + Styled>(
        &self,
        key: K,
        el: E,
        cx: &App,
    ) -> E {
        let el = self.inner.bound.bind(key.clone(), None, el, cx);
        let preview = self.clone();
        on_pointer_down(el, move |event, window, cx| {
            let Some(entity) = preview.inner.entity.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                preview.pointer_down(this, event, &key, window, cx)
            });
        })
    }

    /// Starts a render of the options, from the part that holds them, every
    /// frame it renders. Binding one starts one too; this is what tells a
    /// frame that renders none that they've left.
    pub fn begin(&self, cx: &App) {
        self.inner.bound.begin(cx);
    }

    /// The element that follows the pointer while it slides across the
    /// options. Render it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T, K> {
        self.inner.drags.listener()
    }
}
