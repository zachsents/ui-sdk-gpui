//! Port of `foundation/platform/button.ts`: what Button and Toggle share, a
//! part that acts like a button whatever it's rendered as. A native button
//! keeps its own behavior, a link stays a link, and anything else becomes a
//! button.
//!
//! gpui renders every part as a div, so the component says what a part acts
//! as rather than reading its tag: [`button_element`] makes it a link when it
//! has somewhere to go. gpui turns Enter and Space on a focused element into
//! a keyboard click, which `ui_sdk_kernel::part::on_press` hears, so parts
//! need no key handlers of their own to activate.

use gpui::{ClickEvent, MouseButton, SharedString};
use ui_sdk_kernel::InputEvent;
use ui_sdk_kernel::roles::Role;

/// What a button-like part acts as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ButtonElement {
    /// A native button, which disables natively instead of with
    /// `aria-disabled`.
    Button,
    /// A link: a part with somewhere to go.
    Link,
    /// Anything else, which becomes a button.
    #[default]
    Other,
}

/// What a part acts as: a link when it has somewhere to go, else a button
/// made of a div. The original reads `<button>` and `<a href>` off the element.
pub fn button_element(has_href: bool) -> ButtonElement {
    if has_href {
        ButtonElement::Link
    } else {
        ButtonElement::Other
    }
}

/// The role a button-like part's slot declares, for `Part::aria_role`. A
/// native button or a link keeps its own, which gpui, with no native
/// elements, has to state.
pub fn button_role(element: ButtonElement) -> Role {
    match element {
        ButtonElement::Link => Role::Link,
        ButtonElement::Button | ButtonElement::Other => Role::Button,
    }
}

/// What [`button_element_attrs`] decides from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ButtonElementOptions {
    /// A native button's `type`, like `"submit"`.
    pub button_type: Option<SharedString>,
    pub disabled: bool,
    /// Keeps a disabled part focusable.
    pub focusable: bool,
    /// A tab index the part was given, which wins.
    pub tab_index: Option<isize>,
}

/// How a button-like part disables and takes focus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ButtonElementAttrs {
    /// A native button's `type`.
    pub button_type: Option<SharedString>,
    /// Natively disabled: neither focusable nor pressable.
    pub disabled: bool,
    /// Reported disabled to assistive technology while it stays focusable,
    /// with `Aria::disabled`.
    pub aria_disabled: bool,
    /// Its place in the tab order: `-1` is focusable but not a tab stop
    /// (`FocusHandle::tab_stop(false)`), `0` or more a tab stop, and `None`
    /// leaves the part's own.
    pub tab_index: Option<isize>,
}

/// A native button is natively disabled, or keeps focus with aria-disabled
/// when `focusable`. Links and other elements always use aria-disabled, and
/// leave the tab order when disabled unless focusable.
pub fn button_element_attrs(
    element: ButtonElement,
    options: ButtonElementOptions,
) -> ButtonElementAttrs {
    let native = element == ButtonElement::Button;
    let unfocusable = options.disabled && !options.focusable;
    let leaves_tab_order = unfocusable && !native;
    let default_tab_index = if leaves_tab_order {
        Some(-1)
    } else if element == ButtonElement::Other {
        Some(0)
    } else {
        None
    };
    ButtonElementAttrs {
        button_type: if native { options.button_type } else { None },
        disabled: native && unfocusable,
        aria_disabled: options.disabled && !(native && unfocusable),
        tab_index: options.tab_index.or(default_tab_index),
    }
}

/// A link part's own attributes that send a press elsewhere.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkTarget {
    /// Where it opens, like `"_blank"`. `"_self"` or none is in place.
    pub target: Option<SharedString>,
    /// Whether it downloads what it links to.
    pub download: bool,
}

/// A press a link leaves to the system: a new tab or window, or a download.
/// `link` is the pressed part's, when it's a link.
pub fn opens_elsewhere(event: Option<&InputEvent>, link: Option<&LinkTarget>) -> bool {
    let (Some(event), Some(link)) = (event, link) else {
        return false;
    };
    let (modifiers, button) = match event {
        InputEvent::Click(click) => (click.modifiers(), click_button(click)),
        InputEvent::MouseDown(down) => (down.modifiers, down.button),
        _ => return false,
    };
    modifiers.platform
        || modifiers.control
        || modifiers.shift
        || modifiers.alt
        || button != MouseButton::Left
        || link.download
        || link
            .target
            .as_ref()
            .is_some_and(|target| !target.is_empty() && target.as_ref() != "_self")
}

// The button a click reports, as the DOM's `button`: a key's click and a tap
// are the primary button's, and a long press is touch's secondary one.
fn click_button(click: &ClickEvent) -> MouseButton {
    match click {
        ClickEvent::Mouse(event) => event.down.button,
        ClickEvent::Keyboard(_) => MouseButton::Left,
        ClickEvent::Touch(event) if event.long_press => MouseButton::Right,
        ClickEvent::Touch(_) => MouseButton::Left,
    }
}
