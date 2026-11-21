//! Port of `foundation/kernel/watchers.ts`: hooks `observe` and the agent
//! bridge install to hear every instance and change, including instances made
//! before anything listened. Nothing pays for them until one is installed
//! but a weak handle per live instance.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;

use gpui::SharedString;

use super::attrs::Checked;
use super::details::ChangeDetails;
use super::primitive::{Instance, InstanceHandle};
use super::primitive_types::AgentValue;

/// A change as a watcher hears it.
pub struct WatchedChange<'a> {
    pub instance: &'a Instance,
    pub key: &'static str,
    pub value: &'a dyn Debug,
    pub details: &'a ChangeDetails,
    /// The change was canceled. The devtools hear canceled changes; `observe`
    /// and the bridge skip them, as the original's watchers never hear them.
    pub canceled: bool,
    /// The key is controlled, so the app passes the value back.
    pub controlled: bool,
    /// A commit, which settles the steps of an interaction, like a drag's.
    pub commit: bool,
    any: Option<&'a dyn Any>,
    previous: Option<(&'a dyn Debug, &'a dyn Any)>,
}

impl<'a> WatchedChange<'a> {
    /// The value before the change, for a change that has one.
    pub fn previous(&self) -> Option<&'a dyn Debug> {
        self.previous.map(|(value, _)| value)
    }

    /// The new value as data, for logs and the agent.
    pub fn value_data(&self) -> AgentValue {
        data_of(self.value, self.any)
    }

    /// The value before the change as data.
    pub fn previous_data(&self) -> Option<AgentValue> {
        self.previous.map(|(value, any)| data_of(value, Some(any)))
    }

    /// The new value as data, when its type is one the kernel reads as data
    /// rather than as its debug text, like a `bool`, a `SharedString` or a
    /// list of them.
    pub fn known_value(&self) -> Option<AgentValue> {
        self.any.and_then(known)
    }

    /// The value before the change as data, when it has one of a type the
    /// kernel reads as data.
    pub fn known_previous(&self) -> Option<AgentValue> {
        self.previous.and_then(|(_, any)| known(any))
    }
}

/// What a watcher hears.
pub trait Watcher {
    /// An instance was made.
    fn create(&self, _instance: &Instance) {}
    /// An instance joined the page's registry (`watchers.add`): as it's made,
    /// or, made before the watcher was installed, from `watch_earlier`.
    fn add(&self, _instance: &InstanceHandle) {}
    fn destroy(&self, _instance: &Instance) {}
    /// A change went through, or was canceled, or a commit settled one.
    fn change(&self, _change: &WatchedChange) {}
    /// A write no one can refuse, like an action going pending (`log`),
    /// which the devtools record and `observe` doesn't report.
    fn log(&self, _change: &WatchedChange) {}
}

// Reads a value of one registered type as data.
type ToData = Rc<dyn Fn(&dyn Any) -> Option<AgentValue>>;

thread_local! {
    // The value types components registered with `register_data`.
    static DATA: RefCell<HashMap<TypeId, ToData>> = RefCell::new(HashMap::new());
    static WATCHERS: RefCell<Vec<Rc<dyn Watcher>>> = const { RefCell::new(Vec::new()) };
    static INSTALLED: Cell<usize> = const { Cell::new(0) };
    // Every live instance, weakly, so a watcher installed later finds them.
    static LIVE: RefCell<Vec<InstanceHandle>> = const { RefCell::new(Vec::new()) };
}

/// Installs a watcher. Returns a function that removes it.
pub fn install(watcher: Rc<dyn Watcher>) -> impl FnOnce() {
    WATCHERS.with(|watchers| watchers.borrow_mut().push(watcher.clone()));
    INSTALLED.with(|installed| installed.set(installed.get() + 1));
    move || {
        WATCHERS.with(|watchers| watchers.borrow_mut().retain(|w| !Rc::ptr_eq(w, &watcher)));
        INSTALLED.with(|installed| installed.set(installed.get().saturating_sub(1)));
    }
}

/// Whether a watcher is installed, so work only they read, like a part's
/// accessible node, is skipped otherwise.
pub fn installed() -> bool {
    INSTALLED.with(Cell::get) > 0
}

/// Tells `watcher` about every instance made before it was installed and
/// still alive (`watchEarlier`).
pub fn watch_earlier(watcher: &dyn Watcher) {
    let live = LIVE.with(|live| {
        let mut live = live.borrow_mut();
        live.retain(InstanceHandle::is_alive);
        live.clone()
    });
    for handle in &live {
        watcher.add(handle);
    }
}

/// Every live instance, in the order they were made.
pub fn live_instances() -> Vec<InstanceHandle> {
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        live.retain(InstanceHandle::is_alive);
        live.clone()
    })
}

fn each(run: impl Fn(&dyn Watcher)) {
    let watchers = WATCHERS.with(|watchers| watchers.borrow().clone());
    for watcher in watchers {
        run(watcher.as_ref());
    }
}

#[doc(hidden)]
pub fn emit_create(instance: &Instance) {
    let handle = instance.handle();
    LIVE.with(|live| live.borrow_mut().push(handle.clone()));
    each(|w| {
        w.create(instance);
        w.add(&handle);
    });
}

#[doc(hidden)]
pub fn emit_destroy(instance: &Instance) {
    let handle = instance.handle();
    LIVE.with(|live| live.borrow_mut().retain(|live| !live.ptr_eq(&handle)));
    each(|w| w.destroy(instance));
}

#[doc(hidden)]
pub fn emit_change(
    instance: &Instance,
    key: &'static str,
    value: &dyn Debug,
    details: &ChangeDetails,
    canceled: bool,
    controlled: bool,
) {
    each(|w| {
        w.change(&WatchedChange {
            instance,
            key,
            value,
            details,
            canceled,
            controlled,
            commit: false,
            any: None,
            previous: None,
        })
    });
}

/// `emit_change` with the values as `Any`, so watchers read them as data.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn emit_change_of<T: Debug + 'static>(
    instance: &Instance,
    key: &'static str,
    value: &T,
    previous: &T,
    details: &ChangeDetails,
    canceled: bool,
    controlled: bool,
) {
    each(|w| {
        w.change(&WatchedChange {
            instance,
            key,
            value,
            details,
            canceled,
            controlled,
            commit: false,
            any: Some(value),
            previous: Some((previous, previous)),
        })
    });
}

#[doc(hidden)]
pub fn emit_commit(instance: &Instance, key: &'static str, value: &dyn Debug, details: &ChangeDetails) {
    each(|w| {
        w.change(&WatchedChange {
            instance,
            key,
            value,
            details,
            canceled: false,
            controlled: false,
            commit: true,
            any: None,
            previous: None,
        })
    });
}

#[doc(hidden)]
pub fn emit_commit_of<T: Debug + 'static>(instance: &Instance, key: &'static str, value: &T, details: &ChangeDetails) {
    each(|w| {
        w.change(&WatchedChange {
            instance,
            key,
            value,
            details,
            canceled: false,
            controlled: false,
            commit: true,
            any: Some(value),
            previous: None,
        })
    });
}

#[doc(hidden)]
pub fn emit_log(instance: &Instance, key: &'static str, value: &dyn Debug, details: &ChangeDetails) {
    each(|w| {
        w.log(&WatchedChange {
            instance,
            key,
            value,
            details,
            canceled: false,
            controlled: false,
            commit: false,
            any: None,
            previous: None,
        })
    });
}

// A value as data: the types state keys hold read as themselves, anything
// else as its debug text.
fn data_of(value: &dyn Debug, any: Option<&dyn Any>) -> AgentValue {
    if let Some(any) = any
        && let Some(data) = known(any)
    {
        return data;
    }
    AgentValue::String(format!("{value:?}"))
}

/// Lets watchers, `observe` and history read values of `T` as data, the
/// original's values, rather than as their debug text: a value type of a
/// component's own, like a calendar's, registers how it reads as the
/// original's value as the component is made. The first registration for a
/// type stays.
pub fn register_data<T: 'static>(to_data: fn(&T) -> AgentValue) {
    DATA.with(|data| {
        data.borrow_mut()
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Rc::new(move |any: &dyn Any| any.downcast_ref::<T>().map(to_data)));
    });
}

fn known(any: &dyn Any) -> Option<AgentValue> {
    let registered = DATA.with(|data| data.borrow().get(&any.type_id()).cloned());
    if let Some(to_data) = registered {
        return to_data(any);
    }
    macro_rules! direct {
        ($($ty:ty),*) => {
            $(if let Some(value) = any.downcast_ref::<$ty>() {
                return Some(AgentValue::from(value.clone()));
            })*
        };
    }
    direct!(bool, i8, i16, i32, i64, u8, u16, u32, u64, usize, isize, f32, f64, String);
    if let Some(value) = any.downcast_ref::<SharedString>() {
        return Some(AgentValue::from(value.to_string()));
    }
    if let Some(value) = any.downcast_ref::<&'static str>() {
        return Some(AgentValue::from(*value));
    }
    if let Some(value) = any.downcast_ref::<Checked>() {
        return Some(match value {
            Checked::True => AgentValue::Bool(true),
            Checked::False => AgentValue::Bool(false),
            Checked::Indeterminate => AgentValue::from("indeterminate"),
        });
    }
    macro_rules! optional {
        ($($ty:ty),*) => {
            $(if let Some(value) = any.downcast_ref::<Option<$ty>>() {
                return Some(match value {
                    Some(value) => known(value).unwrap_or(AgentValue::Null),
                    None => AgentValue::Null,
                });
            })*
        };
    }
    optional!(bool, i32, i64, u32, u64, usize, f32, f64, String, SharedString);
    macro_rules! list {
        ($($ty:ty),*) => {
            $(if let Some(values) = any.downcast_ref::<Vec<$ty>>() {
                return Some(AgentValue::Array(
                    values.iter().map(|value| known(value).unwrap_or(AgentValue::Null)).collect(),
                ));
            })*
        };
    }
    list!(bool, i32, i64, u32, u64, usize, f32, f64, String, SharedString);
    None
}
