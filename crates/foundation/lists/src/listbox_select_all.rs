//! Port of `foundation/lists/listbox-select-all.ts`: a "select all" option
//! for lists with `multiple`, shared by Select and Combobox. It selects
//! every item that matches what's typed and can be chosen, in its group or
//! in the whole list, and unselects them once they all are. It reads as
//! checked, unchecked or indeterminate, like a checkbox over the items.

use std::collections::HashSet;
use std::fmt::Debug;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, InteractiveElement, MouseMoveEvent, SharedString, StatefulInteractiveElement,
    Window,
};
use ui_sdk_focus::hold_focus;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, Checked, FocusMode, InputEvent, Instance, Part};

use crate::listbox_engine::{ItemMeta, ListboxEngine};
use crate::listbox_keys::{SELECT_ALL, option_id};
use crate::listbox_value::ValueSelection;

/// What a select-all option styles from: how many of its items are
/// selected (`ListSelectAllState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListSelectAllState {
    /// Whether every item it covers is selected.
    pub checked: bool,
    /// Whether some of its items are selected, but not all.
    pub indeterminate: bool,
    /// Whether it's highlighted, by the keyboard or the pointer.
    pub highlighted: bool,
    /// Whether it can't be used: nothing it covers can be chosen, or
    /// choosing them all would pass `max_selected`.
    pub disabled: bool,
}

/// The group a select-all key covers, or `None` for the whole list.
pub fn group_of(key: &str) -> Option<&str> {
    if key == SELECT_ALL {
        return None;
    }
    key.get(SELECT_ALL.len() + 1..)
}

/// What the select-all parts read from a component (`SelectAllApi`): its
/// list, its value, and how many values may be selected, with
/// `max_selected`.
pub struct SelectAll<'a, C: 'static, T: 'static, X: 'static = ()> {
    pub listbox: &'a ListboxEngine<C, T, X>,
    pub selection: &'a ValueSelection<C, T, X>,
    pub limit: Option<usize>,
}

impl<'a, C, T, X> SelectAll<'a, C, T, X>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    // The items a select-all option covers, how many are selected, and
    // whether choosing them all fits the limit.
    fn coverage(&self, this: &C, key: &str) -> (Vec<SharedString>, usize, bool) {
        let keys = self.listbox.selectable(this, group_of(key));
        let selected = keys
            .iter()
            .filter(|item| self.selection.is_selected(this, item, None))
            .count();
        let fits = self
            .limit
            .is_none_or(|max| self.selection.keys(this).len() + keys.len() - selected <= max);
        (keys, selected, fits)
    }

    /// The state of a select-all option (`selectAllState`).
    pub fn state(&self, this: &C, key: &str, highlighted: Option<&SharedString>) -> ListSelectAllState {
        let (keys, selected, fits) = self.coverage(this, key);
        let checked = !keys.is_empty() && selected == keys.len();
        ListSelectAllState {
            checked,
            indeterminate: selected > 0 && !checked,
            highlighted: highlighted.is_some_and(|highlighted| highlighted == key),
            disabled: keys.is_empty() || (!checked && !fits),
        }
    }

    /// The value after pressing a select-all option: its items added, or
    /// removed once they're all selected. `None` when it can't be used
    /// (`toggledAll`). The labels of the items it adds outlive the list,
    /// for chips and the trigger.
    pub fn toggled(&self, this: &C, key: &str) -> Option<Vec<T>> {
        let (keys, selected, fits) = self.coverage(this, key);
        if keys.is_empty() {
            return None;
        }
        if selected == keys.len() {
            let removed: HashSet<SharedString> = keys.into_iter().collect();
            return Some(
                self.selection
                    .values(this)
                    .into_iter()
                    .filter(|value| !removed.contains(&self.listbox.key_of(this, value)))
                    .collect(),
            );
        }
        if !fits {
            return None;
        }
        let added: Vec<SharedString> = keys
            .into_iter()
            .filter(|item| !self.selection.is_selected(this, item, None))
            .collect();
        let mut values = self.selection.values(this);
        for item in &added {
            if let Some(entry) = self.listbox.find(this, Some(item))
                && let Some(value) = entry.value.clone()
            {
                self.listbox.remember(item.clone(), entry.label, value, None);
            }
            if let Some(value) = self.listbox.value_of(this, item) {
                values.push(value);
            }
        }
        Some(values)
    }
}

/// How a component's options take focus, which its select-all option takes
/// too (`OptionFocus`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OptionFocus {
    /// Real focus moves from option to option.
    Roving,
    /// Focus stays on the field, which points an active descendant at the
    /// highlighted option, until a screen reader moves focus into the list.
    #[default]
    ActiveDescendant,
}

/// Picks a select-all option, inside the component's update.
pub type SelectAllChooseFn<C> = Rc<dyn Fn(&mut C, &SharedString, InputEvent, &mut Window, &mut Context<C>)>;

/// Hears the pointer move on a select-all option, inside the component's
/// update.
pub type SelectAllHoverFn<C> = Rc<dyn Fn(&mut C, &SharedString, &MouseMoveEvent, &mut Window, &mut Context<C>)>;

/// What a select-all option renders with.
pub struct SelectAllPartProps<C: 'static> {
    /// Its key, `select_all_key(group)`.
    pub key: SharedString,
    pub state: ListSelectAllState,
    /// Whether the list holds several values. Only then is it in the page.
    pub multiple: bool,
    /// Whether a screen reader moved real focus into the list.
    pub focus_in_list: bool,
    /// The text it shows, for typeahead (`textOf`).
    pub text: Option<SharedString>,
    /// Its press: the component checks the value can change and calls its
    /// own `choose`, which asks [`SelectAll::toggled`].
    pub choose: SelectAllChooseFn<C>,
    pub hover: SelectAllHoverFn<C>,
}

/// The option itself (`selectAllPart`): checked, unchecked or mixed, never
/// filtered and never counted as a result. It takes focus as the
/// component's items do: real focus, or an active descendant until a screen
/// reader moves focus into the list. A press on it leaves focus where it
/// is. `None` without `multiple`.
pub fn select_all_part<C, T, X>(
    entity: &Entity<C>,
    instance: &Instance,
    listbox: &ListboxEngine<C, T, X>,
    focus: OptionFocus,
    props: SelectAllPartProps<C>,
    cx: &App,
) -> Option<Part<ListSelectAllState>>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
    X: Clone + Default + 'static,
{
    if !props.multiple {
        return None;
    }
    let SelectAllPartProps {
        key,
        state,
        focus_in_list,
        text,
        choose,
        hover,
        ..
    } = props;
    let checked = if state.indeterminate {
        Checked::Indeterminate
    } else {
        Checked::from(state.checked)
    };
    let mut part = Part::new(instance, "select-all", group_of(&key), state)
        .with_element_id(option_id(instance.id(), &key))
        .aria_role(Role::Option);
    part = if focus == OptionFocus::Roving || focus_in_list {
        let handle = listbox.focus_handle(&key, cx);
        part.focus_mode(FocusMode::Roving { tab_stop: false }, Some(&handle), false)
    } else {
        part.focus_mode(FocusMode::ActiveDescendant, None, false)
    };
    let hovered = entity.downgrade();
    let pressed = entity.downgrade();
    let hover_key = key.clone();
    let press_key = key.clone();
    let part = part
        .aria_selected(state.checked)
        .aria(Aria::new().checked(checked).disabled(state.disabled))
        .on_any_mouse_down(hold_focus)
        .on_mouse_move(move |event, window, cx| {
            hovered
                .update(cx, |this, cx| hover(this, &hover_key, event, window, cx))
                .ok();
        })
        .on_press(move |event, window, cx| {
            pressed
                .update(cx, |this, cx| choose(this, &press_key, event, window, cx))
                .ok();
        });
    Some(listbox.bind_item(key, ItemMeta::built_in(text), part, cx))
}

/// Select-all as a feature (`selectAllFeature`): the option
/// ([`SelectAllFeature::part`]), and what pressing it does, the value
/// [`SelectAll::toggled`] gives, which the component asks for once it has
/// checked the value can change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectAllFeature {
    pub focus: OptionFocus,
}

/// Select-all for a component whose options take focus as `focus` says.
pub fn select_all_feature(focus: OptionFocus) -> SelectAllFeature {
    SelectAllFeature { focus }
}

impl SelectAllFeature {
    /// The feature's `select-all` part.
    pub fn part<C, T, X>(
        &self,
        entity: &Entity<C>,
        instance: &Instance,
        listbox: &ListboxEngine<C, T, X>,
        props: SelectAllPartProps<C>,
        cx: &App,
    ) -> Option<Part<ListSelectAllState>>
    where
        C: 'static,
        T: Clone + PartialEq + Debug + 'static,
        X: Clone + Default + 'static,
    {
        select_all_part(entity, instance, listbox, self.focus, props, cx)
    }
}
