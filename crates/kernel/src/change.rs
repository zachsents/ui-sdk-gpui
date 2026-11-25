//! Port of `foundation/kernel/change.ts` and `input-source.ts`'s
//! `createChange`/`createCommit`.
//!
//! In the original a primitive's store holds every key and `createChange`
//! fires `on<Key>Change` from options. Here each changeable key is a
//! [`Controllable`] field on the component's entity: it holds the value, the
//! value the app controls it with, and the callback.
//!
//! Controlling a value: the app passes `Some(value)` with the component's
//! setter, typically from its own `render`, the way a framework re-renders
//! with new props. The callback runs inside the component's update, so it may
//! update any entity but the component itself; the app's own state, then a
//! re-render that passes the value back, is the usual shape.
//!
//! A key made with [`Controllable::after_update`], like every component's
//! `value`, runs its callbacks once the update that changed it is over
//! instead, so they may update any entity: the component, and whatever
//! called into it, like an app clearing a field it holds. Such a change is
//! refused before it happens, inside the update, by the callback
//! [`Controllable::set_on_before_change`] gives it.

use std::cell::{Cell, RefCell};
use std::fmt::Debug;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, SharedString, Window};

use super::details::{ChangeDetails, ChangeOptions, ChangeSource, InputEvent, Reason, resolve_source, with_source};
use super::dev::{self, Origin};
use super::keep;
use super::option_names::{CallbackKind, callback_name};
use super::primitive::Instance;
use super::watchers;

/// `on<Key>Change`: the requested value and what explains it.
pub type ChangeCallback<T> = Rc<dyn Fn(&T, &ChangeDetails, &mut Window, &mut App)>;

/// `on<Key>Commit`: the committed value and what explains it. Commits can't be canceled.
pub type CommitCallback<T> = Rc<dyn Fn(&T, &ChangeDetails, &mut Window, &mut App)>;

/// A state key an interaction changes and the app may control.
pub struct Controllable<T> {
    key: &'static str,
    held: T,
    controlled: Option<T>,
    on_change: Option<ChangeCallback<T>>,
    on_commit: Option<CommitCallback<T>>,
    // With `after_update`: `on_change` and `on_commit` run once the update
    // is over, and `on_before_change`, inside it, is what may cancel.
    after: bool,
    on_before_change: Option<ChangeCallback<T>>,
    // The value the key is changing to while its callback runs, so asking for
    // the same value from inside does nothing (`changing` in the original).
    changing: Option<T>,
    // A controlled change the callback accepted, with its source, until the
    // app passes a value back (`requested`).
    requested: Option<(T, ChangeSource)>,
    // The source of the change that asked for the value the app just passed.
    arrived: Option<ChangeSource>,
    // Development builds: the instance, for warnings, and a controlled change
    // waiting for its value.
    origin: Option<(SharedString, SharedString)>,
    expecting: Rc<RefCell<Option<Expecting<T>>>>,
    generation: Cell<u64>,
    warned_release: bool,
}

struct Expecting<T> {
    before: T,
    generation: u64,
}

impl<T: Clone + PartialEq + Debug + 'static> Controllable<T> {
    /// `defaultValue`: the value it starts with when uncontrolled.
    pub fn new(key: &'static str, default: T) -> Self {
        Self {
            key,
            held: default,
            controlled: None,
            on_change: None,
            on_commit: None,
            after: false,
            on_before_change: None,
            changing: None,
            requested: None,
            arrived: None,
            origin: None,
            expecting: Rc::new(RefCell::new(None)),
            generation: Cell::new(0),
            warned_release: false,
        }
    }

    /// Runs `on_change` and `on_commit` once the update that changed the key
    /// is over, rather than inside it, so they may update any entity, the
    /// component and whatever called into it too. They hear the change
    /// after it happened and can't refuse it: `set_on_before_change` gives
    /// the callback that can.
    pub fn after_update(mut self) -> Self {
        self.after = true;
        self
    }

    pub fn key(&self) -> &'static str {
        self.key
    }

    /// The value parts render: the controlled one when there is one.
    pub fn get(&self) -> &T {
        self.controlled.as_ref().unwrap_or(&self.held)
    }

    pub fn is_controlled(&self) -> bool {
        self.controlled.is_some()
    }

    /// Passes the controlled value, or `None` to hand the key back to the
    /// component (`sync`). Returns whether what renders changed.
    pub fn set_controlled(&mut self, value: Option<T>) -> bool {
        if dev::DEV {
            self.heard(value.as_ref());
            if self.controlled.is_some() && value.is_none() && !self.warned_release {
                self.warned_release = true;
                dev::warn(
                    &format!(
                        "stopped controlling `{}`: it was passed and is now `None`, so the component keeps its last value.",
                        self.key
                    ),
                    self.origin.as_ref().map(|(component, id)| Origin::new(component, id)),
                );
            }
        }
        if let Some(value) = &value
            && let Some((asked, source)) = self.requested.take()
            && asked == *value
        {
            self.arrived = Some(source);
        }
        let before = self.get().clone();
        if let Some(value) = &value {
            // A controlled key the app lets go of keeps its last value.
            self.held = value.clone();
        }
        self.controlled = value;
        before != *self.get()
    }

    /// `set_controlled`, and the source of the change that asked for the
    /// value the app passes, so what the component does with it, like closing
    /// a submenu, reports the same source (`onOptions`' return). Wrap that
    /// work in `with_optional_source`.
    pub fn set_controlled_with_source(&mut self, value: Option<T>) -> (bool, Option<ChangeSource>) {
        self.arrived = None;
        let changed = self.set_controlled(value);
        (changed, self.arrived.take())
    }

    pub fn set_on_change(&mut self, callback: Option<ChangeCallback<T>>) {
        self.on_change = callback;
    }

    pub fn set_on_commit(&mut self, callback: Option<CommitCallback<T>>) {
        self.on_commit = callback;
    }

    /// For a key made with `after_update`: what runs as a change is asked
    /// for, before it happens, inside the component's update, where
    /// `details.cancel()` refuses it. It may update any entity but the
    /// component and what called into it.
    pub fn set_on_before_change(&mut self, callback: Option<ChangeCallback<T>>) {
        self.on_before_change = callback;
    }

    /// Writes the held value with no callback, for bookkeeping no one can
    /// refuse (`context.store.set`). Returns whether it changed.
    pub fn set_held(&mut self, value: T) -> bool {
        if self.held == value {
            return false;
        }
        self.held = value;
        !self.is_controlled()
    }

    /// Asks for `value` (`change(key, value, details)`): runs `on_change`,
    /// then writes it unless the key is controlled or the callback canceled.
    /// Returns whether the request went through: false when the value is
    /// already the same or the change was canceled. For a controlled key,
    /// true means accepted; the app still passes the value back. A key made
    /// with `after_update` runs `on_before_change` where `on_change` would
    /// run, and `on_change` once the update is over, for a change that went
    /// through.
    pub fn change(
        &mut self,
        instance: &Instance,
        value: T,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        if self.changing.as_ref() == Some(&value) {
            return false;
        }
        let previous = self.get().clone();
        if previous == value {
            return false;
        }
        let reason = options.reason.unwrap_or(Reason::Imperative);
        let source = options
            .source
            .unwrap_or_else(|| resolve_source(reason, options.event.as_ref()));
        let details = ChangeDetails::new(reason, source, options.event);
        if dev::DEV {
            self.origin = Some((instance.label().clone(), instance.id().clone()));
            dev::check_event(reason, details.event.as_ref(), Origin::new(instance.label(), instance.id()));
        }
        instance.debug_change(self.key, &value, &details);
        let controlled = self.is_controlled();
        let outer = self.changing.replace(value.clone());
        let inside = if self.after { &self.on_before_change } else { &self.on_change };
        with_source(details.source, || {
            if let Some(callback) = inside.clone() {
                callback(&value, &details, window, cx);
            }
        });
        self.changing = outer;
        if details.canceled() {
            watchers::emit_change_of(instance, self.key, &value, &previous, &details, true, controlled);
            return false;
        }
        if !controlled {
            self.held = value.clone();
        }
        watchers::emit_change_of(instance, self.key, &value, &previous, &details, false, controlled);
        if self.after
            && let Some(callback) = self.on_change.clone()
        {
            self.run_after(instance, callback, value.clone(), details.clone(), true, window, cx);
        }
        if controlled {
            self.requested = Some((value.clone(), details.source));
            if dev::DEV {
                self.expect(instance, &previous, &value, &details, cx);
            }
        }
        true
    }

    /// Reports the end of an interaction that changed the key in steps, like
    /// a slider's drag (`createCommit`), with the key's value. Skipped while
    /// the component is being destroyed, since nobody ended what that cut
    /// short.
    pub fn commit(
        &self,
        instance: &Instance,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.commit_value(instance, self.get(), reason, event, window, cx);
    }

    /// `commit` with the value it commits (`commit(key, value, details)`),
    /// for a controlled key whose app hasn't passed the value back yet, like
    /// a code typed into a controlled input.
    pub fn commit_value(
        &self,
        instance: &Instance,
        value: &T,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if keep::is_destroying() {
            return;
        }
        let source = resolve_source(reason, event.as_ref());
        let details = ChangeDetails::new(reason, source, event);
        if let Some(callback) = self.on_commit.clone() {
            if self.after {
                self.run_after(instance, callback, value.clone(), details.clone(), false, window, cx);
            } else {
                with_source(source, || callback(value, &details, window, cx));
            }
        }
        watchers::emit_commit_of(instance, self.key, value, &details);
    }

    // Runs a callback of an `after_update` key once the update is over, with
    // the source the change had. Development builds warn about a change's
    // `details.cancel()` there, which comes too late to refuse anything.
    #[allow(clippy::too_many_arguments)]
    fn run_after(
        &self,
        instance: &Instance,
        callback: ChangeCallback<T>,
        value: T,
        details: ChangeDetails,
        change: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let origin = (dev::DEV && change).then(|| (instance.label().clone(), instance.id().clone(), self.key));
        window.defer(cx, move |window, cx| {
            with_source(details.source, || callback(&value, &details, window, cx));
            if let Some((component, id, key)) = origin
                && details.canceled()
            {
                let before = callback_name(key, CallbackKind::Change).replacen("on_", "on_before_", 1);
                dev::warn(
                    &format!(
                        "`details.cancel()` came after `{key}` changed, in a callback that runs once the update is over, and did nothing. Refuse the change in `{before}`."
                    ),
                    Some(Origin::new(&component, &id)),
                );
            }
        });
    }

    // A controlled change was asked for: warns when the app still passes the
    // old value a second later, like a controlled `open` it never updates in
    // `on_open_change`. A second leaves time for a save or navigation that
    // passes the value back once it's done (`expectControlled`).
    fn expect(&self, instance: &Instance, before: &T, asked: &T, details: &ChangeDetails, cx: &mut App) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        *self.expecting.borrow_mut() = Some(Expecting {
            before: before.clone(),
            generation,
        });
        let expecting = self.expecting.clone();
        let key = self.key;
        let asked_text = match (key, format!("{asked:?}").as_str()) {
            ("open", "true") => "to open".to_string(),
            ("open", "false") => "to close".to_string(),
            (_, value) => format!("for `{key}` {value}"),
        };
        let before_text = format!("{before:?}");
        let reason = details.reason.as_str();
        let source = details.source.as_str();
        let component = instance.label().clone();
        let id = instance.id().clone();
        let after = self.after;
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let still = expecting
                .borrow_mut()
                .take_if(|waiting| waiting.generation == generation)
                .is_some();
            if still {
                let callback = callback_name(key, CallbackKind::Change);
                let refuse = if after {
                    format!("refuse it with `details.cancel()` in `{}`", callback.replacen("on_", "on_before_", 1))
                } else {
                    "call `details.cancel()` there to refuse".to_string()
                };
                dev::warn(
                    &format!(
                        "asked {asked_text} (`{reason}`, `{source}`), and `{key}` stayed {before_text}. Update it in `{callback}`, right away even when a save or navigation follows, or {refuse}."
                    ),
                    Some(Origin::new(&component, &id)),
                );
            }
        })
        .detach();
    }

    // Stops waiting for a controlled value the app passed something else
    // for, so one that opens and closes again before the check doesn't warn
    // (`heardControlled`).
    fn heard(&self, value: Option<&T>) {
        let mut expecting = self.expecting.borrow_mut();
        if expecting
            .as_ref()
            .is_some_and(|waiting| value != Some(&waiting.before))
        {
            *expecting = None;
        }
    }
}

impl<T> Drop for Controllable<T> {
    fn drop(&mut self) {
        // The component left: nothing waits for its values (`forgetControlled`).
        if let Ok(mut expecting) = self.expecting.try_borrow_mut() {
            *expecting = None;
        }
    }
}

/// Records a write no one can refuse, like an action going pending, for the
/// devtools and `debug` (`log`). `observe` doesn't report it.
pub fn log(instance: &Instance, key: &'static str, value: &dyn Debug, reason: Reason) {
    log_with(instance, key, value, reason, None);
}

/// `log` with the source an event explains it with.
pub fn log_with(
    instance: &Instance,
    key: &'static str,
    value: &dyn Debug,
    reason: Reason,
    source: Option<ChangeSource>,
) {
    let source = source.unwrap_or_else(|| resolve_source(reason, None));
    let details = ChangeDetails::new(reason, source, None);
    instance.debug_change(key, value, &details);
    watchers::emit_log(instance, key, value, &details);
}

/// `createChange` with its source given, for a component of ours whose
/// changes no input causes (`changeFrom`): pass the source in the options,
/// `ChangeOptions::reason(reason).source(source)`.
pub fn change_from(options: ChangeOptions, source: ChangeSource) -> ChangeOptions {
    options.source(source)
}
