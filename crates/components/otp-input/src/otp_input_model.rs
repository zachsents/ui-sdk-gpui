//! Port of `components/otp-input/otp-input-model.ts`: the pure side of OTP
//! Input: which characters a code accepts, what a paste becomes, where the
//! selection goes so the slot under the caret is selected, and the slots
//! rendered from the value and the selection. Positions count characters,
//! one a slot, which the field's byte offsets convert to.

use std::cell::RefCell;
use std::collections::HashMap;

use icu_normalizer::ComposingNormalizerBorrowed;
use regex::Regex;
use ui_sdk_native::SelectionDirection;

/// Which characters a code accepts: `Digits`, `Letters`, `Alphanumeric`, or
/// a regular expression each character must match, like `[A-F0-9]`, with
/// its flags inline, like `(?i)[a-f0-9]`.
#[derive(Clone, Debug, Default)]
pub enum OtpInputPattern {
    #[default]
    Digits,
    Letters,
    Alphanumeric,
    Regex(Regex),
}

impl PartialEq for OtpInputPattern {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Regex(a), Self::Regex(b)) => a.as_str() == b.as_str(),
            (Self::Digits, Self::Digits)
            | (Self::Letters, Self::Letters)
            | (Self::Alphanumeric, Self::Alphanumeric) => true,
            _ => false,
        }
    }
}

impl From<Regex> for OtpInputPattern {
    fn from(regex: Regex) -> Self {
        Self::Regex(regex)
    }
}

/// One slot of the code, rendered from the value and the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OtpInputSlotData {
    /// The character in the slot, or `None` when it's empty.
    pub char: Option<char>,
    /// Whether the slot is where typing goes: the selected character, or the
    /// empty slot after the last one.
    pub active: bool,
    /// Whether the slot should draw a caret: it's active and empty. The real
    /// caret is hidden.
    pub has_fake_caret: bool,
}

/// A selection of the code, in characters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub start: usize,
    pub end: usize,
}

impl Selection {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

thread_local! {
    static ANCHORED: RefCell<HashMap<String, Option<Regex>>> = RefCell::new(HashMap::new());
}

// A character-sized copy of a custom pattern: anchored, so it matches a
// whole character.
fn character_pattern(regex: &Regex) -> Option<Regex> {
    ANCHORED.with(|anchored| {
        anchored
            .borrow_mut()
            .entry(regex.as_str().to_string())
            .or_insert_with(|| Regex::new(&format!("^(?:{})$", regex.as_str())).ok())
            .clone()
    })
}

fn fits_char(c: char, pattern: &OtpInputPattern) -> bool {
    match pattern {
        OtpInputPattern::Digits => c.is_ascii_digit(),
        OtpInputPattern::Letters => c.is_ascii_alphabetic(),
        OtpInputPattern::Alphanumeric => c.is_ascii_alphanumeric(),
        OtpInputPattern::Regex(regex) => {
            character_pattern(regex).is_some_and(|anchored| anchored.is_match(c.encode_utf8(&mut [0; 4])))
        }
    }
}

/// Whether every character of `text` fits the pattern.
pub fn fits_pattern(text: &str, pattern: &OtpInputPattern) -> bool {
    text.chars().all(|c| fits_char(c, pattern))
}

/// Whether a whole value is a code the input can hold: no longer than
/// `max_length`, every character allowed.
pub fn is_valid_code(value: &str, pattern: &OtpInputPattern, max_length: usize) -> bool {
    value.chars().count() <= max_length && fits_pattern(value, pattern)
}

/// The native `pattern` the input's constraint validation checks, so it
/// agrees with the presets and blocks a partial code. `minlength` alone only
/// flags codes the user typed, not a partial default or a call's write, so
/// the pattern asks for the length too, except while the code is already
/// flagged as too short: a mismatched format is reported before a short
/// one, and the user would read "match the requested format" instead of how
/// long the code is. A custom expression only sets the length: its
/// characters were checked as they went in.
pub fn pattern_attribute(pattern: &OtpInputPattern, length: usize, too_short: bool) -> Option<String> {
    let count = if too_short {
        "*".to_string()
    } else {
        format!("{{{length}}}")
    };
    match pattern {
        OtpInputPattern::Digits => Some(format!("[0-9]{count}")),
        OtpInputPattern::Letters => Some(format!("[A-Za-z]{count}")),
        OtpInputPattern::Alphanumeric => Some(format!("[A-Za-z0-9]{count}")),
        OtpInputPattern::Regex(_) => (!too_short).then(|| format!(".{{{length}}}")),
    }
}

/// What inserted text becomes. Full-width characters from an input method,
/// like "１２３", become their plain forms. Codes are often written
/// "123 456" or "123-456", so spaces and dashes go. `None` when anything
/// else doesn't fit the pattern, like "Your code is 123456", which is
/// ignored rather than half pasted.
pub fn pasted_code(text: &str, pattern: &OtpInputPattern) -> Option<String> {
    let code: String = ComposingNormalizerBorrowed::new_nfkc()
        .normalize(text)
        .chars()
        .filter(|c| !is_separator(*c))
        .collect();
    (!code.is_empty() && fits_pattern(&code, pattern)).then_some(code)
}

// Spaces, a no-break space and dashes, which group a code as people write it.
fn is_separator(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0B}'
            | '\u{0C}'
            | '\r'
            | ' '
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
                | '-'
                | '\u{2010}'..='\u{2015}'
    )
}

/// The value after inserting text over a selection, cut to `max_length`. A
/// whole code replaces everything, wherever the caret was.
pub fn insert_code(value: &str, selection: Selection, text: &str, max_length: usize) -> String {
    if text.chars().count() >= max_length {
        return text.chars().take(max_length).collect();
    }
    let chars: Vec<char> = value.chars().collect();
    let start = selection.start.min(chars.len());
    let end = selection.end.min(chars.len()).max(start);
    chars[..start]
        .iter()
        .copied()
        .chain(text.chars())
        .chain(chars[end..].iter().copied())
        .take(max_length)
        .collect()
}

/// Where the selection goes after the caret moves, so the slot under the
/// caret is selected and typing replaces it, and which way it points. A
/// caret after the last character, in a code that isn't full, stays: the
/// next character goes there. `previous` is the selection before the move,
/// which says whether an arrow key went back. `None` when the selection
/// should stay as it is.
pub fn slot_selection(
    selection: Selection,
    previous: Option<Selection>,
    length: usize,
    max_length: usize,
) -> Option<(Selection, SelectionDirection)> {
    let caret = selection.start;
    if selection.start != selection.end || length == 0 {
        return None;
    }
    if caret == length && length < max_length {
        return None;
    }
    if caret >= max_length {
        return Some((Selection::new(max_length - 1, max_length), SelectionDirection::Backward));
    }
    if caret == 0 {
        return Some((Selection::new(0, 1), SelectionDirection::Forward));
    }
    let backward = previous.is_some_and(|previous| caret < previous.end);
    // From a selected slot, ArrowLeft collapses to that slot's start, which
    // is one slot back. From the empty slot at the end, it's the last
    // character.
    let was_inserting =
        previous.is_some_and(|previous| previous.start == previous.end && previous.start < max_length);
    let index = if backward && !was_inserting { caret - 1 } else { caret };
    let direction = if backward {
        SelectionDirection::Backward
    } else {
        SelectionDirection::Forward
    };
    Some((Selection::new(index, index + 1), direction))
}

/// Where the selection starts on focus: the empty slot after the code, or
/// its last character when it's full.
pub fn focus_selection(length: usize, max_length: usize) -> Selection {
    if length >= max_length {
        Selection::new(max_length.saturating_sub(1), max_length)
    } else {
        Selection::new(length, length)
    }
}

/// The selection a press on slot `index` makes: that character, or the
/// empty slot after the code.
pub fn press_selection(index: usize, length: usize, max_length: usize) -> Selection {
    if index >= length {
        return focus_selection(length, max_length);
    }
    Selection::new(index, index + 1)
}

/// Where an arrow key, Home or End moves the selection, a slot at a time:
/// onto the character before or after, or the empty slot after the code,
/// by the keys' names, like `"ArrowLeft"`. `None` for any other key. Right
/// to left, the arrows swap.
pub fn key_selection(
    key: &str,
    selection: Selection,
    length: usize,
    max_length: usize,
    rtl: bool,
) -> Option<Selection> {
    let forward = if rtl { "ArrowLeft" } else { "ArrowRight" };
    let backward = if rtl { "ArrowRight" } else { "ArrowLeft" };
    let end = focus_selection(length, max_length);
    if key == "Home" {
        return Some(if length > 0 { Selection::new(0, 1) } else { end });
    }
    if key == "End" {
        return Some(end);
    }
    if key == backward {
        let collapsed_or_one = selection.start == selection.end || selection.end - selection.start == 1;
        let index = selection.start.saturating_sub(usize::from(collapsed_or_one));
        return Some(if length > 0 {
            Selection::new(index, index + 1)
        } else {
            end
        });
    }
    if key == forward {
        let index = if selection.start == selection.end {
            selection.start
        } else {
            selection.start + 1
        };
        return Some(if index < length {
            Selection::new(index, index + 1)
        } else {
            end
        });
    }
    None
}

/// The slots rendered from the value, and the selection while focused.
pub fn slots_of(value: &str, max_length: usize, selection: Option<Selection>) -> Vec<OtpInputSlotData> {
    let chars: Vec<char> = value.chars().collect();
    (0..max_length)
        .map(|index| {
            let char = chars.get(index).copied();
            let active = selection.is_some_and(|selection| {
                if selection.start == selection.end {
                    index == selection.start
                } else {
                    index >= selection.start && index < selection.end
                }
            });
            OtpInputSlotData {
                char,
                active,
                has_fake_caret: active && char.is_none(),
            }
        })
        .collect()
}
