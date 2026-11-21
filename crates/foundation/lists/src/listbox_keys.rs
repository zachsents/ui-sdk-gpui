//! Port of `foundation/lists/listbox-keys.ts`: keys and element ids of a
//! list's built-in options, like a select-all option, apart from the options
//! themselves.

use gpui::SharedString;
use ui_sdk_kernel::attrs::id_part;

/// Select-all keys in the highlight. Never a real item's.
pub const SELECT_ALL: &str = "\u{0}select-all";

/// The select-all key for a group, or for the whole list.
pub fn select_all_key(group: Option<&str>) -> SharedString {
    match group {
        None => SharedString::new_static(SELECT_ALL),
        Some(group) => format!("{SELECT_ALL}:{group}").into(),
    }
}

/// Whether a key is a select-all option's.
pub fn is_select_all(key: &str) -> bool {
    key.starts_with(SELECT_ALL)
}

/// The id of an option's element: an item's from its key, or a built-in
/// option's, like a create or select-all option, from its name. `id` is the
/// component's instance id. Relations take it as an element id, like
/// `Aria::active_descendant(Some(option_id(..)))`, and an option part takes
/// it with `Part::with_element_id`.
pub fn option_id(id: &str, key: &str) -> SharedString {
    match key.strip_prefix('\u{0}') {
        None => format!("{id}-option-{}", id_part(key)).into(),
        Some(name) => {
            let name: String = name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' })
                .collect();
            format!("{id}-{name}").into()
        }
    }
}
