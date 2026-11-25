//! Port of `components/radio/radio-api.ts`: a radio group's state and api,
//! which Radio, Segmented Control and Rating share.
//!
//! A component that is a radio group keeps a [`RadioGroupApi`] in its entity
//! and implements [`RadioHost`], which the parts in `radio_parts.rs` and the
//! group's form reach the api through. Every change to the checked value, from
//! a press, a key, a form reset or a controlled value, goes through the host,
//! so it hears the change before anything renders it
//! ([`RadioHost::value_changed`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, Entity, KeyDownEvent, Pixels, SharedString, Subscription, Window,
};
use ui_sdk_focus::{
    Direction, Orientation, RovingFocus, RovingFocusMoveContext, RovingFocusOptions, move_index,
    radio_step,
};
use ui_sdk_forms::{
    ElementInternals, Form, INPUT_AND_CHANGE, InternalsOptions, Node, ValueMissingKind,
    bind_internals, dispatch_change, focus_with, is_disabled, keeps_value_on_reset,
    value_missing_message,
};
use ui_sdk_kernel::{ChangeOptions, Controllable, InputEvent, Instance, Key, Parts, Reason, held};

use crate::radio::{RadioGroupOptions, RadioProps, RadioState};

/// The slots a radio group's api reaches its parts through: the group and
/// each radio (`RadioSlotNames`). Radio names them `group` and `root`,
/// Segmented Control `root` and `item`, Rating `root` and `radio`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadioSlotNames {
    pub group: &'static str,
    pub item: &'static str,
}

impl Default for RadioSlotNames {
    fn default() -> Self {
        Self {
            group: "group",
            item: "root",
        }
    }
}

/// A radio group's state and api (`radioGroupState` and `radioGroupApi`),
/// which its slots and its host call. Radio, Segmented Control and Rating
/// each keep one in their entity.
pub struct RadioGroupApi {
    instance: Instance,
    slots: RadioSlotNames,
    value: Controllable<Option<SharedString>>,
    // What a form reset returns to: the first value.
    initial: Option<SharedString>,
    // Whether the options pass a `default_value`, which a controlled group
    // resets to.
    has_default: bool,
    disabled: bool,
    read_only: bool,
    required: bool,
    name: Option<SharedString>,
    form: Option<SharedString>,
    r#loop: bool,
    // Indicators animating out.
    exiting: Vec<SharedString>,
    // Only a segmented control has these.
    orientation: Option<Orientation>,
    direction: Option<Direction>,
    // The radio a pointer previews, which only a rating shows.
    highlighted: Option<SharedString>,
    // Whether the value changed since the group was made.
    changed: bool,
    // The value the last change saw.
    previous: Option<SharedString>,
    // Like native radios: both axes move, disabled radios are skipped, and
    // Tab lands on the checked radio, or with nothing checked, the first
    // enabled one. The roving group keeps the item Tab lands on (`stop`).
    roving: RovingFocus<SharedString>,
    // The group's part in forms.
    internals: ElementInternals,
    // The radios in the page, in order, with the props each rendered with.
    items: Parts<SharedString, RadioProps>,
    // Each radio's part in forms, made the first time it renders: a control
    // that a label around it or pointing at it names and presses.
    radios: RefCell<HashMap<SharedString, ElementInternals>>,
    _subscriptions: Vec<Subscription>,
}

impl RadioGroupApi {
    /// The api for the component of `cx`, which owns `instance` and reaches
    /// the api through [`RadioHost`]. `slots` names the group's and the
    /// radios' slots.
    pub fn new<T: RadioHost>(
        instance: Instance,
        options: &RadioGroupOptions,
        slots: RadioSlotNames,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Self {
        let first = match &options.value {
            Some(value) => value.clone(),
            None => options.default_value.clone().flatten(),
        };
        let mut value = Controllable::new("value", first.clone());
        value.set_controlled(options.value.clone());
        value.set_on_change(options.on_value_change.clone());
        value.set_on_before_change(options.on_before_value_change.clone());
        let roving = RovingFocus::new(RovingFocusOptions::default().remember(false), window, cx);
        let internals = ElementInternals::new(cx);
        // A group isn't focusable: a label pointing at it, or a submit that
        // finds it invalid, focuses its tab stop.
        let focus = {
            let roving = roving.clone();
            focus_with(&internals, move |window, cx| roving.focus(None, window, cx))
        };
        let reset = internals.on_reset(cx, |host: &mut T, window, cx| {
            host.radio_mut().reset(window, cx);
            host.sync_radio(window, cx);
        });
        let api = Self {
            instance,
            slots,
            value,
            initial: first.clone(),
            has_default: options.default_value.is_some(),
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            name: options.name.clone(),
            form: options.form.clone(),
            r#loop: options.r#loop,
            exiting: Vec::new(),
            orientation: None,
            direction: None,
            highlighted: None,
            changed: false,
            previous: first,
            roving,
            internals,
            items: Parts::new(),
            radios: RefCell::new(HashMap::new()),
            _subscriptions: vec![focus, reset],
        };
        api.sync_form();
        api
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`syncRadioGroup`). Returns whether what renders changed. The host
    /// calls [`RadioHost::sync_radio`] after, for a controlled value that
    /// moved.
    pub fn set_options(&mut self, options: &RadioGroupOptions) -> bool {
        let mut changed = self.value.set_controlled(options.value.clone());
        self.value.set_on_change(options.on_value_change.clone());
        self.value.set_on_before_change(options.on_before_value_change.clone());
        // Like a native radio's `defaultChecked`, changing it changes what a
        // reset returns to.
        if let Some(default) = &options.default_value {
            self.initial = default.clone();
        }
        self.has_default = options.default_value.is_some();
        changed |= self.disabled != options.disabled
            || self.read_only != options.read_only
            || self.required != options.required
            || self.name != options.name
            || self.form != options.form;
        self.disabled = options.disabled;
        self.read_only = options.read_only;
        self.required = options.required;
        self.name = options.name.clone();
        self.form = options.form.clone();
        self.r#loop = options.r#loop;
        self.instance.debug = options.debug;
        self.sync_form();
        changed
    }

    /// Passes the controlled value, `Some(None)` to check nothing, or `None`
    /// to let the group keep its own. Returns whether what renders changed.
    pub fn set_controlled_value(&mut self, value: Option<Option<SharedString>>) -> bool {
        let changed = self.value.set_controlled(value);
        self.sync_form();
        changed
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub fn instance_mut(&mut self) -> &mut Instance {
        &mut self.instance
    }

    /// The slots it names its parts by.
    pub fn slots(&self) -> RadioSlotNames {
        self.slots
    }

    /// The instance's id.
    pub fn id(&self) -> &SharedString {
        self.instance.id()
    }

    /// The checked radio's value.
    pub fn get_value(&self) -> Option<SharedString> {
        self.value.get().clone()
    }

    pub fn is_controlled(&self) -> bool {
        self.value.is_controlled()
    }

    /// Whether the value changed since the group was made, which animates
    /// what shows it.
    pub fn is_changed(&self) -> bool {
        self.changed
    }

    /// Whether the group is disabled, by itself, its field or a fieldset. A
    /// field disables the group it's the control of, as the original's
    /// adapters pass the field's `disabled` into it.
    pub fn is_disabled(&self) -> bool {
        is_disabled(
            self.disabled || self.internals.control_attrs().disabled,
            self.internals.form_disabled(),
        )
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn is_required(&self) -> bool {
        self.required
    }

    /// The name the value submits under.
    pub fn name(&self) -> Option<&SharedString> {
        self.name.as_ref()
    }

    /// Whether arrow keys wrap from the last radio to the first.
    pub fn is_loop(&self) -> bool {
        self.r#loop
    }

    /// Indicators animating out, by their radio's value.
    pub fn exiting(&self) -> &[SharedString] {
        &self.exiting
    }

    /// An indicator finished animating out (`finishExit`).
    pub fn finish_exit(&mut self, key: &str) {
        self.exiting.retain(|value| value.as_ref() != key);
    }

    /// Which way the radios run, for a segmented control.
    pub fn orientation(&self) -> Option<Orientation> {
        self.orientation
    }

    pub fn set_orientation(&mut self, orientation: Option<Orientation>) -> bool {
        std::mem::replace(&mut self.orientation, orientation) != orientation
    }

    /// Which way the checked radio last moved on screen, for a segmented
    /// control's indicator.
    pub fn direction(&self) -> Option<Direction> {
        self.direction
    }

    pub fn set_direction(&mut self, direction: Option<Direction>) -> bool {
        std::mem::replace(&mut self.direction, direction) != direction
    }

    /// The radio a pointer previews, which only a rating shows.
    pub fn highlighted(&self) -> Option<&SharedString> {
        self.highlighted.as_ref()
    }

    /// Previews a radio, or with `None` none. Returns whether it changed.
    pub fn set_highlighted(&mut self, key: Option<SharedString>) -> bool {
        if self.highlighted == key {
            return false;
        }
        self.highlighted = key;
        true
    }

    /// The group's roving tab stop, which its radios bind to.
    pub fn roving(&self) -> &RovingFocus<SharedString> {
        &self.roving
    }

    /// The group's part in forms.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    /// The form the group belongs to, as of the last frame.
    pub fn form(&self) -> Option<Form> {
        self.internals.form()
    }

    /// The radios in the page, in order, as of their last render.
    pub fn keys(&self) -> Vec<SharedString> {
        self.items.keys()
    }

    /// The props a radio rendered with, for parts inside it (`itemProps`).
    pub fn item_props(&self, key: &str) -> RadioProps {
        self.items
            .info(&SharedString::from(key.to_string()))
            .unwrap_or_default()
    }

    /// The state a radio renders with, from the props it rendered with.
    pub fn item_state(&self, key: &str) -> RadioState {
        self.item_state_with(key, &self.item_props(key))
    }

    /// The state a radio renders with (`itemState`).
    pub fn item_state_with(&self, key: &str, props: &RadioProps) -> RadioState {
        RadioState {
            checked: self.value.get().as_deref() == Some(key),
            disabled: props.disabled || self.is_disabled(),
            read_only: self.read_only,
            required: self.required,
        }
    }

    /// A radio's bounds as of its last layout (`radio`'s element).
    pub fn radio_bounds(&self, key: &str) -> Option<Bounds<Pixels>> {
        self.instance
            .bound_parts()
            .into_iter()
            .find(|part| part.slot == self.slots.item && part.key.as_deref() == Some(key))
            .and_then(|part| part.bounds)
    }

    /// The group's bounds as of its last layout (`group`'s element).
    pub fn group_bounds(&self) -> Option<Bounds<Pixels>> {
        self.instance
            .bound_parts()
            .into_iter()
            .find(|part| part.slot == self.slots.group)
            .and_then(|part| part.bounds)
    }

    /// Where the group's validation message points: the first radio that
    /// can take focus, or the first radio (`anchor`).
    pub fn anchor(&self) -> Option<Node> {
        let key = self
            .roving
            .items()
            .into_iter()
            .next()
            .map(|item| item.key)
            .or_else(|| self.items.keys().into_iter().next())?;
        self.radios
            .borrow()
            .get(&key)
            .map(|internals| internals.node().clone())
    }

    /// Starts a render of the radios, from the part around them, before they
    /// render: the order they render in is page order.
    pub fn begin(&self, cx: &App) {
        self.items.begin();
        self.roving.begin(cx);
    }

    /// Puts the group's part in forms in its element, which says what form
    /// holds it and the fieldsets around it. Call it before adding children.
    pub fn track<E: gpui::ParentElement + gpui::StatefulInteractiveElement>(&self, el: E) -> E {
        self.internals.track(el)
    }

    /// Reports the group's value and validity to its form, as the original's
    /// `bindInternals` does on every change: the checked value under its
    /// name, and a missing value while required with nothing checked. The
    /// group part calls it as it renders; changes call it themselves.
    pub fn sync_form(&self) {
        let value = self.value.get().clone();
        bind_internals(
            &self.internals,
            InternalsOptions {
                value: value.clone().into(),
                name: self.name.clone(),
                value_missing: self.required && value.is_none(),
                message: value_missing_message(ValueMissingKind::Radio),
                anchor: self.anchor(),
                disabled: self.disabled,
                read_only: self.read_only,
                required: self.required,
                form: self.form.clone(),
                ..Default::default()
            },
        );
    }

    /// Checks a radio, as a user would. Disabled radios and read-only groups
    /// ignore it. A change goes out to its form as `Input` and `Change`.
    /// Returns false when nothing changed or the change was canceled; for a
    /// controlled group, true means accepted. Hosts call it through
    /// [`RadioHost::set_radio_value`], which hears the change.
    pub fn set_value<T: 'static>(
        &mut self,
        value: Option<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        if self.is_disabled() || self.read_only {
            return false;
        }
        if let Some(key) = &value
            && self.item_props(key).disabled
        {
            return false;
        }
        let options = ChangeOptions {
            reason: Some(options.reason.unwrap_or(Reason::Imperative)),
            ..options
        };
        let done = self.value.change(&self.instance, value, options, window, cx);
        if done {
            self.sync_form();
            cx.notify();
            dispatch_change(Some(self.internals.node()), INPUT_AND_CHANGE, window, cx);
        }
        done
    }

    /// What a form reset does: back to the first value, or to
    /// `default_value`. A controlled group with no default keeps its value.
    /// It resets a disabled or read-only group too, as a native one.
    pub fn reset<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        let controlled = self.value.is_controlled().then_some(());
        let default = self.has_default.then_some(());
        if !keeps_value_on_reset(controlled.as_ref(), default.as_ref()) {
            let initial = self.initial.clone();
            let options = ChangeOptions::reason(Reason::Reset);
            if self.value.change(&self.instance, initial, options, window, cx) {
                cx.notify();
            }
        }
        // Fields read validity right after a reset.
        self.sync_form();
    }

    /// What the original's store subscription does once the value changed,
    /// from any source, before anything renders it: the group has changed,
    /// and an indicator still in the page starts animating out. Returns the
    /// value it changed from, or `None` when it didn't change. Hosts call it
    /// through [`RadioHost::sync_radio`], which then moves the tab stop
    /// ([`RadioGroupApi::check_stop`]).
    pub fn take_change(&mut self) -> Option<Option<SharedString>> {
        let value = self.value.get().clone();
        if value == self.previous {
            return None;
        }
        let from = std::mem::replace(&mut self.previous, value.clone());
        self.changed = true;
        self.exiting = self.exiting_after(from.as_ref(), value.as_ref());
        self.sync_form();
        Some(from)
    }

    // The indicators animating out once `value` replaces `from`.
    fn exiting_after(&self, from: Option<&SharedString>, value: Option<&SharedString>) -> Vec<SharedString> {
        let mut exiting: Vec<SharedString> = self
            .exiting
            .iter()
            .filter(|key| Some(*key) != value)
            .cloned()
            .collect();
        if let Some(from) = from
            && self.has_indicator(from)
            && !self.exiting.contains(from)
        {
            exiting.push(from.clone());
        }
        exiting
    }

    // Indicators are keyed by their radio's value, so this is a lookup, not
    // a sort.
    fn has_indicator(&self, value: &str) -> bool {
        held(&self.instance, "indicator", Some(value))
    }

    /// Moves focus to the radio that holds the tab stop, the checked one if
    /// it's enabled, or the first.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.roving.focus(None, window, cx);
    }

    /// Moves the tab stop where the value puts it now, the checked radio if
    /// it's enabled, before the radios render the value: the original's
    /// subscription checks it, and `deriveFromOptions` derives it from a
    /// value in options, in the same pass.
    pub fn check_stop(&self, cx: &mut App) {
        let value = self.value.get();
        self.roving
            .check_with(|item| value.as_ref() == Some(&item.key), cx);
    }

    // A radio rendering: what it rendered with, in page order.
    pub(crate) fn register(&self, key: SharedString, props: RadioProps) {
        self.items.register(key, props);
    }

    // The radio's part in forms, made the first time it renders: a control a
    // label around it or pointing at it names, and presses as a native label
    // does. A radio disabled itself is a disabled control, which its labels
    // don't press.
    pub(crate) fn radio_internals<T: RadioHost>(
        &self,
        this: &Entity<T>,
        key: &SharedString,
        props: &RadioProps,
    ) -> ElementInternals {
        let internals = self
            .radios
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| {
                let internals = ElementInternals::detached();
                internals.node().set_observer(Some(this.entity_id()));
                let entity = this.downgrade();
                let key = key.clone();
                internals.set_activation(Some(Rc::new(move |event, window, cx| {
                    entity
                        .update(cx, |host, cx| host.activate_radio(key.clone(), event, window, cx))
                        .ok();
                })));
                internals
            })
            .clone();
        bind_internals(
            &internals,
            InternalsOptions {
                disabled: props.disabled,
                ..Default::default()
            },
        );
        internals
    }
}

/// The component a radio group's api lives in, like a radio group, a rating
/// or a segmented control, which the group's parts and its form reach the
/// api through. It implements the first two methods; the rest have defaults,
/// and the ones a component changes, like a rating's `check`, it overrides.
pub trait RadioHost: 'static + Sized {
    fn radio(&self) -> &RadioGroupApi;

    fn radio_mut(&mut self) -> &mut RadioGroupApi;

    /// Checks the radio a press landed on (`internal.check`). A rating
    /// unchecks a checked one.
    fn check(
        &mut self,
        key: SharedString,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.set_radio_value(Some(key), options, window, cx)
    }

    /// Runs once the checked value changed from `from`, from any source,
    /// before anything renders the change, like a radio's indicators
    /// animating.
    fn value_changed(&mut self, _from: Option<SharedString>, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Checks a radio, as a user would ([`RadioGroupApi::set_value`]), and
    /// hears the change.
    fn set_radio_value(
        &mut self,
        value: Option<SharedString>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let done = self.radio_mut().set_value(value, options, window, cx);
        self.sync_radio(window, cx);
        done
    }

    /// Hears a change to the checked value from any source:
    /// [`RadioGroupApi::take_change`], the tab stop moving to the checked
    /// radio ([`RadioGroupApi::check_stop`]), then
    /// [`RadioHost::value_changed`]. Call it after handing the api new
    /// options or a controlled value.
    fn sync_radio(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(from) = self.radio_mut().take_change() {
            self.radio().check_stop(cx);
            self.value_changed(from, window, cx);
            cx.notify();
        }
    }

    /// A key pressed on the radio `key` (`internal.onKeyDown`): arrow keys
    /// move focus to the next or previous enabled radio and, like native
    /// radios, check it, unless the group is read-only. The radio's own
    /// capture takes Enter, which submits its form, and gpui clicks a focused
    /// radio on Space's release, as it does a native button.
    fn radio_key_down(
        &mut self,
        event: &KeyDownEvent,
        key: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.alt || modifiers.control || modifiers.platform {
            return;
        }
        if matches!(Key::of(&event.keystroke), Key::Enter | Key::Space) {
            return;
        }
        let r#loop = self.radio().r#loop;
        let roving = self.radio().roving.clone();
        let moved = roving.keydown_with(
            event,
            key,
            |event, context, _cx| radio_move(event, context, r#loop),
            window,
            cx,
        );
        // Like native radios, moving checks the item.
        if let Some(moved) = moved
            && moved.key != moved.from
            && !self.radio().read_only
        {
            let options = ChangeOptions::event(Reason::ListNavigation, event);
            self.set_radio_value(Some(moved.key), options, window, cx);
        }
    }

    /// A label naming the radio `key` was pressed: like a native label, it
    /// focuses the radio and clicks it. A disabled radio takes neither, also
    /// one only its group disables, which keeps a native radio's label from
    /// focusing it.
    fn activate_radio(
        &mut self,
        key: SharedString,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.radio().item_state(&key).disabled {
            return;
        }
        self.radio().roving.focus(Some(&key), window, cx);
        let options = ChangeOptions {
            reason: Some(Reason::ItemPress),
            event,
            source: None,
        };
        self.check(key, options, window, cx);
    }
}

// Where an arrow key moves among the radios that can take focus: like native
// radios, both axes, with Left and Right swapped right to left. Shift is left
// to the browser's own shortcuts, and Home and End move nothing.
fn radio_move(
    event: &KeyDownEvent,
    context: &RovingFocusMoveContext<SharedString>,
    r#loop: bool,
) -> Option<SharedString> {
    if event.keystroke.modifiers.shift {
        return None;
    }
    let step = radio_step(&Key::of(&event.keystroke), context.rtl)?;
    let index = context
        .items
        .iter()
        .position(|item| item.key == context.from.key)?;
    context
        .items
        .get(move_index(index as isize, step, context.items.len(), r#loop))
        .map(|item| item.key.clone())
}
