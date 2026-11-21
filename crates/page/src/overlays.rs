//! Port of `page/overlays/overlays.ts`: the overlay host's registry of
//! overlays opened from anywhere, like `toast()` and `confirm()`, and later
//! sheets. The app renders one host that renders every entry by its type;
//! core never renders anything itself.
//!
//! The original's host is a framework component the app mounts once
//! (`<OverlayHost />`). Here a host is the app's own view: it subscribes to
//! the registry, renders [`Overlays::entries`] by their `kind`, and counts
//! itself with [`Overlays::attach_host`] while it's in the window.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, SharedString, Subscription};
use ui_sdk_kernel::dev;

/// One overlay a host renders.
#[derive(Clone)]
pub struct OverlayEntry {
    /// Unique within its registry.
    pub id: SharedString,
    /// What kind of overlay it is, which picks what renders it: `"toaster"`
    /// or `"confirm"` (`type` in the original).
    pub kind: SharedString,
    /// What the renderer needs, like the toaster whose region it shows.
    /// Downcast it to the type the kind's opener put in.
    pub data: Rc<dyn Any>,
}

impl fmt::Debug for OverlayEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayEntry")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
    static SHARED: Overlays = create_overlays();
}

/// Hears an overlay added or removed.
pub type OverlaysListener = Rc<dyn Fn(&mut App)>;

/// Whether something else already shows an overlay, like a region the app
/// placed, so development builds don't warn about a missing host.
pub type RenderedFn = Rc<dyn Fn(&App) -> bool>;

struct Inner {
    // Replaced on every change, so a host can compare snapshots by identity.
    entries: Rc<[OverlayEntry]>,
    listeners: Vec<(u64, OverlaysListener)>,
    next_listener: u64,
    hosts: usize,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            entries: Rc::from(Vec::new()),
            listeners: Vec::new(),
            next_listener: 0,
            hosts: 0,
        }
    }
}

/// A registry of overlays, from [`create_overlays`]. Clones are the same
/// registry.
#[derive(Clone, Default)]
pub struct Overlays {
    inner: Rc<RefCell<Inner>>,
}

/// Creates a registry of overlays. Most apps use the shared [`overlays()`],
/// which `toast()` and `confirm()` open into; a separate one is for a
/// separate host, like one in another window.
pub fn create_overlays() -> Overlays {
    Overlays::default()
}

impl Overlays {
    fn notify(&self, cx: &mut App) {
        let listeners: Vec<OverlaysListener> = self
            .inner
            .borrow()
            .listeners
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect();
        for listener in listeners {
            listener(cx);
        }
    }

    fn has_entry(&self, id: &str) -> bool {
        self.inner
            .borrow()
            .entries
            .iter()
            .any(|entry| entry.id.as_ref() == id)
    }

    /// Adds an overlay for the hosts to render. Returns its id; pass it to
    /// [`unmount`](Self::unmount) once the overlay has finished closing.
    /// `rendered` says whether something else already shows it, like a
    /// region the app placed, so development builds don't warn about a
    /// missing host.
    pub fn mount(
        &self,
        kind: impl Into<SharedString>,
        data: Rc<dyn Any>,
        rendered: Option<RenderedFn>,
        cx: &mut App,
    ) -> SharedString {
        let kind = kind.into();
        let count = COUNT.with(|count| {
            count.set(count.get() + 1);
            count.get()
        });
        let id: SharedString = format!("{kind}-{count}").into();
        {
            let mut inner = self.inner.borrow_mut();
            let mut entries = inner.entries.to_vec();
            entries.push(OverlayEntry {
                id: id.clone(),
                kind: kind.clone(),
                data,
            });
            inner.entries = Rc::from(entries);
        }
        self.notify(cx);
        // A host that renders in the same frame, or the next, still counts.
        if dev::DEV {
            let overlays = self.clone();
            let waiting = id.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                cx.update(|cx| {
                    let hosted = overlays.inner.borrow().hosts > 0;
                    if !hosted
                        && !rendered.as_ref().is_some_and(|rendered| rendered(cx))
                        && overlays.has_entry(&waiting)
                    {
                        dev::warn(
                            &format!(
                                "A {kind} opened, but no host renders it. Render the overlays once in your app, from a view that calls `attach_host`."
                            ),
                            None,
                        );
                    }
                });
            })
            .detach();
        }
        id
    }

    /// Removes an overlay.
    pub fn unmount(&self, id: &str, cx: &mut App) {
        {
            let mut inner = self.inner.borrow_mut();
            let next: Vec<OverlayEntry> = inner
                .entries
                .iter()
                .filter(|entry| entry.id.as_ref() != id)
                .cloned()
                .collect();
            if next.len() == inner.entries.len() {
                return;
            }
            inner.entries = Rc::from(next);
        }
        self.notify(cx);
    }

    /// The overlays to render, oldest first. The same slice until something
    /// changes.
    pub fn entries(&self) -> Rc<[OverlayEntry]> {
        self.inner.borrow().entries.clone()
    }

    /// Calls `listener` whenever an overlay is added or removed, until the
    /// returned subscription drops. A host notifies itself from it.
    pub fn subscribe(&self, listener: impl Fn(&mut App) + 'static) -> Subscription {
        let id = {
            let mut inner = self.inner.borrow_mut();
            let id = inner.next_listener;
            inner.next_listener += 1;
            inner.listeners.push((id, Rc::new(listener)));
            id
        };
        let inner = Rc::downgrade(&self.inner);
        Subscription::new(move || {
            if let Some(inner) = inner.upgrade() {
                inner.borrow_mut().listeners.retain(|(other, _)| *other != id);
            }
        })
    }

    /// Counts a host that renders this registry. Hosts call it while they're
    /// in the window, and keep the returned subscription, whose drop releases
    /// it.
    pub fn attach_host(&self) -> Subscription {
        self.inner.borrow_mut().hosts += 1;
        let inner = Rc::downgrade(&self.inner);
        Subscription::new(move || {
            if let Some(inner) = inner.upgrade() {
                let mut inner = inner.borrow_mut();
                inner.hosts = inner.hosts.saturating_sub(1);
            }
        })
    }

    /// Whether a host is counted to render the overlays.
    pub fn has_host(&self) -> bool {
        self.inner.borrow().hosts > 0
    }
}

/// The shared registry that `toast()` and `confirm()` open into, and that a
/// host renders by default. One per app.
pub fn overlays() -> Overlays {
    SHARED.with(Overlays::clone)
}
