//! Port of `foundation/focus/grid-focus.ts`: moving focus through a grid of
//! cells by row and column, as WAI-ARIA's grid pattern asks. Arrows move a
//! cell, Home and End to the row's ends, Control with them to the grid's
//! corners, Page Up and Page Down a page of rows. It works by index, not by
//! element, so a grid that renders only the rows in view moves to rows that
//! aren't rendered yet: `reveal` scrolls them in and focus lands once their
//! cell renders.
//!
//! The original keeps the active cell in the primitive's store and binds
//! each cell's element. Here the grid keeps the active cell, and each cell
//! binds as it renders with the focus handle its element tracks. Once
//! the frame that rendered them is drawn, a cell a move was waiting for
//! takes focus.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gpui::{
    AnyWindowHandle, App, Bounds, Context, EntityId, FocusHandle, KeyDownEvent, ParentElement,
    Pixels, Point, Styled, Window, canvas,
};
use ui_sdk_kernel::{InputEvent, Key, Reason};
use ui_sdk_platform::is_rtl;

use crate::after_bind::after_bind;

/// A cell, by its row and column index from 0. Cells order row by row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GridCell {
    pub row: usize,
    pub col: usize,
}

impl GridCell {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// Why the active cell moved: a key (`ListNavigation`), a press on a cell
/// (`ItemPress`), or a call from code (`Imperative`), the reasons a listbox
/// reports.
pub type GridFocusReason = Reason;

/// How a move to a cell reports itself to [`GridFocus::focus`].
#[derive(Clone, Debug)]
pub struct GridFocusDetails {
    pub reason: GridFocusReason,
    pub event: Option<InputEvent>,
    /// Whether Shift was held, so a grid can extend a range from it.
    pub shift: bool,
}

impl Default for GridFocusDetails {
    fn default() -> Self {
        Self {
            reason: Reason::Imperative,
            event: None,
            shift: false,
        }
    }
}

impl GridFocusDetails {
    pub fn reason(reason: GridFocusReason) -> Self {
        Self {
            reason,
            ..Self::default()
        }
    }

    pub fn event(mut self, event: impl Into<InputEvent>) -> Self {
        self.event = Some(event.into());
        self
    }

    pub fn shift(mut self, shift: bool) -> Self {
        self.shift = shift;
        self
    }
}

/// What `on_move` hears besides the cell focus moved to.
#[derive(Clone, Debug)]
pub struct GridMoveDetails {
    /// The cell it left, or `None` before the grid was entered.
    pub from: Option<GridCell>,
    pub reason: GridFocusReason,
    pub event: Option<InputEvent>,
    /// Whether Shift was held.
    pub shift: bool,
}

/// Which cells can't take focus.
pub type GridDisabledFn = Rc<dyn Fn(&GridCell) -> bool>;

/// What `reveal` runs.
pub type GridRevealFn = Rc<dyn Fn(&GridCell, &mut Window, &mut App)>;

/// What `on_move` runs.
pub type GridMoveFn = Rc<dyn Fn(&GridCell, &GridMoveDetails, &mut Window, &mut App)>;

/// How a [`GridFocus`] reads the grid.
#[derive(Clone)]
pub struct GridFocusOptions {
    /// How many rows the grid has, header rows included when they hold
    /// cells.
    pub rows: usize,
    /// How many columns.
    pub cols: usize,
    /// How many rows Page Up and Page Down move, usually how many show.
    /// Defaults to 10.
    pub page_size: usize,
    /// Cells that can't take focus, skipped by the arrows and by the tab
    /// stop before the grid is entered.
    pub disabled: Option<GridDisabledFn>,
    /// Shows a cell that may not be rendered, like scrolling a list's
    /// `ScrollHandle` to its row, before focus moves to it.
    pub reveal: Option<GridRevealFn>,
    /// Runs after the active cell moves, with the cell it left. It runs
    /// inside the update that moved it, so it may update any entity but the
    /// one that update is for.
    pub on_move: Option<GridMoveFn>,
}

impl Default for GridFocusOptions {
    fn default() -> Self {
        Self {
            rows: 0,
            cols: 0,
            page_size: 10,
            disabled: None,
            reveal: None,
            on_move: None,
        }
    }
}

impl GridFocusOptions {
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows;
        self
    }

    pub fn cols(mut self, cols: usize) -> Self {
        self.cols = cols;
        self
    }

    pub fn page_size(mut self, page_size: usize) -> Self {
        self.page_size = page_size;
        self
    }

    pub fn disabled(mut self, disabled: impl Fn(&GridCell) -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(disabled));
        self
    }

    pub fn reveal(mut self, reveal: impl Fn(&GridCell, &mut Window, &mut App) + 'static) -> Self {
        self.reveal = Some(Rc::new(reveal));
        self
    }

    pub fn on_move(
        mut self,
        on_move: impl Fn(&GridCell, &GridMoveDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_move = Some(Rc::new(on_move));
        self
    }
}

/// Gives a grid one tab stop and moves it by row and column. Make it once
/// in the component's constructor, keep it in a field, and wire the cell
/// slot: [`bind`](Self::bind) each cell as it renders, with role
/// `GridCell`, and track its handle with the tab stop that returns; hand its
/// key presses to [`keydown`](Self::keydown), and a press on it to
/// [`focus`](Self::focus) with `Reason::ItemPress`. Render the cells inside
/// rows with role `Row`, inside a root with role `Grid`, whose part calls
/// [`begin`](Self::begin). Shift with a move is reported in `on_move`, so a
/// grid can extend a range selection from it. Cloning it gives another
/// handle to the same grid.
pub struct GridFocus {
    inner: Rc<RefCell<Inner>>,
}

impl Clone for GridFocus {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

struct Inner {
    owner: EntityId,
    window: AnyWindowHandle,
    options: GridFocusOptions,
    active: Option<GridCell>,
    // The cell to focus once it renders, when it wasn't rendered as focus
    // moved to it.
    pending: Option<GridCell>,
    open: bool,
    // Each cell that rendered, with the handle its element tracks. The
    // newest join wins.
    cells: HashMap<GridCell, FocusHandle>,
    handles: HashMap<GridCell, FocusHandle>,
    bounds: HashMap<GridCell, Bounds<Pixels>>,
    // Kept until the next render or options, so rendering every cell scans
    // the grid once.
    first: Option<Option<GridCell>>,
}

impl Inner {
    fn allowed(&self, cell: &GridCell) -> bool {
        cell.row < self.options.rows
            && cell.col < self.options.cols
            && !self.options.disabled.as_ref().is_some_and(|disabled| disabled(cell))
    }

    fn first_allowed(&mut self) -> Option<GridCell> {
        if let Some(first) = self.first {
            return first;
        }
        let mut found = None;
        'rows: for row in 0..self.options.rows {
            for col in 0..self.options.cols {
                let cell = GridCell { row, col };
                if self.allowed(&cell) {
                    found = Some(cell);
                    break 'rows;
                }
            }
        }
        self.first = Some(found);
        found
    }

    // The nearest allowed cell from `(row, col)` going by `dr`, `dc`, or
    // the start itself. The start may sit just outside the grid, like the
    // row before the first, which no cell can take.
    fn step(&self, row: isize, col: isize, dr: isize, dc: isize) -> (isize, isize) {
        let rows = self.options.rows as isize;
        let cols = self.options.cols as isize;
        let (mut next_row, mut next_col) = (row + dr, col + dc);
        while next_row >= 0 && next_col >= 0 && next_row < rows && next_col < cols {
            let cell = GridCell {
                row: next_row as usize,
                col: next_col as usize,
            };
            if self.allowed(&cell) {
                return (next_row, next_col);
            }
            next_row += dr;
            next_col += dc;
        }
        (row, col)
    }

    fn element_at(&self, cell: &GridCell) -> Option<FocusHandle> {
        self.cells.get(cell).cloned()
    }

    // Starts a render of the cells unless one is under way, and says
    // whether it did.
    fn open_pass(&mut self) -> bool {
        if self.open {
            return false;
        }
        self.open = true;
        self.cells.clear();
        self.first = None;
        true
    }
}

impl GridFocus {
    /// A grid focus for a component; `cx` is the component's, which the grid
    /// notifies when the active cell moves so its cells render it, and
    /// `window` the one it renders in.
    pub fn new<T: 'static>(options: GridFocusOptions, window: &Window, cx: &Context<T>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                owner: cx.entity_id(),
                window: window.window_handle(),
                options,
                active: None,
                pending: None,
                open: false,
                cells: HashMap::new(),
                handles: HashMap::new(),
                bounds: HashMap::new(),
                first: None,
            })),
        }
    }

    /// Hands it new options, like a new row count.
    pub fn set_options(&self, options: GridFocusOptions) {
        let mut inner = self.inner.borrow_mut();
        inner.options = options;
        inner.first = None;
    }

    pub fn set_rows(&self, rows: usize) {
        let mut inner = self.inner.borrow_mut();
        inner.options.rows = rows;
        inner.first = None;
    }

    pub fn set_cols(&self, cols: usize) {
        let mut inner = self.inner.borrow_mut();
        inner.options.cols = cols;
        inner.first = None;
    }

    pub fn set_page_size(&self, page_size: usize) {
        self.inner.borrow_mut().options.page_size = page_size;
    }

    /// The active cell, which has the tab stop, or `None` before the grid is
    /// entered.
    pub fn active(&self) -> Option<GridCell> {
        self.inner.borrow().active
    }

    /// Makes a cell active without moving focus or telling `on_move`, or
    /// with `None` starts the grid over, as the original's state key set by
    /// its component.
    pub fn set_active(&self, cell: Option<GridCell>, cx: &mut App) {
        let owner = {
            let mut inner = self.inner.borrow_mut();
            if inner.active == cell {
                return;
            }
            inner.active = cell;
            inner.owner
        };
        cx.notify(owner);
    }

    /// Whether the cell holds the tab stop, for its handle's `tab_stop`: the
    /// active cell, or before the grid is entered, the first cell that isn't
    /// disabled, row by row (`tabIndex`).
    pub fn tab_index(&self, cell: &GridCell) -> bool {
        let mut inner = self.inner.borrow_mut();
        let stop = match inner.active {
            Some(active) => Some(active),
            None => inner.first_allowed(),
        };
        stop.as_ref() == Some(cell)
    }

    /// Whether a cell is the active one, to keep it marked when focus
    /// leaves the grid.
    pub fn is_active(&self, cell: &GridCell) -> bool {
        self.inner.borrow().active.as_ref() == Some(cell)
    }

    /// Where a joined cell's element is now, by the focus handle it tracks.
    pub fn place_of(&self, handle: &FocusHandle) -> Option<GridCell> {
        self.inner
            .borrow()
            .cells
            .iter()
            .find(|(_, joined)| *joined == handle)
            .map(|(cell, _)| *cell)
    }

    /// The cell under a point, like a dragging pointer, from the bounds
    /// [`measure`](Self::measure) records.
    pub fn cell_at(&self, point: Point<Pixels>) -> Option<GridCell> {
        self.inner
            .borrow()
            .bounds
            .iter()
            .find(|(_, bounds)| bounds.contains(&point))
            .map(|(cell, _)| *cell)
    }

    /// The focus handle for a cell, made the first time it's asked for and
    /// kept while the cell renders.
    pub fn focus_handle(&self, cell: GridCell, cx: &App) -> FocusHandle {
        self.inner
            .borrow_mut()
            .handles
            .entry(cell)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// Starts a render of the cells, from the part that holds them, every
    /// frame it renders. Binding a cell starts one too.
    pub fn begin(&self, cx: &App) {
        self.open(cx);
    }

    // Opens a render of the cells, and asks to settle it once its frame is
    // drawn.
    fn open(&self, cx: &App) {
        let window = {
            let mut inner = self.inner.borrow_mut();
            if !inner.open_pass() {
                return;
            }
            inner.window
        };
        let this = self.clone();
        after_bind(window, cx, move |window, cx| this.settle(window, cx));
    }

    // Once the cells have rendered: a cell a move was waiting for takes
    // focus, and cells that left let go of their handles.
    fn settle(&self, window: &mut Window, cx: &mut App) {
        let target = {
            let Ok(mut inner) = self.inner.try_borrow_mut() else {
                return;
            };
            if !inner.open {
                return;
            }
            inner.open = false;
            let pending = inner.pending;
            let Inner {
                cells,
                handles,
                bounds,
                ..
            } = &mut *inner;
            handles.retain(|cell, _| cells.contains_key(cell) || pending.as_ref() == Some(cell));
            bounds.retain(|cell, _| cells.contains_key(cell));
            let target = pending
                .and_then(|cell| inner.element_at(&cell))
                .filter(|handle| handle.is_rendered(window));
            if target.is_some() {
                inner.pending = None;
            }
            target
        };
        if let Some(handle) = target
            && !handle.is_focused(window)
        {
            window.focus(&handle, cx);
        }
    }

    /// Joins a rendered cell, so focus can reach it. Returns whether it
    /// holds the tab stop. A cell that stops rendering leaves.
    pub fn bind(&self, cell: GridCell, handle: &FocusHandle, cx: &App) -> bool {
        self.open(cx);
        self.inner.borrow_mut().cells.insert(cell, handle.clone());
        self.tab_index(&cell)
    }

    /// Adds a layer to a cell's element that records its bounds each
    /// layout, for [`cell_at`](Self::cell_at).
    pub fn measure<E: ParentElement + Styled>(&self, cell: GridCell, el: E) -> E {
        let inner: Weak<RefCell<Inner>> = Rc::downgrade(&self.inner);
        el.relative().child(
            canvas(
                move |bounds, _window, _cx| {
                    if let Some(inner) = inner.upgrade()
                        && let Ok(mut inner) = inner.try_borrow_mut()
                    {
                        inner.bounds.insert(cell, bounds);
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    fn focus_element(&self, cell: GridCell, window: &mut Window, cx: &mut App) {
        let handle = {
            let mut inner = self.inner.borrow_mut();
            let handle = inner.element_at(&cell).filter(|handle| handle.is_rendered(window));
            inner.pending = if handle.is_some() { None } else { Some(cell) };
            handle
        };
        if let Some(handle) = handle
            && !handle.is_focused(window)
        {
            window.focus(&handle, cx);
        }
    }

    /// Makes a cell active and focuses it, scrolling it in through `reveal`
    /// first. Returns false for a cell that can't take focus.
    pub fn focus(
        &self,
        cell: GridCell,
        details: GridFocusDetails,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let (from, owner, reveal, on_move) = {
            let mut inner = self.inner.borrow_mut();
            if !inner.allowed(&cell) {
                return false;
            }
            let from = inner.active;
            inner.active = Some(cell);
            (
                from,
                inner.owner,
                inner.options.reveal.clone(),
                inner.options.on_move.clone(),
            )
        };
        let moved = from != Some(cell);
        if moved {
            cx.notify(owner);
        }
        if let Some(reveal) = reveal {
            reveal(&cell, window, cx);
        }
        self.focus_element(cell, window, cx);
        if moved && let Some(on_move) = on_move {
            on_move(
                &cell,
                &GridMoveDetails {
                    from,
                    reason: details.reason,
                    event: details.event,
                    shift: details.shift,
                },
                window,
                cx,
            );
        }
        true
    }

    /// Handles a key pressed in the grid and returns whether it moved.
    /// Arrows move a cell, flipped left to right in right-to-left layouts;
    /// Home and End go to the row's ends, or with Control (Command on a Mac)
    /// the grid's; Page Up and Page Down move `page_size` rows. A key it
    /// handles calls `window.prevent_default()`.
    pub fn keydown(&self, event: &KeyDownEvent, window: &mut Window, cx: &mut App) -> bool {
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented() || modifiers.alt {
            return false;
        }
        let to = {
            let mut inner = self.inner.borrow_mut();
            let from = match inner.active {
                Some(active) => active,
                None => match inner.first_allowed() {
                    Some(first) => first,
                    None => return false,
                },
            };
            let rtl = is_rtl(cx);
            let page = inner.options.page_size as isize;
            let last_row = inner.options.rows as isize - 1;
            let last_col = inner.options.cols as isize - 1;
            let (row, col) = (from.row as isize, from.col as isize);
            let ctrl = modifiers.control || modifiers.platform;
            let inline = if rtl { -1 } else { 1 };
            match Key::of(&event.keystroke) {
                Key::ArrowUp if !ctrl => Some(inner.step(row, col, -1, 0)),
                Key::ArrowDown if !ctrl => Some(inner.step(row, col, 1, 0)),
                Key::ArrowLeft if !ctrl => Some(inner.step(row, col, 0, -inline)),
                Key::ArrowRight if !ctrl => Some(inner.step(row, col, 0, inline)),
                Key::Home if ctrl => Some(inner.step(-1, 0, 1, 0)),
                Key::Home => Some(inner.step(row, -1, 0, 1)),
                Key::End if ctrl => Some(inner.step(last_row + 1, last_col, -1, 0)),
                Key::End => Some(inner.step(row, last_col + 1, 0, -1)),
                Key::PageUp => Some(inner.step((row - page).max(0) + 1, col, -1, 0)),
                Key::PageDown => Some(inner.step((row + page).min(last_row) - 1, col, 1, 0)),
                _ => None,
            }
        };
        let Some((row, col)) = to else {
            return false;
        };
        window.prevent_default();
        if row < 0 || col < 0 {
            return false;
        }
        self.focus(
            GridCell::new(row as usize, col as usize),
            GridFocusDetails {
                reason: Reason::ListNavigation,
                event: Some(InputEvent::from(event)),
                shift: modifiers.shift,
            },
            window,
            cx,
        )
    }
}
