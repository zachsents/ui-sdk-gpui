//! Port of `components/dialog`: a dialog, which gives a task its own space
//! over the window, and an alert dialog, which asks a question before an
//! action goes on, with `confirm()` on top of it. Drawer builds on the same
//! behavior and parts.
//!
//! # TypeScript file → Rust module
//!
//! `dialog.ts` → [`dialog`] ([`Dialog`], its options, the props and state of
//! the parts, and the types Alert Dialog shares), `dialog-api.ts` →
//! [`dialog_api`](mod@dialog_api) ([`DialogApi`], made by [`dialog_api()`], with the
//! [`DialogHost`] a component implements), `dialog-parts.ts` →
//! [`dialog_parts`] (one builder per part), `dialog-primitive.ts` →
//! [`dialog_primitive`] ([`define_dialog`] and the agent surface),
//! `alert-dialog.ts` → [`alert_dialog`] ([`AlertDialog`],
//! [`create_alert_dialog_controller`], [`create_confirm`] and [`confirm`]).
//!
//! # In gpui
//!
//! - A component built on Dialog holds the `Popup` and the [`DialogApi`] that
//!   [`dialog_api()`] makes and implements [`DialogHost`]. Dialog, Alert
//!   Dialog and Drawer are three such components; their part methods call
//!   the builders in [`dialog_parts`].
//! - The content and its backdrop render above the window, as the popups
//!   crate places them: declare the backdrop before the content, and center
//!   the content with `.m_auto()`. Render the content every frame, so it
//!   hears its props while closed. While a dialog presents as a sheet, put
//!   its content inside its viewport.
//! - What the original writes as attributes and CSS variables is part state:
//!   `data-nested`, `data-nested-open`, `--dialog-stack` (`stack`),
//!   `data-presentation`, `data-pending`, the keyboard's
//!   `--keyboard-inset` and `data-keyboard` (`keyboard`), and a sheet's
//!   `--drawer-*` (`sheet`, `sheet_stack`). Enter and exit are the
//!   `presence` the popups crate runs, with the content's `timing`.
//! - The payload triggers and `open()` hand the content is the type
//!   parameter `P`, `()` by default.
//! - A Close part's `action` says with its `ActionOutcome` whether its work
//!   goes on, which keeps the dialog open and the part pending. Close parts
//!   take a key, since pending and errors are per part.
//! - The `on_open_change` option hears what closed the dialog beside its
//!   details, the original's `details.returnValue`, as the popups crate
//!   passes it. Set it with
//!   [`on_open_change_with_value`](DialogOptions::on_open_change_with_value)
//!   to read it; the [`on_open_change`](DialogOptions::on_open_change)
//!   builder leaves it out.
//! - Reasons are the kernel's `Reason`; each callback lists its subset.
//! - The trigger is the popups crate's, through
//!   [`popup_trigger_with`](ui_sdk_popups::popup_trigger_with), which leaves
//!   out `aria-expanded` on a modal dialog's trigger.
//! - The backdrop is [`DialogBackdrop`], the popups crate's backdrop with the
//!   dialog's own state, like `nested` or a drawer's dim.
//! - The Shortcut part is the shortcuts crate's: without keys of its own it
//!   shows those of the trigger it sits in or beside, found where it
//!   renders.
//! - `confirm()` is a [`Confirm`]: [`Confirm::ask`] returns a future of the
//!   answer, and [`Confirm::controller`] is what a callable with a
//!   `controller` property carried. A host renders `"confirm"` overlay
//!   entries, whose data is a [`ConfirmEntry`], and calls its `open` and
//!   `done`.
//! - Nested dialogs close once the update that closes the dialog around
//!   them is over, not inside it: a nested one may be what's closing it,
//!   like its Close part's action, and gpui can't update an entity from
//!   inside its own update. Its `on_open_change` still hears `ParentClose`
//!   after the outer one's, and focus in it, which counts as inside the
//!   outer one, goes back to where the outer one opened from.
//! - The description part carries its text as `aria_label`, which the
//!   kernel describes the content with through `described_by`: gpui reads no
//!   text from its `text!` children.
//!
//! # Dropped, and why
//!
//! - `createDialogController`, `DialogController`, the `controller` option
//!   and the trigger's `controller` prop: the app holds the entity, whose
//!   methods are the controller's, and a trigger is a part method that
//!   renders wherever the app renders it. Alert Dialog keeps its
//!   `controller` option and [`create_alert_dialog_controller`], since
//!   `confirm()` asks whether a dialog of yours is attached.
//! - `PortalProps` and `container`: `deferred` renders content above the
//!   window.
//! - `measureScrollerEarly` and `measureViewportEarly`, read as a dialog
//!   opens: the popups crate's blocker is the scroll lock and reads nothing,
//!   and the keyboard comes from the window's insets.
//! - `preferredFocus`'s `[autofocus]` and `[data-autofocus]`: gpui has no
//!   such attribute; `initial_focus` takes a focus handle. An alert dialog
//!   still starts on its first Close part.
//! - The presets' `resetStyles`: browser popover styles gpui doesn't have.
//! - The content's development warning about rendering inside a form: gpui
//!   forms nest, and a control belongs to the nearest one around it.
//! - `adoptPlatform`'s microtask: gpui knows the platform from the first
//!   frame; parts still follow a `set_platform` made since as they render.
//! - `isBrowser` checks: there is always a window.

pub mod alert_dialog;
pub mod dialog;
pub mod dialog_api;
pub mod dialog_parts;
pub mod dialog_primitive;

pub use alert_dialog::{
    AlertDialog, Confirm, ConfirmAnswer, confirm, create_alert_dialog_controller, create_confirm,
};
pub use dialog::{
    AlertDialogBackdropState, AlertDialogCloseActionDetails, AlertDialogCloseProps,
    AlertDialogCloseState, AlertDialogContentProps, AlertDialogContentState,
    AlertDialogController, AlertDialogOpenChangeCallback, AlertDialogOptions,
    AlertDialogShortcutProps, AlertDialogShortcutState, AlertDialogTriggerProps,
    AlertDialogTriggerState, ConfirmEntry, ConfirmOptions, Dialog, DialogBackdropState,
    DialogCloseAction, DialogCloseActionDetails, DialogCloseProps, DialogCloseState,
    DialogContentProps, DialogContentState, DialogDismissAttemptDetails, DialogDismissReason,
    DialogFinalFocus, DialogInitialFocus, DialogOpenChangeCallback, DialogOptions,
    DialogShortcutProps, DialogShortcutState, DialogTriggerProps, DialogTriggerState,
};
pub use dialog_api::{
    DialogApi, DialogHook, DialogHost, DialogRules, DialogSettings, DialogState, InterceptFn,
    OWN_CONTENT, PersistentElements, RefusalPolicy, dialog_api, dialog_refusal,
};
pub use dialog_parts::{
    DialogBackdrop, backdrop_part, content_keydown, dialog_backdrop, dialog_close, dialog_content,
    dialog_description, dialog_shortcut, dialog_title, dialog_trigger,
};
pub use dialog_primitive::{DefinedDialog, define_dialog, synced_modal};
pub use ui_sdk_popups::{
    Modal, PopupCloseOptions, PopupOpenOptions, PopupPending, PopupResult, PresentationSetting,
    alert_dialog_preset, dialog_preset,
};
pub use ui_sdk_shortcuts::{ShortcutPart, ShortcutProps, ShortcutState};
