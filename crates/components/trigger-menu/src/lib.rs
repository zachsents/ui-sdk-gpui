//! Port of `components/trigger-menu`: suggestions anchored in text, opened
//! by a trigger like "@", "/" or "#", that narrow as you type and insert the
//! choice into the text.
//!
//! An app makes a [`TriggerMenu`] with `cx.new(|cx| TriggerMenu::new(options,
//! window, cx))`, its triggers in the options, each with a [`TriggerRule`]
//! and its items as data ([`TriggerMenuData`]), a loader
//! ([`TriggerItems::loader`]) or none, for items it renders itself. It renders
//! [`TriggerMenu::input`] over a native text field, styled as the field and
//! named with `aria_label`, and the popup, [`TriggerMenu::content`], with a
//! [`TriggerMenu::list`] and an [`TriggerMenu::item`] per suggestion: the
//! content state's `items`, or its own. [`TriggerMenu::empty`] shows while
//! nothing matches, [`TriggerMenu::status`] tells screen readers the count and
//! the keys, and [`TriggerMenu::hint`] shows a command's arguments at the
//! caret. Groups, their labels, separators and item icons have parts too.
//! The entity is the controller: [`TriggerMenu::open`] types a trigger and
//! opens it, [`TriggerMenu::insert`] inserts at the caret, and
//! [`TriggerMenu::get_match`] reads the match.
//!
//! An editor of the app's own connects through the `editor` option, a
//! `ui_sdk_text::TextEditorAdapter`, like [`text_field_adapter`] over a field
//! the app renders, and wraps its element in [`TriggerMenu::editor_element`].
//!
//! # In gpui
//!
//! - The input part is the native field's root, the original's `<textarea>`,
//!   or a combobox when the field is single-line, its `<input>`. As it first
//!   renders, it builds the field's adapter with [`text_field_adapter`], and
//!   when it leaves the page the menu lets the adapter go.
//! - Its keys are heard on the field's element, before the field acts on
//!   them, which a handled key stops with `window.prevent_default()`. An
//!   editor without a part of its own is heard through
//!   `cx.intercept_keystrokes`, before anything in the window, as the
//!   original's capture-phase listener on the document. An open list's
//!   Escape is the popup layer's, which hears it once nothing prevented it.
//! - The editor reports what happens through a listener that moves dismissed
//!   matches along and skips what the menu writes itself without reading the
//!   menu, which is mid-update while it writes.
//! - The list is the popups crate's `trigger_menu_preset`, placed at the
//!   trigger's place in the text, which it reads every frame from the
//!   editor's `rect_at`. Focus stays in the editor: presses on the list keep
//!   it there, and the field points at the highlight with an active
//!   descendant once an arrow key moves it. Highlighting scrolls the content,
//!   when it scrolls.
//! - The hint renders above the window beside the caret, and takes no
//!   presses.
//! - A status part with `text` shows and says the count and the keys through
//!   its label and value a moment after typing pauses; without a status part
//!   in the page, the menu speaks through the platform's announcer.
//! - Whether a list, an empty part or an input is in the page comes from the
//!   kernel's registry of bound parts, a frame late, as the original's binds
//!   come a microtask late.
//!
//! # TypeScript file → Rust module
//!
//! `trigger-match.ts` → [`trigger_match`], `trigger-menu.ts` →
//! [`trigger_menu`] (its exports are these re-exports), `trigger-menu-api.ts`
//! → [`trigger_menu_api`], `trigger-menu-keyboard.ts` →
//! [`trigger_menu_keyboard`], `trigger-menu-levels.ts` →
//! [`trigger_menu_levels`], `trigger-menu-parts.ts` → [`trigger_menu_parts`],
//! `trigger-menu-select.ts` → [`trigger_menu_select`].
//!
//! # Dropped, and why
//!
//! - `trigger-menu-generic.ts` (`TriggerMenuGeneric`): TypeScript's inference
//!   of the value type for renderers. Values are `SharedString`, which also
//!   lets one list hold a trigger's items and an argument's suggestions,
//!   always strings.
//! - `createTriggerMenuController`, `TriggerMenuController` and the
//!   `controller` option: the app holds the entity, whose methods are the
//!   controller's.
//! - `container` (`PortalProps`) on the menu and the content: the content
//!   renders above the window in `deferred`.
//! - The renderers' default content, which lists the current trigger's
//!   items when none is composed, and their `TriggerMenuItems`: the app
//!   renders the content part, from its state's `items` or its own.
//! - `waitUntil` on the select details: for web components, whose events
//!   return nothing; `on_select` returns its future. So is a web component's
//!   canceled event canceling the details.
//! - The empty part's `stash`, and `stashContent`: for web components, which
//!   can't skip rendering its message.
//! - `keepListing`, the `listing` state and `selects` (`unreadByItems`), and
//!   the attributes written by `live` and `direct` (`aria-selected`,
//!   `hidden`): narrowing which options a change renders again. gpui renders
//!   the parts from the entity.
//! - `whenKept`: a framework keeping or throwing away the instance. The menu
//!   follows its editor while its entity lives.
//! - `characterPattern`: a regular expression's `lastIndex` between tests.
//!   Patterns are functions.
//! - The development check for an item with only an icon: the item can't
//!   read the label an app puts on it.
//! - The hint's `position: fixed` and `pointer-events: none`, and the
//!   status's `textContent`: styles and text the parts render themselves.
//! - `data-*` attributes: part state.
//!
//! # Deviations
//!
//! - Offsets are byte offsets, as the text crate's are, and lengths, like a
//!   rule's `max_length`, count characters. A full-width trigger an input
//!   method commits, like ＠, keeps its bytes in the match.
//! - The list and the content are labelled by the input part, which
//!   accesskit names by the field's own label, or by an app's editor element;
//!   the original copies the field's labels.
//! - An item can't show the text it's matched by: it's its `label`, else its
//!   value.
//! - An editor of the app's own gets the field's ARIA, and presses in it
//!   count as inside, through [`TriggerMenu::editor_element`]; without it, a
//!   press in the editor is an outside press.
//! - A group's key names it to its label and its items, which pass it in
//!   their `group` prop, where renderers fill it in from the group around
//!   them. The status and hint parts take the `text` prop renderers pass.
//! - `on_open_change` and `on_before_open_change` hear the trigger, and
//!   `on_query_change` the trigger and the argument, as arguments beside the
//!   details.
//! - When both the item's and the menu's `on_select` return a future, the
//!   first makes the token pending, as the original's first promise does,
//!   and the other runs to its end.
//! - `after: Submit` submits once the menu's update is over, and the
//!   adapter's `submit` takes no event.
//! - Until the trigger's place is known, the list has no anchor, rather than
//!   a rectangle at the window's corner.
//! - A token the menu inserts carries [`TriggerTokenData`] in its `data`,
//!   the text crate's `TokenTrigger`, which a token input reads to turn the
//!   token back into its trigger and label (`token.trigger()`). Its
//!   arguments are a list of names and values, empty when it took none.

pub mod trigger_match;
pub mod trigger_menu;
pub mod trigger_menu_api;
pub mod trigger_menu_keyboard;
pub mod trigger_menu_levels;
pub mod trigger_menu_parts;
pub mod trigger_menu_select;

pub use trigger_match::{
    DEFAULT_MAX_LENGTH, DismissedMatch, FoundMatch, TriggerBoundary, TriggerMatch, TriggerPattern, TriggerRule,
    TriggerSpaces, default_pattern, find_trigger_match, is_at_boundary, is_valid_query, look_behind, map_dismissed,
    normalize_trigger_text,
};
pub use trigger_menu::{
    TriggerAfter, TriggerArgument, TriggerArguments, TriggerDefinition, TriggerInsert, TriggerItemExtra, TriggerItems,
    TriggerItemsLoader, TriggerMenuChangeOptions, TriggerMenuContentProps, TriggerMenuContentState, TriggerMenuData,
    TriggerMenuDataGroup, TriggerMenuDataItem, TriggerMenuDismissReason, TriggerMenuEmptyState, TriggerMenuFilter,
    TriggerMenuHintState, TriggerMenuInputState, TriggerMenuItemProps, TriggerMenuItemState,
    TriggerMenuOpenChangeCallback, TriggerMenuOpenChangeCompleteCallback, TriggerMenuOpenChangeReason,
    TriggerMenuOptions, TriggerMenuQueryChangeCallback, TriggerMenuQueryChangeReason, TriggerMenuSelectCallback,
    TriggerMenuSelectDetails, TriggerMenuStatusState,
};
pub use trigger_menu_api::{TriggerMenu, TriggerMenuOpenOptions};
pub use trigger_menu_keyboard::INPUT_KEYS;
pub use trigger_menu_parts::{
    TriggerMenuGroupState, TriggerMenuHint, TriggerMenuItemIconProps, TriggerMenuItemIconState,
    TriggerMenuSeparatorState,
};
pub use trigger_menu_select::TriggerTokenData;
pub use ui_sdk_lists::{
    AsyncOptionsContext, AsyncOptionsStatus, ListFilter, ListGroup, ListItem, ListNode, ListSeparator,
};
pub use ui_sdk_text::{
    EditorChange, EditorContent, EditorReplaceOptions, TextEditorAdapter, TextRange, TextRect, Token, TokenInsert,
    text_field_adapter,
};
