//! Port of `foundation/selection/cell-range.ts`: selecting a rectangle of
//! cells, as a spreadsheet does: Shift with the arrows grows it from the
//! active cell, a drag paints it, Shift with a click stretches it, and
//! Control or Shift with Space takes whole columns or rows. It works by
//! index alongside [`GridFocus`], which keeps the active cell, so a range
//! reaches rows that aren't rendered, and it reads and writes a range as
//! tab-separated text, the format spreadsheets copy and paste.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    App, Context, FocusHandle, IntoElement, KeyDownEvent, ParentElement, Pixels, Point, Styled,
    Window,
};
use ui_sdk_drag_and_drop::{
    AutoScroll, AutoScrollOptions, BindDragOptions, DragSession, PointerEvent, create_auto_scroll,
};
use ui_sdk_focus::{GridCell, GridFocus, GridFocusDetails};
use ui_sdk_kernel::{ChangeOptions, Key, Reason};
use ui_sdk_platform::{announce, is_rtl};

/// A range from the cell it started at to the cell it reaches
/// (`CellRange`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRange {
    pub anchor: GridCell,
    pub end: GridCell,
}

impl CellRange {
    pub fn new(anchor: GridCell, end: GridCell) -> Self {
        Self { anchor, end }
    }
}

/// A range's rows and columns, from 0 and inclusive (`CellBounds`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellBounds {
    pub top: usize,
    pub left: usize,
    pub bottom: usize,
    pub right: usize,
}

/// Why the range changed (`CellRangeReason`): a key (`ListNavigation`), a
/// press on a cell (`ItemPress`), a drag across cells (`Drag`), or a call
/// from code (`Imperative`).
pub type CellRangeReason = Reason;

/// The English message [`create_cell_range`] announces unless you pass
/// `message` (`cellRangeMessage`).
pub fn cell_range_message(bounds: &CellBounds) -> String {
    let rows = bounds.bottom - bounds.top + 1;
    let cols = bounds.right - bounds.left + 1;
    if rows * cols == 1 {
        return "1 cell selected".to_string();
    }
    format!(
        "{rows} {} by {cols} {} selected",
        if rows == 1 { "row" } else { "rows" },
        if cols == 1 { "column" } else { "columns" }
    )
}

fn bounds_of(range: &CellRange) -> CellBounds {
    CellBounds {
        top: range.anchor.row.min(range.end.row),
        left: range.anchor.col.min(range.end.col),
        bottom: range.anchor.row.max(range.end.row),
        right: range.anchor.col.max(range.end.col),
    }
}

/// Text with tabs between cells and newlines between rows, quoted where a
/// cell holds either (`cellsToText`).
pub fn cells_to_text<S: AsRef<str>>(rows: &[Vec<S>]) -> String {
    let quote = |value: &str| {
        if value.contains(['\t', '\n', '\r', '"']) {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else {
            value.to_string()
        }
    };
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|cell| quote(cell.as_ref()))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads tab-separated text, as spreadsheets copy it, into rows of cells
/// (`textToCells`).
pub fn text_to_cells(text: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = vec![Vec::new()];
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        if quoted {
            if char == '"' && chars.peek() == Some(&'"') {
                cell.push('"');
                chars.next();
            } else if char == '"' {
                quoted = false;
            } else {
                cell.push(char);
            }
        } else if char == '"' && cell.is_empty() {
            quoted = true;
        } else if char == '\t' {
            rows.last_mut().expect("never empty").push(std::mem::take(&mut cell));
        } else if char == '\n' || char == '\r' {
            if char == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            rows.last_mut().expect("never empty").push(std::mem::take(&mut cell));
            rows.push(Vec::new());
        } else {
            cell.push(char);
        }
    }
    rows.last_mut().expect("never empty").push(cell);
    // A copied range ends with a newline, which isn't another row.
    if rows.len() > 1 && rows.last().is_some_and(|last| last.len() == 1 && last[0].is_empty()) {
        rows.pop();
    }
    rows
}

/// Which sides of the range a cell sits on, for drawing its outline
/// (`data-range-edge`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RangeEdges {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl RangeEdges {
    /// The sides' names, like `"top left"`, or `None` for a cell inside.
    pub fn names(&self) -> Option<String> {
        let names: Vec<&str> = [
            (self.top, "top"),
            (self.right, "right"),
            (self.bottom, "bottom"),
            (self.left, "left"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        (!names.is_empty()).then(|| names.join(" "))
    }
}

/// A cell's place in the range (`cellAttrs`): whether the explicit range
/// holds it, its `aria-selected` and `data-selected`, and the range's sides
/// it sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellRangeCell {
    pub selected: bool,
    pub edges: RangeEdges,
}

/// Reads the component's range.
pub type RangeFn<T> = Rc<dyn Fn(&T) -> Option<CellRange>>;

/// Asks the component for a new range and returns whether it went through.
pub type SetRangeFn<T> =
    Rc<dyn Fn(&mut T, Option<CellRange>, ChangeOptions, &mut Window, &mut Context<T>) -> bool>;

/// Reads a count from the component.
pub type CountFn<T> = Rc<dyn Fn(&T) -> usize>;

/// What `reveal` runs.
pub type RevealFn = Rc<dyn Fn(&GridCell, &mut Window, &mut App)>;

/// What screen readers hear after the range changes from the keyboard.
pub type RangeMessageFn = Rc<dyn Fn(&CellBounds) -> String>;

/// How [`create_cell_range`] reads the grid (`CellRangeOptions`).
pub struct CellRangeOptions<T: 'static> {
    /// The range, or `None` when only the active cell is selected, when the
    /// component keeps it (`key`). Without it, the cell range keeps it.
    pub value: Option<RangeFn<T>>,
    /// The grid's focus, which keeps the active cell.
    pub grid: GridFocus,
    /// How many rows the range can reach.
    pub rows: CountFn<T>,
    /// How many columns.
    pub cols: CountFn<T>,
    /// Shows a cell the range grows to that may not be rendered, like
    /// `GridFocus`'s `reveal`.
    pub reveal: Option<RevealFn>,
    /// Asks for a new range and returns whether it went through. Defaults to
    /// keeping it; pass the component's change to report it. `details.reason`
    /// is `ListNavigation`, `ItemPress`, `Drag` or `Imperative`.
    pub set: Option<SetRangeFn<T>>,
    /// What screen readers hear after the range changes from the keyboard.
    /// Defaults to English ([`cell_range_message`]).
    pub message: Option<RangeMessageFn>,
    /// The scrollers a drag across cells scrolls near their edges. The
    /// original finds them under the pointer.
    pub auto_scroll: AutoScrollOptions,
}

impl<T: 'static> Clone for CellRangeOptions<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            grid: self.grid.clone(),
            rows: self.rows.clone(),
            cols: self.cols.clone(),
            reveal: self.reveal.clone(),
            set: self.set.clone(),
            message: self.message.clone(),
            auto_scroll: self.auto_scroll.clone(),
        }
    }
}

impl<T: 'static> CellRangeOptions<T> {
    /// Options with the grid and its size.
    pub fn new(
        grid: &GridFocus,
        rows: impl Fn(&T) -> usize + 'static,
        cols: impl Fn(&T) -> usize + 'static,
    ) -> Self {
        Self {
            value: None,
            grid: grid.clone(),
            rows: Rc::new(rows),
            cols: Rc::new(cols),
            reveal: None,
            set: None,
            message: None,
            auto_scroll: AutoScrollOptions::default(),
        }
    }

    /// Lets the component keep the range: `value` reads it, `set` asks for
    /// a new one.
    pub fn controlled(
        mut self,
        value: impl Fn(&T) -> Option<CellRange> + 'static,
        set: impl Fn(&mut T, Option<CellRange>, ChangeOptions, &mut Window, &mut Context<T>) -> bool
        + 'static,
    ) -> Self {
        self.value = Some(Rc::new(value));
        self.set = Some(Rc::new(set));
        self
    }

    pub fn set(
        mut self,
        set: impl Fn(&mut T, Option<CellRange>, ChangeOptions, &mut Window, &mut Context<T>) -> bool
        + 'static,
    ) -> Self {
        self.set = Some(Rc::new(set));
        self
    }

    pub fn reveal(mut self, reveal: impl Fn(&GridCell, &mut Window, &mut App) + 'static) -> Self {
        self.reveal = Some(Rc::new(reveal));
        self
    }

    pub fn message(mut self, message: impl Fn(&CellBounds) -> String + 'static) -> Self {
        self.message = Some(Rc::new(message));
        self
    }

    pub fn auto_scroll(mut self, auto_scroll: AutoScrollOptions) -> Self {
        self.auto_scroll = auto_scroll;
        self
    }
}

// A drag painting the range from the cell it pressed.
struct Painting {
    from: GridCell,
    press: PointerEvent,
    last: Point<Pixels>,
    scroll: AutoScroll,
}

struct Inner<T: 'static> {
    options: RefCell<CellRangeOptions<T>>,
    // The range when the component doesn't keep it.
    own: Cell<Option<CellRange>>,
    drags: DragSession<T>,
    painting: RefCell<Option<Painting>>,
}

/// Selects a rectangle of cells (`createCellRange`'s return, the
/// original's `CellRangeSelection`). Cloning it gives another handle to the
/// same range.
pub struct CellRangeSelection<T: 'static> {
    inner: Rc<Inner<T>>,
}

impl<T: 'static> Clone for CellRangeSelection<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lets users select a rectangle of cells (`createCellRange`). Make it in
/// the component's constructor after its [`GridFocus`], keep it in a field,
/// render [`listener`](CellRangeSelection::listener) on every frame, and
/// wire the cell part through it instead of the grid: [`bind`](CellRangeSelection::bind)
/// and [`measure`](CellRangeSelection::measure) each cell, style it from
/// [`cell`](CellRangeSelection::cell), hand its keys to
/// [`keydown`](CellRangeSelection::keydown), which passes ordinary
/// navigation on to the grid, and its presses to
/// [`pointer_down`](CellRangeSelection::pointer_down) through
/// `on_pointer_down`. The keys it takes: the arrows, Home, End, Page Up and
/// Page Down, with and without Shift and Control (⌘ on a Mac), Shift and
/// Control with Space, Control with A, and Escape.
pub fn create_cell_range<T: 'static>(
    options: CellRangeOptions<T>,
    cx: &Context<T>,
) -> CellRangeSelection<T> {
    CellRangeSelection {
        inner: Rc::new(Inner {
            options: RefCell::new(options),
            own: Cell::new(None),
            drags: DragSession::new(cx),
            painting: RefCell::new(None),
        }),
    }
}

impl<T: 'static> CellRangeSelection<T> {
    /// Hands it new options, like callbacks that read new props.
    pub fn set_options(&self, options: CellRangeOptions<T>) {
        *self.inner.options.borrow_mut() = options;
    }

    fn options(&self) -> CellRangeOptions<T> {
        self.inner.options.borrow().clone()
    }

    /// The element that follows the pointer while a drag paints the range.
    /// Render it on every frame, anywhere in the component.
    pub fn listener(&self) -> impl IntoElement + use<T> {
        self.inner.drags.listener()
    }

    /// The explicit range, or `None` when only the active cell is selected.
    pub fn range(&self, this: &T) -> Option<CellRange> {
        let value = self.inner.options.borrow().value.clone();
        match value {
            Some(value) => value(this),
            None => self.inner.own.get(),
        }
    }

    fn write(
        &self,
        this: &mut T,
        range: Option<CellRange>,
        details: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let options = self.options();
        match options.set {
            Some(set) => {
                let done = set(this, range, details, window, cx);
                if done && options.value.is_none() {
                    self.inner.own.set(range);
                    cx.notify();
                }
                done
            }
            None => {
                self.inner.own.set(range);
                cx.notify();
                true
            }
        }
    }

    fn clamp(&self, this: &T, cell: GridCell) -> GridCell {
        let options = self.options();
        let rows = (options.rows)(this);
        let cols = (options.cols)(this);
        GridCell {
            row: cell.row.min(rows.saturating_sub(1)),
            col: cell.col.min(cols.saturating_sub(1)),
        }
    }

    fn last(&self, this: &T) -> GridCell {
        let options = self.options();
        GridCell {
            row: (options.rows)(this).saturating_sub(1),
            col: (options.cols)(this).saturating_sub(1),
        }
    }

    fn every_cell(&self, this: &T) -> CellRange {
        CellRange {
            anchor: GridCell::new(0, 0),
            end: self.last(this),
        }
    }

    fn bounds_with(&self, range: Option<&CellRange>) -> Option<CellBounds> {
        if let Some(range) = range {
            return Some(bounds_of(range));
        }
        let active = self.inner.options.borrow().grid.active()?;
        Some(CellBounds {
            top: active.row,
            left: active.col,
            bottom: active.row,
            right: active.col,
        })
    }

    fn say(&self, range: Option<&CellRange>, window: &mut Window, cx: &mut App) {
        let Some(bounds) = self.bounds_with(range) else {
            return;
        };
        let message = self.inner.options.borrow().message.clone();
        let text = match message {
            Some(message) => message(&bounds),
            None => cell_range_message(&bounds),
        };
        announce(&text, window, cx);
    }

    /// The selected rows and columns: the range, or the active cell alone,
    /// or `None` before the grid is entered.
    pub fn bounds(&self, this: &T) -> Option<CellBounds> {
        self.bounds_with(self.range(this).as_ref())
    }

    /// Whether a cell is in the range.
    pub fn contains(&self, this: &T, cell: &GridCell) -> bool {
        self.bounds(this).is_some_and(|bounds| {
            cell.row >= bounds.top
                && cell.row <= bounds.bottom
                && cell.col >= bounds.left
                && cell.col <= bounds.right
        })
    }

    /// A cell's `aria-selected`, `data-selected`, and `data-range-edge`, the
    /// range's sides it sits on, for drawing its outline (`cellAttrs`).
    pub fn cell(&self, this: &T, cell: &GridCell) -> CellRangeCell {
        let Some(range) = self.range(this) else {
            return CellRangeCell::default();
        };
        if !self.contains(this, cell) {
            return CellRangeCell::default();
        }
        let bounds = bounds_of(&range);
        CellRangeCell {
            selected: true,
            edges: RangeEdges {
                top: cell.row == bounds.top,
                right: cell.col == bounds.right,
                bottom: cell.row == bounds.bottom,
                left: cell.col == bounds.left,
            },
        }
    }

    /// Selects a range, or `None` for the active cell alone, clamped to the
    /// grid. Returns whether it changed.
    pub fn select(
        &self,
        this: &mut T,
        range: Option<CellRange>,
        details: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let next = range.map(|range| CellRange {
            anchor: self.clamp(this, range.anchor),
            end: self.clamp(this, range.end),
        });
        if self.range(this) == next {
            return false;
        }
        self.write(this, next, details, window, cx)
    }

    /// Selects every cell, keeping the active one.
    pub fn select_all(
        &self,
        this: &mut T,
        details: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let every = self.every_cell(this);
        self.select(this, Some(every), details, window, cx)
    }

    /// Selects whole rows, from one to another.
    pub fn select_rows(
        &self,
        this: &mut T,
        from: usize,
        to: usize,
        details: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let last = self.last(this);
        let range = CellRange::new(GridCell::new(from, 0), GridCell::new(to, last.col));
        self.select(this, Some(range), details, window, cx)
    }

    /// Selects whole columns, from one to another.
    pub fn select_columns(
        &self,
        this: &mut T,
        from: usize,
        to: usize,
        details: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let last = self.last(this);
        let range = CellRange::new(GridCell::new(0, from), GridCell::new(last.row, to));
        self.select(this, Some(range), details, window, cx)
    }

    /// Joins a rendered cell to the grid's focus, so focus can reach it.
    /// Returns whether it holds the tab stop.
    pub fn bind(&self, cell: GridCell, handle: &FocusHandle, cx: &App) -> bool {
        let grid = self.inner.options.borrow().grid.clone();
        grid.bind(cell, handle, cx)
    }

    /// Adds a layer to a cell's element that records its bounds each
    /// layout, which a dragging pointer finds it by.
    pub fn measure<E: ParentElement + Styled>(&self, cell: GridCell, el: E) -> E {
        let grid = self.inner.options.borrow().grid.clone();
        grid.measure(cell, el)
    }

    /// Handles a key pressed in the grid and returns whether it did
    /// anything. Shift with an arrow grows the range a cell, with Control to
    /// the grid's edge, and Shift with Home or End to the row's ends;
    /// Shift+Space takes the rows, Control+Space the columns, Control+A
    /// every cell, and Escape goes back to the active cell. Anything else
    /// moves the active cell through the grid, which drops the range.
    pub fn keydown(
        &self,
        this: &mut T,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented() || modifiers.alt {
            return false;
        }
        let options = self.options();
        let grid = options.grid.clone();
        let active = grid.active().unwrap_or_default();
        let range = self.range(this);
        let ctrl = modifiers.control || modifiers.platform;
        let shift = modifiers.shift;
        let key = Key::of(&event.keystroke);
        let done = |next: Option<CellRange>, this: &mut T, window: &mut Window, cx: &mut Context<T>| {
            window.prevent_default();
            let details = ChangeOptions::event(Reason::ListNavigation, event);
            if self.select(this, next, details, window, cx) {
                self.say(next.as_ref(), window, cx);
            }
            true
        };
        let is_a = matches!(&key, Key::Character(typed) if typed.to_lowercase() == "a");
        if ctrl && !shift && is_a {
            let every = self.every_cell(this);
            return done(Some(every), this, window, cx);
        }
        let last = self.last(this);
        if key == Key::Space && shift && !ctrl {
            let (top, bottom) = range
                .map(|range| {
                    let bounds = bounds_of(&range);
                    (bounds.top, bounds.bottom)
                })
                .unwrap_or((active.row, active.row));
            let next = CellRange::new(GridCell::new(top, 0), GridCell::new(bottom, last.col));
            return done(Some(next), this, window, cx);
        }
        if key == Key::Space && ctrl && !shift {
            let (left, right) = range
                .map(|range| {
                    let bounds = bounds_of(&range);
                    (bounds.left, bounds.right)
                })
                .unwrap_or((active.col, active.col));
            let next = CellRange::new(GridCell::new(0, left), GridCell::new(last.row, right));
            return done(Some(next), this, window, cx);
        }
        if key == Key::Escape && range.is_some() {
            return done(None, this, window, cx);
        }
        if shift {
            let rtl = is_rtl(cx);
            let end = range.map_or(active, |range| range.end);
            let far = |to: isize, edge: usize| -> usize {
                if ctrl { edge } else { to.max(0) as usize }
            };
            let (row, col) = (end.row as isize, end.col as isize);
            let to = match key {
                Key::ArrowUp => Some(GridCell::new(far(row - 1, 0), end.col)),
                Key::ArrowDown => Some(GridCell::new(far(row + 1, last.row), end.col)),
                Key::ArrowLeft => Some(GridCell::new(
                    end.row,
                    far(col + if rtl { 1 } else { -1 }, if rtl { last.col } else { 0 }),
                )),
                Key::ArrowRight => Some(GridCell::new(
                    end.row,
                    far(col + if rtl { -1 } else { 1 }, if rtl { 0 } else { last.col }),
                )),
                Key::Home => Some(if ctrl {
                    GridCell::new(0, 0)
                } else {
                    GridCell::new(end.row, 0)
                }),
                Key::End => Some(if ctrl {
                    last
                } else {
                    GridCell::new(end.row, last.col)
                }),
                _ => None,
            };
            if let Some(to) = to {
                let next = CellRange {
                    anchor: range.map_or(active, |range| range.anchor),
                    end: self.clamp(this, to),
                };
                if let Some(reveal) = &options.reveal {
                    reveal(&next.end, window, cx);
                }
                let next = (next.anchor != next.end).then_some(next);
                return done(next, this, window, cx);
            }
        }
        if !grid.keydown(event, window, cx) {
            return false;
        }
        self.select(
            this,
            None,
            ChangeOptions::event(Reason::ListNavigation, event),
            window,
            cx,
        );
        true
    }

    /// Starts a range on a press: the cell becomes active, and dragging
    /// paints the range to the cell under the pointer, scrolling near the
    /// edges of the scrollers `auto_scroll` names. Shift stretches the range
    /// from the active cell instead, and keeps focus where it is.
    pub fn pointer_down(
        &self,
        this: &mut T,
        event: &PointerEvent,
        cell: GridCell,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let drags = self.inner.drags.clone();
        if !drags.accepts(this, event, window, cx) {
            return;
        }
        let options = self.options();
        let grid = options.grid.clone();
        let press = ChangeOptions::event_of(Reason::ItemPress, event.event.clone());
        if event.modifiers.shift
            && let Some(active) = grid.active()
        {
            window.prevent_default();
            let anchor = self.range(this).map_or(active, |range| range.anchor);
            self.select(this, Some(CellRange::new(anchor, cell)), press, window, cx);
            return;
        }
        grid.focus(
            cell,
            GridFocusDetails::reason(Reason::ItemPress).event(event.event.clone()),
            window,
            cx,
        );
        self.select(this, None, press, window, cx);
        let painter = Rc::downgrade(&self.inner);
        let entity = cx.weak_entity();
        let scroll = create_auto_scroll(options.auto_scroll.clone().on_scroll(move |window, cx| {
            let Some(inner) = painter.upgrade() else {
                return;
            };
            entity
                .update(cx, |this, cx| CellRangeSelection { inner }.paint(this, window, cx))
                .ok();
        }));
        *self.inner.painting.borrow_mut() = Some(Painting {
            from: cell,
            press: event.clone(),
            last: event.position,
            scroll,
        });
        let moved = Rc::downgrade(&self.inner);
        let ended = moved.clone();
        drags.start(
            event,
            BindDragOptions::new()
                .on_move(move |this: &mut T, sample, window, cx| {
                    let Some(inner) = moved.upgrade() else {
                        return;
                    };
                    let scroll = {
                        let mut painting = inner.painting.borrow_mut();
                        let Some(painting) = painting.as_mut() else {
                            return;
                        };
                        painting.last = sample.position();
                        painting.scroll.clone()
                    };
                    scroll.update(sample.x, sample.y, window);
                    CellRangeSelection { inner }.paint(this, window, cx);
                })
                .on_end(move |_this: &mut T, _end, _canceled, _window, _cx| {
                    if let Some(inner) = ended.upgrade()
                        && let Some(painting) = inner.painting.borrow_mut().take()
                    {
                        painting.scroll.stop();
                    }
                }),
            window,
            cx,
        );
    }

    // Paints the range from the pressed cell to the cell under the pointer.
    fn paint(&self, this: &mut T, window: &mut Window, cx: &mut Context<T>) {
        let Some((from, press, last)) = self
            .inner
            .painting
            .borrow()
            .as_ref()
            .map(|painting| (painting.from, painting.press.clone(), painting.last))
        else {
            return;
        };
        let grid = self.inner.options.borrow().grid.clone();
        let Some(hit) = grid.cell_at(last) else {
            return;
        };
        let next = (hit != from).then_some(CellRange::new(from, hit));
        let details = ChangeOptions::event_of(Reason::Drag, press.event.clone());
        self.select(this, next, details, window, cx);
    }

    /// The range's text, read cell by cell, with tabs between cells and
    /// newlines between rows (`text`).
    pub fn text(&self, this: &T, read: impl Fn(GridCell) -> String) -> String {
        let Some(bounds) = self.bounds(this) else {
            return String::new();
        };
        let rows: Vec<Vec<String>> = (bounds.top..=bounds.bottom)
            .map(|row| {
                (bounds.left..=bounds.right)
                    .map(|col| read(GridCell::new(row, col)))
                    .collect()
            })
            .collect();
        cells_to_text(&rows)
    }
}
