//! Port of `foundation/focus/focus-recovery.ts`. Focus that would fall to
//! the window when the focused part leaves it, like an item inside a branch
//! that collapses, or a row that's deleted, goes to a part the component
//! picks instead, so a keyboard user keeps their place.
//!
//! The original hears `focusin` on each bound part and recovers when one
//! unbinds holding focus. In gpui, parts bind as they render, and once
//! each frame is drawn the recovery notes which part focus is inside. When
//! focus falls to the window, which gpui reports as it draws the first frame
//! without the focused element, a part that held it and stopped rendering
//! hands it to `fallback`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

use gpui::{AnyWindowHandle, App, Context, FocusHandle, Subscription, Window};

use crate::after_bind::after_bind;

/// A part that left the window while it held focus.
#[derive(Clone, Debug)]
pub struct LostFocus<K, P> {
    /// The part's key.
    pub key: K,
    /// The props of the part's last render.
    pub props: P,
    /// The focus handle its element tracked, which nothing renders now.
    pub handle: FocusHandle,
}

struct Part<K, P> {
    key: K,
    props: P,
    handle: FocusHandle,
}

impl<K: Clone, P: Clone> Part<K, P> {
    fn lost(&self) -> LostFocus<K, P> {
        LostFocus {
            key: self.key.clone(),
            props: self.props.clone(),
            handle: self.handle.clone(),
        }
    }
}

struct Inner<K, P> {
    window: AnyWindowHandle,
    open: bool,
    parts: Vec<Part<K, P>>,
    index: HashMap<K, usize>,
    // The bound part that held focus last, as of the last frame drawn,
    // until focus leaves it for good.
    holder: Option<LostFocus<K, P>>,
}

impl<K: Clone + Eq + Hash + 'static, P: Clone + 'static> Inner<K, P> {
    // Starts a render of the parts unless one is under way, and says
    // whether it did.
    fn open_pass(&mut self) -> bool {
        if self.open {
            return false;
        }
        self.open = true;
        self.parts.clear();
        self.index.clear();
        true
    }

    fn bound(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }

    // Notes the innermost part around focus in the frame just drawn.
    fn settle(&mut self, window: &Window, cx: &App) {
        if !self.open {
            return;
        }
        self.open = false;
        let Some(active) = window.focused(cx) else {
            // Focus fell to the window. From a part that's still here it let
            // go; from one that left, it's what recovery is for.
            if self
                .holder
                .as_ref()
                .is_some_and(|holder| self.bound(&holder.key))
            {
                self.holder = None;
            }
            return;
        };
        let around: Vec<&Part<K, P>> = self
            .parts
            .iter()
            .filter(|part| part.handle == active || part.handle.contains(&active, window))
            .collect();
        let innermost = around.iter().find(|part| {
            !around
                .iter()
                .any(|other| other.key != part.key && part.handle.contains(&other.handle, window))
        });
        self.holder = innermost.map(|part| part.lost());
    }

    // The part that held focus and left, once focus fell from it.
    fn take_lost(&mut self) -> Option<LostFocus<K, P>> {
        let holder = self.holder.as_ref()?;
        if self.bound(&holder.key) {
            return None;
        }
        self.holder.take()
    }
}

/// Keeps focus in a collection when the part that holds it leaves the
/// window. Make it once in the component's constructor and keep it in a
/// field; the part that holds the parts calls [`begin`](Self::begin) as it
/// renders, and each part [`bind`](Self::bind)s. When a bound part stops
/// rendering with focus inside it, `fallback` names the element to focus
/// instead, by its focus handle, as the frame without it is drawn. Nested
/// parts work: focus belongs to the innermost bound part around it.
pub struct FocusRecovery<K, P> {
    inner: Rc<RefCell<Inner<K, P>>>,
    _focus_lost: Subscription,
}

impl<K: Clone + Eq + Hash + 'static, P: Clone + 'static> FocusRecovery<K, P> {
    /// Recovery for a component's parts. `fallback` gets the component, the
    /// part that left, and the component's context, and returns the handle to
    /// focus instead, or `None` to let focus go. It isn't asked when
    /// something else took focus meanwhile, like a dialog the removal opened.
    pub fn new<T: 'static>(
        fallback: impl Fn(&mut T, &LostFocus<K, P>, &mut Window, &mut Context<T>) -> Option<FocusHandle>
        + 'static,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Self {
        let inner = Rc::new(RefCell::new(Inner {
            window: window.window_handle(),
            open: false,
            parts: Vec::new(),
            index: HashMap::new(),
            holder: None,
        }));
        let weak = Rc::downgrade(&inner);
        let focus_lost =
            ui_sdk_kernel::focus_events::on_focus_lost(window, cx, move |this, window, cx| {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let Some(lost) = inner.borrow_mut().take_lost() else {
                    return;
                };
                if let Some(now) = window.focused(cx)
                    && now != lost.handle
                    && now.is_rendered(window)
                {
                    return;
                }
                if let Some(target) = fallback(this, &lost, window, cx)
                    && target.is_rendered(window)
                {
                    window.focus(&target, cx);
                }
            });
        Self {
            inner,
            _focus_lost: focus_lost,
        }
    }

    /// Starts a render of the parts, from the part that holds them, every
    /// frame it renders. Binding a part starts one too.
    pub fn begin(&self, cx: &App) {
        self.open(cx);
    }

    // Opens a render of the parts, and asks to note where focus is once its
    // frame is drawn.
    fn open(&self, cx: &App) {
        let window = {
            let mut inner = self.inner.borrow_mut();
            if !inner.open_pass() {
                return;
            }
            inner.window
        };
        let inner = Rc::downgrade(&self.inner);
        after_bind(window, cx, move |window, cx| {
            if let Some(inner) = inner.upgrade()
                && let Ok(mut inner) = inner.try_borrow_mut()
            {
                inner.settle(window, cx);
            }
        });
    }

    /// Tracks a part as it renders, with its props and the focus handle its
    /// element tracks. A part that stops rendering is no longer tracked.
    pub fn bind(&self, key: K, props: P, handle: &FocusHandle, cx: &App) {
        self.open(cx);
        let mut inner = self.inner.borrow_mut();
        let part = Part {
            key: key.clone(),
            props,
            handle: handle.clone(),
        };
        match inner.index.get(&key).copied() {
            Some(index) => inner.parts[index] = part,
            None => {
                let index = inner.parts.len();
                inner.index.insert(key, index);
                inner.parts.push(part);
            }
        }
    }
}
