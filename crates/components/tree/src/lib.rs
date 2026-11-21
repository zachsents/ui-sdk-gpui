//! Port of `components/tree`: a hierarchy of files, pages or choices whose
//! branches expand, as a WAI-ARIA tree, with one tab stop, typeahead, single
//! or multiple selection, tri-state checkboxes, children that load when a
//! branch first opens, and groups that animate open and closed.
//!
//! `tree.ts` → [`tree`](mod@tree), `tree-api.ts` → [`tree_api`], `index.ts`
//! → these re-exports, which also carry the lists crate's tree shape and
//! checks the original re-exports.
//!
//! An app makes a [`Tree`] with `cx.new(|cx| Tree::new(options, window,
//! cx))` and renders its parts: [`Tree::root`], then for each item
//! [`Tree::item`] around its [`Tree::label`] (with a [`Tree::indicator`] and
//! a [`Tree::checkbox`] in it) and its [`Tree::group`], which holds the
//! items nested in it and is `None` while it's out of the page. Give the
//! tree its shape with `items` or `paths`, and render [`Tree::items`]; or
//! render items of your own and give each its `parent`, `branch` and
//! `label` in [`TreeItemProps`]. Rows can also render flat, from
//! [`visible_tree_rows`], since the data gives the levels and places.
//!
//! The entity is the controller: `expand`, `collapse`, `toggle`,
//! `expand_all`, `collapse_all`, `set_expanded`, `set_value`, `set_checked`,
//! `reveal` and `focus` take the place of `createTreeController()`. Dragging
//! items into a new order and renaming them in place compose with it as in
//! the original, from `ui_sdk_drag_and_drop::create_tree_reorder` and
//! `ui_sdk_lists::create_inline_edit` wired to the item parts; the example
//! does both.
//!
//! # Dropped, and why
//!
//! - `tree-generic.ts`: TypeScript's inference of the value's shape from
//!   `mode`. [`TreeValue`] holds either shape.
//! - `createTreeController`, `TreeController` and the `controller` option:
//!   the app holds the entity, whose methods are the controller's.
//! - Rendering data items for a tree with no children of its own: the
//!   adapters' `TreeItems`. The app renders the parts, from
//!   [`Tree::items`], which it can style as it does any part.
//! - A label that's a link (`a[href]`, its `tabindex` and Enter clicking
//!   it), and the data's `href`: gpui has no links; an app opens an item's
//!   address in `on_action`.
//! - Find in page and `hidden="until-found"`: gpui has neither. A group with
//!   `hidden_until_found` stays in the page, hidden, as `keep_mounted` keeps
//!   it, and `FindInPage` stays in the reasons' vocabulary.
//! - `selects` and `live`, which let a change render only the items it
//!   touches: an entity's parts render again when it notifies.
//! - The root's `MutationObserver` and the per-parent revisions: items
//!   register as they render, which is page order, and once each frame is
//!   drawn the tree compares them with the last frame's, so items that
//!   joined, left or moved rebuild the shape and their places then.
//! - `data-*` attributes, `--tree-level` and `--tree-group-height`, and the
//!   group's `data-starting-style` and `data-ending-style`: part state
//!   ([`TreeItemState`], [`TreeGroupState`]).
//! - Server rendering: `claims` keeps the first frame to one tab stop, as it
//!   keeps server markup to one.
//!
//! # Deviations
//!
//! - An item handles keys only while it holds focus itself, so a control in
//!   it, like a rename field, keeps its keys: gpui's text field edits after
//!   the keydown listeners, so a tree that took typed characters for
//!   typeahead would stop the edit. In the original the control stops them.
//! - Enter and Space reach an item's keydown, which handles them as the
//!   original's does; the click gpui makes of them on release is ignored.
//! - Items nest, so a press on an item reaches the items around it; the
//!   innermost takes it and stops it there (`ownEvent`). A press on a group
//!   itself, between items, focuses its branch, as the browser focuses the
//!   element around it that takes focus.
//! - A press doesn't take focus from a control inside the item that holds
//!   it, where the original keeps it only for text fields: gpui can't tell
//!   a text field from another control.
//! - In the original the item around an item passes `parent`, and a group
//!   among its children makes it a branch. gpui parts have no context to
//!   pass, so an item of your own says both, and its `label`: a div's text
//!   names nothing in gpui, so the label part and typeahead read the
//!   `label` prop, or the data's label. The label part is named with it.
//! - `on_action` and `load_children` return the kernel's `ActionOutcome`,
//!   its port of the Promise the original reads with `isPromise`: `Pending`
//!   work marks the item pending, or the branch loading.
//! - [`TreeLoadSignal`] stands in for the `AbortSignal` `load_children`
//!   gets. Aborting a load also drops its work.
//! - `on_highlight`'s index is `None` where the original's is -1.
//! - `group_timing` is the transition the original's CSS declares for a
//!   group, and `scroller` the element `reveal` scrolls an item into view in
//!   (`scrollIntoView`), which gpui can't find from the item.
//! - [`Tree::focused_value`] tells which item holds focus, to style its row:
//!   the original styles a focused row with `:focus-visible` on the item
//!   around its label, which gpui's styles can't reach.
//! - `set_options` and the controlled setters take the window: a change they
//!   bring, like new data or a new `current`, follows in the same pass.
//! - Focus that a collapse would drop from an item that leaves goes to its
//!   outermost collapsed ancestor, found from the item's last `parent` when
//!   the tree has no data.

pub mod tree;
pub mod tree_api;

pub use tree::{
    Tree, TreeActionCallback, TreeActionDetails, TreeActivation, TreeAnnouncement,
    TreeAnnouncementFn, TreeAnnouncementType, TreeChangeOptions, TreeCheckboxState,
    TreeCheckedChangeReason, TreeExpandedChangeReason, TreeGroupProps, TreeGroupState,
    TreeHighlightCallback, TreeIndicatorState, TreeItemProps, TreeItemState, TreeLabelState,
    TreeLoadChildren, TreeLoadDetails, TreeLoadSignal, TreeMode, TreeOptions, TreeState,
    TreeValue, TreeValueChangeReason,
};
pub use ui_sdk_lists::{
    ListHighlightDetails, TreeCheckStrategy, TreeCheckedState, TreeCheckedStates, TreeCollection,
    TreeCollectionNode, TreeItemData, TreeRow, create_tree_collection, toggle_tree_checked,
    tree_checked_states, tree_data_nodes, tree_from_paths, visible_tree_rows,
};
