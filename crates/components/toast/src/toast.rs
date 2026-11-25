//! Port of `components/toast/toast.ts`: a region that shows a toaster's
//! toasts, and its public types. The queue lives in [`toaster`](crate::toaster).
//! What's on screen, for how long, how it's announced and where focus goes is
//! in [`toast_api`](crate::toast_api), the parts in
//! [`toast_parts`](crate::toast_parts), the countdowns in
//! [`toast_timers`](crate::toast_timers) and the window side in
//! [`toast_dom`](crate::toast_dom).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    App, Context, FocusHandle, Pixels, Point, SharedString, Subscription, Task, WeakFocusHandle,
    Window,
};
use ui_sdk_drag_and_drop::DragSession;
use ui_sdk_kernel::{
    ActionError, ActionOutcome, Agent, AgentAction, AgentState, AgentValue, AnnouncePending,
    InputEvent, Instance, Reason, after_update, flag_while_bound, while_destroying,
};
use ui_sdk_motion::{PresenceState, PresenceTiming};
use ui_sdk_platform::{Region, TrackKeyboard, track_keyboard};

use crate::swipe::SwipeDirection;
use crate::toast_api::{RegionBinding, ToastEntry, has_toasts};
use crate::toast_timers::{ToastPauseReason, ToastTimers, create_toast_timers};
use crate::toaster::{
    Toast, ToastActionDetails, ToastActionHandler, ToastCancel, ToastType, Toaster, toast,
};

/// The edge of the window a region sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ToastSide {
    Top,
    #[default]
    Bottom,
}

impl ToastSide {
    /// The original's name for it, its `data-side`.
    pub fn as_str(self) -> &'static str {
        match self {
            ToastSide::Top => "top",
            ToastSide::Bottom => "bottom",
        }
    }
}

/// Where along its edge a region sits. `Start` and `End` follow the writing
/// direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ToastAlign {
    Start,
    Center,
    #[default]
    End,
}

impl ToastAlign {
    /// The original's name for it, its `data-align`.
    pub fn as_str(self) -> &'static str {
        match self {
            ToastAlign::Start => "start",
            ToastAlign::Center => "center",
            ToastAlign::End => "end",
        }
    }
}

/// The options `ToastRegion` takes.
#[derive(Clone)]
pub struct ToastRegionOptions {
    /// The queue it shows. A second toaster, from `create_toaster`, gives a
    /// second region its own toasts. `None` is the shared [`toast`].
    pub toaster: Option<Toaster>,
    /// The edge of the window it sits on. Swiping toward it dismisses a toast.
    /// Defaults to `Bottom`.
    pub side: ToastSide,
    /// Where along that edge. `Start` and `End` follow the writing direction,
    /// and swiping toward them dismisses too. Defaults to `End`.
    pub align: ToastAlign,
    /// How many toasts show at once. Older ones wait, with their timers
    /// stopped, until there's room. Defaults to 3.
    pub max_visible: usize,
    /// Whether the stack is always expanded. Otherwise it expands while the
    /// pointer or focus is in it.
    pub expanded: bool,
    /// The key that moves focus into the region and back, like `"F6"` or
    /// `"alt+t"`. `None`, or an empty one, turns it off. Defaults to `"F6"`.
    pub shortcut: Option<SharedString>,
    /// What screen readers hear once a toast's action or close button has
    /// been pending for a second, so a slow action doesn't pass in silence.
    /// Set it in the app's language. An empty one says nothing. `None` says
    /// "Loading".
    pub pending_announcement: Option<SharedString>,
    /// The region's name while it has toasts, in the app's language. `None`
    /// is "Notifications". The original takes it from the `aria-label` an
    /// app gives the region.
    pub label: Option<SharedString>,
    /// Whether the app placed the region, rather than the overlay host
    /// rendering it. Defaults to true; a host passes false.
    pub placed: bool,
    /// How toasts animate in and out: the transitions the app's styles
    /// would declare on them. The default has none.
    pub timing: PresenceTiming,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for ToastRegionOptions {
    fn default() -> Self {
        Self {
            toaster: None,
            side: ToastSide::Bottom,
            align: ToastAlign::End,
            max_visible: 3,
            expanded: false,
            shortcut: Some("F6".into()),
            pending_announcement: None,
            label: None,
            placed: true,
            timing: PresenceTiming::default(),
            debug: false,
        }
    }
}

impl ToastRegionOptions {
    pub fn toaster(mut self, toaster: Toaster) -> Self {
        self.toaster = Some(toaster);
        self
    }

    pub fn side(mut self, side: ToastSide) -> Self {
        self.side = side;
        self
    }

    pub fn align(mut self, align: ToastAlign) -> Self {
        self.align = align;
        self
    }

    pub fn max_visible(mut self, max_visible: usize) -> Self {
        self.max_visible = max_visible;
        self
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn shortcut(mut self, shortcut: Option<SharedString>) -> Self {
        self.shortcut = shortcut;
        self
    }

    pub fn pending_announcement(mut self, announcement: impl Into<SharedString>) -> Self {
        self.pending_announcement = Some(announcement.into());
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn placed(mut self, placed: bool) -> Self {
        self.placed = placed;
        self
    }

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = timing;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// A toast as a region renders it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToastItem {
    /// The toast's data.
    pub toast: Toast,
    /// Its place in the stack, 0 for the newest. A closing toast keeps the
    /// place it had.
    pub index: usize,
    /// Whether it's animating out, closed or pushed past `max_visible`.
    pub leaving: bool,
}

/// The region's state, what its part's `with` and the app read.
#[derive(Clone, Debug, PartialEq)]
pub struct ToastRegionState {
    /// The edge it sits on (`data-side`).
    pub side: ToastSide,
    /// Where along that edge (`data-align`).
    pub align: ToastAlign,
    /// Whether the stack is expanded (`data-expanded`).
    pub expanded: bool,
    /// The toasts to render, newest first, including ones animating out.
    pub toasts: Vec<ToastItem>,
    /// The newest toast's height, kept until a new front toast is measured
    /// so the toasts behind it don't collapse for a frame; 0 with no toast in
    /// front, and `None` until one is measured (`--toast-front-height`).
    pub front_height: Option<Pixels>,
    /// Every shown toast's height added up (`--toast-stack-height`).
    pub stack_height: Pixels,
    /// How many toasts show, leaving ones aside (`--toast-count`).
    pub count: usize,
}

/// A toast's state, what its root part's `with` and the app read.
#[derive(Clone, Debug, PartialEq)]
pub struct ToastState {
    /// What it's about (`data-type`).
    pub r#type: ToastType,
    /// Its place in the stack, 0 for the newest (`--toast-index`).
    pub index: usize,
    /// Whether it's the newest, on top of a collapsed stack. A closing toast
    /// keeps it (`data-front`).
    pub front: bool,
    /// Whether the stack is expanded (`data-expanded`).
    pub expanded: bool,
    /// Whether it's animating out. A part can render once more after its
    /// toast left the region.
    pub leaving: bool,
    /// How far the toasts in front of it reach, their heights added up,
    /// once measured (`--toast-offset`).
    pub offset: Option<Pixels>,
    /// Its own height, once measured (`--toast-height`).
    pub height: Option<Pixels>,
    /// Whether it's being swiped (`data-swiping`).
    pub swiping: bool,
    /// How far a swipe moved it, while it's swiped and as a swipe that
    /// dismissed it leaves (`--toast-swipe-x`, `--toast-swipe-y`).
    pub swipe: Option<Point<Pixels>>,
    /// Which way a swipe that dismissed it went, so it can leave that way
    /// (`data-swipe-direction`).
    pub swipe_direction: Option<SwipeDirection>,
    /// Its enter and exit: `starting` for its first frame
    /// (`data-starting-style`), `ending` while it leaves
    /// (`data-ending-style`), and how far it shows.
    pub presence: PresenceState,
}

/// An action or close button's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToastButtonState {
    /// Whether work its handler returned is still going on (`data-pending`).
    pub pending: bool,
    /// Why its handler failed, until it's pressed again.
    pub error: Option<ActionError>,
}

/// What a toast's action part takes, beside the toast it's in.
#[derive(Clone, Default)]
pub struct ToastActionProps {
    /// Tells several action parts in one toast apart. The original's adapters
    /// key each part they render.
    pub id: Option<SharedString>,
    /// Runs when it's pressed, instead of the toast's own `action.on_action`.
    /// Work it returns that goes on marks it pending. `details.source` says
    /// how it was pressed, for changes made after the wait.
    pub on_action: Option<ToastActionHandler>,
    /// Whether pressing it dismisses the toast. `None` takes the toast's
    /// `action.dismiss`, else true.
    pub dismiss: Option<bool>,
}

impl ToastActionProps {
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn on_action(
        mut self,
        on_action: impl Fn(
            Option<&InputEvent>,
            &ToastActionDetails,
            &mut Window,
            &mut App,
        ) -> ActionOutcome
        + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(on_action));
        self
    }

    pub fn dismiss(mut self, dismiss: bool) -> Self {
        self.dismiss = Some(dismiss);
        self
    }
}

/// What a toast's close part takes, beside the toast it's in.
#[derive(Clone, Default)]
pub struct ToastCloseProps {
    /// Tells several close parts in one toast apart.
    pub id: Option<SharedString>,
    /// Runs before the toast closes, inside the region's update, since the
    /// region reads what it returns. Work it returns that goes on keeps the
    /// toast open and the button pending until it settles. `details.source`
    /// says how it was pressed, for changes made after the wait.
    pub action: Option<ToastActionHandler>,
}

impl ToastCloseProps {
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn action(
        mut self,
        action: impl Fn(
            Option<&InputEvent>,
            &ToastActionDetails,
            &mut Window,
            &mut App,
        ) -> ActionOutcome
        + 'static,
    ) -> Self {
        self.action = Some(Rc::new(action));
        self
    }

    /// The toast's cancel button: its `on_cancel` runs as the toast closes,
    /// once the region's update is over, as the original's adapters wire a
    /// `ToastCancel` to a close part.
    pub fn cancel(mut self, cancel: &ToastCancel) -> Self {
        self.action = cancel
            .on_cancel
            .clone()
            .map(|on_cancel| -> ToastActionHandler {
                Rc::new(move |event, details, window, cx| {
                    let on_cancel = on_cancel.clone();
                    let event = event.cloned();
                    let details = details.clone();
                    after_update(window, cx, move |window, cx| {
                        on_cancel(event.as_ref(), &details, window, cx)
                    });
                    ActionOutcome::Done
                })
            });
        self
    }
}

/// An action or close button in the region's state: its slot, its toast,
/// and the id that tells several in one toast apart.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ButtonKey {
    pub(crate) slot: &'static str,
    pub(crate) toast: SharedString,
    pub(crate) id: Option<SharedString>,
}

impl ButtonKey {
    pub(crate) fn new(slot: &'static str, toast: SharedString, id: Option<SharedString>) -> Self {
        Self { slot, toast, id }
    }

    /// Its key among its slot's parts, which its element id is made from.
    pub(crate) fn part_key(&self) -> SharedString {
        match &self.id {
            Some(id) => format!("{}-{}", self.toast, id).into(),
            None => self.toast.clone(),
        }
    }
}

/// The stack's measurements as last written, for the parts.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Stack {
    pub(crate) front_height: Option<Pixels>,
    pub(crate) stack_height: Pixels,
    pub(crate) count: usize,
}

/// A region that shows a toaster's toasts. Its entity is its controller.
pub struct ToastRegion {
    pub(crate) instance: Instance,
    // The state (`initialToastState`).
    pub(crate) items: Vec<ToastItem>,
    pub(crate) toaster: Toaster,
    pub(crate) side: ToastSide,
    pub(crate) align: ToastAlign,
    pub(crate) max_visible: usize,
    pub(crate) always_expanded: bool,
    pub(crate) shortcut: Option<SharedString>,
    pub(crate) label: Option<SharedString>,
    pub(crate) hovered: bool,
    pub(crate) focused: bool,
    // Pending and failed actions and close buttons, by part key, which names
    // the toast each is in.
    pub(crate) buttons: HashMap<ButtonKey, ToastButtonState>,
    // What the api keeps.
    pub(crate) pending_announcement: Option<SharedString>,
    pub(crate) placed: bool,
    pub(crate) timing: PresenceTiming,
    pub(crate) drags: DragSession<Self>,
    pub(crate) timers: ToastTimers<Self>,
    // Which update of each toast was announced, and toasts waiting to be.
    pub(crate) announced: HashMap<SharedString, u64>,
    pub(crate) to_announce: HashSet<SharedString>,
    pub(crate) announce_timer: Option<Task<()>>,
    // When the first toast still waiting to be read arrived.
    pub(crate) waiting_since: Option<Instant>,
    // Exit animations running, by toast id.
    pub(crate) exits: HashSet<SharedString>,
    // Heights, measured while a toast is in front or the stack is expanded.
    pub(crate) heights: HashMap<SharedString, Pixels>,
    // Toasts held open by a pending action.
    pub(crate) held: HashSet<SharedString>,
    // Where focus was before it came into the region, to go back to.
    pub(crate) return_target: Option<WeakFocusHandle>,
    pub(crate) entered_by_shortcut: bool,
    pub(crate) announce_pending: AnnouncePending,
    // gpui's side: its focus, the region element as containment sees it,
    // each toast's own, the buttons' focus, the keyboard under it and the
    // stack as last written.
    pub(crate) focus_handle: FocusHandle,
    pub(crate) area: Region,
    pub(crate) entries: HashMap<SharedString, ToastEntry>,
    pub(crate) button_handles: RefCell<HashMap<ButtonKey, FocusHandle>>,
    pub(crate) keyboard: TrackKeyboard,
    pub(crate) stack: Stack,
    // Whether the pointer is out of the window, or hasn't moved in it yet, so
    // a frame drawn meanwhile doesn't take its last position as hovering.
    pub(crate) pointer_out: bool,
    // What the region holds while it's on the page (`bindRegion`).
    pub(crate) binding: Option<RegionBinding>,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl ToastRegion {
    pub fn new(options: ToastRegionOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("toast").with_agent(cx);
        instance.debug = options.debug;
        // The region leaving the page lets go of what it holds.
        flag_while_bound(&instance, "region");
        let this = cx.entity();
        let bound = cx.observe_in(&this, window, |this: &mut Self, _, window, cx| {
            this.check_bound(window, cx)
        });
        let release = cx.on_release(|this, cx| while_destroying(|| this.destroy(cx)));
        let toaster = options.toaster.clone().unwrap_or_else(|| toast(cx));
        let timers = create_toast_timers(
            |this: &mut Self, id: &SharedString, cx: &mut Context<Self>| {
                this.close_toast(id, Reason::Timeout, None, cx)
            },
        );
        let focus_handle = cx.focus_handle();
        let region = Self {
            instance,
            items: Vec::new(),
            toaster,
            side: options.side,
            align: options.align,
            max_visible: options.max_visible,
            always_expanded: options.expanded,
            shortcut: options.shortcut,
            label: options.label,
            hovered: false,
            focused: false,
            buttons: HashMap::new(),
            pending_announcement: options.pending_announcement,
            placed: options.placed,
            timing: options.timing,
            drags: DragSession::new(cx),
            timers,
            announced: HashMap::new(),
            to_announce: HashSet::new(),
            announce_timer: None,
            waiting_since: None,
            exits: HashSet::new(),
            heights: HashMap::new(),
            held: HashSet::new(),
            return_target: None,
            entered_by_shortcut: false,
            announce_pending: AnnouncePending::new(),
            area: Region::with_focus_handle(focus_handle.clone()),
            focus_handle,
            entries: HashMap::new(),
            button_handles: RefCell::new(HashMap::new()),
            keyboard: track_keyboard(None),
            stack: Stack::default(),
            pointer_out: true,
            binding: None,
            _subscriptions: vec![bound, release],
        };
        // Timers run only while the region is on the page. An entity made and
        // never rendered must never run them.
        region.timers.pause(detached(), cx);
        region
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A new toaster or `max_visible` takes effect at once, while
    /// the region is on the page.
    pub fn set_options(
        &mut self,
        options: ToastRegionOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let toaster = options.toaster.clone().unwrap_or_else(|| toast(cx));
        let followed = toaster != self.toaster;
        let refresh = followed || options.max_visible != self.max_visible;
        let changed = refresh
            || self.side != options.side
            || self.align != options.align
            || self.always_expanded != options.expanded
            || self.shortcut != options.shortcut
            || self.label != options.label;
        self.toaster = toaster;
        self.side = options.side;
        self.align = options.align;
        self.max_visible = options.max_visible;
        self.always_expanded = options.expanded;
        self.shortcut = options.shortcut;
        self.label = options.label;
        self.pending_announcement = options.pending_announcement;
        self.instance.debug = options.debug;
        // Transitions that start from now on take it, as CSS does.
        self.timing = options.timing;
        for entry in self.entries.values() {
            entry.presence.set_timing(self.timing.clone());
        }
        if self.binding.is_some() {
            if followed {
                self.follow(window, cx);
            }
            if refresh {
                self.refresh(window, cx);
            }
            self.sync_shortcut(cx);
        }
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The toaster it shows.
    pub fn toaster(&self) -> &Toaster {
        &self.toaster
    }

    /// The toasts to render, newest first, including ones animating out.
    pub fn toasts(&self) -> &[ToastItem] {
        &self.items
    }

    /// Whether the stack is expanded: always, or while the pointer or focus
    /// is in it.
    pub fn is_expanded(&self) -> bool {
        self.always_expanded || self.hovered || self.focused
    }

    pub fn state(&self) -> ToastRegionState {
        ToastRegionState {
            side: self.side,
            align: self.align,
            expanded: self.is_expanded(),
            toasts: self.items.clone(),
            front_height: self.stack.front_height,
            stack_height: self.stack.stack_height,
            count: self.stack.count,
        }
    }

    pub(crate) fn destroy(&mut self, cx: &mut App) {
        if let Some(binding) = self.binding.take() {
            binding.release(cx);
        }
        self.timers.destroy();
        self.announce_timer = None;
        self.entries.clear();
    }
}

// Timers wait while the region is off the page.
pub(crate) fn detached() -> ToastPauseReason {
    ToastPauseReason::Custom("detached".into())
}

/// `agent: { actions: ["pause"], shown }` and `controlled: []`: an agent can
/// hold the toasts while it reads them, with a reason of its own, and sees the
/// region only while it has toasts. The toasts themselves it reads as the
/// parts they are.
impl Agent for ToastRegion {
    const ACTIONS: &'static [AgentAction<Self>] =
        &[AgentAction::new("pause", |region, args, _window, cx| {
            // A pause without a reason is one reason of its own, as `undefined`
            // is in the original's set.
            let reason = if args.is_missing(0) {
                SharedString::default()
            } else {
                args.string(0)?
            };
            region.pause(ToastPauseReason::from_name(&reason), cx);
            Ok(AgentValue::Null)
        })];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&[]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
    }

    fn agent_shown(&self, _cx: &App) -> bool {
        has_toasts(&self.items)
    }
}
