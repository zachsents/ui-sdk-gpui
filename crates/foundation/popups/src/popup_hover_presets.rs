//! Port of `foundation/popups/popup-hover-presets.ts`: the presets of popups
//! that resting the pointer and keyboard focus open, for
//! [`create_popup`](fn@crate::create_popup).

use std::time::Duration;

use ui_sdk_kernel::roles::Role;
use ui_sdk_platform::Side;

use crate::popup_behavior::PopupBehaviorName;
use crate::popup_presets::PopupPreset;

/// A tooltip: it describes its trigger, opens when the pointer rests on it
/// and at once on keyboard focus, never takes focus, and only one is open at
/// a time, the next opening at once while one is warm (`tooltipPreset`).
pub const fn tooltip_preset() -> PopupPreset {
    PopupPreset {
        payload: true,
        role: Some(Role::Tooltip),
        side: Some(Side::Top),
        delay: Some(Duration::from_millis(600)),
        close_delay: Some(Duration::ZERO),
        focus_delay: Some(Duration::ZERO),
        placement: true,
        hover: true,
        focus: true,
        group: true,
        ..PopupPreset::new(PopupBehaviorName::Tooltip)
    }
}

/// A preview of what a link points to: it opens when the pointer or
/// keyboard focus rests on the link, Tab moves from the link into it, and
/// focus leaving closes it (`hoverCardPreset`).
pub const fn hover_card_preset() -> PopupPreset {
    PopupPreset {
        payload: true,
        delay: Some(Duration::from_millis(600)),
        close_delay: Some(Duration::from_millis(300)),
        placement: true,
        hover: true,
        focus: true,
        ..PopupPreset::new(PopupBehaviorName::HoverCard)
    }
}
