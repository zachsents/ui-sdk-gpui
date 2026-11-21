//! Port of `components/pagination`: pages, with a link or button for each
//! page shown, ellipses for the gaps, and steps to the previous, next, first
//! and last page.
//!
//! `pagination.ts` → [`pagination`], `pagination-range.ts` →
//! [`pagination_range`](mod@pagination_range). The parts are [`Pagination::root`],
//! [`Pagination::list`], [`Pagination::item`], [`Pagination::link`],
//! [`Pagination::ellipsis`], and the steps [`Pagination::previous_part`],
//! [`Pagination::next_part`], [`Pagination::first_part`] and
//! [`Pagination::last_part`], named apart from the api's `previous`, `next`,
//! `first` and `last`, which page it. Render a link for each page of
//! [`PaginationState::items`] and an ellipsis for each gap, each in an item
//! keyed by the entry, so a page's link keeps its focus handle as the range
//! moves.
//!
//! With `get_href`, parts are links: their URL is in their state for the app
//! to follow, since gpui has no browser to navigate. A press changes the page
//! and leaves following the URL to the app, which an app's own click
//! handler on the part, like a router's, does unless `window.default_prevented()`
//! says the change was canceled or the step is at its end. A press with a
//! modifier key or another button leaves the page as it is, for the app to
//! open the link elsewhere.
//!
//! The ellipsis is marked hidden, which gpui reports only for a part with a
//! role; its plain text isn't read anyway, since a div's text children
//! aren't accessible in gpui.
//!
//! # Dropped, and why
//!
//! - `createPaginationController`: the entity is the controller, and its
//!   methods page it from anywhere.
//! - The `held` step: a browser follows a link after the click, when a
//!   framework may already have rendered the new page, so a step link held
//!   its URL until then. A gpui part's handlers and state are the ones it
//!   rendered with, and nothing follows a URL after the press.
//! - The range's cache by key, which kept the same array so parts that
//!   render it don't render again: gpui parts render every frame from the
//!   entity's state.
//! - `partAttrs`' `type="button"`: there are no native buttons.

pub mod pagination;
pub mod pagination_range;

pub use pagination::{
    GetHref, Pagination, PaginationChangeOptions, PaginationLinkState, PaginationOptions,
    PaginationPageChangeReason, PaginationState, PaginationStep, PaginationStepState,
};
pub use pagination_range::{
    PaginationRangeItem, PaginationRangeOptions, clamp_page, clamp_page_number, page_count_of,
    pagination_range,
};
