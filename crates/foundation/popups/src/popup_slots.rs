//! Port of `foundation/popups/popup-slots.ts`: the parts every popup has, one
//! builder each. A component's part methods call them, like
//! `popup_trigger(this, key, props, cx)`, and style or extend what they
//! return. They read the preset, the role and everything they call from the
//! component's [`Popup`], through [`PopupHost`].
//!
//! The content renders above the window: its builder returns a
//! [`PopupContent`], a part that, as it renders, goes into `deferred(..)` at
//! [`LAYER_PRIORITY`], placed by the popup's positioner, or laid out in a box
//! the size of the window when it isn't placed beside an anchor, like a
//! dialog's, where `.m_auto()` centers it. Wherever it renders, in place
//! too, it occludes what's under it, as an element on top does in the DOM,
//! and it paints the layer stack's listener and, while a modal popup blocks
//! the window, the blocker beneath it. A backdrop renders the same way;
//! declare it before the content, so it paints below. A component's own
//! backdrop state goes in [`popup_backdrop_with`].
//!
//! gpui allows one `on_hover` per element, and the trigger and the content
//! take theirs, so a component that wants another hover builds the part with
//! [`popup_trigger_with`] and calls the popup's hover itself.

use std::cell::Cell;
use std::panic;
use std::rc::Rc;

use futures::channel::mpsc::UnboundedSender;
use gpui::{
    AnyElement, App, Bounds, Div, Element, ElementId, Entity, FocusHandle, GlobalElementId,
    HitboxBehavior, Hsla, InspectorElementId, InteractiveElement, Interactivity, IntoElement,
    KeyDownEvent, LayoutId, ParentElement, Pixels, Point, SharedString, Stateful,
    StatefulInteractiveElement, Style, StyleRefinement, Styled, WeakEntity, Window, deferred, point,
    px, size,
};
use ui_sdk_kernel::focus_events::{on_focus_in, on_focus_out};
use ui_sdk_kernel::modality::window_source;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, FocusMode, HasPopup, InputEvent, Part, Reason};
use ui_sdk_motion::{PresenceState, PresenceTiming};
use ui_sdk_platform::{
    Align, ArrowPlacement, LAYER_PRIORITY, PhysicalSide, Placement, PositionProps, listener,
    positioned,
};

use crate::create_popup::{Popup, PopupHost, TriggerEntry};
use crate::hover::HoverEvent;
use crate::modal::block_outside;
use crate::popup::{
    Modal, PopupFocusTarget, disabled_trigger_attrs, popup_trigger_attrs, set_unfocusable,
};
use crate::popup_elements::owns_event;
use crate::popup_steps::HideRequest;
use crate::presentation::Presentation;
use crate::sheet::{Sheet, SheetPartState};
use crate::sheet_presentation::is_drill_hidden;

/// The props a popup's trigger takes (`PopupTriggerProps`).
#[derive(Clone, Debug, PartialEq)]
pub struct PopupTriggerProps<P> {
    /// Stops the trigger from opening the popup. It leaves the tab order,
    /// unless `focusable_when_disabled` is set.
    pub disabled: bool,
    /// Keeps a disabled trigger in the tab order with `aria-disabled`, so it
    /// can host a tooltip that says why. Presses still do nothing.
    pub focusable_when_disabled: bool,
    /// Data for the content to render when this trigger opens the popup, like
    /// the row it belongs to.
    pub payload: Option<P>,
}

impl<P> Default for PopupTriggerProps<P> {
    fn default() -> Self {
        Self {
            disabled: false,
            focusable_when_disabled: false,
            payload: None,
        }
    }
}

impl<P> PopupTriggerProps<P> {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn focusable_when_disabled(mut self, focusable: bool) -> Self {
        self.focusable_when_disabled = focusable;
        self
    }

    pub fn payload(mut self, payload: P) -> Self {
        self.payload = Some(payload);
        self
    }
}

/// Where focus goes when the popup opens (`initialFocus`). `Auto` picks the
/// first tab stop, or the popup itself when it has none, `Content` focuses
/// the popup and `None` leaves focus where it is. A focus handle, or a
/// function that returns one, picks a specific element. Opening on hover
/// never moves focus, opening with touch focuses the popup itself so no
/// on-screen keyboard appears, and hover presets never take focus.
#[derive(Clone, Default)]
pub enum PopupInitialFocus {
    #[default]
    Auto,
    Content,
    None,
    Handle(FocusHandle),
    Fn(Rc<dyn Fn(&mut Window, &mut App) -> Option<FocusHandle>>),
}

impl std::fmt::Debug for PopupInitialFocus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PopupInitialFocus::Auto => write!(f, "Auto"),
            PopupInitialFocus::Content => write!(f, "Content"),
            PopupInitialFocus::None => write!(f, "None"),
            PopupInitialFocus::Handle(handle) => f.debug_tuple("Handle").field(handle).finish(),
            PopupInitialFocus::Fn(_) => write!(f, "Fn(..)"),
        }
    }
}

impl From<FocusHandle> for PopupInitialFocus {
    fn from(handle: FocusHandle) -> Self {
        PopupInitialFocus::Handle(handle)
    }
}

/// The props a popup's content takes (`PopupContentProps`).
#[derive(Clone, Default)]
pub struct PopupContentProps {
    /// Where it goes beside its anchor: the side, alignment, offsets,
    /// collision and arrow padding, and an anchor of its own. A dialog isn't
    /// placed beside its trigger. The side defaults to the preset's, else the
    /// bottom; the alignment to the center; the side offset to 4.
    pub position: PositionProps,
    /// Where focus goes when the popup opens. Defaults to `Auto`.
    pub initial_focus: PopupInitialFocus,
    /// Where focus goes when the popup closes while focus is inside it.
    /// Defaults to `Trigger`.
    pub final_focus: Option<PopupFocusTarget>,
    /// Whether the popup stays in the page, hidden, while closed. Its content
    /// keeps its state between opens.
    pub keep_mounted: bool,
    /// Renders it where it is in the window instead of above it, inside its
    /// parent's layout, and leaves placing it to you: the position props
    /// don't apply.
    pub inline: bool,
    /// How it animates in and out, the transitions its CSS would declare.
    pub timing: Option<PresenceTiming>,
    /// The dim behind it while it presents as a sheet, at its darkest
    /// (`--sheet-backdrop`). Defaults to black at 0.2.
    pub sheet_backdrop: Option<Hsla>,
}

impl PopupContentProps {
    pub fn sheet_backdrop(mut self, color: impl Into<Hsla>) -> Self {
        self.sheet_backdrop = Some(color.into());
        self
    }

    pub fn side(mut self, side: ui_sdk_platform::Side) -> Self {
        self.position.side = Some(side);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.position.align = Some(align);
        self
    }

    pub fn side_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.position.side_offset = Some(offset.into());
        self
    }

    pub fn align_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.position.align_offset = Some(offset.into());
        self
    }

    pub fn collision_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.position.collision_padding = Some(padding.into());
        self
    }

    pub fn arrow_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.position.arrow_padding = Some(padding.into());
        self
    }

    pub fn anchor(mut self, anchor: impl Into<ui_sdk_platform::Anchor>) -> Self {
        self.position.anchor = Some(anchor.into());
        self
    }

    pub fn initial_focus(mut self, focus: impl Into<PopupInitialFocus>) -> Self {
        self.initial_focus = focus.into();
        self
    }

    pub fn final_focus(mut self, focus: impl Into<PopupFocusTarget>) -> Self {
        self.final_focus = Some(focus.into());
        self
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn inline(mut self, inline: bool) -> Self {
        self.inline = inline;
        self
    }

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = Some(timing);
        self
    }
}

/// What a popup's trigger part styles from (`PopupTriggerState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PopupTriggerState {
    /// Whether the popup is open from this trigger.
    pub open: bool,
    /// Set by the trigger's `disabled` prop, which stops it opening the popup.
    pub disabled: bool,
}

/// What a popup's content part styles from (`PopupContentState`), with
/// what the original writes as attributes and variables: its presence, and
/// where it's placed.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupContentState<P> {
    /// Whether the popup is open. False while it animates out.
    pub open: bool,
    /// The `modal` in effect: the option, else the preset's, else true while
    /// it presents as a sheet.
    pub modal: Modal,
    /// How it presents this time it's open.
    pub presentation: Presentation,
    /// The payload of the last open, from the trigger that opened it or a
    /// controller.
    pub payload: Option<P>,
    /// Its enter and exit, the original's `data-starting-style` and
    /// `data-ending-style`.
    pub presence: PresenceState,
    /// Where it's placed, as of the last frame: its side, alignment, room and
    /// transform origin (`data-side`, `--available-height`, ..).
    pub placement: Option<Placement>,
    /// Kept in the page while closed, it renders hidden.
    pub hidden: bool,
    /// The sheet it presents as, while it does: its values, opacity and drag.
    pub sheet: Option<SheetPartState>,
}

/// What a popup's backdrop part styles from (`PopupPartState`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupPartState {
    /// Whether the popup is open. False as soon as it starts closing, while
    /// it still animates out.
    pub open: bool,
    /// The backdrop's own enter and exit.
    pub presence: PresenceState,
}

/// What a popup's arrow part styles from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupArrowState {
    /// Whether the popup is open.
    pub open: bool,
    /// The side the popup opened on, the arrow's `data-side`.
    pub side: Option<PhysicalSide>,
    /// Where the arrow goes along the popup's edge, once placed.
    pub arrow: Option<ArrowPlacement>,
}

/// What [`popup_trigger_with`] puts on the trigger: its handlers, and
/// whether it says it's expanded. All of it by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriggerHandlers {
    /// A press opens and closes it, for presets a press opens.
    pub press: bool,
    /// Resting the pointer opens it, for presets hover opens.
    pub hover: bool,
    /// `aria-expanded`, for presets a press opens. A modal dialog's trigger
    /// leaves it out: it's blocked while the dialog is open, and "collapsed"
    /// once it closes says nothing useful.
    pub expanded: bool,
}

impl Default for TriggerHandlers {
    fn default() -> Self {
        Self {
            press: true,
            hover: true,
            expanded: true,
        }
    }
}

impl<T: PopupHost> Popup<T> {
    /// A trigger as it renders, for a component that builds its own: its
    /// focus handle and region, made the first time, kept with the props it
    /// rendered with. The first trigger is the one focus returns to until
    /// another opens the popup (`bindTrigger`). The last value says whether
    /// focus opening still has to hear it, with
    /// [`subscribe_trigger`](Self::subscribe_trigger).
    pub fn bind_trigger(
        &self,
        key: &SharedString,
        props: &PopupTriggerProps<T::Payload>,
        cx: &App,
    ) -> (FocusHandle, ui_sdk_platform::Region, bool) {
        let mut triggers = self.inner.triggers.borrow_mut();
        let entry = match triggers.iter_mut().position(|entry| entry.key == *key) {
            Some(index) => &mut triggers[index],
            None => {
                let handle = cx.focus_handle();
                triggers.push(TriggerEntry {
                    key: key.clone(),
                    handle: handle.clone(),
                    region: ui_sdk_platform::Region::with_focus_handle(handle),
                    props: props.clone(),
                    subscriptions: None,
                });
                triggers.last_mut().expect("just pushed")
            }
        };
        entry.props = props.clone();
        let result = (entry.handle.clone(), entry.region.clone(), entry.subscriptions.is_none());
        drop(triggers);
        let current = self.trigger_key();
        let stale = current
            .as_ref()
            .is_some_and(|current| current != key && !self.is_open() && !self.bound_triggers().contains(current));
        if current.is_none() || stale {
            *self.inner.trigger_key.borrow_mut() = Some(key.clone());
        }
        result
    }

    /// The payload the trigger with `key` rendered with.
    pub fn trigger_payload(&self, key: &SharedString) -> Option<T::Payload> {
        self.inner
            .triggers
            .borrow()
            .iter()
            .find(|entry| entry.key == *key)
            .and_then(|entry| entry.props.payload.clone())
    }

    /// The props the trigger with `key` rendered with.
    pub fn trigger_props(&self, key: &SharedString) -> Option<PopupTriggerProps<T::Payload>> {
        self.inner
            .triggers
            .borrow()
            .iter()
            .find(|entry| entry.key == *key)
            .map(|entry| entry.props.clone())
    }

    /// Focus opening hears focus reach and leave the trigger with `key`, once
    /// its handle exists. Call it after the trigger first renders, like from
    /// its part's `on_props`.
    pub fn subscribe_trigger(&self, key: SharedString, window: &mut Window, cx: &mut gpui::Context<T>) {
        if self.focus_opening().is_none() {
            return;
        }
        let handle = {
            let triggers = self.inner.triggers.borrow();
            let Some(entry) = triggers.iter().find(|entry| entry.key == key) else {
                return;
            };
            if entry.subscriptions.is_some() {
                return;
            }
            entry.handle.clone()
        };
        let enter_key = key.clone();
        let focus_in = on_focus_in(&handle, window, cx, move |this: &mut T, window, cx| {
            let popup = this.popup().clone();
            let Some(focus) = popup.focus_opening().cloned() else {
                return;
            };
            if popup.trigger_disabled(&enter_key) {
                return;
            }
            let payload = popup.trigger_payload(&enter_key);
            let event = InputEvent::Focus(window_source(window));
            focus.enter(this, enter_key.clone(), payload, event, window, cx);
        });
        let focus_out = on_focus_out(&handle, window, cx, move |this: &mut T, _event, window, cx| {
            let popup = this.popup().clone();
            let Some(focus) = popup.focus_opening().cloned() else {
                return;
            };
            let next = window.focused(cx);
            let event = InputEvent::Focus(window_source(window));
            focus.leave(this, next.as_ref(), event, window, cx);
        });
        let mut triggers = self.inner.triggers.borrow_mut();
        if let Some(entry) = triggers.iter_mut().find(|entry| entry.key == key) {
            entry.subscriptions = Some(vec![focus_in, focus_out]);
        }
    }

    /// A press on the trigger with `key`, for the preset's press opening.
    pub fn trigger_press(
        &self,
        this: &mut T,
        key: &SharedString,
        props: &PopupTriggerProps<T::Payload>,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut gpui::Context<T>,
    ) {
        if let Some(press) = self.press() {
            press.press(this, key, props, event, window, cx);
        }
    }

    /// Keys on a trigger: Tab into an open popup, unless the feature that
    /// opens it leaves Tab to gpui (`triggerKeydown`). Returns whether it took
    /// the key.
    pub fn trigger_keydown(&self, this: &T, event: &KeyDownEvent, window: &mut Window, cx: &mut App) -> bool {
        if self
            .focus_opening()
            .is_some_and(|focus| focus.keydown(self, event, window, cx))
        {
            return false;
        }
        self.layer().trigger_keydown(this, event, window, cx)
    }

    /// The pointer entered or left the trigger with `key`.
    pub fn trigger_hover(
        &self,
        this: &mut T,
        key: &SharedString,
        hovered: bool,
        event: &HoverEvent,
        window: &mut Window,
        cx: &mut gpui::Context<T>,
    ) {
        let Some(hover) = self.hover().cloned() else {
            return;
        };
        if hovered {
            if self.trigger_disabled(key) {
                return;
            }
            let payload = self.trigger_payload(key);
            hover.enter(this, event, Some(key.clone()), payload, window, cx);
        } else {
            hover.leave(this, event, window, cx);
        }
    }

    /// Keys from the content: Escape closes the topmost layer before its
    /// ancestors hear the key, and Tab follows the popup's mode. Keys from a
    /// popup nested in this one bubble through it; only the owner handles
    /// them (`contentKeydown`). Call it from the content's key handler,
    /// outside the component's update.
    pub fn content_keydown(entity: &WeakEntity<T>, event: &KeyDownEvent, window: &mut Window, cx: &mut App) {
        let Some(popup) = entity.upgrade().map(|entity| entity.read(cx).popup().clone()) else {
            return;
        };
        if !owns_event(&popup.content_region(), window, cx) {
            return;
        }
        if popup.layer().escape(event, window, cx) {
            return;
        }
        entity
            .update(cx, |this, cx| {
                let popup = this.popup().clone();
                popup.layer().keydown(this, event, window, cx);
            })
            .ok();
    }
}

/// A popup's trigger (`popupTrigger`). A press opens the popup for most
/// presets; resting the pointer or keyboard focus does for the tooltip and
/// hover card presets, whose trigger is often a link and keeps its own role.
/// Several triggers can share one popup, each handing the content its
/// `payload`.
pub fn popup_trigger<T: PopupHost>(
    this: &Entity<T>,
    key: impl Into<SharedString>,
    props: PopupTriggerProps<T::Payload>,
    cx: &App,
) -> Part<PopupTriggerState> {
    popup_trigger_with(this, key, props, TriggerHandlers::default(), cx)
}

/// [`popup_trigger`], with only what `handlers` names, for a component that
/// presses or hovers its trigger its own way, or leaves out `aria-expanded`.
pub fn popup_trigger_with<T: PopupHost>(
    this: &Entity<T>,
    key: impl Into<SharedString>,
    props: PopupTriggerProps<T::Payload>,
    handlers: TriggerHandlers,
    cx: &App,
) -> Part<PopupTriggerState> {
    let key = key.into();
    let host = this.read(cx);
    let popup = host.popup().clone();
    let instance = host.instance();
    let (handle, region, subscribe) = popup.bind_trigger(&key, &props, cx);
    let state = popup.state(host);
    let active = state.trigger.as_ref().is_none_or(|trigger| *trigger == key);
    let trigger_state = PopupTriggerState {
        open: state.open && active,
        disabled: props.disabled,
    };
    let content_id = instance.element_id(popup.content_slot(), None);
    let mut part = Part::new(instance, popup.trigger_slot(), Some(&key), trigger_state);
    let role = popup.role();
    // What the trigger says it opens comes from the feature that opens it: a
    // press's button, or a hover's link that a tooltip describes.
    if popup.press().is_some() {
        let has_popup = match role {
            Some(Role::Menu) => Some(HasPopup::Menu),
            Some(Role::Listbox) => Some(HasPopup::Listbox),
            Some(_) => Some(HasPopup::Dialog),
            None => None,
        };
        let attrs = popup_trigger_attrs(
            state.open,
            state.mounted,
            state.trigger.as_deref(),
            content_id,
            has_popup,
            &key,
        );
        let disabled = disabled_trigger_attrs(props.disabled, props.focusable_when_disabled);
        set_unfocusable(&handle, disabled.unfocusable);
        let mut aria = Aria::new().has_popup(attrs.has_popup).disabled(disabled.disabled);
        if let Some(controls) = attrs.controls {
            aria = aria.controls(controls);
        }
        part = part
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&handle), disabled.unfocusable)
            .aria(aria);
        if handlers.expanded {
            part = part.aria_expanded(attrs.open);
        }
    } else {
        let mut aria = Aria::new();
        if role == Some(Role::Tooltip) && active {
            aria = aria.described_by(content_id);
        }
        set_unfocusable(&handle, false);
        part = part
            .focus_mode(FocusMode::Native, Some(&handle), false)
            .aria(aria);
    }
    let entity = this.downgrade();
    if handlers.press && popup.press().is_some() {
        let entity = entity.clone();
        let key = key.clone();
        let props = props.clone();
        part = part.on_press(move |event, window, cx| {
            entity
                .update(cx, |this, cx| {
                    let popup = this.popup().clone();
                    popup.trigger_press(this, &key, &props, Some(event), window, cx);
                })
                .ok();
        });
    }
    if handlers.hover && popup.hover().is_some() {
        let entity = entity.clone();
        let key = key.clone();
        part = part.map_el(|el| {
            el.on_hover(move |hovered, window, cx| {
                let event = HoverEvent::from_window(window);
                let hovered = *hovered;
                entity
                    .update(cx, |this, cx| {
                        let popup = this.popup().clone();
                        popup.trigger_hover(this, &key, hovered, &event, window, cx);
                    })
                    .ok();
            })
        });
    }
    {
        let entity = entity.clone();
        part = part.map_el(|el| {
            el.on_key_down(move |event, window, cx| {
                let Some(target) = entity.upgrade() else {
                    return;
                };
                let popup = target.read(cx).popup().clone();
                let took = target.update(cx, |this, cx| popup.trigger_keydown(this, event, window, cx));
                if took {
                    cx.stop_propagation();
                }
            })
        });
    }
    if subscribe && popup.focus_opening().is_some() {
        let entity = entity.clone();
        let key = key.clone();
        part = part.on_props(move |window, cx| {
            entity
                .update(cx, |this, cx| {
                    let popup = this.popup().clone();
                    popup.subscribe_trigger(key, window, cx);
                })
                .ok();
        });
    }
    region.measure(part)
}

struct ContentToken {
    live: Rc<Cell<usize>>,
    departures: UnboundedSender<()>,
}

impl Drop for ContentToken {
    fn drop(&mut self) {
        self.live.set(self.live.get().saturating_sub(1));
        self.departures.unbounded_send(()).ok();
    }
}

/// A popup's content (`popupContent`): the popup itself, with the preset's
/// role, named by its title or else its trigger, described by its
/// description, and placed beside the trigger unless the preset is a
/// dialog's. `None` while it's out of the page: it renders while mounted,
/// kept in the page with `keep_mounted`, and always for a tooltip, hidden
/// while closed, so its trigger is described before focus reaches it.
pub fn popup_content<T: PopupHost>(
    this: &Entity<T>,
    props: PopupContentProps,
    cx: &App,
) -> Option<PopupContent<T, PopupContentState<T::Payload>>> {
    let host = this.read(cx);
    let popup = host.popup().clone();
    let instance = host.instance();
    let state = popup.state(host);
    let role = popup.role();
    let present = state.mounted || props.keep_mounted || role == Some(Role::Tooltip);
    if !present {
        popup.set_content_props(props);
        return None;
    }
    let layer = popup.layer();
    let content_state = PopupContentState {
        open: state.open,
        modal: state.modal,
        presentation: state.presentation,
        payload: state.payload.clone(),
        presence: layer.presence().state(),
        placement: layer.positioner().and_then(|positioner| positioner.placement()),
        hidden: !state.mounted,
        sheet: popup.sheet().map(|sheet| sheet.part_state()),
    };
    let mut part = Part::new(instance, popup.content_slot(), None, content_state);
    if let Some(role) = role {
        part = part.aria_role(role);
    }
    // A tooltip never takes focus.
    if role != Some(Role::Tooltip) {
        let handle = popup.content_focus_handle();
        part = part.map_el(|el| el.track_focus(&handle.tab_stop(false)));
    }
    let mut aria = Aria::new().hidden(!state.mounted);
    if let Some(press) = popup.press() {
        aria = aria.merge(press.content_aria(&popup, &state, instance));
    }
    if let Some(name) = popup.trigger_name() {
        aria = aria.merge(name.content_aria(&popup, &state, instance));
    }
    part = part.aria(aria);
    let entity = this.downgrade();
    // Only Tab: a menu built on these parts brings its own arrow keys.
    {
        let entity = entity.clone();
        part = part.on_keys(&["Tab", "Escape"], move |event, window, cx| {
            Popup::content_keydown(&entity, event, window, cx);
        });
    }
    if popup.hover().is_some() {
        let entity = entity.clone();
        part = part.map_el(|el| {
            el.on_hover(move |hovered, window, cx| {
                let event = HoverEvent::from_window(window);
                let hovered = *hovered;
                entity
                    .update(cx, |this, cx| {
                        let popup = this.popup().clone();
                        let Some(hover) = popup.hover().cloned() else {
                            return;
                        };
                        if hovered {
                            hover.enter(this, &event, None, None, window, cx);
                        } else {
                            hover.leave_content(this, &event, window, cx);
                        }
                    })
                    .ok();
            })
        });
    }
    Some(bind_popup_content(this, part, props, cx))
}

/// Shows `part` as the popup's content as it renders: placed beside its
/// anchor when the preset places it, with focus moving in when a press opens
/// it, and as a sheet when it presents as one (`bindPopupContent`). For a
/// component with its own content part, like a dialog's; [`popup_content`]
/// builds the usual one and calls it. Render it only while the content is
/// present, as [`popup_content`] does.
pub fn bind_popup_content<T: PopupHost, S: 'static>(
    this: &Entity<T>,
    part: Part<S>,
    props: PopupContentProps,
    cx: &App,
) -> PopupContent<T, S> {
    let host = this.read(cx);
    let popup = host.popup().clone();
    popup.set_content_props(props);
    let id = ElementId::Name(format!("{}-{}-layer", host.instance().id(), popup.content_slot()).into());
    PopupContent {
        id,
        part,
        popup,
        entity: this.downgrade(),
        hitbox: None,
    }
}

impl<T: PopupHost> Popup<T> {
    /// The content's props as it renders, which placement and focus read.
    pub fn set_content_props(&self, props: PopupContentProps) {
        if let Some(timing) = &props.timing {
            self.layer().presence().set_timing(timing.clone());
        }
        *self.inner.content.props.borrow_mut() = props;
    }
}

/// A popup's content part, which renders above the window as the popup
/// places it. It's styled and filled like any part.
///
/// Wherever it renders, in place too, it takes presses, hovers and scrolls
/// from what's under it, as an element on top does in the DOM. A component
/// that lets scrolls through to what it sits in, like a sheet to the host
/// that scrolls it, calls `block_mouse_except_scroll()` on it.
pub struct PopupContent<T: PopupHost, S: 'static> {
    id: ElementId,
    part: Part<S>,
    popup: Popup<T>,
    entity: WeakEntity<T>,
    hitbox: Option<HitboxBehavior>,
}

impl<T: PopupHost, S: 'static> PopupContent<T, S> {
    /// The content's state.
    pub fn state(&self) -> &S {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &S) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters, as on any part.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.part = self.part.aria(aria);
        self
    }

    /// Changes the part underneath, like a component adding its own
    /// handlers or role.
    pub fn map_part(mut self, map: impl FnOnce(Part<S>) -> Part<S>) -> Self {
        self.part = map(self.part);
        self
    }
}

impl<T: PopupHost, S: 'static> Styled for PopupContent<T, S> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl<T: PopupHost, S: 'static> InteractiveElement for PopupContent<T, S> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }

    // How it takes the pointer is kept until it renders, which would
    // otherwise put its own over what's set here.
    fn occlude(mut self) -> Self {
        self.hitbox = Some(HitboxBehavior::BlockMouse);
        self
    }

    fn block_mouse_except_scroll(mut self) -> Self {
        self.hitbox = Some(HitboxBehavior::BlockMouseExceptScroll);
        self
    }
}

impl<T: PopupHost, S: 'static> StatefulInteractiveElement for PopupContent<T, S> {}

impl<T: PopupHost, S: 'static> ParentElement for PopupContent<T, S> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl<T: PopupHost, S: 'static> IntoElement for PopupContent<T, S> {
    type Element = PopupContentElement<T, S>;

    fn into_element(self) -> Self::Element {
        PopupContentElement {
            id: self.id,
            part: Some(self.part),
            popup: self.popup,
            entity: self.entity,
            hitbox: self.hitbox.unwrap_or(HitboxBehavior::BlockMouse),
            children: Vec::new(),
            in_place: false,
        }
    }
}

/// What a [`PopupContent`] renders as.
pub struct PopupContentElement<T: PopupHost, S: 'static> {
    id: ElementId,
    part: Option<Part<S>>,
    popup: Popup<T>,
    entity: WeakEntity<T>,
    hitbox: HitboxBehavior,
    children: Vec<AnyElement>,
    in_place: bool,
}

// Takes the pointer from what's under `el` as `hitbox` says.
fn take_pointer<E: InteractiveElement>(el: E, hitbox: HitboxBehavior) -> E {
    match hitbox {
        HitboxBehavior::BlockMouse => el.occlude(),
        HitboxBehavior::BlockMouseExceptScroll => el.block_mouse_except_scroll(),
        HitboxBehavior::Normal => el,
    }
}

impl<T: PopupHost, S: 'static> PopupContentElement<T, S> {
    // The content as it renders this frame, and whether it's in place: kept
    // hidden while closed, inline, or above the window. Shown, it takes the
    // pointer from what's under it wherever it renders, as an element on top
    // does in the DOM; a sheet decides how for itself.
    fn build(&mut self) {
        let Some(part) = self.part.take() else {
            return;
        };
        let popup = &self.popup;
        let layer = popup.layer();
        let mut part = popup.content_region().measure(part).child(listener());
        if let Some(hover) = popup.hover() {
            part = part.child(hover.intent().listener());
        }
        let mounted = layer.is_mounted();
        let shown = layer.is_shown();
        // Mounted before it shows, as it is open from the start, it waits
        // for its first placement out of sight.
        if mounted && !shown {
            part = part.opacity(0.);
        }
        let blocker = {
            let blocker = layer.blocker();
            let blocker = blocker.borrow();
            blocker.frozen.then(|| blocker.kept())
        };
        let region = popup.content_region();
        if !mounted || is_drill_hidden(&region) {
            self.in_place = true;
            self.children.push(part.hidden().into_any_element());
            return;
        }
        if let Some(sheet) = popup.sheet().filter(|_| popup.presentation_value() == Presentation::Sheet && shown) {
            let body = sheet.body(part);
            if in_viewport(popup) {
                self.in_place = true;
                self.children.push(body.into_any_element());
                return;
            }
            // No viewport around it: it hosts itself. The dim goes below the
            // host, as in a viewport, so the host still hears the scrolls the
            // scroll-snap driver moves the sheet with.
            let id = ElementId::Name(format!("{}-sheet-host", self.id).into());
            let dim = sheet_dim(&sheet, popup.content_props().sheet_backdrop);
            let host = gpui::div()
                .size_full()
                .relative()
                .child(dim)
                .child(sheet.host(gpui::div().id(id).child(body)));
            self.children
                .push(deferred(window_box(host)).priority(LAYER_PRIORITY).into_any_element());
            return;
        }
        let part = take_pointer(part, self.hitbox);
        if layer.is_inline() {
            self.in_place = true;
            let part = match blocker {
                Some(keep) => part.child(deferred(block_outside(keep)).priority(LAYER_PRIORITY)),
                None => part,
            };
            self.children.push(part.into_any_element());
            return;
        }
        if let Some(keep) = blocker {
            self.children
                .push(deferred(block_outside(keep)).priority(LAYER_PRIORITY).into_any_element());
        }
        let content = match layer.positioner() {
            Some(positioner) => {
                positioner.set_anchor(layer.anchor());
                if let Some(options) = layer.position_options() {
                    positioner.set_options(options);
                }
                deferred(positioned(&positioner, part)).priority(LAYER_PRIORITY).into_any_element()
            }
            None => deferred(window_box(part)).priority(LAYER_PRIORITY).into_any_element(),
        };
        self.children.push(content);
    }
}

impl<T: PopupHost, S: 'static> Element for PopupContentElement<T, S> {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // Its place in the page, as element state: gpui drops it once a
        // frame doesn't render the content, and the popup hears it left.
        if let Some(global_id) = id {
            let live = self.popup.inner.content.live.clone();
            let departures = self.popup.inner.content.departures.clone();
            let fresh = window.with_element_state::<ContentToken, _>(global_id, |token, _window| match token {
                Some(token) => (false, token),
                None => {
                    live.set(live.get() + 1);
                    (true, ContentToken { live, departures })
                }
            });
            if fresh {
                self.popup.inner.content.present.set(true);
            }
            let layer = self.popup.layer();
            if fresh && layer.is_mounted() && !layer.is_shown() {
                let entity = self.entity.clone();
                window.defer(cx, move |window, cx| {
                    entity
                        .update(cx, |this, cx| {
                            let popup = this.popup().clone();
                            popup.content_bound(this, window, cx);
                        })
                        .ok();
                });
            }
        }
        self.popup.layer().content_rendered(window, cx);
        self.build();
        let layouts: Vec<LayoutId> = self
            .children
            .iter_mut()
            .map(|child| child.request_layout(window, cx))
            .collect();
        if self.in_place && layouts.len() == 1 {
            return (layouts[0], ());
        }
        let style = Style {
            position: gpui::Position::Absolute,
            size: size(px(0.).into(), px(0.).into()),
            ..Style::default()
        };
        (window.request_layout(style, layouts, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        for child in &mut self.children {
            child.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        for child in &mut self.children {
            child.paint(window, cx);
        }
        self.popup.layer().listen(window);
    }
}

/// Lays `child` out in a box the size of the window, at its top left
/// corner, wherever it's declared, for content above the window that isn't
/// placed beside an anchor, like a dialog's or a backdrop. The box is a flex
/// container, so `.m_auto()` centers its child.
pub fn window_box(child: impl IntoElement) -> WindowBox {
    WindowBox {
        child: child.into_any_element(),
    }
}

/// The element [`window_box`] returns.
pub struct WindowBox {
    child: AnyElement,
}

impl Element for WindowBox {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = self.child.request_layout(window, cx);
        let viewport = window.viewport_size();
        let style = Style {
            position: gpui::Position::Absolute,
            display: gpui::Display::Flex,
            size: size(viewport.width.into(), viewport.height.into()),
            ..Style::default()
        };
        (window.request_layout(style, [child], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let offset: Point<Pixels> = point(px(0.), px(0.)) - bounds.origin;
        window.with_element_offset(offset, |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

thread_local! {
    // The popups whose viewport is laying out its children now, so their
    // content renders in place inside it.
    static VIEWPORTS: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn popup_key<T: PopupHost>(popup: &Popup<T>) -> usize {
    Rc::as_ptr(&popup.inner) as *const () as usize
}

fn in_viewport<T: PopupHost>(popup: &Popup<T>) -> bool {
    let key = popup_key(popup);
    VIEWPORTS.with(|viewports| viewports.borrow().contains(&key))
}

// The dim behind a sheet that has no backdrop part: it takes presses beside
// the sheet, which dismiss it as outside presses, unless the sheet rests
// where the window stays usable.
fn sheet_dim<T: 'static>(sheet: &Sheet<T>, color: Option<Hsla>) -> AnyElement {
    let color = color.unwrap_or_else(|| gpui::black().opacity(0.2));
    let values = sheet.values();
    let interactive = sheet.is_interactive_at(sheet.target());
    let dim = gpui::div()
        .id("ui-sdk-sheet-dim")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(color)
        .opacity(values.dim as f32);
    if interactive {
        dim.into_any_element()
    } else {
        dim.occlude().into_any_element()
    }
}

/// A popup's viewport (`popupViewport`): the window-sized element a popup
/// presented as a sheet sits in, which renders above the window with the
/// sheet's dim. Put the content inside it. `None` unless it presents as a
/// sheet and is in the page.
pub fn popup_viewport<T: PopupHost>(this: &Entity<T>, cx: &App) -> Option<PopupViewport<T>> {
    let host = this.read(cx);
    let popup = host.popup().clone();
    if popup.presentation_value() != Presentation::Sheet || !popup.is_mounted() {
        return None;
    }
    let state = popup.sheet().map(|sheet| sheet.part_state());
    Some(PopupViewport {
        part: Part::new(host.instance(), "viewport", None, state),
        popup,
    })
}

/// A popup's viewport part, the sheet's host. It's styled and filled like
/// any part.
pub struct PopupViewport<T: PopupHost> {
    part: Part<Option<SheetPartState>>,
    popup: Popup<T>,
}

impl<T: PopupHost> PopupViewport<T> {
    /// The sheet's state, once it's attached.
    pub fn state(&self) -> &Option<SheetPartState> {
        self.part.state()
    }

    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &Option<SheetPartState>) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }
}

impl<T: PopupHost> Styled for PopupViewport<T> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl<T: PopupHost> InteractiveElement for PopupViewport<T> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl<T: PopupHost> ParentElement for PopupViewport<T> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl<T: PopupHost> IntoElement for PopupViewport<T> {
    type Element = PopupViewportElement<T>;

    fn into_element(self) -> Self::Element {
        PopupViewportElement {
            part: Some(self.part),
            popup: self.popup,
            child: None,
        }
    }
}

/// What a [`PopupViewport`] renders as.
pub struct PopupViewportElement<T: PopupHost> {
    part: Option<Part<Option<SheetPartState>>>,
    popup: Popup<T>,
    child: Option<AnyElement>,
}

impl<T: PopupHost> Element for PopupViewportElement<T> {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let key = popup_key(&self.popup);
        VIEWPORTS.with(|viewports| viewports.borrow_mut().push(key));
        let part = self.part.take().expect("the viewport renders once");
        let mut child = match self.popup.sheet() {
            Some(sheet) => {
                // The dim goes below the viewport and what the app put in it.
                let dim = sheet_dim(&sheet, self.popup.content_props().sheet_backdrop);
                let host = gpui::div().size_full().relative().child(dim).child(sheet.host(part));
                deferred(window_box(host)).priority(LAYER_PRIORITY).into_any_element()
            }
            None => deferred(window_box(self.popup.viewport_region().measure(part)))
                .priority(LAYER_PRIORITY)
                .into_any_element(),
        };
        let layout = child.request_layout(window, cx);
        VIEWPORTS.with(|viewports| {
            viewports.borrow_mut().pop();
        });
        self.child = Some(child);
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child) = &mut self.child {
            child.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child) = &mut self.child {
            child.paint(window, cx);
        }
    }
}

/// A popup's title, which names it while it renders (`popupTitle`). It's a
/// level 2 heading. Give it its text as an `aria_label`, which is the name
/// the popup takes, and draw the same text as a plain child.
pub fn popup_title<T: PopupHost>(this: &Entity<T>, cx: &App) -> Part<()> {
    let host = this.read(cx);
    Part::new(host.instance(), "title", None, ())
        .aria_role(Role::Heading)
        .aria_level(2)
}

/// A popup's description, which describes it while it renders
/// (`popupDescription`). Give it its text as an `aria_label`, which is the
/// description the popup takes.
pub fn popup_description<T: PopupHost>(this: &Entity<T>, cx: &App) -> Part<()> {
    let host = this.read(cx);
    Part::new(host.instance(), "description", None, ()).aria_role(Role::Paragraph)
}

/// A popup's Close button, which closes it with reason `ClosePress`
/// (`popupClose`). A popup with several gives each a key.
pub fn popup_close<T: PopupHost>(this: &Entity<T>, key: Option<SharedString>, cx: &App) -> Part<()> {
    let host = this.read(cx);
    let popup = host.popup().clone();
    let handle = popup.close_focus_handle(key.as_ref(), cx);
    let entity = this.downgrade();
    Part::new(host.instance(), "close", key.as_deref(), ())
        .aria_role(Role::Button)
        .focus_mode(FocusMode::Native, Some(&handle), false)
        .on_press(move |event, window, cx| {
            entity
                .update(cx, |this, cx| {
                    let control = this.popup().control().clone();
                    control.hide(
                        this,
                        HideRequest::new().reason(Reason::ClosePress).event(Some(event)),
                        window,
                        cx,
                    );
                })
                .ok();
        })
}

impl<T: PopupHost> Popup<T> {
    /// The focus handle of the Close part with `key`, made the first time.
    pub fn close_focus_handle(&self, key: Option<&SharedString>, cx: &App) -> FocusHandle {
        let mut closes = self.inner.closes.borrow_mut();
        if let Some((_, handle)) = closes.iter().find(|(own, _)| own.as_ref() == key) {
            return handle.clone();
        }
        let handle = cx.focus_handle();
        closes.push((key.cloned(), handle.clone()));
        handle
    }

    /// The focus handles of the Close parts, in the order they first rendered.
    pub fn close_focus_handles(&self) -> Vec<FocusHandle> {
        self.inner
            .closes
            .borrow()
            .iter()
            .map(|(_, handle)| handle.clone())
            .collect()
    }
}

/// A popup's arrow, which points at the trigger (`popupArrow`). Place it
/// along the popup's edge from its state's `arrow`, on the `side` it opened
/// on; it's hidden from assistive technology.
pub fn popup_arrow<T: PopupHost>(this: &Entity<T>, cx: &App) -> Part<PopupArrowState> {
    let host = this.read(cx);
    let popup = host.popup();
    let placement = popup.layer().positioner().and_then(|positioner| positioner.placement());
    let state = PopupArrowState {
        open: popup.is_open(),
        side: placement.map(|placement| placement.side),
        arrow: placement.and_then(|placement| placement.arrow),
    };
    let measured = popup.arrow_bounds();
    measured.measure(Part::new(host.instance(), "arrow", None, state))
}

/// A popup's backdrop (`popupBackdrop`), below it above the window and shown
/// and hidden with it. A press on it is an outside press. `None` while it's
/// out of the page. It fills the window; declare it before the content.
pub fn popup_backdrop<T: PopupHost>(
    this: &Entity<T>,
    timing: Option<PresenceTiming>,
    cx: &App,
) -> Option<PopupBackdrop<T, PopupPartState>> {
    let popup = this.read(cx).popup();
    let state = PopupPartState {
        open: popup.is_open(),
        presence: popup.layer().backdrop_presence().state(),
    };
    popup_backdrop_with(this, state, timing, cx)
}

/// [`popup_backdrop`] with the component's own state, for a backdrop that
/// says more, like a dialog's, which says whether it's nested.
pub fn popup_backdrop_with<T: PopupHost, S: 'static>(
    this: &Entity<T>,
    state: S,
    timing: Option<PresenceTiming>,
    cx: &App,
) -> Option<PopupBackdrop<T, S>> {
    let host = this.read(cx);
    let popup = host.popup().clone();
    let layer = popup.layer();
    if let Some(timing) = timing {
        layer.backdrop_presence().set_timing(timing);
    }
    if !popup.is_mounted() && !popup.content_props().keep_mounted {
        return None;
    }
    let in_place = layer.is_inline();
    let part = Part::new(host.instance(), "backdrop", None, state)
        .aria_role(Role::Presentation)
        .size_full();
    Some(PopupBackdrop {
        part,
        popup,
        entity: this.downgrade(),
        in_place,
        pass_through: false,
    })
}

/// A popup's backdrop part, which renders above the window below the
/// popup, or in place with an inline one. It's styled and filled like any
/// part.
pub struct PopupBackdrop<T: PopupHost, S: 'static> {
    part: Part<S>,
    popup: Popup<T>,
    entity: WeakEntity<T>,
    in_place: bool,
    pass_through: bool,
}

impl<T: PopupHost, S: 'static> PopupBackdrop<T, S> {
    pub fn state(&self) -> &S {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &S) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Changes the part underneath, like a component setting its opacity.
    pub fn map_part(mut self, map: impl FnOnce(Part<S>) -> Part<S>) -> Self {
        self.part = map(self.part);
        self
    }

    /// Renders it where it's declared instead of above the window. It does
    /// by default while the popup shows in place.
    pub fn in_place(mut self, in_place: bool) -> Self {
        self.in_place = in_place;
        self
    }

    /// Lets presses through to what's under it, like a drawer's while the
    /// window behind it stays usable.
    pub fn pass_through(mut self, pass_through: bool) -> Self {
        self.pass_through = pass_through;
        self
    }
}

impl<T: PopupHost, S: 'static> Styled for PopupBackdrop<T, S> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl<T: PopupHost, S: 'static> InteractiveElement for PopupBackdrop<T, S> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl<T: PopupHost, S: 'static> StatefulInteractiveElement for PopupBackdrop<T, S> {}

impl<T: PopupHost, S: 'static> ParentElement for PopupBackdrop<T, S> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl<T: PopupHost, S: 'static> IntoElement for PopupBackdrop<T, S> {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        let popup = self.popup;
        let region = popup.backdrop_region();
        let layer = popup.layer().clone();
        let part = if self.pass_through {
            self.part
        } else {
            self.part.occlude()
        };
        // A backdrop that renders after its popup has shown joins it.
        if layer.is_shown() && layer.backdrop_element().as_ref() != Some(&region) {
            let entity = self.entity.clone();
            let join = gpui::canvas(
                move |_, window, cx| {
                    let entity = entity.clone();
                    window.defer(cx, move |window, cx| {
                        entity
                            .update(cx, |this, cx| {
                                let popup = this.popup().clone();
                                popup.join_backdrop(window, cx);
                            })
                            .ok();
                    });
                },
                |_, _, _, _| {},
            );
            let part = region.measure(part).child(join);
            if self.in_place {
                return part.into_any_element();
            }
            return deferred(window_box(part)).priority(LAYER_PRIORITY).into_any_element();
        }
        if !popup.is_mounted() {
            return region.measure(part.hidden()).into_any_element();
        }
        let part = region.measure(part);
        if self.in_place {
            return part.into_any_element();
        }
        deferred(window_box(part)).priority(LAYER_PRIORITY).into_any_element()
    }
}

impl<T: PopupHost> Popup<T> {
    // The backdrop rendered after the popup showed: it joins below it.
    fn join_backdrop(&self, window: &mut Window, cx: &mut gpui::Context<T>) {
        let region = self.backdrop_region();
        if !self.layer().is_shown() || self.layer().backdrop_element().as_ref() == Some(&region) {
            return;
        }
        self.layer().backdrop(region, window, cx);
    }
}
