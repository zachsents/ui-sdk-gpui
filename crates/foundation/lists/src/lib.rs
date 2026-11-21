//! Port of `foundation/lists`: options you move through and search, the
//! values they hold, the parts lists share, item icons, and the shape of a
//! tree.
//!
//! # TypeScript file → Rust module
//!
//! | Original | Here |
//! | --- | --- |
//! | `listbox-engine.ts` | [`listbox_engine`]: [`ListboxEngine`] (the original's `Listbox`), [`create_listbox_engine`], [`ListData`], [`ListItem`], [`ListGroup`], [`ListFilter`], [`ItemMeta`], [`reveal_in_list`] |
//! | `listbox.ts` | [`listbox`]: [`create_filtered_listbox`], [`filter_data`], [`item_matches`] |
//! | `listbox-behavior.ts` | [`listbox_behavior`]: [`create_listbox`], [`ListboxBehavior`] |
//! | `listbox-keyboard.ts` | [`listbox_keyboard`]: [`list_step`], [`list_key_down`], [`PAGE`], [`paged_elsewhere`] |
//! | `listbox-keys.ts` | [`listbox_keys`]: [`option_id`], [`is_select_all`], [`select_all_key`] |
//! | `listbox-listing.ts` | [`listbox_listing`]: [`Listing`], [`keep_listing`], [`no_listing`] |
//! | `listbox-parts.ts` | [`listbox_parts`]: the shared parts and form binders |
//! | `listbox-popup.ts` | [`listbox_popup`]: [`list_content_part`], [`list_content_config`], [`list_part`], [`ListSettled`] |
//! | `listbox-search.ts` | [`listbox_search`]: [`ItemTexts`], [`item_text_part`], [`announce_results`], [`create_load_more`] |
//! | `listbox-select-all.ts` | [`listbox_select_all`]: [`SelectAll`], [`select_all_part`], [`select_all_feature`] |
//! | `listbox-value.ts` | [`listbox_value`]: [`create_value_selection`], [`ValueSelection`], [`ListValue`], [`format_labels`] |
//! | `field-name.ts` | [`field_name`]: [`FieldName`], [`list_name`] |
//! | `item-icon.ts` | [`item_icon`]: [`icon_part`], [`item_icon_part`], [`check_icon_item`], [`ItemIcons`], [`item_icons_feature`] |
//! | `async-options.ts` | [`async_options`]: [`create_async_options`], [`AsyncOptions`], [`AsyncOptionsStatus`] |
//! | `autocomplete-core.ts` | [`autocomplete_core`]: [`create_autocomplete_core`], [`AutocompleteCore`], [`SearchFilter`], [`field_part`], [`option_part`] |
//! | `tree-collection.ts` | [`tree_collection`]: [`create_tree_collection`], [`TreeCollection`], [`TreeData`], [`tree_data_nodes`], [`tree_from_paths`], [`visible_tree_rows`] |
//! | `tree-checked.ts` | [`tree_checked`]: [`tree_checked_states`], [`toggle_tree_checked`], [`TreeCheckStrategy`] |
//! | `virtualizer.ts` | [`virtualizer`]: [`create_virtualizer`], [`Virtualizer`], [`VirtualItem`] |
//! | `stick-to-bottom.ts` | [`stick_to_bottom`]: [`create_stick_to_bottom`], [`StickToBottom`] |
//! | `scroll-picker.ts` | [`scroll_picker`]: [`bind_scroll_picker`], [`ScrollPicker`], [`picker_position`], [`nearest_enabled`] |
//! | `inline-edit.ts` | [`inline_edit`]: [`create_inline_edit`], [`InlineEdit`] |
//! | `findable-rows.ts` | dropped, below |
//! | `index.ts` | these re-exports |
//!
//! # In gpui
//!
//! The engine, the value selection and the other behaviors are handles a
//! component makes in its constructor and keeps in a field, like the
//! selection models. They read the component through the getters they're
//! given, so reads take the component (`this`), and calls that move the
//! highlight take it mutably after a clone of the handle. Callbacks run
//! inside the component's update, with it.
//!
//! Items are the parts a list renders, in render order, which is page order:
//! each registers with what it says about itself ([`ItemMeta`]: its value,
//! label, the text it shows, which typeahead and filtering read, and
//! whether it's disabled), and [`ListboxEngine::track`] on its element tells
//! the list where it laid out and when it left, even when the whole list
//! stopped rendering. Once the frame that rendered them is drawn, the engine
//! settles, as the original's microtasks after its items bind: the
//! component hears who joined and left, a highlighted item that rendered
//! late takes focus, one that left for good drops the highlight, and what
//! learns from where they landed runs ([`ListboxEngine::set_settled_hook`]).
//! Reads while a frame renders see the last frame's items, as the original's
//! do before new items bind.
//!
//! Keys are strings, as the original's are; values are generic (`T`), and
//! become keys through `item_to_value` or [`ItemKey`]. A component's own item
//! fields, like a command's shortcut, ride along as `X` in [`ListItem`].
//! Real focus is the items' focus handles, which an item part tracks with
//! `FocusMode::Roving`; virtual focus is an active descendant the field
//! points at [`option_id`]. Keys are matched with `ui_sdk_kernel::Key`.
//! Revealing an item scrolls the [`ListScroller`] its component names, since
//! gpui can't find an item's scrolling ancestor.
//!
//! # Deviations and drops
//!
//! - Pinned entries, a create or select-all option, have no value
//!   ([`ItemMeta::built_in`]): `ListEntry::value` is `None` for them, where
//!   the original gives them their key as a value of another type.
//! - `createListbox`'s `key` option, the state key holding the highlight:
//!   the behavior keeps it ([`ListboxBehavior::highlighted`]), as the engine
//!   keeps a component's (`store.get().highlighted`). The behavior takes a
//!   `container` and a `scroll_root` the original finds around an option.
//! - The engine's `MutationObserver`, which gives focus back to an item an
//!   app moved: moving an element in gpui keeps its focus handle, so focus
//!   stays.
//! - `filterAttribute`, `stashContent`, `rootPart`, and the item text's
//!   `write`: web components only.
//! - The hidden inputs (`hiddenInputPart`, `valueInputPart`) and
//!   `bindFormRoot`'s internals: every control reports through its form
//!   internals ([`hidden_input_part`], [`bind_form_root`]), one value or,
//!   with `multiple`, every key under its name. Autofill and a browser's
//!   restore (`restore`, `stopRestoring`, `isFilling`, `autocomplete`): gpui
//!   has neither.
//! - `ItemsOf`: a type only TypeScript reads.
//! - `itemIndicatorPart`'s `animateIn`: an element that appears animates in
//!   with gpui's `with_animation`.
//! - The mirror rule (`addMirrorRule`) and the `data-has-icon` marks:
//!   [`ItemIconState::flipped`] says when to flip an icon, and [`has_icon`]
//!   and [`has_icons`] what the marks said. Copying an icon into the value
//!   is rendering its [`ItemIconProps::content`] again.
//! - Links an item is (`a[href]`): an item says it's a link in
//!   [`ItemMeta::link`]. Enter on one that holds focus is left to gpui, which
//!   clicks the focused element; one the list only points at, its component
//!   clicks with the kernel's `press_of` (`el.click()`).
//! - Hidden parts (`hidden`): a group or separator that hides lays out as
//!   `display: none` and says so in its state.
//! - `pagedElsewhere`'s `WeakSet` of elements: lists by element id
//!   ([`set_paged_elsewhere`]).
//! - Async options are an entity, observed with `cx.observe`, rather than a
//!   store with `subscribe`; a load returns its work as a future, and fails
//!   with a message.
//! - The autocomplete core hears its component's changes when the component
//!   calls [`AutocompleteCore::observe`] and once each frame that renders the
//!   items is drawn, rather than on every store write, and its options'
//!   changes through [`AutocompleteCore::options_changed`]. Its field's
//!   `value`, `autocomplete`, `autocapitalize` and `spellcheck` are the
//!   native field's own options, its `input` the field's events, and
//!   `takeTypedText` goes with the forms crate's. A hover's `pointerType`
//!   check: gpui's mouse moves never come from a finger.
//!
//! Listbox engine ported.
//! - A tree's checked states are a [`TreeCheckedStates`] to read with `get`,
//!   rather than a function, and a checkbox is the kernel's `Checked`. Tree
//!   rows borrow the items they show ([`TreeRow`]), the same ones passed in.
//!   Data of another type shaped like `TreeItemData`, which TypeScript takes
//!   as it is, implements [`TreeData`].
//! - `findable-rows.ts`, and the virtualizer's `findable` option,
//!   `findableText` and `found` rows: find in page, which gpui has none of
//!   (`hidden="until-found"`); the original attaches nothing where
//!   `supports_hidden_until_found` is false, as it always is here.
//! - The virtualizer keeps the original's math over a gpui scroll container
//!   instead of gpui's `uniform_list` or `list`, which have no estimates,
//!   overscan, sideways rows, scroll margin, kept rows or end; it reads the
//!   scroller as its owner renders, which gpui does on every scroll, rather
//!   than on `scroll` and `ResizeObserver`, and measures rows as they lay
//!   out. Smooth scrolling (`behavior`) isn't gpui's: scrolls land at once.
//! - Stick to bottom keeps the end in view with `ScrollHandle::scroll_to_bottom`
//!   in the frame content grows, and corrects for content above its first
//!   visible row ([`StickToBottom::row`]) a frame later, since nothing reads
//!   a size before gpui lays it out. Turning off `overflow-anchor`: gpui
//!   scrolls with no anchoring. A touch moving up, which only stops its own
//!   scroll in the original, and smooth scrolling: gpui reports neither.
//! - The scroll picker can't read the listbox's options, their selection or
//!   their boxes, so the component names them ([`ScrollPicker::sync`],
//!   [`ScrollPicker::option`]); `--picker-offset` is [`ScrollPicker::offset`].
//!   gpui's scrollers neither snap nor scroll smoothly, so the picker's
//!   settling is the snap and it glides on animation frames itself; there's
//!   no `scrollend`, so rest is the timer. Its scrollbar press check, touch
//!   tracking, the transform scale, right-to-left scroll coordinates (gpui's
//!   run from the start edge), `user-select`, and keeping a press from
//!   focusing an option (focus never scrolls in gpui) go with the DOM.
//! - The inline edit's `key` option, the state key holding the id being
//!   edited: the edit keeps it ([`InlineEdit::editing`]). The focusable
//!   element around the input, which the original finds with `focusables`,
//!   is given to [`InlineEdit::bind_input`]; the input is a native text
//!   field, multi-line for a text area.
//!
//! Autocomplete ported.
//!
//! Trees ported.
//!
//! Ported.

pub mod async_options;
pub mod autocomplete_core;
pub mod field_name;
pub mod inline_edit;
pub mod item_icon;
pub mod listbox;
pub mod listbox_behavior;
pub mod listbox_engine;
pub mod listbox_keyboard;
pub mod listbox_keys;
pub mod listbox_listing;
pub mod listbox_parts;
pub mod listbox_popup;
pub mod listbox_search;
pub mod listbox_select_all;
pub mod listbox_value;
pub mod scroll_picker;
pub mod stick_to_bottom;
pub mod tree_checked;
pub mod tree_collection;
pub mod virtualizer;

pub use async_options::{
    AsyncOptions, AsyncOptionsContext, AsyncOptionsLoad, AsyncOptionsOptions, AsyncOptionsPage, AsyncOptionsState,
    AsyncOptionsStatus, create_async_options,
};
pub use autocomplete_core::{
    AutoHighlight, AutocompleteCore, AutocompleteCoreConfig, AutocompleteCoreItem, AutocompleteCoreState,
    AutocompleteOptionState, CoreHighlightFn, CoreOpenFn, CorePreferredFn, HighlightCause, ListEdge,
    OptionPressFn, SearchFilter, create_autocomplete_core, field_part, option_part,
};
pub use field_name::{FieldName, bind_field_name, field_name_of, list_name};
pub use item_icon::{
    IconContent, IconPlacement, ItemIcon, ItemIconProps, ItemIconState, ItemIcons, ItemIconsFeature,
    check_icon_item, create_item_icons, has_icon, has_icons, icon_part, item_icon_part, item_icons_feature,
    option_icon, remember_icon, value_icon_part,
};
pub use listbox::{create_filtered_listbox, filter_data, filtering, item_matches, item_score, text_matches};
pub use listbox_behavior::{
    ListboxBehavior, ListboxBehaviorOptions, ListboxHighlightFn, ListboxOption, ListboxOptionAttrs, create_listbox,
};
pub use listbox_engine::{
    HighlightOptions, ItemKey, ItemMeta, ListData, ListElement, ListEmptyState, ListEntry, ListFilter,
    ListFilterFn, ListFilterResult, ListFiltering, ListFocus, ListGroup, ListHighlightDetails, ListHighlightFn,
    ListHookFn, ListItem, ListKeyFn, ListNode, ListReadFn, ListScroller, ListSeparator, ListStatusState, Listbox,
    ListboxEngine, ListboxEngineConfig, ListingInputs, VisibleData, WeakListboxEngine, create_listbox_engine,
    flatten_data, highlight_from_code, highlightable, highlighted_value, is_group, is_separator, item_key,
    reveal_in_list, to_list_item,
};
pub use listbox_keyboard::{PAGE, list_key_down, list_step, paged_elsewhere, set_paged_elsewhere};
pub use listbox_keys::{SELECT_ALL, is_select_all, option_id, select_all_key};
pub use listbox_listing::{KeepListing, Listing, ListingSelection, keep_listing, no_listing};
pub use listbox_parts::{
    FormAttrs, FormState, ItemIndicatorState, LIST_STEP_KEYS, OptionGroupState, OptionSeparatorState,
    PositionAttrs, arrow_part, bind_form_root, bind_item, empty_part, form_attrs, group_label_part,
    group_labelled_by, hidden_input_part, is_disabled, is_locked, item_indicator_part, no_items,
    option_group_part, option_separator_part, position_attrs, register_item, status_part, warn_if_unnamed,
};
pub use listbox_popup::{
    FieldState, InputState, ListPopup, ListSettled, input_state, list_content_config, list_content_part, list_id,
    list_part,
};
pub use listbox_search::{
    ANNOUNCE_DELAY, AnnounceResults, AnnounceResultsOptions, ItemTexts, ListAnnouncement, ListAnnouncementFn,
    ListItemTextProps, ListItemTextState, LoadMore, LoadMoreFn, announce_results, create_load_more,
    describe_list, item_text_part, match_mode, opens_into_list,
};
pub use listbox_select_all::{
    ListSelectAllState, OptionFocus, SelectAll, SelectAllChooseFn, SelectAllFeature, SelectAllHoverFn,
    SelectAllPartProps, group_of, select_all_feature, select_all_part,
};
pub use listbox_value::{
    FormatLabelsOptions, ItemToValue, ListValue, ValueOptions, ValueSelection, ValueSelectionConfig, ValuesFn,
    create_value_selection, format_labels, list_of,
};
pub use tree_checked::{
    ToggleTreeCheckedOptions, TreeCheckStrategy, TreeCheckedState, TreeCheckedStates, toggle_tree_checked,
    tree_checked_states,
};
pub use tree_collection::{
    TreeCollection, TreeCollectionNode, TreeData, TreeDataNode, TreeItemData, TreePosition, TreeRow,
    create_tree_collection, tree_data_nodes, tree_from_paths, visible_tree_rows,
};
pub use virtualizer::{VirtualAlign, VirtualItem, Virtualizer, VirtualizerOptions, create_virtualizer};
pub use stick_to_bottom::{StickToBottom, StickToBottomOptions, StickToBottomState, create_stick_to_bottom};
pub use inline_edit::{
    InlineEdit, InlineEditCommitFn, InlineEditDetails, InlineEditOptions, InlineEditReason, InlineEditStart,
    create_inline_edit,
};
pub use scroll_picker::{
    PickerOption, ScrollPicker, ScrollPickerOptions, ScrollPickerSelectDetails, ScrollPickerSelectFn,
    bind_scroll_picker, nearest_enabled, picker_position,
};
