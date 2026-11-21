//! Port of `components/toast/toast-api.ts`: a toast region's state and
//! behavior: what's on screen, for how long, how it's announced and where
//! focus goes. The parts are in [`toast_parts`](crate::toast_parts).

use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Context, FocusHandle, KeyDownEvent, Pixels, Point, SharedString, Subscription, Window,
};
use ui_sdk_drag_and_drop::PointerEvent;
use ui_sdk_focus::tabbables;
use ui_sdk_kernel::action::ActionHandlers;
use ui_sdk_kernel::change::log_with;
use ui_sdk_kernel::part::held;
use ui_sdk_kernel::{
    ActionDetails, ActionOutcome, InputEvent, Key, Reason, change_source, run_action,
};
use ui_sdk_motion::{Presence, PresenceTiming};
use ui_sdk_platform::{
    AnnouncePriority, PersistentLayer, Region, announce, announce_with, bind_persistent_layer,
    is_rtl, prepare_announcements,
};
use ui_sdk_shortcuts::{RegisteredShortcut, ShortcutOptions, register_shortcut};
use ui_sdk_text::js_trim;

use crate::swipe::{SwipeCallbacks, SwipeDirection, track_swipe};
use crate::toast::{ButtonKey, ToastButtonState, ToastItem, ToastRegion, detached};
use crate::toast_dom::{
    Attention, AttentionReport, Away, StackToast, focus_after_close, natural_height, stack_layout,
    swipe_directions, watch_attention,
};
use crate::toast_timers::{TimedToast, ToastPauseReason};
use crate::toaster::{RegionAttachment, Toast, ToastActionHandler};

/// The item for a toast, if the region shows it.
pub fn find_item<'a>(items: &'a [ToastItem], id: &str) -> Option<&'a ToastItem> {
    items.iter().find(|item| item.toast.id.as_ref() == id)
}

/// Whether a region has toasts to show, so it's a landmark and its shortcut
/// works.
pub fn has_toasts(items: &[ToastItem]) -> bool {
    items.iter().any(|item| !item.leaving)
}

/// What the region keeps for one toast it shows: the focus its root takes,
/// the root as containment sees it, kept live over modal layers while it's on
/// the page, its enter and exit, a swipe, and its offset as last written.
pub(crate) struct ToastEntry {
    pub(crate) handle: FocusHandle,
    pub(crate) area: Region,
    pub(crate) presence: Presence,
    pub(crate) persistent: Option<PersistentLayer>,
    pub(crate) swiping: bool,
    pub(crate) swipe: Option<Point<Pixels>>,
    pub(crate) swipe_direction: Option<SwipeDirection>,
    pub(crate) offset: Option<Pixels>,
}

impl ToastEntry {
    fn new(timing: PresenceTiming, cx: &App) -> Self {
        let handle = cx.focus_handle();
        Self {
            area: Region::with_focus_handle(handle.clone()),
            handle,
            presence: Presence::new(false).with_timing(timing),
            persistent: None,
            swiping: false,
            swipe: None,
            swipe_direction: None,
            offset: None,
        }
    }
}

// The toaster the region follows, and its count of the region.
struct Following {
    _changes: Subscription,
    attachment: RegionAttachment,
}

/// What a region holds while it's on the page (`bindRegion`): its toaster,
/// its place over modal layers, the user's attention and its shortcut, with
/// the keys it registered.
pub(crate) struct RegionBinding {
    following: Option<Following>,
    _persistent: PersistentLayer,
    _attention: Attention,
    shortcut: Option<(SharedString, RegisteredShortcut)>,
}

// A function key, like F6, types nothing (`/^F\d+$/i`).
fn is_function_key(keys: &str) -> bool {
    let mut chars = keys.chars();
    matches!(chars.next(), Some('F' | 'f'))
        && !chars.as_str().is_empty()
        && chars.as_str().chars().all(|char| char.is_ascii_digit())
}

impl RegionBinding {
    /// Lets go of all of it, uncounting the region from its toaster.
    pub(crate) fn release(self, cx: &mut App) {
        if let Some(following) = self.following {
            following.attachment.release(cx);
        }
    }
}

impl ToastRegion {
    /// Closes a toast for a reason, unless its `on_dismiss` cancels. Returns
    /// whether it closed.
    pub(crate) fn close_toast(
        &mut self,
        id: &str,
        reason: Reason,
        event: Option<InputEvent>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.toaster.clone().close(id, reason, event, cx)
    }

    fn any_pending(&self) -> bool {
        self.buttons.values().any(|button| button.pending)
    }

    /// Takes what its toaster holds now: the toasts to show, the ones
    /// leaving, their timers and what to announce.
    pub(crate) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let queued: Vec<Toast> = self.toaster.toasts(cx).to_vec();
        let visible: Vec<Toast> = queued.iter().take(self.max_visible).cloned().collect();
        let shown: HashSet<&SharedString> = visible.iter().map(|toast| &toast.id).collect();
        // Toasts that were on screen and no longer are animate out where they
        // were.
        let leaving: Vec<ToastItem> = self
            .items
            .iter()
            .filter(|item| !shown.contains(&item.toast.id))
            .map(|item| ToastItem {
                leaving: true,
                ..item.clone()
            })
            .collect();
        let mut items: Vec<ToastItem> = visible
            .iter()
            .enumerate()
            .map(|(index, toast)| ToastItem {
                toast: toast.clone(),
                index,
                leaving: false,
            })
            .chain(leaving)
            .collect();
        items.sort_by(|a, b| b.toast.order.cmp(&a.toast.order));
        self.items = items;
        self.enter(window, cx);
        self.sync_presence(window, cx);
        self.update_timers(cx);
        for toast in &visible {
            if self.announced.get(&toast.id) == Some(&toast.version) {
                continue;
            }
            self.announced.insert(toast.id.clone(), toast.version);
            self.to_announce.insert(toast.id.clone());
        }
        let ids: HashSet<&SharedString> = queued.iter().map(|toast| &toast.id).collect();
        self.announced.retain(|id, _| ids.contains(id));
        if !self.to_announce.is_empty() {
            self.schedule_announcement(window, cx);
        }
        cx.notify();
    }

    // Toasts that just came on screen animate in, as their elements do as
    // they bind.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fresh: Vec<SharedString> = self
            .items
            .iter()
            .filter(|item| !item.leaving && !self.entries.contains_key(&item.toast.id))
            .map(|item| item.toast.id.clone())
            .collect();
        for id in fresh {
            let entry = ToastEntry::new(self.timing.clone(), cx);
            entry.presence.animate_in(window, cx);
            self.entries.insert(id, entry);
        }
    }

    pub(crate) fn update_timers(&mut self, cx: &mut Context<Self>) {
        let visible: Vec<TimedToast> = self
            .items
            .iter()
            .filter(|item| !item.leaving)
            .map(|item| TimedToast {
                id: item.toast.id.clone(),
                version: item.toast.version,
                duration: item.toast.duration,
                held: self.held.contains(&item.toast.id),
            })
            .collect();
        let queued: HashSet<SharedString> = self
            .toaster
            .toasts(cx)
            .iter()
            .map(|toast| toast.id.clone())
            .collect();
        let timers = self.timers.clone();
        timers.update(visible, &queued, cx);
    }

    // What a screen reader hears for a toast: the text of its title and
    // description parts that are on the page, else both. gpui can't read
    // back what an element shows, so a part's text is the toast's own.
    fn text_for(&self, toast: &Toast) -> String {
        let parts: Vec<Option<&SharedString>> =
            [("title", &toast.title), ("description", &toast.description)]
                .into_iter()
                .filter(|(slot, _)| held(&self.instance, slot, Some(toast.id.as_ref())))
                .map(|(_, text)| text.as_ref())
                .collect();
        if !parts.is_empty() {
            return parts
                .into_iter()
                .flatten()
                .map(|text| js_trim(text))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(". ");
        }
        [&toast.title, &toast.description]
            .into_iter()
            .flatten()
            .map(|text| text.as_ref())
            .collect::<Vec<_>>()
            .join(". ")
    }

    // Toasts that arrive close together are read together: each one waits a
    // little for the next, up to a limit, so a burst is one announcement and
    // the first is never replaced before it's read.
    fn schedule_announcement(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let since = *self.waiting_since.get_or_insert(now);
        let delay = (since + Duration::from_millis(400))
            .saturating_duration_since(now)
            .min(Duration::from_millis(100));
        self.announce_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| this.flush_announcements(window, cx))
                .ok();
        }));
    }

    // Toasts that arrive together are read together, each priority once.
    fn flush_announcements(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // This runs inside the timer's own task, which ends with it.
        if let Some(task) = self.announce_timer.take() {
            task.detach();
        }
        self.waiting_since = None;
        let mut polite = Vec::new();
        let mut assertive = Vec::new();
        for item in &self.items {
            if item.leaving || !self.to_announce.contains(&item.toast.id) {
                continue;
            }
            let text = self.text_for(&item.toast);
            if text.is_empty() {
                continue;
            }
            match item.toast.priority {
                AnnouncePriority::Polite => polite.push(text),
                AnnouncePriority::Assertive => assertive.push(text),
            }
        }
        self.to_announce.clear();
        for (priority, texts) in [
            (AnnouncePriority::Assertive, assertive),
            (AnnouncePriority::Polite, polite),
        ] {
            if !texts.is_empty() {
                announce_with(&texts.join(". "), priority, window, cx);
            }
        }
    }

    // A toast's exit finished, or it had none to run.
    pub(crate) fn finish(
        &mut self,
        id: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.exits.remove(id);
        let before = self.items.len();
        self.items
            .retain(|item| !(item.leaving && &item.toast.id == id));
        let changed = self.items.len() != before;
        if find_item(&self.items, id).is_none() {
            self.drop_buttons(id, window, cx);
            self.forget(id);
        }
        if changed {
            self.sync_presence(window, cx);
            cx.notify();
        }
    }

    // A toast gone from the region for good: what the region kept for it.
    fn forget(&mut self, id: &SharedString) {
        self.entries.remove(id);
        self.heights.remove(id);
        self.button_handles
            .borrow_mut()
            .retain(|key, _| &key.toast != id);
    }

    /// Starts and cancels exit animations as toasts leave and come back.
    pub(crate) fn sync_presence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot: Vec<(SharedString, bool)> = self
            .items
            .iter()
            .map(|item| (item.toast.id.clone(), item.leaving))
            .collect();
        for (id, leaving) in snapshot {
            // An exit that finished at once may have taken it away already.
            if !self
                .items
                .iter()
                .any(|item| item.toast.id == id && item.leaving == leaving)
            {
                continue;
            }
            let running = self.exits.contains(&id);
            if !leaving {
                if running {
                    self.exits.remove(&id);
                    // It came back: the exit stops, and it eases back to its
                    // open look from where the exit left it.
                    if let Some(entry) = self.entries.get(&id) {
                        entry.presence.stop(cx);
                    }
                }
                continue;
            }
            if running {
                continue;
            }
            let rendered = held(&self.instance, "root", Some(id.as_ref()));
            let presence = self
                .entries
                .get(&id)
                .filter(|_| rendered)
                .map(|entry| entry.presence.clone());
            let Some(presence) = presence else {
                self.exits.insert(id.clone());
                cx.defer_in(window, move |this, window, cx| this.finish(&id, window, cx));
                continue;
            };
            let focused = self
                .entries
                .get(&id)
                .is_some_and(|entry| entry.handle.contains_focused(window, cx));
            if focused {
                self.move_focus_from(&id, window, cx);
            }
            self.exits.insert(id.clone());
            presence.animate_out(
                self,
                move |this, window, cx| this.finish(&id, window, cx),
                window,
                cx,
            );
        }
        self.layout();
    }

    // The toasts whose root is on the page, in the order the region shows
    // them, newest first, which is the order the app renders them in.
    fn rendered_roots(&self) -> Vec<SharedString> {
        self.items
            .iter()
            .filter(|item| held(&self.instance, "root", Some(item.toast.id.as_ref())))
            .map(|item| item.toast.id.clone())
            .collect()
    }

    // Focus never falls to nothing when the focused toast closes.
    fn move_focus_from(
        &mut self,
        closing: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open: Vec<FocusHandle> = self
            .rendered_roots()
            .into_iter()
            .filter(|key| {
                key == closing || !find_item(&self.items, key).is_some_and(|item| item.leaving)
            })
            .filter_map(|key| self.entries.get(&key).map(|entry| entry.handle.clone()))
            .collect();
        let Some(closing) = self.entries.get(closing).map(|entry| entry.handle.clone()) else {
            return;
        };
        let fallback = self.connected_return_target(window);
        if let Some(target) = focus_after_close(&open, &closing, fallback) {
            window.focus(&target, cx);
        }
    }

    // The stack's measurements, written where the parts read them.
    pub(crate) fn layout(&mut self) {
        let toasts: Vec<StackToast> = self
            .items
            .iter()
            .map(|item| StackToast {
                id: item.toast.id.clone(),
                index: item.index,
                leaving: item.leaving,
            })
            .collect();
        let stack = stack_layout(&toasts, &self.heights);
        // Kept until a new front toast is measured, so the toasts behind it
        // don't collapse for a frame.
        if let Some(front) = stack.front {
            self.stack.front_height = Some(front);
        }
        self.stack.stack_height = stack.total;
        self.stack.count = stack.layout.len();
        for (id, place) in &stack.layout {
            if let Some(entry) = self.entries.get_mut(id) {
                entry.offset = Some(place.offset);
            }
        }
    }

    // Sets or clears a button's pending or failed state. An entry goes once
    // its press succeeds or its toast has left.
    fn set_button(
        &mut self,
        key: &ButtonKey,
        next: Option<ToastButtonState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match next {
            Some(state) => {
                self.buttons.insert(key.clone(), state);
            }
            None => {
                self.buttons.remove(key);
            }
        }
        self.buttons_changed(window, cx);
    }

    fn drop_buttons(&mut self, toast: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.buttons.len();
        self.buttons.retain(|key, _| &key.toast != toast);
        if self.buttons.len() != before {
            self.buttons_changed(window, cx);
        }
    }

    // A slow action says so once it has been pending for a second.
    fn buttons_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = self.any_pending();
        let text = self.pending_announcement.clone();
        self.announce_pending
            .update(pending, text, announce, window, cx);
        cx.notify();
    }

    fn set_held(&mut self, toast: &SharedString, pending: bool, cx: &mut Context<Self>) {
        if pending {
            self.held.insert(toast.clone());
        } else {
            self.held.remove(toast);
        }
        self.update_timers(cx);
    }

    /// Moves focus to the newest toast, or back where it was when focus is
    /// already in the region. Returns whether it moved.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !held(&self.instance, "region", None) {
            return false;
        }
        let active = window.focused(cx);
        if active
            .as_ref()
            .is_some_and(|active| self.focus_handle.contains(active, window))
        {
            self.return_focus(window, cx);
            return true;
        }
        let first = self
            .rendered_roots()
            .into_iter()
            .filter(|key| !find_item(&self.items, key).is_some_and(|item| item.leaving))
            .find_map(|key| self.entries.get(&key).map(|entry| entry.handle.clone()));
        let Some(first) = first else {
            return false;
        };
        self.return_target = active.map(|active| active.downgrade());
        self.entered_by_shortcut = true;
        window.focus(&first, cx);
        true
    }

    // Where focus was before it came into the region, while it's still on
    // the page.
    fn connected_return_target(&self, window: &Window) -> Option<FocusHandle> {
        self.return_target
            .as_ref()
            .and_then(|target| target.upgrade())
            .filter(|target| target.is_rendered(window))
    }

    fn return_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.entered_by_shortcut = false;
        if let Some(target) = self.connected_return_target(window) {
            window.focus(&target, cx);
        }
    }

    /// Pauses every timer in the region for a reason, until `resume` clears
    /// it. Other reasons keep their own hold.
    pub fn pause(&mut self, reason: impl Into<ToastPauseReason>, cx: &mut Context<Self>) {
        let timers = self.timers.clone();
        timers.pause(reason.into(), cx);
    }

    /// Clears one pause reason. Timers run again once no reason is left.
    pub fn resume(&mut self, reason: impl Into<ToastPauseReason>, cx: &mut Context<Self>) {
        let timers = self.timers.clone();
        timers.resume(&reason.into(), cx);
    }

    /// Whether anything pauses the region's timers now.
    pub fn is_paused(&self) -> bool {
        self.timers.is_paused()
    }

    /// How long a toast has left before it closes on its own, for devtools.
    /// `Duration::MAX` for one that stays until it's dismissed.
    pub fn remaining(&self, id: &str) -> Option<Duration> {
        self.timers.remaining(id)
    }

    // The region part came onto the page or left it.
    pub(crate) fn check_bound(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bound = held(&self.instance, "region", None);
        match (bound, self.binding.is_some()) {
            (true, false) => self.bind_region(window, cx),
            (false, true) => self.unbind_region(cx),
            _ => {}
        }
    }

    fn bind_region(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The live regions exist before the first toast, so it's read.
        prepare_announcements(window);
        // Over every layer, so it stays usable while a modal dialog is open.
        let persistent = bind_persistent_layer(self.area.clone(), window);
        let region = self.focus_handle.clone();
        // The user's attention: each kind pauses the timers on its own.
        let attention = watch_attention(
            self,
            &region,
            AttentionReport {
                focus: Rc::new(|this: &mut Self, focused, from, _window, cx| {
                    this.focus_changed(focused, from, cx)
                }),
                away: Rc::new(|this: &mut Self, away, gone, _window, cx| {
                    let reason = match away {
                        Away::WindowBlur => ToastPauseReason::WindowBlur,
                        Away::PageHide => ToastPauseReason::PageHide,
                    };
                    if gone {
                        this.pause(reason, cx);
                    } else {
                        this.resume(reason, cx);
                    }
                }),
            },
            window,
            cx,
        );
        self.binding = Some(RegionBinding {
            following: None,
            _persistent: persistent,
            _attention: attention,
            shortcut: None,
        });
        // Toasts shown again animate in, as their elements do when they bind
        // again.
        let shown: Vec<Presence> = self
            .items
            .iter()
            .filter(|item| !item.leaving)
            .filter_map(|item| self.entries.get(&item.toast.id))
            .map(|entry| entry.presence.clone())
            .collect();
        for presence in shown {
            presence.animate_in(window, cx);
        }
        self.connect(window, cx);
        self.sync_shortcut(cx);
        self.layout();
        cx.notify();
    }

    fn unbind_region(&mut self, cx: &mut Context<Self>) {
        if let Some(binding) = self.binding.take() {
            binding.release(cx);
        }
        self.pause(detached(), cx);
        for reason in [
            ToastPauseReason::ItemHover,
            ToastPauseReason::FocusIn,
            ToastPauseReason::WindowBlur,
            ToastPauseReason::PageHide,
        ] {
            self.resume(reason, cx);
        }
        self.hovered = false;
        self.focused = false;
        cx.notify();
    }

    // Focus came into the region, from `from`, or left it.
    fn focus_changed(&mut self, focused: bool, from: Option<FocusHandle>, cx: &mut Context<Self>) {
        if focused
            && !self.focused
            && let Some(from) = from
        {
            self.return_target = Some(from.downgrade());
        }
        if !focused {
            self.entered_by_shortcut = false;
        }
        self.focused = focused;
        if focused {
            self.pause(ToastPauseReason::FocusIn, cx);
        } else {
            self.resume(ToastPauseReason::FocusIn, cx);
        }
        cx.notify();
    }

    // The pointer over the region or a toast in it, or off it. Touch doesn't
    // hover.
    pub(crate) fn set_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.hovered == hovered {
            return;
        }
        self.hovered = hovered;
        if hovered {
            self.pause(ToastPauseReason::ItemHover, cx);
        } else {
            self.resume(ToastPauseReason::ItemHover, cx);
        }
        cx.notify();
    }

    // Whether the pointer at `position` is over the region or a toast that
    // reaches out of it.
    fn hovering_at(&self, position: Point<Pixels>, window: &Window) -> bool {
        self.area.contains_point(position, window)
            || self
                .entries
                .values()
                .any(|entry| entry.area.contains_point(position, window))
    }

    pub(crate) fn pointer_moved(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pointer_out = false;
        if self.binding.is_none() || window.last_input_was_touch() {
            return;
        }
        let hovering = self.hovering_at(position, window);
        self.set_hovered(hovering, cx);
    }

    pub(crate) fn pointer_left(&mut self, cx: &mut Context<Self>) {
        self.pointer_out = true;
        self.set_hovered(false, cx);
    }

    /// What the region learns from each frame it draws: the pointer over a
    /// toast that moved under it, the toasts' heights, and which toasts are
    /// on the page, which stay usable over modal layers.
    pub(crate) fn after_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.binding.is_none() {
            return;
        }
        if !self.pointer_out && !window.last_input_was_touch() {
            let hovering = self.hovering_at(window.mouse_position(), window);
            self.set_hovered(hovering, cx);
        }
        // A toast's own height is the stack's, so its content tells when it
        // reflows.
        let expanded = self.is_expanded();
        let parts = self.instance.bound_parts();
        let mut changed = false;
        for item in &self.items {
            if item.leaving || !(expanded || item.index == 0) {
                continue;
            }
            let Some(height) = natural_height(&parts, &item.toast.id) else {
                continue;
            };
            if self.heights.get(&item.toast.id) != Some(&height) {
                self.heights.insert(item.toast.id.clone(), height);
                changed = true;
            }
        }
        if changed {
            self.layout();
            cx.notify();
        }
        // A toast on the page stays usable over modal layers; one leaving lets
        // presses through to what's beneath.
        for item in &self.items {
            let id = &item.toast.id;
            let Some(entry) = self.entries.get_mut(id) else {
                continue;
            };
            if item.leaving {
                entry.persistent = None;
            } else if entry.persistent.is_none() && held(&self.instance, "root", Some(id.as_ref()))
            {
                entry.persistent = Some(bind_persistent_layer(entry.area.clone(), window));
            }
        }
    }

    /// Leaving the stack by Tab after the shortcut brought focus in goes back
    /// where it was.
    pub(crate) fn region_keydown(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if Key::of(&event.keystroke) != Key::Tab || !self.entered_by_shortcut {
            return;
        }
        let stops = tabbables(&self.focus_handle, window, cx);
        let edge = if event.keystroke.modifiers.shift {
            stops.first()
        } else {
            stops.last()
        };
        if !edge.is_some_and(|edge| edge.is_focused(window))
            || self.connected_return_target(window).is_none()
        {
            return;
        }
        window.prevent_default();
        cx.stop_propagation();
        self.return_focus(window, cx);
    }

    /// Escape on a toast closes it, unless it can't be dismissed. Handled
    /// here, so a modal dialog beneath stays open.
    pub(crate) fn toast_keydown(
        &mut self,
        id: &SharedString,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if Key::of(&event.keystroke) != Key::Escape {
            return;
        }
        let dismissible = self
            .toaster
            .toasts(cx)
            .iter()
            .find(|toast| &toast.id == id)
            .map(|toast| toast.dismissible);
        if dismissible != Some(true) {
            return;
        }
        window.prevent_default();
        self.close_toast(
            id,
            Reason::EscapeKey,
            Some(InputEvent::KeyDown(event.clone())),
            cx,
        );
    }

    /// A press on a toast that may become a swipe toward the region's side,
    /// or the edge it's aligned to.
    pub(crate) fn start_swipe(
        &mut self,
        id: &SharedString,
        down: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let drags = self.drags.clone();
        if !drags.accepts(self, down, window, cx) {
            return;
        }
        if !find_item(&self.items, id).is_some_and(|item| item.toast.dismissible) {
            return;
        }
        let directions = swipe_directions(self.side, self.align, is_rtl(cx));
        let started = id.clone();
        let moved = id.clone();
        let ended = id.clone();
        let callbacks = SwipeCallbacks {
            on_start: Rc::new(move |this: &mut Self, _axis, _window, cx| {
                if let Some(entry) = this.entries.get_mut(&started) {
                    entry.swiping = true;
                }
                this.pause(ToastPauseReason::Swipe, cx);
                cx.notify();
            }),
            on_move: Rc::new(move |this: &mut Self, offset, _window, cx| {
                if let Some(entry) = this.entries.get_mut(&moved) {
                    entry.swipe = Some(offset);
                }
                cx.notify();
            }),
            on_end: Rc::new(move |this: &mut Self, direction, event, _window, cx| {
                if let Some(entry) = this.entries.get_mut(&ended) {
                    entry.swiping = false;
                }
                this.resume(ToastPauseReason::Swipe, cx);
                cx.notify();
                if let Some(direction) = direction
                    && this.close_toast(&ended, Reason::Swipe, Some(event.event.clone()), cx)
                {
                    if let Some(entry) = this.entries.get_mut(&ended) {
                        entry.swipe_direction = Some(direction);
                    }
                    return;
                }
                if let Some(entry) = this.entries.get_mut(&ended) {
                    entry.swipe = None;
                }
            }),
        };
        track_swipe(&drags, down, directions, callbacks, window, cx);
    }

    /// Runs an action or close button's handler. Work it returns that goes on
    /// marks the button pending and holds the toast open until it settles.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn press(
        &mut self,
        key: ButtonKey,
        handler: Option<ToastActionHandler>,
        dismiss: bool,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.buttons.get(&key).is_some_and(|button| button.pending) {
            return;
        }
        let source = change_source(reason, event.as_ref());
        let pressed = event.clone();
        let action =
            move |details: &ActionDetails, window: &mut Window, cx: &mut App| match &handler {
                Some(handler) => handler(pressed.as_ref(), details, window, cx),
                None => ActionOutcome::Done,
            };
        let pending_key = key.clone();
        let done_key = key.clone();
        let failed_key = key;
        let handlers = ActionHandlers {
            pending: Box::new(move |this: &mut Self, pending, window, cx| {
                this.set_button(
                    &pending_key,
                    Some(ToastButtonState {
                        pending,
                        error: None,
                    }),
                    window,
                    cx,
                );
                let toast = pending_key.toast.clone();
                this.set_held(&toast, pending, cx);
                log_with(&this.instance, "pending", &pending, reason, Some(source));
            }),
            done: Box::new(move |this: &mut Self, window, cx| {
                if this.buttons.contains_key(&done_key) {
                    this.set_button(&done_key, None, window, cx);
                }
                if dismiss {
                    let toast = done_key.toast.clone();
                    this.close_toast(&toast, reason, event.clone(), cx);
                }
            }),
            failed: Box::new(move |this: &mut Self, error, window, cx| {
                log_with(&this.instance, "error", &error, reason, Some(source));
                this.set_button(
                    &failed_key,
                    Some(ToastButtonState {
                        pending: false,
                        error: Some(error),
                    }),
                    window,
                    cx,
                );
            }),
        };
        run_action(self, &action, source, handlers, window, cx);
    }

    // The region follows its toaster only while it's on the page. An entity
    // made and never rendered must never run timers.
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.follow(window, cx);
        self.refresh(window, cx);
        self.resume(detached(), cx);
    }

    /// Follows the region's toaster, letting go of the one it followed.
    pub(crate) fn follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self
            .binding
            .as_mut()
            .and_then(|binding| binding.following.take());
        if let Some(previous) = previous {
            previous.attachment.release(cx);
        }
        if self.binding.is_none() {
            return;
        }
        let toaster = self.toaster.clone();
        let changes = toaster.observe_in(window, cx, |this: &mut Self, window, cx| {
            this.refresh(window, cx)
        });
        let attachment = toaster.attach_region(self.placed, cx);
        if let Some(binding) = &mut self.binding {
            binding.following = Some(Following {
                _changes: changes,
                attachment,
            });
        }
    }

    /// The shortcut, registered only while the region is on the page and
    /// has one, and again when it changes. It belongs to the region, a
    /// persistent layer, so it runs over every modal.
    pub(crate) fn sync_shortcut(&mut self, cx: &mut Context<Self>) {
        let next = self.shortcut.clone().filter(|keys| !keys.is_empty());
        let Some(binding) = &self.binding else {
            return;
        };
        if binding.shortcut.as_ref().map(|(keys, _)| keys) == next.as_ref() {
            return;
        }
        if let Some(binding) = &mut self.binding {
            binding.shortcut = None;
        }
        let Some(keys) = next else {
            return;
        };
        let shown = cx.weak_entity();
        let options = ShortcutOptions::default()
            .layer(self.area.clone())
            // A function key types nothing, so it works from text fields too.
            .allow_in_input(is_function_key(&keys))
            .when(move |_window, cx| {
                shown
                    .upgrade()
                    .is_some_and(|region| has_toasts(&region.read(cx).items))
            })
            .description("Move focus to notifications");
        let region = cx.weak_entity();
        let registered = register_shortcut(
            keys.clone(),
            move |_event, window, cx| {
                region
                    .update(cx, |region, cx| region.focus(window, cx))
                    .ok();
            },
            options,
            cx,
        );
        if let Some(binding) = &mut self.binding {
            binding.shortcut = Some((keys, registered));
        }
    }
}
