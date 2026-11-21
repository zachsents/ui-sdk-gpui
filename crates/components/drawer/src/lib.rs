//! Port of `components/drawer`: a sheet you can open, drag and resize from
//! an edge of the window. It shares Dialog's focus, dismissal, nesting and
//! Close actions, from `ui-sdk-dialog`, and adds the popups crate's sheet:
//! snap points, dragging with iOS physics, a handle, modality that follows
//! the snap point, the keyboard, and stacked drawers pushing back the one
//! beneath.
//!
//! # TypeScript file → Rust module
//!
//! `drawer.ts` → [`drawer`] ([`Drawer`], its options, the props and state of
//! its parts, and the snap point attribute converters), `drawer-api.ts` →
//! [`drawer_api`] ([`DrawerState`], the snap point logic, what the sheet
//! reports, and [`step_snap_index`]).
//!
//! # In gpui
//!
//! - Render the backdrop, then the viewport with the content inside it,
//!   every frame: `Drawer::viewport(..)` goes above the window and hosts the
//!   sheet, hidden while the drawer is out of the page, and
//!   `Drawer::content(..)` is the sheet, which the drawer moves along its
//!   axis. Size the content yourself; no snap point is larger than it.
//! - The content takes presses from what's under it, as an element on top
//!   does in the DOM. With the scroll-snap driver it lets scrolls through,
//!   since the driver moves the sheet with its host's own scrolling.
//! - The sheet attaches as the content binds, before the popup layer shows
//!   it, and lets go once the content has left the page, the original's
//!   content `bind` and its cleanup, through the popup's `on_bind` and
//!   `on_unbind`. Its scroll areas and what a mouse press doesn't drag from
//!   are the content's `scroll_areas` and `no_drag`, which it hands the sheet
//!   (`Sheet::set_scroll_areas`, `Sheet::set_no_drag`).
//! - What the original writes as attributes and CSS variables is part
//!   state: `data-side`, `data-snap-point`, `data-expanded`,
//!   `data-dragging`, `data-interactive`, `data-driver`, `--drawer-dim`,
//!   `--drawer-progress`, `--drawer-expand` (`sheet`), `--drawer-stack`
//!   (`sheet_stack`), and the viewport's keyboard variables (`keyboard`).
//!   The backdrop's opacity is the sheet's dim, which the drawer sets.
//! - An outlet is a part rendered anywhere in the window. The sheet notifies
//!   the drawer as it moves, which renders the window again, so the outlet's
//!   state follows it.
//! - The snap point and `on_snap_point_change` are a kernel `Controllable`;
//!   a controlled `null` is `Some(None)`.
//!
//! # Dropped, and why
//!
//! - `createDrawerController`, `DrawerController`, and the trigger's and the
//!   outlet's `controller` props: the entity is the controller.
//!   `getSnapPoint` and `setSnapPoint` are [`Drawer::snap_point`] and
//!   [`Drawer::set_snap_point`], and parts render wherever the app renders
//!   them.
//! - `whenKept(prepareSheet)`: the popups crate dropped `prepareSheet`.
//! - The content's `bind`, which attaches the sheet before binding the popup
//!   layer, and its microtask for Vue: the content binds through the popup's
//!   own content binding, rendering in place inside its viewport. That binds
//!   as the drawer opens, not as the content first renders, so a drawer kept
//!   in the page gets its sheet at its first open, rising from closed as an
//!   open sheet does anyway, and keeps it until the content leaves.
//! - `isDragSurface`'s controls and `[data-drawer-no-drag]`, and
//!   `canMouseDrag`'s handle selector: gpui can't tell what's under a press.
//!   `no_drag` names what a mouse press doesn't drag from; the drawer adds
//!   its Close parts and leaves out its handle, a button that drags.
//! - The handle's `touch-action: none`: the sheet takes touches on its body
//!   itself. Its development check for a name (`checkName`): the handle is
//!   a button, which also takes its name from a `text!` child, and the
//!   drawer can't read it, as with the popups crate's triggers.
//! - `themeColor` reading the page's `<meta name="theme-color">`: the app
//!   hands its chrome colors ([`SheetThemeColor`](ui_sdk_popups::SheetThemeColor)),
//!   read as the sheet opens.
//! - `physics` as a partial override: a whole [`SheetPhysics`], built from
//!   `SheetPhysics::IOS`.
//! - The backdrop's `pointer-events: none` while the window behind is
//!   usable: it doesn't occlude then.
//! - `onOptions`' microtask: the sheet hears new options once the update
//!   that handed them over is done, as the popups crate hears a controlled
//!   `open`.
//! - `on_open_change_complete(true)` doesn't wait for the sheet's rise: the
//!   exit waits for its settle, which the drawer counts on the popup's
//!   presence as it closes, but a rise starts on the sheet's first drawn
//!   frame, after the enter started waiting. The popups crate's sheets do
//!   the same.
//! - `snapPointOf` keeping text it can't read as written: [`SnapPoint`] is
//!   typed, so text that names none is no snap point.

pub mod drawer;
pub mod drawer_api;

pub use drawer::{
    Drawer, DrawerBackdropState, DrawerCloseAction, DrawerCloseProps, DrawerCloseState,
    DrawerContent, DrawerContentElement, DrawerContentProps, DrawerContentState,
    DrawerDismissAttemptDetails, DrawerDismissReason, DrawerDriver, DrawerHandleState,
    DrawerKeyframes, DrawerOptions, DrawerOutletProps, DrawerOutletState, DrawerSide,
    DrawerSnapPointChangeCallback, DrawerTriggerProps, DrawerTriggerState, DrawerViewport,
    DrawerViewportState, snap_point_attribute, snap_point_of, snap_points_attribute,
};
pub use drawer_api::{DrawerState, HandleAction, step_snap_index};
pub use ui_sdk_motion::{SHEET_PHYSICS, SheetPhysics, SnapPoint};
