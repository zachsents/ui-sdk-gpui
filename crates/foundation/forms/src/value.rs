//! Port of `foundation/forms/value.ts`. A text field's value changes in ways
//! no event reports: apps write a controlled field's value, clear a chat box
//! after sending, and a form reset puts every default back. Parts that show
//! whether a field is filled, or size a textarea to its text, hear all of them
//! through here.
//!
//! The original wraps a native input's `value` property to hear writes. Here
//! the text control is the native crate's text field, which knows nothing of
//! forms, so whoever writes its value tells the watchers with
//! [`notify_value`]; [`write_value`] and the text field helper do, and the
//! helper tells them of writes from outside it that its control reports.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, SharedString, Subscription, Window};

use crate::document::{FormEvent, FormEventType, Node, SCRIPT_WRITES};
use crate::form::dispatch_change;

/// What a text field helper drives: a single-line or multi-line text field,
/// like the native crate's. A component implements it over its field, which
/// the helper reads and writes outside the field's own updates.
pub trait TextControl {
    /// Its text.
    fn value(&self, cx: &App) -> SharedString;
    /// Replaces its text, as code writing `value` does: no `Input` fires.
    fn set_value(&self, value: SharedString, window: &mut Window, cx: &mut App);
    /// The caret's byte offset into the text.
    fn caret(&self, cx: &App) -> Option<usize>;
    /// Puts the caret at a byte offset into the text.
    fn set_caret(&self, caret: usize, window: &mut Window, cx: &mut App);
    /// Moves focus to it.
    fn focus(&self, window: &mut Window, cx: &mut App);
}

/// What changed the value: typing, a write from code, or a form reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueSource {
    Input,
    Write,
    Reset,
}

/// Runs `run` once a form reset has put every control back, unless the
/// reset was canceled. Call it from a listener of the form's `Reset`.
pub fn after_reset(event: &FormEvent, run: impl FnOnce(&mut Window, &mut App) + 'static) {
    if event.kind() == FormEventType::Reset {
        event.after.borrow_mut().push(Box::new(run));
    }
}

/// Runs `write` as a write from code, which fires no `Input` or `Change`,
/// like setting a native control's state from script.
pub fn write_from_script<R>(node: &Node, write: impl FnOnce() -> R) -> R {
    let key = node.key();
    let nested = SCRIPT_WRITES.with(|writes| writes.borrow().contains(&key));
    if nested {
        return write();
    }
    SCRIPT_WRITES.with(|writes| writes.borrow_mut().push(key));
    struct Done(u64);
    impl Drop for Done {
        fn drop(&mut self) {
            SCRIPT_WRITES
                .try_with(|writes| writes.borrow_mut().retain(|each| *each != self.0))
                .ok();
        }
    }
    let _done = Done(key);
    write()
}

/// Whether code is writing the node's state, or that of an element around it.
pub fn writing_from_script(node: &Node) -> bool {
    let writes: Vec<u64> = SCRIPT_WRITES.with(|writes| writes.borrow().clone());
    if writes.is_empty() {
        return false;
    }
    writes.contains(&node.key())
        || node
            .ancestors()
            .iter()
            .any(|ancestor| writes.contains(&ancestor.key()))
}

/// Writes a text control's value. From code by default, and only when it
/// differs: watchers hear a write and no event fires, like for a hidden input
/// that mirrors a component's value. `as_user` writes it the way typing
/// would, then `Input` goes out from `node`, so form listeners and fields
/// hear it.
pub fn write_value(
    control: &dyn TextControl,
    node: Option<&Node>,
    value: SharedString,
    as_user: bool,
    window: &mut Window,
    cx: &mut App,
) {
    if as_user && !node.is_some_and(writing_from_script) {
        control.set_value(value, window, cx);
        if let Some(node) = node {
            notify_value(node, ValueSource::Input, window, cx);
            dispatch_change(Some(node), &[FormEventType::Input], window, cx);
        }
    } else if control.value(cx) != value {
        control.set_value(value, window, cx);
        if let Some(node) = node {
            notify_value(node, ValueSource::Write, window, cx);
        }
    }
}

type ValueListener = Rc<dyn Fn(ValueSource, &mut Window, &mut App)>;

thread_local! {
    static WATCHERS: RefCell<Vec<(u64, u64, ValueListener)>> = const { RefCell::new(Vec::new()) };
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Calls `listener` whenever the text control of `node` may have changed its
/// value: typing, writes from code, and form resets. Dropping the
/// subscription stops it.
pub fn watch_value(
    node: &Node,
    listener: impl Fn(ValueSource, &mut Window, &mut App) + 'static,
) -> Subscription {
    let id = NEXT.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    let key = node.key();
    WATCHERS.with(|watchers| watchers.borrow_mut().push((id, key, Rc::new(listener))));
    Subscription::new(move || {
        WATCHERS
            .try_with(|watchers| watchers.borrow_mut().retain(|(each, _, _)| *each != id))
            .ok();
    })
}

/// Tells the watchers of `node` that its text control's value changed.
pub fn notify_value(node: &Node, source: ValueSource, window: &mut Window, cx: &mut App) {
    let key = node.key();
    let listeners: Vec<ValueListener> = WATCHERS.with(|watchers| {
        watchers
            .borrow()
            .iter()
            .filter(|(_, each, _)| *each == key)
            .map(|(_, _, listener)| listener.clone())
            .collect()
    });
    for listener in listeners {
        listener(source, window, cx);
    }
}

/// A form reset returns a control to its default. A controlled one with no
/// default keeps its value, like a controlled input in React, so a form
/// action that resets the form leaves the app's state alone.
pub fn keeps_value_on_reset<T>(value: Option<&T>, default_value: Option<&T>) -> bool {
    value.is_some() && default_value.is_none()
}
