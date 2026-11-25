//! Port of `components/dialog/dialog.ts`: the dialog, its options, the props
//! and state of its parts, and the types Alert Dialog and Drawer share with
//! it. Its behavior is `dialog_api.rs`'s and its parts are
//! `dialog_parts.rs`'s; what only Dialog has is its presentation: it can
//! present as a sheet, with a viewport around its content, and a refused
//! dismissal nudges the sheet.

use std::fmt::Debug;
use std::rc::Rc;

use gpui::{App, Context, Entity, Hsla, SharedString, Window};
use ui_sdk_kernel::{
    ActionDetails, ActionError, ActionOutcome, Agent, AgentAction, AgentState, ChangeCallback,
    ChangeDetails, ChangeOptions, Controller, DismissAttemptDetails, DismissAttemptReason,
    InputEvent, Instance, Part,
};
use ui_sdk_motion::{PresenceState, PresenceTiming};
use ui_sdk_platform::{KeyboardInset, Region};
use ui_sdk_shortcuts::{ShortcutPart, ShortcutProps, ShortcutState};
use ui_sdk_popups::{
    DismissAttemptCallback, Modal, OpenChangeCompleteCallback, Popup, PopupCloseOptions,
    PopupContent, PopupContentProps, PopupFocusTarget, PopupHost, PopupInitialFocus,
    PopupOpenChangeCallback, PopupOpenOptions, PopupOptions, PopupPending, PopupTriggerState,
    PopupViewport, Presentation, PresentationSetting, SheetPartState, dialog_preset,
    popup_viewport,
};

use crate::alert_dialog::AlertDialog;
use crate::dialog_api::{
    DialogApi, DialogHost, DialogRules, DialogSettings, DialogState, PersistentElements,
};
use crate::dialog_parts::{
    DialogBackdrop, dialog_backdrop, dialog_close, dialog_content, dialog_description,
    dialog_shortcut, dialog_title, dialog_trigger,
};
use crate::dialog_primitive::{
    DefinedDialog, close_action, define_dialog, dialog_agent_state, open_action, set_open_action,
    toggle_action,
};

/// The ways a user can dismiss a dialog, which `dismissible` and
/// `close_on_outside_press` can refuse (`DialogDismissReason`): `EscapeKey`,
/// `OutsidePress`, `Back` and, for one presented as a sheet, `Swipe`.
pub type DialogDismissReason = DismissAttemptReason;

/// What `on_dismiss_attempt` receives when a dismissal was refused
/// (`DialogDismissAttemptDetails`).
pub type DialogDismissAttemptDetails = DismissAttemptDetails;

/// Hears a dialog open or close: the requested `open`, the details, and
/// what closed it, a Close part's `value` or `close`'s own
/// (`DialogOpenChangeDetails`' `returnValue`). As `on_open_change`, it runs
/// once the dialog's update is over, so it may update any entity, the dialog
/// too; as `on_before_open_change`, it runs inside the update, before the
/// dialog opens or closes, where `details.cancel()` stops it.
pub type DialogOpenChangeCallback = PopupOpenChangeCallback;

/// Where focus goes when the dialog opens (`DialogInitialFocus`). `Auto`
/// picks the first tab stop, else the dialog itself; an alert dialog starts
/// on its first Close part, the least destructive action. `Content` focuses
/// the dialog itself and `None` leaves focus where it is. A focus handle, or
/// a function that returns one, picks a specific element.
pub type DialogInitialFocus = PopupInitialFocus;

/// Where focus goes when the dialog closes (`DialogFinalFocus`). `Trigger`
/// returns it to the trigger, or the nearest element that survived when the
/// trigger is gone. `None` leaves it. A focus handle, or a function that
/// returns one, picks a specific element.
pub type DialogFinalFocus = PopupFocusTarget;

/// What a Close part's `action` receives after the event: how the part was
/// pressed (`DialogCloseActionDetails`).
pub type DialogCloseActionDetails = ActionDetails;

/// A Close part's `action`: it runs when the part is pressed, before the
/// dialog closes, with the press and how it was pressed. It runs inside the
/// dialog's update, since the dialog reads what it returns, so it may update
/// any entity but the dialog. Work that
/// goes on ([`ActionOutcome::wait`]) keeps the dialog open while it runs: the
/// part is pending, screen readers hear the dialog's `pending_announcement`
/// after a second, and nothing but its api closes the dialog. It
/// closes with reason `Action` once the work succeeds. A failure keeps it
/// open and puts the error in the part's `error` state until the next press,
/// or until the dialog opens again. [`ActionOutcome::Canceled`] keeps it open
/// with nothing more. `details.source` says how the part was pressed, for
/// changes made after the work.
pub type DialogCloseAction =
    Rc<dyn Fn(Option<&InputEvent>, &DialogCloseActionDetails, &mut Window, &mut App) -> ActionOutcome>;

/// What the overlay host's alert dialog shows when `confirm()` asks without
/// one of yours (`ConfirmOptions`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfirmOptions {
    /// The question, like "Delete this project?".
    pub title: SharedString,
    /// What agreeing does.
    pub description: Option<SharedString>,
    /// The agreeing button's text. Defaults to "Continue".
    pub confirm_label: Option<SharedString>,
    /// The other button's text. Defaults to "Cancel".
    pub cancel_label: Option<SharedString>,
}

impl ConfirmOptions {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn confirm_label(mut self, label: impl Into<SharedString>) -> Self {
        self.confirm_label = Some(label.into());
        self
    }

    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel_label = Some(label.into());
        self
    }

    /// The agreeing button's text, "Continue" unless it names its own.
    pub fn confirm_text(&self) -> SharedString {
        self.confirm_label.clone().unwrap_or_else(|| "Continue".into())
    }

    /// The other button's text, "Cancel" unless it names its own.
    pub fn cancel_text(&self) -> SharedString {
        self.cancel_label.clone().unwrap_or_else(|| "Cancel".into())
    }
}

/// The `data` of a `"confirm"` overlay entry (`ConfirmEntry`): what an
/// overlay host gets for a question `confirm()` asks, to render it as an
/// alert dialog. The host downcasts the entry's data to it.
pub struct ConfirmEntry<P: Clone + PartialEq + Debug + 'static = ConfirmOptions> {
    /// Pass to the alert dialog the host renders.
    pub controller: AlertDialogController<P>,
    /// The question.
    pub payload: Option<P>,
    pub(crate) open: ConfirmOpen,
    pub(crate) done: Rc<dyn Fn(&mut App)>,
}

// What opens a hosted question's alert dialog.
pub(crate) type ConfirmOpen = Rc<dyn Fn(&mut Window, &mut App)>;

impl<P: Clone + PartialEq + Debug + 'static> ConfirmEntry<P> {
    /// Call once the alert dialog is rendered with the controller, outside
    /// its update. It opens it and answers, once.
    pub fn open(&self, window: &mut Window, cx: &mut App) {
        (self.open)(window, cx);
    }

    /// Call once the dialog has finished closing, so the host stops
    /// rendering it.
    pub fn done(&self, cx: &mut App) {
        (self.done)(cx);
    }
}

/// The options `Dialog` takes (`DialogOptions`).
pub struct DialogOptions<P> {
    /// Whether the dialog is open. Pass it to control the dialog.
    pub open: Option<bool>,
    /// Whether the dialog starts open, when uncontrolled. Focus moves into
    /// it once it's in the page.
    pub default_open: bool,
    /// Runs when the dialog opens or closes. `details.reason` says why:
    /// `TriggerPress`, `ClosePress`, `OutsidePress`, `EscapeKey`,
    /// `ParentClose`, `Back`, `Action`, `Shortcut`, `Swipe` or `Imperative`,
    /// and the return value carries what closed it. It runs once the
    /// dialog's update is over, so it may update any entity, the dialog too.
    pub on_open_change: Option<DialogOpenChangeCallback>,
    /// Runs as the dialog is asked to open or close, before it does, inside
    /// the dialog's update: `details.cancel()` stops it. It hears the same
    /// reasons and return value as `on_open_change`. It may update any
    /// entity but the dialog.
    pub on_before_open_change: Option<DialogOpenChangeCallback>,
    /// Runs once the dialog has finished opening or closing, after any
    /// animation, once the dialog's update is over.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Runs when the payload the content shows changes: a trigger with
    /// another payload opens the dialog, or takes a non-modal dialog over
    /// while it's open, which fires no `on_open_change`, and so does
    /// `open(payload)` on the open dialog. `details.reason` is
    /// `TriggerPress`, `Shortcut` or `Imperative`. It runs once the dialog's
    /// update is over.
    pub on_payload_change: Option<ChangeCallback<Option<P>>>,
    /// Runs as the payload is asked to change, before it does, inside the
    /// dialog's update: `details.cancel()` keeps the previous payload. It
    /// may update any entity but the dialog.
    pub on_before_payload_change: Option<ChangeCallback<Option<P>>>,
    /// Whether Escape, an outside press and the back gesture can close the
    /// dialog. Close parts, triggers and the api always can. Pass `false`
    /// while a form has unsaved changes, and ask in `on_dismiss_attempt`.
    /// Defaults to true.
    pub dismissible: bool,
    /// Runs when a dismissal was refused, because `dismissible` is false, an
    /// outside press doesn't close it, or a Close part's action is running.
    /// Ask "Discard changes?" here. It runs once the dialog's update is
    /// over.
    pub on_dismiss_attempt: Option<DismissAttemptCallback>,
    /// Regions that stay usable while the dialog blocks the window, like a
    /// chat launcher, read when it opens. Presses on them never close it.
    pub persistent_elements: Option<PersistentElements>,
    /// What screen readers hear once a Close part's action has been running
    /// for a second, so a slow action doesn't pass in silence. Set it in the
    /// app's language; empty says nothing. Defaults to "Loading".
    pub pending_announcement: Option<SharedString>,
    /// `True`: the rest of the window is blocked and doesn't scroll, focus is
    /// trapped, and an outside press only closes the dialog. `TrapFocus`:
    /// traps focus, and leaves the window scrollable and pressable. `False`:
    /// a dialog beside the window, which Tab can leave and outside presses
    /// don't close. `Pointer` isn't one of its settings. Defaults to `True`.
    pub modal: Modal,
    /// Whether pressing outside the dialog, like on its backdrop, closes it.
    /// The press counts once it's released outside too, so dragging a text
    /// selection out of a field doesn't close it. Defaults to true, or false
    /// when `modal` is `False`.
    pub close_on_outside_press: Option<bool>,
    /// How the dialog presents. `Sheet` slides it up from the bottom of the
    /// window, where it can be swiped away, and `Auto` does that on small
    /// touch screens. Decided each time it opens. Defaults to `Default`, a
    /// dialog.
    pub presentation: PresentationSetting,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl<P> Default for DialogOptions<P> {
    fn default() -> Self {
        Self {
            open: None,
            default_open: false,
            on_open_change: None,
            on_before_open_change: None,
            on_open_change_complete: None,
            on_payload_change: None,
            on_before_payload_change: None,
            dismissible: true,
            on_dismiss_attempt: None,
            persistent_elements: None,
            pending_announcement: None,
            modal: Modal::True,
            close_on_outside_press: None,
            presentation: PresentationSetting::Default,
            debug: false,
        }
    }
}

impl<P> Clone for DialogOptions<P> {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_before_open_change: self.on_before_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            on_before_payload_change: self.on_before_payload_change.clone(),
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            persistent_elements: self.persistent_elements.clone(),
            pending_announcement: self.pending_announcement.clone(),
            modal: self.modal,
            close_on_outside_press: self.close_on_outside_press,
            presentation: self.presentation,
            debug: self.debug,
        }
    }
}

impl<P> DialogOptions<P> {
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// `on_open_change`, for a callback that doesn't read what closed it.
    pub fn on_open_change(mut self, callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change = Some(Rc::new(move |open, details, _, window, cx| callback(open, details, window, cx)));
        self
    }

    /// `on_open_change` with what closed it: a Close part's `value`, or
    /// `close`'s own.
    pub fn on_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    /// `on_before_open_change`, for a callback that doesn't read what
    /// closes it.
    pub fn on_before_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change =
            Some(Rc::new(move |open, details, _, window, cx| callback(open, details, window, cx)));
        self
    }

    /// `on_before_open_change` with what closes it.
    pub fn on_before_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn on_payload_change(
        mut self,
        callback: impl Fn(&Option<P>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_payload_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_payload_change(
        mut self,
        callback: impl Fn(&Option<P>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_payload_change = Some(Rc::new(callback));
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(
        mut self,
        callback: impl Fn(&DialogDismissAttemptDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
        self
    }

    pub fn persistent_elements(mut self, regions: Vec<Region>) -> Self {
        self.persistent_elements = Some(PersistentElements::List(regions));
        self
    }

    /// `persistent_elements` from a function, read each time it opens.
    pub fn persistent_elements_with(mut self, regions: impl Fn() -> Vec<Region> + 'static) -> Self {
        self.persistent_elements = Some(PersistentElements::Fn(Rc::new(regions)));
        self
    }

    pub fn pending_announcement(mut self, announcement: impl Into<SharedString>) -> Self {
        self.pending_announcement = Some(announcement.into());
        self
    }

    pub fn modal(mut self, modal: impl Into<Modal>) -> Self {
        self.modal = modal.into();
        self
    }

    pub fn close_on_outside_press(mut self, close: bool) -> Self {
        self.close_on_outside_press = Some(close);
        self
    }

    pub fn presentation(mut self, presentation: PresentationSetting) -> Self {
        self.presentation = presentation;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    // What the popup reads, and what Dialog's behavior reads.
    fn split(self) -> (PopupOptions<P>, DialogSettings) {
        let settings = DialogSettings {
            dismissible: self.dismissible,
            persistent_elements: self.persistent_elements,
            pending_announcement: self.pending_announcement,
            close_on_outside_press: self.close_on_outside_press,
        };
        let popup = PopupOptions {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change,
            on_before_open_change: self.on_before_open_change,
            on_open_change_complete: self.on_open_change_complete,
            on_payload_change: self.on_payload_change,
            on_before_payload_change: self.on_before_payload_change,
            modal: Some(self.modal),
            presentation: Some(self.presentation),
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt,
            ..PopupOptions::default()
        };
        (popup, settings)
    }
}

/// Opens, closes and inspects an alert dialog from outside it, before it
/// exists too (`AlertDialogController`): a kernel controller with the popup
/// controller's methods, which `confirm()` asks about.
pub type AlertDialogController<P = ()> = Controller<AlertDialog<P>>;

/// Hears an alert dialog asking to open or close, with what closed it: the
/// `value` of the Close part that did. `details.reason` is one of Dialog's,
/// never `OutsidePress` or `Swipe`.
pub type AlertDialogOpenChangeCallback = DialogOpenChangeCallback;

/// The options `AlertDialog` takes (`AlertDialogOptions`): Dialog's, except
/// `modal`, `close_on_outside_press` and `presentation`. It's always modal,
/// never closes on an outside press, and never presents as a sheet.
pub struct AlertDialogOptions<P: Clone + PartialEq + Debug + 'static> {
    /// Whether it's open. Pass it to control it.
    pub open: Option<bool>,
    /// Whether it starts open, when uncontrolled.
    pub default_open: bool,
    /// Runs when it opens or closes. `details.reason` is `TriggerPress`,
    /// `ClosePress`, `EscapeKey`, `ParentClose`, `Back`, `Action`, `Shortcut`
    /// or `Imperative`, and the return value is the `value` of the Close part
    /// that closed it. It runs once the alert dialog's update is over, so it
    /// may update any entity, the alert dialog too.
    pub on_open_change: Option<AlertDialogOpenChangeCallback>,
    /// Runs as it's asked to open or close, before it does, inside the alert
    /// dialog's update: `details.cancel()` stops it. It hears the same
    /// reasons and return value as `on_open_change`. It may update any
    /// entity but the alert dialog.
    pub on_before_open_change: Option<AlertDialogOpenChangeCallback>,
    /// Runs once it has finished opening or closing, after any animation,
    /// once the alert dialog's update is over.
    pub on_open_change_complete: Option<OpenChangeCompleteCallback>,
    /// Runs when a trigger or `open()` with another payload opens it, before
    /// `on_open_change`, or `open(payload)` gives the open one another
    /// payload. `details.reason` is `TriggerPress`, `Shortcut` or
    /// `Imperative`. It runs once the alert dialog's update is over.
    pub on_payload_change: Option<ChangeCallback<Option<P>>>,
    /// Runs as the payload is asked to change, before it does, inside the
    /// alert dialog's update: `details.cancel()` keeps the previous payload.
    /// It may update any entity but the alert dialog.
    pub on_before_payload_change: Option<ChangeCallback<Option<P>>>,
    /// Whether Escape and the back gesture can close it. Defaults to true.
    pub dismissible: bool,
    /// Runs when a dismissal was refused, because `dismissible` is false or
    /// a Close part's action is running, once the alert dialog's update is
    /// over.
    pub on_dismiss_attempt: Option<DismissAttemptCallback>,
    /// Regions that stay usable while it blocks the window.
    pub persistent_elements: Option<PersistentElements>,
    /// What screen readers hear once a Close part's action has been running
    /// for a second. Defaults to "Loading".
    pub pending_announcement: Option<SharedString>,
    /// Opens, closes and inspects it from anywhere, before it exists too.
    /// Make one with `create_alert_dialog_controller()` or `create_confirm()`.
    pub controller: Option<AlertDialogController<P>>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl<P: Clone + PartialEq + Debug + 'static> Default for AlertDialogOptions<P> {
    fn default() -> Self {
        Self {
            open: None,
            default_open: false,
            on_open_change: None,
            on_before_open_change: None,
            on_open_change_complete: None,
            on_payload_change: None,
            on_before_payload_change: None,
            dismissible: true,
            on_dismiss_attempt: None,
            persistent_elements: None,
            pending_announcement: None,
            controller: None,
            debug: false,
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> Clone for AlertDialogOptions<P> {
    fn clone(&self) -> Self {
        Self {
            open: self.open,
            default_open: self.default_open,
            on_open_change: self.on_open_change.clone(),
            on_before_open_change: self.on_before_open_change.clone(),
            on_open_change_complete: self.on_open_change_complete.clone(),
            on_payload_change: self.on_payload_change.clone(),
            on_before_payload_change: self.on_before_payload_change.clone(),
            dismissible: self.dismissible,
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            persistent_elements: self.persistent_elements.clone(),
            pending_announcement: self.pending_announcement.clone(),
            controller: self.controller.clone(),
            debug: self.debug,
        }
    }
}

impl<P: Clone + PartialEq + Debug + 'static> AlertDialogOptions<P> {
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// `on_open_change`, for a callback that doesn't read what closed it.
    pub fn on_open_change(mut self, callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change = Some(Rc::new(move |open, details, _, window, cx| callback(open, details, window, cx)));
        self
    }

    /// `on_open_change` with the `value` of the Close part that closed it.
    pub fn on_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    /// `on_before_open_change`, for a callback that doesn't read what
    /// closes it.
    pub fn on_before_open_change(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change =
            Some(Rc::new(move |open, details, _, window, cx| callback(open, details, window, cx)));
        self
    }

    /// `on_before_open_change` with what closes it.
    pub fn on_before_open_change_with_value(
        mut self,
        callback: impl Fn(&bool, &ChangeDetails, Option<&SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_open_change = Some(Rc::new(callback));
        self
    }

    pub fn on_open_change_complete(mut self, callback: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_open_change_complete = Some(Rc::new(callback));
        self
    }

    pub fn on_payload_change(
        mut self,
        callback: impl Fn(&Option<P>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_payload_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_payload_change(
        mut self,
        callback: impl Fn(&Option<P>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_payload_change = Some(Rc::new(callback));
        self
    }

    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(
        mut self,
        callback: impl Fn(&DialogDismissAttemptDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
        self
    }

    pub fn persistent_elements(mut self, regions: Vec<Region>) -> Self {
        self.persistent_elements = Some(PersistentElements::List(regions));
        self
    }

    /// `persistent_elements` from a function, read each time it opens.
    pub fn persistent_elements_with(mut self, regions: impl Fn() -> Vec<Region> + 'static) -> Self {
        self.persistent_elements = Some(PersistentElements::Fn(Rc::new(regions)));
        self
    }

    pub fn pending_announcement(mut self, announcement: impl Into<SharedString>) -> Self {
        self.pending_announcement = Some(announcement.into());
        self
    }

    pub fn controller(mut self, controller: AlertDialogController<P>) -> Self {
        self.controller = Some(controller);
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `trigger` slot takes (`DialogTriggerProps`). A trigger
/// renders wherever the app renders it, so one per table row needs no
/// controller.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogTriggerProps<P> {
    /// Stops the trigger from opening the dialog. It leaves the tab order,
    /// unless `focusable_when_disabled` is set.
    pub disabled: bool,
    /// Keeps a disabled trigger in the tab order, reporting it disabled, so
    /// it can host a tooltip that says why. Presses still do nothing.
    pub focusable_when_disabled: bool,
    /// Data for the content to render when this trigger opens the dialog,
    /// like the row it belongs to.
    pub payload: Option<P>,
    /// A keyboard shortcut that presses the trigger, like `"mod+k"`. `mod`
    /// is ⌘ on Apple platforms and Control elsewhere. Sets
    /// `aria-keyshortcuts`, and a Shortcut part inside shows it. A modal
    /// dialog pauses the window's shortcuts while it's open.
    pub shortcut: Option<SharedString>,
}

impl<P> Default for DialogTriggerProps<P> {
    fn default() -> Self {
        Self {
            disabled: false,
            focusable_when_disabled: false,
            payload: None,
            shortcut: None,
        }
    }
}

impl<P> DialogTriggerProps<P> {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn focusable_when_disabled(mut self, focusable: bool) -> Self {
        self.focusable_when_disabled = focusable;
        self
    }

    pub fn payload(mut self, payload: P) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
}

/// The props the `content` slot takes (`DialogContentProps`).
#[derive(Clone, Default)]
pub struct DialogContentProps {
    /// Where focus goes when the dialog opens. Opening with touch focuses
    /// the dialog itself, so no on-screen keyboard appears. Defaults to
    /// `Auto`.
    pub initial_focus: DialogInitialFocus,
    /// Where focus goes when the dialog closes while focus is inside it.
    /// Defaults to `Trigger`.
    pub final_focus: Option<DialogFinalFocus>,
    /// Whether the dialog stays in the page, hidden, while closed. Its
    /// content keeps its state between opens.
    pub keep_mounted: bool,
    /// Renders the dialog and its backdrop where they are in the window
    /// instead of above it, so it can stay inside a frame, like an app
    /// preview. Pair it with `modal: TrapFocus` to keep the rest of the
    /// window usable.
    pub inline: bool,
    /// How it animates in and out, the transitions its CSS would declare.
    pub timing: Option<PresenceTiming>,
    /// The dim behind it while it presents as a sheet, at its darkest
    /// (`--sheet-backdrop`). Defaults to black at 0.2.
    pub sheet_backdrop: Option<Hsla>,
}

impl DialogContentProps {
    pub fn initial_focus(mut self, focus: impl Into<DialogInitialFocus>) -> Self {
        self.initial_focus = focus.into();
        self
    }

    pub fn final_focus(mut self, focus: impl Into<DialogFinalFocus>) -> Self {
        self.final_focus = Some(focus.into());
        self
    }

    pub fn keep_mounted(mut self, keep_mounted: bool) -> Self {
        self.keep_mounted = keep_mounted;
        self
    }

    pub fn inline(mut self, inline: bool) -> Self {
        self.inline = inline;
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

    /// The popup content props they amount to.
    pub fn popup_props(&self) -> PopupContentProps {
        PopupContentProps {
            initial_focus: self.initial_focus.clone(),
            final_focus: self.final_focus.clone(),
            keep_mounted: self.keep_mounted,
            inline: self.inline,
            timing: self.timing.clone(),
            sheet_backdrop: self.sheet_backdrop,
            ..PopupContentProps::default()
        }
    }
}

/// The props the `close` slot takes (`DialogCloseProps`).
#[derive(Clone, Default)]
pub struct DialogCloseProps {
    /// Reported as the return value to `on_open_change` and the future
    /// `open()` returned. `confirm()` answers yes for `"confirm"`.
    pub value: Option<SharedString>,
    /// Runs when the part is pressed, before the dialog closes. See
    /// [`DialogCloseAction`].
    pub action: Option<DialogCloseAction>,
}

impl DialogCloseProps {
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn action(
        mut self,
        action: impl Fn(Option<&InputEvent>, &DialogCloseActionDetails, &mut Window, &mut App) -> ActionOutcome + 'static,
    ) -> Self {
        self.action = Some(Rc::new(action));
        self
    }
}

/// The props the `shortcut` slot takes (`DialogShortcutProps`): the keys to
/// show. Inside a trigger, or beside one, by default its `shortcut`.
pub type DialogShortcutProps = ShortcutProps;

/// What the `trigger` part styles from (`DialogTriggerState`): whether the
/// dialog is open from this trigger, and whether its `disabled` prop stops
/// it opening the dialog.
pub type DialogTriggerState = PopupTriggerState;

/// What the `backdrop` part styles from (`DialogBackdropState`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DialogBackdropState {
    /// Whether the dialog is open. False while it animates out.
    pub open: bool,
    /// Whether the dialog was opened from inside another dialog.
    pub nested: bool,
    /// Whether a dialog opened from inside this one is open.
    pub nested_open: bool,
    /// Its enter and exit, the original's `data-starting-style` and
    /// `data-ending-style`.
    pub presence: PresenceState,
}

/// What the `content` part styles from (`DialogContentState`), with what
/// the original writes as attributes and variables.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogContentState<P> {
    /// Whether the dialog is open. False while it animates out.
    pub open: bool,
    /// The `modal` setting. Always `True` for an alert dialog.
    pub modal: Modal,
    /// Whether the dialog was opened from inside another dialog.
    pub nested: bool,
    /// Whether a dialog opened from inside this one is open.
    pub nested_open: bool,
    /// How many dialogs are open above this one, nested inside it
    /// (`--dialog-stack`).
    pub stack: usize,
    /// The payload of the last open, from the trigger that opened it or the
    /// api.
    pub payload: Option<P>,
    /// Whether a Close part's action is running.
    pub pending: bool,
    /// How it presents this time it's open. An alert dialog is always a
    /// dialog.
    pub presentation: Presentation,
    /// Its enter and exit, the original's `data-starting-style` and
    /// `data-ending-style`.
    pub presence: PresenceState,
    /// Kept in the page while closed, it renders hidden.
    pub hidden: bool,
    /// How much the on-screen keyboard covers, unless it presents as a sheet,
    /// which follows the keyboard itself (`--keyboard-inset`,
    /// `--visual-viewport-height`, `data-keyboard`).
    pub keyboard: KeyboardInset,
    /// The sheet it presents as, while it does: its dim, progress and
    /// expand values, opacity and drag (`--drawer-*`).
    pub sheet: Option<SheetPartState>,
    /// While it presents as a sheet, how far the sheets opened from it push
    /// it back (`--drawer-stack`).
    pub sheet_stack: f64,
}

/// What the `close` part styles from (`DialogCloseState`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DialogCloseState {
    /// Whether this part's action is running.
    pub pending: bool,
    /// Why this part's last action failed, until it's pressed again or the
    /// dialog opens again.
    pub error: Option<ActionError>,
}

/// What the `shortcut` part styles from (`DialogShortcutState`): the
/// shortcut formatted for this platform, `⌘K` on Apple platforms and
/// `Ctrl+K` elsewhere, empty without one.
pub type DialogShortcutState = ShortcutState;

/// The props the `trigger` slot of `AlertDialog` takes
/// (`AlertDialogTriggerProps`).
pub type AlertDialogTriggerProps<P> = DialogTriggerProps<P>;
/// The props the `content` slot of `AlertDialog` takes
/// (`AlertDialogContentProps`).
pub type AlertDialogContentProps = DialogContentProps;
/// The props the `close` slot of `AlertDialog` takes
/// (`AlertDialogCloseProps`).
pub type AlertDialogCloseProps = DialogCloseProps;
/// What a `close` part's `action` receives after the event
/// (`AlertDialogCloseActionDetails`).
pub type AlertDialogCloseActionDetails = ActionDetails;
/// The props the `shortcut` slot of `AlertDialog` takes
/// (`AlertDialogShortcutProps`).
pub type AlertDialogShortcutProps = DialogShortcutProps;
/// What the `trigger` part of `AlertDialog` styles from
/// (`AlertDialogTriggerState`).
pub type AlertDialogTriggerState = DialogTriggerState;
/// What the `backdrop` part of `AlertDialog` styles from
/// (`AlertDialogBackdropState`).
pub type AlertDialogBackdropState = DialogBackdropState;
/// What the `content` part of `AlertDialog` styles from
/// (`AlertDialogContentState`). Its `presentation` is always `Dialog`: an
/// alert dialog never presents as a sheet.
pub type AlertDialogContentState<P> = DialogContentState<P>;
/// What the `close` part of `AlertDialog` styles from
/// (`AlertDialogCloseState`).
pub type AlertDialogCloseState = DialogCloseState;
/// What the `shortcut` part of `AlertDialog` styles from
/// (`AlertDialogShortcutState`).
pub type AlertDialogShortcutState = DialogShortcutState;

/// A dialog. Its entity is its controller: the app calls its methods to
/// open, close and inspect it, from a menu item, a hotkey, or triggers
/// anywhere in the window. `P` is the type of the data triggers and `open()`
/// pass to the content.
pub struct Dialog<P: Clone + PartialEq + Debug + 'static = ()> {
    instance: Instance,
    popup: Popup<Self>,
    dialog: DialogApi<Self>,
}

impl<P: Clone + PartialEq + Debug + 'static> Dialog<P> {
    pub fn new(options: DialogOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let debug = options.debug;
        let (popup, settings) = options.split();
        let DefinedDialog { instance, popup, dialog } =
            define_dialog("dialog", false, popup, settings, debug, presentable(), window, cx);
        Self { instance, popup, dialog }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: DialogOptions<P>, cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
        let (popup, settings) = options.split();
        self.dialog.set_settings(settings);
        self.popup.set_options(popup, cx);
    }

    /// Passes the controlled `open`, or `None` to let the dialog keep its
    /// own. It opens or closes once the current update is over.
    pub fn set_controlled_open(&mut self, open: Option<bool>, cx: &mut Context<Self>) {
        self.popup.set_controlled_open(open, cx);
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// Its behavior, which its parts call.
    pub fn api(&self) -> &DialogApi<Self> {
        &self.dialog
    }

    /// Its state, as its parts and the app read it.
    pub fn state(&self) -> DialogState<P> {
        self.dialog.state(self)
    }

    /// Whether it's open. False as soon as it starts closing, while it still
    /// animates out.
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// Opens or closes it. Returns false when the change was canceled or it
    /// was already open or closed. Closing this way always works, even when
    /// it isn't dismissible or an action is running.
    pub fn set_open(&mut self, open: bool, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.set_open(self, open, options, window, cx)
    }

    /// Opens it, and resolves once it closes, with why, how and any return
    /// value. A payload replaces the open dialog's.
    pub fn open(&mut self, options: PopupOpenOptions<P>, window: &mut Window, cx: &mut Context<Self>) -> PopupPending {
        let popup = self.popup.clone();
        popup.open(self, options, window, cx)
    }

    /// Closes it, with a return value for the future `open()` returned.
    /// Returns false when the close was canceled or it was already closed.
    pub fn close(&mut self, options: PopupCloseOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.close(self, options, window, cx)
    }

    /// Opens it when it's closed, and closes it when it's open.
    pub fn toggle(&mut self, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let popup = self.popup.clone();
        popup.toggle(self, options, window, cx)
    }

    /// The payload of the last open.
    pub fn payload(&self) -> Option<P> {
        self.popup.payload()
    }

    /// Moves focus to the trigger that opened it last, or the first trigger.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.popup.focus(window, cx);
    }

    /// The `trigger` part: a button that opens the dialog, and presses
    /// itself with its `shortcut`. Several can share the dialog, each with a
    /// key and a payload.
    pub fn trigger(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: DialogTriggerProps<P>,
        cx: &App,
    ) -> Part<DialogTriggerState> {
        dialog_trigger(this, key, props, cx)
    }

    /// The `backdrop` part: it dims the window behind a modal dialog, below
    /// it above the window, and a press on it is an outside press. `None`
    /// while it's out of the page. Declare it before the content.
    pub fn backdrop(
        this: &Entity<Self>,
        timing: Option<PresenceTiming>,
        cx: &App,
    ) -> Option<DialogBackdrop<Self, DialogBackdropState>> {
        dialog_backdrop(this, timing, cx)
    }

    /// The `viewport` part: the window-sized element the content sits in
    /// while it presents as a sheet. Put the content inside it. `None` unless
    /// it presents as a sheet and is in the page.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> Option<PopupViewport<Self>> {
        popup_viewport(this, cx)
    }

    /// The `content` part: the dialog itself, named by its title, else its
    /// trigger, and described by its description. `None` while it's out of
    /// the page; render it every frame, so it hears its props while closed.
    pub fn content(
        this: &Entity<Self>,
        props: DialogContentProps,
        cx: &App,
    ) -> Option<PopupContent<Self, DialogContentState<P>>> {
        dialog_content(this, props, cx)
    }

    /// The `title` part, which names the dialog. Give it its text as an
    /// `aria_label`, the dialog's name, and draw the same text inside it.
    pub fn title(this: &Entity<Self>, cx: &App) -> Part<()> {
        dialog_title(this, cx)
    }

    /// The `description` part, which describes the dialog while it renders.
    /// Give it its text as an `aria_label`.
    pub fn description(this: &Entity<Self>, cx: &App) -> Part<()> {
        dialog_description(this, cx)
    }

    /// The `close` part: a button that closes the dialog with its `value`,
    /// after its `action`. A dialog can have several, each with a key.
    pub fn close_part(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: DialogCloseProps,
        cx: &App,
    ) -> Part<DialogCloseState> {
        dialog_close(this, key, props, cx)
    }

    /// The `shortcut` part: the keys of a shortcut, for people to see.
    /// Without keys of its own, it shows the shortcut of the trigger it sits
    /// in or beside, and without children it shows them itself.
    pub fn shortcut(
        this: &Entity<Self>,
        key: Option<SharedString>,
        props: DialogShortcutProps,
        cx: &App,
    ) -> ShortcutPart {
        dialog_shortcut(this, key, props, cx)
    }
}

// A dialog's presentation, which an alert dialog and a drawer don't carry: it
// presents as a sheet with Dialog's preset, and a refused dismissal nudges the
// sheet.
fn presentable<P: Clone + PartialEq + Debug + 'static>() -> DialogRules<Dialog<P>> {
    DialogRules {
        preset: Some(dialog_preset()),
        on_refuse: Some(Rc::new(|this: &mut Dialog<P>, _reason, window, cx| {
            if let Some(sheet) = this.popup.sheet() {
                sheet.nudge(this, window, cx);
            }
        })),
        ..DialogRules::default()
    }
}

impl<P: Clone + PartialEq + Debug + 'static> PopupHost for Dialog<P> {
    type Payload = P;

    fn instance(&self) -> &Instance {
        &self.instance
    }

    fn popup(&self) -> &Popup<Self> {
        &self.popup
    }
}

impl<P: Clone + PartialEq + Debug + 'static> DialogHost for Dialog<P> {
    fn dialog(&self) -> &DialogApi<Self> {
        &self.dialog
    }
}

/// `agent: { actions: ["setOpen"] }`, with `open`, `close` and `toggle`,
/// which `setOpen` covers, unlisted.
impl<P: Clone + PartialEq + Debug + 'static> Agent for Dialog<P> {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setOpen", set_open_action::<Self>),
        AgentAction::unlisted("open", open_action::<Self>),
        AgentAction::unlisted("close", close_action::<Self>),
        AgentAction::unlisted("toggle", toggle_action::<Self>),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        dialog_agent_state(self)
    }
}
