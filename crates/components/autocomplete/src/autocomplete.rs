//! Port of `components/autocomplete/autocomplete.ts`: a text field that
//! suggests as you type. The text is the value, free as any input's, and
//! choosing a suggestion only fills it in. It can complete inline, with the
//! rest of the first match selected after the caret, and it takes part in
//! forms as the text field it is. It's built on the listbox engine Combobox
//! uses. This module is the public API: the options, the props parts take
//! and the state they hand out. State and the API live in
//! `autocomplete_api.rs`, and parts in `autocomplete_parts.rs`.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, Hsla, Pixels, SharedString, Window};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentItem, AgentState, ChangeDetails, ChangeOptions, DismissAttemptDetails, InputEvent,
    Modality,
};
use ui_sdk_lists::{
    AsyncOptionsStatus, InputState, ItemIconProps, ItemIconState, ListAnnouncement,
    ListData, ListEmptyState, ListHighlightDetails, ListItemTextState, ListStatusState, SearchFilter, flatten_data,
};
use ui_sdk_motion::{PresenceState, PresenceTiming};
use ui_sdk_native::TextFieldStyle;
use ui_sdk_platform::{Align, Anchor, Placement, PositionProps, Side};
use ui_sdk_popups::{Modal, PopupArrowState, Presentation, PresentationSetting, SheetPartState};

use crate::autocomplete_api::Autocomplete;

/// The suggestion chosen, which `on_value_change` hears for `ItemPress`, so
/// two with the same label can be told apart (`details.item`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutocompleteChosen {
    pub value: SharedString,
    pub label: SharedString,
}

/// `on_value_change`: the requested text, why, and the suggestion chosen for
/// `ItemPress`. Arrow keys fill in the highlighted suggestion with
/// `inline_complete`, and Escape takes an inline completion back out.
/// Reasons: `InputChange`, `ItemPress`, `ListNavigation`, `EscapeKey`,
/// `ClearPress`, `Reset` and `Imperative`
/// (`AutocompleteValueChangeReason`). `details.cancel()` keeps the text as it
/// was in `on_before_value_change`, which runs inside the autocomplete's
/// update, so it may update any entity but the autocomplete;
/// `on_value_change` runs once the update is over, so it may update any
/// entity, the autocomplete too.
pub type AutocompleteValueCallback =
    Rc<dyn Fn(&SharedString, &ChangeDetails, Option<&AutocompleteChosen>, &mut Window, &mut App)>;

/// `on_open_change`: whether the suggestions open, and why. Reasons:
/// `TriggerPress`, `TriggerFocus`, `InputChange`, `InputCommit`,
/// `ItemPress`, `OutsidePress`, `EscapeKey`, `ClosePress`, `FocusOut`,
/// `WindowBlur`, `Back`, `Swipe` and `Imperative`
/// (`AutocompleteOpenChangeReason`). The dismissals `dismissible` refuses
/// that `on_dismiss_attempt` hears about are `EscapeKey`, `OutsidePress`,
/// `Back` and `Swipe` (`AutocompleteDismissReason`).
pub type AutocompleteOpenCallback = Rc<dyn Fn(&bool, &ChangeDetails, &mut Window, &mut App)>;

/// What `on_submit` receives besides the text (`AutocompleteSubmitDetails`).
#[derive(Debug, Default)]
pub struct AutocompleteSubmitDetails {
    /// The key press or press that submitted.
    pub event: Option<InputEvent>,
    prevented: Cell<bool>,
}

impl AutocompleteSubmitDetails {
    pub fn new(event: Option<InputEvent>) -> Self {
        Self {
            event,
            prevented: Cell::new(false),
        }
    }

    /// Keeps the form from submitting, the event's `preventDefault()`.
    pub fn prevent_default(&self) {
        self.prevented.set(true);
    }

    pub fn default_prevented(&self) -> bool {
        self.prevented.get()
    }
}

/// `on_submit`: the text, when Enter is pressed with no suggestion
/// highlighted, or after choosing one with `submit_on_select`. The form the
/// field is in submits next, unless `details.prevent_default()` is called.
pub type AutocompleteSubmitCallback = Rc<dyn Fn(&SharedString, &AutocompleteSubmitDetails, &mut Window, &mut App)>;

/// `on_highlight`: the highlighted suggestion's value, or `None`, with its
/// index among the visible ones, so a virtualized list can scroll to it.
pub type AutocompleteHighlightCallback = Rc<dyn Fn(Option<&SharedString>, &ListHighlightDetails, &mut Window, &mut App)>;

/// Props of the content (`AutocompleteContentProps`).
#[derive(Clone, Default)]
pub struct AutocompleteContentProps {
    /// Where the suggestions go beside the field: the side, `Bottom` by
    /// default; the alignment along it, `Start` by default; the gap, 4 by
    /// default; the shift along the field, 0 by default; the collision and
    /// arrow padding; and an anchor of its own.
    pub position: PositionProps,
    /// Whether the suggestions stay in the page, hidden, while closed.
    pub keep_mounted: bool,
    /// How they animate in and out, the transitions CSS would declare.
    pub timing: Option<PresenceTiming>,
    /// The dim behind them while they present as a sheet, at its darkest
    /// (`--sheet-backdrop`).
    pub sheet_backdrop: Option<Hsla>,
}

impl AutocompleteContentProps {
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

/// Props of a suggestion (`AutocompleteItemProps`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutocompleteItemProps {
    /// Identifies the suggestion. Choosing it fills the field with its
    /// `label`, or with this when there's none.
    pub value: SharedString,
    /// The text the field gets and filtering matches, when the content isn't
    /// plain text. Not the accessible name: use `aria_label` for that.
    pub label: Option<SharedString>,
    /// More words filtering matches.
    pub keywords: Vec<SharedString>,
    /// Keeps the suggestion from being chosen.
    pub disabled: bool,
    /// The suggestion's position among all visible ones, for virtualized
    /// lists that render only some.
    pub index: Option<usize>,
    /// The group it's in, the key of the group part around it.
    pub group: Option<SharedString>,
    /// It's a link, which the app follows in its own press on the part
    /// (`on_press`): Enter clicks it, so it follows itself and the press
    /// chooses it.
    pub link: bool,
}

impl AutocompleteItemProps {
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            ..Self::default()
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

/// Props of the clear button (`AutocompleteClearProps`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutocompleteClearProps {
    /// Whether the clear button stays in the page while the field is empty.
    pub keep_mounted: bool,
}

/// What the `input` part's `with` and the app read
/// (`AutocompleteInputState`): whether the suggestions are open, and whether
/// the field is disabled, by itself or by a fieldset, read-only or required.
pub type AutocompleteInputState = InputState;

/// What the `trigger` part's `with` reads (`AutocompleteTriggerState`).
pub type AutocompleteTriggerState = InputState;

/// What the `content` and `list` parts' `with` reads
/// (`AutocompleteContentState`), with what the original writes as
/// attributes and variables: its presence and where it's placed.
#[derive(Clone, Debug, PartialEq)]
pub struct AutocompleteContentState {
    /// Whether the suggestions are open. False while they animate out.
    pub open: bool,
    /// How they present this time they're open (`data-presentation`).
    pub presentation: Presentation,
    /// Whether results are loading or failed.
    pub status: AsyncOptionsStatus,
    /// The visible suggestions, for rendering a virtualized list yourself.
    pub items: ListData<SharedString>,
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
    /// keyboard, while focus stays in the field, or a pointer. Style the
    /// highlighted suggestion's keyboard focus from it.
    pub modality: Modality,
}

/// What the `item` part's `with` reads (`AutocompleteItemState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutocompleteItemState {
    /// Whether the suggestion is highlighted, by the keyboard or the
    /// pointer.
    pub highlighted: bool,
    /// Whether the suggestion can't be chosen.
    pub disabled: bool,
}

/// What the `empty` part's `with` reads (`AutocompleteEmptyState`).
pub type AutocompleteEmptyState = ListEmptyState;

/// What the `item-text` part's `with` reads (`AutocompleteItemTextState`).
pub type AutocompleteItemTextState = ListItemTextState;

/// The props the `item-icon` part takes (`AutocompleteItemIconProps`).
pub type AutocompleteItemIconProps = ItemIconProps;

/// What the `item-icon` part's `with` reads (`AutocompleteItemIconState`).
pub type AutocompleteItemIconState = ItemIconState;

/// What the `status` part's `with` reads (`AutocompleteStatusState`).
pub type AutocompleteStatusState = ListStatusState;

/// What the `clear` part's `with` reads (`AutocompleteClearState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutocompleteClearState {
    /// Whether the field is disabled or read-only.
    pub disabled: bool,
}

/// What the `arrow` part's `with` reads (`AutocompleteArrowState`): whether
/// the suggestions are open, the side they opened on and where the arrow
/// goes along their edge.
pub type AutocompleteArrowState = PopupArrowState;

/// The options `Autocomplete` takes (`AutocompleteOptions`).
#[derive(Clone)]
pub struct AutocompleteOptions {
    /// The text. Pass it to control it.
    pub value: Option<SharedString>,
    /// The text at first, when uncontrolled. A form reset returns to it.
    pub default_value: Option<SharedString>,
    /// Runs when the text wants to change: typing, choosing a suggestion, a
    /// Clear part, a form reset or the api. It runs once the autocomplete's
    /// update is over.
    pub on_value_change: Option<AutocompleteValueCallback>,
    /// Runs as the text is asked to change, before it does, inside the
    /// autocomplete's update: `details.cancel()` keeps the text as it was.
    pub on_before_value_change: Option<AutocompleteValueCallback>,
    /// Whether the suggestions are open. Pass it to control them.
    pub open: Option<bool>,
    /// Whether the suggestions start open, when uncontrolled.
    pub default_open: bool,
    /// Runs when the suggestions want to open or close. `details.cancel()`
    /// stops it.
    pub on_open_change: Option<AutocompleteOpenCallback>,
    /// Runs once the suggestions have finished opening or closing, after any
    /// animation.
    pub on_open_change_complete: Option<Rc<dyn Fn(bool, &mut Window, &mut App)>>,
    /// Runs with the text when Enter is pressed with no suggestion
    /// highlighted, or after choosing one with `submit_on_select`.
    pub on_submit: Option<AutocompleteSubmitCallback>,
    /// The suggestions as data, which it filters without rendering every
    /// one. Render the ones to show from the content's state (`items`).
    pub items: Option<ListData<SharedString>>,
    /// Suggestions shown instead of `items` while the field is empty, like
    /// recent searches, with `open_on_focus`. Groups work, like a group
    /// labelled "Recent". Needs `items`.
    pub empty_items: Option<ListData<SharedString>>,
    /// How typing narrows the suggestions: `Contains`, `StartsWith`,
    /// `Fuzzy`, a function of the item and the text, or `Off` to keep every
    /// one, for results a server already filtered. Defaults to `Contains`.
    pub filter: SearchFilter,
    /// Whether typing completes the text with the first suggestion that
    /// starts with it, with the rest selected after the caret: ArrowRight or
    /// End accepts it, Backspace and Escape take it back out, and arrow keys
    /// fill in the suggestion they highlight.
    pub inline_complete: bool,
    /// Whether the first suggestion is highlighted while typing, so Enter
    /// chooses it.
    pub auto_highlight: bool,
    /// Whether the suggestions open when the field gets focus, like recent
    /// searches, even before anything is typed.
    pub open_on_focus: bool,
    /// Whether choosing a suggestion submits, like a search box: `on_submit`
    /// runs, then the form submits.
    pub submit_on_select: bool,
    /// Whether results are loading or failed, for the empty and status
    /// parts. Sets the list busy while loading.
    pub status: AsyncOptionsStatus,
    /// Runs when the highlight moves, with the suggestion's index among the
    /// visible ones, so a virtualized list can scroll to it.
    pub on_highlight: Option<AutocompleteHighlightCallback>,
    /// Runs when the highlight or scrolling reaches the last suggestion, so
    /// the next page can load, like async options' `load_more`. Not again for
    /// the same suggestions, unless the last load failed.
    pub on_load_more: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    /// What screen readers hear after typing pauses, when no status part is
    /// rendered. `None` says nothing. Defaults to "3 results", "No results"
    /// or "Loading".
    pub announcement: Option<Rc<dyn Fn(&ListAnnouncement) -> Option<SharedString>>>,
    /// Whether arrow keys wrap from the last suggestion to the first.
    /// Defaults to true.
    pub r#loop: bool,
    /// `False`: the window stays usable while the suggestions are open.
    /// `Pointer`: an outside press only closes them. `True`: also blocks the
    /// rest of the window. Defaults to false, or true while they present as a
    /// sheet.
    pub modal: Option<Modal>,
    /// How the suggestions present. `Sheet` slides them up from the bottom of
    /// the window with the field in its header, where they can be swiped
    /// away, and `Auto` does that on small touch screens. A tap that opens a
    /// sheet leaves the keyboard closed until the field in the header is
    /// tapped. Decided each time they open.
    pub presentation: Option<PresentationSetting>,
    /// Disables the field like a native input: it can't be focused or
    /// changed, and its text isn't submitted.
    pub disabled: bool,
    /// Keeps the field focusable and its text submitted, but stops changes.
    pub read_only: bool,
    /// Blocks form submission while the field is empty.
    pub required: bool,
    /// The name the text submits under.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when the field isn't inside that form.
    pub form: Option<SharedString>,
    /// The field's placeholder, an attribute of the native input.
    pub placeholder: Option<SharedString>,
    /// The colors and sizes the field's text draws with.
    pub field_style: TextFieldStyle,
    /// Whether Escape, an outside press, focus or the window leaving, the
    /// back gesture and a swipe can close the suggestions. Defaults to true.
    pub dismissible: bool,
    /// Runs when `dismissible` refused a dismissal made on purpose.
    pub on_dismiss_attempt: Option<Rc<dyn Fn(&DismissAttemptDetails, &mut Window, &mut App)>>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for AutocompleteOptions {
    fn default() -> Self {
        Self {
            value: None,
            default_value: None,
            on_value_change: None,
            on_before_value_change: None,
            open: None,
            default_open: false,
            on_open_change: None,
            on_open_change_complete: None,
            on_submit: None,
            items: None,
            empty_items: None,
            filter: SearchFilter::Contains,
            inline_complete: false,
            auto_highlight: false,
            open_on_focus: false,
            submit_on_select: false,
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

impl AutocompleteOptions {
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&SharedString, &ChangeDetails, Option<&AutocompleteChosen>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&SharedString, &ChangeDetails, Option<&AutocompleteChosen>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
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

    pub fn on_submit(
        mut self,
        callback: impl Fn(&SharedString, &AutocompleteSubmitDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_submit = Some(Rc::new(callback));
        self
    }

    pub fn items(mut self, items: impl Into<ListData<SharedString>>) -> Self {
        self.items = Some(items.into());
        self
    }

    pub fn empty_items(mut self, items: impl Into<ListData<SharedString>>) -> Self {
        self.empty_items = Some(items.into());
        self
    }

    pub fn filter(mut self, filter: impl Into<SearchFilter>) -> Self {
        self.filter = filter.into();
        self
    }

    pub fn inline_complete(mut self, inline_complete: bool) -> Self {
        self.inline_complete = inline_complete;
        self
    }

    pub fn auto_highlight(mut self, auto_highlight: bool) -> Self {
        self.auto_highlight = auto_highlight;
        self
    }

    pub fn open_on_focus(mut self, open_on_focus: bool) -> Self {
        self.open_on_focus = open_on_focus;
        self
    }

    pub fn submit_on_select(mut self, submit_on_select: bool) -> Self {
        self.submit_on_select = submit_on_select;
        self
    }

    pub fn status(mut self, status: AsyncOptionsStatus) -> Self {
        self.status = status;
        self
    }

    pub fn on_highlight(
        mut self,
        callback: impl Fn(Option<&SharedString>, &ListHighlightDetails, &mut Window, &mut App) + 'static,
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

/// `agent: { actions: ["setOpen", "setValue", "clear"] }` and `controlled:
/// ["open", "value"]`, with `open`, `close` and `toggle`, which `setOpen`
/// covers, unlisted. Any text is a value; the suggestions as data, and the
/// ones shown while the field is empty, are its items.
impl Agent for Autocomplete {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", |this, args, window, cx| {
            let open = args.boolean(0)?;
            Ok(this.set_open(open, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("setValue", |this, args, window, cx| {
            let value = args.string(0)?;
            Ok(this.set_value(value, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |this, _args, window, cx| {
            Ok(this.clear(ChangeOptions::default(), window, cx).into())
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

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["open", "value"]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        let mut state = AgentState::new()
            .with("open", self.is_open())
            .with("value", self.get_value().to_string())
            .with("disabled", self.is_disabled())
            .with("readOnly", self.read_only)
            .with("required", self.required);
        if let Some(placeholder) = &self.options.placeholder {
            state.set("placeholder", placeholder.to_string());
        }
        state
    }

    fn agent_items(&self, _cx: &App) -> Option<Vec<AgentItem>> {
        let items = self.items.as_ref()?;
        let to_key = |value: &SharedString| value.clone();
        let mut entries = flatten_data(items, &to_key);
        if let Some(empty) = &self.options.empty_items {
            for entry in flatten_data(empty, &to_key) {
                if !entries.iter().any(|known| known.key == entry.key) {
                    entries.push(entry);
                }
            }
        }
        Some(
            entries
                .into_iter()
                .map(|entry| AgentItem::new(entry.key.clone(), entry.label.clone()).disabled(entry.disabled))
                .collect(),
        )
    }
}
