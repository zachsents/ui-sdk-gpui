//! Port of `foundation/selection`: choosing items and extending a
//! selection, selecting a rectangle of items or of grid cells, and an
//! indicator over the active item.
//!
//! | Original | Here |
//! | --- | --- |
//! | `selection-model.ts` | [`selection_model`]: [`Selection`], [`create_selection`], [`KeySelection`], [`create_key_selection`] |
//! | `listbox-range.ts` | [`listbox_range`]: [`range_selection`], [`toggled_keys`], [`all_toggled`], [`keys_between`], [`is_same_keys`] |
//! | `lasso.ts` | [`lasso`]: [`Lasso`], [`create_lasso`] |
//! | `cell-range.ts` | [`cell_range`]: [`CellRangeSelection`], [`create_cell_range`], [`cells_to_text`], [`text_to_cells`] |
//! | `indicator.ts` | [`indicator`]: [`Indicator`], [`bind_indicator`], [`direction_between`] |
//! | `index.ts` | these re-exports |
//!
//! # In gpui
//!
//! Each behavior is a handle the component makes in its constructor and
//! keeps in a field. Where the original reads and writes a state key, it
//! reads the component through the getters its options give, and asks for
//! changes through `set`, which runs inside the component's update with
//! the component, so calls that may change something take `this` after a
//! clone of the handle. Keys are generic, `K`, where the original's are
//! strings; the details of a change are the kernel's
//! [`ChangeOptions`](ui_sdk_kernel::ChangeOptions), so `set` hands them to
//! `Controllable::change`.
//!
//! Drags follow the pointer through a drag session from
//! `ui-sdk-drag-and-drop`, whose listener the component renders on every
//! frame. Items' boxes come from their last layout: a lasso reads the
//! kernel [`Parts`](ui_sdk_kernel::Parts) of its slot, a cell range the
//! bounds `GridFocus::measure` records, an indicator its own measures.
//!
//! `isSameKeys` is defined in drag and drop, which selection builds on, and
//! re-exported from [`listbox_range`].
//!
//! # Deviations and drops
//!
//! - `createSelection` hears its value through the store. Here it compares
//!   the value with the one it saw last as each call starts and after its
//!   own `set`, and [`Selection::sync`] hears one that arrived from outside;
//!   `anchor` takes the component to hear it first.
//! - The lasso's rectangle part: [`Lasso::style`] places it inside the
//!   surface or hides it, the original's `style` string.
//! - `byCellRange`, a `selects` reader so a range change renders only the
//!   cells it touches: gpui has no subscriptions per part; cells render from
//!   their component.
//! - `cellAttrs` returns [`CellRangeCell`], the state its attributes say.
//! - A cell range's drag scrolls the scrollers `auto_scroll` names, since
//!   gpui can't find the ones under the pointer.
//! - `bindIndicator`'s `list`, `active`, `itemSelector` and `subscribe`: the
//!   component measures the list and its items, and names the active one as
//!   it renders the indicator, which reads the box then; a layout that moves
//!   it renders the component again and calls `write`.

pub mod cell_range;
pub mod indicator;
pub mod lasso;
pub mod listbox_range;
pub mod selection_model;

pub use cell_range::{
    CellBounds, CellRange, CellRangeCell, CellRangeOptions, CellRangeReason, CellRangeSelection,
    CountFn, RangeEdges, RangeFn, RangeMessageFn, RevealFn, SetRangeFn, cell_range_message,
    cells_to_text, create_cell_range, text_to_cells,
};
pub use indicator::{Indicator, IndicatorOptions, WriteFn, bind_indicator, direction_between};
pub use lasso::{
    Lasso, LassoDetails, LassoDraft, LassoMessageFn, LassoOptions, SetLassoFn, create_lasso,
};
pub use listbox_range::{
    RangeAnchor, all_toggled, is_same_keys, keys_between, range_selection, toggled_keys,
};
pub use selection_model::{
    ExtendOptions, KeySelection, KeySelectionConfig, KeysFn, MultipleFn, SelectableFn, Selection,
    SelectionModel, SelectionOptions, SetKeysFn, create_key_selection, create_selection,
};
