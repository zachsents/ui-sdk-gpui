//! Port of `foundation/drag-and-drop/tree-reorder.ts`: reordering a tree: an
//! outline, a sidebar of pages, folders of files. The tree is a flat list of
//! rows in reading order, each with its depth, so an item and its children
//! are one block of rows. A drag moves the block up and down between the
//! rows on screen, and sideways in and out of levels; the keyboard does the
//! same with the arrows. Like [`create_reorder`](crate::create_reorder), the
//! rows move through a draft that renders as it changes, only the drop
//! commits, and screen readers hear each step.
//!
//! Rows join as they render with [`TreeReorder::bind`] and record their
//! bounds with [`TreeReorder::measure`]. Only rows that render take part, so
//! the rows inside a collapsed item can stay in the value unrendered.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fmt::Display;
use std::hash::Hash;
use std::rc::{Rc, Weak};

use gpui::{
    App, Bounds, Context, FocusHandle, IntoElement, KeyDownEvent, ParentElement, Pixels, Point,
    SharedString, Styled, Window, px,
};
use ui_sdk_kernel::{ChangeOptions, InputEvent, Key, Reason};
use ui_sdk_platform::{AnnouncePriority, announce_with, is_rtl};
use ui_sdk_text::label_of;

use crate::auto_scroll::{AutoScroll, AutoScrollOptions, create_auto_scroll};
use crate::drag_preview::{DragPreview, bind_drag_preview};
use crate::draft_move::{self, DraftMove, DraftOps, Items, MessageType, MoveDraft};
use crate::pointer::{BindDragOptions, DRAG_THRESHOLD, DragSession, PointerEvent, PointerSample};
use crate::reorder::{KeyFn, MessageFn, ReadFn, drop_index};

/// One row of a tree in reading order: its key and how deep it is, from 0
/// (`TreeReorderRow`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TreeReorderRow<K> {
    pub key: K,
    pub depth: usize,
}

impl<K> TreeReorderRow<K> {
    pub fn new(key: K, depth: usize) -> Self {
        Self { key, depth }
    }
}

/// The item being moved and the rows as they show now
/// (`TreeReorderDraft`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeReorderDraft<K> {
    /// The key of the item picked up.
    pub key: K,
    /// The item and every row under it, which move with it.
    pub keys: Vec<K>,
    /// Every row as it shows now, the moving ones included.
    pub rows: Vec<TreeReorderRow<K>>,
    /// Whether a pointer is dragging it, rather than the keyboard moving it.
    pub pointer: bool,
}

impl<K: Clone + PartialEq + 'static> MoveDraft for TreeReorderDraft<K> {
    type Key = K;

    fn key(&self) -> &K {
        &self.key
    }

    fn pointer(&self) -> bool {
        self.pointer
    }
}

/// Why the rows changed (`TreeReorderReason`): a pointer drag (`Drag`), a
/// keyboard move (`MoveKey`), or a call from code (`Imperative`).
pub type TreeReorderReason = Reason;

/// Where an item went: under which parent, and its place among that
/// parent's children (`TreePlace`), what `set`'s details carry besides the
/// item, the reason and the event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreePlace<K> {
    /// The item that moved.
    pub key: K,
    /// The parent's key, or `None` at the top level.
    pub parent: Option<K>,
    /// Its place among the parent's children, from 0.
    pub index: usize,
}

/// What happened, for the message screen readers hear
/// (`TreeReorderMessage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeReorderMessage<K> {
    /// `Grab` when an item is picked up, `Move` for each step, `Drop` when it
    /// lands, `Cancel` when it goes back.
    pub r#type: MessageType,
    /// The item's key.
    pub key: K,
    /// What the item is called, from `label`, else its text.
    pub label: SharedString,
    /// What its parent is called, or `None` at the top level.
    pub parent: Option<SharedString>,
    /// Its level, from 1.
    pub level: usize,
    /// Its place among its siblings, from 1.
    pub position: usize,
    /// How many siblings it has, itself included.
    pub total: usize,
}

/// The English messages [`create_tree_reorder`] announces unless you pass
/// `message` (`treeReorderMessage`).
pub fn tree_reorder_message<K>(message: &TreeReorderMessage<K>) -> String {
    let within = match &message.parent {
        Some(parent) => format!(" in {parent}"),
        None => String::new(),
    };
    let at = format!(
        "level {}{within}, position {} of {}",
        message.level, message.position, message.total
    );
    let label = &message.label;
    match message.r#type {
        MessageType::Grab => format!("Picked up {label}, {at}."),
        MessageType::Move => format!("{label}, {at}."),
        MessageType::Drop => format!("Dropped {label} at {at}."),
        MessageType::Cancel => format!("Moved {label} back to {at}."),
    }
}

/// The index past the last row under `rows[index]` (`subtreeEnd`).
pub fn subtree_end<K>(rows: &[TreeReorderRow<K>], index: usize) -> usize {
    let mut end = index + 1;
    while end < rows.len() && rows[end].depth > rows[index].depth {
        end += 1;
    }
    end
}

/// The parent of `rows[index]`, by its index, and the row's place among the
/// parent's children (`placeOf`).
pub fn place_of<K>(rows: &[TreeReorderRow<K>], index: usize) -> (Option<usize>, usize) {
    let depth = rows[index].depth;
    let mut parent = None;
    let mut position = 0;
    for i in (0..index).rev() {
        if rows[i].depth == depth {
            position += 1;
        } else if rows[i].depth < depth {
            parent = Some(i);
            break;
        }
    }
    (parent, position)
}

/// The depths a block can take between two rows without changing anyone
/// else's parent: no deeper than the row above's child, and no shallower
/// than the row below, which would adopt it (`depthRange`). Returns the
/// least and the most, with `max_depth` capping the most.
pub fn depth_range<K>(
    above: Option<&TreeReorderRow<K>>,
    below: Option<&TreeReorderRow<K>>,
    max_depth: Option<usize>,
) -> (usize, usize) {
    let max = above.map_or(0, |above| above.depth + 1);
    let max = max_depth.map_or(max, |cap| cap.min(max));
    let min = below.map_or(0, |below| below.depth);
    (min.min(max), max)
}

// The deepest an item can go so the deepest row under it stays within
// `max_depth`.
fn block_max_depth<K: Eq + Hash>(
    rows: &[TreeReorderRow<K>],
    keys: &HashSet<K>,
    max_depth: Option<usize>,
) -> Option<usize> {
    let max_depth = max_depth?;
    let block: Vec<&TreeReorderRow<K>> = rows.iter().filter(|row| keys.contains(&row.key)).collect();
    let first = block.first().map_or(0, |row| row.depth);
    let deepest = block.iter().map(|row| row.depth).max().unwrap_or(first);
    Some(max_depth.saturating_sub(deepest - first))
}

/// The rows with the block at `key` moved before the row `before`, or to the
/// end, at `depth`, its children keeping their depths under it
/// (`movedRows`). The same rows when it's already there.
pub fn moved_rows<K: Clone + PartialEq>(
    rows: &[TreeReorderRow<K>],
    key: &K,
    before: Option<&K>,
    depth: usize,
) -> Vec<TreeReorderRow<K>> {
    let Some(from) = rows.iter().position(|row| row.key == *key) else {
        return rows.to_vec();
    };
    let end = subtree_end(rows, from);
    let shift = depth as isize - rows[from].depth as isize;
    let block: Vec<TreeReorderRow<K>> = rows[from..end]
        .iter()
        .map(|row| TreeReorderRow {
            key: row.key.clone(),
            depth: (row.depth as isize + shift).max(0) as usize,
        })
        .collect();
    let mut rest: Vec<TreeReorderRow<K>> =
        rows[..from].iter().chain(&rows[end..]).cloned().collect();
    let at = match before {
        None => rest.len(),
        Some(before) => match rest.iter().position(|row| row.key == *before) {
            Some(at) => at,
            None => return rows.to_vec(),
        },
    };
    if shift == 0 && at == from {
        return rows.to_vec();
    }
    rest.splice(at..at, block);
    rest
}

// `moved_rows` with the depth held where the block fits between its new
// neighbors.
fn placed_rows<K: Clone + Eq + Hash>(
    rows: &[TreeReorderRow<K>],
    key: &K,
    before: Option<&K>,
    depth: isize,
    max_depth: Option<usize>,
) -> Vec<TreeReorderRow<K>> {
    let Some(from) = rows.iter().position(|row| row.key == *key) else {
        return rows.to_vec();
    };
    let block: HashSet<K> = rows[from..subtree_end(rows, from)]
        .iter()
        .map(|row| row.key.clone())
        .collect();
    let rest: Vec<&TreeReorderRow<K>> = rows.iter().filter(|row| !block.contains(&row.key)).collect();
    let at = match before {
        None => Some(rest.len()),
        Some(before) => rest.iter().position(|row| row.key == *before),
    };
    let (above, below) = match at {
        Some(at) => (at.checked_sub(1).map(|above| rest[above]), rest.get(at).copied()),
        None => (None, None),
    };
    let (min, max) = depth_range(above, below, block_max_depth(rows, &block, max_depth));
    moved_rows(rows, key, before, clamp_depth(depth, min, max))
}

fn clamp_depth(depth: isize, min: usize, max: usize) -> usize {
    depth.clamp(min as isize, max as isize).max(0) as usize
}

fn same_rows<K: PartialEq>(a: &[TreeReorderRow<K>], b: &[TreeReorderRow<K>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.key == b.key && a.depth == b.depth)
}

/// What a row tells [`TreeReorder::bind`] about itself as it renders.
#[derive(Clone, Debug, Default)]
pub struct TreeReorderItem {
    /// What the row says, for the message screen readers hear when no
    /// `label` names it (the original reads the element's text).
    pub text: Option<SharedString>,
    /// The focus handle its element tracks, which a keyboard move keeps
    /// focus on.
    pub focus: Option<FocusHandle>,
}

impl TreeReorderItem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn focus(mut self, handle: &FocusHandle) -> Self {
        self.focus = Some(handle.clone());
        self
    }
}

/// Asks the component for new rows, with where the item went and why, and
/// returns whether they went through. It runs inside the component's update.
pub type SetRowsFn<T, K> = Rc<
    dyn Fn(&mut T, Vec<TreeReorderRow<K>>, TreePlace<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool,
>;

/// How [`create_tree_reorder`] reads, renders and changes the rows
/// (`TreeReorderOptions`). `T` is the component, `K` a row's key.
pub struct TreeReorderOptions<T: 'static, K> {
    /// The rows in reading order, the component's value (`key`).
    pub value: ReadFn<T, Vec<TreeReorderRow<K>>>,
    /// Asks for the new rows and returns whether they went through. Pass the
    /// component's change. The [`TreePlace`] says which item moved and where
    /// to; `details.reason` is `Drag`, `MoveKey` or `Imperative`.
    pub set: SetRowsFn<T, K>,
    /// How far a drag goes sideways for each level. Defaults to 24.
    pub indent: Option<ReadFn<T, Pixels>>,
    /// The deepest an item and the rows under it can go, from 0. Defaults to
    /// no limit.
    pub max_depth: Option<ReadFn<T, usize>>,
    /// Whether an item can't be picked up. It still moves aside for others.
    pub disabled: Option<KeyFn<T, K, bool>>,
    /// The name screen readers hear for an item. Defaults to its text.
    pub label: Option<KeyFn<T, K, Option<SharedString>>>,
    /// The text screen readers hear for each step. Defaults to English
    /// ([`tree_reorder_message`]).
    pub message: Option<MessageFn<TreeReorderMessage<K>>>,
    /// Drags a copy of the row above the page while the rows show where it
    /// would drop. Defaults to false.
    pub preview: bool,
    /// How a drag with `preview` near a scroller's edge scrolls it, with the
    /// scrollers it may scroll, or `None` not to.
    pub auto_scroll: Option<AutoScrollOptions>,
}

impl<T: 'static, K> Clone for TreeReorderOptions<T, K> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            set: self.set.clone(),
            indent: self.indent.clone(),
            max_depth: self.max_depth.clone(),
            disabled: self.disabled.clone(),
            label: self.label.clone(),
            message: self.message.clone(),
            preview: self.preview,
            auto_scroll: self.auto_scroll.clone(),
        }
    }
}

impl<T: 'static, K: 'static> TreeReorderOptions<T, K> {
    /// Options with the rows and how to change them.
    pub fn new(
        value: impl Fn(&T) -> Vec<TreeReorderRow<K>> + 'static,
        set: impl Fn(&mut T, Vec<TreeReorderRow<K>>, TreePlace<K>, ChangeOptions, &mut Window, &mut Context<T>) -> bool
        + 'static,
    ) -> Self {
        Self {
            value: Rc::new(value),
            set: Rc::new(set),
            indent: None,
            max_depth: None,
            disabled: None,
            label: None,
            message: None,
            preview: false,
            auto_scroll: Some(AutoScrollOptions::default()),
        }
    }

    pub fn indent(mut self, indent: impl Fn(&T) -> Pixels + 'static) -> Self {
        self.indent = Some(Rc::new(indent));
        self
    }

    pub fn max_depth(mut self, max_depth: impl Fn(&T) -> usize + 'static) -> Self {
        self.max_depth = Some(Rc::new(max_depth));
        self
    }

    pub fn disabled(mut self, disabled: impl Fn(&T, &K) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(disabled));
        self
    }

    pub fn label(mut self, label: impl Fn(&T, &K) -> Option<SharedString> + 'static) -> Self {
        self.label = Some(Rc::new(label));
        self
    }

    pub fn message(mut self, message: impl Fn(&TreeReorderMessage<K>) -> String + 'static) -> Self {
        self.message = Some(Rc::new(message));
        self
    }

    pub fn preview(mut self, preview: bool) -> Self {
        self.preview = preview;
        self
    }

    pub fn auto_scroll(mut self, auto_scroll: Option<AutoScrollOptions>) -> Self {
        self.auto_scroll = auto_scroll;
        self
    }
}

// The pointer drag in progress.
struct PointerDrag<K> {
    key: K,
    start: Point<Pixels>,
    at: Point<Pixels>,
    start_depth: usize,
    dragging: bool,
    last_move: Option<PointerEvent>,
    preview: Option<(DragPreview, usize)>,
    scroller: Option<AutoScroll>,
}

struct Inner<T: 'static, K: Clone + PartialEq + 'static> {
    options: RefCell<TreeReorderOptions<T, K>>,
    moves: RefCell<DraftMove<TreeReorderDraft<K>, Vec<TreeReorderRow<K>>>>,
    items: Items<K, TreeReorderItem>,
    drags: DragSession<T>,
    pointer: RefCell<Option<PointerDrag<K>>>,
    // The depth the keyboard aims for: where the item started, or where Left
    // and Right last put it, so moving through a deeper branch doesn't keep
    // it there.
    aim: Cell<usize>,
}

/// The copy of the row a pointer drags, for the component to render: which
/// row, how many rows move with it (`data-drag-count` when more than one),
/// and where the copy is.
#[derive(Clone, Debug)]
pub struct TreeReorderPreview<K> {
    pub key: K,
    pub count: usize,
    pub preview: DragPreview,
}

impl<K> TreeReorderPreview<K> {
    /// The copy: `content` where the preview is now. Render it without the
    /// row's moving or dragging state.
    pub fn render<E: IntoElement>(&self, content: E) -> impl IntoElement + use<E, K> {
        self.preview.render(content)
    }
}

/// Lets users reorder and nest a tree's rows (`createTreeReorder`'s
/// return). Cloning it gives another handle to the same behavior.
pub struct TreeReorder<T: 'static, K: Clone + PartialEq + 'static> {
    inner: Rc<Inner<T, K>>,
}

impl<T: 'static, K: Clone + PartialEq + 'static> Clone for TreeReorder<T, K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// The rows on screen other than the moving block, the block's slot among
// them, and its depth.
struct Slots<K> {
    others: Vec<TreeReorderRow<K>>,
    slot: usize,
    depth: usize,
}

/// Lets users reorder and nest a tree's rows by dragging them and with the
/// keyboard (`createTreeReorder`). Render the rows from
/// [`rows`](TreeReorder::rows), each indented by its depth, and leave out
/// the ones inside collapsed items: only rows that render take part. Make it
/// in the component's constructor, keep it in a field, render
/// [`listener`](TreeReorder::listener) on every frame, and wire the row
/// part: `bind` and `measure` it, run `tree.keydown(..)` before its roving
/// focus's, and hand its presses to `tree.pointer_down(..)` through
/// `on_pointer_down`.
///
/// Space or Enter picks a row up with everything under it; then Up and Down
/// move it between the rows on screen, Left and Right take it out a level
/// and in one, Space or Enter drops it, and Escape puts it back. A drag goes
/// in a level for every `indent` it goes sideways. An item never takes
/// another's children: where the row below is deeper, the item goes in as
/// deep.
pub fn create_tree_reorder<T: 'static, K: Clone + Eq + Hash + Display + 'static>(
    options: TreeReorderOptions<T, K>,
    cx: &Context<T>,
) -> TreeReorder<T, K> {
    TreeReorder {
        inner: Rc::new(Inner {
            options: RefCell::new(options),
            moves: RefCell::new(DraftMove::default()),
            items: Items::new(),
            drags: DragSession::new(cx),
            pointer: RefCell::new(None),
            aim: Cell::new(0),
        }),
    }
}

impl<T: 'static, K: Clone + Eq + Hash + Display + 'static> DraftOps<T> for TreeReorder<T, K> {
    type Draft = TreeReorderDraft<K>;
    type Value = Vec<TreeReorderRow<K>>;

    fn moves(&self) -> &RefCell<DraftMove<TreeReorderDraft<K>, Vec<TreeReorderRow<K>>>> {
        &self.inner.moves
    }

    fn value(&self, this: &T) -> Vec<TreeReorderRow<K>> {
        let value = self.inner.options.borrow().value.clone();
        value(this)
    }

    fn shown(&self, draft: &TreeReorderDraft<K>) -> Vec<TreeReorderRow<K>> {
        draft.rows.clone()
    }

    fn same(&self, a: &Vec<TreeReorderRow<K>>, b: &Vec<TreeReorderRow<K>>) -> bool {
        same_rows(a, b)
    }

    fn rebase(
        &self,
        this: &T,
        current: &TreeReorderDraft<K>,
        now: &Vec<TreeReorderRow<K>>,
        moved: bool,
    ) -> Option<TreeReorderDraft<K>> {
        let index = now.iter().position(|row| row.key == current.key)?;
        let keys: Vec<K> = now[index..subtree_end(now, index)]
            .iter()
            .map(|row| row.key.clone())
            .collect();
        if !moved {
            return Some(TreeReorderDraft {
                keys,
                rows: now.clone(),
                ..current.clone()
            });
        }
        let present: HashSet<&K> = now.iter().map(|row| &row.key).collect();
        let at = current
            .rows
            .iter()
            .position(|row| row.key == current.key)
            .unwrap_or(0);
        let before = current.rows[at..]
            .iter()
            .find(|row| {
                present.contains(&row.key)
                    && !current.keys.contains(&row.key)
                    && !keys.contains(&row.key)
            })
            .map(|row| row.key.clone());
        let depth = current.rows.get(at).map_or(0, |row| row.depth);
        let rows = placed_rows(
            now,
            &current.key,
            before.as_ref(),
            depth as isize,
            self.max_depth(this),
        );
        Some(TreeReorderDraft {
            keys,
            rows,
            ..current.clone()
        })
    }

    fn commit(
        &self,
        this: &mut T,
        current: &TreeReorderDraft<K>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        self.commit_rows(this, current.rows.clone(), &current.key, options, window, cx)
    }

    fn say(
        &self,
        this: &T,
        r#type: MessageType,
        current: &TreeReorderDraft<K>,
        rows: &Vec<TreeReorderRow<K>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.say_in(this, r#type, &current.key, rows, window, cx);
    }

    fn element(&self, key: &K) -> Option<FocusHandle> {
        self.inner.items.get(key).and_then(|item| item.focus)
    }
}

impl<T: 'static, K: Clone + Eq + Hash + Display + 'static> TreeReorder<T, K> {
    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: TreeReorderOptions<T, K>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn options(&self) -> TreeReorderOptions<T, K> {
        self.inner.options.borrow().clone()
    }

    fn max_depth(&self, this: &T) -> Option<usize> {
        let max_depth = self.inner.options.borrow().max_depth.clone();
        max_depth.map(|max_depth| max_depth(this))
    }

    /// Starts a render of the rows, from the part that holds them, every
    /// frame it renders. Binding a row starts one too.
    pub fn begin(&self, cx: &App) {
        self.inner.items.begin(cx);
    }

    /// Joins a row as it renders (`bind`). A row that stops rendering, like
    /// one inside a collapsed item, leaves.
    pub fn bind(&self, key: K, item: TreeReorderItem, cx: &App) {
        self.inner.items.bind(key, item, cx);
    }

    /// Adds a layer to a row's element that records its bounds each layout,
    /// which a drag finds its place by.
    pub fn measure<E: ParentElement + Styled>(&self, key: K, el: E) -> E {
        self.inner.items.measure(key, el)
    }

    /// The element that follows the pointer while a row is dragged. Render
    /// it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T, K> {
        self.inner.drags.listener()
    }

    /// The copy of the dragged row to render, while a pointer drags one with
    /// `preview`.
    pub fn preview(&self) -> Option<TreeReorderPreview<K>> {
        let pointer = self.inner.pointer.borrow();
        let drag = pointer.as_ref()?;
        let (preview, count) = drag.preview?;
        Some(TreeReorderPreview {
            key: drag.key.clone(),
            count,
            preview,
        })
    }

    /// The moving item and the rows it shows, while one is picked up.
    pub fn draft(&self, this: &T) -> Option<TreeReorderDraft<K>> {
        draft_move::current(self, this)
    }

    /// The rows to render: the draft's while an item moves, else the value.
    pub fn rows(&self, this: &T) -> Vec<TreeReorderRow<K>> {
        match self.draft(this) {
            Some(draft) => draft.rows,
            None => self.value(this),
        }
    }

    /// Whether a row is picked up, by itself or under the item that is.
    pub fn moving(&self, this: &T, key: &K) -> bool {
        self.draft(this).is_some_and(|draft| draft.keys.contains(key))
    }

    /// Whether a pointer is dragging the row, by itself or under the item it
    /// drags.
    pub fn dragging(&self, this: &T, key: &K) -> bool {
        self.draft(this)
            .is_some_and(|draft| draft.pointer && draft.keys.contains(key))
    }

    fn say_in(
        &self,
        this: &T,
        r#type: MessageType,
        key: &K,
        rows: &[TreeReorderRow<K>],
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(index) = rows.iter().position(|row| row.key == *key) else {
            return;
        };
        let (parent, position) = place_of(rows, index);
        let depth = rows[index].depth;
        let start = parent.map_or(0, |parent| parent + 1);
        let total = rows[start..]
            .iter()
            .take_while(|row| row.depth >= depth)
            .filter(|row| row.depth == depth)
            .count();
        let options = self.options();
        let name = |key: &K| -> SharedString {
            options
                .label
                .as_ref()
                .and_then(|label| label(this, key))
                .or_else(|| {
                    self.inner
                        .items
                        .get(key)
                        .and_then(|item| item.text)
                        .map(|text| label_of(&text, None).into())
                })
                .unwrap_or_else(|| key.to_string().into())
        };
        let message = TreeReorderMessage {
            r#type,
            key: key.clone(),
            label: name(key),
            parent: parent.map(|parent| name(&rows[parent].key)),
            level: depth + 1,
            position: position + 1,
            total,
        };
        let text = match &options.message {
            Some(message_fn) => message_fn(&message),
            None => tree_reorder_message(&message),
        };
        announce_with(&text, AnnouncePriority::Assertive, window, cx);
    }

    fn commit_rows(
        &self,
        this: &mut T,
        rows: Vec<TreeReorderRow<K>>,
        key: &K,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let index = rows.iter().position(|row| row.key == *key).unwrap_or(0);
        let (parent, position) = if rows.is_empty() {
            (None, 0)
        } else {
            place_of(&rows, index)
        };
        let place = TreePlace {
            key: key.clone(),
            parent: parent.map(|parent| rows[parent].key.clone()),
            index: position,
        };
        let set = self.inner.options.borrow().set.clone();
        set(this, rows, place, options, window, cx)
    }

    fn grab_with(&self, this: &T, key: &K, pointer: bool, window: &mut Window, cx: &mut App) -> bool {
        let rows = self.value(this);
        let Some(index) = rows.iter().position(|row| row.key == *key) else {
            return false;
        };
        let disabled = self.inner.options.borrow().disabled.clone();
        if self.draft(this).is_some() || disabled.is_some_and(|disabled| disabled(this, key)) {
            return false;
        }
        self.inner.aim.set(rows[index].depth);
        let keys: Vec<K> = rows[index..subtree_end(&rows, index)]
            .iter()
            .map(|row| row.key.clone())
            .collect();
        draft_move::set_draft(
            self,
            this,
            TreeReorderDraft {
                key: key.clone(),
                keys,
                rows: rows.clone(),
                pointer,
            },
            window,
            cx,
        );
        self.say_in(this, MessageType::Grab, key, &rows, window, cx);
        true
    }

    /// Picks a row up, as Space does. Returns whether it did.
    pub fn grab(&self, this: &mut T, key: &K, window: &mut Window, cx: &mut Context<T>) -> bool {
        let grabbed = self.grab_with(this, key, false, window, cx);
        if grabbed {
            cx.notify();
        }
        grabbed
    }

    fn slots(&self, current: &TreeReorderDraft<K>) -> Slots<K> {
        let shown = |row: &TreeReorderRow<K>| self.inner.items.contains(&row.key);
        let others: Vec<TreeReorderRow<K>> = current
            .rows
            .iter()
            .filter(|row| !current.keys.contains(&row.key) && shown(row))
            .cloned()
            .collect();
        let at = current
            .rows
            .iter()
            .position(|row| row.key == current.key)
            .unwrap_or(0);
        let slot = current.rows[..at]
            .iter()
            .filter(|row| !current.keys.contains(&row.key) && shown(row))
            .count();
        Slots {
            others,
            slot,
            depth: current.rows.get(at).map_or(0, |row| row.depth),
        }
    }

    /// Moves the picked-up row to `slot` among the other rows on screen, as
    /// near `depth` as fits there. Returns whether it moved.
    pub fn place(
        &self,
        this: &mut T,
        slot: isize,
        depth: isize,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        self.place_in(this, slot, depth, window, cx)
    }

    fn place_in(
        &self,
        this: &T,
        slot: isize,
        depth: isize,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let Some(current) = self.draft(this) else {
            return false;
        };
        let Slots { others, .. } = self.slots(&current);
        let to = slot.clamp(0, others.len() as isize) as usize;
        let keys: HashSet<K> = current.keys.iter().cloned().collect();
        let deepest = block_max_depth(&current.rows, &keys, self.max_depth(this));
        let (min, max) = depth_range(
            to.checked_sub(1).and_then(|above| others.get(above)),
            others.get(to),
            deepest,
        );
        let fitted = clamp_depth(depth, min, max);
        let rows = moved_rows(
            &current.rows,
            &current.key,
            others.get(to).map(|row| &row.key),
            fitted,
        );
        if same_rows(&rows, &current.rows) {
            return false;
        }
        let key = current.key.clone();
        let pointer = current.pointer;
        draft_move::set_draft(
            self,
            this,
            TreeReorderDraft {
                rows: rows.clone(),
                ..current
            },
            window,
            cx,
        );
        self.say_in(this, MessageType::Move, &key, &rows, window, cx);
        if !pointer {
            draft_move::focus_later(self, &key, window, cx);
        }
        cx.notify();
        true
    }

    /// Drops the picked-up row where it is, committing the draft, with the
    /// event behind it (`drop`). Reports `MoveKey`.
    pub fn drop(
        &self,
        this: &mut T,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        draft_move::drop(self, this, Reason::MoveKey, event, window, cx);
    }

    /// Puts the picked-up row back where it was.
    pub fn cancel(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        draft_move::cancel(self, this, true, window, cx);
    }

    /// Moves an item and its children before the row `before`, or to the end
    /// with `None`, at `depth` as near as fits there, like an "Indent" menu
    /// item. Reports `Imperative`. Returns whether it went through.
    pub fn r#move(
        &self,
        this: &mut T,
        key: &K,
        before: Option<&K>,
        depth: usize,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let rows = self.value(this);
        let next = placed_rows(&rows, key, before, depth as isize, self.max_depth(this));
        if same_rows(&next, &rows)
            || !self.commit_rows(
                this,
                next.clone(),
                key,
                ChangeOptions::reason(Reason::Imperative),
                window,
                cx,
            )
        {
            return false;
        }
        self.say_in(this, MessageType::Drop, key, &next, window, cx);
        cx.notify();
        true
    }

    /// Handles a key pressed on a row and returns whether it did. Space or
    /// Enter picks the row up; while it's up, Up and Down move it between
    /// the rows on screen, Left and Right out a level and in one, Space or
    /// Enter drops it, Escape puts it back, and Tab puts it back and moves
    /// on. Every other key is swallowed while it's up.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let current = self.draft(this);
        if window.default_prevented() || current.as_ref().is_some_and(|draft| draft.pointer) {
            return current.is_some();
        }
        let modifiers = event.keystroke.modifiers;
        let modified = modifiers.alt || modifiers.control || modifiers.platform;
        let pressed = Key::of(&event.keystroke);
        let Some(current) = current else {
            if modified || (pressed != Key::Space && pressed != Key::Enter) {
                return false;
            }
            if !self.grab_with(this, key, false, window, cx) {
                return false;
            }
            window.prevent_default();
            cx.notify();
            return true;
        };
        if pressed == Key::Tab {
            draft_move::cancel(self, this, false, window, cx);
            return true;
        }
        let Slots {
            others,
            slot,
            depth,
        } = self.slots(&current);
        let rtl = is_rtl(cx);
        let inward = pressed == if rtl { Key::ArrowLeft } else { Key::ArrowRight };
        let outward = pressed == if rtl { Key::ArrowRight } else { Key::ArrowLeft };
        let aim = self.inner.aim.get() as isize;
        if pressed == Key::Space || pressed == Key::Enter {
            window.prevent_default();
            draft_move::drop(
                self,
                this,
                Reason::MoveKey,
                Some(InputEvent::from(event)),
                window,
                cx,
            );
        } else if pressed == Key::Escape {
            window.prevent_default();
            cx.stop_propagation();
            draft_move::cancel(self, this, true, window, cx);
        } else if modified {
            // Swallowed, like every other key while a row is up.
        } else if pressed == Key::ArrowUp || pressed == Key::ArrowDown {
            window.prevent_default();
            let step = if pressed == Key::ArrowUp { -1 } else { 1 };
            self.place_in(this, slot as isize + step, aim, window, cx);
        } else if pressed == Key::Home || pressed == Key::End {
            window.prevent_default();
            let to = if pressed == Key::Home { 0 } else { others.len() as isize };
            self.place_in(this, to, aim, window, cx);
        } else if inward || outward {
            window.prevent_default();
            let to = depth as isize + if inward { 1 } else { -1 };
            if self.place_in(this, slot as isize, to, window, cx)
                && let Some(moved) = self.draft(this)
            {
                self.inner.aim.set(self.slots(&moved).depth);
            }
        }
        true
    }

    // Places the dragged block where the pointer is: between the rows on
    // screen by height, and a level in for every `indent` sideways.
    fn track(&self, this: &T, window: &mut Window, cx: &mut Context<T>) {
        let Some((start, at, start_depth)) = self
            .inner
            .pointer
            .borrow()
            .as_ref()
            .map(|drag| (drag.start, drag.at, drag.start_depth))
        else {
            return;
        };
        let Some(current) = self.draft(this) else {
            return;
        };
        let Slots { others, .. } = self.slots(&current);
        let middles: Vec<Pixels> = others
            .iter()
            .map(|row| {
                self.inner
                    .items
                    .bounds(&row.key)
                    .map_or(Pixels::ZERO, |rect: Bounds<Pixels>| rect.top() + rect.size.height / 2.)
            })
            .collect();
        let sideways = f32::from(at.x - start.x) * if is_rtl(cx) { -1. } else { 1. };
        let indent = self.inner.options.borrow().indent.clone();
        let indent = f32::from(indent.map_or(px(24.), |indent| indent(this)));
        let levels = if indent > 0. { (sideways / indent).round() as isize } else { 0 };
        self.place_in(
            this,
            drop_index(&middles, at.y, false) as isize,
            start_depth as isize + levels,
            window,
            cx,
        );
    }

    /// Starts a pointer drag of a row once the pointer moves a few pixels,
    /// so a click stays a click (`pointerdown`). Escape puts it back.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        key: &K,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drags = self.inner.drags.clone();
        if !self.inner.items.contains(key)
            || !drags.accepts(this, event, window, cx)
            || self.draft(this).is_some()
        {
            return;
        }
        let options = self.options();
        let scroller = if options.preview {
            options.auto_scroll.map(|auto_scroll| {
                let tree = Rc::downgrade(&self.inner);
                let entity = cx.weak_entity();
                create_auto_scroll(auto_scroll.on_scroll(move |window, cx| {
                    let Some(tree) = Self::upgrade(&tree) else {
                        return;
                    };
                    entity.update(cx, |this, cx| tree.track(this, window, cx)).ok();
                }))
            })
        } else {
            None
        };
        *self.inner.pointer.borrow_mut() = Some(PointerDrag {
            key: key.clone(),
            start: event.position,
            at: event.position,
            start_depth: 0,
            dragging: false,
            last_move: None,
            preview: None,
            scroller,
        });
        let weak = Rc::downgrade(&self.inner);
        let started = weak.clone();
        let moved = weak.clone();
        let ended = weak;
        drags.start(
            event,
            BindDragOptions::new()
                .threshold(DRAG_THRESHOLD)
                .cancel_on_escape(true)
                .on_start(move |this: &mut T, _event, window, cx| {
                    Self::upgrade(&started).is_some_and(|tree| tree.drag_started(this, window, cx))
                })
                .on_move(move |this: &mut T, sample, window, cx| {
                    if let Some(tree) = Self::upgrade(&moved) {
                        tree.drag_moved(this, sample, window, cx);
                    }
                })
                .on_end(move |this: &mut T, end, canceled, window, cx| {
                    if let Some(tree) = Self::upgrade(&ended) {
                        tree.drag_ended(this, end, canceled, window, cx);
                    }
                }),
            window,
            cx,
        );
    }

    fn upgrade(inner: &Weak<Inner<T, K>>) -> Option<Self> {
        inner.upgrade().map(|inner| Self { inner })
    }

    fn drag_started(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) -> bool {
        let Some((key, start)) = self
            .inner
            .pointer
            .borrow()
            .as_ref()
            .map(|drag| (drag.key.clone(), drag.start))
        else {
            return false;
        };
        if !self.grab_with(this, &key, true, window, cx) {
            *self.inner.pointer.borrow_mut() = None;
            return false;
        }
        let start_depth = self
            .value(this)
            .iter()
            .find(|row| row.key == key)
            .map_or(0, |row| row.depth);
        let preview = self.options().preview.then(|| {
            let bounds = self
                .inner
                .items
                .bounds(&key)
                .unwrap_or(Bounds::new(start, Default::default()));
            let count = self.draft(this).map_or(1, |draft| draft.keys.len());
            (bind_drag_preview(bounds, start), count)
        });
        if let Some(drag) = self.inner.pointer.borrow_mut().as_mut() {
            drag.start_depth = start_depth;
            drag.dragging = true;
            drag.preview = preview;
        }
        cx.notify();
        true
    }

    fn drag_moved(
        &self,
        this: &mut T,
        sample: &PointerSample,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let scroller = {
            let mut pointer = self.inner.pointer.borrow_mut();
            let Some(drag) = pointer.as_mut() else {
                return;
            };
            drag.last_move = Some(sample.event.clone());
            drag.at = sample.position();
            if let Some((preview, _)) = &mut drag.preview {
                preview.r#move(sample.x, sample.y);
            }
            drag.scroller.clone()
        };
        if let Some(scroller) = scroller {
            scroller.update(sample.x, sample.y, window);
        }
        self.track(this, window, cx);
        cx.notify();
    }

    fn drag_ended(
        &self,
        this: &mut T,
        end: &PointerEvent,
        canceled: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let Some(drag) = self.inner.pointer.borrow_mut().take() else {
            return;
        };
        if let Some(scroller) = &drag.scroller {
            scroller.stop();
        }
        if !drag.dragging {
            return;
        }
        if canceled {
            draft_move::cancel(self, this, true, window, cx);
        } else {
            let event = drag.last_move.unwrap_or_else(|| end.clone()).event;
            draft_move::drop(self, this, Reason::Drag, Some(event), window, cx);
        }
        cx.notify();
    }
}
