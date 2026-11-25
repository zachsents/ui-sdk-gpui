//! Port of `components/waterfall/waterfall.ts`: a tree of spans laid out on
//! a shared time axis, as a WAI-ARIA treegrid: a browser's network
//! requests, an OpenTelemetry trace, the steps of a build. Its options, the
//! state its parts expose, and the parts: `root`, `header-row`,
//! `column-header`, `axis`, `tick`, `mark`, `row`, `row-header`,
//! `indicator`, `cell` and `bar`. `waterfall_api.rs` holds the state and
//! the api.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, Entity, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Window,
};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentItem, AgentState, AgentValue, Aria, ChangeCallback, ChangeDetails,
    ChangeOptions, InputEvent, Need, Part, Reason,
};
use ui_sdk_number::NumberFormat;

use ui_sdk_lists::ListHighlightDetails;
pub use crate::waterfall_api::Waterfall;
use crate::waterfall_time::{Range, Span, WaterfallTickData};

/// One span of a trace: a request, a function call or any step with a start
/// and a duration, and the spans it started.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaterfallDataItem {
    /// Identifies the span. Unique in the waterfall.
    pub value: SharedString,
    /// What its row shows, and what typeahead matches.
    pub label: SharedString,
    /// When it starts, in milliseconds, on the same clock as every other
    /// span.
    pub start: f64,
    /// How long it lasts, in milliseconds.
    pub duration: f64,
    /// The spans it started. `None` for a span with none.
    pub children: Option<Vec<WaterfallDataItem>>,
}

impl WaterfallDataItem {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>, start: f64, duration: f64) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            start,
            duration,
            children: None,
        }
    }

    pub fn children(mut self, children: impl IntoIterator<Item = WaterfallDataItem>) -> Self {
        self.children = Some(children.into_iter().collect());
        self
    }
}

impl Span for WaterfallDataItem {
    fn start(&self) -> f64 {
        self.start
    }

    fn duration(&self) -> f64 {
        self.duration
    }

    fn children(&self) -> &[Self] {
        self.children.as_deref().unwrap_or_default()
    }
}

// Spans are tree data, as the original passes them where `TreeItemData`
// goes.
impl ui_sdk_lists::TreeData for WaterfallDataItem {
    fn value(&self) -> &SharedString {
        &self.value
    }

    fn children(&self) -> Option<&[Self]> {
        self.children.as_deref()
    }
}

/// The spans of a trace, as `items` takes them.
pub type WaterfallData = Vec<WaterfallDataItem>;

/// One row that shows when only the expanded spans are open, for a
/// waterfall that renders only the rows in view: the span it shows, the one
/// you passed in, the value of the span it's nested in, or `None` at the top
/// level, its depth, 1 at the top level, and its place among the rows that
/// show, from 0.
pub type WaterfallDataRow<'a> = ui_sdk_lists::TreeRow<'a, WaterfallDataItem>;

/// The rows that show when only the spans in `expanded` are open, in order,
/// for a waterfall that renders only the rows in view. Render each as a
/// [`Waterfall::row`] with its span's `value`.
pub fn visible_waterfall_rows<'a>(items: &'a [WaterfallDataItem], expanded: &[SharedString]) -> Vec<WaterfallDataRow<'a>> {
    ui_sdk_lists::visible_tree_rows(items, expanded)
}

/// Why spans expanded or collapsed: an indicator was clicked
/// (`TriggerPress`), an arrow key opened or closed it (`ListNavigation`),
/// or a controller or api call changed it (`Imperative`).
pub type WaterfallExpandedChangeReason = Reason;

/// Why the selected span changed: a row was clicked, or Enter or Space was
/// pressed on it (`ItemPress`), or a controller or api call changed it
/// (`Imperative`).
pub type WaterfallValueChangeReason = Reason;

/// What `get_bar_label` receives besides the span: its start and duration as
/// `format` and the locale show them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaterfallBarLabelDetails {
    /// When the span starts, formatted, like "120 ms".
    pub start: String,
    /// How long it lasts, formatted, like "340 ms".
    pub duration: String,
}

/// Options for every write method.
pub type WaterfallChangeOptions = ChangeOptions;

/// `get_bar_label`: what screen readers hear for a span's bar.
pub type WaterfallBarLabelFn = Rc<dyn Fn(&WaterfallDataItem, &WaterfallBarLabelDetails) -> String>;

/// `on_highlight`: the row the keyboard moved focus to, and its place among
/// the rows that show. It runs once the waterfall's update is over, so it
/// may update any entity, the waterfall too.
pub type WaterfallHighlightCallback = Rc<dyn Fn(&SharedString, &ListHighlightDetails, &mut Window, &mut App)>;

/// The options `Waterfall` takes.
#[derive(Clone, Default)]
pub struct WaterfallOptions {
    /// The spans, each with a `value`, a `label`, a `start`, a `duration`
    /// and the spans it started as `children`.
    pub items: Rc<[WaterfallDataItem]>,
    /// The stretch of time the axis shows, as `[start, end]` in
    /// milliseconds. Bars outside it shrink to its edges. Change it to zoom
    /// or pan. From the earliest start to the latest end by default.
    pub range: Option<Range>,
    /// The selected span's value. Pass it to control the selection; `None`
    /// inside selects nothing.
    pub value: Option<Option<SharedString>>,
    /// The span selected at first, when uncontrolled.
    pub default_value: Option<SharedString>,
    /// Runs when the selection wants to change. `details.reason` is
    /// `ItemPress` or `Imperative`. It runs once the waterfall's update is
    /// over, so it may update any entity, the waterfall too.
    pub on_value_change: Option<ChangeCallback<Option<SharedString>>>,
    /// Runs as the selection is asked to change, before it does, inside the
    /// waterfall's update: `details.cancel()` stops it. It may update any
    /// entity but the waterfall.
    pub on_before_value_change: Option<ChangeCallback<Option<SharedString>>>,
    /// The expanded spans' values. Pass it to control which spans show
    /// their children.
    pub expanded: Option<Vec<SharedString>>,
    /// The spans expanded at first, when uncontrolled.
    pub default_expanded: Vec<SharedString>,
    /// Runs when spans expand or collapse. `details.reason` is
    /// `TriggerPress`, `ListNavigation` or `Imperative`. It runs once the
    /// waterfall's update is over, so it may update any entity, the
    /// waterfall too.
    pub on_expanded_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Runs as spans are asked to expand or collapse, before they do, inside
    /// the waterfall's update: `details.cancel()` stops it. It may update any
    /// entity but the waterfall.
    pub on_before_expanded_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Runs when the keyboard moves focus to a row, with its place among the
    /// rows that show, from 0. A waterfall that renders only the rows in view
    /// scrolls that row into view here, so it can take focus once it renders.
    pub on_highlight: Option<WaterfallHighlightCallback>,
    /// How times read on the axis and to screen readers: number format
    /// options for a number of milliseconds, or a function of it.
    /// Milliseconds, seconds or minutes, whichever reads best, by default.
    pub format: Option<NumberFormat>,
    /// The locale times are formatted in. The app's by default
    /// (`ui_sdk_text::page_locale`).
    pub locale: Option<SharedString>,
    /// What screen readers hear for a span's bar. Set it in the app's
    /// language. "Starts at 120 ms, lasts 340 ms" by default.
    pub get_bar_label: Option<WaterfallBarLabelFn>,
    /// The scroll handle of the element the rows scroll in, whose height
    /// Page Up and Page Down move by (the scroller around the rows, in the
    /// original). Without one, they move 10 rows.
    pub scroller: Option<ScrollHandle>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl WaterfallOptions {
    pub fn items(mut self, items: impl Into<Rc<[WaterfallDataItem]>>) -> Self {
        self.items = items.into();
        self
    }

    pub fn range(mut self, range: Range) -> Self {
        self.range = Some(range);
        self
    }

    pub fn value(mut self, value: Option<impl Into<SharedString>>) -> Self {
        self.value = Some(value.map(Into::into));
        self
    }

    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&Option<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&Option<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn expanded(mut self, expanded: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.expanded = Some(expanded.into_iter().map(Into::into).collect());
        self
    }

    pub fn default_expanded(mut self, expanded: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.default_expanded = expanded.into_iter().map(Into::into).collect();
        self
    }

    pub fn on_expanded_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_expanded_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_expanded_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_expanded_change = Some(Rc::new(callback));
        self
    }

    pub fn on_highlight(
        mut self,
        callback: impl Fn(&SharedString, &ListHighlightDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_highlight = Some(Rc::new(callback));
        self
    }

    pub fn format(mut self, format: NumberFormat) -> Self {
        self.format = Some(format);
        self
    }

    pub fn locale(mut self, locale: impl Into<SharedString>) -> Self {
        self.locale = Some(locale.into());
        self
    }

    pub fn get_bar_label(
        mut self,
        label: impl Fn(&WaterfallDataItem, &WaterfallBarLabelDetails) -> String + 'static,
    ) -> Self {
        self.get_bar_label = Some(Rc::new(label));
        self
    }

    pub fn scroller(mut self, scroller: ScrollHandle) -> Self {
        self.scroller = Some(scroller);
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `axis` part takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WaterfallAxisProps {
    /// The most ticks the axis shows. Ticks fall on round times, so it may
    /// show fewer. 6 by default.
    pub max_ticks: Option<usize>,
}

impl WaterfallAxisProps {
    pub fn max_ticks(mut self, max_ticks: usize) -> Self {
        self.max_ticks = Some(max_ticks);
        self
    }
}

/// What the root's `with` and the app read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterfallState {
    /// The stretch of time the axis shows, as `[start, end]` in
    /// milliseconds.
    pub range: Range,
}

/// What a row's `with` reads. Its header, indicator, cells and bar get the
/// same.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterfallRowState {
    /// Whether it's a span with children that's expanded.
    pub expanded: bool,
    /// Whether it's the selected span.
    pub selected: bool,
    /// Whether its span has children.
    pub branch: bool,
    /// Its depth, 1 at the top level (`--waterfall-level`).
    pub level: usize,
    /// Where its span begins on the axis, from 0 to 100
    /// (`--waterfall-start`): place the bar from it.
    pub start: f64,
    /// How much of the axis its span covers, from 0 to 100
    /// (`--waterfall-size`).
    pub size: f64,
}

/// What the axis's `with` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterfallAxisState {
    /// The ticks it shows, at round times inside the range. Render a
    /// [`Waterfall::tick`] for each.
    pub ticks: Vec<WaterfallTickData>,
}

/// What a tick's or a mark's `with` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterfallTimeState {
    /// The moment it marks, in milliseconds.
    pub time: f64,
    /// The time as `format` and the locale show it.
    pub label: String,
    /// Where it sits between the range's start and end, from 0 to 100
    /// (`--waterfall-tick` or `--waterfall-mark`).
    pub position: f64,
    /// Whether its time is outside the range, so it doesn't show on the axis
    /// (`data-outside`).
    pub outside: bool,
}

// The keys a row handles, also for the cells in it.
const ROW_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Home",
    "End",
    "mod+Home",
    "mod+End",
    "PageUp",
    "PageDown",
    "Enter",
    " ",
    "typeahead",
];

impl Waterfall {
    /// The `root` part: a treegrid. Name it with `aria_label`. Build it
    /// before its rows: it starts their render, so the order they render in
    /// is page order.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<WaterfallState> {
        let waterfall = this.read(cx);
        waterfall.begin(cx);
        let entity = this.downgrade();
        let part = Part::new(&waterfall.instance, "root", None, waterfall.state())
            .aria_role(Role::TreeGrid)
            .unneeded(Need::Keys, "Focus stays on a row or a cell, whose keydown handles them.")
            .on_props(move |window, cx| {
                entity.update(cx, |waterfall, cx| waterfall.rendered(window, cx)).ok();
            });
        // Development builds check something names it.
        if ui_sdk_kernel::dev::DEV { part.keep_name() } else { part }
    }

    /// The `header-row` part: the row of column headers, above the rows.
    pub fn header_row(this: &Entity<Self>, cx: &App) -> Part<()> {
        let waterfall = this.read(cx);
        waterfall.headers.set(0);
        Part::new(&waterfall.instance, "header-row", None, ()).aria_role(Role::Row)
    }

    /// A `column-header` part: a column's header, like "Name" or "Timeline".
    /// Build them inside the header row, in order.
    pub fn column_header(this: &Entity<Self>, cx: &App) -> Part<()> {
        let waterfall = this.read(cx);
        let column = waterfall.headers.get() + 1;
        waterfall.headers.set(column);
        let key = column.to_string();
        Part::new(&waterfall.instance, "column-header", Some(&key), ()).aria_role(Role::ColumnHeader)
    }

    /// The `axis` part: the ruler of round times above the bars. Hidden from
    /// screen readers, which hear each bar's times. Render a
    /// [`Waterfall::tick`] for each of its state's ticks.
    pub fn axis(this: &Entity<Self>, props: WaterfallAxisProps, cx: &App) -> Part<WaterfallAxisState> {
        let waterfall = this.read(cx);
        let state = WaterfallAxisState {
            ticks: waterfall.ticks(props.max_ticks.unwrap_or(6), cx),
        };
        Part::new(&waterfall.instance, "axis", None, state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true))
    }

    /// A `tick` part: one time on the axis. Place it at its state's
    /// `position` and show its `label`.
    pub fn tick(this: &Entity<Self>, time: f64, cx: &App) -> Part<WaterfallTimeState> {
        let waterfall = this.read(cx);
        let key = time.to_string();
        Part::new(&waterfall.instance, "tick", Some(&key), waterfall.time_state(time, cx))
    }

    /// A `mark` part: a moment across every row, like DOMContentLoaded,
    /// placed at its state's `position`.
    pub fn mark(this: &Entity<Self>, time: f64, cx: &App) -> Part<WaterfallTimeState> {
        let waterfall = this.read(cx);
        let key = time.to_string();
        Part::new(&waterfall.instance, "mark", Some(&key), waterfall.time_state(time, cx))
    }

    /// A `row` part: one span's row, by the span's `value`. Build its header,
    /// cells and bar after it, as its children. It's hidden while a span
    /// around it is collapsed, so every span's row can render, or only the
    /// ones [`visible_waterfall_rows`] gives.
    pub fn row(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<WaterfallRowState> {
        let key: SharedString = value.into();
        let waterfall = this.read(cx);
        let state = waterfall.row_state(&key);
        let handle = waterfall.roving.focus_handle(&key, cx);
        let tab_stop = waterfall.bind_row(&key, &handle, cx);
        let shows = waterfall.shows(&key);
        let position = waterfall.position(&key);
        let typed = this.downgrade();
        let pressed = this.downgrade();
        let key_key = key.clone();
        let press_key = key.clone();
        let mut part = Part::new(&waterfall.instance, "row", Some(&key), state)
            .aria_role(Role::Row)
            .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
            .aria_level(state.level)
            .on_keys(ROW_KEYS, move |event, window, cx| {
                typed
                    .update(cx, |waterfall, cx| waterfall.keydown(event, &key_key, window, cx))
                    .ok();
            })
            .on_press(move |event, window, cx| {
                // Enter and Space reach the row's keydown, which selects.
                if matches!(event, InputEvent::Click(ClickEvent::Keyboard(_))) {
                    return;
                }
                pressed
                    .update(cx, |waterfall, cx| waterfall.press(&press_key, Some(event), window, cx))
                    .ok();
            });
        if !shows {
            part = part.hidden().aria(Aria::new().hidden(true));
        }
        if let Some(position) = position {
            part = part
                .aria_position_in_set(position.index)
                .aria_size_of_set(position.size);
        }
        if state.branch {
            part = part.aria_expanded(state.expanded);
        }
        if state.selected {
            part = part.aria_selected(true);
        }
        part
    }

    /// The `row-header` part of the row with `value`: the cell that names
    /// the span, which screen readers repeat as focus moves down a column.
    /// Put the indicator and the span's label in it; it's named by the
    /// span's label.
    pub fn row_header(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<WaterfallRowState> {
        let key: SharedString = value.into();
        let waterfall = this.read(cx);
        let (handle, tab_stop, _) = waterfall.bind_cell(&key, cx);
        let label = waterfall.label_of(&key);
        let part = Part::new(&waterfall.instance, "row-header", Some(&key), waterfall.row_state(&key))
            .aria_role(Role::RowHeader)
            .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false);
        match label {
            Some(label) => part.aria_label(label),
            None => part,
        }
    }

    /// The `indicator` part of the row with `value`: the arrow that expands
    /// and collapses a span with children. Hidden from screen readers, which
    /// hear the row's expanded state.
    pub fn indicator(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<WaterfallRowState> {
        let key: SharedString = value.into();
        let waterfall = this.read(cx);
        let entity = this.downgrade();
        Part::new(&waterfall.instance, "indicator", Some(&key), waterfall.row_state(&key))
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true))
            .on_press(move |event, window, cx| {
                // The row's press would select it too.
                cx.stop_propagation();
                entity
                    .update(cx, |waterfall, cx| waterfall.press_indicator(&key, Some(event), window, cx))
                    .ok();
            })
    }

    /// A `cell` part of the row with `value`: a column of your own, like a
    /// status or a size. Name it with `aria_label` of what it shows.
    pub fn cell(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<WaterfallRowState> {
        let key: SharedString = value.into();
        let waterfall = this.read(cx);
        let (handle, tab_stop, column) = waterfall.bind_cell(&key, cx);
        let id = format!("{key}/{column}");
        Part::new(&waterfall.instance, "cell", Some(&id), waterfall.row_state(&key))
            .aria_role(Role::GridCell)
            .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
    }

    /// The `bar` part of the row with `value`: the cell on the time axis,
    /// which screen readers hear as the span's start and duration. Place
    /// its bar from the state's `start` and `size`.
    pub fn bar(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<WaterfallRowState> {
        let key: SharedString = value.into();
        let waterfall = this.read(cx);
        let (handle, tab_stop, _) = waterfall.bind_cell(&key, cx);
        let label = waterfall.bar_label(&key, cx);
        let part = Part::new(&waterfall.instance, "bar", Some(&key), waterfall.row_state(&key))
            .aria_role(Role::GridCell)
            .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false);
        match label {
            Some(label) => part.aria_label(label),
            None => part,
        }
    }
}

/// `agent: { actions: ["setValue", "setExpanded", "expand", "collapse",
/// "expandAll", "collapseAll"] }` and `controlled: ["value", "expanded"]`,
/// with `toggle`, which `expand` and `collapse` cover, unlisted. Its items
/// are its spans, every level flattened.
impl Agent for Waterfall {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |waterfall, args, window, cx| {
            let value = if args.is_missing(0) { None } else { Some(args.string(0)?) };
            Ok(waterfall.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setExpanded", |waterfall, args, window, cx| {
            let expanded = args.list(0)?;
            Ok(waterfall.set_expanded(expanded, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("expand", |waterfall, args, window, cx| {
            let value = args.string(0)?;
            Ok(waterfall.expand(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("collapse", |waterfall, args, window, cx| {
            let value = args.string(0)?;
            Ok(waterfall.collapse(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("expandAll", |waterfall, _args, window, cx| {
            Ok(waterfall.expand_all(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("collapseAll", |waterfall, _args, window, cx| {
            Ok(waterfall.collapse_all(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |waterfall, args, window, cx| {
            let value = args.string(0)?;
            Ok(waterfall.toggle(value, ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["value", "expanded"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        let value = match self.get_value() {
            Some(value) => AgentValue::from(value.to_string()),
            None => AgentValue::Null,
        };
        let expanded: Vec<String> = self.get_expanded().iter().map(ToString::to_string).collect();
        AgentState::new().with("value", value).with("expanded", expanded)
    }

    fn agent_items(&self, _cx: &App) -> Option<Vec<AgentItem>> {
        fn flatten(items: &[WaterfallDataItem], into: &mut Vec<AgentItem>) {
            for item in items {
                into.push(AgentItem::new(item.value.clone(), item.label.clone()));
                flatten(item.children(), into);
            }
        }
        let mut into = Vec::new();
        flatten(self.items(), &mut into);
        Some(into)
    }
}
