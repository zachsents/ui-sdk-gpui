//! Port of `components/waterfall`: a trace's spans, or a page's requests, as
//! bars on a shared time axis, with spans that expand to show what they
//! started. It's a WAI-ARIA treegrid: one tab stop, rows that move like a
//! tree's, and cells that Right enters and Left leaves.
//!
//! `waterfall.ts` → [`waterfall`](mod@waterfall), `waterfall-api.ts` →
//! [`waterfall_api`], `waterfall-time.ts` → [`waterfall_time`], `index.ts` →
//! these re-exports.
//!
//! An app makes a [`Waterfall`] with `cx.new(|cx| Waterfall::new(options,
//! window, cx))`, gives it every span as `items`, and renders its parts:
//! [`Waterfall::root`], an optional [`Waterfall::header_row`] of
//! [`Waterfall::column_header`]s holding the [`Waterfall::axis`] with its
//! [`Waterfall::tick`]s and any [`Waterfall::mark`], then a
//! [`Waterfall::row`] per span, flat, with its [`Waterfall::row_header`]
//! (and a [`Waterfall::indicator`] for a span with children), any
//! [`Waterfall::cell`]s, and its [`Waterfall::bar`]. Rows of spans inside a
//! collapsed one hide themselves, or the app renders only the rows
//! [`visible_waterfall_rows`] gives. A row's state says where its bar goes
//! on the axis, from 0 to 100.
//!
//! The entity is the controller: `expand`, `collapse`, `toggle`,
//! `expand_all`, `collapse_all`, `set_expanded`, `set_value`, `reveal` and
//! `focus` take the place of `createWaterfallController()`.
//!
//! # Dropped, and why
//!
//! - `createWaterfallController`, `WaterfallController` and the `controller`
//!   option: the app holds the entity, whose methods are the controller's.
//! - Rendering a row for every span, and the axis's ticks, when the app
//!   renders none (`internal.all()`): the adapters'. The app renders the
//!   parts, from the axis's state and its spans.
//! - `bindPageLocale`: the parts read the app's language as they render
//!   (`ui_sdk_text::locale_of`), so a new one shows at once.
//! - `selects` and `direct`, which let a change render only the rows it
//!   touches and write `hidden` without a render: an entity's parts render
//!   again when it notifies.
//! - The cached ticks, kept so a render reads the same array: the axis works
//!   them out as it renders.
//! - The inline styles `--waterfall-level`, `--waterfall-start`,
//!   `--waterfall-size`, `--waterfall-tick` and `--waterfall-mark`, and the
//!   `data-*` attributes: part state.
//!
//! # Deviations
//!
//! - The original finds a row's cells in its DOM. Here each cell part joins
//!   its row as it renders, so build a row's header, cells and bar after the
//!   row, as its children; column headers and cells number themselves by the
//!   order they render in.
//! - Focus landing on a row or a cell (`focusin`) is read once each frame is
//!   drawn, since gpui tells where focus is when it draws.
//! - Page Up and Page Down move by the height of the `scroller` the app
//!   names, the scroller around the rows that the original finds: gpui can't
//!   find it from a row. Without one, they move 10 rows, as the original
//!   does with no scroller.
//! - Enter and Space reach a row's keydown, which selects it; the click gpui
//!   makes of them on release is ignored.
//! - Spans are the lists crate's `TreeData`, as the original passes them
//!   where `TreeItemData` goes, so [`visible_waterfall_rows`] is its
//!   `visible_tree_rows`, whose rows borrow the spans they show.
//! - `value` is `Option<Option<SharedString>>`: `Some(None)` controls the
//!   selection to nothing, `None` leaves it to the waterfall.
//! - `on_highlight`'s index is `None` where the original's is -1.
//! - `set_options` and `set_controlled_expanded` take the window: a
//!   collapse they bring moves focus out of the rows it hides in the same
//!   pass.

pub mod waterfall;
pub mod waterfall_api;
pub mod waterfall_time;

pub use ui_sdk_lists::ListHighlightDetails;
pub use ui_sdk_number::NumberFormat;
pub use waterfall::{
    Waterfall, WaterfallAxisProps, WaterfallAxisState, WaterfallBarLabelDetails,
    WaterfallBarLabelFn, WaterfallChangeOptions, WaterfallData, WaterfallDataItem,
    WaterfallDataRow, WaterfallExpandedChangeReason, WaterfallHighlightCallback,
    WaterfallOptions, WaterfallRowState, WaterfallState, WaterfallTimeState,
    WaterfallValueChangeReason, visible_waterfall_rows,
};
pub use waterfall_time::{
    Placement, Range, Span, WaterfallTickData, format_time, percent_at, span_placement, span_range,
    tick_times, waterfall_ticks,
};
