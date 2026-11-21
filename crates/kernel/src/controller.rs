//! Port of `foundation/kernel/controller.ts`.
//!
//! An app holds a component's entity and calls its methods, so a component
//! needs no controller for that. What a controller adds is a handle made
//! before any component, passed in its options, which a component attaches
//! to as it's made: calls before that, or once the component is gone, do
//! nothing, and development builds warn once for each action called before
//! it first attached. Swapping it for another in new options lets the old
//! one go, unless another component attached to it since. An alert dialog's
//! `confirm()` asks whether one is attached.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::panic::Location;
use std::rc::Rc;

use gpui::{App, Context, Entity, WeakEntity, Window};

use super::dev;

struct Inner<T> {
    current: Option<WeakEntity<T>>,
    has_attached: Cell<bool>,
    warned: RefCell<HashSet<&'static Location<'static>>>,
}

/// A handle created outside any component and passed in its options
/// (`createController`). Calls go to the component it's attached to.
pub struct Controller<T> {
    inner: Rc<RefCell<Inner<T>>>,
}

impl<T> Clone for Controller<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> PartialEq for Controller<T> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

impl<T: 'static> Default for Controller<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: 'static> Controller<T> {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                current: None,
                has_attached: Cell::new(false),
                warned: RefCell::new(HashSet::new()),
            })),
        }
    }

    /// Whether a component drives it now (`isControllerAttached`).
    pub fn is_attached(&self, cx: &App) -> bool {
        self.entity(cx).is_some()
    }

    /// The component attached to it, while it's alive.
    pub fn entity(&self, _cx: &App) -> Option<Entity<T>> {
        self.inner.borrow().current.as_ref()?.upgrade()
    }

    /// Calls an action on the attached component, like `open()`. Before it
    /// attaches, and once it's gone, it does nothing and returns `None`;
    /// development builds warn once per call site for a call before it first
    /// attached. Call it outside the component's own update.
    #[track_caller]
    pub fn update<R>(&self, cx: &mut App, call: impl FnOnce(&mut T, &mut Context<T>) -> R) -> Option<R> {
        let caller = Location::caller();
        match self.entity(cx) {
            Some(entity) => Some(entity.update(cx, call)),
            None => {
                self.unattached(caller);
                None
            }
        }
    }

    /// `update`, for an action that needs the window.
    #[track_caller]
    pub fn update_in<R>(
        &self,
        window: &mut Window,
        cx: &mut App,
        call: impl FnOnce(&mut T, &mut Window, &mut Context<T>) -> R,
    ) -> Option<R> {
        let caller = Location::caller();
        match self.entity(cx) {
            Some(entity) => Some(entity.update(cx, |this, cx| call(this, window, cx))),
            None => {
                self.unattached(caller);
                None
            }
        }
    }

    /// Reads the attached component, like `isOpen()`. Reads before it
    /// attaches render on purpose, so they never warn.
    pub fn read<R>(&self, cx: &App, read: impl FnOnce(&T, &App) -> R) -> Option<R> {
        let entity = self.entity(cx)?;
        Some(read(entity.read(cx), cx))
    }

    fn unattached(&self, caller: &'static Location<'static>) {
        if !dev::DEV {
            return;
        }
        let inner = self.inner.borrow();
        if inner.has_attached.get() || !inner.warned.borrow_mut().insert(caller) {
            return;
        }
        dev::warn(
            &format!("a controller call at {caller} came before its component attached and did nothing."),
            None,
        );
    }

    /// Hands the controller a component (`attachController`).
    pub fn attach(&self, entity: WeakEntity<T>) {
        let mut inner = self.inner.borrow_mut();
        inner.current = Some(entity);
        inner.has_attached.set(true);
    }

    /// Takes a component back from the controller, if it's still the one
    /// attached (`detachController`).
    pub fn detach(&self, entity: &WeakEntity<T>) {
        let mut inner = self.inner.borrow_mut();
        if inner
            .current
            .as_ref()
            .is_some_and(|current| current.entity_id() == entity.entity_id())
        {
            inner.current = None;
        }
    }
}

/// A component's side of its `controller` option (`connectController`): it
/// attaches as the component is made, swaps when new options carry another
/// controller, and lets go when the component drops, unless another
/// component attached since.
pub struct ConnectedController<T: 'static> {
    entity: WeakEntity<T>,
    current: Option<Controller<T>>,
}

impl<T: 'static> ConnectedController<T> {
    /// Attaches `controller`, if the options have one, to the component
    /// being made.
    pub fn new(controller: Option<Controller<T>>, cx: &Context<T>) -> Self {
        let entity = cx.weak_entity();
        if let Some(controller) = &controller {
            controller.attach(entity.clone());
        }
        Self {
            entity,
            current: controller,
        }
    }

    /// New options' controller. The same one leaves it be, so options handed
    /// over again never take it from a component that attached since.
    pub fn set(&mut self, controller: Option<Controller<T>>) {
        if self.current == controller {
            return;
        }
        if let Some(old) = &self.current {
            old.detach(&self.entity);
        }
        if let Some(new) = &controller {
            new.attach(self.entity.clone());
        }
        self.current = controller;
    }

    pub fn controller(&self) -> Option<&Controller<T>> {
        self.current.as_ref()
    }
}

impl<T: 'static> Drop for ConnectedController<T> {
    fn drop(&mut self) {
        if let Some(controller) = &self.current {
            controller.detach(&self.entity);
        }
    }
}
