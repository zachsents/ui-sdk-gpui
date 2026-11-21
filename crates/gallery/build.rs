//! Turns each component's example into a gallery section: the example's view,
//! and a `make` function that opens it in the gallery's window instead of its own.

use std::{env, fs, path::Path};

/// A section's module, its title, its example, and the expression that makes
/// its view from `window` and `cx`.
const SECTIONS: &[(&str, &str, &str, &str)] = &[
    ("button", "Button", "../components/button/examples/button.rs", PLAIN),
    ("toggle", "Toggle", "../components/toggle/examples/toggle.rs", PLAIN),
    ("toggle_group", "Toggle group", "../components/toggle/examples/toggle_group.rs", PLAIN),
    ("toolbar", "Toolbar", "../components/toolbar/examples/toolbar.rs", PLAIN),
    ("switch", "Switch", "../components/switch/examples/switch.rs", PLAIN),
    ("checkbox", "Checkbox", "../components/checkbox/examples/checkbox.rs", PLAIN),
    ("radio", "Radio", "../components/radio/examples/radio.rs", PLAIN),
    ("rating", "Rating", "../components/rating/examples/rating.rs", PLAIN),
    ("segmented_control", "Segmented control", "../components/segmented-control/examples/segmented_control.rs", PLAIN),
    ("slider", "Slider", "../components/slider/examples/slider.rs", PLAIN),
    ("number_input", "Number input", "../components/number-input/examples/number_input.rs", PLAIN),
    ("color_picker", "Color picker", "../components/color-picker/examples/color_picker.rs", PLAIN),
    ("image_cropper", "Image cropper", "../components/image-cropper/examples/image_cropper.rs", PLAIN),
    ("label", "Label", "../components/label/examples/label.rs", PLAIN),
    ("field", "Field", "../components/field/examples/field.rs", PLAIN),
    ("input", "Input", "../components/input/examples/input.rs", PLAIN),
    ("password_input", "Password input", "../components/password-input/examples/password_input.rs", PLAIN),
    ("otp_input", "OTP input", "../components/otp-input/examples/otp_input.rs", PLAIN),
    ("text_field", "Text field", "../native/examples/text_field.rs", PLAIN),
    ("file_input", "File input", "../components/file-input/examples/file_input.rs", PLAIN),
    ("calendar", "Calendar", "../components/calendar/examples/calendar.rs", PLAIN),
    ("date_input", "Date input", "../components/date-input/examples/date_input.rs", PLAIN),
    ("date_picker", "Date picker", "../components/date-picker/examples/date_picker.rs", PLAIN),
    ("tabs", "Tabs", "../components/tabs/examples/tabs.rs", PLAIN),
    ("navigation_menu", "Navigation menu", "../components/navigation-menu/examples/navigation_menu.rs", PLAIN),
    ("accordion", "Accordion", "../components/accordion/examples/accordion.rs", PLAIN),
    ("collapsible", "Collapsible", "../components/collapsible/examples/collapsible.rs", PLAIN),
    ("resizable", "Resizable", "../components/resizable/examples/resizable.rs", PLAIN),
    ("scroll_area", "Scroll area", "../components/scroll-area/examples/scroll_area.rs", PLAIN),
    ("carousel", "Carousel", "../components/carousel/examples/carousel.rs", PLAIN),
    ("listbox", "Listbox", "../components/listbox/examples/listbox.rs", PLAIN),
    ("select", "Select", "../components/select/examples/select.rs", PLAIN),
    ("menu", "Menu", "../components/menu/examples/menu.rs", PLAIN),
    ("menubar", "Menubar", "../components/menu/examples/menubar.rs", PLAIN),
    ("dialog", "Dialog", "../components/dialog/examples/dialog.rs", PLAIN),
    ("drawer", "Drawer", "../components/drawer/examples/drawer.rs", PLAIN),
    ("popover", "Popover", "../components/popover/examples/popover.rs", PLAIN),
    ("tooltip", "Tooltip", "../components/tooltip/examples/tooltip.rs", PLAIN),
    ("hover_card", "Hover card", "../components/hover-card/examples/hover_card.rs", PLAIN),
    ("toast", "Toast", "../components/toast/examples/toast.rs", PLAIN),
    ("combobox", "Combobox", "../components/combobox/examples/combobox.rs", PLAIN),
    ("autocomplete", "Autocomplete", "../components/autocomplete/examples/autocomplete.rs", PLAIN),
    ("trigger_menu", "Trigger menu", "../components/trigger-menu/examples/trigger_menu.rs", PLAIN),
    ("command", "Command", "../components/command/examples/command.rs", PLAIN),
    ("token_input", "Token input", "../components/token-input/examples/token_input.rs", PLAIN),
    ("avatar", "Avatar", "../components/avatar/examples/avatar.rs", AVATAR),
    ("separator", "Separator", "../components/separator/examples/separator.rs", PLAIN),
    ("meter", "Meter", "../components/meter/examples/meter.rs", PLAIN),
    ("progress", "Progress", "../components/progress/examples/progress.rs", PLAIN),
    ("pagination", "Pagination", "../components/pagination/examples/pagination.rs", PLAIN),
    ("timeline", "Timeline", "../components/timeline/examples/timeline.rs", PLAIN),
    ("tree", "Tree", "../components/tree/examples/tree.rs", PLAIN),
    ("waterfall", "Waterfall", "../components/waterfall/examples/waterfall.rs", PLAIN),
    ("agent", "Agent", "../page/examples/agent.rs", PLAIN),
    ("sink_notifications", "Sink: Notifications", "../sink/examples/notifications.rs", PLAIN),
    ("sink_trip_budget", "Sink: Trip budget", "../sink/examples/trip_budget.rs", PLAIN),
    ("sink_book_stay", "Sink: Book the stay", "../sink/examples/book_stay.rs", PLAIN),
    ("sink_release_history", "Sink: Release history", "../sink/examples/release_history.rs", PLAIN),
];

const PLAIN: &str = "cx.new(|cx| Example::new(window, cx))";

const AVATAR: &str = "{
    let portrait = std::env::temp_dir().join(\"ui-sdk-avatar-portrait.svg\");
    std::fs::write(&portrait, PORTRAIT).ok();
    cx.new(|cx| Example::new(portrait, window, cx))
}";

fn main() {
    let out = env::var("OUT_DIR").unwrap();
    let mut list = String::from("pub const SECTIONS: &[Section] = &[\n");
    let mut modules = String::new();
    for (module, title, path, make) in SECTIONS {
        println!("cargo:rerun-if-changed={path}");
        let source = fs::read_to_string(path).unwrap_or_else(|error| panic!("{path}: {error}"));
        let mut body: String = source
            .lines()
            .filter(|line| !line.starts_with("//!"))
            .map(|line| match line.strip_prefix("fn main()") {
                Some(rest) => format!("fn example_main(){rest}\n"),
                None => format!("{line}\n"),
            })
            .collect();
        body.push_str(&format!(
            "pub fn make(window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::AnyView {{\n    use gpui::AppContext as _;\n    gpui::AnyView::from({make})\n}}\n"
        ));
        fs::write(Path::new(&out).join(format!("{module}.rs")), body).unwrap();
        modules.push_str(&format!(
            "pub mod {module} {{\n    #![allow(dead_code, unused_imports)]\n    include!(concat!(env!(\"OUT_DIR\"), \"/{module}.rs\"));\n}}\n"
        ));
        list.push_str(&format!("    Section {{ title: \"{title}\", make: {module}::make }},\n"));
    }
    list.push_str("];\n");
    fs::write(Path::new(&out).join("sections.rs"), modules + &list).unwrap();
}
