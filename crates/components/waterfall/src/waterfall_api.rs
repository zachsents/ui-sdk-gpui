//! Port of `components/waterfall/waterfall-api.ts`. Rows move like Tree's,
//! on `RovingFocus` and a tree collection. Each row's cells register as it
//! renders, so a row holds any columns: Right enters them, Left leaves
//! them, Up and Down keep the column, and only the focused one is a tab
//! stop.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, Bounds, Context, FocusHandle, KeyDownEvent, Pixels, ScrollHandle, SharedString, Window};
use ui_sdk_focus::{RovingFocus, RovingFocusOptions, RovingMember};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    ChangeOptions, Controllable, InputEvent, Instance, Key, Parts, Reason, dev, name_of,
};
use ui_sdk_lists::{ListHighlightDetails, TreeCollection, TreePosition, create_tree_collection, tree_data_nodes};
use ui_sdk_number::{NumberFormat, clamp};
use ui_sdk_platform::is_rtl;
use ui_sdk_text::{NameFrom, NameSources, Typeahead, check_name, is_typeahead_key, locale_of};

use crate::waterfall::{
    WaterfallBarLabelDetails, WaterfallBarLabelFn, WaterfallDataItem, WaterfallHighlightCallback,
    WaterfallOptions, WaterfallRowState, WaterfallState, WaterfallTimeState,
};
use crate::waterfall_time::{
    Range, Span, WaterfallTickData, format_time, percent_at, span_placement, span_range,
    waterfall_ticks,
};

fn default_bar_label(_item: &WaterfallDataItem, details: &WaterfallBarLabelDetails) -> String {
    format!("Starts at {}, lasts {}", details.start, details.duration)
}

// The shape, kept until `items` changes.
struct Known {
    tree: TreeCollection,
    range: Range,
}

fn known(items: &[WaterfallDataItem]) -> Known {
    Known {
        tree: create_tree_collection(&tree_data_nodes(items)),
        range: span_range(items),
    }
}

/// A waterfall. Its entity is its controller: "Expand all" and "Collapse
/// all" buttons, a link to a span, or a test call its methods.
pub struct Waterfall {
    pub(crate) instance: Instance,
    items: Rc<[WaterfallDataItem]>,
    range: Option<Range>,
    value: Controllable<Option<SharedString>>,
    expanded: Controllable<Vec<SharedString>>,
    // The row one of whose cells has focus, so the cell holds the tab stop
    // instead.
    cell: Option<SharedString>,
    format: Option<NumberFormat>,
    locale: Option<SharedString>,
    get_bar_label: Option<WaterfallBarLabelFn>,
    on_highlight: Option<WaterfallHighlightCallback>,
    scroller: Option<ScrollHandle>,
    pub(crate) roving: RovingFocus<SharedString>,
    typeahead: Typeahead,
    known: Known,
    // The rows in the page, in order.
    rows: Parts<SharedString, ()>,
    // Each row's cells in order, as it rendered last.
    cells: RefCell<HashMap<SharedString, Vec<FocusHandle>>>,
    handles: RefCell<HashMap<(SharedString, usize), FocusHandle>>,
    // The column headers rendered so far in the header row.
    pub(crate) headers: Cell<usize>,
    // The cell with the tab stop, by its row and column.
    stop_cell: Option<(SharedString, usize)>,
    // A cell to focus once its row renders, for a key that moved to a row
    // not rendered yet.
    pending_cell: Option<(SharedString, usize)>,
    // The expanded spans the last change saw.
    seen: Vec<SharedString>,
    // Whether the root's name was checked, once it rendered.
    name_checked: bool,
}

impl Waterfall {
    pub fn new(options: WaterfallOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut value = Controllable::new(
            "value",
            options.value.clone().unwrap_or_else(|| options.default_value.clone()),
        ).after_update();
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());
        let mut expanded = Controllable::new(
            "expanded",
            options.expanded.clone().unwrap_or_else(|| options.default_expanded.clone()),
        );
        expanded.set_controlled(options.expanded.clone());
        expanded.set_on_change(options.on_expanded_change.clone());
        let mut instance = Instance::new("waterfall").with_agent(cx);
        instance.debug = options.debug;
        let landed = cx.weak_entity();
        let roving = RovingFocus::new(
            RovingFocusOptions::default().on_items_change(move |window, cx| {
                landed
                    .update(cx, |waterfall, cx| waterfall.rows_changed(window, cx))
                    .ok();
            }),
            window,
            cx,
        );
        let seen = expanded.get().clone();
        Self {
            instance,
            known: known(&options.items),
            items: options.items,
            range: options.range,
            value,
            expanded,
            cell: None,
            format: options.format,
            locale: options.locale,
            get_bar_label: options.get_bar_label,
            on_highlight: options.on_highlight,
            scroller: options.scroller,
            roving,
            typeahead: Typeahead::default(),
            rows: Parts::new(),
            cells: RefCell::new(HashMap::new()),
            handles: RefCell::new(HashMap::new()),
            headers: Cell::new(0),
            stop_cell: None,
            pending_cell: None,
            seen,
            name_checked: false,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: WaterfallOptions, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = self.value.set_controlled(options.value);
        self.value.set_on_change(options.on_value_change);
        self.value.set_on_before_change(options.on_before_value_change);
        changed |= self.expanded.set_controlled(options.expanded);
        self.expanded.set_on_change(options.on_expanded_change);
        if !Rc::ptr_eq(&self.items, &options.items) {
            self.known = known(&options.items);
            self.items = options.items;
            changed = true;
        }
        changed |= self.range != options.range
            || self.format != options.format
            || self.locale != options.locale
            || !same_fn(self.get_bar_label.as_ref(), options.get_bar_label.as_ref());
        self.range = options.range;
        self.format = options.format;
        self.locale = options.locale;
        self.get_bar_label = options.get_bar_label;
        self.on_highlight = options.on_highlight;
        self.scroller = options.scroller;
        self.instance.debug = options.debug;
        self.expanded_changed(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, `Some(None)` for nothing selected, or
    /// `None` to let the waterfall keep its own.
    pub fn set_controlled_value(&mut self, value: Option<Option<SharedString>>, cx: &mut Context<Self>) {
        if self.value.set_controlled(value) {
            cx.notify();
        }
    }

    /// Passes the controlled expanded spans, or `None` to let the waterfall
    /// keep its own.
    pub fn set_controlled_expanded(
        &mut self,
        expanded: Option<Vec<SharedString>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.expanded.set_controlled(expanded) {
            cx.notify();
        }
        self.expanded_changed(window, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The spans, as `items` gave them.
    pub fn items(&self) -> &[WaterfallDataItem] {
        &self.items
    }

    /// What the root part's state holds.
    pub fn state(&self) -> WaterfallState {
        WaterfallState { range: self.range() }
    }

    /// The stretch of time the axis shows: `range`, else from the earliest
    /// start to the latest end.
    pub fn range(&self) -> Range {
        self.range.unwrap_or(self.known.range)
    }

    // The span with `key`, found by its place among its siblings and its
    // ancestors' (`byKey`).
    fn item(&self, key: &SharedString) -> Option<&WaterfallDataItem> {
        let tree = &self.known.tree;
        if !tree.has(key) {
            return None;
        }
        let mut list: &[WaterfallDataItem] = &self.items;
        let mut found = None;
        for step in tree.ancestors(key).iter().chain([key]) {
            let item = list.get(tree.position(step).index.checked_sub(1)?)?;
            list = item.children();
            found = Some(item);
        }
        found
    }

    // The rows that show, in order.
    fn visible(&self) -> Rc<[SharedString]> {
        self.known.tree.visible(self.expanded.get())
    }

    fn is_branch(&self, key: &SharedString) -> bool {
        self.known.tree.is_branch(key)
    }

    fn ancestors(&self, key: &SharedString) -> Vec<SharedString> {
        self.known.tree.ancestors(key)
    }

    fn parent_of(&self, key: &SharedString) -> Option<SharedString> {
        self.known.tree.parent(key)
    }

    // Whether every ancestor is expanded, so the row shows.
    pub(crate) fn shows(&self, key: &SharedString) -> bool {
        let expanded = self.expanded.get();
        self.ancestors(key).iter().all(|ancestor| expanded.contains(ancestor))
    }

    pub(crate) fn position(&self, key: &SharedString) -> Option<TreePosition> {
        self.known.tree.has(key).then(|| self.known.tree.position(key))
    }

    pub(crate) fn label_of(&self, key: &SharedString) -> Option<SharedString> {
        self.item(key).map(|item| item.label.clone())
    }

    // Every part of a row shares its state (`rowState`), with where its
    // span falls on the axis.
    pub(crate) fn row_state(&self, key: &SharedString) -> WaterfallRowState {
        let branch = self.is_branch(key);
        let placement = self
            .item(key)
            .map(|item| span_placement(item, self.range()))
            .unwrap_or_default();
        WaterfallRowState {
            expanded: branch && self.expanded.get().contains(key),
            selected: self.value.get().as_ref() == Some(key),
            branch,
            level: self.known.tree.level(key),
            start: placement.start,
            size: placement.size,
        }
    }

    fn locale(&self, cx: &App) -> SharedString {
        locale_of(self.locale.as_ref(), cx)
    }

    // The ticks the axis shows.
    pub(crate) fn ticks(&self, max: usize, cx: &App) -> Vec<WaterfallTickData> {
        waterfall_ticks(self.range(), max, &self.locale(cx), self.format.as_ref())
    }

    pub(crate) fn time_state(&self, time: f64, cx: &App) -> WaterfallTimeState {
        let now = self.range();
        let scale = now[0].abs().max(now[1].abs());
        WaterfallTimeState {
            time,
            label: format_time(time, &self.locale(cx), self.format.as_ref(), Some(scale), None),
            position: percent_at(time, now),
            outside: time < now[0] || time > now[1],
        }
    }

    pub(crate) fn bar_label(&self, key: &SharedString, cx: &App) -> Option<String> {
        let item = self.item(key)?;
        let locale = self.locale(cx);
        let details = WaterfallBarLabelDetails {
            start: format_time(item.start, &locale, self.format.as_ref(), None, None),
            duration: format_time(item.duration, &locale, self.format.as_ref(), None, None),
        };
        Some(match &self.get_bar_label {
            Some(label) => label(item, &details),
            None => default_bar_label(item, &details),
        })
    }

    // Starts a render of the rows, from the root.
    pub(crate) fn begin(&self, cx: &App) {
        self.rows.begin();
        self.roving.begin(cx);
    }

    // Joins a row to the roving focus as it renders, and starts its cells.
    // Returns whether it holds the tab stop: never while one of its cells
    // does. On the first frame, the selected row holds it when it shows,
    // else the first row.
    pub(crate) fn bind_row(&self, key: &SharedString, handle: &FocusHandle, cx: &App) -> bool {
        self.rows.register(key.clone(), ());
        self.cells.borrow_mut().insert(key.clone(), Vec::new());
        let shows = self.shows(key);
        let selected = self.value.get().as_ref();
        let claims = shows
            && match selected {
                None => true,
                Some(selected) => {
                    !self.known.tree.has(selected) || !self.shows(selected) || key == selected
                }
            };
        let member = RovingMember::new()
            .focusable(shows)
            .preferred(selected == Some(key))
            .claims(claims);
        let holds = self.roving.bind(key.clone(), handle, member, cx);
        holds && self.cell.as_ref() != Some(key)
    }

    // Joins a cell to its row as it renders: its focus handle, whether it
    // holds the tab stop, and its column, from 1.
    pub(crate) fn bind_cell(&self, key: &SharedString, cx: &App) -> (FocusHandle, bool, usize) {
        let mut cells = self.cells.borrow_mut();
        let row = cells.entry(key.clone()).or_default();
        let column = row.len() + 1;
        let handle = self
            .handles
            .borrow_mut()
            .entry((key.clone(), column))
            .or_insert_with(|| cx.focus_handle())
            .clone();
        row.push(handle.clone());
        let stop = self.stop_cell.as_ref() == Some(&(key.clone(), column));
        (handle, stop, column)
    }

    // A row's cells in order.
    fn cells_of(&self, key: &SharedString) -> Vec<FocusHandle> {
        self.cells.borrow().get(key).cloned().unwrap_or_default()
    }

    // Focus landing on a row or one of its cells moves the tab stop there
    // (`focusin`), read once each frame is drawn, since gpui tells where
    // focus is when it draws. Rows and cells that left take their stop with
    // them.
    pub(crate) fn rendered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.check_name(window);
        let rows = self.rows.keys();
        self.cells.borrow_mut().retain(|key, _| rows.contains(key));
        self.handles.borrow_mut().retain(|(key, _), _| rows.contains(key));
        let mut changed = false;
        if self.stop_cell.as_ref().is_some_and(|(key, _)| !rows.contains(key)) {
            self.stop_cell = None;
            changed = true;
        }
        if self.cell.as_ref().is_some_and(|key| !rows.contains(key)) {
            self.cell = None;
            changed = true;
        }
        if let Some(focused) = window.focused(cx) {
            let landed = self.cells.borrow().iter().find_map(|(key, cells)| {
                cells
                    .iter()
                    .position(|cell| *cell == focused)
                    .map(|index| (key.clone(), index + 1))
            });
            match landed {
                Some((key, column)) => {
                    if self.stop_cell.as_ref() != Some(&(key.clone(), column)) || self.cell.as_ref() != Some(&key) {
                        self.stop_cell = Some((key.clone(), column));
                        self.cell = Some(key.clone());
                        changed = true;
                    }
                    if self.roving.stop().as_ref() != Some(&key) {
                        self.roving.remember(&key, cx);
                    }
                }
                None => {
                    let on_row = rows
                        .iter()
                        .any(|key| self.roving.focus_handle(key, cx) == focused);
                    if on_row && (self.stop_cell.is_some() || self.cell.is_some()) {
                        self.stop_cell = None;
                        self.cell = None;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    // The root rendered: development builds check, once, that something
    // names it (`checkName`), from the name it rendered with. A labelled-by
    // names it, since its targets' text can't be read.
    fn check_name(&mut self, window: &Window) {
        if !dev::DEV || self.name_checked {
            return;
        }
        let Some(name) = name_of(&self.instance, "root", None) else {
            return;
        };
        self.name_checked = true;
        if !name.labelled_by.is_empty() {
            return;
        }
        let mut sources = NameSources::new(Role::TreeGrid);
        if let Some(label) = name.aria_label {
            sources = sources.aria_label(label);
        }
        let from = NameFrom {
            component: "waterfall",
            id: Some(self.instance.id().clone()),
        };
        check_name(&self.instance.element_id("root", None), &sources, "Waterfall", from, window);
    }

    // Rows joined or left: a cell waiting for its row to render takes focus
    // once the roving focus moved to the row.
    fn rows_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, column)) = self.pending_cell.clone() else {
            return;
        };
        if !self.rows.contains(&key) {
            return;
        }
        self.pending_cell = None;
        let row = self.roving.focus_handle(&key, cx);
        if window.focused(cx).is_none_or(|focused| focused == row) {
            self.focus_cell(&key, column, window, cx);
        }
    }

    fn highlight(&self, key: &SharedString, index: Option<usize>, event: Option<InputEvent>, window: &mut Window, cx: &mut App) {
        if let Some(on_highlight) = &self.on_highlight {
            on_highlight(key, &ListHighlightDetails { index, event }, window, cx);
        }
    }

    fn focus_cell(&self, key: &SharedString, column: usize, window: &mut Window, cx: &mut App) {
        let cells = self.cells_of(key);
        match column.min(cells.len()).checked_sub(1).and_then(|index| cells.get(index)) {
            Some(cell) => window.focus(cell, cx),
            None => window.focus(&self.roving.focus_handle(key, cx), cx),
        }
    }

    // Moves focus to a row, or to the cell in `column` of it, from 1, and
    // reports it.
    fn move_to(&mut self, key: &SharedString, column: usize, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.visible().iter().position(|other| other == key);
        self.highlight(key, index, event, window, cx);
        if column == 0 || !self.rows.contains(key) {
            self.pending_cell = (column != 0).then(|| (key.clone(), column));
            self.roving.focus(Some(key), window, cx);
            return;
        }
        self.focus_cell(key, column, window, cx);
    }

    fn change_value(&mut self, value: Option<SharedString>, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let changed = self.value.change(&self.instance, value, why, window, cx);
        if changed {
            cx.notify();
        }
        changed
    }

    fn change_expanded(
        &mut self,
        expanded: Vec<SharedString>,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut seen = std::collections::HashSet::new();
        let expanded: Vec<SharedString> = expanded.into_iter().filter(|key| seen.insert(key.clone())).collect();
        let changed = self.expanded.change(&self.instance, expanded, why, window, cx);
        self.expanded_changed(window, cx);
        if changed {
            cx.notify();
        }
        changed
    }

    fn set_open(
        &mut self,
        key: &SharedString,
        opening: bool,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let expanded = self.expanded.get();
        if !self.is_branch(key) || expanded.contains(key) == opening {
            return false;
        }
        let next = if opening {
            expanded.iter().cloned().chain([key.clone()]).collect()
        } else {
            expanded.iter().filter(|other| *other != key).cloned().collect()
        };
        self.change_expanded(next, why, window, cx)
    }

    // Focus in a row that a collapse hides goes to the outermost collapsed
    // ancestor.
    fn expanded_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let expanded = self.expanded.get().clone();
        if expanded == self.seen {
            return;
        }
        self.seen = expanded;
        let Some(active) = self.roving.stop() else {
            return;
        };
        if self.shows(&active) {
            return;
        }
        let Some(target) = self
            .ancestors(&active)
            .into_iter()
            .find(|key| !self.seen.contains(key))
        else {
            return;
        };
        let row = self
            .rows
            .contains(&active)
            .then(|| self.roving.focus_handle(&active, cx));
        let held = row.as_ref().is_none_or(|row| row.contains_focused(window, cx));
        self.stop_cell = None;
        self.cell = None;
        self.roving.remember(&target, cx);
        if held {
            cx.defer_in(window, move |waterfall, window, cx| {
                let lost = window.focused(cx).is_none_or(|now| {
                    !now.is_rendered(window)
                        || row
                            .as_ref()
                            .is_some_and(|row| *row == now || row.contains(&now, window))
                });
                if lost {
                    waterfall.roving.focus(Some(&target), window, cx);
                }
            });
        }
    }

    pub(crate) fn press(&mut self, key: &SharedString, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        // A press on a cell focuses the cell; anywhere else in the row
        // focuses the row.
        let row = self.roving.focus_handle(key, cx);
        if !row.contains_focused(window, cx) {
            window.focus(&row, cx);
        }
        if self.roving.stop().as_ref() != Some(key) {
            self.roving.remember(key, cx);
        }
        if self.value.get().as_ref() != Some(key) {
            let why = ChangeOptions {
                reason: Some(Reason::ItemPress),
                event,
                source: None,
            };
            self.change_value(Some(key.clone()), why, window, cx);
        }
    }

    // The bounds of a row as of its last layout.
    fn row_bounds(&self, key: &SharedString) -> Option<Bounds<Pixels>> {
        self.instance
            .bound_parts()
            .into_iter()
            .find(|part| part.slot == "row" && part.key.as_ref() == Some(key))
            .and_then(|part| part.bounds)
    }

    // How many rows Page Up and Page Down move: as many as the scroller
    // around the rows shows, less one.
    fn page_size(&self, key: &SharedString) -> usize {
        let height = self.row_bounds(key).map_or(0.0, |bounds| f32::from(bounds.size.height));
        let view = self
            .scroller
            .as_ref()
            .filter(|scroller| scroller.max_offset().y > Pixels::ZERO)
            .map_or(0.0, |scroller| f32::from(scroller.bounds().size.height));
        if height > 0.0 && view > height {
            (view / height).floor() as usize - 1
        } else {
            10
        }
    }

    pub(crate) fn keydown(&mut self, event: &KeyDownEvent, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if window.default_prevented() || modifiers.alt {
            return;
        }
        let Some(focused) = window.focused(cx) else {
            return;
        };
        let row = self.roving.focus_handle(key, cx);
        let cells = self.cells_of(key);
        // Keys inside a cell's own controls, like a link, stay theirs, and so
        // do a nested waterfall's.
        let column = if focused == row {
            0
        } else {
            match cells.iter().position(|cell| *cell == focused) {
                Some(index) => index + 1,
                None => return,
            }
        };
        let r#mod = modifiers.control || modifiers.platform;
        let pressed = Key::of(&event.keystroke);
        let input = InputEvent::from(event);
        let horizontal = matches!(pressed, Key::ArrowLeft | Key::ArrowRight);
        let rtl = horizontal && is_rtl(cx);
        let (forward, backward) = if rtl {
            (Key::ArrowLeft, Key::ArrowRight)
        } else {
            (Key::ArrowRight, Key::ArrowLeft)
        };
        let keys = self.visible();
        if keys.is_empty() {
            return;
        }
        let at = keys.iter().position(|other| other == key).unwrap_or(0) as f64;
        let last = (keys.len() - 1) as f64;
        let to_row = |waterfall: &mut Self, to: f64, window: &mut Window, cx: &mut Context<Self>| {
            let index = clamp(to, 0.0, last) as usize;
            waterfall.move_to(&keys[index], column, Some(input.clone()), window, cx);
        };
        if pressed == Key::ArrowDown && !r#mod {
            to_row(self, at + 1.0, window, cx);
        } else if pressed == Key::ArrowUp && !r#mod {
            to_row(self, at - 1.0, window, cx);
        } else if pressed == Key::PageDown {
            let page = self.page_size(key) as f64;
            to_row(self, at + page, window, cx);
        } else if pressed == Key::PageUp {
            let page = self.page_size(key) as f64;
            to_row(self, at - page, window, cx);
        } else if pressed == Key::Home {
            if column > 0 && !r#mod {
                window.focus(&cells[0], cx);
            } else {
                to_row(self, 0.0, window, cx);
            }
        } else if pressed == Key::End {
            if column > 0 && !r#mod {
                window.focus(&cells[cells.len() - 1], cx);
            } else {
                to_row(self, last, window, cx);
            }
        } else if pressed == forward && !r#mod {
            let expanded = self.expanded.get().contains(key);
            if column == 0 && self.is_branch(key) && !expanded {
                let why = ChangeOptions::event_of(Reason::ListNavigation, input);
                self.set_open(key, true, why, window, cx);
            } else if column < cells.len() {
                window.focus(&cells[column], cx);
            }
        } else if pressed == backward && !r#mod {
            if column > 1 {
                window.focus(&cells[column - 2], cx);
            } else if column == 1 {
                window.focus(&row, cx);
            } else if self.expanded.get().contains(key) {
                let why = ChangeOptions::event_of(Reason::ListNavigation, input);
                self.set_open(key, false, why, window, cx);
            } else {
                let Some(parent) = self.parent_of(key) else {
                    return;
                };
                self.move_to(&parent, 0, Some(input), window, cx);
            }
        } else if (pressed == Key::Enter || (pressed == Key::Space && !self.typeahead.active())) && !r#mod {
            let why = ChangeOptions::event_of(Reason::ItemPress, input);
            self.change_value(Some(key.clone()), why, window, cx);
        } else if is_typeahead_key(&event.keystroke) {
            let labels: Vec<SharedString> = keys
                .iter()
                .map(|other| self.label_of(other).unwrap_or_else(|| other.clone()))
                .collect();
            let current = keys.iter().position(|other| other == key).or(Some(0));
            let Some(found) = self.typeahead.search(&pressed, &labels, current) else {
                return;
            };
            self.move_to(&keys[found], 0, Some(input), window, cx);
        } else {
            return;
        }
        window.prevent_default();
    }

    pub(crate) fn press_indicator(
        &mut self,
        key: &SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Focus goes to the row, where Left and Right expand.
        window.focus(&self.roving.focus_handle(key, cx), cx);
        let opening = !self.expanded.get().contains(key);
        let why = ChangeOptions {
            reason: Some(Reason::TriggerPress),
            event,
            source: None,
        };
        self.set_open(key, opening, why, window, cx);
    }

    /// The selected span's value, or `None`.
    pub fn get_value(&self) -> Option<&SharedString> {
        self.value.get().as_ref()
    }

    /// Selects a span, or nothing with `None`. Returns false when canceled.
    pub fn set_value(
        &mut self,
        value: Option<impl Into<SharedString>>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.change_value(value.map(Into::into), options, window, cx)
    }

    /// The values of the expanded spans.
    pub fn get_expanded(&self) -> &Vec<SharedString> {
        self.expanded.get()
    }

    /// Expands exactly these spans. Returns false when canceled.
    pub fn set_expanded(
        &mut self,
        expanded: Vec<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.change_expanded(expanded, options, window, cx)
    }

    /// Whether a span is expanded.
    pub fn is_expanded(&self, value: &str) -> bool {
        self.expanded.get().iter().any(|key| key.as_ref() == value)
    }

    /// Expands a span.
    pub fn expand(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.set_open(&value.into(), true, options, window, cx)
    }

    /// Collapses a span. Focus inside it moves to its row.
    pub fn collapse(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.set_open(&value.into(), false, options, window, cx)
    }

    /// Expands a collapsed span, or collapses an expanded one.
    pub fn toggle(
        &mut self,
        value: impl Into<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let value = value.into();
        let opening = !self.expanded.get().contains(&value);
        self.set_open(&value, opening, options, window, cx)
    }

    /// Expands every span with children.
    pub fn expand_all(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let branches: Vec<SharedString> = self
            .known
            .tree
            .keys()
            .into_iter()
            .filter(|key| self.is_branch(key))
            .collect();
        self.change_expanded(branches, options, window, cx)
    }

    /// Collapses every span.
    pub fn collapse_all(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.change_expanded(Vec::new(), options, window, cx)
    }

    /// Expands a span's ancestors and focuses its row, through
    /// `on_highlight` when it isn't rendered. A controlled waterfall shows the
    /// row once its `expanded` comes back, a render later.
    pub fn reveal(&mut self, value: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let value = value.into();
        if !self.known.tree.has(&value) {
            return;
        }
        let expanded = self.expanded.get().clone();
        let hidden: Vec<SharedString> = self
            .ancestors(&value)
            .into_iter()
            .filter(|key| !expanded.contains(key))
            .collect();
        if !hidden.is_empty() {
            let next = expanded.into_iter().chain(hidden).collect();
            if !self.change_expanded(next, ChangeOptions::reason(Reason::Imperative), window, cx) {
                return;
            }
        }
        cx.defer_in(window, move |waterfall, window, cx| waterfall.move_to(&value, 0, None, window, cx));
    }

    /// Focuses a span's row, or with `None` the row with the tab stop.
    pub fn focus(&self, value: Option<&str>, window: &mut Window, cx: &mut App) {
        let key = value.map(|value| SharedString::from(value.to_string()));
        self.roving.focus(key.as_ref(), window, cx);
    }
}

fn same_fn<T: ?Sized>(a: Option<&Rc<T>>, b: Option<&Rc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
