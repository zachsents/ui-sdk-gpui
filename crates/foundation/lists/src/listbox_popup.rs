//! Port of `foundation/lists/listbox-popup.ts`: what Combobox and
//! Autocomplete share as fields that pop up a list: the content the list
//! opens in, the list part inside it, and the state their field and its
//! buttons reflect.
//!
//! The content builds on the popups crate's [`popup_content`]: it renders
//! above the window, placed below its anchor, and presents as a sheet. What
//! `bindListContent` hands the presentation, the anchor, the placement and
//! where focus goes, is [`list_content_config`], which the component returns
//! from its popup rules' `bind`.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use gpui::{
    App, Context, Element, ElementId, Entity, EntityId, GlobalElementId, InspectorElementId,
    InteractiveElement, KeyDownEvent, LayoutId, Pixels, Bounds, Refineable, Style, StyleRefinement,
    Styled, Window, px,
};
use ui_sdk_focus::hold_focus;
use ui_sdk_kernel::roles::{HandledKey, Need, Role};
use ui_sdk_kernel::{Aria, Instance, InstanceHandle, Part};
use ui_sdk_platform::{Align, Anchor, ElementBounds, Side};
use ui_sdk_popups::{
    AttachConfig, InitialFocusFn, PlacementDefaults, Popup, PopupContent, PopupContentProps,
    PopupContentState, PopupHost, SheetConfig, content_anchor, placement_options, popup_content,
    sheet_snap_points,
};

use crate::async_options::AsyncOptionsStatus;
use crate::field_name::{FieldName, list_name};
use crate::listbox_parts::is_disabled;

/// What a field that pops up a list keeps for its field and its buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldState {
    pub open: bool,
    pub disabled: bool,
    pub form_disabled: bool,
    pub read_only: bool,
    pub required: bool,
}

/// What the field and the button that opens the list show (`inputState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputState {
    pub open: bool,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
}

/// What the field and the button that opens the list show (`inputState`).
pub fn input_state(state: &FieldState) -> InputState {
    InputState {
        open: state.open,
        disabled: is_disabled(state.disabled, state.form_disabled),
        read_only: state.read_only,
        required: state.required,
    }
}

/// The element id of what the field controls: the list part's while it's in
/// the page (`listed`), or the content's (`listId`).
pub fn list_id(listed: bool, instance: &Instance) -> ElementId {
    instance.element_id(if listed { "list" } else { "content" }, None)
}

/// What the list's content and list parts read from their component.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListPopup {
    /// Whether the list part is in the page, which then is the listbox
    /// instead of the content: `flag_while_bound(instance, "list")`.
    pub listed: bool,
    /// Whether a label part is in the page, which names the list.
    pub labeled: bool,
    /// What names the field, when no label part does.
    pub field_name: Option<FieldName>,
    /// A combobox's: options can be chosen together.
    pub multiple: bool,
    pub status: AsyncOptionsStatus,
    /// A combobox's: the list renders hidden while its items tell a value's
    /// label.
    pub prepared: bool,
}

// The keys the content takes, only while a screen reader moved real focus
// into the list. Otherwise the field's input takes them.
const LIST_CONTENT_KEYS: &[HandledKey] = &[
    "ArrowDown",
    "ArrowUp",
    "PageDown",
    "PageUp",
    "ctrl+n",
    "ctrl+p",
    "Home",
    "End",
    "Enter",
    " ",
    "typeahead",
];

/// The popup the list opens in (`listContentPart`). It's the listbox itself
/// until a list part renders inside it, next to a header or a status, and
/// then has no role of its own. A press on it leaves focus in the field.
/// `on_list_key_down` hears the keys pressed while focus is in the list,
/// inside the component's update. `None` while it's out of the page: it
/// renders while mounted, kept in the page with `keep_mounted`, and while
/// `prepared`, hidden.
pub fn list_content_part<C: PopupHost>(
    this: &Entity<C>,
    list: &ListPopup,
    mut props: PopupContentProps,
    on_list_key_down: impl Fn(&mut C, &KeyDownEvent, &mut Window, &mut Context<C>) + 'static,
    cx: &App,
) -> Option<PopupContent<C, PopupContentState<C::Payload>>> {
    props.keep_mounted |= list.prepared;
    let content = popup_content(this, props, cx)?;
    let host = this.read(cx);
    let instance = host.instance();
    let busy = !list.listed && list.status == AsyncOptionsStatus::Loading;
    let mut aria = Aria::new()
        .multiselectable(!list.listed && list.multiple)
        .busy(busy);
    if !list.listed {
        aria = aria.merge(list_name(list.labeled, list.field_name.as_ref(), instance));
    }
    let role = if list.listed { Role::Presentation } else { Role::Listbox };
    let entity = this.downgrade();
    Some(content.map_part(move |part| {
        part.aria_role(role)
            .aria(aria)
            .on_keys(LIST_CONTENT_KEYS, move |event, window, cx| {
                entity
                    .update(cx, |this, cx| on_list_key_down(this, event, window, cx))
                    .ok();
            })
            .on_any_mouse_down(hold_focus)
    }))
}

/// What a list's content binds with beside its popup's own
/// (`bindListContent`), for the component to return from its popup rules'
/// `bind`: placed below its anchor, `anchor` without an anchor prop, or
/// lined up over `aligned` instead, covering it, like a select's selected
/// item over its trigger; focus staying in the field unless `initial_focus`
/// moves it, like a screen reader's tap that opened the list; and as a
/// sheet, opening at half the screen at most and growing for the keyboard.
pub fn list_content_config<C: PopupHost>(
    popup: &Popup<C>,
    instance: &Instance,
    anchor: Option<Rc<dyn Fn() -> Option<Anchor>>>,
    aligned: Option<Rc<dyn Fn() -> Option<Anchor>>>,
    initial_focus: Option<InitialFocusFn>,
) -> SheetConfig<C> {
    let weak = popup.weak();
    let content_anchor_fn = {
        let weak = weak.clone();
        let aligned = aligned.clone();
        Rc::new(move || {
            let popup = weak.upgrade()?;
            content_anchor(popup.content_props().position.anchor.clone(), || {
                aligned
                    .as_ref()
                    .and_then(|aligned| aligned())
                    .or_else(|| anchor.as_ref().and_then(|anchor| anchor()))
            })
        }) as Rc<dyn Fn() -> Option<Anchor>>
    };
    let handle: InstanceHandle = instance.handle();
    let position = {
        let weak = weak.clone();
        Rc::new(move || {
            let Some(popup) = weak.upgrade() else {
                return ui_sdk_platform::PositionOptions::default();
            };
            let mut props = popup.content_props().position;
            // Lined up over its anchor, it covers it.
            if aligned.as_ref().and_then(|aligned| aligned()).is_some() {
                props.side = Some(Side::Bottom);
                props.align = Some(Align::Start);
                props.side_offset = Some(px(0.));
            }
            let arrow = {
                let measured = popup.arrow_bounds();
                let handle = handle.clone();
                ElementBounds::new(move || {
                    handle
                        .bound_parts()
                        .iter()
                        .any(|part| part.slot == "arrow")
                        .then(|| measured.get())
                        .flatten()
                })
            };
            placement_options(&props, PlacementDefaults::new(Align::Start), Some(arrow))
        }) as Rc<dyn Fn() -> ui_sdk_platform::PositionOptions>
    };
    SheetConfig {
        attach: AttachConfig {
            position: Some(position),
            anchor: Some(content_anchor_fn),
            initial_focus,
            ..AttachConfig::default()
        },
        snap_points: Some(sheet_snap_points()),
        ..SheetConfig::default()
    }
}

// What `ListSettled` puts in the content's element: it takes no space and
// draws nothing, and its element state clears the flag once a frame doesn't
// draw the content.
struct SettleProbe {
    settled: Rc<Cell<bool>>,
    entity: EntityId,
    style: StyleRefinement,
}

struct SettleToken(Weak<Cell<bool>>);

impl Drop for SettleToken {
    fn drop(&mut self) {
        if let Some(settled) = self.0.upgrade() {
            settled.set(false);
        }
    }
}

impl Styled for SettleProbe {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Element for SettleProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("ui-sdk-list-settled".into()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        if let Some(id) = id {
            let settled = self.settled.clone();
            let entity = self.entity;
            window.with_element_state::<SettleToken, _>(id, |token, window| {
                let token = token.unwrap_or_else(|| {
                    // Items render in the same frame as the content. Once
                    // it's drawn, an empty list is really empty.
                    let later = settled.clone();
                    window.defer(cx, move |_window, cx| {
                        if !later.replace(true) {
                            cx.notify(entity);
                        }
                    });
                    SettleToken(Rc::downgrade(&settled))
                });
                ((), token)
            });
        }
        let mut style = Style::default();
        style.refine(&self.style);
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

/// Whether a list's content has rendered with its items (the original's
/// `settled` flag, which `bindListContent` sets): true once the frame that
/// first drew the content is drawn, when an empty list is really empty, and
/// false again once a frame doesn't draw it. Keep one in a field and
/// [`track`](Self::track) the content's element. Clones share it.
#[derive(Clone, Default)]
pub struct ListSettled(Rc<Cell<bool>>);

impl ListSettled {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the content has settled.
    pub fn get(&self) -> bool {
        self.0.get()
    }

    /// Puts the flag's probe in the content's element, which notifies
    /// `entity`, the component, once it settles.
    pub fn track<E: gpui::ParentElement + Styled>(&self, entity: EntityId, el: E) -> E {
        el.child(
            SettleProbe {
                settled: self.0.clone(),
                entity,
                style: StyleRefinement::default(),
            }
            .absolute()
            .size_0(),
        )
    }
}

/// The listbox inside the content, when the content holds more than the
/// list (`listPart`). The field's input takes its keys, or the content
/// around it once focus is in it. The component reads whether it's in the
/// page with `flag_while_bound(instance, "list")`, which makes the content
/// a presentation.
pub fn list_part(instance: &Instance, list: &ListPopup) -> Part<()> {
    Part::new(instance, "list", None, ())
        .aria_role(Role::Listbox)
        .unneeded(
            Need::Keys,
            "The field's input handles them, or the content around the list once focus is in it.",
        )
        .aria(
            Aria::new()
                .multiselectable(list.multiple)
                .busy(list.status == AsyncOptionsStatus::Loading)
                .merge(list_name(list.labeled, list.field_name.as_ref(), instance)),
        )
}
