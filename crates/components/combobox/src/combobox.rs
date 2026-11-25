//! Port of `components/combobox/combobox.ts`: a text input that filters a
//! list of options and holds the chosen one, with chips for several. This
//! module is the public API: the options, the props parts take and the state
//! they hand out. State and the API live in `combobox_api.rs`, keys in
//! `combobox_keyboard.rs`, and parts in `combobox_parts.rs`, on the listbox
//! engine and the selection model Select shares.

use std::fmt::Debug;
use std::rc::Rc;

use gpui::{App, Hsla, Pixels, SharedString, Window};
use ui_sdk_kernel::{
    ActionOutcome, Agent, AgentAction, AgentArgs, AgentError, AgentItem, AgentState, AgentSurface, AgentValue,
    ChangeDetails, ChangeOptions, DismissAttemptDetails, Modality,
};
use ui_sdk_lists::{
    AsyncOptionsStatus, InputState, ItemIconProps, ItemIconState, ItemIndicatorState, ListAnnouncement,
    ListData, ListEmptyState, ListFilter, ListHighlightDetails, ListItemTextState, ListSelectAllState,
    ListStatusState, ListValue,
};
use ui_sdk_motion::{PresenceState, PresenceTiming};
use ui_sdk_native::TextFieldStyle;
use ui_sdk_platform::{Align, Anchor, Placement, PositionProps, Side};
use ui_sdk_popups::{Modal, PopupArrowState, Presentation, PresentationSetting, SheetPartState};
use ui_sdk_text::normalize;

use crate::combobox_api::{Combobox, ComboboxItemValue};

/// The value of a combobox: one item's value or nothing, or a list with
/// `multiple` (`ComboboxValue`), the listbox's value Select shares.
pub type ComboboxValue<T> = ListValue<T>;

/// When `auto_highlight` highlights: the first match while typing, so Enter
/// chooses it (`true`), also the selected item, or the first, when the list
/// opens (`"always"`), or only the selected item when the list opens
/// (`"selected"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ComboboxAutoHighlight {
    #[default]
    Off,
    /// The first matching item while typing, so Enter chooses it.
    Typing,
    /// That, and the selected item, or the first, when the list opens, so
    /// arrows move from there.
    Always,
    /// That, and the selected item when the list opens, and nothing when
    /// none is.
    Selected,
}

impl ComboboxAutoHighlight {
    /// Whether it highlights anything, `true` for every mode but `Off`.
    pub fn is_on(self) -> bool {
        self != ComboboxAutoHighlight::Off
    }
}

impl From<bool> for ComboboxAutoHighlight {
    fn from(on: bool) -> Self {
        if on {
            ComboboxAutoHighlight::Typing
        } else {
            ComboboxAutoHighlight::Off
        }
    }
}

/// `on_value_change`: the requested value and why. Reasons: `ItemPress`,
/// `InputChange`, `ClearPress`, `RemovePress`, `EscapeKey`, `Reset`,
/// `Restore` and `Imperative` (`ComboboxValueChangeReason`).
pub type ComboboxValueCallback<T> = Rc<dyn Fn(&ComboboxValue<T>, &ChangeDetails, &mut Window, &mut App)>;

/// `on_query_change`: the requested text and why. Closing the list puts back
/// text nobody chose, with the reason it closed. Reasons: `TriggerPress`,
/// `InputChange`, `ItemPress`, `ClearPress`, `EscapeKey`, `ClosePress`,
/// `OutsidePress`, `FocusOut`, `WindowBlur`, `Back`, `Swipe`, `Reset` and
/// `Imperative` (`ComboboxQueryChangeReason`).
pub type ComboboxQueryCallback = Rc<dyn Fn(&SharedString, &ChangeDetails, &mut Window, &mut App)>;

/// `on_open_change`: whether the list opens, and why. Reasons:
/// `TriggerPress`, `InputChange`, `ItemPress`, `OutsidePress`, `EscapeKey`,
/// `ClosePress`, `FocusOut`, `WindowBlur`, `Back`, `Swipe` and `Imperative`
/// (`ComboboxOpenChangeReason`). The dismissals `dismissible` refuses that
/// `on_dismiss_attempt` hears about are `EscapeKey`, `OutsidePress`, `Back`
/// and `Swipe` (`ComboboxDismissReason`).
pub type ComboboxOpenCallback = Rc<dyn Fn(&bool, &ChangeDetails, &mut Window, &mut App)>;

/// The string a value submits as and is identified by (`itemToValue`).
pub type ItemToValue<T> = Rc<dyn Fn(&T) -> SharedString>;

/// The text a value shows (`itemToLabel`).
pub type ItemToLabel<T> = Rc<dyn Fn(&T) -> SharedString>;

/// Whether an item's value is the selected value (`isItemEqualToValue`).
pub type IsItemEqualToValue<T> = Rc<dyn Fn(&T, &T) -> bool>;

/// Props of the content (`ComboboxContentProps`).
#[derive(Clone, Default)]
pub struct ComboboxContentProps {
    /// Where the list goes beside the input: the side, `Bottom` by default;
    /// the alignment along it, `Start` by default; the gap, 4 by default;
    /// the shift along the input, 0 by default; the collision and arrow
    /// padding; and an anchor of its own.
    pub position: PositionProps,
    /// Whether the list stays in the page, hidden, while closed.
    pub keep_mounted: bool,
    /// How it animates in and out, the transitions its CSS would declare.
    pub timing: Option<PresenceTiming>,
    /// The dim behind it while it presents as a sheet, at its darkest
    /// (`--sheet-backdrop`).
    pub sheet_backdrop: Option<Hsla>,
}

impl ComboboxContentProps {
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

/// Props of an item (`ComboboxItemProps`).
#[derive(Clone, Debug, PartialEq)]
pub struct ComboboxItemProps<T> {
    /// What the value becomes when the item is chosen.
    pub value: T,
    /// The text the input shows once chosen, and filtering matches, when the
    /// content isn't plain text. Not the accessible name: use `aria_label`
    /// for that.
    pub label: Option<SharedString>,
    /// More words filtering matches.
    pub keywords: Vec<SharedString>,
    /// Keeps the item from being chosen. The keyboard and the pointer skip
    /// it.
    pub disabled: bool,
    /// The item's position among all visible items, for virtualized lists
    /// that render only some. Sets its position in the set and the set's
    /// size.
    pub index: Option<usize>,
    /// The group it's in, the key of the group part around it.
    pub group: Option<SharedString>,
    /// It's a link, which the app follows in its own press on the part
    /// (`on_press`): Enter clicks it, so it follows itself and the press
    /// chooses it.
    pub link: bool,
}

impl<T> ComboboxItemProps<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            label: None,
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

    pub fn keywords(mut self, keywords: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
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

/// What the create option runs with the input's text (`onCreate`). It
/// doesn't change the value by itself. Return `ActionOutcome::Pending` to
/// mark the option pending until the work settles; the list closes (or the
/// input empties, with `multiple`) once it succeeds. It runs inside the
/// combobox's update, so it may update any entity but the combobox.
pub type CreateCallback = Rc<dyn Fn(&SharedString, &mut Window, &mut App) -> ActionOutcome>;

/// Props of the create option (`ComboboxCreateProps`).
#[derive(Clone)]
pub struct ComboboxCreateProps {
    /// Runs with the input's text when the option is chosen.
    pub on_create: Option<CreateCallback>,
    /// How many characters the text needs before the option shows. Defaults
    /// to 1.
    pub min_length: usize,
}

impl Default for ComboboxCreateProps {
    fn default() -> Self {
        Self {
            on_create: None,
            min_length: 1,
        }
    }
}

impl ComboboxCreateProps {
    pub fn on_create(
        mut self,
        on_create: impl Fn(&SharedString, &mut Window, &mut App) -> ActionOutcome + 'static,
    ) -> Self {
        self.on_create = Some(Rc::new(on_create));
        self
    }

    pub fn min_length(mut self, min_length: usize) -> Self {
        self.min_length = min_length;
        self
    }
}

/// Props of the clear button (`ComboboxClearProps`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ComboboxClearProps {
    /// Whether the clear button stays in the page while there's nothing to
    /// clear.
    pub keep_mounted: bool,
}

/// The words for the labels past `max_labels`, like "2 more".
pub type MoreLabel = Rc<dyn Fn(usize) -> SharedString>;

/// Props of the value (`ComboboxValueProps`).
#[derive(Clone, Default)]
pub struct ComboboxValueProps {
    /// With `multiple`, how many labels the `label` in its state shows
    /// before the rest are counted, as in "France, Chile and 2 more", joined
    /// the way the page's language joins lists, or in English with the
    /// default `get_more_label`.
    pub max_labels: Option<usize>,
    /// The words for the labels past `max_labels`. Defaults to "N more".
    pub get_more_label: Option<MoreLabel>,
}

impl ComboboxValueProps {
    pub fn max_labels(mut self, max_labels: usize) -> Self {
        self.max_labels = Some(max_labels);
        self
    }

    pub fn get_more_label(mut self, more: impl Fn(usize) -> SharedString + 'static) -> Self {
        self.get_more_label = Some(Rc::new(more));
        self
    }
}

/// Props of the check inside an item (`ComboboxItemIndicatorProps`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ComboboxItemIndicatorProps {
    /// Whether the indicator stays in the page while its item isn't
    /// selected.
    pub keep_mounted: bool,
}

/// What the `input` part's `with` and the app read (`ComboboxInputState`):
/// whether the list is open, whether the combobox is disabled, by itself or
/// by a fieldset, whether it keeps its value but refuses changes, and
/// whether a value must be selected before the form submits.
pub type ComboboxInputState = InputState;

/// What the `trigger` part's `with` reads (`ComboboxTriggerState`).
pub type ComboboxTriggerState = InputState;

/// What the `item-indicator` part's `with` reads
/// (`ComboboxItemIndicatorState`): whether its item is selected. While
/// false, the indicator isn't shown unless `keep_mounted` is set.
pub type ComboboxItemIndicatorState = ItemIndicatorState;

/// What the `empty` part's `with` reads (`ComboboxEmptyState`).
pub type ComboboxEmptyState = ListEmptyState;

/// What the `item-text` part's `with` reads (`ComboboxItemTextState`).
pub type ComboboxItemTextState = ListItemTextState;

/// What the `select-all` part's `with` reads (`ComboboxSelectAllState`).
pub type ComboboxSelectAllState = ListSelectAllState;

/// What the `status` part's `with` reads (`ComboboxStatusState`).
pub type ComboboxStatusState = ListStatusState;

/// The props the `item-icon` part takes (`ComboboxItemIconProps`).
pub type ComboboxItemIconProps = ItemIconProps;

/// What the `item-icon` and `value-icon` parts' `with` reads
/// (`ComboboxItemIconState`).
pub type ComboboxItemIconState = ItemIconState;

/// What the `arrow` part's `with` reads (`ComboboxArrowState`): whether the
/// list is open, the side it opened on and where the arrow goes along its
/// edge.
pub type ComboboxArrowState = PopupArrowState;

/// What the `content` and `list` parts' `with` reads
/// (`ComboboxContentState`), with what the original writes as attributes
/// and variables: its presence and where it's placed.
#[derive(Clone, Debug, PartialEq)]
pub struct ComboboxContentState<T> {
    /// Whether the list is open. False while it animates out.
    pub open: bool,
    /// How it presents this time it's open (`data-presentation`).
    pub presentation: Presentation,
    /// Whether several items can be selected.
    pub multiple: bool,
    /// Whether results are loading or failed.
    pub status: AsyncOptionsStatus,
    /// The visible items, for rendering a virtualized list yourself.
    pub items: ListData<T>,
    /// Its enter and exit, `data-starting-style` and `data-ending-style`.
    pub presence: PresenceState,
    /// Where it's placed, as of the last frame (`data-side`,
    /// `--available-height`, `--anchor-width`).
    pub placement: Option<Placement>,
    /// Kept in the page while closed, it renders hidden.
    pub hidden: bool,
    /// The `modal` in effect.
    pub modal: Modal,
    /// The sheet it presents as, while it does.
    pub sheet: Option<SheetPartState>,
    /// How the user works in the list now (`data-modality`): with the
    /// keyboard, while focus stays in the input, or a pointer. Style the
    /// highlighted item's keyboard focus from it.
    pub modality: Modality,
}

/// What the `item` part's `with` reads (`ComboboxItemState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ComboboxItemState {
    /// Whether the item is highlighted, by the keyboard or the pointer.
    pub highlighted: bool,
    /// Whether its value is in the combobox's value, compared with
    /// `is_item_equal_to_value` when there is one.
    pub selected: bool,
    /// Whether the item can't be chosen, by itself or because
    /// `max_selected` is reached.
    pub disabled: bool,
    /// Whether `max_selected` is reached and the item isn't selected, which
    /// is why it's disabled.
    pub max_reached: bool,
}

/// What the `create` part's `with` reads (`ComboboxCreateState`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComboboxCreateState {
    /// The text the option would create.
    pub query: SharedString,
    /// Whether the option is highlighted.
    pub highlighted: bool,
    /// Whether `on_create`'s work is running.
    pub pending: bool,
    /// Why the last `on_create` failed, until the next try.
    pub error: Option<SharedString>,
}

/// What the `chips-overflow` part's `with` reads
/// (`ComboboxChipsOverflowState`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComboboxChipsOverflowState {
    /// How many chips don't fit and are hidden.
    pub count: usize,
    /// The labels of the hidden chips.
    pub labels: Vec<SharedString>,
}

/// What the `chip` part's `with` reads (`ComboboxChipState`).
#[derive(Clone, Debug, PartialEq)]
pub struct ComboboxChipState<T> {
    /// The selected value the chip stands for.
    pub value: T,
    /// The text for the value: from `item_to_label`, its item's label, or
    /// the value itself.
    pub label: SharedString,
    /// Whether the combobox is disabled or read-only, so the chip can't be
    /// removed.
    pub disabled: bool,
    /// Collapsed into the overflow part, past the chips that fit, so it
    /// renders hidden.
    pub hidden: bool,
}

/// What the `clear` and `chip-remove` parts' `with` reads
/// (`ComboboxClearState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ComboboxClearState {
    /// Whether the combobox is disabled or read-only.
    pub disabled: bool,
}

/// What the `value` part hands the app (`ComboboxValueState`).
#[derive(Clone, Debug, PartialEq)]
pub struct ComboboxValueState<T> {
    /// The selected value: one, none, or a list with `multiple`.
    pub value: ComboboxValue<T>,
    /// Each selected item's label, in the order they were selected.
    pub labels: Vec<SharedString>,
    /// The labels joined with commas, or with `max_labels`, as a list in the
    /// page's language, like "France, Chile and 2 more". Empty for none.
    pub label: SharedString,
}

/// The options `Combobox` takes, beside its value's (`ComboboxOptions`).
pub struct ComboboxOptions<T> {
    /// Whether several items can be selected. The value is then a list, the
    /// list stays open as items toggle, and the input empties after each
    /// choice.
    pub multiple: bool,
    /// The selected value. Pass it to control the combobox.
    pub value: Option<ComboboxValue<T>>,
    /// The value selected at first, when uncontrolled. A form reset returns
    /// to it.
    pub default_value: Option<ComboboxValue<T>>,
    /// Runs when a user, a form reset or the api wants to change the value.
    /// It runs once the combobox's update is over, so it may update any
    /// entity, the combobox too.
    pub on_value_change: Option<ComboboxValueCallback<T>>,
    /// Runs as the value is asked to change, before it does, inside the
    /// combobox's update: `details.cancel()` stops it. It may update any
    /// entity but the combobox.
    pub on_before_value_change: Option<ComboboxValueCallback<T>>,
    /// The input's text, which filters the list. Pass it to control it.
    /// Choosing an item sets it to the item's label, or empties it with
    /// `multiple`.
    pub query: Option<SharedString>,
    /// The input's text at first, when uncontrolled. Defaults to the selected
    /// item's label.
    pub default_query: Option<SharedString>,
    /// Runs when the input's text wants to change: typing, choosing an item,
    /// or closing with text that wasn't chosen.
    pub on_query_change: Option<ComboboxQueryCallback>,
    /// Whether the list is open. Pass it to control it.
    pub open: Option<bool>,
    /// Whether the list starts open, when uncontrolled.
    pub default_open: bool,
    /// Runs when the list wants to open or close. `details.cancel()` stops
    /// it.
    pub on_open_change: Option<ComboboxOpenCallback>,
    /// Runs once the list has finished opening or closing, after any
    /// animation.
    pub on_open_change_complete: Option<Rc<dyn Fn(bool, &mut Window, &mut App)>>,
    /// With `multiple`, how many items can be selected. Once reached, the
    /// other items are disabled.
    pub max_selected: Option<usize>,
    /// The items as data. The list renders them from its state's `items`,
    /// and filters them without rendering every one.
    pub items: Option<ListData<T>>,
    /// Items shown instead of `items` while the input is empty, like recent
    /// choices or suggestions. Groups work. Needs `items`.
    pub empty_items: Option<ListData<T>>,
    /// How typing narrows the list: `Contains`, `StartsWith`, a function of
    /// the item and the text, whose score also sorts data items, or `Off` to
    /// keep every item, for results a server already filtered. Defaults to
    /// `Contains`.
    pub filter: ListFilter<T>,
    /// The string a value submits as and is identified by, for values that
    /// aren't text.
    pub item_to_value: Option<ItemToValue<T>>,
    /// The text a value shows, for values whose item isn't rendered, like
    /// async results.
    pub item_to_label: Option<ItemToLabel<T>>,
    /// Whether an item's value is the selected value, for values that are
    /// equal another way.
    pub is_item_equal_to_value: Option<IsItemEqualToValue<T>>,
    /// Whether the first matching item is highlighted while typing, so Enter
    /// chooses it, and what's highlighted as the list opens.
    pub auto_highlight: ComboboxAutoHighlight,
    /// With `multiple`, whether the input keeps its text after an item is
    /// chosen.
    pub keep_query: bool,
    /// What screen readers hear once the create option's `on_create` has
    /// been pending for a second. Set it in the page's language. Empty says
    /// nothing. Defaults to "Loading".
    pub pending_announcement: Option<SharedString>,
    /// Whether text that matches no item becomes the value, when Enter is
    /// pressed with nothing highlighted, or, without `multiple`, when the list
    /// closes. The value is the text itself, through `custom_value`.
    pub allow_custom_value: bool,
    /// The value typed text becomes with `allow_custom_value`. Text is the
    /// value itself in the original; a value that isn't text says how.
    pub custom_value: Option<Rc<dyn Fn(&SharedString) -> T>>,
    /// Whether results are loading or failed, for the status and empty
    /// parts. Marks the list busy while loading. Defaults to `Idle`.
    pub status: AsyncOptionsStatus,
    /// Runs when the highlight moves, with the item's index among the
    /// visible items, so a virtualized list can scroll to it. It runs inside
    /// the combobox's update.
    pub on_highlight: Option<Rc<dyn Fn(Option<&T>, &ListHighlightDetails, &mut Window, &mut App)>>,
    /// Runs when the highlight or scrolling reaches the last item, so the
    /// next page can load. Not again for the same items, unless the last
    /// load failed.
    pub on_load_more: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    /// What screen readers hear after typing pauses, when no status part is
    /// rendered, and when a chip is removed. `None` says nothing. Defaults
    /// to "3 results", "No results", "Loading", or "France removed".
    pub announcement: Option<Rc<dyn Fn(&ListAnnouncement) -> Option<SharedString>>>,
    /// Whether arrow keys wrap from the last item to the first. Defaults to
    /// true.
    pub r#loop: bool,
    /// `False`: the window stays usable while the list is open. `Pointer`:
    /// an outside press only closes the list. `True`: also blocks the rest of
    /// the window. Defaults to false, or true while it presents as a sheet.
    pub modal: Option<Modal>,
    /// How the list presents. `Sheet` slides it up from the bottom of the
    /// window with the input in its header, where it can be swiped away, and
    /// `Auto` does that on small touch screens. Decided each time it opens.
    pub presentation: Option<PresentationSetting>,
    /// Disables the combobox like a native input: it can't be focused or
    /// changed, and the value isn't submitted.
    pub disabled: bool,
    /// Keeps the combobox focusable and its value submitted, but stops
    /// changes.
    pub read_only: bool,
    /// Blocks form submission until a value is selected.
    pub required: bool,
    /// The name the value submits under. Without one, it submits nothing.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when the combobox isn't inside it.
    pub form: Option<SharedString>,
    /// The input's placeholder, an attribute of the native input.
    pub placeholder: Option<SharedString>,
    /// The colors and sizes the input's text draws with.
    pub field_style: TextFieldStyle,
    /// Whether Escape, an outside press, focus or the window leaving, the
    /// back gesture and a swipe can close the list. Defaults to true.
    pub dismissible: bool,
    /// Runs when `dismissible` refused a dismissal made on purpose.
    pub on_dismiss_attempt: Option<Rc<dyn Fn(&DismissAttemptDetails, &mut Window, &mut App)>>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl<T> Default for ComboboxOptions<T> {
    fn default() -> Self {
        Self {
            multiple: false,
            value: None,
            default_value: None,
            on_value_change: None,
            on_before_value_change: None,
            query: None,
            default_query: None,
            on_query_change: None,
            open: None,
            default_open: false,
            on_open_change: None,
            on_open_change_complete: None,
            max_selected: None,
            items: None,
            empty_items: None,
            filter: ListFilter::Contains,
            item_to_value: None,
            item_to_label: None,
            is_item_equal_to_value: None,
            auto_highlight: ComboboxAutoHighlight::Off,
            keep_query: false,
            pending_announcement: None,
            allow_custom_value: false,
            custom_value: None,
            status: AsyncOptionsStatus::Idle,
            on_highlight: None,
            on_load_more: None,
            announcement: None,
            r#loop: true,
            modal: None,
            presentation: None,
            disabled: false,
            read_only: false,
            required: false,
            name: None,
            form: None,
            placeholder: None,
            field_style: TextFieldStyle::default(),
            dismissible: true,
            on_dismiss_attempt: None,
            debug: false,
        }
    }
}

impl<T: Clone> Clone for ComboboxOptions<T> {
    fn clone(&self) -> Self {
        Self {
            multiple: self.multiple,
            value: self.value.clone(),
            default_value: self.default_value.clone(),
            on_value_change: self.on_value_change.clone(),
            on_before_value_change: self.on_before_value_change.clone(),
            query: self.query.clone(),
            default_query: self.default_query.clone(),
            on_query_change: self.on_query_change.clone(),
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            max_selected: self.max_selected,
            items: self.items.clone(),
            empty_items: self.empty_items.clone(),
            filter: self.filter.clone(),
            item_to_value: self.item_to_value.clone(),
            item_to_label: self.item_to_label.clone(),
            is_item_equal_to_value: self.is_item_equal_to_value.clone(),
            auto_highlight: self.auto_highlight,
            keep_query: self.keep_query,
            pending_announcement: self.pending_announcement.clone(),
            allow_custom_value: self.allow_custom_value,
            custom_value: self.custom_value.clone(),
            status: self.status,
            on_highlight: self.on_highlight.clone(),
            on_load_more: self.on_load_more.clone(),
            announcement: self.announcement.clone(),
            r#loop: self.r#loop,
            modal: self.modal,
            presentation: self.presentation,
            disabled: self.disabled,
            read_only: self.read_only,
            required: self.required,
            name: self.name.clone(),
            form: self.form.clone(),
            placeholder: self.placeholder.clone(),
            field_style: self.field_style,
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            debug: self.debug,
        }
    }
}

impl<T: Clone + PartialEq + Debug + 'static> ComboboxOptions<T> {
    pub fn multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    pub fn value(mut self, value: impl Into<ComboboxValue<T>>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn default_value(mut self, value: impl Into<ComboboxValue<T>>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&ComboboxValue<T>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&ComboboxValue<T>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn query(mut self, query: impl Into<SharedString>) -> Self {
        self.query = Some(query.into());
        self
    }

    pub fn default_query(mut self, query: impl Into<SharedString>) -> Self {
        self.default_query = Some(query.into());
        self
    }

    pub fn on_query_change(
        mut self,
        callback: impl Fn(&SharedString, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_query_change = Some(Rc::new(callback));
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

    pub fn on_open_change(mut self, callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn max_selected(mut self, max: usize) -> Self {
        self.max_selected = Some(max);
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

    pub fn filter(mut self, filter: ListFilter<T>) -> Self {
        self.filter = filter;
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

    pub fn auto_highlight(mut self, auto_highlight: impl Into<ComboboxAutoHighlight>) -> Self {
        self.auto_highlight = auto_highlight.into();
        self
    }

    pub fn keep_query(mut self, keep_query: bool) -> Self {
        self.keep_query = keep_query;
        self
    }

    pub fn pending_announcement(mut self, announcement: impl Into<SharedString>) -> Self {
        self.pending_announcement = Some(announcement.into());
        self
    }

    pub fn allow_custom_value(mut self, allow: bool) -> Self {
        self.allow_custom_value = allow;
        self
    }

    pub fn custom_value(mut self, custom_value: impl Fn(&SharedString) -> T + 'static) -> Self {
        self.custom_value = Some(Rc::new(custom_value));
        self
    }

    pub fn status(mut self, status: AsyncOptionsStatus) -> Self {
        self.status = status;
        self
    }

    pub fn on_highlight(
        mut self,
        callback: impl Fn(Option<&T>, &ListHighlightDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_highlight = Some(Rc::new(callback));
        self
    }

    pub fn on_load_more(mut self, callback: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_load_more = Some(Rc::new(callback));
        self
    }

    pub fn announcement(mut self, announcement: impl Fn(&ListAnnouncement) -> Option<SharedString> + 'static) -> Self {
        self.announcement = Some(Rc::new(announcement));
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
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

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn field_style(mut self, style: TextFieldStyle) -> Self {
        self.field_style = style;
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(mut self, callback: impl Fn(&DismissAttemptDetails, &mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// `agent: { actions: ["setOpen", "setValue", "clear", "setQuery"], closed:
/// true }` and `controlled: ["open", "value", "query"]`, with `open`,
/// `close` and `toggle`, which `setOpen` covers, unlisted. A value is given
/// by its items' keys, or their labels, and text that names no item only
/// with `allow_custom_value`.
impl<T: ComboboxItemValue> Agent for Combobox<T> {
    const SURFACE: AgentSurface = AgentSurface::new().closed(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", |this, args, window, cx| {
            let open = args.boolean(0)?;
            Ok(this.set_open(open, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setValue", |this, args, window, cx| {
            let value = this.agent_value(args)?;
            Ok(this.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |this, _args, window, cx| {
            Ok(this.clear(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setQuery", |this, args, window, cx| {
            let query = args.string(0)?;
            Ok(this.set_query(query, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("open", |this, _args, window, cx| {
            Ok(this.set_open(true, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("close", |this, _args, window, cx| {
            Ok(this.set_open(false, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::unlisted("toggle", |this, _args, window, cx| {
            Ok(this.toggle(ChangeOptions::default(), window, cx).into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["open", "value", "query"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        let keys: Vec<AgentValue> = self
            .value
            .get()
            .as_slice()
            .iter()
            .map(|value| AgentValue::from(self.key_of(value).to_string()))
            .collect();
        let value = if self.multiple {
            AgentValue::Array(keys)
        } else {
            keys.into_iter().next().unwrap_or(AgentValue::Null)
        };
        let mut state = AgentState::new()
            .with("open", self.is_open())
            .with("value", value)
            .with("query", self.query.get().to_string())
            .with("multiple", self.multiple)
            .with("disabled", self.is_disabled())
            .with("readOnly", self.read_only)
            .with("required", self.required)
            .with("pending", self.pending);
        if let Some(placeholder) = &self.options.placeholder {
            state.set("placeholder", placeholder.to_string());
        }
        if let Some(error) = &self.error {
            state.set("error", error.to_string());
        }
        state
    }

    fn agent_items(&self, _cx: &App) -> Option<Vec<AgentItem>> {
        let entries = self
            .listbox
            .data_items(self)
            .unwrap_or_else(|| self.listbox.entries(self));
        Some(
            entries
                .iter()
                .filter(|entry| !entry.pinned)
                .map(|entry| {
                    AgentItem::new(entry.key.clone(), entry.label.clone())
                        .disabled(self.listbox.disabled(self, entry))
                })
                .collect(),
        )
    }

    fn agent_custom_value(&self, _cx: &App) -> bool {
        self.options.allow_custom_value
    }
}

impl<T: ComboboxItemValue> Combobox<T> {
    // The value an agent's step names: items by key, else by label, and
    // text that names none with `allow_custom_value`.
    fn agent_value(&self, args: &AgentArgs) -> Result<ComboboxValue<T>, AgentError> {
        if args.is_missing(0) {
            return Ok(ListValue::empty(self.multiple));
        }
        let texts = if self.multiple {
            args.list(0)?
        } else {
            vec![args.string(0)?]
        };
        let entries = self
            .listbox
            .data_items(self)
            .unwrap_or_else(|| self.listbox.entries(self));
        let mut values = Vec::with_capacity(texts.len());
        for text in texts {
            let by_label = || {
                let needle = normalize(&text);
                entries
                    .iter()
                    .find(|entry| !entry.pinned && normalize(&entry.label) == needle)
                    .and_then(|entry| entry.value.clone())
            };
            let custom = || {
                self.options
                    .allow_custom_value
                    .then(|| self.custom_value_of(&text))
                    .flatten()
            };
            match self.listbox.value_of(self, &text).or_else(by_label).or_else(custom) {
                Some(value) => values.push(value),
                None => return Err(AgentError::refused(format!("No item is \u{201c}{text}\u{201d}."))),
            }
        }
        Ok(ListValue::from_values(values, self.multiple))
    }
}
