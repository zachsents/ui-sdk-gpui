//! Port of `components/accordion/accordion.ts`.
//!
//! A stack of items, each a heading with a trigger and the panel it opens:
//! a group of disclosures, so it builds on the disclosure crate's [`Panel`]
//! as the collapsible does, one per item. Each panel stays in the page while
//! it animates closed, and its part exposes the phases and the size it
//! animates through.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, Context, Entity, SharedString, StatefulInteractiveElement, Styled, Window};
use ui_sdk_disclosure::{
    Opening, Panel, PanelBinding, PanelState, PendingReason, create_pending_reason,
    focus_pressed_trigger, hidden_attrs,
};
use ui_sdk_forms::{Node, is_disabled, watch_form_disabled};
use ui_sdk_kernel::dev::{self, Origin};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentSurface, Aria, ChangeCallback, ChangeDetails,
    ChangeOptions, Controllable, Instance, Part, Parts, Reason, change_source, held,
};
use ui_sdk_motion::PresenceTiming;

/// Why items opened or closed: a trigger was pressed (`TriggerPress`), find
/// in page matched text in a closed panel (`FindInPage`), or a controller or
/// api call changed them (`Imperative`).
pub type AccordionValueChangeReason = Reason;

/// Options for every write method.
pub type AccordionChangeOptions = ChangeOptions;

/// `on_open_change_complete`: an item's panel finished opening or closing,
/// with whether it's open.
pub type OpenChangeCompleteCallback = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// The options `Accordion` takes.
#[derive(Clone)]
pub struct AccordionOptions {
    /// The open items' values. Pass it to control the accordion.
    pub value: Option<Vec<SharedString>>,
    /// The items open at first, when uncontrolled.
    pub default_value: Vec<SharedString>,
    /// Runs when items want to open or close. `details.reason` is
    /// `TriggerPress`, `FindInPage` or `Imperative`. It runs once the
    /// accordion's update is over, so it may update any entity, the accordion
    /// too.
    pub on_value_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Runs as items are asked to open or close, before they do, inside the
    /// accordion's update and after every item's `on_open_change`:
    /// `details.cancel()` stops it. It may update any entity but the
    /// accordion.
    pub on_before_value_change: Option<ChangeCallback<Vec<SharedString>>>,
    /// Whether several items can be open at once. Otherwise opening one
    /// closes the others.
    pub multiple: bool,
    /// Whether the open item can close when only one may be open. With
    /// `false`, one item always stays open and its trigger reports it can't
    /// be pressed. Defaults to `true`.
    pub collapsible: bool,
    /// Stops every item from opening or closing. Triggers stay focusable.
    pub disabled: bool,
    /// Whether closed panels stay in the page, hidden.
    pub keep_mounted: bool,
    /// Keeps closed panels in the page, hidden, where the original keeps them
    /// as `hidden="until-found"` for the browser's find in page. gpui has
    /// none, so it keeps them as `keep_mounted` does.
    pub hidden_until_found: bool,
    /// How panels animate open and closed, as their CSS transition would
    /// declare. The default has none, so they show and hide at once.
    pub panel_timing: PresenceTiming,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for AccordionOptions {
    fn default() -> Self {
        Self {
            value: None,
            default_value: Vec::new(),
            on_value_change: None,
            on_before_value_change: None,
            multiple: false,
            collapsible: true,
            disabled: false,
            keep_mounted: false,
            hidden_until_found: false,
            panel_timing: PresenceTiming::default(),
            debug: false,
        }
    }
}

impl AccordionOptions {
    pub fn value(mut self, value: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.value = Some(value.into_iter().map(Into::into).collect());
        self
    }

    pub fn default_value(mut self, value: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.default_value = value.into_iter().map(Into::into).collect();
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&Vec<SharedString>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    pub fn collapsible(mut self, collapsible: bool) -> Self {
        self.collapsible = collapsible;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn hidden_until_found(mut self, hidden_until_found: bool) -> Self {
        self.hidden_until_found = hidden_until_found;
        self
    }

    pub fn panel_timing(mut self, timing: PresenceTiming) -> Self {
        self.panel_timing = timing;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `item` part takes, beside its value, which identifies the
/// item: the accordion's value lists the open items' values.
#[derive(Clone, Default)]
pub struct AccordionItemProps {
    /// Stops the item from opening or closing. Its trigger stays focusable.
    pub disabled: bool,
    /// Runs before the item opens or closes, before the accordion's
    /// `on_value_change`. `details.cancel()` stops the whole change. It runs
    /// inside the accordion's update.
    pub on_open_change: Option<ChangeCallback<bool>>,
    /// Runs once the item's panel has finished opening or closing, after any
    /// animation.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
}

impl AccordionItemProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }
}

/// The props the `trigger` part takes, beside its item's value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccordionTriggerProps {
    /// Stops this trigger from opening or closing the item. It stays
    /// focusable.
    pub disabled: bool,
}

impl AccordionTriggerProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The props the `panel` part takes, beside its item's value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccordionPanelProps {
    /// Whether the panel stays in the page, hidden, while closed. Overrides
    /// the accordion's.
    pub keep_mounted: Option<bool>,
    /// Whether the panel stays in the page, hidden, as for find in page.
    /// Overrides the accordion's.
    pub hidden_until_found: Option<bool>,
}

impl AccordionPanelProps {
    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = Some(keep_mounted);
        self
    }

    pub fn hidden_until_found(mut self, hidden_until_found: bool) -> Self {
        self.hidden_until_found = Some(hidden_until_found);
        self
    }
}

/// The state the `root` part hands `with`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccordionState {
    /// The open items' values.
    pub value: Vec<SharedString>,
    /// Whether the whole accordion is disabled, by its `disabled` option or a
    /// disabled fieldset around it.
    pub disabled: bool,
    /// Whether several items can be open at once.
    pub multiple: bool,
}

/// The state the `item` part hands `with`, which its header, trigger and
/// panel share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccordionItemState {
    /// Whether the accordion's value holds this item's value.
    pub open: bool,
    /// Whether the item can't open or close.
    pub disabled: bool,
    /// The item's position among the accordion's items, from 0. `None` until
    /// it's in the page (-1 in the original).
    pub index: Option<usize>,
}

/// The state the `header` part hands `with`.
pub type AccordionHeaderState = AccordionItemState;

/// The state the `trigger` part hands `with`.
pub type AccordionTriggerState = AccordionItemState;

/// The state the `panel` part hands `with`: its item's, with the panel's
/// presence and the size it animates through, which the original's
/// `data-starting-style`, `data-ending-style` and `--accordion-panel-height`
/// carry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccordionPanelState {
    /// Whether the accordion's value holds this item's value.
    pub open: bool,
    /// Whether the item can't open or close.
    pub disabled: bool,
    /// The item's position among the accordion's items, from 0.
    pub index: Option<usize>,
    /// The panel's presence, and the height and width to give it now.
    pub panel: PanelState,
}

/// The value after opening or closing one item.
fn next_value(value: &[SharedString], key: &SharedString, open: bool, multiple: bool) -> Vec<SharedString> {
    if !open {
        return value.iter().filter(|item| *item != key).cloned().collect();
    }
    if value.contains(key) {
        return value.to_vec();
    }
    if multiple {
        let mut next = value.to_vec();
        next.push(key.clone());
        next
    } else {
        vec![key.clone()]
    }
}

/// An accordion. Its entity is its controller.
pub struct Accordion {
    instance: Instance,
    value: Controllable<Vec<SharedString>>,
    // Options that change rendering.
    multiple: bool,
    collapsible: bool,
    disabled: bool,
    keep_mounted: bool,
    hidden_until_found: bool,
    // The root's place in the page, which says whether a fieldset disables it.
    form: Node,
    // Panels animating out. They stay shown until their animation ends.
    exiting: Vec<SharedString>,
    // Items in the page, in order, with the props each rendered with.
    items: Parts<SharedString, AccordionItemProps>,
    // Triggers in the page, in order, with their focus handles.
    triggers: Parts<SharedString, ()>,
    // The props each panel rendered with last, which decide whether it stays
    // in the page.
    panel_props: RefCell<HashMap<SharedString, AccordionPanelProps>>,
    panels: HashMap<SharedString, Panel>,
    panel_timing: PresenceTiming,
    // Items opened after mount, with the reason they opened. Their panels
    // animate in; panels open on the first paint don't, and find in page
    // shows at once.
    opened: HashMap<SharedString, Reason>,
    reason: PendingReason<Reason>,
    // The value the last change saw.
    previous: Vec<SharedString>,
}

impl Accordion {
    pub fn new(options: AccordionOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut value = Controllable::new("value", options.default_value.clone()).after_update();
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());
        let mut instance = Instance::new("accordion").with_agent(cx);
        instance.debug = options.debug;
        let previous = value.get().clone();
        let mut this = Self {
            instance,
            value,
            multiple: options.multiple,
            collapsible: options.collapsible,
            disabled: options.disabled,
            keep_mounted: options.keep_mounted,
            hidden_until_found: options.hidden_until_found,
            form: watch_form_disabled(cx),
            exiting: Vec::new(),
            items: Parts::new(),
            triggers: Parts::new(),
            panel_props: RefCell::new(HashMap::new()),
            panels: HashMap::new(),
            panel_timing: options.panel_timing,
            opened: HashMap::new(),
            reason: create_pending_reason(Reason::Imperative),
            previous,
        };
        this.sync_panels(window, cx);
        this
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A closing `value` starts its panels' exits in the same pass.
    pub fn set_options(&mut self, options: AccordionOptions, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = self.value.set_controlled(options.value);
        self.value.set_on_change(options.on_value_change);
        self.value.set_on_before_change(options.on_before_value_change);
        changed |= self.multiple != options.multiple
            || self.collapsible != options.collapsible
            || self.disabled != options.disabled
            || self.keep_mounted != options.keep_mounted
            || self.hidden_until_found != options.hidden_until_found;
        self.multiple = options.multiple;
        self.collapsible = options.collapsible;
        self.disabled = options.disabled;
        self.keep_mounted = options.keep_mounted;
        self.hidden_until_found = options.hidden_until_found;
        for panel in self.panels.values() {
            panel.set_timing(options.panel_timing.clone());
        }
        self.panel_timing = options.panel_timing;
        self.instance.debug = options.debug;
        self.after_change(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled `value`, or `None` to let the accordion keep its
    /// own.
    pub fn set_controlled_value(
        &mut self,
        value: Option<Vec<SharedString>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.value.set_controlled(value) {
            cx.notify();
        }
        self.after_change(window, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// What the root part's state holds.
    pub fn state(&self) -> AccordionState {
        AccordionState {
            value: self.value.get().clone(),
            disabled: self.is_disabled(),
            multiple: self.multiple,
        }
    }

    /// Whether the whole accordion is disabled: by its `disabled` option, or
    /// a disabled fieldset around it.
    pub fn is_disabled(&self) -> bool {
        is_disabled(self.disabled, self.form.form_disabled())
    }

    /// The open items' values.
    pub fn get_value(&self) -> &[SharedString] {
        self.value.get()
    }

    /// Whether an item is open.
    pub fn is_open(&self, value: &str) -> bool {
        self.value.get().iter().any(|item| item.as_ref() == value)
    }

    /// The items in the page, in order, as of their last render.
    pub fn items(&self) -> Vec<SharedString> {
        self.items.keys()
    }

    fn item_props(&self, key: &SharedString) -> Option<AccordionItemProps> {
        self.items.info(key)
    }

    fn item_disabled(&self, key: &SharedString) -> bool {
        self.is_disabled() || self.item_props(key).is_some_and(|props| props.disabled)
    }

    fn disabled_of(&self, key: &SharedString, own: bool) -> bool {
        own || self.is_disabled() || self.item_disabled(key)
    }

    // Whether the open item must stay open: only one may be open and it can't
    // collapse.
    fn locked(&self, key: &SharedString) -> bool {
        !self.multiple && !self.collapsible && self.value.get().contains(key)
    }

    // The state every part of an item shares.
    fn item_state(&self, key: &SharedString, own: bool) -> AccordionItemState {
        AccordionItemState {
            open: self.value.get().contains(key),
            disabled: self.disabled_of(key, own),
            index: self.items.index_of(key),
        }
    }

    fn keeps_mounted(&self, props: &AccordionPanelProps) -> bool {
        props.keep_mounted.unwrap_or(self.keep_mounted)
    }

    fn findable(&self, props: &AccordionPanelProps) -> bool {
        props.hidden_until_found.unwrap_or(self.hidden_until_found)
    }

    // Whether a panel is in the page: open, animating out, or kept.
    fn panel_present(&self, key: &SharedString, props: &AccordionPanelProps) -> bool {
        self.value.get().contains(key)
            || self.exiting.contains(key)
            || self.keeps_mounted(props)
            || self.findable(props)
    }

    /// Sets the open items. Only the first counts unless `multiple` is set.
    /// Every item's `on_open_change` hears first, including the item that
    /// would close when another opens, and any of them can stop the change.
    /// Returns false when canceled or when nothing changed.
    pub fn set_value(
        &mut self,
        value: Vec<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let current = self.value.get().clone();
        let next: Vec<SharedString> = if self.multiple {
            value
        } else {
            value.into_iter().take(1).collect()
        };
        let why = options.reason.unwrap_or(Reason::Imperative);
        let mut keys = current.clone();
        for key in &next {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        for key in keys {
            let open = next.contains(&key);
            if open == current.contains(&key) {
                continue;
            }
            let Some(on_open_change) = self.item_props(&key).and_then(|props| props.on_open_change) else {
                continue;
            };
            let source = options
                .source
                .unwrap_or_else(|| change_source(why, options.event.as_ref()));
            let details = ChangeDetails::new(why, source, options.event.clone());
            on_open_change(&open, &details, window, cx);
            if details.canceled() {
                return false;
            }
        }
        self.reason.set(why);
        let changed = self.value.change(&self.instance, next, options, window, cx);
        if !changed {
            self.reason.take();
        }
        self.after_change(window, cx);
        changed
    }

    fn set_open(&mut self, key: SharedString, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.value.get().contains(&key) == open {
            return false;
        }
        if !open && self.locked(&key) {
            return false;
        }
        let next = next_value(self.value.get(), &key, open, self.multiple);
        self.set_value(next, options, window, cx)
    }

    /// Opens an item. With one item open at a time, closes the others.
    pub fn open(&mut self, value: impl Into<SharedString>, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.set_open(value.into(), true, options, window, cx)
    }

    /// Closes an item, unless it's the one item that must stay open.
    pub fn close(&mut self, value: impl Into<SharedString>, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.set_open(value.into(), false, options, window, cx)
    }

    /// Opens a closed item, or closes an open one.
    pub fn toggle(&mut self, value: impl Into<SharedString>, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let value = value.into();
        let open = !self.value.get().contains(&value);
        self.set_open(value, open, options, window, cx)
    }

    /// Opens every enabled item. Only when `multiple` is set.
    pub fn open_all(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.multiple {
            dev::warn(
                "`open_all` needs an accordion with `multiple`.",
                Some(Origin::new(self.instance.label(), self.instance.id())),
            );
            return false;
        }
        let mut next = self.value.get().clone();
        for key in self.items.keys() {
            if !self.item_disabled(&key) && !next.contains(&key) {
                next.push(key);
            }
        }
        self.set_value(next, options, window, cx)
    }

    /// Closes every item that may close.
    pub fn close_all(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let next = self
            .value
            .get()
            .iter()
            .filter(|key| self.locked(key) || self.item_disabled(key))
            .cloned()
            .collect();
        self.set_value(next, options, window, cx)
    }

    /// Moves focus to an item's trigger, or with `None` the first trigger.
    pub fn focus(&self, value: Option<&str>, window: &mut Window, cx: &mut App) {
        let key = match value {
            Some(value) => Some(SharedString::from(value.to_string())),
            None => self.triggers.keys().into_iter().next(),
        };
        if let Some(handle) = key.and_then(|key| self.triggers.existing_focus_handle(&key)) {
            window.focus(&handle, cx);
        }
    }

    // An open panel animates in only when it opened after mount. Find in page
    // shows its match at once.
    fn opening(&self, key: &SharedString) -> Opening {
        match self.opened.get(key) {
            None => Opening::Mount,
            Some(Reason::FindInPage) => Opening::Instant,
            Some(_) => Opening::Animate,
        }
    }

    // The panels animating out once `value` replaces `from`: the ones it
    // closes, and the ones already leaving that it doesn't open again.
    fn exiting_after(&self, from: &[SharedString], value: &[SharedString]) -> Vec<SharedString> {
        let closed = from.iter().filter(|key| {
            !value.contains(key) && held(&self.instance, "panel", Some(key)) && !self.exiting.contains(key)
        });
        self.exiting
            .iter()
            .filter(|key| !value.contains(key))
            .chain(closed)
            .cloned()
            .collect()
    }

    // What the original's store subscription does: hears `value` change from
    // any source, a press, find in page, the api or a controlled value,
    // before anything renders the change.
    fn after_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.value.get().clone();
        if value != self.previous {
            let from = std::mem::replace(&mut self.previous, value.clone());
            // A later value from options, like a controlled "Expand all",
            // isn't this change.
            let why = self.reason.take();
            for key in &value {
                if !from.contains(key) {
                    self.opened.insert(key.clone(), why);
                }
            }
            for key in &from {
                if !value.contains(key) {
                    self.opened.remove(key);
                }
            }
            self.exiting = self.exiting_after(&from, &value);
            cx.notify();
        }
        self.sync_panels(window, cx);
    }

    // The panels that may need binding: open, leaving, kept or bound.
    fn panel_keys(&self) -> Vec<SharedString> {
        let mut keys: Vec<SharedString> = self.value.get().clone();
        let items = self.items.keys();
        let kept = self
            .panel_props
            .borrow()
            .iter()
            .filter(|(key, props)| items.contains(key) && (self.keeps_mounted(props) || self.findable(props)))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let bound = self
            .panels
            .iter()
            .filter(|(_, panel)| panel.is_bound())
            .map(|(key, _)| key.clone());
        for key in self.exiting.iter().cloned().chain(kept).chain(bound) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys
    }

    fn sync_panels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for key in self.panel_keys() {
            self.sync_panel(&key, window, cx);
        }
        // Panels of items that left the page, once they're out of it too.
        let items = self.items.keys();
        self.panels.retain(|key, panel| panel.is_bound() || items.contains(key));
    }

    // Binds, checks or unbinds one panel for the state now (`bindPanel`).
    fn sync_panel(&mut self, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let props = self.panel_props.borrow().get(key).copied().unwrap_or_default();
        let present = self.panel_present(key, &props);
        let panel = match self.panels.get(key) {
            Some(panel) => panel.clone(),
            None if present => {
                let panel = Panel::new(cx).with_timing(self.panel_timing.clone());
                self.panels.insert(key.clone(), panel.clone());
                panel
            }
            None => return,
        };
        let settled = key.clone();
        let exited = key.clone();
        let binding = PanelBinding::default()
            .open(self.value.get().contains(key))
            .exiting(self.exiting.contains(key))
            .opening(self.opening(key))
            .trigger(self.triggers.existing_focus_handle(key))
            .settled(move |this: &mut Self, open, window, cx| this.settled(&settled, open, window, cx))
            .exited(move |this: &mut Self, window, cx| this.finish_exit(&exited, window, cx));
        panel.sync(self, present, &binding, window, cx);
    }

    fn settled(&mut self, key: &SharedString, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.opened.remove(key);
        if let Some(callback) = self.item_props(key).and_then(|props| props.on_open_change_complete) {
            callback(open, window, cx);
        }
    }

    fn finish_exit(&mut self, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.exiting.retain(|value| value != key);
        cx.notify();
        self.sync_panel(key, window, cx);
    }

    /// The `root` part, around the items. Build it before its items: it
    /// starts their render, so the order they render in is page order.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<AccordionState> {
        let accordion = this.read(cx);
        accordion.items.begin();
        accordion.triggers.begin();
        let form = accordion.form.clone();
        Part::new(&accordion.instance, "root", None, accordion.state()).map_el(|el| form.track(el))
    }

    /// The `item` part for `value`: its header with the trigger, and the
    /// panel the trigger opens. Values identify the items, so each is unique.
    pub fn item(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: AccordionItemProps,
        cx: &App,
    ) -> Part<AccordionItemState> {
        let value = value.into();
        let accordion = this.read(cx);
        let own = props.disabled;
        accordion.items.register(value.clone(), props);
        let state = accordion.item_state(&value, own);
        Part::new(&accordion.instance, "item", Some(&value), state)
    }

    /// The `header` part of the item with `value`: a heading around its
    /// trigger, so screen readers move between items by heading. It's level
    /// 3, as the ready-made accordion's `h3`; set `aria_level` for another
    /// that fits the page.
    pub fn header(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<AccordionHeaderState> {
        let value = value.into();
        let accordion = this.read(cx);
        let state = accordion.item_state(&value, false);
        Part::new(&accordion.instance, "header", Some(&value), state)
            .aria_role(Role::Heading)
            .aria_level(3)
    }

    /// The `trigger` part of the item with `value`: the button that opens and
    /// closes its panel. Put it inside the item's header. Disabled, it stays
    /// in the tab order. Tab moves between triggers; arrow keys leave focus
    /// where it is, as the APG asks now.
    pub fn trigger(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: AccordionTriggerProps,
        cx: &App,
    ) -> Part<AccordionTriggerState> {
        let value = value.into();
        let accordion = this.read(cx);
        let handle = accordion.triggers.focus_handle(&value, cx);
        accordion.triggers.register(value.clone(), ());
        let state = accordion.item_state(&value, props.disabled);
        let locked = accordion.locked(&value);
        let entity = this.downgrade();
        let key = value.clone();
        Part::new(&accordion.instance, "trigger", Some(&value), state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::TabStop, Some(&handle), false)
            .aria_expanded(state.open)
            .aria(
                Aria::new()
                    // Also when the open item can't close, as the APG asks.
                    .disabled(state.disabled || locked)
                    .controls(accordion.instance.element_id("panel", Some(&value))),
            )
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |accordion, cx| {
                        if accordion.disabled_of(&key, props.disabled) {
                            return;
                        }
                        let trigger = accordion.triggers.existing_focus_handle(&key);
                        focus_pressed_trigger(trigger.as_ref(), window, cx);
                        accordion.toggle(key.clone(), ChangeOptions::event_of(Reason::TriggerPress, event), window, cx);
                    })
                    .ok();
            })
    }

    /// The `panel` part of the item with `value`, or `None` while it's out of
    /// the page: a region named by its trigger. It stays in the page while it
    /// animates closed, and while closed with `keep_mounted`, hidden. Style it
    /// from its state: give it `panel.height` and clip its overflow while it
    /// animates.
    pub fn panel(
        this: &Entity<Self>,
        value: impl Into<SharedString>,
        props: AccordionPanelProps,
        cx: &App,
    ) -> Option<Part<AccordionPanelState>> {
        let value = value.into();
        let accordion = this.read(cx);
        // A render with new props, like `keep_mounted`, checks the panel
        // again (`stateAndRenders`).
        let stale = accordion.panel_props.borrow_mut().insert(value.clone(), props) != Some(props);
        if !accordion.panel_present(&value, &props) {
            return None;
        }
        let item = accordion.item_state(&value, false);
        let shown = item.open || accordion.exiting.contains(&value);
        let hidden = hidden_attrs(shown, accordion.findable(&props));
        let panel = accordion.panels.get(&value).cloned();
        let state = AccordionPanelState {
            open: item.open,
            disabled: item.disabled,
            index: item.index,
            panel: panel.as_ref().map(Panel::state).unwrap_or_default(),
        };
        let mut part = Part::new(&accordion.instance, "panel", Some(&value), state)
            .aria_role(Role::Region)
            .aria(
                Aria::new()
                    .labelled_by(accordion.instance.element_id("trigger", Some(&value)))
                    .hidden(hidden),
            );
        if hidden {
            part = part.hidden();
        }
        if let Some(panel) = &panel {
            part = panel.track(part);
        }
        if stale || panel.is_none() {
            let entity = this.downgrade();
            part = part.on_props(move |window, cx| {
                entity
                    .update(cx, |accordion, cx| accordion.sync_panel(&value, window, cx))
                    .ok();
            });
        }
        Some(part)
    }
}

/// `agent: { actions: ["setValue", "open", "close", "toggle", "openAll",
/// "closeAll"], closed: true, reveals: true }`.
impl Agent for Accordion {
    const SURFACE: AgentSurface = AgentSurface::new().closed(true).reveals(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |accordion, args, window, cx| {
            let value = args.list(0)?;
            Ok(accordion.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("open", |accordion, args, window, cx| {
            let value = args.string(0)?;
            Ok(accordion.open(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("close", |accordion, args, window, cx| {
            let value = args.string(0)?;
            Ok(accordion.close(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("toggle", |accordion, args, window, cx| {
            let value = args.string(0)?;
            Ok(accordion.toggle(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("openAll", |accordion, _args, window, cx| {
            Ok(accordion.open_all(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("closeAll", |accordion, _args, window, cx| {
            Ok(accordion.close_all(ChangeOptions::default(), window, cx).into())
        }),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        let value: Vec<String> = self.value.get().iter().map(ToString::to_string).collect();
        AgentState::new()
            .with("value", value)
            .with("disabled", self.is_disabled())
            .with("multiple", self.multiple)
    }
}
