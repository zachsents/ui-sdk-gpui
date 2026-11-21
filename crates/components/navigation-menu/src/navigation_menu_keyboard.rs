//! Port of `components/navigation-menu/navigation-menu-keyboard.ts`: the
//! optional arrow keys of the APG disclosure navigation pattern, as pure
//! decisions. They add to Tab and never replace it, and nothing wraps.
//! <https://www.w3.org/WAI/ARIA/apg/patterns/disclosure/examples/disclosure-navigation/>

use gpui::{ClickEvent, Modifiers, MouseButton, Orientation};
use ui_sdk_kernel::{InputEvent, Key};

/// How content entered or left, in item order: `start` is the side of the
/// first item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NavigationMenuMotion {
    FromStart,
    FromEnd,
    ToStart,
    ToEnd,
}

impl NavigationMenuMotion {
    /// The original's name for it, its `data-motion`, like `"from-end"`.
    pub fn as_str(self) -> &'static str {
        match self {
            NavigationMenuMotion::FromStart => "from-start",
            NavigationMenuMotion::FromEnd => "from-end",
            NavigationMenuMotion::ToStart => "to-start",
            NavigationMenuMotion::ToEnd => "to-end",
        }
    }
}

/// What a key does in a list or a panel: move by one, go to an end, or, on
/// a top-level trigger, open its panel and move into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyStep {
    By(isize),
    First,
    Last,
    Enter,
}

/// What a key does on a top-level trigger or link: move along the list, or
/// open the trigger's panel and move into it. `None` for other keys
/// (`listKey`).
pub fn list_key(key: &Key, orientation: Orientation, rtl: bool) -> Option<KeyStep> {
    let inline_end = if rtl { Key::ArrowLeft } else { Key::ArrowRight };
    let inline_start = if rtl { Key::ArrowRight } else { Key::ArrowLeft };
    let (next, previous, enter) = match orientation {
        Orientation::Vertical => (Key::ArrowDown, Key::ArrowUp, inline_end),
        Orientation::Horizontal => (inline_end, inline_start, Key::ArrowDown),
    };
    if *key == next {
        return Some(KeyStep::By(1));
    }
    if *key == previous {
        return Some(KeyStep::By(-1));
    }
    match key {
        Key::Home => Some(KeyStep::First),
        Key::End => Some(KeyStep::Last),
        _ if *key == enter => Some(KeyStep::Enter),
        _ => None,
    }
}

/// What a key does on a link inside an open panel: Down and the inline end
/// go to the next one, Up and the inline start to the previous one
/// (`panelKey`).
pub fn panel_key(key: &Key, rtl: bool) -> Option<KeyStep> {
    let (end, start) = if rtl {
        (Key::ArrowLeft, Key::ArrowRight)
    } else {
        (Key::ArrowRight, Key::ArrowLeft)
    };
    if *key == Key::ArrowDown || *key == end {
        return Some(KeyStep::By(1));
    }
    if *key == Key::ArrowUp || *key == start {
        return Some(KeyStep::By(-1));
    }
    match key {
        Key::Home => Some(KeyStep::First),
        Key::End => Some(KeyStep::Last),
        _ => None,
    }
}

/// Keys pressed with a modifier belong to the system, like Alt+Left for
/// Back, so the menu leaves them alone (`isModified`).
pub fn is_modified(modifiers: &Modifiers) -> bool {
    modifiers.alt || modifiers.control || modifiers.platform || modifiers.shift
}

/// The index a step lands on, stopping at the ends (`stepIndex`).
pub fn step_index(index: usize, step: KeyStep, count: usize) -> usize {
    let last = count.saturating_sub(1);
    match step {
        KeyStep::First => 0,
        KeyStep::Last => last,
        KeyStep::By(by) => (index as isize + by).clamp(0, last as isize) as usize,
        KeyStep::Enter => index.min(last),
    }
}

/// Which way content moves when the open item changes, from the items'
/// order. Order is logical, so `end` is on the left in right-to-left layouts
/// (`motionOf`).
pub fn motion_of(
    key: &str,
    value: Option<&str>,
    previous: Option<&str>,
    forward: Option<bool>,
) -> Option<NavigationMenuMotion> {
    let forward = forward?;
    if Some(key) == value {
        return Some(if forward {
            NavigationMenuMotion::FromEnd
        } else {
            NavigationMenuMotion::FromStart
        });
    }
    if Some(key) == previous {
        return Some(if forward {
            NavigationMenuMotion::ToStart
        } else {
            NavigationMenuMotion::ToEnd
        });
    }
    None
}

/// Whether a link press should leave the menu alone: a modifier or the
/// middle button opens it somewhere else, so the menu stays as it was
/// (`opensElsewhere`).
pub fn opens_elsewhere(event: &InputEvent) -> bool {
    // A key's click carries the key's modifiers, as a browser's does.
    let (modifiers, button) = match event {
        InputEvent::Click(ClickEvent::Mouse(click)) => (click.up.modifiers, click.down.button),
        InputEvent::Click(click) => (click.modifiers(), MouseButton::Left),
        InputEvent::MouseDown(down) => (down.modifiers, down.button),
        InputEvent::MouseUp(up) => (up.modifiers, up.button),
        _ => return false,
    };
    modifiers.platform || modifiers.control || modifiers.shift || modifiers.alt || button == MouseButton::Middle
}
