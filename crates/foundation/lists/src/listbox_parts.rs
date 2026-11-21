//! Port of `foundation/lists/listbox-parts.ts`: parts the listbox components
//! share, as part builders and the state they style from: what a form
//! submits, item registration, the check inside an item, groups and their
//! labels, separators, the empty and status messages of a filtered list, and
//! the arrow.
//!
//! The original's slot definitions are spread into each component's slots.
//! Here each is a function a component's part method calls with its
//! instance and what the part reads, and styles or extends what it returns.
//! Slot names are the original's: `group`, `group-label`, `separator`,
//! `empty`, `status` and `item-indicator`.

use std::fmt::Debug;
use std::time::Duration;

use gpui::{App, Context, Entity, SharedString, Styled, Window};
use ui_sdk_forms::{
    ElementInternals, FormValue, InternalsOptions, ValueInputOptions, ValueMissingKind, bind_internals,
    bind_value_input, value_missing_message,
};
use ui_sdk_kernel::dev::{self, Origin};
use ui_sdk_kernel::part::with_role;
use ui_sdk_kernel::roles::{HandledKey, Role};
use ui_sdk_kernel::{Aria, Instance, Live, Part, key_while_bound, name_of};
use ui_sdk_popups::{PopupArrowState, PopupHost, popup_arrow};

use crate::listbox_engine::{ItemMeta, ListData, ListEmptyState, ListStatusState, ListboxEngine};
use crate::listbox_value::ValueSelection;

/// What a control holding listbox values keeps for its form (`FormState`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormState {
    pub multiple: bool,
    pub disabled: bool,
    /// A disabled fieldset around it disables it.
    pub form_disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
}

/// Disabled by itself or by a fieldset around it (`isDisabled`).
pub fn is_disabled(disabled: bool, form_disabled: bool) -> bool {
    ui_sdk_forms::is_disabled(disabled, form_disabled)
}

/// Disabled or read-only: nothing can be cleared or removed (`isLocked`).
pub fn is_locked(disabled: bool, form_disabled: bool, read_only: bool) -> bool {
    is_disabled(disabled, form_disabled) || read_only
}

/// The form state every part of the control reflects (`formAttrs`):
/// `data-disabled`, `data-readonly` and `data-required`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FormAttrs {
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
}

/// The form state every part of the control reflects (`formAttrs`).
pub fn form_attrs(state: &FormState) -> FormAttrs {
    FormAttrs {
        disabled: is_disabled(state.disabled, state.form_disabled),
        read_only: state.read_only,
        required: state.required,
    }
}

/// The data list a part shows while there are no items (`noItems`).
pub fn no_items<T, X>() -> ListData<T, X> {
    ListData::empty()
}

/// The keys [`list_step`](crate::list_step) moves by (`listStepKeys`).
/// Control+N and Control+P step only on Apple platforms.
pub const LIST_STEP_KEYS: &[HandledKey] = &["ArrowDown", "ArrowUp", "PageDown", "PageUp", "ctrl+n", "ctrl+p"];

/// Warns when the control has no accessible name once its labels have had
/// time to connect, 300ms after its part first renders (`warnIfUnnamed`).
/// Not `check_name`, since a combobox takes no name from its content: only a
/// label, an `aria_label` or a labelled-by name it, which it reads off the
/// part of `slot`, with `key`, as it last rendered, so that part keeps its
/// name (`Part::keep_name`) from its first render. A part out of the page by
/// then isn't warned about. `named` checks names the part can't show, like a
/// label part, or the labels pointing at the control (`labelsOf`). Call it
/// as the part first renders; `instance` finds the component's instance.
/// Development builds only.
pub fn warn_if_unnamed<C: 'static>(
    instance: fn(&C) -> &Instance,
    slot: &'static str,
    key: Option<&'static str>,
    message: &'static str,
    named: impl Fn(&C, &App) -> bool + 'static,
    window: &Window,
    cx: &mut Context<C>,
) {
    if !dev::DEV {
        return;
    }
    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor().timer(Duration::from_millis(300)).await;
        this.update(cx, |this, cx| {
            let instance = instance(this);
            if named(this, cx) || name_of(instance, slot, key).is_none_or(|name| !name.is_empty()) {
                return;
            }
            dev::warn(message, Some(Origin::new(instance.name(), instance.id())));
        })
        .ok();
    })
    .detach();
}

/// An item's place among all items, for virtualized lists that render only
/// some (`positionAttrs`): `aria-posinset` and `aria-setsize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PositionAttrs {
    pub pos_in_set: usize,
    pub set_size: usize,
}

impl PositionAttrs {
    /// Puts them on a part.
    pub fn apply<S>(self, part: Part<S>) -> Part<S> {
        use gpui::StatefulInteractiveElement as _;
        part.aria_position_in_set(self.pos_in_set)
            .aria_size_of_set(self.set_size)
    }
}

/// An item's place among all items, when it's placed by `index`
/// (`positionAttrs`).
pub fn position_attrs<C, T, X>(index: Option<usize>, listbox: &ListboxEngine<C, T, X>, this: &C) -> Option<PositionAttrs>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    let index = index?;
    Some(PositionAttrs {
        pos_in_set: index + 1,
        set_size: listbox.entries(this).len(),
    })
}

/// Registers an item with the listbox as it renders, with the props it
/// renders with, so a changed label or disabled state applies at once
/// (`registerItem`).
pub fn register_item<C, T, X, S>(
    listbox: &ListboxEngine<C, T, X>,
    key: impl Into<SharedString>,
    meta: ItemMeta<T>,
    part: Part<S>,
    cx: &App,
) -> Part<S>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    listbox.bind_item(key, meta, part, cx)
}

/// Registers an item, and a selected item's label shows in the value once
/// it's known (`bindItem`).
pub fn bind_item<C, T, X, S>(
    this: &C,
    listbox: &ListboxEngine<C, T, X>,
    selection: &ValueSelection<C, T, X>,
    key: impl Into<SharedString>,
    meta: ItemMeta<T>,
    part: Part<S>,
    cx: &App,
) -> Part<S>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    let key = key.into();
    let selected = selection.is_selected(this, &key, meta.value.as_ref());
    let part = register_item(listbox, key, meta, part, cx);
    if selected {
        selection.queue_labels(cx);
    }
    part
}

/// What a group part styles from: `hidden` while none of its items match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OptionGroupState {
    pub hidden: bool,
}

/// A group is named by its label only while one is in the page, so an
/// unlabeled group points at no missing part (`groupLabelledBy`).
pub fn group_labelled_by(instance: &Instance, group: &str) -> Option<gpui::ElementId> {
    key_while_bound(instance, "group-label")
        .iter()
        .any(|key| key == group)
        .then(|| instance.element_id("group-label", Some(group)))
}

/// A group of items (`optionGroupPart`), named by its label, and hidden while
/// none of its items match. Hidden, it lays out as `display: none`, which a
/// style of the app's that sets `display` again overrides.
pub fn option_group_part<C, T, X>(
    instance: &Instance,
    key: &str,
    listbox: &ListboxEngine<C, T, X>,
    this: &C,
) -> Part<OptionGroupState>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    let hidden = !listbox.group_matches(this, key);
    let mut aria = Aria::new().hidden(hidden);
    if let Some(label) = group_labelled_by(instance, key) {
        aria = aria.labelled_by(label);
    }
    let part = Part::new(instance, "group", Some(key), OptionGroupState { hidden })
        .aria_role(Role::Group)
        .aria(aria);
    if hidden { part.hidden() } else { part }
}

/// What a separator part styles from: `hidden` while a composed list is
/// filtered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OptionSeparatorState {
    pub hidden: bool,
}

/// A line between items (`optionSeparatorPart`). A listbox may only hold
/// options and groups, so separators are left out for assistive technology
/// (role `none`). Composed separators hide while the list is filtered:
/// `filtering` is whether a query filters items that aren't data.
pub fn option_separator_part(instance: &Instance, key: Option<&str>, filtering: bool) -> Part<OptionSeparatorState> {
    let part = Part::new(instance, "separator", key, OptionSeparatorState { hidden: filtering })
        .aria_role(Role::None);
    if filtering { part.hidden() } else { part }
}

/// Shown while no item matches (`emptyPart`). It stays in the page, so its
/// message is announced when it appears: a polite, atomic status.
pub fn empty_part(instance: &Instance, state: ListEmptyState) -> Part<ListEmptyState> {
    Part::new(instance, "empty", None, state)
        .aria_role(Role::Status)
        .aria(Aria::new().live(Live::Polite).atomic(true))
}

/// A polite live region for loading and result counts (`statusPart`). Its
/// content is yours.
pub fn status_part(instance: &Instance, state: ListStatusState) -> Part<ListStatusState> {
    Part::new(instance, "status", None, state)
        .aria_role(Role::Status)
        .aria(Aria::new().live(Live::Polite).atomic(true))
}

// What the control submits: its keys, or the first.
fn form_value(state: &FormState, keys: &[SharedString]) -> FormValue {
    if state.multiple {
        FormValue::List(keys.to_vec())
    } else {
        keys.first().cloned().map_or(FormValue::None, FormValue::Text)
    }
}

/// Reports a control's value and validity through its internals, and names
/// its control from the labels pointing at it (`bindFormRoot`): its keys, or
/// the first, under its name, and a missing value while required and empty.
/// Call it each time the control renders and after it changes. Returns the
/// ARIA the labels give, for the element that stands for the control, like a
/// select's trigger. Reset and fieldsets come through `associate`, and focus
/// through the internals' focus handle.
pub fn bind_form_root(internals: &ElementInternals, state: &FormState, keys: &[SharedString]) -> Aria {
    bind_internals(
        internals,
        InternalsOptions {
            value: form_value(state, keys),
            name: state.name.clone(),
            value_missing: state.required && keys.is_empty(),
            message: value_missing_message(ValueMissingKind::Select),
            disabled: state.disabled,
            read_only: state.read_only,
            required: state.required,
            form: state.form.clone(),
            ..InternalsOptions::default()
        },
    );
    ui_sdk_forms::name_from_labels(internals.node())
}

/// What submits, resets and validates the value (`hiddenInputPart`): the
/// original's visually hidden textarea, reported through the control's
/// internals, its first key, or with `multiple` every key under its name,
/// which the original's one hidden input per value submits
/// (`valueInputPart`). Call it each time the control renders and after it
/// changes.
pub fn hidden_input_part(internals: &ElementInternals, state: &FormState, keys: &[SharedString]) {
    if state.multiple {
        bind_internals(
            internals,
            InternalsOptions {
                value: form_value(state, keys),
                name: state.name.clone(),
                value_missing: state.required && keys.is_empty(),
                message: value_missing_message(ValueMissingKind::Select),
                disabled: state.disabled,
                read_only: state.read_only,
                required: state.required,
                form: state.form.clone(),
                ..InternalsOptions::default()
            },
        );
        return;
    }
    bind_value_input(
        internals,
        ValueInputOptions {
            value: keys.first().cloned().unwrap_or_default(),
            name: state.name.clone(),
            required: state.required,
            disabled: state.disabled,
            read_only: state.read_only,
            form: state.form.clone(),
            missing: Some(ValueMissingKind::Select),
        },
    );
}

/// What the check inside an item styles from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemIndicatorState {
    pub selected: bool,
}

/// The check inside a selected item (`itemIndicatorPart`), by its item's
/// key, while it's selected, or always with `keep_mounted`. Decorative, so
/// it's left out for assistive technology. The original runs its enter
/// transition as it binds selected (`animateIn`); in gpui an element that
/// appears animates in with `with_animation`, which runs as it first
/// renders.
pub fn item_indicator_part(
    instance: &Instance,
    item: &str,
    selected: bool,
    keep_mounted: bool,
) -> Option<Part<ItemIndicatorState>> {
    if !keep_mounted && !selected {
        return None;
    }
    Some(
        Part::new(instance, "item-indicator", Some(item), ItemIndicatorState { selected })
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true)),
    )
}

/// A group's label (`groupLabelPart`), by its group's key, which names the
/// group while it's in the page. Give it its text as an `aria_label`, which
/// is the group's name.
pub fn group_label_part(instance: &Instance, group: &str) -> Part<()> {
    Part::new(instance, "group-label", Some(group), ()).map_el(|el| with_role(el, gpui::Role::Label))
}

/// The arrow of a list's popup (`arrowPart`): decorative, pointing at the
/// trigger once placed, with the popup's open state.
pub fn arrow_part<C: PopupHost>(this: &Entity<C>, cx: &App) -> Part<PopupArrowState> {
    popup_arrow(this, cx)
        .aria_role(Role::Group)
        .aria(Aria::new().hidden(true))
}
