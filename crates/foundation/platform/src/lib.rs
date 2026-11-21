//! Port of `foundation/platform`: what the platform gives every behavior,
//! mapped to gpui: the layout direction, focus and containment, keyboard
//! shortcuts, the stack of open layers, placement beside an anchor,
//! screen-reader announcements, the on-screen keyboard and the viewport.
//!
//! The original reads all of this from the DOM. Here:
//!
//! - An element is a [`Region`]: its measured bounds, its hitbox and its
//!   focus handle. Containment asks about a [`Target`]: a point, like a press,
//!   or an element's focus handle.
//! - Layers are a stack per window ([`push_layer`]). A layer's element paints
//!   [`listener`] so the window hears presses, which also tells the layer
//!   where its element is, for [`in_exiting_layer`]: the original's
//!   `[data-ending-style]` on a popup animating out.
//! - A popup's content renders in [`positioned`], usually inside
//!   `deferred(..)` at [`LAYER_PRIORITY`], and its [`Positioner`] places it on
//!   every frame.
//! - Announcements reach assistive technology through [`live_regions`], which
//!   the app renders once in each window; [`announce()`] takes the shape
//!   `AnnouncePending` and other callers pass, `(&str, &mut Window, &mut App)`.
//! - What the original writes as CSS variables and `data-*` attributes is
//!   state parts read: [`Placement`], [`KeyboardInset`], [`ViewportState`].
//!
//! # Dropped
//!
//! - `anchoring.ts`: CSS anchor positioning, which lets the browser place a
//!   popup. [`positioned`] always places it.
//! - `moves.ts`: watching what can move a popup or its anchor once both sit
//!   still (scrolls, resizes, mutations, loads, animations, shadow roots).
//!   gpui draws a frame for anything that moves, and [`positioned`] places the
//!   popup on every frame it draws; a virtual rectangle anchor is read every
//!   frame while shown.
//! - `env.ts` `isBrowser`: there's always a window.
//! - `platform.ts` `portalTarget` and `composedHops`: portals, which
//!   `deferred` replaces, and shadow roots, which gpui doesn't have.
//! - `position.ts` `preparePosition`, which registers CSS properties ahead;
//!   `PortalProps`, a container to render into, which `deferred` replaces;
//!   `exactSize` and `untransformed`, which undo CSS box sizing, offset
//!   rounding, transforms and `zoom`, none of which gpui has; anchors given as
//!   CSS selectors; and the first placement's `transition: none`, since gpui
//!   never transitions a popup's position ([`Positioner::is_placed`] tells a
//!   first placement).
//! - `style.ts` `setStyle`, `writeStyle`, `enforceHidden` and
//!   `registerUninherited`: inline styles, stylesheets and CSS properties.
//!   gpui styles come from render, and a hidden part isn't rendered.
//! - `keyboard.ts` `measureViewportEarly` and the shared viewport listeners:
//!   reading the viewport costs nothing and needs no listener here, and
//!   content never widens a window's layout viewport. The keyboard's height
//!   comes from the window's insets, not from watching a field take focus.
//! - `viewport.ts`'s hidden elements that read `env(safe-area-inset-*)` and
//!   the iOS visual-viewport origin: the window's insets and bounds say both.
//! - `scrollbar.ts` `pressedScrollbar`: gpui draws no native scrollbars, so a
//!   press never lands on one, and layers have none to skip.
//! - `button.ts` `buttonKeydown`, `buttonKeyup` and the `"unknown"` element:
//!   gpui turns Enter and Space on a focused element into a keyboard click
//!   itself (Enter on release too), and there's no server render.
//! - `bind-layer.ts` `topLayer`: rendering in `deferred(..)` is the app's.
//! - `layers.ts`: the click a consumed press owes, which never forms, since
//!   consuming stops the press; the touch move check, which gpui's tap
//!   recognizer makes; `raiseAbovePopups`, which
//!   [`PERSISTENT_LAYER_PRIORITY`] replaces; per-document listening, which is
//!   per window.
//! - `announce.ts`: the regions' CSS hiding and `data-ui-sdk-announcer`.

pub mod announce;
pub mod bind_layer;
pub mod button;
pub mod env;
pub mod keyboard;
pub mod layers;
pub mod layout;
pub mod platform;
pub mod position;
pub mod scrollbar;
pub mod sides;
pub mod style;
pub mod viewport;

pub use announce::{
    AnnouncePriority, Announcer, announce, announce_with, live_regions, prepare_announcements,
    set_announcer,
};
pub use bind_layer::{
    BindLayerContains, BindLayerOptions, BoundLayer, DismissCallback, LayerDismissReason, bind_layer,
};
pub use button::{
    ButtonElement, ButtonElementAttrs, ButtonElementOptions, LinkTarget, button_element,
    button_element_attrs, button_role, opens_elsewhere,
};
pub use env::{Direction, direction, is_ios, is_rtl, set_direction};
pub use keyboard::{
    KeyboardChange, KeyboardInset, TrackKeyboard, ViewportSize, keyboard_height, keyboard_inset,
    measure_viewport, track_keyboard,
};
pub use layers::{
    BlurCallback, EscapeCallback, KeepRegion, LAYER_PRIORITY, Layer, LayerContains, LayerOptions,
    OutsidePress, OutsidePressCallback, OutsidePressEvent, OverBackdrop, PERSISTENT_LAYER_PRIORITY,
    PersistentLayer, PressKind, PushedLayer, ReturnTarget, active_layers, bind_persistent_layer,
    escape_layer, find_layer, in_exiting_layer, in_persistent_layer, last_closed, last_escape,
    layer_contains, layer_count, listen, listener, mark_closed, opened_since, persistent_layers,
    push_layer,
};
pub use layout::{LayoutBox, layout_box, layout_origin, layout_size};
pub use platform::{
    Platform, Region, Target, active_element, aria_keyshortcuts, composed_closest,
    composed_contains, format_shortcut, has_command_modifier, initial_apple, is_apple,
    matches_shortcut, prefers_reduced_motion, set_platform, supports_top_layer,
};
pub use position::{
    Align, Anchor, ArrowPlacement, ComputedPlacement, ElementBounds, Placement, PositionOptions,
    PositionProps, Positioned, Positioner, VirtualAnchor, anchor_rect, arrow_offset,
    compute_arrow, compute_placement, position, positioned, transform_origin,
};
pub use scrollbar::{ScrollbarBox, on_scrollbar};
pub use sides::{PhysicalSide, Side, opposite, physical_side};
pub use style::{AfterPaint, after_paint, ahead};
pub use viewport::{
    AvoidKeyboard, BindViewport, ViewportState, bind_avoid_keyboard, bind_viewport,
    viewport_state,
};
