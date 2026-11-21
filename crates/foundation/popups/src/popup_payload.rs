//! Port of `foundation/popups/popup-payload.ts`: the payload a popup's
//! triggers hand its content. Several triggers can share one popup, each
//! with a payload: one that opens the popup writes its key and payload, and
//! so does one that takes the open popup over. A controller's
//! `open(payload)` on the open popup replaces it the same way.
//! `on_payload_change` hears all three. A handover opens nothing, so that
//! callback is how code outside the popup hears about it.

use std::cell::RefCell;
use std::fmt::Debug;
use std::rc::Rc;

use gpui::{Context, SharedString, Window};
use ui_sdk_kernel::{ChangeOptions, Controllable, Instance};

/// Reads the component's instance, which its changes report with.
pub type InstanceOf<T> = fn(&T) -> &Instance;

struct Inner<T: 'static, P> {
    trigger: Rc<RefCell<Option<SharedString>>>,
    payload: Rc<RefCell<Controllable<Option<P>>>>,
    instance: InstanceOf<T>,
}

/// The payload and the trigger key that handed it over
/// (`createPopupPayload`). Clones share it.
pub struct PopupPayload<T: 'static, P> {
    inner: Rc<Inner<T, P>>,
}

impl<T: 'static, P> Clone for PopupPayload<T, P> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Puts back the trigger and payload an open that was refused had written.
pub struct PayloadRestore<P> {
    trigger: Rc<RefCell<Option<SharedString>>>,
    previous_trigger: Option<SharedString>,
    payload: Rc<RefCell<Controllable<Option<P>>>>,
    previous_payload: Option<P>,
}

impl<P: Clone + PartialEq + Debug + 'static> PayloadRestore<P> {
    pub fn restore(self) {
        *self.trigger.borrow_mut() = self.previous_trigger;
        self.payload.borrow_mut().set_held(self.previous_payload);
    }
}

/// A popup's payload over the trigger key and payload it keeps, changed
/// through `payload`, whose callback is `on_payload_change`.
pub fn create_popup_payload<T: 'static, P: Clone + PartialEq + Debug + 'static>(
    trigger: Rc<RefCell<Option<SharedString>>>,
    payload: Rc<RefCell<Controllable<Option<P>>>>,
    instance: InstanceOf<T>,
) -> PopupPayload<T, P> {
    PopupPayload {
        inner: Rc::new(Inner {
            trigger,
            payload,
            instance,
        }),
    }
}

impl<T: 'static, P: Clone + PartialEq + Debug + 'static> PopupPayload<T, P> {
    fn change(&self, this: &T, payload: Option<P>, options: ChangeOptions, window: &mut Window, cx: &mut Context<T>) -> bool {
        let instance = (self.inner.instance)(this);
        self.inner
            .payload
            .borrow_mut()
            .change(instance, payload, options, window, cx)
    }

    /// The payload of the last open.
    pub fn get(&self) -> Option<P> {
        self.inner.payload.borrow().get().clone()
    }

    /// The trigger that handed it over last, by key.
    pub fn trigger(&self) -> Option<SharedString> {
        self.inner.trigger.borrow().clone()
    }

    /// Call before an open fires `on_open_change`, so it reads the new
    /// payload. Returns what puts the trigger and payload back, for an open
    /// that was canceled.
    pub fn opening(
        &self,
        this: &T,
        trigger: Option<SharedString>,
        payload: Option<P>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> PayloadRestore<P> {
        let previous_trigger = self.inner.trigger.borrow().clone();
        let previous_payload = self.get();
        if let Some(trigger) = trigger {
            *self.inner.trigger.borrow_mut() = Some(trigger);
        }
        self.change(this, payload, options, window, cx);
        PayloadRestore {
            trigger: self.inner.trigger.clone(),
            previous_trigger,
            payload: self.inner.payload.clone(),
            previous_payload,
        }
    }

    /// Another trigger takes the open popup over, with its own payload
    /// (`handOver`).
    pub fn hand_over(
        &self,
        this: &T,
        trigger: SharedString,
        payload: Option<P>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        *self.inner.trigger.borrow_mut() = Some(trigger);
        self.change(this, payload, options, window, cx);
    }

    /// A controller opens the popup while it's open. A payload it passes
    /// replaces the current one, like a handover, and the trigger key stays,
    /// as when a controller opens it closed. Without a payload nothing
    /// changes. Returns whether the payload changed, so the caller can move
    /// its anchor too.
    pub fn reopening(
        &self,
        this: &T,
        payload: Option<Option<P>>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        match payload {
            Some(payload) => self.change(this, payload, options, window, cx),
            None => false,
        }
    }
}
