//! Port of `foundation/motion/panel.ts`.
//!
//! A panel that shows and hides with its own enter and exit animations, size
//! and focus handling: Collapsible's and Accordion's panels, and any panel a
//! custom primitive shows, like a tree's group.
//!
//! The original binds a panel element: it writes `data-starting-style` and
//! `data-ending-style`, size variables CSS animates the height with, and
//! `hidden="until-found"`, and waits for the panel's animations. Here a
//! [`Panel`] keeps that state for one panel and its owner calls it as its
//! state changes, where the original subscribes to the owner's store: `bind`
//! as the panel enters the page, `check` after each change, `unbind` as it
//! leaves, or `sync`, which picks. The panel part takes [`Panel::track`],
//! which measures its content and knows when focus is inside it, and styles
//! itself from [`PanelState`]: a height and width that animate between 0 and
//! the content's own size, which the original's CSS computes from the
//! variables, and the panel's presence.
//!
//! Find in page has no gpui meaning, so `hidden="until-found"`, a findable
//! panel and its `reveal` are dropped: a closed panel is hidden like any
//! other.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    App, Context, FocusHandle, Pixels, ScrollHandle, Size, StatefulInteractiveElement, Window, px,
};

use crate::presence::{Presence, PresenceState, PresenceTiming, Started, Transitioned, erase};

/// Whether a panel is hidden: unless it's shown, open or animating out.
/// `findable` would keep a closed panel findable with `hidden="until-found"`;
/// with no find in page it changes nothing, so a findable panel is hidden like
/// any other.
pub fn hidden_attrs(shown: bool, _findable: bool) -> bool {
    !shown
}

/// Whether find in page can reveal `hidden="until-found"` content. gpui has
/// no find in page, so never.
pub fn supports_hidden_until_found() -> bool {
    false
}

/// How a panel opens: as it mounts with the page, at once, or animated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Opening {
    /// Open on the first paint: nothing animates.
    Mount,
    /// At once, with no animation, then `settled`, like the original's find in
    /// page match.
    Instant,
    /// With its enter animation.
    #[default]
    Animate,
}

impl Opening {
    /// The original's name for it, like `"instant"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Opening::Mount => "mount",
            Opening::Instant => "instant",
            Opening::Animate => "animate",
        }
    }
}

/// `settled`: the open or close animation finished, or there was none, with
/// whether the panel is open.
pub type PanelSettledCallback<T> = Rc<dyn Fn(&mut T, bool, &mut Window, &mut Context<T>)>;

/// `exited`: the exit animation finished.
pub type PanelExitedCallback<T> = Rc<dyn Fn(&mut T, &mut Window, &mut Context<T>)>;

/// What a panel asks of the primitive that owns it, `T`, as that primitive's
/// state stands now. Callbacks run inside the owner's update with its state.
pub struct PanelBinding<T> {
    /// Whether the panel is open now.
    pub open: bool,
    /// Whether a closed panel is still shown while it animates out. Keep the
    /// panel present while it is.
    pub exiting: bool,
    /// How the panel opens: as it mounts with the page, at once, or animated.
    pub opening: Opening,
    /// Where focus goes when the panel closes with focus inside it, like its
    /// trigger.
    pub trigger: Option<FocusHandle>,
    /// The open or close animation finished, or there was none. Never on
    /// mount.
    pub settled: Option<PanelSettledCallback<T>>,
    /// The exit animation finished, so the panel can leave the page: stop
    /// counting it as exiting here.
    pub exited: Option<PanelExitedCallback<T>>,
}

impl<T> Default for PanelBinding<T> {
    fn default() -> Self {
        Self {
            open: false,
            exiting: false,
            opening: Opening::default(),
            trigger: None,
            settled: None,
            exited: None,
        }
    }
}

impl<T> Clone for PanelBinding<T> {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            exiting: self.exiting,
            opening: self.opening,
            trigger: self.trigger.clone(),
            settled: self.settled.clone(),
            exited: self.exited.clone(),
        }
    }
}

impl<T> PanelBinding<T> {
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn exiting(mut self, exiting: bool) -> Self {
        self.exiting = exiting;
        self
    }

    pub fn opening(mut self, opening: Opening) -> Self {
        self.opening = opening;
        self
    }

    pub fn trigger(mut self, trigger: Option<FocusHandle>) -> Self {
        self.trigger = trigger;
        self
    }

    pub fn settled(
        mut self,
        callback: impl Fn(&mut T, bool, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        self.settled = Some(Rc::new(callback));
        self
    }

    pub fn exited(mut self, callback: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static) -> Self {
        self.exited = Some(Rc::new(callback));
        self
    }
}

/// What a panel part styles from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelState {
    /// The panel's presence: its phase, the starting and ending styles,
    /// `inert` and how far it shows.
    pub presence: PresenceState,
    /// The height to give the panel now: `None` for its own (`auto`), what it
    /// animates through while it opens or closes, 0 while closed. Clip the
    /// panel's overflow while it animates.
    pub height: Option<Pixels>,
    /// The width, the same way, for panels that open sideways.
    pub width: Option<Pixels>,
}

const HEIGHT: usize = 0;
const WIDTH: usize = 1;

/// One panel's binding: its presence, the size it animates through, and what
/// it measures. Cheap to clone: clones share it.
#[derive(Clone)]
pub struct Panel {
    shared: Rc<Shared>,
}

struct Shared {
    presence: Presence,
    inner: RefCell<PanelInner>,
}

struct PanelInner {
    bound: bool,
    /// Whether the panel was open when it last looked.
    open: bool,
    /// What the original writes to the size variables: `None` for `auto`,
    /// else px.
    size: Option<Size<f32>>,
    focus: FocusHandle,
    scroll: ScrollHandle,
}

impl Panel {
    pub fn new(cx: &mut App) -> Self {
        let presence = Presence::new(false);
        presence.inner_mut().properties = vec![Transitioned::new(0.0); 2];
        Self {
            shared: Rc::new(Shared {
                presence,
                inner: RefCell::new(PanelInner {
                    bound: false,
                    open: false,
                    size: Some(Size::default()),
                    focus: cx.focus_handle(),
                    scroll: ScrollHandle::new(),
                }),
            }),
        }
    }

    pub fn with_timing(self, timing: PresenceTiming) -> Self {
        self.set_timing(timing);
        self
    }

    /// Sets the timing the panel's transitions read as they start, as its
    /// CSS would declare them.
    pub fn set_timing(&self, timing: PresenceTiming) {
        self.shared.presence.set_timing(timing);
    }

    /// The panel's presence.
    pub fn presence(&self) -> &Presence {
        &self.shared.presence
    }

    /// What the panel part styles from, as of the frame being drawn.
    pub fn state(&self) -> PanelState {
        let presence = self.shared.presence.state();
        let size = self.shared.inner.borrow().size;
        let (height, width) = match size {
            None => (None, None),
            Some(_) => {
                let inner = self.shared.presence.inner();
                let now = inner.frame_time;
                let value = |index: usize| inner.properties[index].value(now);
                (Some(px(value(HEIGHT))), Some(px(value(WIDTH))))
            }
        };
        PanelState {
            presence,
            height,
            width,
        }
    }

    /// The focus the panel part tracks, which `track` puts on it.
    pub fn focus_handle(&self) -> FocusHandle {
        self.shared.inner.borrow().focus.clone()
    }

    /// What measures the panel's content, which `track` puts on it.
    pub fn scroll_handle(&self) -> ScrollHandle {
        self.shared.inner.borrow().scroll.clone()
    }

    /// Puts what the panel needs on its part: a scroll handle that measures
    /// its content, the size an opening animates to (`scrollHeight` in the
    /// original), and a focus handle out of the tab order that tells when
    /// focus is inside it.
    pub fn track<E: StatefulInteractiveElement>(&self, el: E) -> E {
        let inner = self.shared.inner.borrow();
        el.track_focus(&inner.focus).track_scroll(&inner.scroll)
    }

    /// Whether the panel is in the page, between `bind` and `unbind`.
    pub fn is_bound(&self) -> bool {
        self.shared.inner.borrow().bound
    }

    /// Binds the panel as it enters the page (`bindPanel`): open, it opens as
    /// `binding.opening` says; closed, its size is 0.
    pub fn bind<T: 'static>(
        &self,
        owner: &mut T,
        binding: &PanelBinding<T>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        {
            let mut inner = self.shared.inner.borrow_mut();
            inner.bound = true;
            inner.open = binding.open;
        }
        // A panel entering the page starts as a new element does, with
        // nothing on it.
        self.shared.presence.set_open(false, cx);
        self.shared.size(Some(Size::default()));
        if binding.open {
            self.opening(owner, binding, window, cx);
        }
    }

    /// Hears a change to the owner's state, which the original subscribes
    /// to: the panel opens or closes when `binding.open` changed. Call it
    /// after each change while the panel is bound.
    pub fn check<T: 'static>(
        &self,
        owner: &mut T,
        binding: &PanelBinding<T>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        {
            let mut inner = self.shared.inner.borrow_mut();
            if !inner.bound || binding.open == inner.open {
                return;
            }
            inner.open = binding.open;
        }
        if binding.open {
            self.opening(owner, binding, window, cx);
        } else {
            self.closing(owner, binding, window, cx);
        }
    }

    /// Unbinds the panel as it leaves the page, `bindPanel`'s cleanup: what
    /// runs stops, and it rests closed until it binds again, as the element
    /// it stands for is gone.
    pub fn unbind<T: 'static>(&self, cx: &mut Context<T>) {
        let was_bound = std::mem::replace(&mut self.shared.inner.borrow_mut().bound, false);
        if was_bound {
            self.shared.presence.set_open(false, cx);
        }
    }

    /// `bind`, `check` and `unbind` in one, from whether the panel is
    /// `present`, in the page, now: call it after every change to the owner's
    /// state or the panel's options. A panel leaving hears the change first.
    pub fn sync<T: 'static>(
        &self,
        owner: &mut T,
        present: bool,
        binding: &PanelBinding<T>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if self.is_bound() {
            self.check(owner, binding, window, cx);
            if !present {
                self.unbind(cx);
            }
        } else if present {
            self.bind(owner, binding, window, cx);
        }
    }

    fn opening<T: 'static>(
        &self,
        owner: &mut T,
        binding: &PanelBinding<T>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let presence = &self.shared.presence;
        presence.stop(cx);
        if binding.opening != Opening::Animate {
            // Nothing animates on the first paint or for an opening at once.
            presence.set_open(true, cx);
            self.shared.size(None);
            if binding.opening == Opening::Instant
                && let Some(settled) = binding.settled.clone()
            {
                settled(owner, true, window, cx);
            }
            return;
        }
        let at_rest = !presence.inner().transitioning(Instant::now());
        // The starting frame lays the panel out at its starting size, which
        // measures its content: the size the opening animates to.
        let shared = Rc::downgrade(&self.shared);
        let started: Started = Box::new(move |inner, now, reduce| {
            let Some(shared) = shared.upgrade() else {
                return;
            };
            let content = shared.measure();
            shared.inner.borrow_mut().size = Some(content);
            let timing = inner.timing.enter.clone();
            inner.retarget_property(HEIGHT, content.height, &timing, reduce, now);
            inner.retarget_property(WIDTH, content.width, &timing, reduce, now);
        });
        let shared = Rc::downgrade(&self.shared);
        let on_settled = binding.settled.clone();
        let done = erase(
            move |owner: &mut T, window: &mut Window, cx: &mut Context<T>| {
                if let Some(shared) = shared.upgrade() {
                    shared.size(None);
                }
                if let Some(settled) = on_settled {
                    settled(owner, true, window, cx);
                }
            },
            cx,
        );
        presence.enter(Some(done), Some(started), window, cx);
        // The starting style's size, at once when the panel isn't moving.
        if at_rest {
            let mut inner = presence.inner_mut();
            for property in &mut inner.properties {
                property.jump(0.0);
            }
        }
    }

    fn closing<T: 'static>(
        &self,
        owner: &mut T,
        binding: &PanelBinding<T>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let presence = &self.shared.presence;
        presence.stop(cx);
        // Focus inside a closing panel would drop to the page. Its trigger is
        // the closest place.
        let focus = self.focus_handle();
        if focus.contains_focused(window, cx)
            && let Some(trigger) = &binding.trigger
        {
            window.focus(trigger, cx);
        }
        if !binding.exiting {
            self.shared.size(Some(Size::default()));
            presence.set_open(false, cx);
            if let Some(settled) = binding.settled.clone() {
                settled(owner, false, window, cx);
            }
            return;
        }
        // From the size it has now, so an interrupted opening animates back
        // from where it is, then to the ending style's 0.
        let now = Instant::now();
        let current = self.shared.current_size(now);
        self.shared.size(Some(current));
        {
            let reduce = cx.reduce_motion();
            let mut inner = presence.inner_mut();
            let timing = inner.timing.exit.clone();
            inner.retarget_property(HEIGHT, 0.0, &timing, reduce, now);
            inner.retarget_property(WIDTH, 0.0, &timing, reduce, now);
        }
        let shared = Rc::downgrade(&self.shared);
        let on_exited = binding.exited.clone();
        let on_settled = binding.settled.clone();
        presence.animate_out(
            owner,
            move |owner, window, cx| {
                if let Some(shared) = shared.upgrade() {
                    shared.size(Some(Size::default()));
                }
                if let Some(exited) = on_exited {
                    exited(owner, window, cx);
                }
                if let Some(settled) = on_settled {
                    settled(owner, false, window, cx);
                }
            },
            window,
            cx,
        );
    }
}

impl Shared {
    // `size`: what the variables hold, `None` for `auto`. A new size stops a
    // size transition where it is, as writing the variable does.
    fn size(&self, size: Option<Size<f32>>) {
        self.inner.borrow_mut().size = size;
        let mut presence = self.presence.inner_mut();
        let now = Instant::now();
        let values = match size {
            Some(size) => [size.height, size.width],
            None => [
                presence.properties[HEIGHT].value(now),
                presence.properties[WIDTH].value(now),
            ],
        };
        for (property, value) in presence.properties.iter_mut().zip(values) {
            property.jump(value);
        }
    }

    // The content's own size: what the panel laid out at its last frame, and
    // what overflowed it.
    fn measure(&self) -> Size<f32> {
        let scroll = self.inner.borrow().scroll.clone();
        let bounds = scroll.bounds();
        let overflow = scroll.max_offset();
        Size {
            width: f32::from(bounds.size.width + overflow.x),
            height: f32::from(bounds.size.height + overflow.y),
        }
    }

    // The size the panel shows now, what the original's computed style reads.
    fn current_size(&self, now: Instant) -> Size<f32> {
        if self.inner.borrow().size.is_none() {
            let bounds = self.inner.borrow().scroll.bounds();
            return Size {
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
            };
        }
        let presence = self.presence.inner();
        Size {
            width: presence.properties[WIDTH].value(now),
            height: presence.properties[HEIGHT].value(now),
        }
    }
}

impl std::fmt::Debug for Panel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Panel").field(&self.state()).finish()
    }
}
