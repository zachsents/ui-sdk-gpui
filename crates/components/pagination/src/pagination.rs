//! Port of `components/pagination/pagination.ts`: pages, with links or
//! buttons for each page and steps to the previous, next, first and last.

use std::rc::Rc;

use gpui::{App, Context, Entity, FocusHandle, SharedString, Window};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, Aria, ChangeCallback, ChangeDetails, ChangeOptions, Controllable,
    Current, InputEvent, Instance, Part, Parts, Reason,
};
use ui_sdk_platform::{LinkTarget, opens_elsewhere};

use crate::pagination_range::{
    PaginationRangeItem, PaginationRangeOptions, clamp_page, clamp_page_number, page_count_of,
    pagination_range,
};

/// Why a pagination's page wants to change: `ItemPress` for a page link,
/// `PreviousPress`, `NextPress`, `FirstPress` or `LastPress` for a step, and
/// `Imperative` for an api call.
pub type PaginationPageChangeReason = Reason;

/// Options for every write method: the reason and event `on_page_change`
/// receives.
pub type PaginationChangeOptions = ChangeOptions;

/// The URL of a page.
pub type GetHref = Rc<dyn Fn(usize) -> SharedString>;

/// The options `Pagination` takes.
#[derive(Clone, Default)]
pub struct PaginationOptions {
    /// The current page, from 1. Pass it to control the pagination.
    pub page: Option<usize>,
    /// The page it starts on, when uncontrolled. 1 by default.
    pub default_page: Option<usize>,
    /// Runs when the page changes: a page link (`ItemPress`), the Previous,
    /// Next, First or Last part (`PreviousPress`, `NextPress`, `FirstPress`,
    /// `LastPress`), or an api call (`Imperative`). It runs once the
    /// pagination's update is over, so it may update any entity, the pagination
    /// too.
    pub on_page_change: Option<ChangeCallback<usize>>,
    /// Runs as the page is asked to change, before it does, inside the
    /// pagination's update: `details.cancel()` keeps the page, and stops a link
    /// from navigating: the press calls `window.prevent_default()`, which an
    /// app's own click handler on the part, like a router's, checks. It may
    /// update any entity but the pagination.
    pub on_before_page_change: Option<ChangeCallback<usize>>,
    /// How many pages there are. Or pass `total` and `page_size`.
    pub page_count: Option<usize>,
    /// How many items there are, for `page_count` from `page_size`.
    pub total: Option<usize>,
    /// How many items fit on a page, with `total`. 10 by default.
    pub page_size: Option<usize>,
    /// How many pages show on each side of the current one. 1 by default.
    pub siblings: Option<usize>,
    /// How many pages always show at each end. 1 by default.
    pub boundaries: Option<usize>,
    /// The URL of a page. With it, parts are links, with the URL in their
    /// state for the app to follow, and a press with a modifier key or
    /// another button leaves the page as it is, for the app to open the link
    /// elsewhere, as a browser opens a new tab. Without it, they're buttons.
    pub get_href: Option<GetHref>,
    /// Whether every page and step does nothing, like a step at its end. An
    /// api call still pages it.
    pub disabled: bool,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl PaginationOptions {
    pub fn page(mut self, page: usize) -> Self {
        self.page = Some(page);
        self
    }

    pub fn default_page(mut self, page: usize) -> Self {
        self.default_page = Some(page);
        self
    }

    pub fn on_page_change(
        mut self,
        callback: impl Fn(&usize, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_page_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_page_change(
        mut self,
        callback: impl Fn(&usize, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_page_change = Some(Rc::new(callback));
        self
    }

    pub fn page_count(mut self, page_count: usize) -> Self {
        self.page_count = Some(page_count);
        self
    }

    pub fn total(mut self, total: usize) -> Self {
        self.total = Some(total);
        self
    }

    pub fn page_size(mut self, page_size: usize) -> Self {
        self.page_size = Some(page_size);
        self
    }

    pub fn siblings(mut self, siblings: usize) -> Self {
        self.siblings = Some(siblings);
        self
    }

    pub fn boundaries(mut self, boundaries: usize) -> Self {
        self.boundaries = Some(boundaries);
        self
    }

    pub fn get_href(mut self, get_href: impl Fn(usize) -> SharedString + 'static) -> Self {
        self.get_href = Some(Rc::new(get_href));
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root's and the list's `with` and the app read.
#[derive(Clone, Debug, PartialEq)]
pub struct PaginationState {
    /// The current page, from 1.
    pub page: usize,
    /// How many pages there are.
    pub page_count: usize,
    /// The pages to show, with an ellipsis for each gap. Render a link or an
    /// ellipsis for each, keyed by [`PaginationRangeItem::key`].
    pub items: Vec<PaginationRangeItem>,
    /// Whether every page and step does nothing (`data-disabled`).
    pub disabled: bool,
}

/// What a link's `with` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct PaginationLinkState {
    /// The page it goes to.
    pub page: usize,
    /// Whether it's the current page (`aria-current="page"`, `data-current`).
    pub current: bool,
    /// Whether the pagination is disabled, so it does nothing
    /// (`data-disabled`).
    pub disabled: bool,
    /// Where it goes, with `get_href`, while it isn't disabled.
    pub href: Option<SharedString>,
}

/// What the Previous, Next, First and Last parts' `with` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct PaginationStepState {
    /// The page it goes to.
    pub page: usize,
    /// Whether it's at the end it goes to, or the pagination is disabled, so
    /// it does nothing (`data-disabled`).
    pub disabled: bool,
    /// Where it goes, with `get_href`, while it isn't disabled.
    pub href: Option<SharedString>,
}

/// One of the step parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaginationStep {
    Previous,
    Next,
    First,
    Last,
}

impl PaginationStep {
    /// Its slot, like `"previous"`.
    pub fn slot(self) -> &'static str {
        match self {
            PaginationStep::Previous => "previous",
            PaginationStep::Next => "next",
            PaginationStep::First => "first",
            PaginationStep::Last => "last",
        }
    }

    /// The reason a press on it reports, like `PreviousPress`.
    pub fn reason(self) -> Reason {
        match self {
            PaginationStep::Previous => Reason::PreviousPress,
            PaginationStep::Next => Reason::NextPress,
            PaginationStep::First => Reason::FirstPress,
            PaginationStep::Last => Reason::LastPress,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

// Where a part points, and whether it does nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Target {
    page: usize,
    disabled: bool,
}

// The part a press came from: a page link, or a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pressed {
    Link(usize),
    Step(PaginationStep),
}

/// A pagination. Its entity is its controller: `get_page`, `get_page_count`,
/// `set_page`, `next`, `previous`, `first` and `last` page it from anywhere.
pub struct Pagination {
    instance: Instance,
    page: Controllable<usize>,
    page_count: usize,
    siblings: usize,
    boundaries: usize,
    get_href: Option<GetHref>,
    disabled: bool,
    // The links' focus handles, by page, so a link keeps focus as the range
    // moves.
    links: Parts<usize>,
    steps: [FocusHandle; 4],
}

struct Settings {
    page_count: usize,
    siblings: usize,
    boundaries: usize,
    get_href: Option<GetHref>,
    disabled: bool,
}

fn settings(options: &PaginationOptions) -> Settings {
    let page_count = match (options.page_count, options.total) {
        (Some(page_count), _) => page_count.max(1),
        (None, Some(total)) => page_count_of(total, options.page_size.unwrap_or(10)),
        (None, None) => 1,
    };
    Settings {
        page_count,
        siblings: options.siblings.unwrap_or(1),
        boundaries: options.boundaries.unwrap_or(1),
        get_href: options.get_href.clone(),
        disabled: options.disabled,
    }
}

impl Pagination {
    pub fn new(options: PaginationOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first = options.page.or(options.default_page).unwrap_or(1);
        let mut page = Controllable::new("page", first);
        page.set_controlled(options.page);
        page.set_on_change(options.on_page_change.clone());
        page.set_on_before_change(options.on_before_page_change.clone());
        let mut instance = Instance::new("pagination").with_agent(cx);
        instance.debug = options.debug;
        let settings = settings(&options);
        Self {
            instance,
            page,
            page_count: settings.page_count,
            siblings: settings.siblings,
            boundaries: settings.boundaries,
            get_href: settings.get_href,
            disabled: settings.disabled,
            links: Parts::new(),
            steps: [cx.focus_handle(), cx.focus_handle(), cx.focus_handle(), cx.focus_handle()],
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: PaginationOptions, cx: &mut Context<Self>) {
        let mut changed = self.page.set_controlled(options.page);
        self.page.set_on_change(options.on_page_change.clone());
        self.page.set_on_before_change(options.on_before_page_change.clone());
        let settings = settings(&options);
        // A `get_href` an app makes anew each render counts as the same.
        changed |= self.page_count != settings.page_count
            || self.siblings != settings.siblings
            || self.boundaries != settings.boundaries
            || self.disabled != settings.disabled
            || self.get_href.is_some() != settings.get_href.is_some();
        self.page_count = settings.page_count;
        self.siblings = settings.siblings;
        self.boundaries = settings.boundaries;
        self.get_href = settings.get_href;
        self.disabled = settings.disabled;
        self.instance.debug = options.debug;
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled page, or `None` to let the pagination keep its
    /// own.
    pub fn set_controlled_page(&mut self, page: Option<usize>, cx: &mut Context<Self>) {
        if self.page.set_controlled(page) {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    fn current_page(&self) -> usize {
        clamp_page(*self.page.get(), self.page_count)
    }

    /// The current page, from 1. A page past the end, like after the count
    /// shrinks, reads as the last page.
    pub fn get_page(&self) -> usize {
        self.current_page()
    }

    /// How many pages there are.
    pub fn get_page_count(&self) -> usize {
        self.page_count
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// The range the parts render: the pages to show, with an ellipsis for
    /// each gap.
    pub fn items(&self) -> Vec<PaginationRangeItem> {
        pagination_range(PaginationRangeOptions {
            page: self.current_page(),
            page_count: self.page_count,
            siblings: Some(self.siblings),
            boundaries: Some(self.boundaries),
        })
    }

    pub fn state(&self) -> PaginationState {
        PaginationState {
            page: self.current_page(),
            page_count: self.page_count,
            items: self.items(),
            disabled: self.disabled,
        }
    }

    // Where a step part points. The api steps from the current page.
    fn step_target(&self, step: PaginationStep) -> Target {
        let page = self.current_page();
        let target = match step {
            PaginationStep::Previous => page.saturating_sub(1),
            PaginationStep::Next => page + 1,
            PaginationStep::First => 1,
            PaginationStep::Last => self.page_count,
        };
        let at_end = match step {
            PaginationStep::Previous | PaginationStep::First => page == 1,
            PaginationStep::Next | PaginationStep::Last => page == self.page_count,
        };
        Target {
            page: clamp_page(target, self.page_count),
            disabled: at_end,
        }
    }

    // What a step part does: nothing at its end or while the pagination is
    // disabled.
    fn step_part(&self, step: PaginationStep) -> Target {
        let target = self.step_target(step);
        Target {
            disabled: target.disabled || self.disabled,
            ..target
        }
    }

    // A part's URL: none without `get_href`, and none at an end, so a link
    // there goes nowhere.
    fn href(&self, page: usize, disabled: bool) -> Option<SharedString> {
        match &self.get_href {
            Some(get_href) if !disabled => Some(get_href(page)),
            _ => None,
        }
    }

    /// Goes to a page, clamped to the range. Returns false when canceled or
    /// already there.
    pub fn set_page(
        &mut self,
        page: usize,
        options: PaginationChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.go(page, options, None, window, cx)
    }

    /// Goes to the next page. Returns false on the last page.
    pub fn next(&mut self, options: PaginationChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.step(PaginationStep::Next, options, window, cx)
    }

    /// Goes to the previous page. Returns false on the first page.
    pub fn previous(&mut self, options: PaginationChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.step(PaginationStep::Previous, options, window, cx)
    }

    /// Goes to the first page.
    pub fn first(&mut self, options: PaginationChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.step(PaginationStep::First, options, window, cx)
    }

    /// Goes to the last page.
    pub fn last(&mut self, options: PaginationChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.step(PaginationStep::Last, options, window, cx)
    }

    fn step(
        &mut self,
        step: PaginationStep,
        options: PaginationChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let target = self.step_target(step);
        !target.disabled && self.go(target.page, options, None, window, cx)
    }

    fn go(
        &mut self,
        page: usize,
        options: PaginationChangeOptions,
        pressed: Option<Pressed>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let target = clamp_page(page, self.page_count);
        if target == self.current_page() {
            return false;
        }
        self.keep_focus(pressed, window, cx);
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        let changed = self.page.change(&self.instance, target, options, window, cx);
        if changed {
            cx.notify();
        }
        changed
    }

    fn handle_of(&self, pressed: Pressed, cx: &App) -> FocusHandle {
        match pressed {
            Pressed::Link(page) => self.links.focus_handle(&page, cx),
            Pressed::Step(step) => self.steps[step.index()].clone(),
        }
    }

    // Whether focus is on one of its pages or steps.
    fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.links.contains_focus(window, cx)
            || self.steps.iter().any(|handle| handle.contains_focused(window, cx))
    }

    // Puts focus back when the new page took it away: the pressed part
    // stopped rendering, like a link the app no longer shows, which leaves
    // focus with nothing. Focus goes to the pressed part, or the one that
    // replaced it: the current page's link, or the same step. Once the new
    // page has rendered. Focus the app moved on purpose stays.
    fn keep_focus(&self, pressed: Option<Pressed>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pressed) = pressed else {
            return;
        };
        if !self.contains_focus(window, cx) {
            return;
        }
        let this = cx.weak_entity();
        window.on_next_frame(move |window, _cx| {
            window.on_next_frame(move |window, cx| {
                if window
                    .focused(cx)
                    .is_some_and(|focused| focused.is_rendered(window))
                {
                    return;
                }
                let Some(this) = this.upgrade() else {
                    return;
                };
                let target = {
                    let pagination = this.read(cx);
                    let handle = pagination.handle_of(pressed, cx);
                    if handle.is_rendered(window) {
                        Some(handle)
                    } else {
                        match pressed {
                            Pressed::Link(_) => pagination
                                .links
                                .existing_focus_handle(&pagination.current_page())
                                .filter(|handle| handle.is_rendered(window)),
                            Pressed::Step(_) => None,
                        }
                    }
                };
                if let Some(target) = target {
                    window.focus(&target, cx);
                }
            });
        });
    }

    // A press on a page link or a step part. A link pressed with a modifier
    // opens elsewhere and leaves the page as it is. A canceled change stops a
    // link from navigating.
    fn press(
        &mut self,
        pressed: Pressed,
        target: Target,
        event: InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if target.disabled {
            window.prevent_default();
            return;
        }
        let link = self.get_href.as_ref().map(|_| LinkTarget::default());
        if opens_elsewhere(Some(&event), link.as_ref()) || target.page == self.current_page() {
            return;
        }
        let reason = match pressed {
            Pressed::Link(_) => Reason::ItemPress,
            Pressed::Step(step) => step.reason(),
        };
        if !self.go(target.page, ChangeOptions::event_of(reason, event), Some(pressed), window, cx) {
            window.prevent_default();
        }
    }

    // A link with `get_href`, else a button.
    fn role(&self) -> Role {
        if self.get_href.is_some() {
            Role::Link
        } else {
            Role::Button
        }
    }

    /// The `root` part: the navigation landmark. Name it with `aria_label`.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<PaginationState> {
        let pagination = this.read(cx);
        Part::new(&pagination.instance, "root", None, pagination.state()).aria_role(Role::Navigation)
    }

    /// The `list` part, which holds the items. It keeps the list role
    /// however it's styled.
    pub fn list(this: &Entity<Self>, cx: &App) -> Part<PaginationState> {
        let pagination = this.read(cx);
        Part::new(&pagination.instance, "list", None, pagination.state()).aria_role(Role::List)
    }

    /// An `item` part, which holds a link, a step or an ellipsis, by a key
    /// of its own, like the range entry's [`PaginationRangeItem::key`] or
    /// `"previous"`.
    pub fn item(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part {
        let pagination = this.read(cx);
        let key = key.into();
        Part::new(&pagination.instance, "item", Some(&key), ()).aria_role(Role::ListItem)
    }

    /// A `link` part, for `page`: a link with `get_href`, else a button. The
    /// current page's is `aria-current="page"`. While the pagination is
    /// disabled, a link has no URL and leaves the tab order, keeping focus if
    /// it had it, and a button stays in the tab order.
    pub fn link(this: &Entity<Self>, page: usize, cx: &App) -> Part<PaginationLinkState> {
        let pagination = this.read(cx);
        let disabled = pagination.disabled;
        let current = page == pagination.current_page();
        let state = PaginationLinkState {
            page,
            current,
            disabled,
            href: pagination.href(page, disabled),
        };
        let handle = pagination.links.focus_handle(&page, cx);
        let unfocusable = pagination.get_href.is_some() && disabled;
        let entity = this.downgrade();
        Part::new(&pagination.instance, "link", Some(&page.to_string()), state)
            .aria_role(pagination.role())
            .focus_mode(FocusMode::Native, Some(&handle), unfocusable)
            .aria(
                Aria::new()
                    .disabled(disabled)
                    .current(current.then_some(Current::Page)),
            )
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |pagination, cx| {
                        let target = Target {
                            page,
                            disabled: pagination.disabled,
                        };
                        pagination.press(Pressed::Link(page), target, event, window, cx);
                    })
                    .ok();
            })
    }

    fn step_slot(this: &Entity<Self>, step: PaginationStep, cx: &App) -> Part<PaginationStepState> {
        let pagination = this.read(cx);
        let target = pagination.step_part(step);
        let state = PaginationStepState {
            page: target.page,
            disabled: target.disabled,
            href: pagination.href(target.page, target.disabled),
        };
        let unfocusable = pagination.get_href.is_some() && target.disabled;
        let entity = this.downgrade();
        Part::new(&pagination.instance, step.slot(), None, state)
            .aria_role(pagination.role())
            .focus_mode(FocusMode::Native, Some(&pagination.steps[step.index()]), unfocusable)
            .aria(Aria::new().disabled(target.disabled))
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |pagination, cx| {
                        let target = pagination.step_part(step);
                        pagination.press(Pressed::Step(step), target, event, window, cx);
                    })
                    .ok();
            })
    }

    /// The `previous` part: a step to the page before. At the first page, or
    /// while the pagination is disabled, it's disabled and does nothing: a
    /// link has no URL and leaves the tab order, keeping focus if it had it,
    /// and a button stays in the tab order. Give it text or an `aria_label`.
    pub fn previous_part(this: &Entity<Self>, cx: &App) -> Part<PaginationStepState> {
        Self::step_slot(this, PaginationStep::Previous, cx)
    }

    /// The `next` part: a step to the page after, disabled at the last page.
    pub fn next_part(this: &Entity<Self>, cx: &App) -> Part<PaginationStepState> {
        Self::step_slot(this, PaginationStep::Next, cx)
    }

    /// The `first` part: a step to page 1, disabled there.
    pub fn first_part(this: &Entity<Self>, cx: &App) -> Part<PaginationStepState> {
        Self::step_slot(this, PaginationStep::First, cx)
    }

    /// The `last` part: a step to the last page, disabled there.
    pub fn last_part(this: &Entity<Self>, cx: &App) -> Part<PaginationStepState> {
        Self::step_slot(this, PaginationStep::Last, cx)
    }

    /// An `ellipsis` part, for a gap, by its entry's key. Only the gap's
    /// look: the links on each side already say which pages there are, so
    /// it's hidden from screen readers.
    pub fn ellipsis(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part {
        let pagination = this.read(cx);
        let key = key.into();
        Part::new(&pagination.instance, "ellipsis", Some(&key), ()).aria(Aria::new().hidden(true))
    }
}

/// `agent: { actions: ["setPage", "next", "previous", "first", "last"] }`
/// and `controlled: ["page"]`.
impl Agent for Pagination {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setPage", |pagination, args, window, cx| {
            let page = clamp_page_number(args.number(0)?, pagination.page_count);
            Ok(pagination.set_page(page, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("next", |pagination, _args, window, cx| {
            Ok(pagination.next(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("previous", |pagination, _args, window, cx| {
            Ok(pagination.previous(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("first", |pagination, _args, window, cx| {
            Ok(pagination.first(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("last", |pagination, _args, window, cx| {
            Ok(pagination.last(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["page"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("page", self.current_page())
            .with("disabled", self.disabled)
    }
}
