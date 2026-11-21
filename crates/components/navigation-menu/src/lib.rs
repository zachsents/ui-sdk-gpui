//! Port of `components/navigation-menu`: a site's main links, grouped into
//! panels that open from a row of triggers.
//!
//! # TypeScript file → Rust module
//!
//! `navigation-menu.ts` → [`navigation_menu`](mod@navigation_menu) (the
//! options, states and parts), `navigation-menu-api.ts` →
//! [`navigation_menu_api`] (the [`NavigationMenu`] itself: its surfaces,
//! opening and closing, hover, links, the viewport and the contents),
//! `navigation-menu-focus.ts` → [`navigation_menu_focus`],
//! `navigation-menu-keyboard.ts` → [`navigation_menu_keyboard`],
//! `navigation-menu-dom.ts` → [`navigation_menu_dom`], `index.ts` → these
//! re-exports.
//!
//! An app makes a [`NavigationMenu`] with `cx.new(|cx|
//! NavigationMenu::new(options, window, cx))` and renders, first,
//! [`NavigationMenu::root`], named with `aria_label`, around
//! [`NavigationMenu::list`], which holds a [`NavigationMenu::item`] per
//! value with a [`NavigationMenu::trigger`] and its
//! [`NavigationMenu::content`], or a [`NavigationMenu::link`]; then, if it
//! wants them, the [`NavigationMenu::indicator`] and, after the list, the
//! [`NavigationMenu::viewport`]. A content holds links, each with an
//! optional [`NavigationMenu::link_icon`]. A menu made with its `parent`
//! option, inside another one's content, is nested: no landmark, and it
//! closes once its parent moves on to another item.
//!
//! It follows the disclosure navigation pattern. Resting the pointer on a
//! trigger opens its panel after `delay`, moving to another trigger while
//! one is open switches at once unless the pointer is only crossing it on
//! its way to the open panel, and the panel closes `close_delay` after the
//! pointer leaves it and its trigger. A press opens and closes it, and a
//! press on a panel the pointer opened keeps it open. Tab moves through the
//! triggers and top-level links; Tab from the open trigger moves into its
//! panel, wherever it renders, and on past it to the next item, and
//! Shift+Tab comes back. The arrows add to Tab: along the list between the
//! top-level items, Down (the inline end in a vertical menu) into a
//! trigger's panel, and between the links in a panel, with Home and End;
//! nothing wraps. Escape closes the panel and returns focus to its trigger,
//! and so does focus leaving the navigation or a press outside it, which
//! leave focus where it went.
//!
//! Every panel shows on a surface from `ui-sdk-popups`: a stepped popup over
//! one view of the value. With a viewport, the open content renders into
//! it, sized from its state's `size`, the open content's measured size, and
//! the viewport is placed against the open trigger; without one, each
//! content is its own popup placed against its trigger. Contents in the
//! viewport animate in and out with their own presence as items switch,
//! and say which way they move in `motion`.
//!
//! # Dropped, and why
//!
//! - `navigation-menu-generic.ts` (`NavigationMenuGeneric`): TypeScript's
//!   inference of the item value type. Values are `SharedString`.
//! - `createNavigationMenuController`, `NavigationMenuController` and the
//!   `controller` option: the app holds the entity, whose methods are the
//!   controller's.
//! - `container` and `PortalProps`: surfaces render above the window in
//!   `deferred(..)`, which needs no container.
//! - Find in page: `bindFindable`, `hidden="until-found"` and
//!   `collapseWhileHidden`. gpui has none, so `hidden_until_found` keeps a
//!   closed content in the page, hidden, as `keep_mounted` does, and a hidden
//!   content takes no room.
//! - `prepareReset` and `resetPopoverStyles`: the browser's `[popover]`
//!   styles. gpui has none.
//! - `triggerKeydown` from the disclosure, which clicks a trigger that isn't
//!   a button on Enter and Space: gpui clicks a focused part on both.
//! - `afterDispatch`, which closes after a link's click has reached handlers
//!   delegated higher up: the press defers the close until the event has
//!   run, so the app's own click, like a router's, sees the link first.
//! - Generated item values: items, triggers and contents take their item's
//!   value, and links a key of the app's.
//! - `data-*` attributes and the `--viewport-*` and `--active-item-*`
//!   variables: part state.
//!
//! # Deviations
//!
//! - A content in the viewport renders into it from its item: the content
//!   part hands itself to the viewport, which renders it after its own
//!   children. So render the root first, and the viewport after the list.
//!   The original portals it there.
//! - The viewport's `size` is the open content's; give the viewport its own
//!   border on top, where the original adds a `border-box` viewport's
//!   border to its variables.
//! - The indicator's box is relative to the root, where the original's is
//!   relative to the indicator's positioned ancestor, which gpui can't find:
//!   render it in the root.
//! - The list and its items are a list and list items, and a content is a
//!   group, since a gpui part without a role has no node for its trigger's
//!   `aria-controls` to point at. The original's are a `<ul>`, `<li>`s and a
//!   `<div>`.
//! - Every content and the viewport track a focus handle, which is how gpui
//!   finds what's inside them, so a press on one focuses it; a press
//!   anywhere in the root focuses the root. Both count as inside the
//!   navigation.
//! - The popup layer hears focus leave a flowing popup only from its
//!   content and its trigger, so the menu closes on focus leaving its root
//!   itself, wherever it leaves from, as the original's document listener
//!   does.
//! - The development check for a link with only an icon reads the name the
//!   link rendered with, its `aria_label` or a labelled-by: gpui can't read
//!   the text it shows.
//! - `set_options` and `set_controlled_value` take the window: a controlled
//!   value mounts its surface in the same pass, which animates on frames.

pub mod navigation_menu;
pub mod navigation_menu_api;
pub mod navigation_menu_dom;
pub mod navigation_menu_focus;
pub mod navigation_menu_keyboard;

pub use navigation_menu::{
    NavigationMenu, NavigationMenuChangeOptions, NavigationMenuContent, NavigationMenuContentProps,
    NavigationMenuContentState, NavigationMenuIndicatorState, NavigationMenuItemState,
    NavigationMenuLinkIconProps, NavigationMenuLinkIconState, NavigationMenuLinkProps,
    NavigationMenuLinkState, NavigationMenuListState, NavigationMenuMotion, NavigationMenuOptions,
    NavigationMenuOrientation, NavigationMenuParent, NavigationMenuPlacementProps, NavigationMenuState,
    NavigationMenuTriggerProps, NavigationMenuTriggerState, NavigationMenuValueChangeReason,
    NavigationMenuViewport, NavigationMenuViewportProps, NavigationMenuViewportState,
    OpenChangeCompleteCallback,
};
