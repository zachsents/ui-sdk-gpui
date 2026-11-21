//! Port of `foundation/popups/popup-features.ts`: what a preset adds to
//! [`create_popup`](fn@crate::create_popup): placement beside the trigger,
//! opening on a press, on hover and on keyboard focus, tooltips' warm group,
//! and a dialog named after its trigger. Each is its own type, made only
//! when the popup's preset names it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{App, Context, FocusHandle, Pixels, SharedString, Task, Window, px};
use ui_sdk_focus::tabbables;
use ui_sdk_kernel::{Aria, InputEvent, Reason, is_focus_from_keyboard};
use ui_sdk_kernel::roles::Role;
use ui_sdk_platform::{Align, Anchor, ElementBounds, PositionOptions, PositionProps, Region, Side, Target};

use crate::create_popup::{Popup, PopupHost, PopupState};
use crate::hover::{HoverEvent, HoverIntent, HoverIntentOptions, create_hover_intent};
use crate::popup::{InitialFocusFn, Modal, PopupOpener, ReturnMode, opener_of};
use crate::popup_slots::{PopupInitialFocus, PopupTriggerProps};
use crate::popup_steps::{HideRequest, ShowRequest};
use crate::presentation::Presentation;

/// What a popup is placed against: its content's `anchor` setting, else
/// `fallback` (`contentAnchor`).
pub fn content_anchor(setting: Option<Anchor>, fallback: impl FnOnce() -> Option<Anchor>) -> Option<Anchor> {
    setting.or_else(fallback)
}

/// A component's own placement defaults. Each names its own `align`.
#[derive(Clone, Copy, Debug)]
pub struct PlacementDefaults {
    pub side: Option<Side>,
    pub align: Align,
    pub side_offset: Option<Pixels>,
    pub align_offset: Option<Pixels>,
}

impl PlacementDefaults {
    pub fn new(align: Align) -> Self {
        Self {
            side: None,
            align,
            side_offset: None,
            align_offset: None,
        }
    }

    pub fn side(mut self, side: Option<Side>) -> Self {
        self.side = side;
        self
    }
}

/// What the positioner reads on every frame, from the content's newest props
/// and the component's defaults (`placementOptions`).
pub fn placement_options(props: &PositionProps, defaults: PlacementDefaults, arrow: Option<ElementBounds>) -> PositionOptions {
    let base = PositionOptions::default();
    PositionOptions {
        side: props.side.or(defaults.side).unwrap_or(Side::Bottom),
        align: props.align.unwrap_or(defaults.align),
        side_offset: props.side_offset.or(defaults.side_offset).unwrap_or(px(4.)),
        align_offset: props.align_offset.or(defaults.align_offset).unwrap_or(px(0.)),
        collision_padding: props.collision_padding.unwrap_or(base.collision_padding),
        arrow,
        arrow_padding: props.arrow_padding.unwrap_or(base.arrow_padding),
    }
}

/// Where focus goes on open: the content's `initial_focus`. `Auto` picks
/// what the popup prefers, like a dialog's autofocused element, else the
/// first tab stop, else the content itself.
pub fn resolve_initial_focus(
    content: &Region,
    setting: &PopupInitialFocus,
    preferred: Option<&InitialFocusFn>,
    window: &mut Window,
    cx: &mut App,
) -> Option<FocusHandle> {
    let own = content.focus_handle();
    match setting {
        PopupInitialFocus::None => None,
        PopupInitialFocus::Content => own,
        PopupInitialFocus::Auto => {
            if let Some(preferred) = preferred.and_then(|preferred| preferred(window, cx)) {
                return Some(preferred);
            }
            own.clone()
                .and_then(|root| tabbables(&root, window, cx).into_iter().next())
                .or(own)
        }
        PopupInitialFocus::Fn(pick) => pick(window, cx).or(own),
        PopupInitialFocus::Handle(handle) => Some(handle.clone()),
    }
}

/// A popup a press opens (`pressOpening`): a press on the trigger opens and
/// closes it, another trigger takes the open popup over, and a press on one
/// the pointer opened keeps it open. The trigger is a button that says what
/// it opens, and the popup takes focus and is named by its title or trigger.
#[derive(Clone, Copy, Debug, Default)]
pub struct PressOpening;

impl PressOpening {
    /// A press on the trigger with `key`.
    pub fn press<T: PopupHost>(
        &self,
        this: &mut T,
        key: &SharedString,
        props: &PopupTriggerProps<T::Payload>,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if props.disabled {
            return;
        }
        let popup = this.popup().clone();
        let by = opener_of(event.as_ref());
        let control = popup.control().clone();
        if !popup.is_open() {
            let request = ShowRequest {
                reason: Some(Reason::TriggerPress),
                event,
                payload: Some(props.payload.clone()),
                by: Some(by),
                trigger: Some(key.clone()),
                ..ShowRequest::default()
            };
            control.show(this, request, None, window, cx);
            return;
        }
        let active = popup.trigger_key();
        if active.as_ref().is_some_and(|active| active != key) {
            control.take_over(this, key.clone(), props.payload.clone(), None, Reason::TriggerPress, event, window, cx);
            return;
        }
        if popup.layer().opener() == PopupOpener::Hover {
            let request = ShowRequest {
                event,
                by: Some(by),
                ..ShowRequest::default()
            };
            control.show(this, request, None, window, cx);
            return;
        }
        control.hide(this, HideRequest::new().reason(Reason::TriggerPress).event(event), window, cx);
    }

    /// A press on another trigger takes the popup over instead of closing
    /// and reopening it. Focus moving there still leaves it: only a press
    /// comes with an `event`.
    pub fn contains<T: PopupHost>(
        &self,
        popup: &Popup<T>,
        target: &Target,
        event: Option<&InputEvent>,
        window: &Window,
    ) -> bool {
        event.is_some() && popup.trigger_contains(target, window)
    }

    /// The ARIA of the content: named by its title, else its trigger, and
    /// described by its description. Until the parts render, the trigger
    /// names it: a title may not render, and a missing one would leave it
    /// with no name at all. Only a popup that can present as a sheet says how
    /// it presents (`contentAttrs`).
    pub fn content_aria<T: PopupHost>(&self, popup: &Popup<T>, state: &PopupState<T::Payload>, instance: &ui_sdk_kernel::Instance) -> Aria {
        let mut aria = Aria::new();
        if popup.role().is_some() {
            aria = if state.titled == Some(true) {
                aria.labelled_by(instance.element_id("title", None))
            } else {
                match popup.active_trigger_id(instance, state) {
                    Some(trigger) => aria.labelled_by(trigger),
                    None => aria,
                }
            };
        }
        if state.described {
            aria = aria.described_by(instance.element_id("description", None));
        }
        let dialog = matches!(popup.role(), Some(Role::Dialog | Role::AlertDialog));
        aria.modal(dialog && state.modal == Modal::True)
    }

    /// How it presents, for a popup that can present as a sheet
    /// (`data-presentation`).
    pub fn presentation<T: PopupHost>(&self, popup: &Popup<T>, state: &PopupState<T::Payload>) -> Option<Presentation> {
        popup.preset().presentation.then_some(state.presentation)
    }
}

/// One of the popups the pointer opened, which another one opening by
/// hover closes, unless its trigger is inside them.
struct HoverOpened {
    id: u64,
    // Whether it's still open and the pointer opened it.
    hover_open: Rc<dyn Fn() -> bool>,
    content: Region,
    close: Rc<dyn Fn(&InputEvent, &mut Window, &mut App)>,
}

thread_local! {
    static HOVER_OPENED: RefCell<Vec<HoverOpened>> = const { RefCell::new(Vec::new()) };
    static NEXT_FEATURE: Cell<u64> = const { Cell::new(1) };
}

fn next_id() -> u64 {
    NEXT_FEATURE.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    })
}

// Popovers and hover cards the pointer opened. Another one opening by hover
// closes them, unless its trigger is inside them, so they don't pile up when
// a trigger sits in the area between one's trigger and popup.
fn close_other_hovered(id: u64, trigger: Option<&Region>, event: &InputEvent, window: &mut Window, cx: &mut App) {
    let closes: Vec<Rc<dyn Fn(&InputEvent, &mut Window, &mut App)>> = HOVER_OPENED.with(|opened| {
        let mut opened = opened.borrow_mut();
        opened.retain(|other| (other.hover_open)());
        opened
            .iter()
            .filter(|other| {
                other.id != id
                    && !trigger
                        .and_then(Region::focus_handle)
                        .is_some_and(|trigger| other.content.contains_focus(&trigger, window))
                    && !window
                        .focused(cx)
                        .is_some_and(|focused| other.content.contains_focus(&focused, window))
            })
            .map(|other| other.close.clone())
            .collect()
    });
    for close in closes {
        close(event, window, cx);
    }
}

/// Resting the pointer on a trigger opens it after `delay`, always for a
/// hover preset and with `open_on_hover` for the others (`hoverOpening`).
/// The pointer may cross the gap between the trigger and the popup; leaving
/// the area that spans both closes it after `close_delay`. Another trigger
/// takes the open popup over at once. Clones share it.
pub struct HoverOpening<T: PopupHost> {
    inner: Rc<HoverInner<T>>,
}

struct HoverInner<T: PopupHost> {
    id: u64,
    intent: HoverIntent<T>,
}

impl<T: PopupHost> Drop for HoverInner<T> {
    fn drop(&mut self) {
        let id = self.id;
        HOVER_OPENED.with(|opened| {
            if let Ok(mut opened) = opened.try_borrow_mut() {
                opened.retain(|other| other.id != id);
            }
        });
    }
}

impl<T: PopupHost> Clone for HoverOpening<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Hover opening for the popup being made.
pub fn hover_opening<T: PopupHost>(cx: &Context<T>) -> HoverOpening<T> {
    let intent = create_hover_intent(
        HoverIntentOptions {
            trigger: Rc::new(|this: &T| {
                this.popup()
                    .trigger_element()
                    .and_then(|trigger| trigger.bounds())
            }),
            popup: Rc::new(|this: &T| {
                this.popup()
                    .layer()
                    .element()
                    .and_then(|content| content.bounds())
            }),
            closes_on_leave: Rc::new(|this: &T| {
                let popup = this.popup();
                popup.is_open() && popup.layer().opener() == PopupOpener::Hover
            }),
            close_delay: Rc::new(|this: &T| {
                let popup = this.popup();
                popup
                    .close_delay_option()
                    .or(popup.preset().close_delay)
                    .unwrap_or(Duration::ZERO)
            }),
            close: Rc::new(|this: &mut T, event: &InputEvent, window: &mut Window, cx: &mut Context<T>| {
                let popup = this.popup().clone();
                // Someone working inside the popup keeps it open.
                if let Some(content) = popup.layer().element()
                    && window
                        .focused(cx)
                        .is_some_and(|focused| content.contains_focus(&focused, window))
                {
                    return;
                }
                popup.control().clone().hide(
                    this,
                    HideRequest::new()
                        .reason(Reason::TriggerHover)
                        .event(Some(event.clone()))
                        .focus(ReturnMode::Leave),
                    window,
                    cx,
                );
            }),
        },
        cx,
    );
    HoverOpening {
        inner: Rc::new(HoverInner { id: next_id(), intent }),
    }
}

impl<T: PopupHost> HoverOpening<T> {
    /// The intent underneath, whose listener the content paints.
    pub fn intent(&self) -> &HoverIntent<T> {
        &self.inner.intent
    }

    /// Forgets any pending open or close.
    pub fn clear(&self) {
        self.inner.intent.clear();
    }

    fn opens(&self, popup: &Popup<T>) -> bool {
        (!popup.preset().press || popup.open_on_hover_option()) && !popup.disabled_option()
    }

    /// The pointer entered a trigger, with its key, or the content, without
    /// one.
    pub fn enter(
        &self,
        this: &mut T,
        event: &HoverEvent,
        key: Option<SharedString>,
        payload: Option<T::Payload>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let popup = this.popup().clone();
        if !self.opens(&popup) {
            return;
        }
        let intent = self.inner.intent.clone();
        let Some(key) = key else {
            intent.enter(this, event, None, Duration::ZERO, window, cx);
            return;
        };
        let id = self.inner.id;
        let grouped = popup.group().is_some();
        let pointer = event.event.clone();
        let open_key = key.clone();
        // Disabled while the delay ran, it stays closed.
        let open = move |this: &mut T, window: &mut Window, cx: &mut Context<T>| {
            let popup = this.popup().clone();
            let Some(hover) = popup.hover().cloned() else {
                return;
            };
            if !hover.opens(&popup) || popup.trigger_disabled(&open_key) {
                return;
            }
            if !grouped {
                let trigger = popup.trigger_region(&open_key);
                close_other_hovered(id, trigger.as_ref(), &pointer, window, cx);
                hover.register(&popup);
            }
            let request = ShowRequest {
                reason: Some(Reason::TriggerHover),
                event: Some(pointer.clone()),
                payload: Some(payload.clone()),
                by: Some(PopupOpener::Hover),
                trigger: Some(open_key.clone()),
                ..ShowRequest::default()
            };
            popup.control().clone().show(this, request, None, window, cx);
        };
        if popup.is_open() {
            intent.enter(this, event, None, Duration::ZERO, window, cx);
            if popup.trigger_key().as_ref() != Some(&key) {
                open(this, window, cx);
            }
            return;
        }
        let delay = popup
            .delay_option()
            .or(popup.preset().delay)
            .unwrap_or(Duration::from_millis(300));
        let delay = match popup.group() {
            Some(group) => group.delay(delay),
            None => delay,
        };
        intent.enter(this, event, Some(Box::new(open)), delay, window, cx);
    }

    fn register(&self, popup: &Popup<T>) {
        let id = self.inner.id;
        let weak = popup.weak();
        let entity = popup.layer().entity();
        let window = popup.layer().window_handle();
        let content = popup.content_region();
        HOVER_OPENED.with(|opened| {
            let mut opened = opened.borrow_mut();
            opened.retain(|other| other.id != id);
            opened.push(HoverOpened {
                id,
                hover_open: Rc::new(move || {
                    weak.upgrade()
                        .is_some_and(|popup| popup.is_open() && popup.layer().opener() == PopupOpener::Hover)
                }),
                content,
                close: Rc::new(move |event, current, cx| {
                    let event = event.clone();
                    let run = |window: &mut Window, cx: &mut App| {
                        entity
                            .update(cx, |this, cx| {
                                let control = this.popup().control().clone();
                                control.hide(
                                    this,
                                    HideRequest::new()
                                        .reason(Reason::TriggerHover)
                                        .event(Some(event.clone()))
                                        .focus(ReturnMode::Leave),
                                    window,
                                    cx,
                                );
                            })
                            .ok();
                    };
                    if current.window_handle().window_id() == window.window_id() {
                        run(current, cx);
                    } else {
                        window.update(cx, |_, window, cx| run(window, cx)).ok();
                    }
                }),
            });
        });
    }

    /// The pointer left a trigger or the content.
    pub fn leave(&self, this: &mut T, event: &HoverEvent, window: &mut Window, cx: &mut Context<T>) {
        let popup = this.popup().clone();
        if self.opens(&popup) {
            self.inner.intent.clone().leave(this, event, window, cx);
        }
    }

    /// The pointer left the content, unless onto the trigger.
    pub fn leave_content(&self, this: &mut T, event: &HoverEvent, window: &mut Window, cx: &mut Context<T>) {
        let popup = this.popup().clone();
        let onto_trigger = popup
            .trigger_element()
            .is_some_and(|trigger| trigger.contains_point(event.position, window));
        if !onto_trigger {
            self.leave(this, event, window, cx);
        }
    }
}

/// Keyboard focus resting on a trigger opens it after `focus_delay`, else
/// `delay`, and focus leaving the trigger closes what focus opened, unless
/// it moved into the popup (`focusOpening`). Focus a popup moved in or back,
/// or that came back with the window, isn't keyboard use. Such a trigger,
/// often a link, keeps its own role, and a tooltip describes it.
pub struct FocusOpening<T: PopupHost> {
    timer: Rc<RefCell<Option<Task<()>>>>,
    _owner: std::marker::PhantomData<T>,
}

impl<T: PopupHost> Clone for FocusOpening<T> {
    fn clone(&self) -> Self {
        Self {
            timer: self.timer.clone(),
            _owner: std::marker::PhantomData,
        }
    }
}

/// Focus opening for the popup being made.
pub fn focus_opening<T: PopupHost>() -> FocusOpening<T> {
    FocusOpening {
        timer: Rc::new(RefCell::new(None)),
        _owner: std::marker::PhantomData,
    }
}

impl<T: PopupHost> FocusOpening<T> {
    pub fn clear(&self) {
        *self.timer.borrow_mut() = None;
    }

    /// Focus reached the trigger with `key`.
    pub fn enter(
        &self,
        this: &mut T,
        key: SharedString,
        payload: Option<T::Payload>,
        event: InputEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let popup = this.popup().clone();
        if popup.disabled_option() || !is_focus_from_keyboard(window, cx) {
            return;
        }
        let open = move |this: &mut T, window: &mut Window, cx: &mut Context<T>| {
            let popup = this.popup().clone();
            // Focus may have moved on, or the trigger been disabled, while
            // waiting.
            let still = popup.trigger_region(&key).is_some_and(|trigger| {
                window
                    .focused(cx)
                    .is_some_and(|focused| trigger.contains_focus(&focused, window))
            });
            if popup.disabled_option() || popup.trigger_disabled(&key) || !still {
                return;
            }
            let request = ShowRequest {
                reason: Some(Reason::TriggerFocus),
                event: Some(event.clone()),
                payload: Some(payload.clone()),
                by: Some(PopupOpener::Keyboard),
                trigger: Some(key.clone()),
                ..ShowRequest::default()
            };
            popup.control().clone().show(this, request, None, window, cx);
        };
        let wait = popup
            .preset()
            .focus_delay
            .or(popup.delay_option())
            .or(popup.preset().delay)
            .unwrap_or(Duration::ZERO);
        self.clear();
        if wait.is_zero() {
            open(this, window, cx);
            return;
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update_in(cx, |this, window, cx| open(this, window, cx)).ok();
        });
        *self.timer.borrow_mut() = Some(task);
    }

    /// Focus left the trigger, to `next`.
    pub fn leave(&self, this: &mut T, next: Option<&FocusHandle>, event: InputEvent, window: &mut Window, cx: &mut Context<T>) {
        self.clear();
        let popup = this.popup().clone();
        if !popup.is_open() || popup.layer().opener() == PopupOpener::Hover {
            return;
        }
        if let (Some(next), Some(content)) = (next, popup.layer().element())
            && content.contains_focus(next, window)
        {
            return;
        }
        popup.control().clone().hide(
            this,
            HideRequest::new()
                .reason(Reason::FocusOut)
                .event(Some(event))
                .focus(ReturnMode::Leave),
            window,
            cx,
        );
    }

    /// Tab from the trigger moves into open content only when it has
    /// something to focus. Says whether it leaves Tab to gpui.
    pub fn keydown(&self, popup: &Popup<T>, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut App) -> bool {
        if ui_sdk_kernel::Key::of(&event.keystroke) != ui_sdk_kernel::Key::Tab {
            return false;
        }
        let content = popup.layer().element().and_then(|content| content.focus_handle());
        !content.is_some_and(|content| !tabbables(&content, window, cx).is_empty())
    }
}

/// Names an untitled popup after the trigger it opened from, or its first
/// trigger (`triggerName`). The original copies the trigger's name, since a
/// modal dialog makes the page inert, its trigger included, so the trigger
/// can't name it by reference. gpui keeps the trigger in the accessibility
/// tree, so the content names it by reference. Until the parts render, its
/// title names it.
#[derive(Clone, Copy, Debug, Default)]
pub struct TriggerName;

impl TriggerName {
    /// The content's ARIA. Only a dialog that blocks the window is modal to
    /// assistive technology; one that only traps focus leaves it usable.
    pub fn content_aria<T: PopupHost>(&self, popup: &Popup<T>, state: &PopupState<T::Payload>, instance: &ui_sdk_kernel::Instance) -> Aria {
        let aria = if state.titled != Some(false) {
            Aria::new().labelled_by(instance.element_id("title", None))
        } else {
            match popup.active_trigger_id(instance, state) {
                Some(trigger) => Aria::new().labelled_by(trigger),
                None => Aria::new(),
            }
        };
        aria.modal(state.modal == Modal::True)
    }
}

// Tooltips open at once while another one is open or closed a moment ago,
// so moving along a toolbar shows each without waiting, and only one is open
// at a time. State per thread, so there's no provider and nothing re-renders.
const WARM_FOR: Duration = Duration::from_millis(400);

type WarmClose = Rc<dyn Fn(Option<&InputEvent>, &mut Window, &mut App)>;

#[derive(Default)]
struct Warm {
    close: Option<(u64, WarmClose)>,
    closed_at: Option<Instant>,
}

thread_local! {
    static WARM: RefCell<Warm> = RefCell::new(Warm::default());
}

fn is_warm() -> bool {
    WARM.with(|warm| {
        let warm = warm.borrow();
        warm.close.is_some() || warm.closed_at.is_some_and(|at| at.elapsed() < WARM_FOR)
    })
}

/// Why the open tooltip closes when another one opens, from the event that
/// opened the other: focus or the pointer moved on, or a press landed
/// elsewhere (`takeoverReason`).
pub fn takeover_reason(event: Option<&InputEvent>) -> Reason {
    match event {
        None => Reason::Imperative,
        Some(InputEvent::Focus(_)) => Reason::FocusOut,
        Some(
            InputEvent::MouseMove(_)
            | InputEvent::MouseDown(_)
            | InputEvent::MouseUp(_)
            | InputEvent::TapDown(_)
            | InputEvent::TapUp(_),
        ) => Reason::TriggerHover,
        Some(_) => Reason::OutsidePress,
    }
}

/// One open at a time, the next opening at once while one is warm
/// (`warmGroup`). Clones share it.
pub struct WarmGroup<T: PopupHost> {
    inner: Rc<WarmInner>,
    _owner: std::marker::PhantomData<T>,
}

struct WarmInner {
    id: u64,
    close: WarmClose,
    instant: Cell<bool>,
}

impl Drop for WarmInner {
    fn drop(&mut self) {
        let id = self.id;
        WARM.with(|warm| {
            if let Ok(mut warm) = warm.try_borrow_mut()
                && warm.close.as_ref().is_some_and(|(open, _)| *open == id)
            {
                warm.close = None;
            }
        });
    }
}

impl<T: PopupHost> Clone for WarmGroup<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _owner: std::marker::PhantomData,
        }
    }
}

/// The warm group for the popup being made.
pub fn warm_group<T: PopupHost>(window: &Window, cx: &Context<T>) -> WarmGroup<T> {
    let entity = cx.weak_entity();
    let handle = window.window_handle();
    let close: WarmClose = Rc::new(move |event, current, cx| {
        let event = event.cloned();
        let run = |window: &mut Window, cx: &mut App| {
            entity
                .update(cx, |this, cx| {
                    let control = this.popup().control().clone();
                    control.hide(
                        this,
                        HideRequest::new()
                            .reason(takeover_reason(event.as_ref()))
                            .event(event.clone()),
                        window,
                        cx,
                    );
                })
                .ok();
        };
        if current.window_handle().window_id() == handle.window_id() {
            run(current, cx);
        } else {
            handle.update(cx, |_, window, cx| run(window, cx)).ok();
        }
    });
    WarmGroup {
        inner: Rc::new(WarmInner {
            id: next_id(),
            close,
            instant: Cell::new(false),
        }),
        _owner: std::marker::PhantomData,
    }
}

impl<T: PopupHost> WarmGroup<T> {
    /// The delay to wait: none while one is warm.
    pub fn delay(&self, delay: Duration) -> Duration {
        if is_warm() { Duration::ZERO } else { delay }
    }

    fn close_others(&self, event: Option<&InputEvent>, window: &mut Window, cx: &mut App) {
        let id = self.inner.id;
        let other = WARM.with(|warm| {
            warm.borrow()
                .close
                .as_ref()
                .filter(|(open, _)| *open != id)
                .map(|(_, close)| close.clone())
        });
        if let Some(close) = other {
            close(event, window, cx);
        }
    }

    /// The one taking over closes the open one, the way the pointer or focus
    /// left it.
    pub fn opening(&self, event: Option<&InputEvent>, window: &mut Window, cx: &mut App) {
        self.close_others(event, window, cx);
    }

    /// The popup opened or closed.
    pub fn change(&self, open: bool, window: &mut Window, cx: &mut App) {
        let id = self.inner.id;
        if open {
            self.inner.instant.set(is_warm());
            self.close_others(None, window, cx);
            let close = self.inner.close.clone();
            WARM.with(|warm| warm.borrow_mut().close = Some((id, close)));
            return;
        }
        WARM.with(|warm| {
            let mut warm = warm.borrow_mut();
            if warm.close.as_ref().is_none_or(|(open, _)| *open != id) {
                return;
            }
            warm.close = None;
            warm.closed_at = Some(Instant::now());
        });
    }

    /// Whether the last open skipped its delay, because another was warm.
    pub fn instant(&self) -> bool {
        self.inner.instant.get()
    }

    /// One open from the start is the open one too, once its content is in
    /// the page.
    pub fn join(&self, open: bool) {
        if !open {
            return;
        }
        let close = self.inner.close.clone();
        let id = self.inner.id;
        WARM.with(|warm| {
            let mut warm = warm.borrow_mut();
            if warm.close.is_none() {
                warm.close = Some((id, close));
            }
        });
    }
}
