//! Port of `foundation/kernel/option-names.ts`: the options a state key names,
//! `on_<key>_change`, `on_<key>_commit` and `default_<key>`, for a key like
//! `value`. Keys keep the original's names, like `"highlightedValue"`; the
//! options are their snake case, as Rust names them.

/// What a callback hears: a change or a commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackKind {
    Change,
    Commit,
}

/// The snake case of a key, like `highlighted_value` for `"highlightedValue"`.
pub fn snake(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for c in key.chars() {
        if c.is_uppercase() {
            if !out.is_empty() {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else if c == '-' {
            out.push('_');
        } else {
            out.push(c);
        }
    }
    out
}

/// The callback that hears a key change or commit, like `on_value_change` for `"value"`.
pub fn callback_name(key: &str, kind: CallbackKind) -> String {
    let kind = match kind {
        CallbackKind::Change => "change",
        CallbackKind::Commit => "commit",
    };
    format!("on_{}_{kind}", snake(key))
}

/// The option an uncontrolled key starts from, like `default_value` for `"value"`.
pub fn default_name(key: &str) -> String {
    format!("default_{}", snake(key))
}
