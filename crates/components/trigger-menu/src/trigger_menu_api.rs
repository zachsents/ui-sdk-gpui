//! Port of `components/trigger-menu/trigger-menu-api.ts`: the trigger menu's
//! state and api: what the list shows for the current level, when it opens,
//! what screen readers hear, and the editor it follows. The match and its
//! levels live in `trigger_menu_levels.rs`, and choosing in
//! `trigger_menu_select.rs`.
//!
//! The editor reports what happens through a listener the menu subscribes,
//! which moves dismissed matches along and skips the menu's own writes
//! without reading the menu, since the menu is mid-update while it writes.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Duration;

use futures::FutureExt;
use gpui::{
    App, Context, ElementId, Entity, EntityId, KeyDownEvent, KeystrokeEvent, ScrollHandle, SharedString, Subscription,
    Task, Window,
};
use ui_sdk_kernel::bound_keys::bound;
use ui_sdk_kernel::part::held;
use ui_sdk_kernel::{
    AnnouncePending, Aria, AutoComplete, ChangeDetails, HasPopup, InputEvent, Instance, Reason, change_source,
    flag_while_bound, is_composing, with_source,
};
use ui_sdk_lists::{
    AsyncOptions, AsyncOptionsOptions, AsyncOptionsPage, AsyncOptionsStatus, HighlightOptions, ListFilter, ListFocus,
    ListNode, ListScroller, ListboxEngine, ListboxEngineConfig, create_async_options, create_filtered_listbox,
};
use ui_sdk_native::TextField;
use ui_sdk_platform::{
    Align, Anchor, PositionOptions, Positioner, Region, Side, Target, announce, is_apple, is_rtl, matches_shortcut,
    position,
};
use ui_sdk_popups::{
    AbortSignal, AttachConfig, ContainsFn, CreatePopupOptions, HideRequest, PlacementDefaults, Popup, PopupHost,
    PopupOpenChangeCallback, PopupOpener, PopupOptions, PopupPending, PopupRules, RegionFn, SheetConfig, ShowRequest,
    content_anchor, create_popup, derive_open, placement_options, trigger_menu_preset, unopened,
};
use ui_sdk_text::{
    EditorChange, EditorContent, EditorListener, TextEditorAdapter, TextRange, TextRect, js_trim, text_field_adapter,
};

use crate::trigger_match::{TriggerMatch, TriggerRule};
use crate::trigger_menu::{
    TriggerDefinition, TriggerItemExtra, TriggerItems, TriggerMenuChangeOptions, TriggerMenuContentProps,
    TriggerMenuData, TriggerMenuFilter, TriggerMenuItemProps, TriggerMenuOpenChangeReason, TriggerMenuOptions,
    TriggerMenuQueryChangeCallback, TriggerMenuQueryChangeReason, TriggerMenuSelectCallback,
};
use crate::trigger_menu_levels::{TriggerLevel, TriggerLevels, argument_of, move_dismissed};
use crate::trigger_menu_select::ByKey;

/// The listbox engine the menu's items move through: keys are values.
pub(crate) type TriggerListbox = ListboxEngine<TriggerMenu, SharedString, TriggerItemExtra>;

// One entry of loaded suggestions.
type TriggerNode = ListNode<SharedString, TriggerItemExtra>;

/// What kind of field the menu writes its ARIA to (`field`): a single-line
/// field takes the combobox role; a multi-line one, or an editor of the
/// app's own, stays a textbox, which ARIA forbids `aria-expanded` on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TriggerMenuField {
    Input,
    #[default]
    Multiline,
}

/// What `open` takes.
#[derive(Clone, Debug, Default)]
pub struct TriggerMenuOpenOptions {
    /// The trigger to open, like `"/"`. Defaults to the first trigger.
    pub trigger: Option<SharedString>,
    /// Why it opens. Defaults to `Imperative`.
    pub reason: Option<TriggerMenuOpenChangeReason>,
    pub event: Option<InputEvent>,
    /// Closes the menu, with reason `Imperative`, when it aborts.
    pub signal: Option<AbortSignal>,
}

impl TriggerMenuOpenOptions {
    pub fn trigger(mut self, trigger: impl Into<SharedString>) -> Self {
        self.trigger = Some(trigger.into());
        self
    }

    pub fn reason(mut self, reason: TriggerMenuOpenChangeReason) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn event(mut self, event: impl Into<InputEvent>) -> Self {
        self.event = Some(event.into());
        self
    }

    pub fn signal(mut self, signal: AbortSignal) -> Self {
        self.signal = Some(signal);
        self
    }
}

// The editor an input part built, by the field it's for.
struct OwnEditor {
    field: EntityId,
    adapter: Rc<dyn TextEditorAdapter>,
}

// The editor the menu follows now, and what follows it.
struct Connected {
    editor: Rc<dyn TextEditorAdapter>,
    _subscriptions: Vec<Subscription>,
}

/// A trigger menu. Its entity is its controller.
pub struct TriggerMenu {
    pub(crate) instance: Instance,
    pub(crate) popup: Popup<Self>,
    pub(crate) listbox: TriggerListbox,
    loader: Entity<AsyncOptions<TriggerNode>>,
    // The loader's last items, as data, so the same items stay the same data.
    landed: Option<(Rc<[TriggerNode]>, TriggerMenuData)>,

    pub(crate) triggers: Vec<TriggerDefinition>,
    editor_option: Option<Rc<dyn TextEditorAdapter>>,
    on_query_change: Option<TriggerMenuQueryChangeCallback>,
    pub(crate) on_select: Option<TriggerMenuSelectCallback>,
    pending_announcement: Option<SharedString>,
    status_option: Option<AsyncOptionsStatus>,
    auto_highlight: bool,
    r#loop: bool,
    shortcut: Option<SharedString>,

    // An arrow key moved the highlight since the last edit, so screen
    // readers hear about it.
    pub(crate) navigated: bool,
    pub(crate) trigger: Option<SharedString>,
    pub(crate) query: SharedString,
    pub(crate) argument: Option<SharedString>,
    pub(crate) hint: SharedString,
    pub(crate) items: Option<TriggerMenuData>,
    // A loader's state, unless the `status` option says otherwise.
    loading: AsyncOptionsStatus,
    // What kind of field the menu writes ARIA to.
    pub(crate) field: TriggerMenuField,
    // Rendering an empty part keeps the menu open with no matches.
    has_empty: bool,
    // The content rendered and its items had their frame, so an empty list
    // is really empty.
    pub(crate) settled: Cell<bool>,
    // `on_select` futures that haven't settled.
    pub(crate) pending: usize,

    // The editor an input part built. It wins over the `editor` option.
    own_editor: Option<OwnEditor>,
    connected: Option<Connected>,
    // The query a loader is loading for, so results for anything else never
    // land.
    requested: Option<SharedString>,
    // The level shown last, to tell a new level from a new query.
    shown: Option<TriggerLevel>,
    // The api's writes reach the popup through the match levels, so they
    // read the outcome here.
    moved: bool,
    pub(crate) levels: TriggerLevels,
    pub(crate) writing: Rc<Cell<bool>>,
    pub(crate) by_key: ByKey,
    // Empty parts in the page.
    empties: usize,
    mounts_queued: bool,
    status_timer: Option<Task<()>>,
    // What the status part says, a moment after typing pauses.
    pub(crate) status_text: SharedString,
    // Whether the status part renders that text, having none of its own.
    pub(crate) status_writes: Cell<bool>,
    announce_pending: AnnouncePending,
    // The editor's element, for the popup's trigger and containment.
    pub(crate) editor_region: Region,
    // The trigger `on_open_change` hears, read inside the popup's change.
    open_trigger: Rc<RefCell<Option<SharedString>>>,
    pub(crate) content_props: Rc<RefCell<TriggerMenuContentProps>>,
    // What each item last rendered with (`context.props("item", key)`).
    pub(crate) item_props: RefCell<HashMap<SharedString, TriggerMenuItemProps>>,
    // The element id of an editor of the app's own, which names the list.
    pub(crate) editor_id: RefCell<Option<ElementId>>,
    pub(crate) scroll: ScrollHandle,
    pub(crate) hint_positioner: Positioner,
    _subscriptions: Vec<Subscription>,
}

impl PopupHost for TriggerMenu {
    type Payload = ();

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

// The arguments' hints still to type, like "<name> <effort>".
fn hint_for(level: &TriggerLevel, query: &str) -> SharedString {
    let Some(item) = &level.item else {
        return SharedString::default();
    };
    let list = &item.item.extra.arguments;
    let index = level.starts.len().saturating_sub(1);
    let from = if query.is_empty() { index } else { index + 1 };
    let hints: Vec<SharedString> = list.iter().skip(from).map(|argument| argument.hint_text()).collect();
    hints.join(" ").into()
}

fn same_level(a: Option<&TriggerLevel>, b: &TriggerLevel) -> bool {
    a.is_some_and(|a| {
        a.rule == b.rule
            && match (&a.item, &b.item) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && a.starts.len() == b.starts.len()
            && a.query_start == b.query_start
    })
}

fn same_editor(a: Option<&Rc<dyn TextEditorAdapter>>, b: Option<&Rc<dyn TextEditorAdapter>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::ptr::addr_eq(Rc::as_ptr(a), Rc::as_ptr(b)),
        (None, None) => true,
        _ => false,
    }
}

fn same_items(a: Option<&TriggerMenuData>, b: Option<&TriggerMenuData>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.ptr_eq(b),
        (None, None) => true,
        _ => false,
    }
}

// The popup's options from the menu's: `on_open_change` hears the trigger
// beside the details (`details`).
fn popup_options(options: &TriggerMenuOptions, trigger: &Rc<RefCell<Option<SharedString>>>) -> PopupOptions<()> {
    let on_open_change = options.on_open_change.clone().map(|callback| {
        let trigger = trigger.clone();
        Rc::new(
            move |open: &bool, details: &ChangeDetails, _: Option<&SharedString>, window: &mut Window, cx: &mut App| {
                let trigger = trigger.borrow().clone();
                callback(open, details, trigger.as_ref(), window, cx);
            },
        ) as PopupOpenChangeCallback
    });
    PopupOptions {
        open: options.open,
        default_open: false,
        on_open_change,
        on_open_change_complete: options.on_open_change_complete.clone(),
        dismissible: options.dismissible,
        on_dismiss_attempt: options.on_dismiss_attempt.clone(),
        ..PopupOptions::default()
    }
}

// What only the trigger menu's popup does: it opens from the editor, which
// isn't outside, and focus stays there.
fn popup_rules(
    editor_region: &Region,
    content_props: &Rc<RefCell<TriggerMenuContentProps>>,
    cx: &Context<TriggerMenu>,
) -> PopupRules<TriggerMenu> {
    let trigger: RegionFn = {
        let region = editor_region.clone();
        Rc::new(move || region.focus_handle().is_some().then(|| region.clone()))
    };
    let contains: ContainsFn = {
        let region = editor_region.clone();
        Rc::new(move |target: &Target, _event: Option<&InputEvent>, window: &Window, _: &App| {
            region.contains(target, window)
        })
    };
    let weak = cx.weak_entity();
    let props = content_props.clone();
    let bind = Rc::new(move |_menu: &TriggerMenu| {
        // The trigger's place in the text, read every frame. While the
        // editor loses focus for a moment, the list stays where it was.
        let last: Rc<Cell<Option<TextRect>>> = Rc::default();
        let anchor = {
            let weak = weak.clone();
            let props = props.clone();
            Rc::new(move || {
                let own = props.borrow().anchor.clone();
                let weak = weak.clone();
                let last = last.clone();
                content_anchor(own, move || {
                    Some(Anchor::rect(move |window, cx| {
                        let rect = weak.upgrade().and_then(|menu| menu.read(cx).anchor_rect(window, cx));
                        if rect.is_some() {
                            last.set(rect);
                        }
                        last.get()
                    }))
                })
            }) as Rc<dyn Fn() -> Option<Anchor>>
        };
        let position = {
            let props = props.clone();
            Rc::new(move || {
                placement_options(
                    &props.borrow().popup().position,
                    PlacementDefaults::new(Align::Start),
                    None,
                )
            }) as Rc<dyn Fn() -> PositionOptions>
        };
        SheetConfig {
            attach: AttachConfig {
                anchor: Some(anchor),
                position: Some(position),
                ..AttachConfig::default()
            },
            ..SheetConfig::default()
        }
    });
    PopupRules {
        trigger: Some(trigger),
        // The editor isn't outside: a press in it moves the caret, which
        // decides.
        contains: Some(contains),
        // Without a sheet or a close watcher, it never closes for a swipe or
        // Back.
        dismiss: Some(Rc::new(|menu: &mut TriggerMenu, reason, details, window, cx| {
            menu.end(reason, details.event, reason == Reason::EscapeKey, window, cx);
        })),
        // Focus stays in the editor.
        return_focus: Some(false),
        on_open_change: Some(Rc::new(|menu: &mut TriggerMenu, open, _, _, cx| {
            if !open {
                menu.listbox.set_highlighted(None);
                menu.navigated = false;
                cx.notify();
            }
        })),
        // Closed, the content leaves the page unless it's kept: when it
        // renders again, its items settle again.
        on_open_change_complete: Some(Rc::new(|menu: &mut TriggerMenu, open, _, _, _| {
            if !open && !menu.content_props.borrow().keep_mounted {
                menu.settled.set(false);
            }
        })),
        bind: Some(bind),
        ..PopupRules::default()
    }
}

// The loader the current level's loader runs through, a moment after typing
// pauses.
fn create_loader(cx: &mut Context<TriggerMenu>) -> Entity<AsyncOptions<TriggerNode>> {
    let menu = cx.weak_entity();
    let load = move |query: SharedString, context, cx: &mut App| {
        let loader = menu.upgrade().and_then(|menu| match menu.read(cx).level_items() {
            Some(TriggerItems::Loader(loader)) => Some(loader),
            _ => None,
        });
        match loader {
            Some(loader) => {
                let work = loader(query, context, cx);
                async move { work.await.map(|data| AsyncOptionsPage::from(data.nodes().to_vec())) }.boxed_local()
            }
            None => futures::future::ready(Ok(AsyncOptionsPage::from(Vec::new()))).boxed_local(),
        }
    };
    create_async_options(
        load,
        AsyncOptionsOptions::default().debounce(Duration::from_millis(150)),
        cx,
    )
}

fn listbox_config() -> ListboxEngineConfig<TriggerMenu, SharedString, TriggerItemExtra> {
    ListboxEngineConfig::new(|_: &TriggerMenu, value: &SharedString| value.clone())
        .focus(|_| ListFocus::Virtual)
        .items(|menu: &TriggerMenu| menu.items.clone())
        .filter(|menu: &TriggerMenu| menu.level_filter())
        .query(|menu: &TriggerMenu| menu.query.clone())
        .r#loop(|menu: &TriggerMenu| menu.r#loop)
        .active(|menu: &TriggerMenu| menu.popup.is_open())
        .scroll_root(|menu: &TriggerMenu| Some(ListScroller::new(menu.scroll.clone())))
        .on_register(|menu: &mut TriggerMenu, _, window, cx| menu.registered(window, cx))
}

impl TriggerMenu {
    pub fn new(options: TriggerMenuOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("trigger-menu").with_agent(cx);
        instance.debug = options.debug;
        let editor_region = Region::new();
        let open_trigger: Rc<RefCell<Option<SharedString>>> = Rc::default();
        let content_props: Rc<RefCell<TriggerMenuContentProps>> = Rc::default();
        let popup = create_popup(
            &instance,
            CreatePopupOptions::new(trigger_menu_preset())
                .rules(popup_rules(&editor_region, &content_props, cx))
                .options(popup_options(&options, &open_trigger)),
            window,
            cx,
        );
        let listbox = create_filtered_listbox(listbox_config(), window, cx);
        let loader = create_loader(cx);
        let landing = cx.observe_in(&loader, window, |menu: &mut Self, _, window, cx| {
            menu.loaded(window, cx)
        });
        // Parts whose coming and going decides something: the input, which
        // is the editor, an empty part, which keeps the menu open, and the
        // list, which takes the listbox role from the content.
        for slot in ["input", "empty", "list"] {
            flag_while_bound(&instance, slot);
        }
        let this = cx.entity();
        let parts = cx.observe_in(&this, window, |menu: &mut Self, _, window, cx| {
            menu.parts_changed(window, cx)
        });
        let mut menu = Self {
            instance,
            popup,
            listbox,
            loader,
            landed: None,
            triggers: options.triggers,
            editor_option: options.editor,
            on_query_change: options.on_query_change,
            on_select: options.on_select,
            pending_announcement: options.pending_announcement,
            status_option: options.status,
            auto_highlight: options.auto_highlight,
            r#loop: options.r#loop,
            shortcut: options.shortcut,
            navigated: false,
            trigger: None,
            query: SharedString::default(),
            argument: None,
            hint: SharedString::default(),
            items: None,
            loading: AsyncOptionsStatus::Idle,
            field: TriggerMenuField::Multiline,
            has_empty: false,
            settled: Cell::new(false),
            pending: 0,
            own_editor: None,
            connected: None,
            requested: None,
            shown: None,
            moved: false,
            levels: TriggerLevels::default(),
            writing: Rc::new(Cell::new(false)),
            by_key: RefCell::new(None),
            empties: 0,
            mounts_queued: false,
            status_timer: None,
            status_text: SharedString::default(),
            status_writes: Cell::new(false),
            announce_pending: AnnouncePending::new(),
            editor_region,
            open_trigger,
            content_props,
            item_props: RefCell::new(HashMap::new()),
            editor_id: RefCell::new(None),
            scroll: ScrollHandle::new(),
            hint_positioner: position(None, PositionOptions::default()),
            _subscriptions: vec![parts, landing],
        };
        menu.follow(window, cx);
        menu
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A new `editor` is followed at once.
    pub fn set_options(&mut self, options: TriggerMenuOptions, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.popup.is_open();
        let mut changed = self.popup.set_options(popup_options(&options, &self.open_trigger), cx);
        // A controlled close clears the highlight in the same pass.
        if derive_open(options.open, before, |open| !open).unwrap_or(false) {
            changed |= self.listbox.set_highlighted(None);
            self.navigated = false;
        }
        changed |= self.status_option != options.status
            || self.auto_highlight != options.auto_highlight
            || self.r#loop != options.r#loop;
        self.triggers = options.triggers;
        self.on_query_change = options.on_query_change;
        self.on_select = options.on_select;
        self.pending_announcement = options.pending_announcement;
        self.status_option = options.status;
        self.auto_highlight = options.auto_highlight;
        self.r#loop = options.r#loop;
        self.shortcut = options.shortcut;
        self.instance.debug = options.debug;
        if !same_editor(self.editor_option.as_ref(), options.editor.as_ref()) {
            self.editor_option = options.editor;
            self.follow(window, cx);
        }
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    // The editor an input part built, else the `editor` option.
    pub(crate) fn editor(&self) -> Option<Rc<dyn TextEditorAdapter>> {
        self.own_editor
            .as_ref()
            .map(|own| own.adapter.clone())
            .or_else(|| self.editor_option.clone())
    }

    /// The triggers' rules, as matching reads them.
    pub(crate) fn rules(&self) -> Vec<TriggerRule> {
        self.triggers.iter().map(|trigger| trigger.rule.clone()).collect()
    }

    /// Whether items are loading or failed: the `status` option, else a
    /// loader's.
    pub(crate) fn loading_status(&self) -> AsyncOptionsStatus {
        self.status_option.unwrap_or(self.loading)
    }

    // What the current level lists: an argument's items, else the trigger's.
    pub(crate) fn level_items(&self) -> Option<TriggerItems> {
        let level = self.levels.current.as_ref()?;
        if level.item.is_some() {
            return argument_of(Some(level)).and_then(|argument| argument.items.clone());
        }
        self.triggers.get(level.rule).and_then(|rule| rule.items.clone())
    }

    // A free-text argument shows no list.
    pub(crate) fn is_free_text(&self) -> bool {
        self.levels.current.as_ref().is_some_and(|level| {
            level.item.is_some() && argument_of(Some(level)).is_none_or(|argument| argument.items.is_none())
        })
    }

    // How typing narrows the current level's items. A loader's aren't.
    fn level_filter(&self) -> Option<TriggerMenuFilter> {
        if self.level_items().is_some_and(|items| items.is_loader()) {
            return Some(ListFilter::Off);
        }
        let rule = self
            .levels
            .current
            .as_ref()
            .and_then(|level| self.triggers.get(level.rule));
        Some(
            rule.and_then(|rule| rule.filter.clone())
                .unwrap_or(ListFilter::Contains),
        )
    }

    /// The highlighted item while the list is open.
    pub(crate) fn highlighted_while_open(&self) -> Option<SharedString> {
        if self.popup.is_open() {
            self.listbox.highlighted()
        } else {
            None
        }
    }

    fn show(
        &mut self,
        reason: Reason,
        event: Option<InputEvent>,
        by: PopupOpener,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.popup.is_open() {
            return;
        }
        self.sync_editor_region(cx);
        let control = self.popup.control().clone();
        let request = ShowRequest::new().reason(reason).event(event).by(by);
        self.moved = control.attempt(self, request, None, window, cx).0;
    }

    fn hide(&mut self, reason: Reason, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let control = self.popup.control().clone();
        self.moved = control.hide(self, HideRequest::new().reason(reason).event(event), window, cx);
    }

    // Whether what `write` does opens or closes the popup.
    fn moving(&mut self, write: impl FnOnce(&mut Self)) -> bool {
        self.moved = false;
        write(self);
        self.moved
    }

    fn set_query(
        &mut self,
        query: SharedString,
        reason: TriggerMenuQueryChangeReason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.query == query {
            return;
        }
        self.query = query.clone();
        cx.notify();
        let Some(level) = self.levels.current.as_ref() else {
            return;
        };
        let trigger = level.r#match.trigger.clone();
        let argument = argument_of(Some(level)).map(|argument| argument.name.clone());
        if let Some(callback) = self.on_query_change.clone() {
            let source = change_source(reason, event.as_ref());
            let details = ChangeDetails::new(reason, source, event);
            with_source(source, || {
                callback(&query, &details, &trigger, argument.as_ref(), window, cx)
            });
        }
    }

    // New items, or new item parts: the highlight stays valid as they render
    // and load. Rows can leave after the edit that filtered them.
    fn items_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_open() && !self.visible() {
            return self.settle(Reason::InputChange, None, window, cx);
        }
        self.settle_highlight(window, cx);
    }

    fn set_items(&mut self, items: Option<TriggerMenuData>, window: &mut Window, cx: &mut Context<Self>) {
        if same_items(self.items.as_ref(), items.as_ref()) {
            return;
        }
        self.items = items;
        cx.notify();
        self.items_changed(window, cx);
    }

    // Items joined or left the list: once per frame.
    fn registered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mounts_queued {
            return;
        }
        self.mounts_queued = true;
        cx.defer_in(window, |menu, window, cx| {
            menu.mounts_queued = false;
            menu.items_changed(window, cx);
        });
    }

    fn cancel_loader(&self, cx: &mut Context<Self>) {
        self.loader.update(cx, |loader, cx| loader.cancel(cx));
    }

    // What a level lists. Data shows at once; a loader starts loading,
    // keeping the previous items within the same level.
    fn load_level(&mut self, query: SharedString, level_changed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let data = match self.level_items() {
            Some(TriggerItems::Loader(_)) => {
                if level_changed {
                    self.cancel_loader(cx);
                    self.set_items(Some(TriggerMenuData::empty()), window, cx);
                }
                self.requested = Some(query.clone());
                self.loading = AsyncOptionsStatus::Loading;
                cx.notify();
                self.loader.update(cx, |loader, cx| loader.set_query(query, cx));
                return;
            }
            Some(TriggerItems::Data(data)) => Some(data),
            None => None,
        };
        self.cancel_loader(cx);
        self.requested = None;
        let data = if self.is_free_text() {
            Some(TriggerMenuData::empty())
        } else {
            data
        };
        self.loading = AsyncOptionsStatus::Idle;
        self.set_items(data, window, cx);
    }

    // A load landed, or the loader changed. Results for a level or query
    // that's gone never land.
    fn loaded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.loader.read(cx).get_state();
        if self.levels.current.is_none()
            || Some(&state.query) != self.requested.as_ref()
            || state.status == AsyncOptionsStatus::Loading
        {
            return;
        }
        let (items, status) = (state.items.clone(), state.status);
        let data = match &self.landed {
            Some((landed, data)) if Rc::ptr_eq(landed, &items) => data.clone(),
            _ => {
                let data = TriggerMenuData::new(items.iter().cloned());
                self.landed = Some((items, data.clone()));
                data
            }
        };
        self.loading = status;
        self.set_items(Some(data), window, cx);
        self.settle(Reason::InputChange, None, window, cx);
    }

    /// Shows a level and its query: its state, its items and whether the
    /// list opens (`apply`).
    pub(crate) fn apply_level(
        &mut self,
        level: TriggerLevel,
        query: SharedString,
        reason: TriggerMenuQueryChangeReason,
        open: TriggerMenuOpenChangeReason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let level_changed = !same_level(self.shown.as_ref(), &level);
        self.set_trigger(Some(level.r#match.trigger.clone()));
        self.argument = argument_of(Some(&level)).map(|argument| argument.name.clone());
        self.hint = hint_for(&level, &query);
        self.navigated = false;
        self.shown = Some(level);
        cx.notify();
        self.set_query(query.clone(), reason, event.clone(), window, cx);
        if level_changed || self.level_items().is_some_and(|items| items.is_loader()) {
            self.load_level(query, level_changed, window, cx);
        }
        self.settle(open, event, window, cx);
    }

    /// Ends the match and closes the list (`close`).
    pub(crate) fn close_level(
        &mut self,
        reason: TriggerMenuOpenChangeReason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shown = None;
        self.requested = None;
        self.cancel_loader(cx);
        self.hide(reason, event, window, cx);
        // The query empties without `on_query_change`, which reports only
        // while a level is current, and the level has ended.
        self.set_trigger(None);
        self.argument = None;
        self.hint = SharedString::default();
        self.navigated = false;
        self.loading = AsyncOptionsStatus::Idle;
        self.query = SharedString::default();
        cx.notify();
        self.announce_state(window, cx);
    }

    fn set_trigger(&mut self, trigger: Option<SharedString>) {
        *self.open_trigger.borrow_mut() = trigger.clone();
        self.trigger = trigger;
    }

    // Whether the list has anything to show. Item parts that never rendered
    // are unknown, so they count as there.
    fn has_entries(&self) -> bool {
        let entries = self.listbox.entries(self);
        if self.items.is_none() && entries.is_empty() && !self.settled.get() {
            return true;
        }
        !entries.is_empty()
    }

    // The list for the current level shows while items match, while they
    // load, or while an empty part can say there are none.
    pub(crate) fn visible(&self) -> bool {
        self.levels.current.is_some()
            && !self.is_free_text()
            && (self.has_entries() || self.loading_status() == AsyncOptionsStatus::Loading || self.empties > 0)
    }

    pub(crate) fn settle(
        &mut self,
        reason: TriggerMenuOpenChangeReason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.visible() {
            self.show(reason, event, PopupOpener::Keyboard, window, cx);
        } else {
            self.hide(reason, event, window, cx);
        }
        self.settle_highlight(window, cx);
        self.announce_state(window, cx);
    }

    // The first match is highlighted with `auto_highlight`, and a highlight
    // whose item left goes.
    fn settle_highlight(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = self.listbox.entries(self);
        let current = self.listbox.highlighted();
        let present = current
            .as_ref()
            .is_some_and(|key| list.iter().any(|entry| &entry.key == key));
        let listbox = self.listbox.clone();
        if self.auto_highlight && !list.is_empty() {
            if present && self.navigated {
                return;
            }
            let first = list
                .iter()
                .find(|entry| !listbox.disabled(self, entry))
                .unwrap_or(&list[0]);
            if Some(&first.key) != current.as_ref() {
                listbox.highlight(self, Some(first.key.clone()), HighlightOptions::default(), window, cx);
            }
            return;
        }
        if current.is_some() && !present {
            listbox.highlight(self, None, HighlightOptions::default(), window, cx);
        }
    }

    // What screen readers hear now: the count and the keys, loading, nothing
    // matching, or the argument being typed.
    pub(crate) fn status_message(&self) -> SharedString {
        let open = self.popup.is_open();
        let current = self.levels.current.as_ref();
        if !open && current.is_none() {
            return SharedString::default();
        }
        if self.loading_status() == AsyncOptionsStatus::Loading {
            return "Loading".into();
        }
        if !open {
            return match &self.argument {
                Some(argument) => js_trim(&format!("{argument}. {}", self.hint)).to_string().into(),
                None => SharedString::default(),
            };
        }
        let count = self.listbox.entries(self).len();
        if count == 0 {
            return "No matches".into();
        }
        let next = if current.is_some_and(|level| level.item.is_some()) {
            ", Tab for the next"
        } else {
            ""
        };
        let noun = if count == 1 { "suggestion" } else { "suggestions" };
        format!("{count} {noun}. Up and Down to choose, Enter to insert{next}").into()
    }

    /// Says the status through a status part without content of its own,
    /// once typing pauses, or else through the window's shared announcer.
    pub(crate) fn announce_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.status_message();
        if !held(&self.instance, "status", None) {
            return announce(&message, window, cx);
        }
        if !self.status_writes.get() {
            return;
        }
        self.status_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(350)).await;
            this.update(cx, |menu, cx| {
                // The same message again is changed a little, so it's said
                // again.
                menu.status_text = if !message.is_empty() && menu.status_text == message {
                    format!("{message}\u{a0}").into()
                } else {
                    message
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// The pending count changed: a slow `on_select` says so after a second.
    pub(crate) fn pending_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.pending_announcement.clone();
        self.announce_pending.update(
            self.pending > 0,
            text,
            |text, window, cx| announce(text, window, cx),
            window,
            cx,
        );
        cx.notify();
    }

    /// Keys on the editor while a match is active: the shortcut that reopens
    /// a dismissed match, then the menu's keys.
    pub(crate) fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if window.default_prevented() || is_composing(window, cx) {
            return;
        }
        let shortcut = self.shortcut.clone().unwrap_or_else(|| "ctrl+space".into());
        if !shortcut.is_empty() && matches_shortcut(&event.keystroke, &shortcut, is_apple(cx)) {
            if self.reopen(Reason::Shortcut, Some(InputEvent::from(event)), window, cx) {
                window.prevent_default();
            }
            return;
        }
        self.keyboard_keydown(event, window, cx);
    }

    // The input part's editor or the `editor` option, now.
    fn follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let own_keys = self.own_editor.is_some();
        self.connect(self.editor(), own_keys, window, cx);
    }

    fn disconnect(&mut self) {
        self.connected = None;
        self.editor_region.set_focus_handle(None);
    }

    // Follows an editor: its edits and caret moves, and for editors without
    // a part of their own, its keys.
    fn connect(
        &mut self,
        next: Option<Rc<dyn TextEditorAdapter>>,
        own_keys: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if same_editor(
            self.connected.as_ref().map(|connected| &connected.editor),
            next.as_ref(),
        ) {
            return;
        }
        self.disconnect();
        let Some(next) = next else {
            return;
        };
        let mut subscriptions = vec![next.subscribe(self.editor_listener(cx), cx)];
        if !own_keys {
            subscriptions.push(intercept_keys(&next, cx));
        }
        self.connected = Some(Connected {
            editor: next,
            _subscriptions: subscriptions,
        });
        self.sync_editor_region(cx);
    }

    // The editor's focus handle, once it has one, for the popup's trigger
    // and containment.
    fn sync_editor_region(&self, cx: &App) {
        let handle = self.editor().and_then(|editor| editor.focus_handle(cx));
        self.editor_region.set_focus_handle(handle);
    }

    // What the editor reports. Its edits move the dismissed matches, the
    // menu's own too, which come while the menu writes, mid-update: those
    // stop there.
    fn editor_listener(&self, cx: &Context<Self>) -> EditorListener {
        let entity = cx.weak_entity();
        let dismissed = self.levels.dismissed.clone();
        let writing = self.writing.clone();
        Rc::new(move |change: &EditorChange, window: &mut Window, cx: &mut App| {
            if let EditorChange::Input { range, inserted, .. } = change {
                move_dismissed(&dismissed, *range, inserted.len());
            }
            if writing.get() {
                return;
            }
            entity
                .update(cx, |menu, cx| menu.on_editor_change(change, window, cx))
                .ok();
        })
    }

    /// The input part's field is the menu's own editor, or none once the
    /// part leaves (`setOwnEditor`).
    pub(crate) fn set_own_editor(
        &mut self,
        field: Option<&Entity<TextField>>,
        kind: TriggerMenuField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.own_editor = field.map(|field| OwnEditor {
            field: field.entity_id(),
            adapter: Rc::new(text_field_adapter(field.read(cx).handle(), cx)),
        });
        self.field = kind;
        self.follow(window, cx);
        cx.notify();
    }

    /// Whether the input part's field is the menu's own editor now.
    pub(crate) fn owns_field(&self, field: &Entity<TextField>) -> bool {
        self.own_editor
            .as_ref()
            .is_some_and(|own| own.field == field.entity_id())
    }

    // Parts came or went: the input takes its editor with it, and an empty
    // part counts while it's in the page, so no matches keeps the menu open.
    fn parts_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.own_editor.is_some() && !bound(&self.instance, "input") {
            self.set_own_editor(None, TriggerMenuField::Multiline, window, cx);
        }
        self.empties = usize::from(bound(&self.instance, "empty"));
        let has_empty = self.empties > 0;
        if self.has_empty == has_empty {
            return;
        }
        self.has_empty = has_empty;
        if !has_empty && self.popup.is_open() {
            self.settle(Reason::InputChange, None, window, cx);
        }
    }

    /// Whether the content holds a list part, which is the listbox then.
    pub(crate) fn listed(&self) -> bool {
        flag_while_bound(&self.instance, "list") == Some(true)
    }

    // The item the field's active descendant points at, after an arrow key.
    fn active_descendant(&self) -> Option<ElementId> {
        let highlighted = self.listbox.highlighted()?;
        if !self.popup.is_open() || !self.navigated || !self.listbox.is_connected(&highlighted) {
            return None;
        }
        Some(self.instance.element_id("item", Some(&highlighted)))
    }

    /// The field's ARIA, and its `aria-expanded` when it's a combobox. A
    /// single-line field is a combobox; a multi-line one or an editor of the
    /// app's own stays a textbox, which ARIA forbids `aria-expanded` on.
    pub(crate) fn field_aria(&self, field: TriggerMenuField) -> (Aria, Option<bool>) {
        let open = self.popup.is_open();
        let mut aria = Aria::new()
            .autocomplete(AutoComplete::List)
            .has_popup(HasPopup::Listbox)
            .active_descendant(self.active_descendant());
        if open {
            let target = if self.listed() { "list" } else { "content" };
            aria = aria.controls(self.instance.element_id(target, None));
        }
        (aria, (field == TriggerMenuField::Input).then_some(open))
    }

    /// The list is named like the field: by its own label, through the
    /// input part or the app's editor. Not by the field's text.
    pub(crate) fn list_label(&self) -> Aria {
        let mut aria = Aria::new().labelled_by(self.instance.element_id("input", None));
        if let Some(id) = self.editor_id.borrow().clone() {
            aria = aria.labelled_by(id);
        }
        aria
    }

    /// The list opens at the trigger, or at an argument's start, and stays
    /// there as the query grows.
    pub(crate) fn anchor_rect(&self, window: &Window, cx: &App) -> Option<TextRect> {
        let level = self.levels.current.as_ref()?;
        let offset = if level.item.is_some() || level.continued {
            level.query_start
        } else {
            level.r#match.start
        };
        self.editor()?.rect_at(offset, window, cx)
    }

    /// The caret's rectangle, where the hint goes.
    pub(crate) fn caret_rect(&self, window: &Window, cx: &App) -> Option<TextRect> {
        let at = self.caret(window, cx)?;
        self.editor()?.rect_at(at, window, cx)
    }

    /// Where the hint goes beside the caret: after it, which is its left in
    /// right-to-left text.
    pub(crate) fn hint_options(cx: &App) -> PositionOptions {
        let side = if is_rtl(cx) { Side::Left } else { Side::Right };
        PositionOptions::default()
            .side(side)
            .align(Align::Center)
            .side_offset(gpui::px(2.))
            .collision_padding(gpui::px(0.))
    }

    /// Whether no item matches, once the items are known (`isEmpty`).
    pub(crate) fn is_empty(&self) -> bool {
        self.loading_status() != AsyncOptionsStatus::Loading
            && (self.items.is_some() || self.settled.get())
            && self.listbox.entries(self).is_empty()
    }

    /// How many items match.
    pub(crate) fn item_count(&self) -> usize {
        self.listbox.entries(self).len()
    }

    /// The content rendered, and its items with it: once that frame is
    /// drawn, an empty list is really empty.
    pub(crate) fn content_settled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settled.replace(true) {
            return;
        }
        self.settle(Reason::InputChange, None, window, cx);
    }

    /// Whether the menu is open.
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// Opens or closes the menu. Opening needs a match; see [`open`](Self::open)
    /// for one that types the trigger. Returns false when the change was
    /// canceled or it was already open or closed.
    pub fn set_open(
        &mut self,
        open: bool,
        options: TriggerMenuChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        let event = options.event;
        if !open {
            return self.moving(|menu| menu.end(reason, event, false, window, cx));
        }
        // A match that `close` ended is still in the text, so it opens again.
        self.moving(|menu| {
            if menu.levels.current.is_some() {
                menu.show(reason, event, PopupOpener::Imperative, window, cx);
            } else {
                menu.reopen(reason, event, window, cx);
            }
        })
    }

    /// Opens the menu for a trigger, typing it at the caret first unless a
    /// match for it is already there, and resolves once it closes. Resolves
    /// exactly once and never fails.
    pub fn open(
        &mut self,
        options: TriggerMenuOpenOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupPending {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        let event = options.event;
        let target = self.editor();
        let char = options
            .trigger
            .or_else(|| self.triggers.first().map(|trigger| trigger.rule.char.clone()))
            .filter(|char| !char.is_empty());
        let aborted = options.signal.as_ref().is_some_and(AbortSignal::is_aborted);
        let (Some(target), Some(char), false) = (target, char, aborted) else {
            return unopened(reason, event.as_ref());
        };
        if self.caret(window, cx).is_none()
            && let Some(handle) = target.focus_handle(cx)
        {
            window.focus(&handle, cx);
        }
        // Types the trigger at the caret, unless a match for it is there
        // already.
        let there = self
            .match_at_caret(window, cx)
            .is_some_and(|found| found.r#match.trigger == char);
        if !there {
            let Some(at) = self.caret(window, cx) else {
                return unopened(reason, event.as_ref());
            };
            self.write(TextRange::caret(at), char, None, window, cx);
        }
        if !self.reopen(reason, event.clone(), window, cx) {
            return unopened(reason, event.as_ref());
        }
        self.popup.control().result(options.signal)
    }

    /// Closes the menu. The match isn't dismissed, so typing in it opens it
    /// again. Returns false when the close was canceled or it was already
    /// closed.
    pub fn close(&mut self, options: TriggerMenuChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        self.moving(|menu| menu.end(reason, options.event, false, window, cx))
    }

    /// The text after the trigger, or after the current argument's start.
    /// Empty while closed (`getQuery`).
    pub fn get_query(&self) -> SharedString {
        self.query.clone()
    }

    /// The current match, if any (`getMatch`).
    pub fn get_match(&self, window: &Window, cx: &App) -> Option<TriggerMatch> {
        let level = self.levels.current.as_ref()?;
        Some(TriggerMatch {
            end: self.caret(window, cx).unwrap_or(level.r#match.end),
            query: self.query.clone(),
            ..level.r#match.clone()
        })
    }

    /// Replaces the current match with text or a token, or inserts at the
    /// caret without one.
    pub fn insert(&mut self, content: impl Into<EditorContent>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.caret(window, cx) else {
            return;
        };
        let level = self.levels.current.as_ref().map(|level| level.r#match.start);
        self.write(TextRange::new(level.unwrap_or(at), at), content, None, window, cx);
        if level.is_some() {
            self.end(Reason::ItemPress, None, false, window, cx);
        }
    }

    /// Moves focus to the editor.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(handle) = self.editor().and_then(|editor| editor.focus_handle(cx)) {
            window.focus(&handle, cx);
        }
    }

    /// Whether an `on_select` future is pending.
    pub fn is_pending(&self) -> bool {
        self.pending > 0
    }

    /// The trigger of the current match, like `"@"`, or none.
    pub fn trigger(&self) -> Option<&SharedString> {
        self.trigger.as_ref()
    }

    /// The argument being typed, by name, or none.
    pub fn argument(&self) -> Option<&SharedString> {
        self.argument.as_ref()
    }

    /// The highlighted item's key.
    pub fn highlighted(&self) -> Option<SharedString> {
        self.listbox.highlighted()
    }
}

// Keys for an editor without a part of its own, before anything in the
// window hears them, as the original's capture-phase listener on the
// document does: an open menu's keys win over the editor's own, like Enter
// submitting a prompt.
fn intercept_keys(editor: &Rc<dyn TextEditorAdapter>, cx: &mut Context<TriggerMenu>) -> Subscription {
    let entity = cx.weak_entity();
    let editor: Weak<dyn TextEditorAdapter> = Rc::downgrade(editor);
    cx.intercept_keystrokes(move |event: &KeystrokeEvent, window: &mut Window, cx: &mut App| {
        let Some(editor) = editor.upgrade() else {
            return;
        };
        let inside = editor
            .focus_handle(cx)
            .is_some_and(|handle| handle.contains_focused(window, cx));
        if !inside {
            return;
        }
        let down = KeyDownEvent {
            keystroke: event.keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        };
        entity.update(cx, |menu, cx| menu.on_key_down(&down, window, cx)).ok();
    })
}
