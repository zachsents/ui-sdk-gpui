//! Port of `components/toast`: notifications that stack, close on their own,
//! offer an action like Undo, swipe away, and are announced to screen readers
//! without taking focus.
//!
//! A [`Toaster`] is the queue: `toast(cx).show("Saved", cx)` adds to the
//! shared one from anywhere there's an app, and [`create_toaster`] makes
//! another for a second region. A [`ToastRegion`] shows a toaster's toasts:
//! the app renders its [`region`](ToastRegion::region) once, anywhere in the
//! window's tree, and inside it a [`root`](ToastRegion::root) for each of the
//! region's [`toasts`](ToastRegion::toasts), with their
//! [`title`](ToastRegion::title), [`description`](ToastRegion::description),
//! [`action`](ToastRegion::action) and [`close`](ToastRegion::close) parts.
//! The window renders `ui_sdk_platform::live_regions(window)` once for the
//! announcements.
//!
//! # TypeScript file → Rust module
//!
//! | Original | Here |
//! | --- | --- |
//! | `toaster.ts` | [`toaster`]: [`Toaster`], [`create_toaster`], [`toast`](fn@toast), [`Toast`], [`ToastOptions`] and the action, cancel, type and reason types |
//! | `toast.ts` | [`toast`](mod@toast): [`ToastRegion`], its options, and the region's, toasts' and buttons' states |
//! | `toast-api.ts` | [`toast_api`]: what's on screen, for how long, how it's announced and where focus goes |
//! | `toast-parts.ts` | [`toast_parts`]: the parts, and [`ToastRegionPart`], the region above the window |
//! | `toast-timers.ts` | [`toast_timers`]: [`ToastTimers`], [`ToastPauseReason`] |
//! | `toast-dom.ts` | [`toast_dom`]: where the region sits, the stack's measurements, swipe directions, where focus goes, attention |
//! | `swipe.ts` | [`swipe`]: [`SwipeDirection`], the swipe's math, [`track_swipe`] |
//! | `index` (the toast exports) | these re-exports |
//!
//! # In gpui
//!
//! - The toaster is an entity its regions observe, so a toast added inside
//!   another component's update shows once that update is over. Its calls
//!   take the app; `on_dismiss` gets the app too, since a toast can close
//!   with no window at hand, like when a promise settles.
//! - Content is text: titles, descriptions and button labels are
//!   `SharedString`, where the original's `Content` can be any framework
//!   node, since a gpui element is used once and the region announces the
//!   text. Anything else a toast renders goes in `data`.
//! - Promise toasts take a future, which runs as a task on the app's main
//!   thread: where the original takes a function that makes the promise, the
//!   app calls it. The original's third argument goes in `loading`.
//! - The toaster's call, `toast(message, options)`, is [`Toaster::show`], and
//!   its second argument is the builder on the options text converts to.
//! - Durations are `Duration`s; `Duration::MAX` stands for `Infinity`.
//! - A toaster with no region the app placed opens an overlay of kind
//!   [`TOASTER_OVERLAY`] in its `ui_sdk_page` registry from its first toast
//!   on, whose data is the [`Toaster`]. The overlay host is the app's view,
//!   which renders a region for it made with `placed(false)`.
//! - The region renders above everything in `deferred(..)` at
//!   `PERSISTENT_LAYER_PRIORITY`, as the original's top layer, laid out at
//!   its side of the window over the keyboard and inside the safe area: the
//!   original's default styles. The region and each toast on the page are
//!   persistent layers, so presses on them never dismiss a popup and modal
//!   layers leave them usable, and presses on them don't reach what's
//!   beneath.
//! - The shortcut registers in `ui_sdk_shortcuts`' shared set while the
//!   region is on the page, belonging to the region, so it runs over every
//!   modal.
//! - CSS variables and `data-*` attributes are state: the region's
//!   [`ToastRegionState`] has the front and stack heights and the count, a
//!   toast's [`ToastState`] its index, offset, height, swipe and presence,
//!   for the app to lay the stack out from.
//! - Enter and exit run on each toast's `ui_sdk_motion` presence, with the
//!   region's `timing` option standing in for its CSS transitions. A toast
//!   pushed out that comes back before its exit ends eases back in from
//!   where it was.
//! - Swipes follow the pointer through the region's drag session, which
//!   claims a touch as it starts, the original's `touch-action: none`.
//! - Hover is the pointer over the region, or over a toast that reaches out
//!   of it, as `pointerenter` counts descendants; focus comes and goes
//!   through the kernel's focus events, which say where it came from.
//! - Heights are a toast's laid-out height, read after the frame that drew
//!   it, while it's in front or the stack is expanded. Leave a toast's height
//!   to its content then: gpui can't lay it out again at `height: auto`, as
//!   the original's `naturalHeight` does, so a height the app holds it to
//!   would read back as its own.
//! - The region is a focusable container, so focus in it can be told; a
//!   press on its background takes no focus. While a toast animates out it
//!   takes no presses, keys or focus and is hidden from assistive
//!   technology, as the original's `inert` has it.
//! - The title and description parts are labels named with the toast's
//!   text, which names and describes its root; their visible text is a plain
//!   child. The action part is named with the action's label.
//! - A toast is read by the text of its title and description parts that
//!   are on the page, or both: gpui can't read back what an element shows.
//! - The action and close parts are keyed by their toast's id, and an `id`
//!   prop when a toast has several, where the original's adapters key each
//!   part they render.
//! - A link action's `href` is the app's to follow, as gpui has no browser
//!   to navigate; the part is a link and its press dismisses the toast.
//! - `focus` and the shortcut move focus with `window.focus`, and Tab past
//!   the edge needs `ui_sdk_focus::bind_tab_keys`.
//!
//! # Dropped, and why
//!
//! - `resetPopoverStyles`, `addToastStyles`' stylesheet, `writeHeight`,
//!   `showPopover` and `touch-action`: CSS and the top layer. Their effects
//!   are the layout above and state the app styles from.
//! - `pressedText`, the selectable-text check before a mouse swipe, and
//!   clearing the selection as a swipe starts: gpui selects no text in parts.
//! - The `input, textarea, select, [contenteditable]` check before a swipe: a
//!   text field handles its own press and stops it.
//! - The height measurement's wait for running height transitions, and its
//!   resize and mutation observers: gpui has no CSS transitions, and each
//!   frame that draws a toast measures it.
//! - The window blur's iframe check: gpui windows hold no iframes.
//! - `connections`, which counts binds of one region for React's StrictMode,
//!   and `Toaster<Content>`'s `isOptions`, which tells content from options:
//!   a region is on the page or not, and text converts to options.
//! - `shortcutAttribute`, which parses a web component's attribute.
//! - `createToastTimers`' `now`, the clock tests swap for fake timers.
//! - `globalThis` sharing of the shared toaster and its internals between
//!   copies of the package: a Rust app links one copy.
//! - An update can't take a field away by passing it `undefined`, which an
//!   object spread does: a field it leaves out keeps the toast's.

pub mod swipe;
pub mod toast;
pub mod toast_api;
pub mod toast_dom;
pub mod toast_parts;
pub mod toast_timers;
pub mod toaster;

pub use swipe::{
    SwipeAxis, SwipeCallbacks, SwipeDirection, SwipeRelease, along, swipe_axis, swipe_offset,
    swipe_outcome, track_swipe,
};
pub use toast::{
    ToastActionProps, ToastAlign, ToastButtonState, ToastCloseProps, ToastItem, ToastRegion,
    ToastRegionOptions, ToastRegionState, ToastSide, ToastState,
};
pub use toast_api::{find_item, has_toasts};
pub use toast_dom::{focus_after_close, stack_layout, swipe_directions};
pub use toast_parts::{ToastRegionElement, ToastRegionPart};
pub use toast_timers::{TimedToast, ToastPauseReason, ToastTimers, create_toast_timers};
pub use toaster::{
    PromiseMessage, RegionAttachment, TOASTER_OVERLAY, Toast, ToastAction, ToastActionDetails,
    ToastActionHandler, ToastCancel, ToastCancelHandler, ToastDismissCallback, ToastDismissDetails,
    ToastDismissReason, ToastOptions, ToastPromise, ToastType, Toaster, ToasterOptions,
    create_toaster, toast,
};
pub use ui_sdk_platform::AnnouncePriority;
