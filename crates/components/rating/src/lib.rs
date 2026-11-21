//! Port of `components/rating`: a score picked from a row of stars, whole or
//! in parts, with a preview as a pointer moves over them.
//!
//! `rating.ts` → [`rating`](mod@rating), `index.ts` → these re-exports.
//!
//! A rating is a radio group, on `ui-sdk-radio`'s api and parts: one value,
//! one tab stop, arrow keys that check as they move (stopping at the ends
//! unless `loop` is set), Space to check, Enter to submit its form. An app
//! makes a [`Rating`] with `cx.new(|cx| Rating::new(options, window, cx))`
//! and renders [`Rating::root`], then for each star from 1 to
//! [`Rating::max`] a [`Rating::item`] with its icon and a [`Rating::radio`]
//! for each of [`Rating::values`], which sit over the icon and split it
//! between them. The item's state says how much of its star the shown score
//! covers. Name the rating with `aria_label`, or a label or field pointing at
//! it; a read-only rating is one image named by `get_value_label`.
//!
//! # Dropped, and why
//!
//! - `createRatingController`, `RatingController` and the `controller`
//!   option: the app holds the entity, whose methods are the controller's.
//! - `RatingItemProps`: the item part takes its index, which keys it.
//! - The `input` slot, a native radio per score for React and Vue: the
//!   rating reports to forms through its root's internals, as the web
//!   adapter's form-associated rating does.
//! - The `Restore` reason: gpui restores no forms after a reload.
//! - `--rating-fill` and `data-*` attributes: the item's state carries the
//!   fill, and whether it's filled or partial.
//!
//! # Deviations
//!
//! - The root ends a preview through its `on_hover`, which gpui lets an
//!   element have one of, and each radio starts one through its
//!   `on_mouse_move`, which only fires while the pointer is over it. gpui's
//!   mouse moves carry no pointer type, so a touch screen that moves the
//!   mouse previews too.
//! - The radios' hit areas are absolutely positioned inside their star, which
//!   the item part makes `relative`, as the original's inline styles do.

pub mod rating;

pub use rating::{
    Rating, RatingChangeOptions, RatingItemState, RatingOptions, RatingState,
    RatingValueChangeReason, ValueLabelFn, rating_item_values,
};
