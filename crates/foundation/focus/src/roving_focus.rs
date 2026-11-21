//! Port of `foundation/focus/roving-focus.ts`: the roving tab stop. One tab
//! stop for a slot's parts, kept by the group so the parts render it, with
//! arrow keys between them. Tabs, radio groups, toggle groups, toolbars,
//! menubars and carousels run on it, the same block a component of your own
//! uses.
//!
//! The original binds each part's element and keeps the stop in the
//! primitive's store. Here each part binds to the group as it renders,
//! which is page order, with the focus handle its element tracks and what
//! the group needs to know of it ([`RovingMember`]). The group keeps the
//! stop, and once the frame that rendered the items is drawn, it settles
//! it, as the original does a microtask after they bind. Focus landing on an
//! item, from a press, Tab or code, is read then too: gpui tells where focus
//! is when it draws, not with events on elements.
//!
//! A component keeps one in a field. The part that holds the items calls
//! [`begin`](RovingFocus::begin), and each item part binds and tracks
//! its handle with the tab stop it gets back:
//!
//! ```text
//! let props = RovingMember::new().disabled(disabled).preferred(selected);
//! let handle = tabs.roving.focus_handle(&value, cx);
//! let tab_stop = tabs.roving.bind(value.clone(), &handle, props, cx);
//! part.focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
//!     .on_key_down(move |event, window, cx| {
//!         if let Some(moved) = roving.keydown(event, &value, window, cx) { .. }
//!     })
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::{Rc, Weak};

use gpui::{
    AnyWindowHandle, App, Bounds, Context, EntityId, FocusHandle, KeyDownEvent, ParentElement,
    Pixels, Styled, Subscription, Window, canvas,
};
use ui_sdk_kernel::Key;
use ui_sdk_platform::is_rtl;

use crate::after_bind::after_bind;
use crate::collection::{Arrows, Orientation, RovingCandidate, move_index, roving_stop, step_for};
use crate::roving_scope::RovingMember;

/// Which arrows move focus in a roving group: Up and Down, or Left and
/// Right.
pub type RovingOrientation = Orientation;

/// An item of a roving group, as the group's callbacks see it.
#[derive(Clone, Debug)]
pub struct RovingFocusItem<K> {
    /// The part's key.
    pub key: K,
    /// What the part told the group in its newest render.
    pub props: RovingMember,
    /// The focus handle its element tracks, which stands for the element.
    pub handle: FocusHandle,
    /// Its bounds as of its last layout, when it's measured with
    /// [`RovingFocus::measure`].
    pub bounds: Option<Bounds<Pixels>>,
}

/// What a custom `move` gets besides the event.
#[derive(Clone, Debug)]
pub struct RovingFocusMoveContext<K> {
    /// The item focus is on.
    pub from: RovingFocusItem<K>,
    /// The items that can take focus now, in page order.
    pub items: Vec<RovingFocusItem<K>>,
    /// Whether the items are laid out right to left, so Left and Right swap.
    pub rtl: bool,
}

/// A custom `move`: the key focus moves to for a key press, or `None` to
/// leave the press alone.
pub type RovingMoveFn<K> =
    Rc<dyn Fn(&KeyDownEvent, &RovingFocusMoveContext<K>, &App) -> Option<K>>;

/// What `on_items_change` runs.
pub type ItemsChangeFn = Rc<dyn Fn(&mut Window, &mut App)>;

// What a follower of the stop runs.
type Listener = Rc<dyn Fn(&mut App)>;

/// What [`RovingFocus::keydown`] returns for a key that was the group's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RovingMoved<K> {
    /// The key focus moved to. At either end without `loop`, it's `from`.
    pub key: K,
    /// The key it moved from.
    pub from: K,
    /// Whether Shift was held, so the caller can extend a selection.
    pub shift: bool,
}

/// How a roving group decides its tab stop and moves focus. What each item
/// is, disabled, focusable or preferred, it says itself as it renders
/// ([`RovingMember`]).
pub struct RovingFocusOptions<K> {
    /// Which arrows move focus when there's no `move`: Up and Down, Left and
    /// Right, or all four, like a rating. Defaults to [`Arrows::Vertical`].
    pub orientation: Arrows,
    /// Whether moving past either end wraps around. Defaults to `false`.
    pub r#loop: bool,
    /// Whether the item focused last keeps the stop, like a toolbar, rather
    /// than the preferred one, like tabs. Defaults to `true`.
    pub remember: bool,
    /// The key focus moves to for a key press, or `None` to leave the press
    /// alone. Replaces the default, which moves along `orientation` with the
    /// arrows, Home and End. Use it for any other shape: a tree's visible
    /// order, or a grid's rows. Shift is never swallowed, so a list can
    /// extend a range. The key may be one that isn't rendered or focusable
    /// yet, like the first day of next month in a calendar: the stop moves to
    /// it at once, and focus follows when its item can take it, as long as
    /// focus is still where the press left it. It runs inside the key
    /// handler; [`RovingFocus::keydown_with`] takes one for a single press
    /// instead, which can read the component it runs in.
    pub r#move: Option<RovingMoveFn<K>>,
    /// Runs once after items join or leave, however many did, once the stop
    /// has moved to where the rules put it. It runs after the frame that
    /// rendered them is drawn, outside any update, so it may update the
    /// component.
    pub on_items_change: Option<ItemsChangeFn>,
}

impl<K> Default for RovingFocusOptions<K> {
    fn default() -> Self {
        Self {
            orientation: Arrows::Vertical,
            r#loop: false,
            remember: true,
            r#move: None,
            on_items_change: None,
        }
    }
}

impl<K> Clone for RovingFocusOptions<K> {
    fn clone(&self) -> Self {
        Self {
            orientation: self.orientation,
            r#loop: self.r#loop,
            remember: self.remember,
            r#move: self.r#move.clone(),
            on_items_change: self.on_items_change.clone(),
        }
    }
}

impl<K> RovingFocusOptions<K> {
    pub fn orientation(mut self, orientation: impl Into<Arrows>) -> Self {
        self.orientation = orientation.into();
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
        self
    }

    pub fn remember(mut self, remember: bool) -> Self {
        self.remember = remember;
        self
    }

    pub fn r#move(
        mut self,
        r#move: impl Fn(&KeyDownEvent, &RovingFocusMoveContext<K>, &App) -> Option<K> + 'static,
    ) -> Self {
        self.r#move = Some(Rc::new(r#move));
        self
    }

    pub fn on_items_change(mut self, callback: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_items_change = Some(Rc::new(callback));
        self
    }
}

/// Whether an item rendering before its group has settled the stop, like on
/// the first frame, holds it: the first one its group calls eligible does,
/// and keeps it until the real stop is known, so the frame has exactly one
/// (`createClaim`).
pub struct Claim<K> {
    claimed: RefCell<Option<K>>,
}

impl<K> Default for Claim<K> {
    fn default() -> Self {
        Self {
            claimed: RefCell::new(None),
        }
    }
}

impl<K: Clone + PartialEq> Claim<K> {
    /// Whether `key` holds the stop: the first eligible key to ask does.
    pub fn claim(&self, key: &K, eligible: bool) -> bool {
        let mut claimed = self.claimed.borrow_mut();
        if claimed.is_none() && eligible {
            *claimed = Some(key.clone());
        }
        claimed.as_ref() == Some(key)
    }
}

/// A new [`Claim`].
pub fn create_claim<K>() -> Claim<K> {
    Claim::default()
}

/// One tab stop for a slot's parts, and arrow keys between them, as the
/// WAI-ARIA practices ask of toolbars, listboxes, trees and grids.
///
/// Before the items have settled, the first item that claims the stop holds
/// it. Once they have, the stop goes to the item focused last (with
/// `remember`), else the first preferred enabled item, else the first
/// enabled one. Cloning it gives another handle to the same group, so a key
/// handler can hold one.
///
/// Make it once in the component's constructor, keep it in a field, and
/// wire the item slot: the part that holds the items calls
/// [`begin`](Self::begin) as it renders, each item [`bind`](Self::bind)s and
/// tracks its handle with the tab stop that returns, and hands its key
/// presses to [`keydown`](Self::keydown).
pub struct RovingFocus<K> {
    inner: Rc<RefCell<Inner<K>>>,
    followers: Rc<RefCell<Followers<K>>>,
}

impl<K> Clone for RovingFocus<K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            followers: self.followers.clone(),
        }
    }
}

struct Entry<K> {
    key: K,
    props: RovingMember,
    handle: FocusHandle,
}

enum Wait<K> {
    // A move to an item that hadn't rendered or couldn't take focus yet,
    // with the item remembered before it.
    Move { before: Option<K> },
    // The item with the stop left with focus, and may render again.
    Left,
}

struct Pending<K> {
    key: K,
    // The element focus was on, which focus only follows from.
    from: Option<FocusHandle>,
    wait: Wait<K>,
    // The render it waits for comes after this one.
    pass: u64,
}

struct Follower<K> {
    id: usize,
    key: K,
    listener: Listener,
}

struct Followers<K> {
    next: usize,
    list: Vec<Follower<K>>,
}

struct Inner<K> {
    owner: EntityId,
    window: AnyWindowHandle,
    orientation: Arrows,
    r#loop: bool,
    remember: bool,
    r#move: Option<RovingMoveFn<K>>,
    on_items_change: Option<ItemsChangeFn>,
    // Renders of the items: whether one is under way, and which.
    pass: u64,
    open: bool,
    // Items in page order, as they bound in the newest render.
    items: Vec<Entry<K>>,
    index: HashMap<K, usize>,
    // The items as the last settle saw them, to tell which joined or left.
    settled: Vec<(K, FocusHandle)>,
    stop: Option<K>,
    // The item focus was on last, which a group that remembers keeps the
    // stop on.
    remembered: Option<K>,
    pending: Option<Pending<K>>,
    // The item that has focus, which keeps it until focus leaves it.
    holder: Option<K>,
    claim: Claim<K>,
    handles: HashMap<K, FocusHandle>,
    bounds: HashMap<K, Bounds<Pixels>>,
    queued: bool,
}

// What a change asks of the world outside the group, run once the group's
// state is put away.
struct Effects<K> {
    notify: bool,
    focus: Option<FocusHandle>,
    moves: Vec<(Option<K>, Option<K>)>,
    items_changed: bool,
}

impl<K> Default for Effects<K> {
    fn default() -> Self {
        Self {
            notify: false,
            focus: None,
            moves: Vec::new(),
            items_changed: false,
        }
    }
}

// Whether focus is still where a move left it: on the element it left, or
// nowhere, or on an element that's gone, which drops focus to the window.
fn untouched(active: Option<&FocusHandle>, from: Option<&FocusHandle>, window: &Window) -> bool {
    match active {
        None => true,
        Some(active) => Some(active) == from || !active.is_rendered(window),
    }
}

impl<K: Clone + Eq + Hash + 'static> Inner<K> {
    // Starts a render of the items unless one is under way. Says whether it
    // did, so the render's settling is asked for once.
    fn open_pass(&mut self) -> bool {
        if self.open {
            return false;
        }
        self.open = true;
        self.pass += 1;
        self.items.clear();
        self.index.clear();
        true
    }

    fn entry(&self, key: &K) -> Option<&Entry<K>> {
        self.index.get(key).map(|&index| &self.items[index])
    }

    fn item(&self, entry: &Entry<K>) -> RovingFocusItem<K> {
        RovingFocusItem {
            key: entry.key.clone(),
            props: entry.props,
            handle: entry.handle.clone(),
            bounds: self.bounds.get(&entry.key).copied(),
        }
    }

    fn tab_index(&self, key: &K, props: &RovingMember) -> bool {
        match &self.stop {
            Some(stop) => stop == key,
            None => {
                let eligible = props.claims.unwrap_or(!props.disabled && props.focusable);
                self.claim.claim(key, eligible)
            }
        }
    }

    fn set_stop(&mut self, stop: Option<K>, fx: &mut Effects<K>) {
        if self.stop == stop {
            return;
        }
        let old = std::mem::replace(&mut self.stop, stop.clone());
        fx.notify = true;
        fx.moves.push((old, stop));
    }

    // Where the group's rules put the stop, with `preferred` saying which
    // items are preferred, or `None` while they can't decide it.
    fn stop_by_rules(&self, preferred: impl Fn(usize) -> bool) -> Option<Option<K>> {
        if self.items.is_empty() {
            return None;
        }
        // The stop already sits on an item about to render; the items there
        // are now don't decide it.
        if let Some(pending) = &self.pending
            && !self
                .entry(&pending.key)
                .is_some_and(|entry| entry.props.focusable)
        {
            return None;
        }
        let candidates: Vec<RovingCandidate<K>> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, entry)| RovingCandidate {
                key: entry.key.clone(),
                disabled: entry.props.disabled,
                focusable: entry.props.focusable,
                preferred: preferred(index),
            })
            .collect();
        Some(roving_stop(&candidates, self.remembered.as_ref()))
    }

    // Moves the stop to where the group's rules put it now.
    fn check(&mut self, fx: &mut Effects<K>) {
        if let Some(stop) = self.stop_by_rules(|index| self.items[index].props.preferred) {
            self.set_stop(stop, fx);
        }
    }

    // Focus landed on an item: it holds focus, and with `remember` the stop.
    fn landed(&mut self, key: K, fx: &mut Effects<K>) {
        self.holder = Some(key.clone());
        if self.remember {
            self.remembered = Some(key.clone());
            self.set_stop(Some(key), fx);
        }
    }

    // Focuses the item a move was waiting for, once it can take focus.
    fn focus_pending(&mut self, window: &Window, cx: &App, fx: &mut Effects<K>) {
        let Some(pending) = &self.pending else {
            return;
        };
        let Some(entry) = self.entry(&pending.key) else {
            return;
        };
        if !entry.props.focusable {
            return;
        }
        let active = window.focused(cx);
        if untouched(active.as_ref(), pending.from.as_ref(), window) {
            // One whose state just made it focusable may not have rendered.
            if !entry.handle.is_rendered(window) {
                return;
            }
            fx.focus = Some(entry.handle.clone());
            let key = entry.key.clone();
            self.landed(key, fx);
        }
        self.pending = None;
    }

    fn settle(&mut self, window: &Window, cx: &App) -> Effects<K> {
        let mut fx = Effects::default();
        if !self.open {
            return fx;
        }
        self.open = false;
        let pass = self.pass;
        let active = window.focused(cx);

        // The item with the stop that left with focus may come back under a
        // new key with the same element, like an item whose value changed
        // while it had focus. It keeps the stop it had, and takes focus back.
        let rejoined = match &self.pending {
            Some(Pending {
                key,
                from: Some(from),
                wait: Wait::Left,
                ..
            }) if !self.index.contains_key(key) => self
                .items
                .iter()
                .find(|entry| &entry.handle == from)
                .map(|entry| (key.clone(), entry.key.clone(), entry.handle.is_focused(window))),
            _ => None,
        };
        if let Some((was, key, focused)) = rejoined {
            if let Some(pending) = &mut self.pending {
                pending.key = key.clone();
            }
            if self.remembered.as_ref() == Some(&was) {
                self.remembered = Some(key.clone());
            }
            if self.stop.as_ref() == Some(&was) {
                self.set_stop(Some(key.clone()), &mut fx);
            }
            if focused {
                self.holder = Some(key);
            }
        }

        // Items that left. One removed with focus fires no blur, and focus
        // drops. The item with the stop may render again, like a card an undo
        // moves to another column: focus follows it there if it does by the
        // next render, unless a move already sends focus elsewhere.
        let mut changed = self.settled.len() != self.items.len();
        let left: Vec<(K, FocusHandle)> = self
            .settled
            .iter()
            .filter(|(key, _)| !self.index.contains_key(key))
            .cloned()
            .collect();
        for (key, handle) in left {
            changed = true;
            let held = self.holder.as_ref() == Some(&key);
            let had = handle.is_focused(window) || (held && active.is_none());
            if held {
                self.holder = None;
            }
            if had && self.pending.is_none() && self.stop.as_ref() == Some(&key) {
                self.pending = Some(Pending {
                    key,
                    from: Some(handle),
                    wait: Wait::Left,
                    pass,
                });
                // Renders again, so the wait ends by the next settle.
                fx.notify = true;
            }
        }

        // Focus landing on an item, from anywhere, or leaving it.
        let on = self
            .items
            .iter()
            .find(|entry| entry.handle.is_focused(window))
            .map(|entry| entry.key.clone());
        if on != self.holder {
            match on {
                Some(key) => self.landed(key, &mut fx),
                None => self.holder = None,
            }
        }

        // What the original runs a microtask after items bind.
        self.check(&mut fx);
        self.focus_pending(window, cx, &mut fx);
        fx.items_changed = changed;

        // And what its timers run a task later: frameworks render a key
        // press's update before then, so an item that can't take focus by now
        // never will, and an item that left with the stop and isn't back
        // leaves it to the rules.
        if self.pending.as_ref().is_some_and(|pending| pending.pass < pass)
            && let Some(pending) = self.pending.take()
        {
            if let Wait::Move { before } = pending.wait
                && self.remembered.as_ref() == Some(&pending.key)
            {
                self.remembered = before;
            }
            self.check(&mut fx);
        }

        self.settled = self
            .items
            .iter()
            .map(|entry| (entry.key.clone(), entry.handle.clone()))
            .collect();
        let pending = self.pending.as_ref().map(|pending| pending.key.clone());
        self.handles
            .retain(|key, _| self.index.contains_key(key) || pending.as_ref() == Some(key));
        self.bounds.retain(|key, _| self.index.contains_key(key));
        fx
    }
}

fn notify_followers<K: PartialEq>(
    followers: &RefCell<Followers<K>>,
    moves: &[(Option<K>, Option<K>)],
    cx: &mut App,
) {
    for (old, new) in moves {
        let listeners: Vec<Listener> = followers
            .borrow()
            .list
            .iter()
            .filter(|follower| {
                old.is_none() || old.as_ref() == Some(&follower.key) || new.as_ref() == Some(&follower.key)
            })
            .map(|follower| follower.listener.clone())
            .collect();
        for listener in listeners {
            listener(cx);
        }
    }
}

// The default `move`: along `orientation` with the arrows, Home and End.
fn step_move<K: Clone + PartialEq>(
    event: &KeyDownEvent,
    context: &RovingFocusMoveContext<K>,
    orientation: Arrows,
    r#loop: bool,
) -> Option<K> {
    let modifiers = event.keystroke.modifiers;
    if modifiers.control || modifiers.platform {
        return None;
    }
    let step = step_for(&Key::of(&event.keystroke), orientation, context.rtl)?;
    let index = context
        .items
        .iter()
        .position(|item| item.key == context.from.key)?;
    context
        .items
        .get(move_index(index as isize, step, context.items.len(), r#loop))
        .map(|item| item.key.clone())
}

impl<K: Clone + Eq + Hash + 'static> RovingFocus<K> {
    /// A roving group for a component; `cx` is the component's, which the
    /// group notifies when the stop moves so its parts render it, and
    /// `window` the one it renders in.
    pub fn new<T: 'static>(options: RovingFocusOptions<K>, window: &Window, cx: &Context<T>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                owner: cx.entity_id(),
                window: window.window_handle(),
                orientation: options.orientation,
                r#loop: options.r#loop,
                remember: options.remember,
                r#move: options.r#move,
                on_items_change: options.on_items_change,
                pass: 0,
                open: false,
                items: Vec::new(),
                index: HashMap::new(),
                settled: Vec::new(),
                stop: None,
                remembered: None,
                pending: None,
                holder: None,
                claim: Claim::default(),
                handles: HashMap::new(),
                bounds: HashMap::new(),
                queued: false,
            })),
            followers: Rc::new(RefCell::new(Followers {
                next: 0,
                list: Vec::new(),
            })),
        }
    }

    /// Hands it new options, as the component's own change, like its
    /// `orientation`, `loop` or `move` reading new state.
    pub fn set_options(&self, options: RovingFocusOptions<K>) {
        let mut inner = self.inner.borrow_mut();
        inner.orientation = options.orientation;
        inner.r#loop = options.r#loop;
        inner.remember = options.remember;
        inner.r#move = options.r#move;
        inner.on_items_change = options.on_items_change;
    }

    pub fn set_orientation(&self, orientation: impl Into<Arrows>) {
        self.inner.borrow_mut().orientation = orientation.into();
    }

    pub fn set_loop(&self, r#loop: bool) {
        self.inner.borrow_mut().r#loop = r#loop;
    }

    /// Which arrows move focus when there's no `move`.
    pub fn orientation(&self) -> Arrows {
        self.inner.borrow().orientation
    }

    /// Starts a render of the items, from the part that holds them, every
    /// frame it renders. Binding an item starts one too, so the order an
    /// app builds the parts in doesn't matter; this is what tells a group
    /// that renders no items that they've all left.
    pub fn begin(&self, cx: &App) {
        self.open(cx);
    }

    // Opens a render of the items, and asks to settle it once its frame is
    // drawn.
    fn open(&self, cx: &App) {
        let window = {
            let mut inner = self.inner.borrow_mut();
            if !inner.open_pass() {
                return;
            }
            inner.window
        };
        let this = self.clone();
        after_bind(window, cx, move |window, cx| this.settle(window, cx));
    }

    // Once the items have rendered: moves the stop where the rules put it,
    // notes focus landing on an item, focuses an item a move waits for, and
    // runs `on_items_change`.
    fn settle(&self, window: &mut Window, cx: &mut App) {
        let (fx, on_items_change) = {
            let Ok(mut inner) = self.inner.try_borrow_mut() else {
                return;
            };
            (inner.settle(window, cx), inner.on_items_change.clone())
        };
        if let Some(handle) = &fx.focus {
            window.focus(handle, cx);
        }
        let items_changed = fx.items_changed;
        self.apply(fx, cx);
        if items_changed && let Some(callback) = on_items_change {
            callback(window, cx);
        }
    }

    // Runs what a change in a handler asks for, at once.
    fn apply(&self, fx: Effects<K>, cx: &mut App) {
        let owner = self.inner.borrow().owner;
        if fx.notify {
            cx.notify(owner);
        }
        notify_followers(&self.followers, &fx.moves, cx);
    }

    /// The focus handle for an item, made the first time it's asked for and
    /// kept while the item renders. An item can bring its own handle to
    /// [`bind`](Self::bind) instead, like a member from another
    /// primitive.
    pub fn focus_handle(&self, key: &K, cx: &App) -> FocusHandle {
        self.inner
            .borrow_mut()
            .handles
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// Joins an item to the group as it renders, with the focus handle its
    /// element tracks and what it is now. Returns whether it holds the stop,
    /// for its handle's `tab_stop` (`tabIndex`). An item that stops rendering
    /// leaves the group. A key bound twice in one render keeps its first
    /// place.
    pub fn bind(&self, key: K, handle: &FocusHandle, props: RovingMember, cx: &App) -> bool {
        self.open(cx);
        let mut inner = self.inner.borrow_mut();
        match inner.index.get(&key).copied() {
            Some(index) => {
                let entry = &mut inner.items[index];
                entry.props = props;
                entry.handle = handle.clone();
            }
            None => {
                let index = inner.items.len();
                inner.index.insert(key.clone(), index);
                inner.items.push(Entry {
                    key: key.clone(),
                    props,
                    handle: handle.clone(),
                });
            }
        }
        inner.tab_index(&key, &props)
    }

    /// Whether an item holds the stop, without joining it: the stop's key,
    /// or before the group has decided it, the first item that claims it
    /// (`tabIndex`).
    pub fn tab_index(&self, key: &K, props: &RovingMember) -> bool {
        self.inner.borrow().tab_index(key, props)
    }

    /// Adds a layer to an item's element that records its bounds each
    /// layout, which [`RovingFocusItem::bounds`] reads, like `spatial_move`.
    pub fn measure<E: ParentElement + Styled>(&self, key: K, el: E) -> E {
        let inner: Weak<RefCell<Inner<K>>> = Rc::downgrade(&self.inner);
        el.relative().child(
            canvas(
                move |bounds, _window, _cx| {
                    if let Some(inner) = inner.upgrade()
                        && let Ok(mut inner) = inner.try_borrow_mut()
                    {
                        inner.bounds.insert(key, bounds);
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    /// Moves focus for a key pressed on the item `key`. Returns the key it
    /// moved to, the key it moved from and whether Shift was held, so the
    /// caller can extend a selection, or `None` when the key isn't the
    /// group's. At either end without `loop`, `key` is `from`. A key that's
    /// the group's calls `window.prevent_default()`, which later handlers
    /// check, as `defaultPrevented` in the original; one whose default was
    /// prevented already, or with Alt held, is left alone.
    pub fn keydown(
        &self,
        event: &KeyDownEvent,
        key: &K,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<RovingMoved<K>> {
        let (custom, orientation, r#loop) = {
            let inner = self.inner.borrow();
            (inner.r#move.clone(), inner.orientation, inner.r#loop)
        };
        self.keydown_with(
            event,
            key,
            |event, context, cx| match &custom {
                Some(custom) => custom(event, context, cx),
                None => step_move(event, context, orientation, r#loop),
            },
            window,
            cx,
        )
    }

    /// [`keydown`](Self::keydown) with a `move` for this press, in place of
    /// the option, which can read the component it's called in.
    pub fn keydown_with(
        &self,
        event: &KeyDownEvent,
        key: &K,
        r#move: impl FnOnce(&KeyDownEvent, &RovingFocusMoveContext<K>, &App) -> Option<K>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<RovingMoved<K>> {
        if window.default_prevented() || event.keystroke.modifiers.alt {
            return None;
        }
        let items = self.items();
        let from = items.iter().find(|item| &item.key == key)?.clone();
        let context = RovingFocusMoveContext {
            from,
            items,
            rtl: is_rtl(cx),
        };
        let to = r#move(event, &context, cx)?;
        window.prevent_default();
        let from = context.from;
        let next = context.items.into_iter().find(|item| item.key == to);
        // A key no item can take focus for yet, like next month's day,
        // renders before it does.
        let moved = next.is_some_and(|next| next.key == from.key || self.focus_on(&next, window, cx));
        if !moved {
            self.move_to(to.clone(), Some(from.handle.clone()), cx);
        }
        Some(RovingMoved {
            key: to,
            from: from.key,
            shift: event.keystroke.modifiers.shift,
        })
    }

    // Focuses an item's element, and says whether it took focus: one that
    // hasn't rendered yet can't.
    fn focus_on(&self, item: &RovingFocusItem<K>, window: &mut Window, cx: &mut App) -> bool {
        if !item.handle.is_rendered(window) {
            return false;
        }
        window.focus(&item.handle, cx);
        if !item.handle.is_focused(window) {
            return false;
        }
        let fx = {
            let mut fx = Effects::default();
            self.inner.borrow_mut().landed(item.key.clone(), &mut fx);
            fx
        };
        self.apply(fx, cx);
        true
    }

    // Moves the stop to an item that hasn't rendered yet, or can't take
    // focus yet, and focus with it once it can. Frameworks render a key
    // press's update before the next frame, so an item that can't by the
    // render after this never will, and the group's rules take over.
    fn move_to(&self, key: K, from: Option<FocusHandle>, cx: &mut App) {
        let fx = {
            let mut inner = self.inner.borrow_mut();
            let before = inner.remembered.clone();
            if inner.remember {
                inner.remembered = Some(key.clone());
            }
            let pass = inner.pass;
            inner.pending = Some(Pending {
                key: key.clone(),
                from,
                wait: Wait::Move { before },
                pass,
            });
            let mut fx = Effects::default();
            inner.set_stop(Some(key), &mut fx);
            // Renders again, so the move ends by the next settle.
            fx.notify = true;
            fx
        };
        self.apply(fx, cx);
    }

    /// The items that can take focus now, in page order, with their handles.
    pub fn items(&self) -> Vec<RovingFocusItem<K>> {
        let inner = self.inner.borrow();
        inner
            .items
            .iter()
            .filter(|entry| entry.props.focusable)
            .map(|entry| inner.item(entry))
            .collect()
    }

    /// The item that has focus now, if one does.
    pub fn focused(&self, window: &Window) -> Option<K> {
        let inner = self.inner.borrow();
        inner
            .items
            .iter()
            .find(|entry| entry.handle.is_focused(window))
            .map(|entry| entry.key.clone())
    }

    /// Focuses an item, or with `None` the one with the stop, or the first.
    /// An item that isn't rendered or focusable yet takes the stop now and
    /// focus once it can.
    pub fn focus(&self, key: Option<&K>, window: &mut Window, cx: &mut App) {
        let Some(key) = key else {
            let target = {
                let inner = self.inner.borrow();
                inner
                    .stop
                    .as_ref()
                    .and_then(|stop| inner.entry(stop))
                    .or(inner.items.first())
                    .map(|entry| inner.item(entry))
            };
            if let Some(target) = target {
                self.focus_on(&target, window, cx);
            }
            return;
        };
        let item = {
            let inner = self.inner.borrow();
            inner.entry(key).map(|entry| inner.item(entry))
        };
        if let Some(item) = item
            && item.props.focusable
            && self.focus_on(&item, window, cx)
        {
            return;
        }
        self.move_to(key.clone(), window.focused(cx), cx);
    }

    /// Moves the stop to an item as if focus had landed on it, for an item
    /// whose focus went elsewhere, like a menubar trigger whose menu took
    /// it. A group that doesn't `remember` ignores it, since its stop follows
    /// the preferred item.
    pub fn remember(&self, key: &K, cx: &mut App) {
        let fx = {
            let mut inner = self.inner.borrow_mut();
            let mut fx = Effects::default();
            if inner.remember {
                inner.remembered = Some(key.clone());
                inner.set_stop(Some(key.clone()), &mut fx);
            }
            fx
        };
        self.apply(fx, cx);
    }

    /// Moves the stop to where the group's rules put it now. It also runs on
    /// its own once items render, so a change the items render with, like a
    /// selection or a branch expanding, moves it then.
    pub fn check(&self, cx: &mut App) {
        let fx = {
            let mut fx = Effects::default();
            self.inner.borrow_mut().check(&mut fx);
            fx
        };
        self.apply(fx, cx);
    }

    /// [`check`](Self::check) with which items are preferred now, in place
    /// of what they last rendered with, so a change the component makes,
    /// like a radio group's new value, moves the stop at once rather than
    /// once the items render with it. The items keep those until they render
    /// again. It can read the component it's called in, as the original's
    /// `check` reads its `preferred` callback.
    pub fn check_with(&self, preferred: impl Fn(&RovingFocusItem<K>) -> bool, cx: &mut App) {
        let fx = {
            let mut inner = self.inner.borrow_mut();
            let flags: Vec<bool> = inner
                .items
                .iter()
                .map(|entry| preferred(&inner.item(entry)))
                .collect();
            for (entry, flag) in inner.items.iter_mut().zip(flags) {
                entry.props.preferred = flag;
            }
            let mut fx = Effects::default();
            inner.check(&mut fx);
            fx
        };
        self.apply(fx, cx);
    }

    /// Checks the stop once after the current update, however often it's
    /// asked, for a change the group can't see.
    pub fn queue_check(&self, cx: &mut App) {
        if std::mem::replace(&mut self.inner.borrow_mut().queued, true) {
            return;
        }
        let this = self.clone();
        cx.defer(move |cx| {
            this.inner.borrow_mut().queued = false;
            this.check(cx);
        });
    }

    /// Where the group's rules would put the stop if `preferred` said which
    /// items are preferred, like the tab a value from options selects,
    /// without moving it. `None` while the items can't decide it, like when
    /// none is preferred yet: the item may be about to render, as when its
    /// value changed, and the group decides once it has. `Some(None)` when no
    /// item can take focus.
    pub fn stop_for(&self, preferred: impl Fn(&RovingFocusItem<K>) -> bool) -> Option<Option<K>> {
        let inner = self.inner.borrow();
        let items: Vec<RovingFocusItem<K>> =
            inner.items.iter().map(|entry| inner.item(entry)).collect();
        if !items.iter().any(&preferred) {
            return None;
        }
        inner.stop_by_rules(|index| preferred(&items[index]))
    }

    /// The key of the item that holds the stop, or `None` before the items
    /// have settled.
    pub fn stop(&self) -> Option<K> {
        self.inner.borrow().stop.clone()
    }

    /// Forgets the stop and the item focused last, so the group starts over,
    /// like a date picker opening on its value: the original's stop set back
    /// to `null`.
    pub fn reset(&self, cx: &mut App) {
        let fx = {
            let mut inner = self.inner.borrow_mut();
            let mut fx = Effects::default();
            inner.remembered = None;
            inner.set_stop(None, &mut fx);
            fx
        };
        self.apply(fx, cx);
    }

    pub(crate) fn follow(&self, key: K, listener: Listener) -> Subscription {
        let id = {
            let mut followers = self.followers.borrow_mut();
            let id = followers.next;
            followers.next += 1;
            followers.list.push(Follower { id, key, listener });
            id
        };
        let followers = Rc::downgrade(&self.followers);
        Subscription::new(move || {
            if let Some(followers) = followers.upgrade()
                && let Ok(mut followers) = followers.try_borrow_mut()
            {
                followers.list.retain(|follower| follower.id != id);
            }
        })
    }
}
