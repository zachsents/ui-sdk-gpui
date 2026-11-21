//! Port of `components/tabs`: panels that share one space, switched with a
//! list of tabs, one selected at a time.
//!
//! `tabs.ts` → [`tabs`](mod@tabs), `index.ts` → these re-exports.
//!
//! An app makes [`Tabs`] with `cx.new(|cx| Tabs::new(options, window, cx))`
//! and renders [`Tabs::root`] around [`Tabs::list`], which holds a
//! [`Tabs::tab`] per value, each with an optional [`Tabs::tab_icon`], and,
//! if it wants one, the [`Tabs::indicator`], and a [`Tabs::panel`] per
//! value, which is `None` while the panel is out of the page. Give each tab and its panel the same value, and the list an
//! `aria_label`. Without a `default_value`, the first enabled tab is
//! selected, without calling `on_value_change`.
//!
//! The list is one tab stop, on the selected tab, and runs on
//! `ui-sdk-focus`'s roving focus: arrow keys along its orientation, Home and
//! End move between tabs, wrapping unless `loop` is off, and select them,
//! unless activation is manual, where Enter or Space selects the focused
//! tab. Disabled tabs stay focusable and never select. Right to left, Left
//! and Right follow the reading direction.
//!
//! Panels show while their tab is selected, and animate in and out with a
//! `ui-sdk-motion` presence, never on the first paint: the panel a change
//! leaves stays in the page through its exit, then leaves or, with
//! `keep_mounted`, hides. `panel_timing` is the transition the original's
//! CSS declares. The indicator's box comes from `ui-sdk-selection`'s
//! `bind_indicator`, measured from the list, and `direction` says which way
//! the selection moved.
//!
//! # Dropped, and why
//!
//! - `tabs-generic.ts` (`TabsGeneric`): TypeScript's inference of the value
//!   type for renderers. Values are `SharedString`.
//! - `createTabsController`, `TabsController` and the `controller` option:
//!   the app holds the entity, whose methods are the controller's.
//! - Find in page: `bindFindable` and `hidden="until-found"`. gpui has
//!   neither, so `hidden_until_found` keeps a panel in the page, hidden, as
//!   `keep_mounted` does, and `FindInPage` is a reason only a call passes.
//! - The streamed default (`streamed`), a server's selection kept while its
//!   boundary hydrates: there's no server render.
//! - The `elements` state and a button's `type`: every part is a div, and a
//!   tab renders as a link through its `link` prop.
//! - `selects`, `live`, the `panels` state and `onTabsChange`: the parts
//!   render from the entity's state, the kernel leaves `aria-controls` out
//!   while the panel isn't in the page, and the indicator measures as the
//!   list and its tabs lay out.
//! - Clicking a native button or link on Enter and Space: gpui clicks a
//!   focused part on both itself.
//! - `data-*` attributes and the `--active-tab-*` variables: part state.
//!
//! # Deviations
//!
//! - gpui scrolls nothing into view as it takes focus, so a list made to
//!   scroll brings the selected tab, and each tab arrow keys focus, into
//!   view itself, where the original leaves focused tabs to the browser.
//! - `set_options` and `set_controlled_value` take the window: a new value
//!   starts its panels' animations in the same pass, which run on animation
//!   frames.

pub mod tabs;

pub use tabs::{
    Tabs, TabsActivation, TabsChangeOptions, TabsDirection, TabsIndicatorState, TabsListState,
    TabsOptions, TabsOrientation, TabsPanelProps, TabsPanelState, TabsState, TabsTabIconProps,
    TabsTabIconState, TabsTabProps, TabsTabState, TabsValueChangeReason,
};
