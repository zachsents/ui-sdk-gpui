//! Port of `components/timeline`: dated events in order, like an activity
//! feed, a changelog or an order's progress, with one tab stop and the arrow
//! keys between events.
//!
//! `timeline.ts` → [`timeline`]. The parts are [`Timeline::root`],
//! [`Timeline::item`], and inside an item [`Timeline::marker`],
//! [`Timeline::connector`], [`Timeline::time`], [`Timeline::title`] and
//! [`Timeline::description`], each given its event's `value`. The parts
//! inside an item read the status their item rendered with, as the
//! original's take it from the item around them, so build them after the
//! item, as its children. The events' tab stop is `ui_sdk_focus`'s roving
//! focus. Each item's focus handle stands for its element, so an app hears
//! an event take focus through [`Timeline::item_focus_handle`], where the
//! original's app listens on the item.
//!
//! An event is named by its title and described by its time and description
//! through relations, which the kernel writes only while those parts render,
//! the original's `titles`, `times` and `descriptions` bookkeeping. In gpui
//! a div's plain text children name nothing, so the title and description
//! take the role accesskit gives text labels, and the time the `time` role,
//! which lets the relations find them; their text is the app's `aria_label`
//! on them, or accessible text. The marker and connector are marked hidden,
//! which gpui reports only for a part with a role; their plain content isn't
//! read anyway.
//!
//! # Dropped, and why
//!
//! - `createTimelineController`: the entity is the controller, and its
//!   `focus` moves focus to an event from anywhere.
//! - The inline styles `--timeline-count` and `--timeline-index`: gpui has
//!   no CSS variables, and [`TimelineState::count`] and
//!   [`TimelineItemState::index`] carry them.
//! - `selects`, `live` and `direct`, so a move renders only what it touches
//!   and a title binding renames its item without a render: gpui parts render
//!   from their entity's state every frame.
//! - The root's `MutationObserver` for events that move without binding
//!   again: the timeline reads where events are after every frame that
//!   renders it.
//! - A `value` generated when left out: a part there are many of needs its
//!   key in Rust.

pub mod timeline;

pub use timeline::{
    Timeline, TimelineConnectorState, TimelineDescriptionState, TimelineItemState,
    TimelineMarkerState, TimelineOptions, TimelineOrientation, TimelineState, TimelineStatus,
    TimelineTimeState, TimelineTitleState,
};
