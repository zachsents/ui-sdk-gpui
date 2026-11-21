//! Port of `foundation/popups`: building a popup, composing one, the layer
//! every overlay shares, hover intent, dismissal, modality, and the sheets
//! popups present as.
//!
//! # Building a popup
//!
//! A component built on a popup holds a [`Popup`] from [`create_popup`](fn@create_popup) in a
//! field and implements [`PopupHost`], which is how the popup reaches the
//! component's instance and itself from inside the component's update:
//!
//! ```ignore
//! struct Invite { instance: Instance, popup: Popup<Self> }
//!
//! impl PopupHost for Invite {
//!     type Payload = ();
//!     fn instance(&self) -> &Instance { &self.instance }
//!     fn popup(&self) -> &Popup<Self> { &self.popup }
//! }
//!
//! fn new(options: InviteOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
//!     let instance = Instance::new("invite").with_agent(cx);
//!     let popup = create_popup(&instance, CreatePopupOptions::new(popover_preset()).options(options.popup), window, cx);
//!     Self { instance, popup }
//! }
//! ```
//!
//! Its part methods call the builders in [`popup_slots`]: [`popup_trigger`],
//! [`popup_content`] (or [`bind_popup_content`] for a content part of its
//! own), [`popup_title`], [`popup_description`], [`popup_close`],
//! [`popup_arrow`], [`popup_backdrop`] and [`popup_viewport`]. Its api calls
//! the popup's: [`Popup::is_open`], [`Popup::set_open`], [`Popup::open`],
//! which returns a [`PopupPending`] that resolves once it closes,
//! [`Popup::close`], [`Popup::toggle`], [`Popup::payload`] and
//! [`Popup::focus`]. Its `set_options` hands the popup its
//! [`PopupOptions`] with [`Popup::set_options`]. Its controller is a kernel
//! `Controller` with [`PopupControllerExt`]'s methods. What a preset doesn't
//! cover goes in [`PopupRules`], the original's `rules`.
//!
//! A preset is a value: [`popover_preset`], [`dialog_preset`],
//! [`alert_dialog_preset`], [`drawer_preset`], [`menu_preset`],
//! [`select_preset`], [`combobox_preset`], [`date_picker_preset`],
//! [`trigger_menu_preset`], [`tooltip_preset`] and [`hover_card_preset`].
//!
//! # In gpui
//!
//! - The content renders above the window: in `deferred(..)` at the
//!   platform's `LAYER_PRIORITY`, placed by a `Positioner` while it's beside
//!   an anchor, else in a box the size of the window ([`window_box`]),
//!   painting the layer stack's listener. Wherever it renders, in place too,
//!   it occludes what's under it, as an element on top does in the DOM: no
//!   press, hover or scroll reaches what's beneath. A sheet with the
//!   scroll-snap driver lets scrolls through to the host that moves it,
//!   which takes the pointer over the window, or, while the sheet rests
//!   where the window stays usable, only over the sheet: its hitboxes are
//!   clipped to the sheet, the original's `pointer-events: none` on it.
//!   Parts that render that way are [`PopupContent`] and [`PopupBackdrop`],
//!   which style like any part; declare a backdrop before its content.
//! - The popup's state is plain state in the popup, so the layer stack never
//!   reads an entity that may be mid-update. Its open and payload are kernel
//!   `Controllable`s; a controlled `open` passed in new options mounts the
//!   popup once the update that passed it is over.
//! - The content binds as the popup opens, and as it renders open from the
//!   start; it unbinds once it leaves the page, open or closed, as the
//!   original's `bind` and its cleanup do. Focus moves in once the content
//!   has rendered, since tab order is the frame's.
//! - A modal popup can't make the rest of the window inert, so it paints a
//!   blocker beneath itself ([`modal::block_outside`]): hitboxes over the
//!   window, with holes for what stays usable, that take every press, hover
//!   and scroll from what's under them. That's the inert page and the scroll
//!   lock. Tab is trapped with `ui_sdk_focus::tab_within`, and focus moves
//!   with the kernel's `move_focus` and `restore_focus`.
//! - Hover comes from gpui's `on_hover` with the window's pointer
//!   ([`HoverEvent`]); while the pointer crosses the bridge between trigger and
//!   content, the content paints [`HoverIntent::listener`], which hears every
//!   move in the window.
//! - Focus is heard through the kernel's `focus_events`. Containment asks
//!   about a platform `Target`: a point is a press, a focus handle is focus,
//!   where the original passes a node. A press comes with its event, as the
//!   original's does ([`ContainsFn`]).
//! - The content binds through the popup, not its part, so a component that
//!   binds something to it, like a sheet, does it in the rules' `on_bind`
//!   and lets go in `on_unbind`, the content's own `bind` and its cleanup.
//!   The rules' `bind` reads the component as the content binds; `trigger`
//!   and `contains` read only what they hold, since the window's layers ask
//!   them from inside other components' updates.
//! - The back gesture stands in for Android's close watcher: the topmost
//!   dialog hears it.
//!
//! # Sheets
//!
//! A popup presents as a sheet with its `presentation` option, through
//! [`present_sheet`]: its content renders inside [`popup_viewport`], which
//! goes above the window with the sheet's dim, or hosts itself without one.
//! Drawer attaches a sheet to its own parts with [`attach_sheet`], renders
//! its host with [`Sheet::host`] and the sheet with [`Sheet::body`], and
//! moves it with [`Sheet::open`], [`Sheet::snap_to`] and the rest. The sheet
//! runs on `ui_sdk_motion`'s machine and physics. The pointer driver
//! ([`sheet_pointer`]) follows mouse and touch through a
//! `ui_sdk_drag_and_drop` session and samples each settle's spring every
//! frame; the scroll-snap driver ([`sheet_scroll_snap`]) moves it with the
//! host's own scrolling. The effects that follow it, the dim, its progress
//! and how far it pushes back the sheets beneath it, are values its parts
//! style from ([`SheetValues`], [`SheetPartState`], [`SheetStack`]), and
//! outlets are views it notifies as it moves ([`SheetOutlet`]). A sheet
//! closing calls `track_animation` on its popup's presence, so the exit
//! waits for it.
//!
//! # TypeScript file → Rust module
//!
//! `bridge` → [`bridge`], `create-popup` → [`create_popup`](mod@create_popup),
//! `dismissal` → [`dismissal`], `hover` → [`hover`], `modal` → [`modal`],
//! `modal-attribute` → [`modal_attribute`](mod@modal_attribute), `popup` → [`popup`],
//! `popup-behavior` → [`popup_behavior`], `popup-elements` →
//! [`popup_elements`], `popup-features` → [`popup_features`],
//! `popup-hover-presets` → [`popup_hover_presets`], `popup-payload` →
//! [`popup_payload`], `popup-presets` → [`popup_presets`](mod@popup_presets), `popup-slots` →
//! [`popup_slots`], `popup-steps` → [`popup_steps`], `presentation` →
//! [`presentation`], `safe-area` → [`safe_area`], `sheet` → [`sheet`],
//! `sheet-axis` → [`sheet_axis`], `sheet-driver` → [`sheet_driver`],
//! `sheet-effects` → [`sheet_effects`], `sheet-pointer` → [`sheet_pointer`],
//! `sheet-presentation` → [`sheet_presentation`], `sheet-scroll-snap` →
//! [`sheet_scroll_snap`], `theme-color` → [`theme_color`], `index` → these
//! re-exports.
//!
//! # Deviations and drops
//!
//! - `frame-guards.ts`: guards around a focused iframe so a focus trap hears
//!   Tab inside it. gpui embeds no frames.
//! - `resetPopoverStyles`, `prepareReset` and the presets' `resetStyles`
//!   (`styleReset`): undoing the browser's `[popover]` styles. gpui has none.
//!   `placement.prepare` (`preparePosition`) registers CSS properties ahead.
//! - `modal.ts`'s `measureScrollerEarly`, `scrollLockStyles` and the iOS
//!   touch guard (`guardTouch`, `touchOwner`): the scroll lock measures and
//!   restyles the document's scroller, and works around iOS Safari scrolling
//!   the page behind a modal. The blocker takes scrolls from what's under it
//!   in any window, touch included.
//! - `inertOutside`'s live regions: they stay usable since nothing is made
//!   inert for assistive technology; gpui can't hide the rest of the window
//!   from it, so a modal dialog says `aria-modal` instead.
//! - A form with `method="dialog"` closing its dialog with the submitter's
//!   value: gpui forms have no `method`. A Close part's `value` reports a
//!   return value.
//! - `selectFilled`, which selects a filled text field focus lands on as a
//!   popup opens, and the caret a popup puts back in the contenteditable it
//!   opened from: the popup can't tell a text field from another focus
//!   handle, and gpui's fields keep their selection through focus changes.
//! - `pressOpening.keydown`, Enter and Space on a trigger that isn't a
//!   button: gpui turns both into a keyboard click on any part with a press.
//! - Selectors as an `initialFocus`, `finalFocus` or `anchor`: they're focus
//!   handles, functions and `Anchor`s here.
//! - `triggerName` copies the trigger's name, since a modal dialog makes its
//!   trigger inert, which can't name it by reference then. gpui keeps the
//!   trigger in the accessibility tree, so an untitled dialog is labelled by
//!   its trigger part.
//! - The trigger's development check for a name (`checkName`): a trigger is
//!   a button, or often a link for a hover card, and either also takes its
//!   name from a `text!` child, which the popup can't read, so it would warn
//!   about every trigger named by its text.
//! - `popupSlots`, a set of slot definitions to spread: the builders are
//!   functions a component calls.
//! - `PopupRules.details`, more of what `onOpenChange` hears: a component's
//!   own callback carries it. The `on_open_change` option hears a close's
//!   return value beside the details: set it with
//!   [`on_open_change_with_value`](PopupOptions::on_open_change_with_value)
//!   to read it; the [`on_open_change`](PopupOptions::on_open_change)
//!   builder leaves it out.
//! - `PopupController`'s `Result` type: return values are `SharedString`,
//!   the original's default.
//! - While it animates out, its content and backdrop still take presses,
//!   where the original's are `inert` and let them through: gpui can't make
//!   a subtree let the pointer through, and content that stopped occluding
//!   would leave its children taking presses beside what's beneath.
//! - Content shown in place occludes the elements it's laid out in too, so
//!   they stop hearing the pointer over it, where the DOM keeps an ancestor
//!   hovered while its descendant is: gpui's occlusion covers everything
//!   that painted before it.
//! - `blur: "iframe"` closes on focus moving into an iframe outside, which
//!   gpui doesn't embed, so such a popup stays open when the window loses
//!   focus.
//! - Focus leaving a flowing popup is heard where it leaves the content or
//!   the trigger, not wherever it lands in the window.
//! - Showing again for nested popups open from the start (`showsAgain`) waits
//!   for the frame that rendered both.
//! - A tooltip's content kept in the page while closed renders hidden; gpui
//!   may leave a hidden element out of the accessibility tree, so the
//!   trigger's description can't point at it until it shows.
//!
//! The sheets:
//!
//! - WAAPI animations, `linear()` easings, `sampleTranslate`,
//!   `supportsLinear`, scroll timelines and `prepareVariables` (`prepareSheet`),
//!   which registers the sheet's CSS variables: a settle is a spring the
//!   driver samples every frame, and its effects are values computed from the
//!   same extent.
//! - The viewport's `::backdrop` and `--sheet-backdrop`: the viewport paints
//!   the dim, its color the content's `sheet_backdrop`.
//! - Scroll areas found by walking up from a press, and `isDragSurface`'s
//!   controls, inputs and `[data-drawer-no-drag]`: gpui can't list what's
//!   under a point, so the owner names its scroll areas
//!   ([`Sheet::set_scroll_areas`]) and what a mouse press doesn't drag from
//!   ([`Sheet::set_no_drag`]). A field that claims a touch keeps it.
//! - Touches heard on the document so the sheet can take over a scroll the
//!   browser started (`claimEarly`): gpui decides what a touch is as it
//!   starts, so the sheet follows its touches from the start and the machine
//!   scrolls the content under the finger itself.
//! - The scroll-snap driver's markers and native snapping: gpui's scrollers
//!   don't snap, so when the scroll rests the driver settles on the snap point
//!   the machine picks; scrolled out of view, it's dismissed. Its scroll areas
//!   aren't locked by the driver: [`SheetPartState::scroll_locked`] says when
//!   they should stay still.
//! - `revealFocused`, iOS's `focusWithoutScrolling`, and the sheet growing
//!   when a field in it takes focus: gpui can't read a focus handle's bounds
//!   or tell a field from another focus handle. A component calls
//!   [`Sheet::snap_to`] as its field takes focus.
//! - `isolate`, which hides a drilled-into popup's other elements:
//!   [`drilled_into`] tells that popup's content what to render instead.
//! - `<meta name="theme-color">`: the app hands its chrome colors
//!   ([`SheetThemeColor`]) and paints what the sheet writes.
//! - Inline styles, `will-change`, `user-select`, `overscroll-behavior` and
//!   pointer capture: the drivers write what gpui reads, the sheet's offset,
//!   and a drag session follows the pointer.
//!
//! The popup core is ported: create-popup, popup, popup-features,
//! popup-slots, popup-steps, popup-behavior, the presets, hover and its
//! presets, dismissal, safe-area, modal, modal-attribute, presentation,
//! popup-elements, popup-payload, bridge, theme-color and index. The sheets
//! are ported too.

pub mod bridge;
pub mod create_popup;
pub mod dismissal;
pub mod hover;
pub mod modal;
pub mod modal_attribute;
pub mod popup;
pub mod popup_behavior;
pub mod popup_elements;
pub mod popup_features;
pub mod popup_hover_presets;
pub mod popup_payload;
pub mod popup_presets;
pub mod popup_slots;
pub mod popup_steps;
pub mod presentation;
pub mod safe_area;
pub mod sheet;
pub mod sheet_axis;
pub mod sheet_driver;
pub mod sheet_effects;
pub mod sheet_pointer;
pub mod sheet_presentation;
pub mod sheet_scroll_snap;
pub mod theme_color;

pub use bridge::{Bridge, bridge_between, bridge_between_with};
pub use create_popup::{
    CreatePopupOptions, OpenChangeCompleteCallback, Popup, PopupControllerExt, PopupHost,
    PopupOpenChangeCallback, PopupOptions, PopupRules, PopupState, WeakPopup, create_popup,
    popup_state,
};
pub use dismissal::{
    DismissAttemptCallback, DismissOptions, DismissRules, Dismissal, Refusal, RefusalFn,
    RefusedFn, SheetDismissal, create_dismissal,
};
pub use hover::{HoverEvent, HoverIntent, HoverIntentOptions, create_hover_intent};
pub use modal::{Blocker, block_outside, blocked_rects};
pub use modal_attribute::{modal_attribute, pointer_modal_attribute};
pub use popup::{
    AbortSignal, AttachConfig, CloseDetails, CloseFn, ContainsFn, DisabledTriggerAttrs,
    EscapeFn, HookFn, InitialFocusFn, InitialFocusPlan, Modal, ObserveFn, OpenChangeFn,
    PopupClose, PopupCloseOptions, PopupCloseReason, PopupContentAttrs, PopupController,
    PopupFocusTarget, PopupLayer, PopupLayerOptions, PopupModal, PopupOpenOptions, PopupOpener,
    PopupPending, PopupResult, PopupTriggerAria, RegionFn, ReturnMode, ShowCleanup, ShowFn,
    create_popup_controller, derive_open, disabled_trigger_attrs, first_usable,
    initial_focus_plan, modal_triple, opener_by_source, opener_of, outside_press_focus,
    popup_content_attrs, popup_trigger_attrs, set_unfocusable, usable,
};
pub use popup_behavior::{
    BlockingSetting, PopupBehavior, PopupBehaviorName, PopupBehaviorOverrides, PopupBlur,
    PopupTab, TabSetting, popup_presets, resolve_behavior,
};
pub use popup_elements::{
    PopupElement, in_popup_element, owns_event, owns_event_among, register_popup_element,
};
pub use popup_features::{
    FocusOpening, HoverOpening, PlacementDefaults, PressOpening, TriggerName, WarmGroup,
    content_anchor, placement_options, resolve_initial_focus, takeover_reason,
};
pub use popup_hover_presets::{hover_card_preset, tooltip_preset};
pub use popup_payload::{InstanceOf, PayloadRestore, PopupPayload, create_popup_payload};
pub use popup_presets::{
    PopupPreset, alert_dialog_preset, combobox_preset, date_picker_preset, dialog_preset,
    drawer_preset, menu_preset, popover_preset, select_preset, trigger_menu_preset,
};
pub use popup_slots::{
    PopupArrowState, PopupBackdrop, PopupContent, PopupContentElement, PopupContentProps,
    PopupContentState, PopupInitialFocus, PopupPartState, PopupTriggerProps, PopupTriggerState,
    PopupViewport, PopupViewportElement, TriggerHandlers, WindowBox, bind_popup_content,
    popup_arrow, popup_backdrop, popup_backdrop_with, popup_close, popup_content,
    popup_description, popup_title, popup_trigger, popup_trigger_with, popup_viewport, window_box,
};
pub use popup_steps::{
    ChangeOpenFn, CloseRule, CloseStep, HideRequest, OpenChange, OpenRule, OpenRules, OpenStep,
    OpenSteps, OpenStepsOptions, SettledShow, ShowRequest, create_open_steps,
    create_stepped_popup, unopened,
};
pub use presentation::{
    DrillIn, OnSheet, Presentation, PresentationFeature, PresentationSetting, SheetConfig, SheetInput,
    bind_sheet_input, coarse_pointer, create_presentation, is_phone, sheet_modal,
    sheet_snap_points,
};
pub use safe_area::{SafeArea, SafeAreaAxis, create_safe_area, create_safe_area_with};
pub use sheet::{
    Sheet, SheetBody, SheetCallback, SheetHost, SheetLayer, SheetLayout, SheetOptions,
    SheetPartState, SheetThemeColor, ThemeSink, attach_sheet, is_drag_surface, parent_sheet,
    register_sheet_layer,
};
pub use sheet_axis::{
    ScrollArea, SheetSide, axis_point, is_vertical, resolve_side, scroll_area_at, scroll_context,
    scroll_edges, scroll_forward, size_along, toward_edge, translate_value,
};
pub use sheet_driver::{
    OPEN_HOLD, REDUCED_FADE, REDUCED_SETTLE, SettleMotion, SheetDriverName, SheetDriverOptions,
    SheetHook, TOOLBAR_BLEED, after_first_frame, closed_extent_for,
};
pub use sheet_effects::{
    SheetEffects, SheetEffectsTargets, SheetKeyframes, SheetOutlet, SheetStack, SheetValues,
    create_sheet_effects,
};
pub use sheet_pointer::{PointerDriver, create_pointer_driver};
pub use sheet_presentation::{drilled_into, is_drill_hidden, present_sheet};
pub use sheet_scroll_snap::{ScrollSnapDriver, create_scroll_snap_driver};
pub use theme_color::{ThemeColor, dim_theme_color};

// What the public API takes and gives from `ui_sdk_motion`: the content's
// and backdrop's presence and how they're timed, snap points, the sheet's
// physics and the machine its drivers run.
pub use ui_sdk_motion::SheetConfig as SheetMachineConfig;
pub use ui_sdk_motion::{
    Easing, Presence, PresencePhase, PresenceState, PresenceTiming, ResolvedSnapPoint,
    ScrollContext, SheetPhysics, SheetSnapReason, SnapPoint, Timing,
};
