//! Port of `components/dialog/dialog-parts.ts`: the parts Dialog, Alert
//! Dialog and Drawer share. Each component's part methods call these, and
//! build their own where they differ, like a drawer's backdrop and content.

use std::rc::Rc;

use gpui::{
    App, Entity, InteractiveElement, KeyDownEvent, SharedString, StatefulInteractiveElement,
    WeakEntity, Window,
};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, FocusMode, InputEvent, Part};
use ui_sdk_motion::PresenceTiming;
use ui_sdk_platform::{Region, aria_keyshortcuts};
use ui_sdk_shortcuts::{
    ShortcutLayer, ShortcutOptions, ShortcutPart, bind_part_shortcut, shortcut_of, shortcut_slot,
};
use ui_sdk_popups::{
    PopupBackdrop, PopupContent, PopupTriggerProps, Presentation, TriggerHandlers,
    bind_popup_content, owns_event_among, popup_backdrop_with, popup_description, popup_title,
    popup_trigger_with,
};

use crate::dialog::{
    DialogBackdropState, DialogCloseProps, DialogCloseState, DialogContentProps,
    DialogContentState, DialogShortcutProps, DialogTriggerProps, DialogTriggerState,
};
use crate::dialog_api::{DialogHost, OWN_CONTENT};

/// A trigger: a popup trigger whose `shortcut` presses it. A modal dialog's
/// trigger is blocked while it's open, and "collapsed" once it closes says
/// nothing useful, so it leaves `aria-expanded` out. A non-modal one keeps
/// it, and so does one that only traps focus, since the window still takes
/// presses.
pub fn dialog_trigger<T: DialogHost>(
    this: &Entity<T>,
    key: impl Into<SharedString>,
    props: DialogTriggerProps<T::Payload>,
    cx: &App,
) -> Part<DialogTriggerState> {
    let key = key.into();
    let host = this.read(cx);
    let popup = host.popup().clone();
    let dialog = host.dialog().clone();
    dialog.adopt_platform(cx);
    dialog.set_trigger_shortcut(&key, props.shortcut.clone());
    let trigger_props = PopupTriggerProps {
        disabled: props.disabled,
        focusable_when_disabled: props.focusable_when_disabled,
        payload: props.payload.clone(),
    };
    let handlers = TriggerHandlers {
        expanded: !popup.modal().is_true(),
        ..TriggerHandlers::default()
    };
    let mut part = popup_trigger_with(this, key.clone(), trigger_props, handlers, cx);
    if let Some(shortcut) = &props.shortcut {
        part = part.aria_keyshortcuts(aria_keyshortcuts(shortcut, dialog.apple()));
    }
    let region = popup
        .trigger_region(&key)
        .expect("a trigger has a region once it renders");
    bind_trigger_shortcut(this, &key, props.shortcut, &region, part)
}

// The trigger's own layer scopes its shortcut: in the window, a modal dialog
// pauses it. It's registered while the trigger renders with one, and again
// whenever it renders with another (`bindPartShortcut`).
fn bind_trigger_shortcut<T: DialogHost>(
    this: &Entity<T>,
    key: &SharedString,
    shortcut: Option<SharedString>,
    region: &Region,
    part: Part<DialogTriggerState>,
) -> Part<DialogTriggerState> {
    let entity = this.downgrade();
    let key = key.clone();
    bind_part_shortcut(
        part,
        shortcut,
        move |event, window, cx| {
            entity
                .update(cx, |this, cx| {
                    let dialog = this.dialog().clone();
                    dialog.shortcut_press(this, &key, Some(InputEvent::KeyDown(event.clone())), window, cx);
                })
                .ok();
        },
        ShortcutOptions {
            layer: Some(ShortcutLayer::Element(region.clone())),
            ..ShortcutOptions::default()
        },
    )
}

/// A backdrop: it dims the window below the dialog, shown and hidden with
/// it, and a press on it is an outside press. Kept in the page with the
/// content, so both show together.
pub fn dialog_backdrop<T: DialogHost>(
    this: &Entity<T>,
    timing: Option<PresenceTiming>,
    cx: &App,
) -> Option<DialogBackdrop<T, DialogBackdropState>> {
    let host = this.read(cx);
    let dialog = host.dialog();
    let popup = host.popup();
    let state = DialogBackdropState {
        open: popup.is_open(),
        nested: dialog.nested(),
        nested_open: dialog.stack() > 0,
        presence: popup.layer().backdrop_presence().state(),
    };
    backdrop_part(this, state, timing, cx)
}

/// A backdrop part with `state`, for a component whose backdrop says more,
/// like a drawer's. `None` while it's out of the page.
pub fn backdrop_part<T: DialogHost, S: 'static>(
    this: &Entity<T>,
    state: S,
    timing: Option<PresenceTiming>,
    cx: &App,
) -> Option<DialogBackdrop<T, S>> {
    popup_backdrop_with(this, state, timing, cx)
}

/// A dialog's backdrop part, which renders above the window below the
/// dialog, or in place with an inline one: the popups crate's backdrop, with
/// the dialog's state. It's styled and filled like any part.
pub type DialogBackdrop<T, S> = PopupBackdrop<T, S>;

/// Keys from a content built on Dialog: Escape closes the topmost layer
/// before its ancestors hear the key, and Tab stays inside a trapping one.
/// Keys from a dialog or drawer nested inside bubble through this one; only
/// the owner handles them.
pub fn content_keydown<T: DialogHost>(entity: &WeakEntity<T>, event: &KeyDownEvent, window: &mut Window, cx: &mut App) {
    let Some(popup) = entity.upgrade().map(|entity| entity.read(cx).popup().clone()) else {
        return;
    };
    if !owns_event_among(&popup.content_region(), OWN_CONTENT, window, cx) {
        return;
    }
    if popup.layer().escape(event, window, cx) {
        return;
    }
    entity
        .update(cx, |this, cx| {
            let popup = this.popup().clone();
            popup.layer().keydown(this, event, window, cx);
        })
        .ok();
}

/// A content: the dialog itself, with its role, named by its title, else
/// its trigger, and described by its description. `None` while it's out of
/// the page, with its props still heard. It's modal to assistive technology
/// only while it blocks the window.
pub fn dialog_content<T: DialogHost>(
    this: &Entity<T>,
    props: DialogContentProps,
    cx: &App,
) -> Option<PopupContent<T, DialogContentState<T::Payload>>> {
    let host = this.read(cx);
    let popup = host.popup().clone();
    let dialog = host.dialog().clone();
    let instance = host.instance();
    let popup_props = props.popup_props();
    let popup_state = popup.state(host);
    let state = dialog.state(host);
    if !state.mounted && !props.keep_mounted {
        popup.set_content_props(popup_props);
        return None;
    }
    let sheet = popup
        .sheet()
        .filter(|_| state.presentation == Presentation::Sheet);
    let content_state = DialogContentState {
        open: state.open,
        modal: state.modal,
        nested: state.nested,
        nested_open: state.stack > 0,
        stack: state.stack,
        payload: state.payload.clone(),
        pending: state.pending.is_some(),
        presentation: state.presentation,
        presence: popup.layer().presence().state(),
        hidden: !state.mounted,
        keyboard: dialog.keyboard().get(),
        sheet: sheet.as_ref().map(|sheet| sheet.part_state()),
        sheet_stack: sheet.as_ref().map_or(0.0, |sheet| sheet.stack().stack()),
    };
    let role = popup
        .role()
        .unwrap_or(if dialog.alert() { Role::AlertDialog } else { Role::Dialog });
    let handle = popup.content_focus_handle();
    let mut aria = Aria::new().hidden(!state.mounted);
    if let Some(name) = popup.trigger_name() {
        aria = aria.merge(name.content_aria(&popup, &popup_state, instance));
    }
    if state.described {
        aria = aria.described_by(instance.element_id("description", None));
    }
    let entity = this.downgrade();
    let mut part = Part::new(instance, popup.content_slot(), None, content_state)
        .aria_role(role)
        .map_el(|el| el.track_focus(&handle.tab_stop(false)))
        .aria(aria)
        .on_keys(&["Tab", "Escape"], move |event, window, cx| {
            content_keydown(&entity, event, window, cx)
        });
    // How much the on-screen keyboard covers, so the dialog can move above
    // it. A sheet follows it itself.
    if state.presentation != Presentation::Sheet {
        part = dialog.keyboard().track(part);
    }
    Some(bind_popup_content(this, part, popup_props, cx))
}

/// A title, which names the dialog while it renders. It's a level 2
/// heading.
pub fn dialog_title<T: DialogHost>(this: &Entity<T>, cx: &App) -> Part<()> {
    popup_title(this, cx)
}

/// A description, which describes the dialog while it renders.
pub fn dialog_description<T: DialogHost>(this: &Entity<T>, cx: &App) -> Part<()> {
    popup_description(this, cx)
}

/// A Close part: a button that closes the dialog, after its action. While
/// its action runs it's busy and reports itself disabled, without the
/// disabled state a native button would take, which would drop focus.
pub fn dialog_close<T: DialogHost>(
    this: &Entity<T>,
    key: impl Into<SharedString>,
    props: DialogCloseProps,
    cx: &App,
) -> Part<DialogCloseState> {
    let key = key.into();
    let host = this.read(cx);
    let dialog = host.dialog().clone();
    let (handle, region) = dialog.close_handle(&key, cx);
    let pending = dialog.pending().as_ref() == Some(&key);
    let state = DialogCloseState {
        pending,
        error: dialog.error(&key),
    };
    let entity = this.downgrade();
    let pressed = key.clone();
    let props = Rc::new(props);
    let part = Part::new(host.instance(), "close", Some(&key), state)
        .aria_role(Role::Button)
        .focus_mode(FocusMode::Native, Some(&handle), false)
        .aria(Aria::new().busy(pending).disabled(pending))
        .on_press(move |event, window, cx| {
            entity
                .update(cx, |this, cx| {
                    let dialog = this.dialog().clone();
                    dialog.close_press(this, pressed.clone(), (*props).clone(), Some(event), window, cx);
                })
                .ok();
        });
    region.measure(part)
}

/// A Shortcut part: the keys of a shortcut, for people to see. Screen
/// readers hear `aria-keyshortcuts` on the trigger instead, so the part is
/// hidden from them. Without keys of its own, it shows the shortcut of the
/// trigger it sits in or beside, and without children it shows them itself.
pub fn dialog_shortcut<T: DialogHost>(
    this: &Entity<T>,
    key: Option<SharedString>,
    props: DialogShortcutProps,
    cx: &App,
) -> ShortcutPart {
    let host = this.read(cx);
    let dialog = host.dialog();
    // A dialog opened only through its api has no trigger to learn the
    // platform from.
    dialog.adopt_platform(cx);
    let instance = host.instance().handle();
    let shortcuts = dialog.trigger_shortcuts();
    shortcut_slot(host.instance(), key.as_deref(), props, None, dialog.apple()).inherit_from(move |el, _cx| {
        shortcut_of(&instance, "trigger", el, true, |trigger| {
            trigger.and_then(|trigger| shortcuts.borrow().get(trigger).cloned())
        })
    })
}
