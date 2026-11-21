//! Headless, accessible components for gpui: a Rust port of Vercel's UI SDK
//! (`@ui-sdk/core`). Each component brings its behavior, keyboard support,
//! focus handling and accessibility, and renders as parts you style.
//!
//! This crate re-exports the whole workspace under the original's paths:
//! [`components`] (`ui_sdk::components::select`), the [`foundation`]
//! behaviors they're built from, [`page`] (the agent bridge, `observe`,
//! history) and [`native`] (the text field that stands in for `<input>`).
//! Depend on it, or on the `ui-sdk-<name>` crates one at a time.
//!
//! Call [`init`] once as the app starts, and render
//! [`live_regions`](foundation::platform::live_regions) once in each window
//! so screen readers hear announcements.

pub use ui_sdk_kernel as kernel;
pub use ui_sdk_native as native;
pub use ui_sdk_page as page;

/// The behaviors components are built from (`foundation`).
pub mod foundation {
    pub use ui_sdk_kernel as kernel;
    pub use ui_sdk_canvas as canvas;
    pub use ui_sdk_color as color;
    pub use ui_sdk_date as date;
    pub use ui_sdk_direct_manipulation as direct_manipulation;
    pub use ui_sdk_disclosure as disclosure;
    pub use ui_sdk_drag_and_drop as drag_and_drop;
    pub use ui_sdk_focus as focus;
    pub use ui_sdk_forms as forms;
    pub use ui_sdk_gauge as gauge;
    pub use ui_sdk_image as image;
    pub use ui_sdk_lists as lists;
    pub use ui_sdk_motion as motion;
    pub use ui_sdk_number as number;
    pub use ui_sdk_platform as platform;
    pub use ui_sdk_popups as popups;
    pub use ui_sdk_preferences as preferences;
    pub use ui_sdk_selection as selection;
    pub use ui_sdk_shortcuts as shortcuts;
    pub use ui_sdk_storage as storage;
    pub use ui_sdk_text as text;
}

/// The components (`components`).
pub mod components {
    pub use ui_sdk_accordion as accordion;
    pub use ui_sdk_autocomplete as autocomplete;
    pub use ui_sdk_avatar as avatar;
    pub use ui_sdk_button as button;
    pub use ui_sdk_calendar as calendar;
    pub use ui_sdk_carousel as carousel;
    pub use ui_sdk_checkbox as checkbox;
    pub use ui_sdk_collapsible as collapsible;
    pub use ui_sdk_color_picker as color_picker;
    pub use ui_sdk_combobox as combobox;
    pub use ui_sdk_command as command;
    pub use ui_sdk_date_input as date_input;
    pub use ui_sdk_date_picker as date_picker;
    pub use ui_sdk_dialog as dialog;
    pub use ui_sdk_drawer as drawer;
    pub use ui_sdk_field as field;
    pub use ui_sdk_file_input as file_input;
    pub use ui_sdk_hover_card as hover_card;
    pub use ui_sdk_image_cropper as image_cropper;
    pub use ui_sdk_input as input;
    pub use ui_sdk_label as label;
    pub use ui_sdk_listbox as listbox;
    pub use ui_sdk_menu as menu;
    pub use ui_sdk_menu::menubar;
    pub use ui_sdk_meter as meter;
    pub use ui_sdk_navigation_menu as navigation_menu;
    pub use ui_sdk_number_input as number_input;
    pub use ui_sdk_otp_input as otp_input;
    pub use ui_sdk_pagination as pagination;
    pub use ui_sdk_password_input as password_input;
    pub use ui_sdk_popover as popover;
    pub use ui_sdk_progress as progress;
    pub use ui_sdk_radio as radio;
    pub use ui_sdk_rating as rating;
    pub use ui_sdk_resizable as resizable;
    pub use ui_sdk_scroll_area as scroll_area;
    pub use ui_sdk_segmented_control as segmented_control;
    pub use ui_sdk_select as select;
    pub use ui_sdk_separator as separator;
    pub use ui_sdk_slider as slider;
    pub use ui_sdk_switch as switch;
    pub use ui_sdk_tabs as tabs;
    pub use ui_sdk_timeline as timeline;
    pub use ui_sdk_toast as toast;
    pub use ui_sdk_toggle as toggle;
    pub use ui_sdk_token_input as token_input;
    pub use ui_sdk_toolbar as toolbar;
    pub use ui_sdk_tooltip as tooltip;
    pub use ui_sdk_tree as tree;
    pub use ui_sdk_trigger_menu as trigger_menu;
    pub use ui_sdk_waterfall as waterfall;
}

pub mod prelude {
    pub use ui_sdk_kernel::prelude::*;
}

/// Sets up what a browser gives the original and gpui doesn't: Tab and
/// Shift-Tab move focus between tab stops ([`focus::bind_tab_keys`]), and
/// local storage keeps its values in a directory named `app_name` in the
/// platform's per-user data directory ([`storage::init`]). Call it once as
/// the app starts.
///
/// [`focus::bind_tab_keys`]: ui_sdk_focus::bind_tab_keys
/// [`storage::init`]: ui_sdk_storage::init
pub fn init(app_name: &str, cx: &mut gpui::App) {
    ui_sdk_focus::bind_tab_keys(cx);
    ui_sdk_storage::init(app_name, cx);
}
