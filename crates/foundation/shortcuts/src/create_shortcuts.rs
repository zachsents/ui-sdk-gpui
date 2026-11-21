//! Port of `foundation/shortcuts/create-shortcuts.ts`: one set of keyboard
//! shortcuts with one key listener, scoped to the open layers.
//!
//! The original listens for `keydown` on the document. Here the set hears
//! keys through `cx.observe_keystrokes`, which runs once the focused element
//! and the keymap had their turn, as a document's listener does: a key an
//! element prevented (`window.prevent_default()`) or stopped, or one an
//! action took, never reaches it. It hears every window, and a key is
//! measured against the layers of the window it was pressed in.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fmt;
use std::rc::{Rc, Weak};

use gpui::{App, FocusHandle, KeyDownEvent, KeystrokeEvent, SharedString, Subscription, Window};
use ui_sdk_kernel::{dev, is_composing, key_held};
use ui_sdk_platform::{
    Layer, Region, active_layers, aria_keyshortcuts, find_layer, format_shortcut, has_command_modifier,
    in_persistent_layer, is_apple, matches_shortcut,
};
use ui_sdk_popups::in_popup_element;

use crate::shortcut_order::{is_shortcut_allowed, is_typing_in, shortcut_order};

/// Runs when a shortcut's keys are pressed. The key press is already marked
/// handled. It runs from the window's key observer, outside any entity's
/// update, so it may update any entity.
pub type ShortcutHandler = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>;

/// Whether a registered shortcut may run now (`when`).
pub type ShortcutWhen = Rc<dyn Fn(&Window, &App) -> bool>;

/// The element a shortcut belongs to (`layer`): an element, as containment
/// sees it, or a function that returns one, read on every key press.
///
/// The element is a platform [`Region`]: the shortcut asks about its focus
/// handle, so give it one its element tracks, like a trigger's. One without
/// a focus handle is asked about at the center of its last bounds. Its
/// element is in the page while gpui rendered its focus handle last frame.
#[derive(Clone)]
pub enum ShortcutLayer {
    Element(Region),
    Fn(Rc<dyn Fn(&App) -> Option<Region>>),
}

impl ShortcutLayer {
    /// A function read on every key press, for an element that changes.
    pub fn from_fn(element: impl Fn(&App) -> Option<Region> + 'static) -> Self {
        ShortcutLayer::Fn(Rc::new(element))
    }

    /// The element, as of now.
    pub fn element(&self, cx: &App) -> Option<Region> {
        match self {
            ShortcutLayer::Element(region) => Some(region.clone()),
            ShortcutLayer::Fn(element) => element(cx),
        }
    }

    // The same layer, as the original compares them, by identity: one
    // region, or two tracking one focus handle, or one function.
    fn same(&self, other: &ShortcutLayer) -> bool {
        match (self, other) {
            (ShortcutLayer::Element(a), ShortcutLayer::Element(b)) => {
                a == b
                    || a.focus_handle()
                        .zip(b.focus_handle())
                        .is_some_and(|(a, b)| a == b)
            }
            (ShortcutLayer::Fn(a), ShortcutLayer::Fn(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl From<Region> for ShortcutLayer {
    fn from(region: Region) -> Self {
        ShortcutLayer::Element(region)
    }
}

impl From<&Region> for ShortcutLayer {
    fn from(region: &Region) -> Self {
        ShortcutLayer::Element(region.clone())
    }
}

impl From<FocusHandle> for ShortcutLayer {
    fn from(handle: FocusHandle) -> Self {
        ShortcutLayer::Element(Region::with_focus_handle(handle))
    }
}

impl From<&FocusHandle> for ShortcutLayer {
    fn from(handle: &FocusHandle) -> Self {
        ShortcutLayer::Element(Region::with_focus_handle(handle.clone()))
    }
}

impl fmt::Debug for ShortcutLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShortcutLayer::Element(region) => f.debug_tuple("Element").field(region).finish(),
            ShortcutLayer::Fn(_) => write!(f, "Fn(..)"),
        }
    }
}

/// Where and when a registered shortcut runs.
#[derive(Clone, Default)]
pub struct ShortcutOptions {
    /// A named scope, like `"editor"`. The shortcut runs only while
    /// `set_scope(name, true)` is in effect. By default it runs in every
    /// scope.
    pub scope: Option<SharedString>,
    /// An element inside a popup, or a function that returns one. The
    /// shortcut belongs to that popup: it runs only while the popup is open,
    /// and keeps running while a modal is open above the window. An element
    /// in the window, outside any popup, makes it a window shortcut, which a
    /// modal pauses. An element in a persistent layer, like a toast region,
    /// keeps it running over every modal. A closed popup's content that stays
    /// in the window, hidden, keeps it from running. By default, a window
    /// shortcut.
    pub layer: Option<ShortcutLayer>,
    /// Whether it runs while focus is in a text field. Shortcuts with
    /// Command, Control or Option always do. Other keys typed into a field
    /// are text, so they don't run unless this is set. Defaults to false.
    pub allow_in_input: bool,
    /// Whether it runs again while the keys are held down, like `j` moving
    /// through a list. Defaults to false, so it runs once per press.
    pub repeat: bool,
    /// Checked each time the keys are pressed, and by `list()`. The shortcut
    /// runs only while it returns true.
    pub when: Option<ShortcutWhen>,
    /// What the shortcut does, for a help screen that lists them.
    pub description: Option<SharedString>,
}

impl ShortcutOptions {
    pub fn scope(mut self, scope: impl Into<SharedString>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    pub fn layer(mut self, layer: impl Into<ShortcutLayer>) -> Self {
        self.layer = Some(layer.into());
        self
    }

    pub fn allow_in_input(mut self, allow: bool) -> Self {
        self.allow_in_input = allow;
        self
    }

    pub fn repeat(mut self, repeat: bool) -> Self {
        self.repeat = repeat;
        self
    }

    pub fn when(mut self, when: impl Fn(&Window, &App) -> bool + 'static) -> Self {
        self.when = Some(Rc::new(when));
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl fmt::Debug for ShortcutOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShortcutOptions")
            .field("scope", &self.scope)
            .field("layer", &self.layer)
            .field("allow_in_input", &self.allow_in_input)
            .field("repeat", &self.repeat)
            .field("when", &self.when.as_ref().map(|_| ".."))
            .field("description", &self.description)
            .finish()
    }
}

/// A registered shortcut, as `list()` describes it for a help screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutInfo {
    /// The keys it was registered with, like `"mod+k"`.
    pub keys: SharedString,
    /// The keys formatted for this platform: `⌘K` on Apple platforms and
    /// `Ctrl+K` elsewhere.
    pub hint: SharedString,
    /// The keys as `aria-keyshortcuts` names them, like `"Meta+K"`.
    pub aria_keyshortcuts: SharedString,
    /// What it does, from `description`.
    pub description: Option<SharedString>,
    /// Its named scope, from `scope`.
    pub scope: Option<SharedString>,
}

/// Keys like `"mod+k"`, or a function read on every key press, for keys that
/// change. A function that returns `None` turns it off.
#[derive(Clone)]
pub enum ShortcutKeys {
    Keys(SharedString),
    Fn(Rc<dyn Fn(&App) -> Option<SharedString>>),
}

impl ShortcutKeys {
    /// A function read on every key press.
    pub fn from_fn(keys: impl Fn(&App) -> Option<SharedString> + 'static) -> Self {
        ShortcutKeys::Fn(Rc::new(keys))
    }
}

impl From<&'static str> for ShortcutKeys {
    fn from(keys: &'static str) -> Self {
        ShortcutKeys::Keys(keys.into())
    }
}

impl From<String> for ShortcutKeys {
    fn from(keys: String) -> Self {
        ShortcutKeys::Keys(keys.into())
    }
}

impl From<SharedString> for ShortcutKeys {
    fn from(keys: SharedString) -> Self {
        ShortcutKeys::Keys(keys)
    }
}

impl fmt::Debug for ShortcutKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShortcutKeys::Keys(keys) => f.debug_tuple("Keys").field(keys).finish(),
            ShortcutKeys::Fn(_) => write!(f, "Fn(..)"),
        }
    }
}

struct Entry {
    keys: ShortcutKeys,
    handler: ShortcutHandler,
    options: ShortcutOptions,
    id: u64,
}

fn keys_of(entry: &Entry, cx: &App) -> Option<SharedString> {
    match &entry.keys {
        ShortcutKeys::Keys(keys) => Some(keys.clone()),
        ShortcutKeys::Fn(keys) => keys(cx),
    }
    .filter(|keys| !keys.is_empty())
}

// The same keys on every platform, to tell conflicting registrations apart.
fn normalize(keys: &str) -> String {
    let aria = aria_keyshortcuts(keys, true);
    let mut parts: Vec<&str> = aria.split('+').collect();
    parts.sort_unstable();
    parts.join("+")
}

#[derive(Default)]
struct Inner {
    entries: Vec<Rc<Entry>>,
    scopes: HashSet<SharedString>,
    next: u64,
    off: Option<Subscription>,
}

/// A set of keyboard shortcuts with one key listener, like the shared
/// [`shortcuts()`](crate::shortcuts()). Clones are the same set.
#[derive(Clone, Default)]
pub struct Shortcuts {
    inner: Rc<RefCell<Inner>>,
}

/// Creates a set of keyboard shortcuts with one key listener. The app has
/// one, the shared [`shortcuts()`](crate::shortcuts()), which components and
/// apps register into.
pub fn create_shortcuts() -> Shortcuts {
    Shortcuts::default()
}

// The order of the topmost open layer that pauses shortcuts outside it.
fn top_blocking(window: &Window) -> Option<u64> {
    active_layers(window)
        .into_iter()
        .filter(Layer::blocking)
        .last()
        .map(|layer| layer.order())
}

impl Shortcuts {
    fn entries(&self) -> Vec<Rc<Entry>> {
        self.inner.borrow().entries.clone()
    }

    // A shortcut's place among the layers, if it may run at all right now.
    fn runnable(&self, entry: &Entry, blocking: Option<u64>, window: &Window, cx: &App) -> Option<u64> {
        let options = &entry.options;
        if let Some(scope) = &options.scope
            && !self.inner.borrow().scopes.contains(scope)
        {
            return None;
        }
        if let Some(when) = &options.when
            && !when(window, cx)
        {
            return None;
        }
        let order = shortcut_order(
            options.layer.as_ref(),
            |target| find_layer(target, window, cx),
            |target| in_persistent_layer(target, window),
            |target| in_popup_element(target, window),
            window,
            cx,
        );
        if is_shortcut_allowed(order, blocking) {
            order
        } else {
            None
        }
    }

    fn keydown(&self, event: &KeystrokeEvent, window: &mut Window, cx: &mut App) {
        if event.action.is_some() || window.default_prevented() || is_composing(window, cx) {
            return;
        }
        // Whether the key repeats because it's held, as the parts it passed
        // on its way to the focused element saw it.
        let held = key_held(&event.keystroke);
        let entries = self.entries();
        // Read only once some keys match, so typing that matches nothing
        // reads nothing.
        let mut context: Option<(bool, Option<u64>)> = None;
        let mut best: Option<(Rc<Entry>, u64)> = None;
        let apple = is_apple(cx);
        for entry in entries {
            let Some(keys) = keys_of(&entry, cx) else {
                continue;
            };
            if (held && !entry.options.repeat) || !matches_shortcut(&event.keystroke, &keys, apple) {
                continue;
            }
            let (typing, blocking) =
                *context.get_or_insert_with(|| (is_typing_in(window), top_blocking(window)));
            if typing && !entry.options.allow_in_input && !has_command_modifier(&keys) {
                continue;
            }
            let Some(order) = self.runnable(&entry, blocking, window, cx) else {
                continue;
            };
            // The topmost layer's shortcut wins, then the newest registration.
            let better = best
                .as_ref()
                .is_none_or(|(current, at)| order > *at || (order == *at && entry.id > current.id));
            if better {
                best = Some((entry, order));
            }
        }
        let Some((entry, _)) = best else {
            return;
        };
        window.prevent_default();
        let down = KeyDownEvent {
            keystroke: event.keystroke.clone(),
            is_held: held,
            prefer_character_input: false,
        };
        let handler = entry.handler.clone();
        handler(&down, window, cx);
    }

    // It keeps listening once it has, as a document listener costs nothing
    // while no keys match. A text field edits once every observer had the
    // key, so a shortcut that takes it keeps it from the field.
    fn listen(&self, cx: &mut App) {
        if self.inner.borrow().off.is_some() {
            return;
        }
        let weak = Rc::downgrade(&self.inner);
        let off = cx.observe_keystrokes(move |event, window, cx| {
            if let Some(inner) = weak.upgrade() {
                Shortcuts { inner }.keydown(event, window, cx);
            }
        });
        self.inner.borrow_mut().off = Some(off);
    }

    /// Registers a shortcut, like `register("mod+k", open_search, ..)`. Keys
    /// are `+`-separated: `mod` is Command on Apple platforms and Control
    /// elsewhere. Returns a guard that unregisters it, when released or
    /// dropped.
    pub fn register(
        &self,
        keys: impl Into<ShortcutKeys>,
        handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
        options: ShortcutOptions,
        cx: &mut App,
    ) -> RegisteredShortcut {
        self.register_handler(keys.into(), Rc::new(handler), options, cx)
    }

    /// [`register`](Self::register), with a handler already shared.
    pub fn register_handler(
        &self,
        keys: ShortcutKeys,
        handler: ShortcutHandler,
        options: ShortcutOptions,
        cx: &mut App,
    ) -> RegisteredShortcut {
        let id = {
            let mut inner = self.inner.borrow_mut();
            // Development builds only, so release builds skip the scan.
            if dev::DEV
                && let ShortcutKeys::Keys(text) = &keys
                && options.when.is_none()
            {
                let key = normalize(text);
                let clash = inner.entries.iter().any(|entry| {
                    matches!(&entry.keys, ShortcutKeys::Keys(other) if normalize(other) == key)
                        && entry.options.scope == options.scope
                        && match (&entry.options.layer, &options.layer) {
                            (None, None) => true,
                            (Some(a), Some(b)) => a.same(b),
                            _ => false,
                        }
                        && entry.options.when.is_none()
                });
                if clash {
                    dev::warn(
                        &format!(
                            "The shortcut \"{text}\" is registered twice in the same scope. The newest one wins."
                        ),
                        None,
                    );
                }
            }
            let id = inner.next;
            inner.next += 1;
            inner.entries.push(Rc::new(Entry {
                keys,
                handler,
                options,
                id,
            }));
            id
        };
        self.listen(cx);
        RegisteredShortcut {
            inner: Rc::downgrade(&self.inner),
            id,
            done: Cell::new(false),
        }
    }

    /// The shortcuts that would run now in `window`, oldest first, formatted
    /// for this platform. For a help screen.
    pub fn list(&self, window: &Window, cx: &App) -> Vec<ShortcutInfo> {
        let blocking = top_blocking(window);
        let apple = is_apple(cx);
        self.entries()
            .iter()
            .filter_map(|entry| {
                let keys = keys_of(entry, cx)?;
                self.runnable(entry, blocking, window, cx)?;
                Some(ShortcutInfo {
                    hint: format_shortcut(&keys, apple).into(),
                    aria_keyshortcuts: aria_keyshortcuts(&keys, apple).into(),
                    description: entry.options.description.clone(),
                    scope: entry.options.scope.clone(),
                    keys,
                })
            })
            .collect()
    }

    /// Turns a named scope on or off. Shortcuts registered with that `scope`
    /// run only while it's on.
    pub fn set_scope(&self, name: impl Into<SharedString>, active: bool) {
        let name = name.into();
        let mut inner = self.inner.borrow_mut();
        if active {
            inner.scopes.insert(name);
        } else {
            inner.scopes.remove(&name);
        }
    }

    /// Whether a named scope is on.
    pub fn is_scope_active(&self, name: &str) -> bool {
        self.inner
            .borrow()
            .scopes
            .iter()
            .any(|scope| scope.as_ref() == name)
    }

    /// Unregisters every shortcut and stops listening.
    pub fn destroy(&self) {
        let off = {
            let mut inner = self.inner.borrow_mut();
            inner.entries.clear();
            inner.scopes.clear();
            inner.off.take()
        };
        drop(off);
    }
}

/// A shortcut in its set until released or dropped (the function `register`
/// returns in the original).
#[must_use = "dropping it unregisters the shortcut"]
pub struct RegisteredShortcut {
    inner: Weak<RefCell<Inner>>,
    id: u64,
    done: Cell<bool>,
}

impl RegisteredShortcut {
    /// Unregisters the shortcut. Safe to call twice.
    pub fn unregister(&self) {
        if self.done.replace(true) {
            return;
        }
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let Ok(mut inner) = inner.try_borrow_mut() else {
            return;
        };
        let id = self.id;
        inner.entries.retain(|entry| entry.id != id);
    }
}

impl Drop for RegisteredShortcut {
    fn drop(&mut self) {
        self.unregister();
    }
}

impl fmt::Debug for RegisteredShortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisteredShortcut")
            .field("id", &self.id)
            .field("done", &self.done.get())
            .finish()
    }
}
