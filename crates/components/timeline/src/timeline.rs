//! Port of `components/timeline/timeline.ts`: dated events in order, like an
//! activity feed, a changelog or an order's progress. It's a list for screen
//! readers, or a feed of articles, with one tab stop and the arrow keys
//! between events.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{
    App, Context, Entity, FocusHandle, KeyDownEvent, Orientation, Role, SharedString,
    StatefulInteractiveElement, Window,
};
use ui_sdk_focus::{RovingFocus, RovingFocusOptions, RovingMember, orientation_of};
use ui_sdk_kernel::part::{FocusMode, with_role};
use ui_sdk_kernel::roles::Role as AriaRole;
use ui_sdk_kernel::{Agent, AgentState, Aria, Current, Instance, Key, Part, dev, name_of};
use ui_sdk_text::{NameFrom, NameSources, check_name};

/// Which way the events run, which picks the arrow keys that move between
/// them.
pub type TimelineOrientation = Orientation;

/// Where an event stands in a process: done, the one in progress, or still
/// to come.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimelineStatus {
    /// `data-complete`.
    Complete,
    /// `data-current`, and `aria-current="step"` on its item.
    Current,
    /// `data-upcoming`.
    Upcoming,
}

impl TimelineStatus {
    /// The original's name for it, like `"current"`.
    pub fn as_str(self) -> &'static str {
        match self {
            TimelineStatus::Complete => "complete",
            TimelineStatus::Current => "current",
            TimelineStatus::Upcoming => "upcoming",
        }
    }
}

/// The options `Timeline` takes.
#[derive(Clone, Default)]
pub struct TimelineOptions {
    /// Which arrow keys move between events: up and down, or left and right.
    /// Vertical by default.
    pub orientation: Option<TimelineOrientation>,
    /// Whether arrow keys wrap from the last event to the first. True by
    /// default.
    pub r#loop: Option<bool>,
    /// Whether the events are a feed that loads more as people scroll,
    /// rather than a list. Each event becomes an article with its position,
    /// and Page Down and Page Up move between them from anywhere inside one.
    pub feed: bool,
    /// Whether a feed is loading or changing events, which sets busy so
    /// screen readers wait until it's done. Only a feed reads it.
    pub busy: bool,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl TimelineOptions {
    pub fn orientation(mut self, orientation: TimelineOrientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = Some(r#loop);
        self
    }

    pub fn feed(mut self, feed: bool) -> Self {
        self.feed = feed;
        self
    }

    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineState {
    /// Which way the events run (`data-orientation`).
    pub orientation: TimelineOrientation,
    /// Whether the events are a feed rather than a list.
    pub feed: bool,
    /// Whether a feed is loading or changing events. A feed that's busy is
    /// `data-busy` in the original.
    pub busy: bool,
    /// How many events it holds, as the last frame rendered them, the
    /// original's `--timeline-count`, for a horizontal row's grid columns.
    pub count: usize,
}

/// What an item's `with` reads, and every part inside it: its event's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineItemState {
    /// Where the event stands in a process, or `None` when it has no status.
    pub status: Option<TimelineStatus>,
    /// Which way the events run (`data-orientation`).
    pub orientation: TimelineOrientation,
    /// Whether it's the first event (`data-first`), once the events are in
    /// the page.
    pub first: bool,
    /// Whether it's the last event (`data-last`), like for hiding the
    /// connector after it.
    pub last: bool,
    /// Its place from 0, the original's `--timeline-index`, once the events
    /// are in the page, for staggering an animation.
    pub index: Option<usize>,
}

/// What the marker's `with` reads: its event's.
pub type TimelineMarkerState = TimelineItemState;
/// What the connector's `with` reads: its event's.
pub type TimelineConnectorState = TimelineItemState;
/// What the time's `with` reads: its event's.
pub type TimelineTimeState = TimelineItemState;
/// What the title's `with` reads: its event's.
pub type TimelineTitleState = TimelineItemState;
/// What the description's `with` reads: its event's.
pub type TimelineDescriptionState = TimelineItemState;

// The keys an event handles: the arrows, Home and End move between events,
// and a feed's Page Down and Page Up.
const ITEM_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Home",
    "End",
    "PageUp",
    "PageDown",
];

// Page Down and Page Up with no modifier: one event on, or one back.
fn page_step(event: &KeyDownEvent) -> Option<isize> {
    let modifiers = event.keystroke.modifiers;
    if modifiers.alt || modifiers.control || modifiers.platform || modifiers.shift {
        return None;
    }
    match Key::of(&event.keystroke) {
        Key::PageDown => Some(1),
        Key::PageUp => Some(-1),
        _ => None,
    }
}

/// A timeline. Its entity is its controller: `focus` moves focus to an
/// event from outside it, like a "Jump to current step" button.
pub struct Timeline {
    instance: Instance,
    orientation: TimelineOrientation,
    r#loop: bool,
    feed: bool,
    busy: bool,
    roving: RovingFocus<SharedString>,
    // The events in page order, as the last frame rendered them, and each
    // one's place. Only a feed writes positions from them, since a list's
    // come from its markup.
    order: Vec<SharedString>,
    places: HashMap<SharedString, usize>,
    // Each event's status, as its item last rendered, for the parts inside.
    statuses: RefCell<HashMap<SharedString, Option<TimelineStatus>>>,
    // Whether development builds checked that something names a feed.
    name_checked: bool,
}

impl Timeline {
    pub fn new(options: TimelineOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let orientation = orientation_of(options.orientation, Orientation::Vertical);
        let r#loop = options.r#loop.unwrap_or(true);
        let roving = RovingFocus::new(
            RovingFocusOptions::default()
                .orientation(orientation)
                .r#loop(r#loop),
            window,
            cx,
        );
        let mut instance = Instance::new("timeline").with_agent(cx);
        instance.debug = options.debug;
        Self {
            instance,
            orientation,
            r#loop,
            feed: options.feed,
            busy: options.busy,
            roving,
            order: Vec::new(),
            places: HashMap::new(),
            statuses: RefCell::new(HashMap::new()),
            name_checked: false,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: TimelineOptions, cx: &mut Context<Self>) {
        let orientation = orientation_of(options.orientation, Orientation::Vertical);
        let r#loop = options.r#loop.unwrap_or(true);
        let changed = self.orientation != orientation
            || self.r#loop != r#loop
            || self.feed != options.feed
            || self.busy != options.busy;
        self.orientation = orientation;
        self.r#loop = r#loop;
        self.feed = options.feed;
        self.busy = options.busy;
        self.roving.set_orientation(orientation);
        self.roving.set_loop(r#loop);
        self.instance.debug = options.debug;
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub fn state(&self) -> TimelineState {
        TimelineState {
            orientation: self.orientation,
            feed: self.feed,
            busy: self.busy,
            count: self.order.len(),
        }
    }

    /// Moves focus to an event by its `value`, or with `None` to the event
    /// that holds the tab stop. An event that isn't rendered yet takes the
    /// stop now and focus once it renders.
    pub fn focus(&self, value: Option<&str>, window: &mut Window, cx: &mut App) {
        let key = value.map(|value| SharedString::from(value.to_string()));
        self.roving.focus(key.as_ref(), window, cx);
    }

    /// The focus handle the item for `value` tracks, which stands for its
    /// element: made the first time it's asked for, and kept while the item
    /// renders. An app hears the event take focus, the item's `focus` event,
    /// with the kernel's `focus_events` on it.
    pub fn item_focus_handle(&self, value: &str, cx: &App) -> FocusHandle {
        self.roving
            .focus_handle(&SharedString::from(value.to_string()), cx)
    }

    // The root rendered: development builds check, once, as it first
    // renders, that something names it when it's a feed (`checkName`), from
    // the name it rendered with. A labelled-by names it, since its targets'
    // text can't be read.
    fn check_name(&mut self, feed: bool, window: &Window) {
        if !dev::DEV || self.name_checked {
            return;
        }
        self.name_checked = true;
        if !feed {
            return;
        }
        let Some(name) = name_of(&self.instance, "root", None) else {
            return;
        };
        if !name.labelled_by.is_empty() {
            return;
        }
        let mut sources = NameSources::new(AriaRole::Feed);
        if let Some(label) = name.aria_label {
            sources = sources.aria_label(label);
        }
        let from = NameFrom {
            component: "timeline",
            id: Some(self.instance.id().clone()),
        };
        check_name(&self.instance.element_id("root", None), &sources, "Timeline", from, window);
    }

    // Where the events are now: the first and last, and each one's place,
    // which the parts read. It runs once the frame that rendered them is
    // drawn, so events that joined, left or moved, like a list sorted newest
    // first, get their places.
    fn measure(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<SharedString> = self.roving.items().into_iter().map(|item| item.key).collect();
        if keys != self.order {
            self.places = keys
                .iter()
                .enumerate()
                .map(|(index, key)| (key.clone(), index))
                .collect();
            self.order = keys;
            cx.notify();
        }
        let places = &self.places;
        self.statuses.borrow_mut().retain(|key, _| places.contains_key(key));
    }

    fn item_state(&self, key: &SharedString, status: Option<TimelineStatus>) -> TimelineItemState {
        TimelineItemState {
            status,
            orientation: self.orientation,
            first: self.order.first() == Some(key),
            last: self.order.last() == Some(key),
            index: self.places.get(key).copied(),
        }
    }

    // The state of a part inside an event: its item's, with the status the
    // item rendered with.
    fn inside_state(&self, key: &SharedString) -> TimelineItemState {
        let status = self.statuses.borrow().get(key).copied().flatten();
        self.item_state(key, status)
    }

    /// The `root` part: a list of events, or a feed of articles. Name it
    /// with `aria_label`, which the feed pattern asks for.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<TimelineState> {
        let timeline = this.read(cx);
        timeline.roving.begin(cx);
        let entity = this.downgrade();
        let feed = timeline.feed;
        let mut part = Part::new(&timeline.instance, "root", None, timeline.state())
            .aria_role(if feed { AriaRole::Feed } else { AriaRole::List })
            .aria(Aria::new().busy(feed && timeline.busy));
        // The feed pattern asks for a name, which development builds check.
        if dev::DEV && feed && !timeline.name_checked {
            part = part.keep_name();
        }
        part.on_props(move |window, cx| {
            entity
                .update(cx, |timeline, cx| {
                    timeline.check_name(feed, window);
                    timeline.measure(cx);
                })
                .ok();
        })
    }

    /// An `item` part: one event, by its `value`, which names it for
    /// `focus` and for the parts inside it, and its `status` in a process.
    /// The `Current` one is `aria-current="step"` and takes the tab stop
    /// first. It's named by its title and described by its time and
    /// description, when they render. Arrow keys move only from the event
    /// itself, so controls inside keep them; a feed's Page Down and Page Up
    /// move from anywhere inside one.
    pub fn item(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        status: Option<TimelineStatus>,
        cx: &App,
    ) -> Part<TimelineItemState> {
        let timeline = this.read(cx);
        let key: SharedString = value.into();
        timeline.statuses.borrow_mut().insert(key.clone(), status);
        let current = status == Some(TimelineStatus::Current);
        let handle = timeline.roving.focus_handle(&key, cx);
        let tab_stop = timeline
            .roving
            .bind(key.clone(), &handle, RovingMember::new().preferred(current), cx);
        let state = timeline.item_state(&key, status);
        let id = &timeline.instance;
        let aria = Aria::new()
            .current(current.then_some(Current::Step))
            .labelled_by(id.element_id("title", Some(&key)))
            .described_by(id.element_id("time", Some(&key)))
            .described_by(id.element_id("description", Some(&key)));
        let roving = timeline.roving.clone();
        let feed = timeline.feed;
        let own = handle.clone();
        let pressed = key.clone();
        let part = Part::new(&timeline.instance, "item", Some(&key), state)
            .aria_role(if feed {
                AriaRole::Article
            } else {
                AriaRole::ListItem
            })
            .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
            .aria(aria)
            .on_keys(ITEM_KEYS, move |event, window, cx| {
                if window.default_prevented() {
                    return;
                }
                if let Some(step) = feed.then(|| page_step(event)).flatten() {
                    let items = roving.items();
                    let next = items
                        .iter()
                        .position(|item| item.key == pressed)
                        .and_then(|index| index.checked_add_signed(step))
                        .and_then(|index| items.get(index));
                    if let Some(next) = next {
                        window.prevent_default();
                        roving.focus(Some(&next.key), window, cx);
                    }
                    return;
                }
                if own.is_focused(window) && !event.keystroke.modifiers.shift {
                    roving.keydown(event, &pressed, window, cx);
                }
            });
        match (feed, state.index) {
            (true, Some(index)) => part
                .aria_position_in_set(index + 1)
                .aria_size_of_set(timeline.order.len()),
            _ => part,
        }
    }

    /// The `marker` part: the dot or icon on the line, for its event's
    /// `value`. Only for looks, so it's hidden from screen readers. Like
    /// every part inside an item, it reads the status its item rendered
    /// with, so build it after the item.
    pub fn marker(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<TimelineMarkerState> {
        let timeline = this.read(cx);
        let key: SharedString = value.into();
        Part::new(&timeline.instance, "marker", Some(&key), timeline.inside_state(&key))
            .aria(Aria::new().hidden(true))
    }

    /// The `connector` part: the line to the next event, for its event's
    /// `value`. Only for looks, so it's hidden from screen readers.
    pub fn connector(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        cx: &App,
    ) -> Part<TimelineConnectorState> {
        let timeline = this.read(cx);
        let key: SharedString = value.into();
        Part::new(&timeline.instance, "connector", Some(&key), timeline.inside_state(&key))
            .aria(Aria::new().hidden(true))
    }

    /// The `time` part, for its event's `value`, which describes the event.
    /// Name it with an `aria_label` of the time as it reads, since a div's
    /// plain text children aren't read in gpui.
    pub fn time(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<TimelineTimeState> {
        let timeline = this.read(cx);
        let key: SharedString = value.into();
        Part::new(&timeline.instance, "time", Some(&key), timeline.inside_state(&key))
            .aria_role(AriaRole::Time)
    }

    /// The `title` part, for its event's `value`, which names the event.
    /// Give it its text as an `aria_label`, or as accessible text.
    pub fn title(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<TimelineTitleState> {
        let timeline = this.read(cx);
        let key: SharedString = value.into();
        // A role of its own, so the item's labelled-by finds it.
        Part::new(&timeline.instance, "title", Some(&key), timeline.inside_state(&key))
            .map_el(|el| with_role(el, Role::Label))
    }

    /// The `description` part, for its event's `value`, which describes the
    /// event after its time. Give it its text as an `aria_label`, or as
    /// accessible text.
    pub fn description(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        cx: &App,
    ) -> Part<TimelineDescriptionState> {
        let timeline = this.read(cx);
        let key: SharedString = value.into();
        // A role of its own, so the item's described-by finds it.
        Part::new(&timeline.instance, "description", Some(&key), timeline.inside_state(&key))
            .map_el(|el| with_role(el, Role::Label))
    }
}

/// No `agent` key, `controlled` or api method named like a change: an agent
/// reads a timeline's events as the parts they are, and its state holds no
/// key the bridge reports.
impl Agent for Timeline {
    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
    }
}
