//! Port of `foundation/forms/input-hints.ts`: what a field offers as someone
//! types: a phone's keys and Return key, autofill, and corrections. A text
//! field reads them from its options, as an `<input>` reads the attributes.

/// What a phone labels the Return key, through `enterkeyhint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EnterKeyHint {
    Enter,
    Done,
    Go,
    Next,
    Previous,
    Search,
    Send,
}

/// The kinds of field `input_hints` knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputHintKind {
    Search,
    Email,
    Tel,
    Url,
    OneTimeCode,
    Numeric,
    Decimal,
    Name,
    Username,
    CurrentPassword,
    NewPassword,
    PostalCode,
    Message,
}

/// The keyboard a field asks for (`inputmode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputMode {
    Search,
    Email,
    Tel,
    Url,
    Numeric,
    Decimal,
}

/// Which letters a field capitalizes as someone types (`autocapitalize`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AutoCapitalize {
    None,
    Sentences,
    Words,
    Characters,
}

/// What `input_hints` returns. Unset hints leave the field's own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputHints {
    pub input_mode: Option<InputMode>,
    pub enter_key_hint: Option<EnterKeyHint>,
    /// What autofill offers, like `"email"` or `"one-time-code"`, or `"off"`.
    pub auto_complete: Option<&'static str>,
    pub auto_capitalize: Option<AutoCapitalize>,
    /// Whether the platform corrects words as they're typed.
    pub auto_correct: Option<bool>,
    /// Whether misspelled words are marked.
    pub spell_check: Option<bool>,
}

/// Options for `input_hints`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputHintOptions {
    /// What the Return key says, like `Next` for a field with more after it.
    /// Replaces the kind's own.
    pub enter: Option<EnterKeyHint>,
}

// Typed exactly as it's meant, like an address or a code: no capitals,
// corrections or spelling marks.
fn exact(hints: InputHints) -> InputHints {
    InputHints {
        auto_capitalize: Some(AutoCapitalize::None),
        auto_correct: Some(false),
        spell_check: Some(false),
        ..hints
    }
}

fn kind_hints(kind: InputHintKind) -> InputHints {
    match kind {
        InputHintKind::Search => InputHints {
            input_mode: Some(InputMode::Search),
            enter_key_hint: Some(EnterKeyHint::Search),
            auto_complete: Some("off"),
            auto_capitalize: Some(AutoCapitalize::None),
            spell_check: Some(false),
            ..InputHints::default()
        },
        InputHintKind::Email => exact(InputHints {
            input_mode: Some(InputMode::Email),
            auto_complete: Some("email"),
            ..InputHints::default()
        }),
        InputHintKind::Tel => InputHints {
            input_mode: Some(InputMode::Tel),
            auto_complete: Some("tel"),
            ..InputHints::default()
        },
        InputHintKind::Url => exact(InputHints {
            input_mode: Some(InputMode::Url),
            auto_complete: Some("url"),
            ..InputHints::default()
        }),
        InputHintKind::OneTimeCode => exact(InputHints {
            input_mode: Some(InputMode::Numeric),
            auto_complete: Some("one-time-code"),
            ..InputHints::default()
        }),
        InputHintKind::Numeric => InputHints {
            input_mode: Some(InputMode::Numeric),
            auto_complete: Some("off"),
            ..InputHints::default()
        },
        InputHintKind::Decimal => InputHints {
            input_mode: Some(InputMode::Decimal),
            auto_complete: Some("off"),
            ..InputHints::default()
        },
        InputHintKind::Name => InputHints {
            auto_complete: Some("name"),
            auto_capitalize: Some(AutoCapitalize::Words),
            auto_correct: Some(false),
            spell_check: Some(false),
            ..InputHints::default()
        },
        InputHintKind::Username => exact(InputHints {
            auto_complete: Some("username"),
            ..InputHints::default()
        }),
        InputHintKind::CurrentPassword => exact(InputHints {
            auto_complete: Some("current-password"),
            ..InputHints::default()
        }),
        InputHintKind::NewPassword => exact(InputHints {
            auto_complete: Some("new-password"),
            ..InputHints::default()
        }),
        InputHintKind::PostalCode => InputHints {
            auto_complete: Some("postal-code"),
            auto_capitalize: Some(AutoCapitalize::Characters),
            auto_correct: Some(false),
            spell_check: Some(false),
            ..InputHints::default()
        },
        InputHintKind::Message => InputHints {
            enter_key_hint: Some(EnterKeyHint::Send),
            auto_complete: Some("off"),
            auto_capitalize: Some(AutoCapitalize::Sentences),
            auto_correct: Some(true),
            spell_check: Some(true),
            ..InputHints::default()
        },
    }
}

/// The hints that suit a field of `kind`: a phone's keys and Return key,
/// autofill, and capitals, corrections and spelling. A field's own options
/// set after them win.
pub fn input_hints(kind: InputHintKind, options: InputHintOptions) -> InputHints {
    let mut hints = kind_hints(kind);
    if let Some(enter) = options.enter {
        hints.enter_key_hint = Some(enter);
    }
    hints
}
