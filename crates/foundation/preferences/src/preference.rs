//! Port of `foundation/preferences/preference.ts`.
//!
//! A preference: one value from a fixed list that people choose, like an
//! emoji skin tone, kept in a storage between runs. Views read it in `render`
//! and style by it, where the original shows it on a `data-*` attribute for
//! CSS.
//!
//! The original reads local and session storage back once something
//! subscribes, after hydration, with the reason `Restore`. Nothing hydrates
//! here, so a preference reads its storage as it's created, as the original
//! does for a cookie, and hears it for as long as it lives: the value starts
//! as saved, and a value saved under the same ID later, by another
//! preference or a write to the storage, arrives with the reason
//! `StorageSync`.

use std::cell::Cell;
use std::fmt::Debug;
use std::rc::Rc;

use gpui::{App, Context, Entity, EntityId, SharedString, Subscription, Window};
use ui_sdk_kernel::change;
use ui_sdk_kernel::{ChangeCallback, ChangeDetails, ChangeOptions, Controllable, Instance, Reason};
use ui_sdk_storage::ValueStorage;

/// What a preference holds: one of a fixed list of values, saved as its
/// name, like a `&'static str`, a `SharedString`, or an enum with
/// `AsRef<str>`.
pub trait PreferenceValue: Clone + PartialEq + Debug + AsRef<str> + 'static {}

impl<T: Clone + PartialEq + Debug + AsRef<str> + 'static> PreferenceValue for T {}

/// Options for [`Preference`]. `values` and `default_value` have no default,
/// so they start from [`PreferenceOptions::new`].
#[derive(Clone)]
pub struct PreferenceOptions<V> {
    /// Every value people can choose. A saved value that isn't one of them is
    /// ignored.
    pub values: Vec<V>,
    /// The value before anything is saved, and after a saved value is
    /// removed.
    pub default_value: V,
    /// The value, when the app controls it. A change asks `on_value_change`,
    /// and the app passes the new value back with `set_controlled_value`.
    pub value: Option<V>,
    /// Where the value is kept between runs, from `create_local_storage` or
    /// `create_session_storage`.
    pub storage: Option<ValueStorage>,
    /// Called when the value changes, with its reason: `Imperative`, or the
    /// one `set` was given, and `StorageSync` when a value saved under the
    /// same storage arrives. It runs once the preference's update is over, so
    /// it may update any entity, the preference too.
    pub on_value_change: Option<ChangeCallback<V>>,
    /// Called before the value changes, with its reason, inside the
    /// preference's update: `details.cancel()` keeps the current one. It may
    /// update any entity but the preference.
    pub on_before_value_change: Option<ChangeCallback<V>>,
}

impl<V: PreferenceValue> PreferenceOptions<V> {
    pub fn new(values: impl IntoIterator<Item = V>, default_value: V) -> Self {
        Self {
            values: values.into_iter().collect(),
            default_value,
            value: None,
            storage: None,
            on_value_change: None,
            on_before_value_change: None,
        }
    }

    pub fn values(mut self, values: impl IntoIterator<Item = V>) -> Self {
        self.values = values.into_iter().collect();
        self
    }

    pub fn default_value(mut self, value: V) -> Self {
        self.default_value = value;
        self
    }

    pub fn value(mut self, value: V) -> Self {
        self.value = Some(value);
        self
    }

    pub fn storage(mut self, storage: ValueStorage) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&V, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&V, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }
}

/// A preference's state, from `get`, `subscribe` and `follow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PreferenceState<V> {
    /// The value chosen.
    pub value: V,
}

/// Keeps the field of an entity's state that `key` returns set to
/// `source`'s state: writes it at the end of this update, so it can be called
/// while the entity is built, and again on every change after, notifying
/// when it differs (`followIn`). Dropping the subscription stops it.
pub(crate) fn follow_in<S: 'static, State: PartialEq + 'static, T: 'static>(
    source: &Entity<S>,
    get: impl Fn(&S, &App) -> State + 'static,
    cx: &mut Context<T>,
    key: impl Fn(&mut T) -> &mut State + 'static,
) -> Subscription {
    let write = Rc::new(move |this: &mut T, source: &Entity<S>, cx: &mut Context<T>| {
        let next = get(source.read(cx), cx);
        let field = key(this);
        if *field != next {
            *field = next;
            cx.notify();
        }
    });
    let stopped = Rc::new(Cell::new(false));
    let first = {
        let (this, source, write, stopped) =
            (cx.weak_entity(), source.clone(), write.clone(), stopped.clone());
        move |cx: &mut App| {
            if !stopped.get() {
                this.update(cx, |this, cx| write(this, &source, cx)).ok();
            }
        }
    };
    cx.defer(first);
    let observer = cx.observe(source, move |this, source, cx| write(this, &source, cx));
    Subscription::join(observer, Subscription::new(move || stopped.set(true)))
}

/// A value people choose from a list, kept between runs in its storage.
/// Views read it with `get` in `render` and style by it. Create one with
/// `cx.new(|cx| Preference::new(options, cx))` and share the entity, or one
/// per window with the same storage, which keeps them in step.
pub struct Preference<V> {
    instance: Instance,
    value: Controllable<V>,
    values: Vec<V>,
    default_value: V,
    storage: Option<ValueStorage>,
    // What the storage held when this last read or wrote it, so its own
    // write coming back changes nothing.
    synced: Option<SharedString>,
    _storage: Option<Subscription>,
}

impl<V: PreferenceValue> Preference<V> {
    /// Panics when `default_value` isn't one of `values`.
    pub fn new(options: PreferenceOptions<V>, cx: &mut Context<Self>) -> Self {
        Self::named("preference", options, cx)
    }

    // A preference whose changes are logged and watched under `name`.
    pub(crate) fn named(
        name: &'static str,
        options: PreferenceOptions<V>,
        cx: &mut Context<Self>,
    ) -> Self {
        let PreferenceOptions {
            values,
            default_value,
            value,
            storage,
            on_value_change,
            on_before_value_change,
        } = options;
        assert!(
            values.contains(&default_value),
            "{:?} isn't one of the preference's values.",
            default_value.as_ref()
        );
        let synced = storage.as_ref().and_then(|storage| storage.get(cx));
        let start = value_in(&values, synced.as_deref()).unwrap_or_else(|| default_value.clone());
        let mut controllable = Controllable::new("value", start);
        controllable.set_controlled(value);
        controllable.set_on_change(on_value_change);
        controllable.set_on_before_change(on_before_value_change);
        let subscription = storage.as_ref().map(|storage| {
            let this = cx.weak_entity();
            storage.subscribe(cx, move |saved, cx| {
                this.update(cx, |this, cx| this.storage_changed(saved, cx)).ok();
            })
        });
        Self {
            instance: Instance::new(name),
            value: controllable,
            values,
            default_value,
            storage,
            synced,
            _storage: subscription,
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// Every value people can choose.
    pub fn values(&self) -> &[V] {
        &self.values
    }

    /// The value when nothing allowed is saved.
    pub fn default_value(&self) -> &V {
        &self.default_value
    }

    /// Where the value is kept between runs.
    pub fn storage(&self) -> Option<&ValueStorage> {
        self.storage.as_ref()
    }

    /// The current state.
    pub fn get(&self) -> PreferenceState<V> {
        PreferenceState {
            value: self.value.get().clone(),
        }
    }

    /// Changes the value, through `on_before_value_change`, which can cancel
    /// it, and `on_value_change`, and
    /// saves it. The reason defaults to `Imperative`. Returns whether it
    /// changed, or for a controlled value, whether the change was accepted.
    pub fn set(
        &mut self,
        value: V,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let before = self.value.get().clone();
        let changed = self.change(value, options, window, cx);
        if *self.value.get() != before {
            cx.notify();
        }
        changed
    }

    /// Passes the controlled value, or `None` to let the preference keep its
    /// own.
    pub fn set_controlled_value(&mut self, value: Option<V>, cx: &mut Context<Self>) {
        if self.value.set_controlled(value) {
            cx.notify();
        }
    }

    /// Hears every change, with the state after it. Dropping the
    /// subscription stops it.
    pub fn subscribe(
        this: &Entity<Self>,
        cx: &mut App,
        listener: impl Fn(&PreferenceState<V>, &mut App) + 'static,
    ) -> Subscription {
        cx.observe(this, move |this, cx| {
            let state = this.read(cx).get();
            listener(&state, cx);
        })
    }

    /// Keeps the field of an entity's state that `key` returns set to the
    /// preference's state: writes it at the end of this update and on every
    /// change after. Keep the subscription while it follows.
    pub fn follow<T: 'static>(
        this: &Entity<Self>,
        cx: &mut Context<T>,
        key: impl Fn(&mut T) -> &mut PreferenceState<V> + 'static,
    ) -> Subscription {
        follow_in(this, |preference, _| preference.get(), cx, key)
    }

    fn change(&mut self, next: V, options: ChangeOptions, window: &mut Window, cx: &mut App) -> bool {
        if !self.values.contains(&next) {
            return false;
        }
        let reason = options.reason.unwrap_or(Reason::Imperative);
        if !self.value.change(&self.instance, next.clone(), options, window, cx) {
            return false;
        }
        // A value read from storage is already there.
        if !matches!(reason, Reason::Restore | Reason::StorageSync)
            && let Some(storage) = &self.storage
            && storage.set(next.as_ref(), cx)
        {
            self.synced = Some(next.as_ref().into());
        }
        true
    }

    // A value saved under the storage's ID, or its removal, which goes back
    // to the default.
    fn storage_changed(&mut self, saved: Option<SharedString>, cx: &mut Context<Self>) {
        // Its own write coming back, or the same value again.
        if saved == self.synced {
            return;
        }
        let next = value_in(&self.values, saved.as_deref()).unwrap_or_else(|| self.default_value.clone());
        self.synced = saved;
        let before = self.value.get().clone();
        let options = ChangeOptions::reason(Reason::StorageSync);
        let entity = cx.entity_id();
        let ran = in_window(entity, cx, |window, cx| {
            self.change(next.clone(), options, window, cx);
        });
        // With no window open to run the callbacks in, the value follows.
        if ran.is_none() && self.value.set_held(next.clone()) {
            change::log(&self.instance, "value", &next, Reason::StorageSync);
        }
        if *self.value.get() != before {
            cx.notify();
        }
    }
}

// The value saved as `saved`, if it's one of `values`.
fn value_in<V: PreferenceValue>(values: &[V], saved: Option<&str>) -> Option<V> {
    let saved = saved?;
    values.iter().find(|value| value.as_ref() == saved).cloned()
}

// Runs `f` in a window, for a change no window asked for: the one that last
// rendered the entity, or else the active one, or else any. `None` when none
// is open.
fn in_window<R>(
    entity: EntityId,
    cx: &mut App,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> Option<R> {
    let mut f = Some(f);
    if let Some(result) = cx
        .with_window(entity, |window, cx| f.take().map(|f| f(window, cx)))
        .flatten()
    {
        return Some(result);
    }
    let f = f?;
    let window = cx
        .active_window()
        .or_else(|| cx.windows().into_iter().next())?;
    window.update(cx, |_, window, cx| f(window, cx)).ok()
}
