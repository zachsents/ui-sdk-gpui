//! Port of `foundation/focus/roving-scope.ts`: the scope a member from
//! another primitive joins a roving group through, like a toggle in its
//! toggle group or in the toolbar around it.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, FocusHandle, KeyDownEvent, SharedString, Subscription, Window};

use crate::roving_focus::{RovingFocus, RovingOrientation};

/// How an item takes part in a roving group: what it tells the group each
/// time it renders. The original's members, like a toggle joining its
/// toggle group's or its toolbar's group, pass functions the group calls;
/// here every item, a group's own or another primitive's, passes the values
/// as it renders, and the group decides from the newest ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RovingMember {
    /// Whether it can't be activated. Enabled items take the stop first;
    /// disabled ones can still take focus.
    pub disabled: bool,
    /// Whether it can take focus now. An item that can't stays in the group
    /// and is skipped, like a tree item under a collapsed branch. Defaults to
    /// `true`.
    pub focusable: bool,
    /// Whether it takes the stop before focus has been in the group: the
    /// selected tab, the checked radio, a pressed toggle.
    pub preferred: bool,
    /// Whether it may hold the stop while the group hasn't decided it yet,
    /// like on the first frame. The first such item does, so the frame has
    /// exactly one stop. `None` lets enabled, focusable items; set it when
    /// the stop should start on a selected item, like
    /// `value.is_none() || selected` (`claims` in the original).
    pub claims: Option<bool>,
}

impl Default for RovingMember {
    fn default() -> Self {
        Self {
            disabled: false,
            focusable: true,
            preferred: false,
            claims: None,
        }
    }
}

impl RovingMember {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn focusable(mut self, focusable: bool) -> Self {
        self.focusable = focusable;
        self
    }

    pub fn preferred(mut self, preferred: bool) -> Self {
        self.preferred = preferred;
        self
    }

    pub fn claims(mut self, claims: bool) -> Self {
        self.claims = Some(claims);
        self
    }
}

/// What a member from another primitive sees of a roving group, wherever
/// the group lives: a toggle's own group, or the toolbar that group sits in.
/// A group hands it to its members as an `Rc<dyn RovingScope>`; it
/// implements the first three methods, and the rest default to its
/// [`RovingFocus`].
pub trait RovingScope {
    /// The group members join.
    fn roving(&self) -> &RovingFocus<SharedString>;

    /// Which way the group runs.
    fn orientation(&self, cx: &App) -> RovingOrientation;

    /// Whether the whole group is disabled, like a disabled toolbar.
    fn disabled(&self, cx: &App) -> bool;

    /// Joins a member as it renders, with the focus handle its element
    /// tracks. Returns whether it holds the stop; before the group has
    /// decided it, the first member that claims it does (`claim`).
    fn bind(
        &self,
        key: SharedString,
        handle: &FocusHandle,
        member: RovingMember,
        cx: &App,
    ) -> bool {
        self.roving().bind(key, handle, member, cx)
    }

    /// Whether a member holds the stop, without joining. The group decides
    /// which members are eligible before it knows the stop.
    fn claim(&self, key: &SharedString, member: &RovingMember) -> bool {
        self.roving().tab_index(key, member)
    }

    /// Checks the stop after a change the group can't see, like a toggle's
    /// own disabled.
    fn queue_check(&self, cx: &mut App) {
        self.roving().queue_check(cx);
    }

    /// Moves focus for a key pressed on a member.
    fn keydown(&self, event: &KeyDownEvent, key: &SharedString, window: &mut Window, cx: &mut App) {
        self.roving().keydown(event, key, window, cx);
    }

    /// The key that holds the stop, or `None` until members render.
    fn stop(&self) -> Option<SharedString> {
        self.roving().stop()
    }

    /// Runs `listener` on the stop moves a member with this key can show.
    /// See [`follow_stop`].
    fn follow(&self, key: SharedString, listener: Rc<dyn Fn(&mut App)>) -> Subscription {
        follow_stop(self.roving(), key, move |cx| listener(cx))
    }
}

/// Runs `run` once, after the current update, however many times it's
/// called before then, so many members changing at once check the stop
/// once.
pub fn queued(run: impl Fn(&mut App) + 'static) -> Rc<dyn Fn(&mut App)> {
    let waiting = Rc::new(Cell::new(false));
    let run = Rc::new(run);
    Rc::new(move |cx: &mut App| {
        if waiting.replace(true) {
            return;
        }
        let waiting = waiting.clone();
        let run = run.clone();
        cx.defer(move |cx| {
            waiting.set(false);
            run(cx);
        });
    })
}

/// Runs `listener` when a group's stop moves to or from `key`, or from
/// `None`, which members read before the group decides. An arrow key then
/// reaches the two members it moves the stop between, not every member.
/// Members render from the group anyway; this is for one that keeps what it
/// shows elsewhere, or must hear the move itself. Drop the subscription to
/// stop.
pub fn follow_stop<K: Clone + Eq + std::hash::Hash + 'static>(
    roving: &RovingFocus<K>,
    key: K,
    listener: impl Fn(&mut App) + 'static,
) -> Subscription {
    roving.follow(key, Rc::new(listener))
}
