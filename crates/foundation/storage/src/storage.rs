//! Port of `foundation/storage/storage.ts`.
//!
//! Where a value is kept between runs: local storage or session storage,
//! under an ID. Local storage is unavailable until the app names its file
//! with [`init`](crate::init), or where its directory can't be made, and then
//! reads return `None` and writes do nothing.
//!
//! Every storage with the same kind and ID hears the same writes, so a write
//! through one reaches the others. Every window shares the app's storage, so
//! other windows hear a write the way other tabs hear the browser's `storage`
//! event.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, SharedString, Subscription};

use crate::area::{self, Areas, Kind};

fn check_id(id: &str) {
    assert!(
        !id.is_empty(),
        "A storage needs an ID, like create_local_storage(\"color-scheme\")."
    );
}

/// A value kept under an ID, from [`create_local_storage`] or
/// [`create_session_storage`]. Pass one to anything that saves a value, like
/// a color scheme's `storage`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ValueStorage {
    kind: Kind,
    id: SharedString,
}

impl ValueStorage {
    /// The ID the value is kept under: its key in local or session storage.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Where the value is kept: `Local` or `Session`.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Whether storage appears available. A later write can still fail.
    pub fn available(&self, cx: &App) -> bool {
        self.kind.available(cx)
    }

    /// The value kept under the ID, or `None` when nothing is, or the storage
    /// is unavailable.
    pub fn get(&self, cx: &App) -> Option<SharedString> {
        self.kind.get_item(&self.id, cx)
    }

    /// Saves a value under the ID. Returns false when writing is unavailable.
    /// Local storage writes its file in the background, and logs a write that
    /// fails; the value then lasts while the app runs.
    pub fn set(&self, value: impl Into<SharedString>, cx: &mut App) -> bool {
        self.kind.set_item(&self.id, value, cx)
    }

    /// Removes the value. Returns false when writing is unavailable.
    pub fn remove(&self, cx: &mut App) -> bool {
        self.kind.remove_item(&self.id, cx)
    }

    /// Hears the value change, through any storage with the same kind and ID,
    /// in any window. It hears each write at the end of the update that made
    /// it, with the value then, and not the value it starts with. Dropping
    /// the subscription stops it.
    pub fn subscribe(
        &self,
        cx: &mut App,
        listener: impl Fn(Option<SharedString>, &mut App) + 'static,
    ) -> Subscription {
        // One per subscription, so the same listener subscribed twice stops
        // once per subscription dropped.
        let entry = Rc::new(Entry {
            storage: self.clone(),
            heard: Cell::new(area::writes(self.kind, &self.id, cx)),
            stopped: Cell::new(false),
            listener: Box::new(listener),
        });
        let observer = cx.observe_global::<Areas>({
            let entry = entry.clone();
            move |cx| entry.hear(cx)
        });
        // The observer starts at the end of this update, so a write before
        // then reaches the listener this way.
        cx.defer({
            let entry = entry.clone();
            move |cx| entry.hear(cx)
        });
        Subscription::join(observer, Subscription::new(move || entry.stopped.set(true)))
    }
}

type Listener = Box<dyn Fn(Option<SharedString>, &mut App)>;

// A subscription, and the writes to its key it heard.
struct Entry {
    storage: ValueStorage,
    heard: Cell<u64>,
    stopped: Cell<bool>,
    listener: Listener,
}

impl Entry {
    fn hear(&self, cx: &mut App) {
        let writes = area::writes(self.storage.kind, &self.storage.id, cx);
        if self.stopped.get() || writes == self.heard.get() {
            return;
        }
        self.heard.set(writes);
        (self.listener)(self.storage.get(cx), cx);
    }
}

// Local and session storage, which only differ in the area they use.
fn create_area_storage(kind: Kind, id: SharedString) -> ValueStorage {
    check_id(&id);
    ValueStorage { kind, id }
}

/// A value kept in local storage under `id`, which lasts until it's removed,
/// between runs, and reaches every window.
pub fn create_local_storage(id: impl Into<SharedString>) -> ValueStorage {
    create_area_storage(Kind::Local, id.into())
}

/// A value kept in session storage under `id`, which lasts while the app
/// runs and reaches every window.
pub fn create_session_storage(id: impl Into<SharedString>) -> ValueStorage {
    create_area_storage(Kind::Session, id.into())
}
