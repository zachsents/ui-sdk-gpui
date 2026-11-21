//! Port of `components/toast/toast-parts.ts`: a toast region's parts and
//! what they carry.
//!
//! The region renders above the window, as the original shows it in the top
//! layer: [`ToastRegion::region`] returns a [`ToastRegionPart`], which, as it
//! renders, goes into `deferred(..)` at the platform's
//! `PERSISTENT_LAYER_PRIORITY`, above every popup, laid out at its side of
//! the window wherever the app declares it. Declare its toasts inside it, a
//! [`ToastRegion::root`] for each of [`ToastRegionState::toasts`], newest
//! first, with their title, description, action and close parts inside.

use std::panic;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Div, Element, ElementId, Entity, FocusHandle,
    GlobalElementId, InspectorElementId, InteractiveElement, Interactivity, IntoElement, LayoutId,
    MouseButton, MouseExitEvent, MouseMoveEvent, ParentElement, Pixels, SharedString, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled, WeakEntity, Window, canvas, deferred,
};
use ui_sdk_drag_and_drop::on_pointer_down;
use ui_sdk_kernel::part::{FocusMode, with_role};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, Part, Reason};
use ui_sdk_platform::{
    PERSISTENT_LAYER_PRIORITY, TrackKeyboard, aria_keyshortcuts, button_element, button_role,
    is_apple, is_rtl,
};
use ui_sdk_popups::window_box;

use crate::toast::{
    ButtonKey, ToastActionProps, ToastAlign, ToastButtonState, ToastCloseProps, ToastRegion,
    ToastRegionState, ToastSide, ToastState,
};
use crate::toast_api::{find_item, has_toasts};
use crate::toast_dom::{place_region, region_defaults};
use crate::toaster::ToastType;

impl ToastRegion {
    /// A toast's state, as its root part has it.
    pub fn toast_state(&self, id: &str) -> ToastState {
        let item = find_item(&self.items, id);
        let entry = self.entries.get(id);
        ToastState {
            r#type: item.map_or(ToastType::Default, |item| item.toast.r#type),
            index: item.map_or(0, |item| item.index),
            front: item.is_some_and(|item| item.index == 0),
            expanded: self.is_expanded(),
            // A part can render once more after its toast left the region.
            leaving: item.is_none_or(|item| item.leaving),
            offset: entry.and_then(|entry| entry.offset),
            height: self.heights.get(id).copied(),
            swiping: entry.is_some_and(|entry| entry.swiping),
            swipe: entry.and_then(|entry| entry.swipe),
            swipe_direction: entry.and_then(|entry| entry.swipe_direction),
            presence: entry
                .map(|entry| entry.presence.state())
                .unwrap_or_default(),
        }
    }

    // A toast that's leaving takes no input, its own `inert`.
    pub(crate) fn is_inert(&self, id: &str) -> bool {
        find_item(&self.items, id).is_none_or(|item| item.leaving)
    }

    pub(crate) fn button_handle(&self, key: &ButtonKey, cx: &App) -> FocusHandle {
        self.button_handles
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// The `region` part: the landmark the toasts render in, named
    /// "Notifications", or the `label` option, while it has toasts, and
    /// advertising its shortcut. It renders above the window at its side,
    /// and a Tab past its last stop after the shortcut brought focus in goes
    /// back where focus was.
    pub fn region(this: &Entity<Self>, cx: &App) -> ToastRegionPart {
        let region = this.read(cx);
        let mut part = Part::new(&region.instance, "region", None, region.state());
        // A landmark only while there's something in it.
        if has_toasts(&region.items) {
            let label = region
                .label
                .clone()
                .unwrap_or_else(|| "Notifications".into());
            part = part.aria_role(Role::Region).aria_label(label);
            if let Some(shortcut) = region
                .shortcut
                .as_ref()
                .filter(|shortcut| !shortcut.is_empty())
            {
                part = part.aria_keyshortcuts(aria_keyshortcuts(shortcut, is_apple(cx)));
            }
        }
        let keys = this.downgrade();
        let frames = this.downgrade();
        let handle = region.focus_handle.clone().tab_stop(false);
        let part = part
            .on_keys(&["Tab"], move |event, window, cx| {
                keys.update(cx, |region, cx| region.region_keydown(event, window, cx))
                    .ok();
            })
            .map_el(|el| region.area.measure(el.track_focus(&handle)))
            // Its own background takes no focus, as a plain element's doesn't.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .block_mouse_except_scroll()
            .child(region.drags.listener())
            .child(hover_listener(this.downgrade()))
            .on_props(move |window, cx| {
                frames
                    .update(cx, |region, cx| region.after_frame(window, cx))
                    .ok();
            });
        ToastRegionPart {
            part,
            side: region.side,
            align: region.align,
            rtl: is_rtl(cx),
            keyboard: region.keyboard.clone(),
        }
    }

    /// The `list` part: the toasts, as a list.
    pub fn list(this: &Entity<Self>, cx: &App) -> Part<()> {
        let region = this.read(cx);
        Part::new(&region.instance, "list", None, ()).aria_role(Role::List)
    }

    /// A toast's `root` part, by its id: a list item in the tab order, named
    /// by its title and described by its description while they render.
    /// Escape dismisses it, and so does a swipe toward the region's side or
    /// the edge it's aligned to, unless it can't be dismissed. Swipes and
    /// presses are the toast's, not what's beneath it. While it animates out
    /// it takes no input and is hidden from assistive technology.
    pub fn root(this: &Entity<Self>, toast: impl Into<SharedString>, cx: &App) -> Part<ToastState> {
        let region = this.read(cx);
        let key: SharedString = toast.into();
        let state = region.toast_state(&key);
        let leaving = state.leaving;
        let instance = &region.instance;
        let aria = Aria::new()
            .labelled_by(instance.element_id("title", Some(&key)))
            .described_by(instance.element_id("description", Some(&key)))
            .hidden(leaving);
        let part = Part::new(instance, "root", Some(&key), state)
            .aria_role(Role::ListItem)
            .aria(aria);
        let Some(entry) = region.entries.get(&key).filter(|_| !leaving) else {
            return part;
        };
        let keys = this.downgrade();
        let presses = this.downgrade();
        let escaped = key.clone();
        let swiped = key;
        part.focus_mode(FocusMode::TabStop, Some(&entry.handle), false)
            .on_keys(&["Escape"], move |event, window, cx| {
                keys.update(cx, |region, cx| {
                    region.toast_keydown(&escaped, event, window, cx)
                })
                .ok();
            })
            .map_el(|el| {
                let el = on_pointer_down(el, move |event, window, cx| {
                    presses
                        .update(cx, |region, cx| {
                            region.start_swipe(&swiped, event, window, cx)
                        })
                        .ok();
                });
                entry.area.measure(el)
            })
            .block_mouse_except_scroll()
    }

    /// A toast's `title` part, by its toast's id, which names the toast. It's
    /// labelled with the toast's title; render the text as a child, and give
    /// it an `aria_label` of your own when it shows something else.
    pub fn title(this: &Entity<Self>, toast: impl Into<SharedString>, cx: &App) -> Part<()> {
        let region = this.read(cx);
        let key: SharedString = toast.into();
        let text = find_item(&region.items, &key).and_then(|item| item.toast.title.clone());
        // A role of its own, so the toast's labelled-by finds it.
        let part = Part::new(&region.instance, "title", Some(&key), ())
            .map_el(|el| with_role(el, gpui::Role::Label));
        match text {
            Some(text) => part.aria_label(text),
            None => part,
        }
    }

    /// A toast's `description` part, by its toast's id, which describes the
    /// toast. It's labelled with the toast's description, like the title.
    pub fn description(this: &Entity<Self>, toast: impl Into<SharedString>, cx: &App) -> Part<()> {
        let region = this.read(cx);
        let key: SharedString = toast.into();
        let text = find_item(&region.items, &key).and_then(|item| item.toast.description.clone());
        // A role of its own, so the toast's described-by finds it.
        let part = Part::new(&region.instance, "description", Some(&key), ())
            .map_el(|el| with_role(el, gpui::Role::Label));
        match text {
            Some(text) => part.aria_label(text),
            None => part,
        }
    }

    /// A toast's `action` part, by its toast's id, like Undo: a button, or a
    /// link when the toast's action has an `href`, labelled with the action's
    /// label. A press runs `props.on_action`, else the toast's
    /// `action.on_action`, then dismisses the toast unless told not to. Work
    /// the handler returns that goes on keeps the toast open and the button
    /// pending; presses do nothing meanwhile.
    pub fn action(
        this: &Entity<Self>,
        toast: impl Into<SharedString>,
        props: ToastActionProps,
        cx: &App,
    ) -> Part<ToastButtonState> {
        let region = this.read(cx);
        let key = ButtonKey::new("action", toast.into(), props.id.clone());
        let action =
            find_item(&region.items, &key.toast).and_then(|item| item.toast.action.clone());
        let element = button_element(action.as_ref().is_some_and(|action| action.href.is_some()));
        let mut part = button_part(region, &key, button_role(element), cx);
        if let Some(action) = &action {
            part = part.aria_label(action.label.clone());
        }
        let entity = this.downgrade();
        part.on_press(move |event, window, cx| {
            entity
                .update(cx, |region, cx| {
                    if region.is_inert(&key.toast) {
                        return;
                    }
                    let action = region
                        .toaster
                        .toasts(cx)
                        .iter()
                        .find(|queued| queued.id == key.toast)
                        .and_then(|queued| queued.action.clone());
                    let handler = props
                        .on_action
                        .clone()
                        .or_else(|| action.as_ref().and_then(|action| action.on_action.clone()));
                    let dismiss = props
                        .dismiss
                        .or(action.as_ref().and_then(|action| action.dismiss))
                        .unwrap_or(true);
                    region.press(
                        key.clone(),
                        handler,
                        dismiss,
                        Reason::Action,
                        Some(event),
                        window,
                        cx,
                    );
                })
                .ok();
        })
    }

    /// A toast's `close` part, by its toast's id: a button that closes the
    /// toast, after its `action` when it has one. Work the action returns
    /// that goes on keeps the toast open and the button pending until it
    /// settles. Name it with an `aria_label`, like the toast's cancel label.
    pub fn close(
        this: &Entity<Self>,
        toast: impl Into<SharedString>,
        props: ToastCloseProps,
        cx: &App,
    ) -> Part<ToastButtonState> {
        let region = this.read(cx);
        let key = ButtonKey::new("close", toast.into(), props.id.clone());
        let part = button_part(region, &key, Role::Button, cx);
        let entity = this.downgrade();
        part.on_press(move |event, window, cx| {
            entity
                .update(cx, |region, cx| {
                    if region.is_inert(&key.toast) {
                        return;
                    }
                    region.press(
                        key.clone(),
                        props.action.clone(),
                        true,
                        Reason::ClosePress,
                        Some(event),
                        window,
                        cx,
                    );
                })
                .ok();
        })
    }
}

// What an action and a close part share: their state, focus, and the ARIA of
// a pending press. Presses do nothing while it's pending, which says so
// without the native attribute, which would drop focus.
fn button_part(
    region: &ToastRegion,
    key: &ButtonKey,
    role: Role,
    cx: &App,
) -> Part<ToastButtonState> {
    let state = region.buttons.get(key).cloned().unwrap_or_default();
    let inert = region.is_inert(&key.toast);
    let handle = region.button_handle(key, cx);
    let aria = Aria::new().busy(state.pending).disabled(state.pending);
    Part::new(&region.instance, key.slot, Some(&key.part_key()), state)
        .aria_role(role)
        .focus_mode(FocusMode::Native, Some(&handle), inert)
        // A toast animating out takes no focus from a press either.
        .on_mouse_down(MouseButton::Left, move |_, window, _| {
            if inert {
                window.prevent_default();
            }
        })
        .aria(aria)
}

// Hears the pointer over the region and the toasts that reach out of it, or
// leaving the window, while the region paints.
fn hover_listener(region: WeakEntity<ToastRegion>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            let moved = region.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    moved
                        .update(cx, |region, cx| {
                            region.pointer_moved(event.position, window, cx)
                        })
                        .ok();
                }
            });
            window.on_mouse_event(move |_: &MouseExitEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble {
                    region.update(cx, |region, cx| region.pointer_left(cx)).ok();
                }
            });
        },
    )
    .absolute()
}

/// The region's part, which renders above the window at its side. It's
/// styled and filled like any part; its state is [`ToastRegionState`].
pub struct ToastRegionPart {
    part: Part<ToastRegionState>,
    side: ToastSide,
    align: ToastAlign,
    rtl: bool,
    keyboard: TrackKeyboard,
}

impl ToastRegionPart {
    /// The region's state.
    pub fn state(&self) -> &ToastRegionState {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(
        mut self,
        style: impl FnOnce(Stateful<Div>, &ToastRegionState) -> Stateful<Div>,
    ) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters, as on any part.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.part = self.part.aria(aria);
        self
    }

    /// Changes the part underneath.
    pub fn map_part(
        mut self,
        map: impl FnOnce(Part<ToastRegionState>) -> Part<ToastRegionState>,
    ) -> Self {
        self.part = map(self.part);
        self
    }
}

impl Styled for ToastRegionPart {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl InteractiveElement for ToastRegionPart {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl StatefulInteractiveElement for ToastRegionPart {}

impl ParentElement for ToastRegionPart {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl IntoElement for ToastRegionPart {
    type Element = ToastRegionElement;

    fn into_element(self) -> Self::Element {
        ToastRegionElement {
            region: Some(self),
            child: None,
        }
    }
}

/// What a [`ToastRegionPart`] renders as: the part, in a deferred layer over
/// the window, at its side.
pub struct ToastRegionElement {
    region: Option<ToastRegionPart>,
    child: Option<AnyElement>,
}

impl Element for ToastRegionElement {
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
        let child = match self.region.take() {
            Some(region) => {
                let part = region_defaults(region.part, window.viewport_size());
                let frame = place_region(
                    part,
                    region.side,
                    region.align,
                    region.rtl,
                    region.keyboard.get().inset,
                    window.insets().safe_area,
                );
                // Above the on-screen keyboard: the frame follows it.
                let frame = region.keyboard.track(frame).absolute().top_0().left_0();
                deferred(window_box(frame))
                    .priority(PERSISTENT_LAYER_PRIORITY)
                    .into_any_element()
            }
            None => gpui::Empty.into_any_element(),
        };
        let child = self.child.insert(child);
        (child.request_layout(window, cx), ())
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
