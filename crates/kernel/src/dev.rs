//! Port of `foundation/kernel/dev.ts`: development-only warnings. Everything
//! here runs only in debug builds (`cfg!(debug_assertions)`), so release
//! builds pay nothing; `dev-stub.ts`, the original's production stand-in,
//! has no counterpart.
//!
//! Warnings go to `log::warn!` with the component and instance they're about,
//! like `[ui-sdk] toggle#ui-sdk-3: …`. Each warns once, by what it's about.

use std::cell::RefCell;
use std::collections::HashSet;

use accesskit::{Action, Node};

use crate::details::{InputEvent, Reason};
use crate::roles::{self, Met, Need, Role, RoleAria, Written};

/// Whether this is a development build.
pub const DEV: bool = cfg!(debug_assertions);

/// The component and instance a warning is about, like `Popover#ui-sdk-3`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Origin<'a> {
    pub component: &'a str,
    pub id: Option<&'a str>,
}

impl<'a> Origin<'a> {
    pub fn new(component: &'a str, id: &'a str) -> Self {
        Self {
            component,
            id: Some(id),
        }
    }

    fn prefix(&self) -> String {
        if self.component.is_empty() {
            return String::new();
        }
        match self.id {
            Some(id) => format!("{}#{}: ", self.component, id),
            None => format!("{}: ", self.component),
        }
    }
}

/// Prints a development warning, naming the component and instance when it
/// knows them.
pub fn warn(message: &str, from: Option<Origin>) {
    if DEV {
        let name = from.map(|from| from.prefix()).unwrap_or_default();
        log::warn!("[ui-sdk] {name}{message}");
    }
}

/// Like `warn`, for what the app meant to happen, like an action that failed.
/// The original never throws these in strict mode; here both log the same.
pub fn note(message: &str, from: Option<Origin>, detail: Option<&dyn std::fmt::Debug>) {
    if DEV {
        let name = from.map(|from| from.prefix()).unwrap_or_default();
        match detail {
            Some(detail) => log::warn!("[ui-sdk] {name}{message} {detail:?}"),
            None => log::warn!("[ui-sdk] {name}{message}"),
        }
    }
}

thread_local! {
    static WARNED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// `warn`, once per `key`.
pub fn warn_once(key: impl Into<String>, message: &str, from: Option<Origin>) {
    if !DEV {
        return;
    }
    let fresh = WARNED.with(|warned| warned.borrow_mut().insert(key.into()));
    if fresh {
        warn(message, from);
    }
}

fn need_message(need: Need) -> &'static str {
    match need {
        Need::Focus => {
            "takes focus, so its part needs a focus mode, `Part::focus_mode`, or `Part::unneeded(Need::Focus, \"why\")` when another element takes it"
        }
        Need::Press => "activates, so its part needs a press, `Part::on_press`",
        Need::Keys => {
            "moves with keys, so its part needs `Part::on_keys` with the keys it handles, or `Part::unneeded(Need::Keys, \"why\")` when another element handles them"
        }
        Need::KeyDown => "lists keys, so its part needs a key handler that handles them",
    }
}

/// What a part says about its role's needs, beside its node.
#[derive(Clone, Copy, Debug, Default)]
pub struct Declared {
    pub focus: bool,
    pub press: bool,
    pub keys: bool,
    pub focus_unneeded: bool,
    pub press_unneeded: bool,
    pub keys_unneeded: bool,
}

/// The role-specific attributes a part's node carries, from gpui's setters
/// and the kernel's `Aria`.
pub fn written_of(role: Role, node: &Node, aria: &[RoleAria], named: bool) -> Written {
    let mut attributes: Vec<RoleAria> = aria.to_vec();
    let mut add = |attribute: RoleAria, present: bool| {
        if present && !attributes.contains(&attribute) {
            attributes.push(attribute);
        }
    };
    let toggled = node.toggled().is_some();
    add(RoleAria::Pressed, toggled && role == Role::Button);
    add(RoleAria::Checked, toggled && role != Role::Button);
    add(RoleAria::Expanded, node.is_expanded().is_some());
    add(RoleAria::Selected, node.is_selected().is_some());
    add(RoleAria::Level, node.level().is_some());
    add(RoleAria::Orientation, node.orientation().is_some());
    add(RoleAria::PosInSet, node.position_in_set().is_some());
    add(RoleAria::SetSize, node.size_of_set().is_some());
    add(RoleAria::RowIndex, node.row_index().is_some());
    add(RoleAria::ColIndex, node.column_index().is_some());
    add(RoleAria::RowCount, node.row_count().is_some());
    add(RoleAria::ColCount, node.column_count().is_some());
    add(RoleAria::ValueNow, node.numeric_value().is_some());
    add(RoleAria::ValueMin, node.min_numeric_value().is_some());
    add(RoleAria::ValueMax, node.max_numeric_value().is_some());
    Written {
        attributes,
        named: named || node.label().is_some(),
    }
}

/// Checks the role a part renders with against what it has and declares:
/// the needs the role has that it doesn't meet, the ARIA the role doesn't
/// take, and the states the role requires that it writes without
/// (`checkRoles`, `checkRole`, `checkAria`, `checkFocus`). Once per
/// component, slot and problem.
pub fn check_role(slot: &str, role: Role, node: &Node, declared: Declared, written: &Written, from: Origin) {
    if !DEV {
        return;
    }
    let a = role.article();
    let name = role.as_str();
    let met = Met {
        focus: declared.focus || declared.focus_unneeded || node.supports_action(Action::Focus),
        press: declared.press || declared.press_unneeded || node.supports_action(Action::Click),
        keys: declared.keys || declared.keys_unneeded,
        keydown: declared.keys || declared.keys_unneeded,
    };
    for need in roles::unmet_needs(role, met) {
        warn_once(
            format!("{}:{slot}:{name}:{}", from.component, need.as_str()),
            &format!("the \"{slot}\" part is {a} {name}, which {}.", need_message(need)),
            Some(from),
        );
    }
    let problems = roles::aria_problems(role, written);
    for attribute in &problems.not_allowed {
        warn_once(
            format!("{}:{slot}:{name}:{attribute}", from.component),
            &format!(
                "the \"{slot}\" part is {a} {name}, which doesn't take `{attribute}`, so assistive technology ignores it."
            ),
            Some(from),
        );
    }
    for state in &problems.missing {
        let attribute = state.name();
        warn_once(
            format!("{}:{slot}:{name}:{attribute}", from.component),
            &format!("the \"{slot}\" part is {a} {name}, which needs `{attribute}`, and the part doesn't write it."),
            Some(from),
        );
    }
}

/// Checks that a part that says how it takes focus can take it: a focus
/// handle tracked by its element (`checkFocus`). A disabled part may leave
/// the tab order on purpose.
pub fn check_focus(slot: &str, declared_focus: bool, active_descendant: bool, node: &Node, from: Origin) {
    if !DEV || !declared_focus || active_descendant || node.is_disabled() {
        return;
    }
    if !node.supports_action(Action::Focus) {
        warn_once(
            format!("{}:{slot}:focus-handle", from.component),
            &format!("the \"{slot}\" part says how it takes focus, but has no focus handle, so it can't take focus."),
            Some(from),
        );
    }
}

/// The kinds of event a reason may carry, mirroring the event types the
/// vocabulary lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Mouse,
    Keyboard,
    Touch,
    Pointer,
    Focus,
    Wheel,
    Drag,
}

// `None` takes any event: the vocabulary's plain `Event`, and the event types
// gpui has none of, like a clipboard or storage event.
fn allowed(reason: Reason) -> Option<&'static [Class]> {
    use Class as C;
    use Reason as R;
    Some(match reason {
        R::TriggerPress => &[C::Mouse, C::Keyboard, C::Touch],
        R::TriggerHover | R::ItemHover | R::TriggerRelease | R::TrackPress | R::Drag | R::Scrub => {
            &[C::Pointer]
        }
        R::TriggerFocus | R::FocusIn => &[C::Focus],
        R::ItemPress
        | R::ClosePress
        | R::Press
        | R::LinkPress
        | R::HandlePress
        | R::Action
        | R::ClearPress
        | R::RemovePress
        | R::TogglePress
        | R::PreviousPress
        | R::NextPress
        | R::FirstPress
        | R::LastPress
        | R::Collapse => &[C::Mouse, C::Keyboard],
        R::ListNavigation
        | R::EscapeKey
        | R::HandleKey
        | R::Shortcut
        | R::HistoryKey
        | R::StepKey
        | R::MoveKey
        | R::Undo
        | R::Redo => &[C::Keyboard],
        R::OutsidePress => &[C::Mouse],
        R::FocusOut | R::InputCommit => &[C::Focus, C::Keyboard],
        R::SubmenuSwitch => &[C::Pointer, C::Keyboard],
        R::Swipe => &[C::Pointer, C::Touch],
        R::IncrementPress | R::DecrementPress => &[C::Pointer, C::Mouse],
        R::Wheel => &[C::Wheel],
        R::Drop => &[C::Drag],
        R::ParentClose
        | R::ItemRemoved
        | R::Reset
        | R::Restore
        | R::ContentResize
        | R::Imperative
        | R::Timeout
        | R::Unmount
        | R::Resize
        | R::SrcChange => &[],
        R::WindowBlur
        | R::FindInPage
        | R::StorageSync
        | R::Scroll
        | R::Back
        | R::InputChange
        | R::AccessibilityAdjust
        | R::Paste
        | R::Picker
        | R::CaretMove
        | R::Cut
        | R::ImageLoad
        | R::ImageError
        | R::FormSubmit
        | R::PageHide => return None,
        // Reasons components declare beyond the shared vocabulary, which
        // types no event for them.
        R::AspectRatioChange
        | R::ConnectKey
        | R::DoubleClick
        | R::EditKey
        | R::EnterKey
        | R::EyeDropperPick
        | R::FlipPress
        | R::Lasso
        | R::Pinch
        | R::PresetPress
        | R::ResizeKey
        | R::RotateKey
        | R::RotatePress
        | R::SelectChange
        | R::TabKey
        | R::Type
        | R::ZoomInput
        | R::ZoomKey => return None,
    })
}

fn classes(event: &InputEvent) -> &'static [Class] {
    use gpui::ClickEvent;
    use Class as C;
    match event {
        // A click from Enter or Space is a `click`, a mouse event, in the DOM.
        InputEvent::Click(ClickEvent::Keyboard(_)) => &[C::Mouse, C::Keyboard],
        InputEvent::Click(ClickEvent::Mouse(_)) => &[C::Mouse, C::Pointer],
        InputEvent::Click(ClickEvent::Touch(_)) => &[C::Mouse, C::Pointer, C::Touch],
        InputEvent::KeyDown(_) | InputEvent::KeyUp(_) => &[C::Keyboard],
        InputEvent::MouseDown(_) | InputEvent::MouseUp(_) | InputEvent::MouseMove(_) => {
            &[C::Mouse, C::Pointer]
        }
        InputEvent::Scroll(_) => &[C::Wheel, C::Mouse],
        InputEvent::Focus(_) => &[C::Focus],
        // A virtual click is a `click` too.
        InputEvent::Accessibility => &[C::Mouse, C::Pointer],
        InputEvent::Touch(_) | InputEvent::TouchDrag(_) | InputEvent::BackGesture(_) => {
            &[C::Touch, C::Pointer]
        }
        InputEvent::FileDrop(_) => &[C::Drag, C::Mouse],
        // A trackpad pinch is a `wheel` event with Control held in the DOM.
        InputEvent::Pinch(_) => &[C::Wheel, C::Mouse],
        InputEvent::LongPress(_) => &[C::Touch, C::Pointer],
        // A finger's `pointerdown` and `pointerup`: pointer events, so mouse
        // events too, but not touch events.
        InputEvent::TapDown(_) | InputEvent::TapUp(_) => &[C::Mouse, C::Pointer],
    }
}

/// Warns once when a change's event doesn't match what its reason declares
/// (`checkEvent`).
pub fn check_event(reason: Reason, event: Option<&InputEvent>, from: Origin) {
    if !DEV {
        return;
    }
    let (Some(event), Some(allowed)) = (event, allowed(reason)) else {
        return;
    };
    if classes(event).iter().any(|class| allowed.contains(class)) {
        return;
    }
    let kind = event.kind();
    warn_once(
        format!("event:{}:{kind}", reason.as_str()),
        &format!("reported `{}` with a {kind} event, which the vocabulary doesn't list.", reason.as_str()),
        Some(from),
    );
}
