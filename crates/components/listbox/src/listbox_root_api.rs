//! Port of `components/listbox/listbox-root-api.ts`: Listbox's state and
//! api: the value and its form, the option that holds the tab stop, and the
//! ways a selection changes: choosing one, toggling one, replacing the
//! selection, Shift ranges and selecting everything.

use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;

use gpui::{App, Context, FocusHandle, FocusOutEvent, Focusable, ScrollHandle, SharedString, Subscription, Window};
use ui_sdk_focus::{Claim, Orientation, create_claim};
use ui_sdk_forms::{
    Association, ElementInternals, INPUT_AND_CHANGE, associate, dispatch_change, focus_with, is_disabled,
    keeps_value_on_reset, labels_of,
};
use ui_sdk_kernel::focus_events::{on_focus, on_focus_out};
use ui_sdk_kernel::{ChangeOptions, Controllable, InputEvent, Instance, Reason};
use ui_sdk_lists::listbox_engine::{
    HighlightOptions, ItemKey, ListData, ListFiltering, ListFocus, ListHighlightDetails, ListScroller,
    ListboxEngine, ListboxEngineConfig, create_listbox_engine, highlight_from_code, highlightable,
    item_key, reveal_in_list,
};
use ui_sdk_lists::listbox_parts::{bind_form_root, warn_if_unnamed};
use ui_sdk_lists::listbox_value::{ListValue, ValueOptions, ValueSelection, ValueSelectionConfig, create_value_selection};
use ui_sdk_selection::{KeySelection, KeySelectionConfig, create_key_selection, is_same_keys};

use crate::listbox_root::{
    HighlightCallback, ItemEqualFn, ItemToValueFn, ListboxActivation, ListboxOptions, ListboxValue,
};

/// A listbox. Its entity is its controller.
pub struct Listbox<T: ItemKey + Clone + PartialEq + Debug + 'static = SharedString> {
    pub(crate) instance: Instance,
    pub(crate) value: Controllable<ListboxValue<T>>,
    // What a form reset returns to.
    pub(crate) initial: ListboxValue<T>,
    // Whether the app passed a default value, which a form reset of a
    // controlled listbox needs.
    has_default: bool,
    // The option that holds the list's one tab stop, once the options have
    // rendered.
    pub(crate) stop: Option<SharedString>,
    pub(crate) multiple: bool,
    pub(crate) activation: ListboxActivation,
    pub(crate) deselectable: bool,
    pub(crate) orientation: Orientation,
    pub(crate) keep_in_view: bool,
    // Focus stays on the list, which points at the highlighted option.
    pub(crate) virtual_focus: bool,
    pub(crate) disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) name: Option<SharedString>,
    pub(crate) form: Option<SharedString>,
    pub(crate) items: Option<ListData<T>>,
    pub(crate) r#loop: bool,
    pub(crate) item_to_value: Option<ItemToValueFn<T>>,
    pub(crate) is_item_equal_to_value: Option<ItemEqualFn<T>>,
    pub(crate) on_highlight: Option<HighlightCallback<T>>,
    // Whether focus is in the list. It stays true while a focused option
    // leaves the page, so focus can come back to the list.
    pub(crate) focus_inside: bool,
    // The option focused last, which keeps the tab stop while it's in the
    // list.
    pub(crate) remembered: Option<SharedString>,
    // The keys the list last asked for, so a controlled value coming back is
    // known as the list's own change, which keeps the anchor.
    requested: Option<Vec<SharedString>>,
    check_queued: bool,
    // Whether the selected option has been scrolled into view, once.
    revealed: bool,
    // The value as last seen, so one from outside the list is told apart.
    previous: ListboxValue<T>,
    // The option that held focus as of the last frame.
    pub(crate) focused_item: Option<SharedString>,
    // Whether the root rendered, which starts the check that something
    // names the list.
    root_bound: bool,
    pub(crate) listbox: ListboxEngine<Self, T>,
    pub(crate) selection: ValueSelection<Self, T>,
    pub(crate) key_selection: KeySelection<Self, SharedString>,
    pub(crate) claim: Claim<SharedString>,
    pub(crate) root_handle: FocusHandle,
    pub(crate) scroll_handle: ScrollHandle,
    pub(crate) internals: ElementInternals,
    _subscriptions: Vec<Subscription>,
}

impl<T: ItemKey + Clone + PartialEq + Debug + 'static> Listbox<T> {
    pub fn new(options: ListboxOptions<T>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("listbox").with_agent(cx);
        instance.debug = options.debug;
        let multiple = options.multiple;
        let empty = ListValue::empty(multiple);
        let first = options
            .value
            .clone()
            .or_else(|| options.default_value.clone())
            .unwrap_or_else(|| empty.clone());
        let mut value = Controllable::new("value", first.clone()).after_update();
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());

        // Nobody types into a listbox, so it ships no filtering.
        let config = ListboxEngineConfig::new(|this: &Self, value: &T| this.key_of(value))
            .focus(|this| {
                if this.virtual_focus {
                    ListFocus::Virtual
                } else {
                    ListFocus::Real
                }
            })
            .items(|this| this.items.clone())
            .r#loop(|this| this.r#loop)
            .skip_disabled(true)
            .active(|this| this.focus_inside)
            .container(|this| Some(this.root_handle.clone()))
            .scroll_root(|this| Some(ListScroller::new(this.scroll_handle.clone())))
            .keep_in_view(|this| this.keep_in_view)
            .on_highlight(|this, key, index, event, window, cx| this.highlight_moved(key, index, event, window, cx))
            .on_register(|this, _key, _window, cx| this.queue_check(cx));
        let listbox = create_listbox_engine(config, ListFiltering::none(), window, cx);
        // Once the options render, the list learns which took focus, and
        // checks its stop.
        listbox.set_settled_hook(Some(Rc::new(|this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            this.settle(window, cx)
        })));
        let selection = create_value_selection(
            ValueSelectionConfig::new(
                listbox.clone(),
                |this: &Self| this.value.get().as_slice(),
                |this| this.multiple,
                |this| ValueOptions {
                    item_to_value: this.item_to_value.clone(),
                    item_to_label: None,
                    is_item_equal_to_value: this.is_item_equal_to_value.clone(),
                },
            ),
            cx,
        );
        // Toggling, replacing, Shift ranges from the anchor and select all, on keys.
        let key_selection = create_key_selection(KeySelectionConfig::new(
            |this: &Self| this.selection.keys(this).to_vec(),
            |this: &mut Self, keys, why, window, cx| this.set_keys(keys, why, window, cx),
            |this: &Self| this.order(),
            |this: &Self, key: &SharedString| this.selectable(key),
        ));

        let root_handle = cx.focus_handle();
        let internals = ElementInternals::new(cx);
        let entity = cx.weak_entity();
        let subscriptions = vec![
            // The list itself got focus, like from a press on its padding, a
            // label, or an option that left the page.
            on_focus(&root_handle, window, cx, |this: &mut Self, window, cx| this.focus_stop(window, cx)),
            on_focus_out(&root_handle, window, cx, |this: &mut Self, event, window, cx| {
                this.blurred(event, window, cx)
            }),
            // A label pointing at the list, or a submit that finds it invalid,
            // focuses the option that holds the tab stop.
            focus_with(&internals, move |window, cx| {
                entity.update(cx, |this, cx| this.focus_stop(window, cx)).ok();
            }),
            associate(
                &internals,
                Association::new().reset(cx, |this: &mut Self, window, cx| this.reset(window, cx)),
            ),
        ];

        let mut this = Self {
            instance,
            value,
            initial: options.default_value.clone().unwrap_or(empty),
            has_default: options.default_value.is_some(),
            stop: None,
            multiple,
            // The APG's models: selection follows focus in a single list, and
            // waits for Space with several.
            activation: options.activation.unwrap_or(if multiple {
                ListboxActivation::Manual
            } else {
                ListboxActivation::Automatic
            }),
            deselectable: options.deselectable,
            orientation: options.orientation.unwrap_or(Orientation::Vertical),
            keep_in_view: options.keep_in_view,
            virtual_focus: options.virtual_focus,
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            name: options.name,
            form: options.form,
            items: options.items,
            r#loop: options.r#loop,
            item_to_value: options.item_to_value,
            is_item_equal_to_value: options.is_item_equal_to_value,
            on_highlight: options.on_highlight,
            focus_inside: false,
            remembered: None,
            requested: None,
            check_queued: false,
            revealed: false,
            previous: first,
            focused_item: None,
            root_bound: false,
            listbox,
            selection,
            key_selection,
            claim: create_claim(),
            root_handle,
            scroll_handle: ScrollHandle::new(),
            internals,
            _subscriptions: subscriptions,
        };
        // Data items are known at once, so their stop is too.
        if this.items.is_some() {
            this.stop = this.stop_key(window);
        }
        this.sync_form();
        this
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`syncListboxStore`).
    pub fn set_options(&mut self, options: ListboxOptions<T>, cx: &mut Context<Self>) {
        let multiple = options.multiple;
        // A list that starts or stops holding several values keeps what was
        // chosen, in its new shape, unless the value is controlled.
        if multiple != self.multiple {
            self.initial = self.initial.clone().shaped(multiple);
            if options.value.is_none() {
                let shaped = self.value.get().clone().shaped(multiple);
                self.value.set_held(shaped);
            }
        }
        let mut changed = self.value.set_controlled(options.value.clone());
        self.value.set_on_change(options.on_value_change.clone());
        self.value.set_on_before_change(options.on_before_value_change.clone());
        // Like a native select's default, changing it changes what a reset
        // returns to.
        if let Some(default) = &options.default_value {
            self.initial = default.clone();
        }
        self.has_default = options.default_value.is_some();
        let activation = options.activation.unwrap_or(if multiple {
            ListboxActivation::Manual
        } else {
            ListboxActivation::Automatic
        });
        let orientation = options.orientation.unwrap_or(Orientation::Vertical);
        changed |= self.multiple != multiple
            || self.activation != activation
            || self.deselectable != options.deselectable
            || self.orientation != orientation
            || self.keep_in_view != options.keep_in_view
            || self.virtual_focus != options.virtual_focus
            || self.disabled != options.disabled
            || self.read_only != options.read_only
            || self.required != options.required
            || self.name != options.name
            || self.form != options.form
            || self.items != options.items
            || self.r#loop != options.r#loop;
        self.multiple = multiple;
        self.activation = activation;
        self.deselectable = options.deselectable;
        self.orientation = orientation;
        self.keep_in_view = options.keep_in_view;
        self.virtual_focus = options.virtual_focus;
        self.disabled = options.disabled;
        self.read_only = options.read_only;
        self.required = options.required;
        self.name = options.name;
        self.form = options.form;
        self.items = options.items;
        self.r#loop = options.r#loop;
        self.item_to_value = options.item_to_value;
        self.is_item_equal_to_value = options.is_item_equal_to_value;
        self.on_highlight = options.on_highlight;
        self.instance.debug = options.debug;
        self.value_changed(cx);
        self.sync_form();
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the listbox keep its
    /// own.
    pub fn set_controlled_value(&mut self, value: Option<ListboxValue<T>>, cx: &mut Context<Self>) {
        if self.value.set_controlled(value) {
            self.value_changed(cx);
            self.sync_form();
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The key of a value: `item_to_value`'s, else its own (`keyOf`).
    pub fn key_of(&self, value: &T) -> SharedString {
        match &self.item_to_value {
            Some(item_to_value) => item_to_value(value),
            None => item_key(value, None),
        }
    }

    /// Whether the listbox is disabled, by itself, its field or a fieldset
    /// (`isDisabled`). A field disables the listbox it's the control of, as
    /// the original's adapters pass the field's `disabled` into it.
    pub fn is_disabled(&self) -> bool {
        is_disabled(
            self.disabled || self.internals.control_attrs().disabled,
            self.internals.form_disabled(),
        )
    }

    /// Whether it keeps its value but refuses changes.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether several items can be selected.
    pub fn is_multiple(&self) -> bool {
        self.multiple
    }

    pub(crate) fn order(&self) -> Vec<SharedString> {
        self.listbox.entries(self).iter().map(|entry| entry.key.clone()).collect()
    }

    pub(crate) fn selectable(&self, key: &SharedString) -> bool {
        self.listbox
            .find(self, Some(key))
            .is_some_and(|entry| !self.listbox.disabled(self, &entry))
    }

    // The option that holds the tab stop: the one focused last, the first
    // selected, the first enabled, or the first, as the APG's listbox asks.
    // A disabled selected option doesn't hold it, since keys skip disabled
    // ones.
    pub(crate) fn stop_key(&mut self, window: &Window) -> Option<SharedString> {
        let list = self.listbox.entries(self);
        if let Some(remembered) = &self.remembered
            && list.iter().any(|entry| &entry.key == remembered)
        {
            return Some(remembered.clone());
        }
        // An option whose value changed while it had focus never told it
        // took focus, so it's found by its focus handle.
        if self.remembered.is_some()
            && let Some(held) = self.listbox.focused(window)
            && list.iter().any(|entry| entry.key == held)
        {
            if self.listbox.highlighted() == self.remembered {
                self.listbox.set_highlighted(Some(held.clone()));
            }
            self.remembered = Some(held.clone());
            return Some(held);
        }
        let enabled: Vec<_> = list.iter().filter(|entry| !entry.disabled).collect();
        let selected = enabled
            .iter()
            .find(|entry| self.selection.is_selected(self, &entry.key, entry.value.as_ref()));
        selected
            .or(enabled.first())
            .map(|entry| entry.key.clone())
            .or_else(|| list.first().map(|entry| entry.key.clone()))
    }

    // Moves the tab stop where it belongs, and scrolls the selected option
    // into view once (`check`).
    pub(crate) fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.check_queued = false;
        let stop = self.stop_key(window);
        if stop != self.stop {
            self.stop = stop.clone();
            cx.notify();
        }
        if !self.revealed {
            self.revealed = self.reveal_selected(stop.as_ref());
        }
    }

    // Once the options are in place, the selected one scrolls into view
    // without taking focus, like in a native list. Returns whether they
    // were.
    fn reveal_selected(&self, key: Option<&SharedString>) -> bool {
        let Some(key) = key else {
            return false;
        };
        if !self.listbox.is_connected(key) {
            return false;
        }
        if !self.selection.keys(self).is_empty()
            && self.keep_in_view
            && let Some(bounds) = self.listbox.bounds(key)
        {
            reveal_in_list(bounds, &ListScroller::new(self.scroll_handle.clone()));
        }
        true
    }

    // Batched, since a long list registers all at once: the check runs once
    // the options have rendered again.
    pub(crate) fn queue_check(&mut self, cx: &mut Context<Self>) {
        if self.check_queued {
            return;
        }
        self.check_queued = true;
        cx.notify();
    }

    // What the list learns once each frame that rendered its options is
    // drawn: which option took focus, and a check that was queued.
    pub(crate) fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Focus landed on an option, from a key, a press or Tab.
        let focused = self.listbox.focused(window);
        if focused != self.focused_item {
            self.focused_item = focused.clone();
            if let Some(key) = focused {
                self.focused(key, cx);
            }
        }
        if self.check_queued {
            self.check(window, cx);
        }
    }

    // The root rendered: development builds check, once, that something
    // names the list.
    pub(crate) fn root_rendered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.root_bound, true) {
            return;
        }
        warn_if_unnamed(
            |this: &Self| &this.instance,
            "root",
            None,
            "The listbox has no accessible name. Add a Field label, an aria_label or a labelled-by.",
            |this: &Self, _cx| !labels_of(this.internals.node()).is_empty(),
            window,
            cx,
        );
    }

    // Reports the value and validity to the form, as the original's
    // `bindFormRoot` does on every change: the selected keys under its name,
    // and a missing value while required with nothing selected.
    pub(crate) fn sync_form(&self) {
        bind_form_root(&self.internals, &self.form_state(), &self.selection.keys(self));
    }

    // Asks for a value (`setValue`): a disabled or read-only listbox refuses
    // anything but a reset, and a change goes out to the form.
    pub(crate) fn change_value(
        &mut self,
        value: ListboxValue<T>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        if reason != Reason::Reset && (self.is_disabled() || self.read_only) {
            return false;
        }
        let options = ChangeOptions {
            reason: Some(reason),
            ..options
        };
        let done = self.value.change(&self.instance, value, options, window, cx);
        if done {
            self.value_changed(cx);
            self.sync_form();
            cx.notify();
            if reason != Reason::Reset && reason != Reason::Restore {
                dispatch_change(Some(self.internals.node()), INPUT_AND_CHANGE, window, cx);
            }
        }
        done
    }

    // Selects these keys, keeping the values already selected as they are.
    fn set_keys(&mut self, keys: Vec<SharedString>, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let current: HashMap<SharedString, T> = self
            .selection
            .keys(self)
            .iter()
            .cloned()
            .zip(self.selection.values(self))
            .collect();
        let values: Vec<T> = keys
            .iter()
            .filter_map(|key| current.get(key).cloned().or_else(|| self.listbox.value_of(self, key)))
            .collect();
        self.requested = Some(keys);
        let done = self.change_value(ListValue::Multiple(values), why, window, cx);
        // A refused change, like on a read-only list, has nothing to recognize later.
        if !done {
            self.requested = None;
        }
        done
    }

    // Chooses an option of a single listbox. Choosing the selected one again
    // keeps it, or unselects it with `deselectable`.
    pub(crate) fn choose(&mut self, key: &SharedString, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) {
        let listbox = self.listbox.clone();
        let Some(entry) = listbox.find(self, Some(key)) else {
            return;
        };
        if listbox.disabled(self, &entry) {
            return;
        }
        let Some(value) = entry.value else {
            return;
        };
        if self.selection.is_selected(self, key, Some(&value)) {
            if self.deselectable && why.reason == Some(Reason::ItemPress) {
                self.change_value(ListValue::Single(None), why, window, cx);
            }
            return;
        }
        self.change_value(ListValue::Single(Some(value)), why, window, cx);
    }

    pub(crate) fn toggle(&mut self, key: &SharedString, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.key_selection.clone();
        selection.toggle(self, key, why, window, cx);
    }

    pub(crate) fn replace(&mut self, key: &SharedString, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.key_selection.clone();
        selection.replace(self, key, why, window, cx);
    }

    pub(crate) fn extend(
        &mut self,
        to: &SharedString,
        from: Option<&SharedString>,
        additive: bool,
        why: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = self.key_selection.clone();
        selection.extend(self, to, from, additive, why, window, cx);
    }

    pub(crate) fn toggle_all(&mut self, why: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiple {
            let selection = self.key_selection.clone();
            selection.toggle_all(self, why, window, cx);
        }
    }

    // Focus landed on an option, from a key, a press or Tab.
    pub(crate) fn focused(&mut self, key: SharedString, cx: &mut Context<Self>) {
        self.focus_inside = true;
        self.remembered = Some(key.clone());
        self.listbox.set_highlighted(Some(key.clone()));
        self.stop = Some(key);
        cx.notify();
    }

    // The list itself got focus, like from a press on its padding, a label,
    // or an option that left the page. It hands focus to the tab stop, or
    // with `virtual_focus` keeps it and highlights the tab stop.
    pub(crate) fn focus_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_inside = true;
        let key = match self.stop.clone() {
            Some(stop) => Some(stop),
            None => self.stop_key(window),
        };
        if self.virtual_focus {
            if !self.root_handle.is_focused(window) {
                window.focus(&self.root_handle, cx);
            }
            if let Some(key) = key
                && self.listbox.highlighted().is_none()
            {
                self.focused(key, cx);
            }
            return;
        }
        if let Some(key) = key
            && self.listbox.is_connected(&key)
            && let Some(handle) = self.listbox.existing_focus_handle(&key)
        {
            window.focus(&handle, cx);
        }
    }

    // A press or the pointer moves the highlight while focus stays on the
    // list.
    pub(crate) fn highlight_virtually(
        &mut self,
        key: &SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let listbox = self.listbox.clone();
        if let Some(entry) = listbox.find(self, Some(key))
            && !listbox.disabled(self, &entry)
        {
            self.focus_inside = true;
            listbox.highlight(self, Some(key.clone()), HighlightOptions::event(event), window, cx);
        }
    }

    // The highlight moved. With `virtual_focus`, the option highlighted last
    // holds the tab stop, as the one focused last does without it.
    fn highlight_moved(
        &mut self,
        key: Option<&SharedString>,
        index: Option<usize>,
        event: Option<&InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.virtual_focus
            && let Some(key) = key
            && Some(key) != self.remembered.as_ref()
        {
            self.remembered = Some(key.clone());
            if self.stop.as_ref() != Some(key) {
                self.stop = Some(key.clone());
                cx.notify();
            }
        }
        if let Some(on_highlight) = self.on_highlight.clone() {
            let value = key.and_then(|key| self.listbox.value_of(self, key));
            let details = ListHighlightDetails {
                index,
                event: event.cloned(),
            };
            on_highlight(value.as_ref(), &details, window, cx);
        }
    }

    // Focus left the list. An option that left the page takes focus with it,
    // which isn't leaving: the engine gives focus back once it sees it left,
    // so this waits until after that.
    fn blurred(&mut self, event: FocusOutEvent, window: &mut Window, cx: &mut Context<Self>) {
        let target = event.blurred;
        cx.defer_in(window, move |this, window, cx| {
            if target.upgrade().is_none_or(|handle| !handle.is_rendered(window)) {
                return;
            }
            if this.root_handle.contains_focused(window, cx) {
                return;
            }
            this.focus_inside = false;
            this.focused_item = None;
            if this.listbox.set_highlighted(None) {
                cx.notify();
            }
        });
    }

    // A value from outside the list, like a controlled value or a reset,
    // starts ranges and the tab stop over. An empty selection drops the
    // anchor.
    fn value_changed(&mut self, cx: &mut Context<Self>) {
        if *self.value.get() == self.previous {
            return;
        }
        self.previous = self.value.get().clone();
        let keys = self.selection.keys(self);
        let own = self
            .requested
            .take()
            .is_some_and(|requested| is_same_keys(&requested, &keys));
        if !own || keys.is_empty() {
            self.key_selection.reset_anchor();
        }
        if !own && !self.focus_inside {
            self.remembered = None;
        }
        self.queue_check(cx);
    }

    /// What a form reset does: back to `default_value`. A controlled
    /// listbox with no default keeps its value. It resets a disabled or
    /// read-only listbox too, as a native one.
    pub(crate) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let controlled = self.value.is_controlled().then_some(());
        let default = self.has_default.then_some(());
        if keeps_value_on_reset(controlled.as_ref(), default.as_ref()) {
            return;
        }
        let initial = self.initial.clone();
        self.change_value(initial, ChangeOptions::reason(Reason::Reset), window, cx);
    }

    /// The selected value.
    pub fn value(&self) -> &ListboxValue<T> {
        self.value.get()
    }

    /// Selects a value, as a user would. A disabled or read-only listbox
    /// ignores it. Returns false when the change was canceled or nothing
    /// changed; for a controlled listbox, true means accepted.
    pub fn set_value(
        &mut self,
        value: impl Into<ListboxValue<T>>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.change_value(value.into(), options, window, cx)
    }

    /// Selects nothing: `None`, or an empty list with `multiple`. Returns
    /// false when the change was canceled or nothing changed.
    pub fn clear(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let empty = self.selection.empty(self);
        self.change_value(empty, options, window, cx)
    }

    /// Selects every enabled item, with `multiple`. Returns false when the
    /// change was canceled or nothing changed.
    pub fn select_all(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.multiple {
            return false;
        }
        let missing: Vec<SharedString> = self
            .order()
            .into_iter()
            .filter(|key| self.selectable(key) && !self.selection.is_selected(self, key, None))
            .collect();
        if missing.is_empty() {
            return false;
        }
        let mut keys = self.selection.keys(self).to_vec();
        keys.extend(missing);
        self.set_keys(keys, options, window, cx)
    }

    /// The value of the highlighted option, or `None` when none is, like
    /// while focus is outside the list (`getHighlighted`).
    pub fn highlighted(&self) -> Option<T> {
        let key = self.listbox.highlighted()?;
        self.listbox.value_of(self, &key)
    }

    /// Highlights the option with this value, scrolled into view, or none
    /// with `None`. While focus is outside the list, the option takes the
    /// tab stop instead, so focus lands on it next. Returns false when no
    /// enabled option has the value.
    pub fn highlight(&mut self, value: Option<&T>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let key = value.map(|value| self.key_of(value));
        if self.root_handle.contains_focused(window, cx) {
            let current = self.listbox.highlighted();
            let listbox = self.listbox.clone();
            return highlight_from_code(&listbox, self, key, current.as_ref(), window, cx);
        }
        let Some(key) = key else {
            return false;
        };
        let Some(entry) = highlightable(&self.listbox, self, &key) else {
            return false;
        };
        self.remembered = Some(entry.key.clone());
        self.stop = Some(entry.key);
        cx.notify();
        true
    }

    /// Moves focus into the list, onto the option that holds its tab stop.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_stop(window, cx);
    }
}

impl<T: ItemKey + Clone + PartialEq + Debug + 'static> Focusable for Listbox<T> {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.root_handle.clone()
    }
}
