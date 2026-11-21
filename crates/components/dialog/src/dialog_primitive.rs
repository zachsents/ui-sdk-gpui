//! Port of `components/dialog/dialog-primitive.ts`: what Dialog and Alert
//! Dialog are made of: an instance, the popup and Dialog's behavior over it,
//! the parts they share, and `agent: { actions: ["setOpen"] }`. What only
//! Dialog has, its presentation, comes in as an extension, so an alert dialog
//! doesn't carry it.

use gpui::{Context, Window};
use ui_sdk_kernel::{Agent, AgentArgs, AgentResult, AgentState, ChangeOptions, Instance, PENDING, json};
use ui_sdk_popups::{Modal, Popup, PopupCloseOptions, PopupOpenOptions, PopupOptions};

use crate::dialog_api::{DialogApi, DialogHost, DialogRules, DialogSettings, dialog_api};

/// What [`define_dialog`] makes, for the component to keep.
pub struct DefinedDialog<T: DialogHost> {
    pub instance: Instance,
    pub popup: Popup<T>,
    pub dialog: DialogApi<T>,
}

/// Makes the instance `name`, like `"dialog"` or `"alert-dialog"`, and its
/// popup with Dialog's behavior and the extension's (`defineDialog`). Call it
/// in the component's constructor.
#[allow(clippy::too_many_arguments)]
pub fn define_dialog<T: DialogHost + Agent>(
    name: &'static str,
    alert: bool,
    options: PopupOptions<T::Payload>,
    settings: DialogSettings,
    debug: bool,
    extension: DialogRules<T>,
    window: &mut Window,
    cx: &mut Context<T>,
) -> DefinedDialog<T> {
    let mut instance = Instance::new(name).with_agent(cx);
    instance.debug = debug;
    let (popup, dialog) = dialog_api(&instance, alert, options, settings, extension, window, cx);
    DefinedDialog {
        instance,
        popup,
        dialog,
    }
}

/// The `modal` in effect for a `modal` option: an alert dialog is always
/// modal (`sync`).
pub fn synced_modal(alert: bool, modal: Modal) -> Modal {
    if alert { Modal::True } else { modal }
}

/// `setOpen`, as the agent calls it.
pub fn set_open_action<T: DialogHost>(
    this: &mut T,
    args: &AgentArgs,
    window: &mut Window,
    cx: &mut Context<T>,
) -> AgentResult {
    let open = args.boolean(0)?;
    let popup = this.popup().clone();
    Ok(popup.set_open(this, open, ChangeOptions::default(), window, cx).into())
}

/// `open`, which goes on until it closes, so the step doesn't wait for it.
pub fn open_action<T: DialogHost>(
    this: &mut T,
    _args: &AgentArgs,
    window: &mut Window,
    cx: &mut Context<T>,
) -> AgentResult {
    let popup = this.popup().clone();
    popup.open(this, PopupOpenOptions::default(), window, cx);
    Ok(PENDING.into())
}

/// `close`.
pub fn close_action<T: DialogHost>(
    this: &mut T,
    _args: &AgentArgs,
    window: &mut Window,
    cx: &mut Context<T>,
) -> AgentResult {
    let popup = this.popup().clone();
    Ok(popup.close(this, PopupCloseOptions::default(), window, cx).into())
}

/// `toggle`.
pub fn toggle_action<T: DialogHost>(
    this: &mut T,
    _args: &AgentArgs,
    window: &mut Window,
    cx: &mut Context<T>,
) -> AgentResult {
    let popup = this.popup().clone();
    Ok(popup.toggle(this, ChangeOptions::default(), window, cx).into())
}

/// The state the agent bridge reads: `open`, the Close part whose action is
/// `pending`, and the `errors` of those that failed, by key.
pub fn dialog_agent_state<T: DialogHost>(this: &T) -> AgentState {
    let dialog = this.dialog();
    let mut errors = json!({});
    if let Some(map) = errors.as_object_mut() {
        for (key, error) in dialog.state(this).errors {
            map.insert(key.to_string(), json!(error.to_string()));
        }
    }
    AgentState::new()
        .with("open", this.popup().is_open())
        .with("pending", dialog.pending().map(|key| key.to_string()))
        .with("errors", errors)
}
