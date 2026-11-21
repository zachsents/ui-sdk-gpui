//! Port of `components/menu` and `components/menubar`: actions and options
//! chosen from a button, a right-click or a menubar, with groups, checkbox
//! and radio items, submenus, typeahead and shortcuts, as a popover or a
//! sheet. The two import each other, so they share this crate.
//!
//! # TypeScript file → Rust module
//!
//! `menu/menu.ts` → [`menu`] (`menu` is [`Menu`], its options, details, data
//! and states), `menu/menu-api.ts` → [`menu_api`] (`MenuApi` is `Menu`'s
//! methods, the props its parts take, and `MenuNode`), `menu/menu-parts.ts`
//! → [`menu_parts`] (the slots, one method each, the original's features
//! included), `menubar/menubar.ts` → [`menubar`].
//!
//! # In gpui
//!
//! - A menu is `create_popup` with `menu_preset`, its rules the original's.
//!   A submenu is a menu of its own, an entity made with its parent in
//!   [`MenuOptions::parent`]; its trigger is an item of the parent and its
//!   content goes inside the parent's, where the trigger is. A menu sits in
//!   a menubar with [`MenuOptions::menubar`], and its trigger joins the
//!   menubar's tab stop as it renders.
//! - The menus of a tree, and a menubar's menus, change each other from
//!   inside their own updates, so what one reads or changes of another while
//!   that one may be updating is plain state beside each entity, the
//!   original's `MenuNode`: a parent's items and highlight, the open child,
//!   the root's callbacks and modal setting, each popup. A submenu that
//!   closes its whole tree, after an item ran or Tab, asks its root once its
//!   own update is over, since the root closes its submenus first. Left and
//!   Right that move to another menu of the menubar run once the menu's
//!   update is over, and so do a menubar trigger's keys.
//! - Items register as they render, which is page order: build a menu's
//!   items after its content, and an indicator after its item. Every item,
//!   checkbox item, radio item, group and separator takes a key. Items keep
//!   their focus handles and where they laid out, for focus, a press released
//!   over one, and scrolling one into view, which the content's
//!   `ScrollHandle` does while the app makes it scroll.
//! - gpui keeps no text to read off an element, so typeahead matches each
//!   item's `label`, and a submenu's or a menubar's trigger's; an item
//!   without one warns in development builds.
//! - The content decides whose key it is as the key arrives, in the capture
//!   phase, before a submenu's handler moves focus to its parent, where the
//!   original reads the key's target. It handles Enter and Space then too,
//!   and stops them, since gpui would press the focused item itself as the
//!   key comes up. A trigger's Enter and Space are gpui's keyboard click.
//! - Focus landing on an item highlights it through each item's
//!   `focus_events::on_focus`, the content's `focusin` in the original.
//! - Resting the pointer on a submenu's trigger opens it through the
//!   popups' hover intent, after `delay`, and the pointer crosses other
//!   items on its way into an open submenu through the safe area. The
//!   menubar's triggers use their own safe area the same way.
//! - The press that opens a menu hears its release anywhere in the window
//!   through the trigger's `capture_any_mouse_up`.
//! - A context area opens the menu on a right-click, on a Control-click on a
//!   Mac, which gpui reports as the primary button, on a long press through
//!   `ui_sdk_direct_manipulation`'s hold, and on Shift+F10 or the key gpui
//!   names `menu` from anything focused inside it. The window's layers ask
//!   whether a press is inside before the area hears it, with the press, so
//!   an area counts as inside only for the secondary button, or a
//!   Control-click on a Mac.
//! - The popup's rules read the menu's shared state: its trigger and what
//!   counts as inside are asked by the window's layers, from inside other
//!   components' updates too, where the menu can't be read.
//! - Presented as a sheet, a submenu shows in its parent's place through
//!   `ui_sdk_popups::drilled_into`: the parent's items, separators, labels
//!   and other submenus' triggers render hidden, and the drilled-in
//!   submenu's trigger stays above it as the row that goes back. Groups stay,
//!   empty, as do elements of the app's own.
//! - A data menu's item shortcuts register in each root trigger's layer,
//!   from an element the trigger renders, so they leave with the trigger.
//! - `data-*` attributes are state fields: `highlighted`, `pending`,
//!   `error`, `destructive`, `checked`, `has_icon` on items, `has_icons` on
//!   the content and the menubar, `nested`, `presentation`, `presence` and
//!   `placement` on the content. The indicator's enter is a `Presence` whose
//!   timing its props give.
//!
//! # Deviations and drops
//!
//! - `menu-generic.ts`: TypeScript's inference of a radio group's value
//!   type, `T extends string`. Radio values are `SharedString`.
//! - `MenuPrimitive`, `definePrimitive` and `selects`: the component is the
//!   [`Menu`] struct, and its parts render from its state.
//! - The trigger's `controller` prop, which connects a trigger outside the
//!   menu: any part of the window takes the menu's entity, or
//!   `controller.entity(cx)`. The menu's `controller` option stays, a kernel
//!   `Controller` with `PopupControllerExt`'s methods, as does the
//!   menubar's, with [`MenubarControllerExt`].
//! - Rendering a data menu's `items` as parts when the app places no content
//!   (`MenuItems` in the adapters): the app renders the parts from the same
//!   data. `items` still registers the items' shortcuts.
//! - `container` (`PortalProps`): the content renders above the window in
//!   `deferred(..)`, wherever it's declared.
//! - Items rendered as links (`a[href]`, `linkClick`, `taken`, Enter's
//!   `el.click()`): gpui has no links that follow themselves, so an item
//!   navigates in its `on_select`.
//! - `itemSelector`, `byElement` and `composedClosest` over the DOM: items
//!   are found by key, and a press by the regions they laid out in.
//! - `data-highlighted` written without a render: the highlight is state.
//! - `adoptPlatform`'s microtask: the platform is known from the first frame
//!   (`initial_apple`); parts follow a `set_platform` made since.
//! - `triggerElements`, read off the element: a trigger says what it acts as
//!   in [`MenuTriggerProps::element`].
//! - The trigger's `checkName` and the icon's "An item with only an icon":
//!   triggers and items are buttons, links or menu items, which also take
//!   their name from a `text!` child, and the menu can't read it, so both
//!   would warn about every one named by its text.
//! - `firstItemOffset`'s computed style: a submenu lines its first item up
//!   with its trigger from the padding and border its content's style sets,
//!   rems at the window's rem size.
//! - A context menu opened from the keyboard anchors at the context area,
//!   where the original anchors at the focused element inside it: gpui can't
//!   tell where a focus handle laid out. A secondary press on another of the
//!   menu's context areas takes the open menu over, since the layer that
//!   consumes the press keeps the area from hearing it, where the original's
//!   `contextmenu` event still arrives.
//! - `pointerType === "pen"`: gpui reports a pen as a mouse or a finger.
//! - The submenu's `rendered` hook: placement follows every frame.
//! - Menubar triggers that aren't buttons opening on Enter and Space, and
//!   the menubar's `subscribe`, which renders the tab stop on each trigger:
//!   gpui presses the trigger, and the menubar's roving focus renders its
//!   menus again when the stop moves.

pub mod menu;
pub mod menu_api;
pub mod menu_parts;
pub mod menubar;

pub use menu::{
    Menu, MenuActionCallback, MenuActionDetails, MenuCheckboxItemProps, MenuCheckboxItemState,
    MenuCheckedChangeCallback, MenuCheckedChangeDetails, MenuCheckedChangeReason, MenuContentProps, MenuContentState,
    MenuContextTriggerProps, MenuContextTriggerState, MenuController, MenuData, MenuDataEntry, MenuDataItem,
    MenuDataRadio, MenuDismissReason, MenuFinalFocus, MenuItemData, MenuItemIconProps, MenuItemIconState,
    MenuItemIndicatorProps, MenuItemIndicatorState, MenuItemProps, MenuItemState, MenuModal, MenuOpenChangeReason,
    MenuOpenOptions, MenuOptions, MenuPayloadChangeReason, MenuRadioGroupProps, MenuRadioGroupState,
    MenuRadioItemProps, MenuRadioItemState, MenuShortcutProps, MenuShortcutState, MenuSubTriggerProps,
    MenuTriggerProps, MenuTriggerState, MenuValueChangeCallback, MenuValueChangeDetails, MenuValueChangeReason,
    create_menu_controller,
};
pub use menu_api::{Focus, Presentation, SelectCallback};
pub use menu_parts::{MenuContent, MenuIndicatorItem};
pub use menubar::{
    Menubar, MenubarClose, MenubarController, MenubarControllerExt, MenubarMember, MenubarOpen, MenubarOptions,
    MenubarOrientation, MenubarState, create_menubar_controller,
};
pub use ui_sdk_popups::PresentationSetting;
