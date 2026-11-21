//! Port of `foundation/popups/popup-presets.ts`: the presets
//! [`create_popup`](fn@crate::create_popup) takes, as values: each names its row
//! in the behavior table and the features it needs. The presets of popups a
//! press opens are here, with the sheet they can present as; hover presets
//! are in [`popup_hover_presets`](crate::popup_hover_presets).

use std::time::Duration;

use ui_sdk_kernel::roles::Role;
use ui_sdk_platform::Side;

use crate::popup::Modal;
use crate::popup_behavior::PopupBehaviorName;

/// How a kind of popup behaves, and which features it has (`PopupPreset`).
/// Pass one of the presets to [`create_popup`](fn@crate::create_popup), like
/// [`popover_preset`].
#[derive(Clone, Copy, Debug)]
pub struct PopupPreset {
    /// How Tab, Escape, outside presses and the window losing focus behave,
    /// by the pattern it follows.
    pub behavior: PopupBehaviorName,
    /// The content's role. None for content that isn't a widget of its own,
    /// like a hover card.
    pub role: Option<Role>,
    /// The `modal` it has when the component's option doesn't say. Without
    /// one, the component keeps `modal` itself, or it's never modal.
    pub modal: Option<Modal>,
    /// The side it opens on when its content doesn't say.
    pub side: Option<Side>,
    /// How long the pointer rests on the trigger before it opens, unless the
    /// `delay` option says.
    pub delay: Option<Duration>,
    /// How long before it closes once the pointer has left, unless the
    /// `close_delay` option says.
    pub close_delay: Option<Duration>,
    /// How long keyboard focus rests on the trigger before it opens. `delay`
    /// by default.
    pub focus_delay: Option<Duration>,
    /// The data each trigger hands the content, its `payload`.
    pub payload: bool,
    /// Placement beside the trigger.
    pub placement: bool,
    /// Presenting as a sheet, with the `presentation` option.
    pub presentation: bool,
    /// `dismissible` and `on_dismiss_attempt`. Hover content takes neither,
    /// since WCAG 1.4.13 needs it dismissible.
    pub dismissal: bool,
    /// Opening on a press, with a button trigger and focus moving in. Without
    /// it, resting the pointer and keyboard focus open it.
    pub press: bool,
    /// Opening when the pointer rests on the trigger.
    pub hover: bool,
    /// Opening when keyboard focus rests on the trigger.
    pub focus: bool,
    /// One open at a time, the next opening at once while one is warm.
    pub group: bool,
    /// Naming an untitled popup after the trigger that opened it.
    pub trigger_name: bool,
}

impl PopupPreset {
    /// A preset of `behavior` with no features, for a component to build up.
    pub const fn new(behavior: PopupBehaviorName) -> Self {
        Self {
            behavior,
            role: None,
            modal: None,
            side: None,
            delay: None,
            close_delay: None,
            focus_delay: None,
            payload: false,
            placement: false,
            presentation: false,
            dismissal: false,
            press: false,
            hover: false,
            focus: false,
            group: false,
            trigger_name: false,
        }
    }
}

/// Rich content opened from a button: a non-modal dialog by default, part of
/// the window's Tab order, placed beside its trigger, that can open on hover
/// with `open_on_hover` and present as a sheet (`popoverPreset`).
pub const fn popover_preset() -> PopupPreset {
    PopupPreset {
        payload: true,
        role: Some(Role::Dialog),
        dismissal: true,
        modal: Some(Modal::False),
        placement: true,
        presentation: true,
        press: true,
        hover: true,
        ..PopupPreset::new(PopupBehaviorName::Popover)
    }
}

/// A modal dialog by default: it holds focus, blocks the window and can
/// present as a sheet. Not placed beside its trigger. An untitled one is
/// named after the trigger that opened it (`dialogPreset`).
pub const fn dialog_preset() -> PopupPreset {
    PopupPreset {
        payload: true,
        role: Some(Role::Dialog),
        dismissal: true,
        modal: Some(Modal::True),
        presentation: true,
        press: true,
        trigger_name: true,
        ..PopupPreset::new(PopupBehaviorName::Dialog)
    }
}

/// Alert Dialog's: a dialog that never presents as a sheet. Alert Dialog
/// keeps itself modal, whatever its options say, so the preset names no
/// `modal` (`alertDialogPreset`).
pub const fn alert_dialog_preset() -> PopupPreset {
    PopupPreset {
        payload: true,
        role: Some(Role::AlertDialog),
        dismissal: true,
        press: true,
        trigger_name: true,
        ..PopupPreset::new(PopupBehaviorName::Dialog)
    }
}

/// Drawer's: a dialog that is a sheet of its own. Its snap point decides
/// whether it's modal, so the preset names no `modal` (`drawerPreset`).
pub const fn drawer_preset() -> PopupPreset {
    PopupPreset {
        payload: true,
        role: Some(Role::Dialog),
        dismissal: true,
        press: true,
        trigger_name: true,
        ..PopupPreset::new(PopupBehaviorName::Dialog)
    }
}

/// Menu's: placed beside its trigger, presenting as a sheet, and handing
/// each trigger's payload to the content. Its own triggers open it, and it
/// keeps its `modal` itself, since a submenu and a menubar's menus share
/// their root's (`menuPreset`).
pub const fn menu_preset() -> PopupPreset {
    PopupPreset {
        payload: true,
        role: Some(Role::Menu),
        dismissal: true,
        placement: true,
        presentation: true,
        ..PopupPreset::new(PopupBehaviorName::Menu)
    }
}

/// Select's: like a native select, Tab closes it and focus moves on from the
/// trigger. Placed below its trigger, modal to the pointer, and presenting as
/// a sheet. Its own trigger and list do the rest (`selectPreset`).
pub const fn select_preset() -> PopupPreset {
    PopupPreset {
        modal: Some(Modal::Pointer),
        dismissal: true,
        placement: true,
        presentation: true,
        ..PopupPreset::new(PopupBehaviorName::Select)
    }
}

/// The list of Combobox and Autocomplete: focus stays in their input while
/// it's open, so Tab is gpui's. Placed below the input and presenting as a
/// sheet. Their own input, triggers and list do the rest (`comboboxPreset`).
pub const fn combobox_preset() -> PopupPreset {
    PopupPreset {
        modal: Some(Modal::False),
        dismissal: true,
        placement: true,
        presentation: true,
        ..PopupPreset::new(PopupBehaviorName::Combobox)
    }
}

/// Date Picker's calendar: placed beside its input and presenting as a
/// sheet, and never modal. Its own trigger and inputs open it, so it carries
/// no presses or payloads (`datePickerPreset`).
pub const fn date_picker_preset() -> PopupPreset {
    PopupPreset {
        dismissal: true,
        placement: true,
        presentation: true,
        ..PopupPreset::new(PopupBehaviorName::Popover)
    }
}

/// Trigger Menu's list: opened by a character typed in an editor and placed
/// at the caret, while focus stays in the editor. It never presents as a
/// sheet (`triggerMenuPreset`).
pub const fn trigger_menu_preset() -> PopupPreset {
    PopupPreset {
        dismissal: true,
        placement: true,
        ..PopupPreset::new(PopupBehaviorName::Combobox)
    }
}
