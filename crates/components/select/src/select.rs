//! Port of `components/select/select.ts`: a button that opens a list of
//! options and shows the chosen one, like a native `<select>`. This module
//! is the public api: the options, the value, the reasons and what each part
//! styles from. The state and the api live in
//! [`select_api`](crate::select_api), keys and pointer presses in
//! [`select_keyboard`](crate::select_keyboard), and parts in
//! [`select_parts`](crate::select_parts), on the listbox engine and the
//! selection model Combobox shares.

use std::rc::Rc;

use gpui::{App, Hsla, Pixels, SharedString, Window};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentArgs, AgentError, AgentItem, AgentState, AgentSurface, AgentValue,
    ChangeCallback, ChangeDetails, ChangeOptions, DismissAttemptDetails, DismissAttemptReason, Reason,
};
use ui_sdk_kernel::modality::Modality;
use ui_sdk_lists::async_options::AsyncOptionsStatus;
use ui_sdk_lists::item_icon::{ItemIconProps, ItemIconState};
use ui_sdk_lists::listbox_engine::{ItemKey, ListData, ListEmptyState, ListFilter, ListHighlightDetails, ListStatusState};
use ui_sdk_lists::listbox_parts::ItemIndicatorState;
use ui_sdk_lists::listbox_search::{ListAnnouncementFn, ListItemTextState, LoadMoreFn};
use ui_sdk_lists::listbox_select_all::ListSelectAllState;
use ui_sdk_lists::listbox_value::ListValue;
use ui_sdk_motion::{PresenceState, PresenceTiming};
use ui_sdk_native::TextFieldOptions;
use ui_sdk_platform::{Align, Anchor, Placement, PositionProps, Side};
use ui_sdk_popups::{
    DismissAttemptCallback, Modal, OpenChangeCompleteCallback, PopupArrowState, Presentation,
    PresentationSetting, SheetPartState,
};

use crate::select_api::Select;

/// Why a select's value wants to change: a press, Enter or Space on an item,
/// or Alt+ArrowUp (`ItemPress`), typing on the closed trigger
/// (`ListNavigation`), the clear button (`ClearPress`), a form reset
/// (`Reset`) or restore (`Restore`), and a call from the app (`Imperative`).
pub type SelectValueChangeReason = Reason;

/// Why the text in a select's search field wants to change: typing
/// (`InputChange`), or closing the list, which empties it, with the reason it
/// closed: `ItemPress`, `TriggerPress`, `OutsidePress`, `EscapeKey`,
/// `ClosePress`, `FocusOut`, `WindowBlur`, `Back`, `Swipe` or `Imperative`.
pub type SelectQueryChangeReason = Reason;

/// Why a select opened or closed: its trigger (`TriggerPress`), an item
/// chosen (`ItemPress`), a press outside (`OutsidePress`), Escape
/// (`EscapeKey`), a Close part (`ClosePress`), Tab or focus leaving
/// (`FocusOut`), the window losing focus (`WindowBlur`), the back gesture
/// (`Back`), a sheet swiped away (`Swipe`), or a call from the app
/// (`Imperative`).
pub type SelectOpenChangeReason = Reason;

/// The dismissals `dismissible` refuses that `on_dismiss_attempt` hears
/// about: `EscapeKey`, `OutsidePress`, `Back` and `Swipe`.
pub type SelectDismissReason = DismissAttemptReason;

/// Options for every write method on a select.
pub type SelectChangeOptions = ChangeOptions;

/// The value of a select (`SelectValue`): one item's value or nothing,
/// `Single`, or with `multiple` a list, `Multiple`.
pub type SelectValue<T> = ListValue<T>;

/// `get_more_label`: the words for the labels past `max_labels`, from how
/// many there are.
pub type MoreLabelFn = Rc<dyn Fn(usize) -> SharedString>;

/// `item_to_value`: the string an item's value submits as and is identified
/// by.
pub type ItemToValueFn<T> = Rc<dyn Fn(&T) -> SharedString>;

/// `item_to_label`: the text a value shows in the trigger.
pub type ItemToLabelFn<T> = Rc<dyn Fn(&T) -> SharedString>;

/// `is_item_equal_to_value`: whether an item's value is the selected value.
pub type ItemEqualFn<T> = Rc<dyn Fn(&T, &T) -> bool>;

/// `on_highlight`: the value the highlight moved to, or `None`, and where it
/// is. It runs inside the select's update, so it may update any entity but
/// the select.
pub type HighlightCallback<T> = Rc<dyn Fn(Option<&T>, &ListHighlightDetails, &mut Window, &mut App)>;

/// The options `Select` takes (`SelectOptions`).
pub struct SelectOptions<T> {
    /// The selected value. Pass it to control the select: `Single(None)`
    /// selects nothing, and with `multiple` it's `Multiple`.
    pub value: Option<SelectValue<T>>,
    /// The value selected at first, when uncontrolled. A form reset returns
    /// to it. Without one, nothing: `None`, or an empty list with `multiple`.
    pub default_value: Option<SelectValue<T>>,
    /// Runs when a user, a form reset or a call wants to change the value.
    /// `details.reason` is one of [`SelectValueChangeReason`]'s, and
    /// `details.cancel()` stops it. It runs inside the select's update, so it
    /// may update any entity but the select.
    pub on_value_change: Option<ChangeCallback<SelectValue<T>>>,
    /// Whether the list is open. Pass it to control it.
    pub open: Option<bool>,
    /// Whether the list starts open, when uncontrolled.
    pub default_open: bool,
    /// Runs when the list wants to open or close. `details.reason` is one of
    /// [`SelectOpenChangeReason`]'s, and `details.cancel()` stops it. It runs
    /// inside the select's update.
    pub on_open_change: Option<ChangeCallback<bool>>,
    /// Runs once the list has finished opening or closing, after any
    /// animation.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Whether several items can be selected. The value is then a list, and
    /// the list stays open as items toggle.
    pub multiple: bool,
    /// The items as data. The content's state hands them back for rendering,
    /// and they let the trigger show labels and find items by typing before
    /// the list has ever opened.
    pub items: Option<ListData<T>>,
    /// Items shown instead of `items` while the search field is empty, like
    /// recent choices. Groups work, like a group labelled "Recent". Needs
    /// `items` and an input part.
    pub empty_items: Option<ListData<T>>,
    /// The text in the search field, which filters the list. Pass it to
    /// control it. Closing the list empties it.
    pub query: Option<SharedString>,
    /// The search field's text at first, when uncontrolled.
    pub default_query: SharedString,
    /// Runs when the search field's text wants to change: typing, or closing
    /// the list, which empties it. `details.reason` is one of
    /// [`SelectQueryChangeReason`]'s. It runs inside the select's update.
    pub on_query_change: Option<ChangeCallback<SharedString>>,
    /// How an input part narrows the list: `Contains`, `StartsWith`, a
    /// function of the item and the text, whose score also sorts data
    /// items, or `Off` to keep every item, for results a server already
    /// filtered. Defaults to `Contains`.
    pub filter: Option<ListFilter<T>>,
    /// Whether results are loading or failed, for the status and empty
    /// parts. The list reports it's busy while loading.
    pub status: AsyncOptionsStatus,
    /// Runs when the highlight or scrolling reaches the last item, so the
    /// next page can load. Not again for the same items, unless the last
    /// load failed.
    pub on_load_more: Option<LoadMoreFn>,
    /// What screen readers hear after typing pauses in the search field,
    /// when no status part is rendered. `None` from it says nothing. Defaults
    /// to "3 results", "No results" or "Loading".
    pub announcement: Option<ListAnnouncementFn>,
    /// What the value shows while nothing is selected.
    pub placeholder: Option<SharedString>,
    /// The string an item's value submits as and is identified by, for
    /// values of your own type. Strings pass through.
    pub item_to_value: Option<ItemToValueFn<T>>,
    /// The text a value shows in the trigger, for values whose item isn't
    /// rendered, like async results.
    pub item_to_label: Option<ItemToLabelFn<T>>,
    /// Whether an item's value is the selected value, for values that aren't
    /// equal but stand for the same item.
    pub is_item_equal_to_value: Option<ItemEqualFn<T>>,
    /// Runs when the highlight moves, with the item's index among the
    /// visible items, so a virtualized list can scroll to it.
    pub on_highlight: Option<HighlightCallback<T>>,
    /// Whether arrow keys wrap from the last item to the first. Defaults to
    /// true.
    pub r#loop: bool,
    /// Keeps focus on the trigger while the list is open, as on a native
    /// select: arrow keys, Home, End and typing move the highlight, and the
    /// trigger points at the highlighted item as its active descendant. A
    /// search field in the list still takes focus.
    pub virtual_focus: bool,
    /// `Pointer`: an outside press only closes the list. `True`: also blocks
    /// the rest of the window, which doesn't scroll. `False`: outside presses
    /// reach what they hit. Defaults to `Pointer`, or `True` while it
    /// presents as a sheet.
    pub modal: Option<Modal>,
    /// How the list presents. `Sheet` slides it up from the bottom of the
    /// window, where it can be swiped away, and `Auto` does that on small
    /// touch screens. Decided each time it opens. Defaults to `Default`, a
    /// popover.
    pub presentation: Option<PresentationSetting>,
    /// Disables the select like a native one: it can't be focused or
    /// changed, and the value isn't submitted.
    pub disabled: bool,
    /// Keeps the select focusable and its value submitted, but stops changes.
    pub read_only: bool,
    /// Blocks form submission until a value is selected, with the browser's
    /// own message.
    pub required: bool,
    /// The name the value submits under. Without one, the select submits
    /// nothing.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when the select isn't inside that form.
    pub form: Option<SharedString>,
    /// A hint for autofill, like `"country"`.
    pub auto_complete: Option<SharedString>,
    /// Whether Escape, an outside press, focus or the window leaving, the
    /// back gesture and a sheet's swipe can close the list. Defaults to true.
    pub dismissible: bool,
    /// Runs when `dismissible` refused a dismissal made on purpose:
    /// `details.reason` is one of [`SelectDismissReason`]'s.
    pub on_dismiss_attempt: Option<DismissAttemptCallback>,
    /// The search field's own options, what the original's input element
    /// takes as attributes, like its placeholder and its look. Its typing is
    /// the select's.
    pub search: TextFieldOptions,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl<T> Default for SelectOptions<T> {
    fn default() -> Self {
        Self {
            value: None,
            default_value: None,
            on_value_change: None,
            open: None,
            default_open: false,
            on_open_change: None,
            on_open_change_complete: None,
            multiple: false,
            items: None,
            empty_items: None,
            query: None,
            default_query: SharedString::default(),
            on_query_change: None,
            filter: None,
            status: AsyncOptionsStatus::Idle,
            on_load_more: None,
            announcement: None,
            placeholder: None,
            item_to_value: None,
            item_to_label: None,
            is_item_equal_to_value: None,
            on_highlight: None,
            r#loop: true,
            virtual_focus: false,
            modal: None,
            presentation: None,
            disabled: false,
            read_only: false,
            required: false,
            name: None,
            form: None,
            auto_complete: None,
            dismissible: true,
            on_dismiss_attempt: None,
            search: TextFieldOptions::default(),
            debug: false,
        }
    }
}

impl<T: Clone> Clone for SelectOptions<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            default_value: self.default_value.clone(),
            on_value_change: self.on_value_change.clone(),
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            multiple: self.multiple,
            items: self.items.clone(),
            empty_items: self.empty_items.clone(),
            query: self.query.clone(),
            default_query: self.default_query.clone(),
            on_query_change: self.on_query_change.clone(),
            filter: self.filter.clone(),
            status: self.status,
            on_load_more: self.on_load_more.clone(),
            announcement: self.announcement.clone(),
            placeholder: self.placeholder.clone(),
            item_to_value: self.item_to_value.clone(),
            item_to_label: self.item_to_label.clone(),
            is_item_equal_to_value: self.is_item_equal_to_value.clone(),
            on_highlight: self.on_highlight.clone(),
            r#loop: self.r#loop,
            virtual_focus: self.virtual_focus,
            modal: self.modal,
            presentation: self.presentation,
            disabled: self.disabled,
            read_only: self.read_only,
            required: self.required,
            name: self.name.clone(),
            form: self.form.clone(),
            auto_complete: self.auto_complete.clone(),
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            search: self.search.clone(),
            debug: self.debug,
        }
    }
}

impl<T> SelectOptions<T> {
    pub fn value(mut self, value: impl Into<SelectValue<T>>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn default_value(mut self, value: impl Into<SelectValue<T>>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&SelectValue<T>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
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

    pub fn multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    pub fn items(mut self, items: impl Into<ListData<T>>) -> Self {
        self.items = Some(items.into());
        self
    }

    pub fn empty_items(mut self, items: impl Into<ListData<T>>) -> Self {
        self.empty_items = Some(items.into());
        self
    }

    pub fn query(mut self, query: impl Into<SharedString>) -> Self {
        self.query = Some(query.into());
        self
    }

    pub fn default_query(mut self, query: impl Into<SharedString>) -> Self {
        self.default_query = query.into();
        self
    }

    pub fn on_query_change(
        mut self,
        callback: impl Fn(&SharedString, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_query_change = Some(Rc::new(callback));
        self
    }

    pub fn filter(mut self, filter: ListFilter<T>) -> Self {
        self.filter = Some(filter);
        self
    }

    pub fn status(mut self, status: AsyncOptionsStatus) -> Self {
        self.status = status;
        self
    }

    pub fn on_load_more(mut self, callback: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_load_more = Some(Rc::new(callback));
        self
    }

    pub fn announcement(
        mut self,
        announcement: impl Fn(&ui_sdk_lists::listbox_search::ListAnnouncement) -> Option<SharedString> + 'static,
    ) -> Self {
        self.announcement = Some(Rc::new(announcement));
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn item_to_value(mut self, item_to_value: impl Fn(&T) -> SharedString + 'static) -> Self {
        self.item_to_value = Some(Rc::new(item_to_value));
        self
    }

    pub fn item_to_label(mut self, item_to_label: impl Fn(&T) -> SharedString + 'static) -> Self {
        self.item_to_label = Some(Rc::new(item_to_label));
        self
    }

    pub fn is_item_equal_to_value(mut self, equal: impl Fn(&T, &T) -> bool + 'static) -> Self {
        self.is_item_equal_to_value = Some(Rc::new(equal));
        self
    }

    pub fn on_highlight(
        mut self,
        callback: impl Fn(Option<&T>, &ListHighlightDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_highlight = Some(Rc::new(callback));
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
        self
    }

    pub fn virtual_focus(mut self, virtual_focus: bool) -> Self {
        self.virtual_focus = virtual_focus;
        self
    }

    pub fn modal(mut self, modal: impl Into<Modal>) -> Self {
        self.modal = Some(modal.into());
        self
    }

    pub fn presentation(mut self, presentation: PresentationSetting) -> Self {
        self.presentation = Some(presentation);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn form(mut self, form: impl Into<SharedString>) -> Self {
        self.form = Some(form.into());
        self
    }

    pub fn auto_complete(mut self, auto_complete: impl Into<SharedString>) -> Self {
        self.auto_complete = Some(auto_complete.into());
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(
        mut self,
        callback: impl Fn(&DismissAttemptDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
        self
    }

    pub fn search(mut self, search: TextFieldOptions) -> Self {
        self.search = search;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props of the trigger (`SelectTriggerProps`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectTriggerProps {
    /// Stops the trigger from opening the list. A disabled trigger can't be
    /// focused or pressed, like a native button.
    pub disabled: bool,
}

impl SelectTriggerProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The props of the value (`SelectValueProps`).
#[derive(Clone, Default)]
pub struct SelectValueProps {
    /// What shows while nothing is selected. Overrides the select's
    /// `placeholder`.
    pub placeholder: Option<SharedString>,
    /// With `multiple`, how many labels show before the rest are counted, as
    /// in "Apple, Banana and 2 more", joined the way the app's language joins
    /// lists, or in English with the default `get_more_label`.
    pub max_labels: Option<usize>,
    /// The words for the labels past `max_labels`. Defaults to "2 more".
    pub get_more_label: Option<MoreLabelFn>,
}

impl SelectValueProps {
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn max_labels(mut self, max_labels: usize) -> Self {
        self.max_labels = Some(max_labels);
        self
    }

    pub fn get_more_label(mut self, label: impl Fn(usize) -> SharedString + 'static) -> Self {
        self.get_more_label = Some(Rc::new(label));
        self
    }
}

impl std::fmt::Debug for SelectValueProps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectValueProps")
            .field("placeholder", &self.placeholder)
            .field("max_labels", &self.max_labels)
            .field("get_more_label", &self.get_more_label.as_ref().map(|_| ".."))
            .finish()
    }
}

/// The props of the content (`SelectContentProps`).
#[derive(Clone, Default)]
pub struct SelectContentProps {
    /// Where it goes beside the trigger. The side defaults to the bottom,
    /// the alignment to the start, the side offset to 4 and the align offset
    /// to 0.
    pub position: PositionProps,
    /// Whether the list overlaps the trigger so the selected item sits on
    /// it, like a native select on macOS. Falls back to opening below when
    /// there isn't room, and for touch.
    pub align_item_with_trigger: bool,
    /// Whether the list stays in the page, hidden, while closed. Its content
    /// keeps its state between opens.
    pub keep_mounted: bool,
    /// How it animates in and out, the transitions its CSS would declare.
    pub timing: Option<PresenceTiming>,
    /// The dim behind it while it presents as a sheet, at its darkest.
    /// Defaults to black at 0.2.
    pub sheet_backdrop: Option<Hsla>,
}

impl SelectContentProps {
    pub fn side(mut self, side: Side) -> Self {
        self.position.side = Some(side);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.position.align = Some(align);
        self
    }

    pub fn side_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.position.side_offset = Some(offset.into());
        self
    }

    pub fn align_offset(mut self, offset: impl Into<Pixels>) -> Self {
        self.position.align_offset = Some(offset.into());
        self
    }

    pub fn collision_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.position.collision_padding = Some(padding.into());
        self
    }

    pub fn arrow_padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.position.arrow_padding = Some(padding.into());
        self
    }

    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.position.anchor = Some(anchor.into());
        self
    }

    pub fn align_item_with_trigger(mut self, align: bool) -> Self {
        self.align_item_with_trigger = align;
        self
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn timing(mut self, timing: PresenceTiming) -> Self {
        self.timing = Some(timing);
        self
    }

    pub fn sheet_backdrop(mut self, color: impl Into<Hsla>) -> Self {
        self.sheet_backdrop = Some(color.into());
        self
    }
}

/// The props of an item (`SelectItemProps`).
#[derive(Clone, Debug, PartialEq)]
pub struct SelectItemProps<T> {
    /// What the value becomes when the item is chosen.
    pub value: T,
    /// The text the trigger shows and typing finds, when it isn't the text
    /// the item shows. Not the accessible name: `text`, or an `aria_label`
    /// of your own, is that.
    pub label: Option<SharedString>,
    /// The text the item shows, which typing and the search find, the
    /// trigger shows once it's chosen, and a screen reader reads it as. gpui
    /// can't read text back from what the item renders, so give it here.
    pub text: Option<SharedString>,
    /// More words the search matches, like "team" for a plan called Pro.
    pub keywords: Vec<SharedString>,
    /// Keeps the item from being chosen. The keyboard and the pointer skip
    /// it, and it reports `aria-disabled`.
    pub disabled: bool,
    /// The item's position among all items, for virtualized lists that
    /// render only some. It reports its place in the set.
    pub index: Option<usize>,
    /// The group it's in, by the key its group part has.
    pub group: Option<SharedString>,
    /// It's a link, which the app follows in its own press on the part
    /// (`on_press`): Enter clicks it, so it follows itself and the press
    /// chooses it.
    pub link: bool,
}

impl<T> SelectItemProps<T> {
    pub fn new(value: impl Into<T>) -> Self {
        Self {
            value: value.into(),
            label: None,
            text: None,
            keywords: Vec::new(),
            disabled: false,
            index: None,
            group: None,
            link: false,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn keywords<K: Into<SharedString>>(mut self, keywords: impl IntoIterator<Item = K>) -> Self {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn index(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }

    pub fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn link(mut self, link: bool) -> Self {
        self.link = link;
        self
    }
}

/// The props of the check inside an item (`SelectItemIndicatorProps`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectItemIndicatorProps {
    /// Whether the indicator stays in the page while its item isn't
    /// selected, so it can be styled or animated in both states.
    pub keep_mounted: bool,
}

impl SelectItemIndicatorProps {
    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }
}

/// The props of the clear button (`SelectClearProps`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectClearProps {
    /// Whether the clear button stays in the page while nothing is selected.
    pub keep_mounted: bool,
}

impl SelectClearProps {
    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }
}

/// What the `trigger` part styles from (`SelectTriggerState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectTriggerState {
    /// Whether the list is open (`data-open`, `data-closed`).
    pub open: bool,
    /// Whether the select is disabled, by itself, its trigger's `disabled`
    /// or a fieldset (`data-disabled`).
    pub disabled: bool,
    /// Whether the select keeps its value but refuses changes, from its
    /// `read_only` option (`data-readonly`).
    pub read_only: bool,
    /// Whether a value must be selected before the form submits, from its
    /// `required` option (`data-required`).
    pub required: bool,
    /// Whether nothing is selected, so the placeholder shows
    /// (`data-placeholder`).
    pub placeholder: bool,
}

/// What the `value` part styles from (`SelectValueState`).
#[derive(Clone, Debug, PartialEq)]
pub struct SelectValueState<T> {
    /// The selected value.
    pub value: SelectValue<T>,
    /// The selected item's label, the labels joined with commas for several,
    /// or with `max_labels` as a list in the app's language, like "Apple,
    /// Banana and 2 more". Empty for none.
    pub label: SharedString,
    /// Each selected item's label, in the order they were selected.
    pub labels: Rc<[SharedString]>,
    /// Whether nothing is selected, so the placeholder shows
    /// (`data-placeholder`).
    pub placeholder: bool,
    /// What the value shows: the label, or the placeholder while nothing is
    /// selected, as the original's value renders without children of its
    /// own. Render it with gpui's `text!`, or your own words from the rest.
    pub text: SharedString,
}

/// What the `content` and `list` parts style from (`SelectContentState`).
#[derive(Clone, Debug, PartialEq)]
pub struct SelectContentState<T> {
    /// Whether the list is open. False while it animates out.
    pub open: bool,
    /// How it presents this time it's open (`data-presentation`).
    pub presentation: Presentation,
    /// Whether several items can be selected.
    pub multiple: bool,
    /// The data items that match the search, for rendering them, like a
    /// virtualized list. Empty without `items`.
    pub items: ListData<T>,
    /// Whether results are loading or failed.
    pub status: AsyncOptionsStatus,
    /// Whether the keyboard or a pointer is in use, for styling the
    /// highlight (`data-modality`). `None` while it isn't shown.
    pub modality: Option<Modality>,
    /// Its enter and exit (`data-starting-style`, `data-ending-style`).
    pub presence: PresenceState,
    /// Where it's placed, as of the last frame: its side, alignment, room
    /// and the trigger's size (`data-side`, `--available-height`,
    /// `--anchor-width`).
    pub placement: Option<Placement>,
    /// Kept in the page while closed, it renders hidden.
    pub hidden: bool,
    /// The sheet it presents as, while it does.
    pub sheet: Option<SheetPartState>,
}

/// What the `item` part styles from (`SelectItemState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectItemState {
    /// Whether the item has keyboard focus or is under the pointer
    /// (`data-highlighted`).
    pub highlighted: bool,
    /// Whether its value is in the select's value, compared with
    /// `is_item_equal_to_value` when you pass one (`data-selected`).
    pub selected: bool,
    /// Whether the item's own `disabled` prop is set. It can't be highlighted
    /// or chosen (`data-disabled`).
    pub disabled: bool,
}

/// What the `input` part styles from (`SelectInputState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectInputState {
    /// Whether the list is open.
    pub open: bool,
    /// Whether the select is disabled, by itself or a fieldset. A read-only
    /// select's search field stays enabled.
    pub disabled: bool,
}

/// What the `empty` part styles from (`SelectEmptyState`).
pub type SelectEmptyState = ListEmptyState;

/// What the `status` part styles from (`SelectStatusState`).
pub type SelectStatusState = ListStatusState;

/// What the `item-text` part styles from (`SelectItemTextState`).
pub type SelectItemTextState = ListItemTextState;

/// The props the `item-icon` part takes (`SelectItemIconProps`).
pub type SelectItemIconProps = ItemIconProps;

/// What the `item-icon` and `value-icon` parts style from
/// (`SelectItemIconState`).
pub type SelectItemIconState = ItemIconState;

/// What the `select-all` part styles from (`SelectSelectAllState`).
pub type SelectSelectAllState = ListSelectAllState;

/// What the `item-indicator` part styles from (`SelectItemIndicatorState`):
/// whether its item is selected. While it isn't, the indicator isn't shown
/// unless `keep_mounted` is set.
pub type SelectItemIndicatorState = ItemIndicatorState;

/// What the `clear` part styles from (`SelectClearState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectClearState {
    /// Whether the select is disabled or read-only, so there's nothing it can
    /// clear (`data-disabled`).
    pub disabled: bool,
}

/// What the `arrow` part styles from (`SelectArrowState`): whether the list
/// is open, and where the arrow goes once placed.
pub type SelectArrowState = PopupArrowState;

/// `agent: { actions: ["setOpen", "setValue", "clear", "setQuery"], closed:
/// true }` and `controlled: ["open", "value", "query"]`, with `open`, `close`
/// and `toggle`, which `setOpen` covers, unlisted. An agent names items by
/// their keys, as the state shows them.
impl<T: ItemKey + Clone + PartialEq + std::fmt::Debug + 'static> Agent for Select<T> {
    const SURFACE: AgentSurface = AgentSurface::new().closed(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", |select, args, window, cx| {
            let open = args.boolean(0)?;
            Ok(select.set_open(open, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setValue", |select, args, window, cx| {
            let value = select.agent_value(args)?;
            Ok(select.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |select, _args, window, cx| {
            Ok(select.clear(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setQuery", |select, args, window, cx| {
            let query = args.string(0)?;
            Ok(select.set_query(query, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("open", |select, _args, window, cx| {
            select.set_open(true, ChangeOptions::default(), window, cx);
            Ok(AgentValue::from(ui_sdk_kernel::PENDING))
        }),
        AgentAction::unlisted("close", |select, _args, window, cx| {
            Ok(select.set_open(false, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |select, _args, window, cx| {
            Ok(select.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["open", "value", "query"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        let keys = self.selection.keys(self);
        let value = if self.multiple {
            AgentValue::from(keys.iter().map(|key| key.to_string()).collect::<Vec<_>>())
        } else {
            keys.first().map_or(AgentValue::Null, |key| AgentValue::from(key.to_string()))
        };
        let mut state = AgentState::new()
            .with("open", self.is_open())
            .with("value", value)
            .with("query", self.query().to_string())
            .with("multiple", self.multiple)
            .with("disabled", self.is_disabled())
            .with("readOnly", self.read_only)
            .with("required", self.required);
        if let Some(placeholder) = &self.placeholder {
            state.set("placeholder", placeholder.to_string());
        }
        state
    }

    fn agent_items(&self, _cx: &App) -> Option<Vec<AgentItem>> {
        Some(
            self.listbox
                .entries(self)
                .iter()
                .filter(|entry| !entry.pinned)
                .map(|entry| AgentItem::new(entry.key.clone(), entry.label.clone()).disabled(entry.disabled))
                .collect(),
        )
    }
}

impl<T: ItemKey + Clone + PartialEq + std::fmt::Debug + 'static> Select<T> {
    // The value an agent's keys name: one key or none, or a list with
    // `multiple`. A key no item has is refused, since the value is always
    // one of the items.
    fn agent_value(&self, args: &AgentArgs) -> Result<SelectValue<T>, AgentError> {
        let value_of = |key: &SharedString| {
            self.listbox
                .value_of(self, key)
                .ok_or_else(|| AgentError::refused(format!("No item has the value \"{key}\".")))
        };
        if self.multiple {
            let keys = args.list(0)?;
            return Ok(ListValue::Multiple(keys.iter().map(value_of).collect::<Result<_, _>>()?));
        }
        if args.is_missing(0) {
            return Ok(ListValue::Single(None));
        }
        Ok(ListValue::Single(Some(value_of(&args.string(0)?)?)))
    }
}
