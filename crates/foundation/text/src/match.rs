//! Port of `foundation/text/match.ts`.
//!
//! How text matches what the user typed, in their language: case and accents
//! don't count where their language says they don't, through ICU4X's collator
//! at primary strength (`Intl.Collator`'s `sensitivity: "base"`). Folding
//! keeps every character in place, one for one, so a match in the folded text
//! is the same characters in the label, which is what match highlighting
//! marks. Ranges are byte offsets into the label, so `&label[range.range()]`
//! slices it and gpui's `StyledText::with_highlights` takes them as they are.
//! Pure, so it runs anywhere.
//!
//! The original asks for `usage: "search"`. ICU4X ships no search tailorings,
//! as its docs say, so the standard collation decides, which agrees on single
//! letters at primary strength.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use gpui::SharedString;
use icu_casemap::CaseMapper;
use icu_collator::options::{CollatorOptions, Strength};
use icu_collator::{CollatorBorrowed, CollatorPreferences};
use icu_locale_core::{LanguageIdentifier, Locale};
use icu_normalizer::DecomposingNormalizerBorrowed;
use icu_properties::props::{Diacritic, Script};
use icu_properties::{CodePointMapData, CodePointSetData};

use crate::locale::user_locale;

/// A matched part of a text, from `start` up to `end`, as byte offsets into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MatchRange {
    pub start: usize,
    pub end: usize,
}

impl MatchRange {
    /// The range to slice the text with, or to highlight it with.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }
}

/// A piece of an item's text, and whether it matched what was typed
/// (`match` in the original, a keyword here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchSegment {
    pub text: SharedString,
    pub matched: bool,
}

/// How a filter matches: what [`match_ranges`] marks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MatchMode {
    /// The first place the query appears.
    #[default]
    Contains,
    /// The query at the start, after any leading whitespace.
    StartsWith,
    /// The whole query where it appears, preferring a word start, else its
    /// letters in order.
    Fuzzy,
}

// Letters NFD can't take apart, like ø or ł, are tried against these.
const LATIN_LETTERS: &str = "abcdefghijklmnopqrstuvwxyz";

// Locales whose lowercase differs from the default, like Turkish dotless i
// (`/^(tr|az|lt)\b/i`).
fn special_case(locale: &str) -> bool {
    let lower = locale.to_ascii_lowercase();
    ["tr", "az", "lt"].iter().any(|prefix| {
        lower.starts_with(prefix)
            && lower[prefix.len()..]
                .chars()
                .next()
                .is_none_or(|next| !(next.is_ascii_alphanumeric() || next == '_'))
    })
}

/// Folds text for matching in one locale: `fold("Łódź")` is `"lodz"` in
/// English, while Swedish keeps `"ä"` apart from `"a"`. The result has the
/// text's characters, one for one, so the nth character of it is the nth of
/// the text (`createFolder`).
pub struct Folder {
    locale: Option<SharedString>,
    // The locale's collator, or the root one. Compiled data always has the
    // root, so it's there in practice.
    collator: Option<CollatorBorrowed<'static>>,
    // The language whose lowercase differs from the default, if it's one.
    special: Option<LanguageIdentifier>,
    chars: RefCell<HashMap<char, char>>,
    texts: RefCell<HashMap<SharedString, SharedString>>,
}

impl Folder {
    /// A folder for `locale`, or the root collation without one.
    pub fn new(locale: Option<&str>) -> Self {
        let mut options = CollatorOptions::default();
        options.strength = Some(Strength::Primary);
        let parsed = locale.and_then(|locale| Locale::try_from_str(locale).ok());
        let collator = parsed
            .as_ref()
            .and_then(|locale| CollatorBorrowed::try_new(CollatorPreferences::from(locale), options).ok())
            .or_else(|| CollatorBorrowed::try_new(CollatorPreferences::default(), options).ok());
        let special = parsed
            .filter(|_| locale.is_some_and(special_case))
            .map(|locale| locale.id);
        Self {
            locale: locale.map(SharedString::new),
            collator,
            special,
            chars: RefCell::new(HashMap::new()),
            texts: RefCell::new(HashMap::new()),
        }
    }

    /// The locale it folds for.
    pub fn locale(&self) -> Option<&SharedString> {
        self.locale.as_ref()
    }

    fn lower(&self, text: &str) -> String {
        match &self.special {
            Some(langid) => CaseMapper::new().lowercase_to_string(text, langid).into_owned(),
            None => text.to_lowercase(),
        }
    }

    fn equal(&self, a: &str, b: &str) -> bool {
        match &self.collator {
            Some(collator) => collator.compare(a, b) == Ordering::Equal,
            None => a == b,
        }
    }

    // One character's folded form, a single character, decided once by the collator.
    fn fold_char(&self, char: char) -> char {
        if let Some(folded) = self.chars.borrow().get(&char) {
            return *folded;
        }
        let mut buffer = [0; 4];
        let text: &str = char.encode_utf8(&mut buffer);
        let lowered = self.lower(text);
        if char < '\u{80}' {
            let folded = single(&lowered).unwrap_or(char);
            self.chars.borrow_mut().insert(char, folded);
            return folded;
        }
        let decomposed = DecomposingNormalizerBorrowed::new_nfd().normalize(text);
        let diacritics = CodePointSetData::new::<Diacritic>();
        let undecorated: String = decomposed.chars().filter(|c| !diacritics.contains(*c)).collect();
        let stripped = self.lower(&undecorated);
        let mut folded = None;
        if let Some(stripped) = single(&stripped)
            && stripped.to_string() != lowered
            && self.equal(text, &stripped.to_string())
        {
            folded = Some(stripped);
        } else if CodePointMapData::<Script>::new().get(char) == Script::Latin {
            folded = LATIN_LETTERS
                .chars()
                .find(|letter| self.equal(text, letter.encode_utf8(&mut [0; 4])));
        }
        let folded = folded.unwrap_or_else(|| single(&lowered).unwrap_or(char));
        self.chars.borrow_mut().insert(char, folded);
        folded
    }

    /// The text folded: case and accents that don't count in the locale are gone.
    pub fn fold(&self, text: &str) -> SharedString {
        if let Some(folded) = self.texts.borrow().get(text) {
            return folded.clone();
        }
        let folded: String = if text.is_ascii() && self.special.is_none() {
            text.to_ascii_lowercase()
        } else if text.is_ascii() {
            // Lowercase alone, one character at a time, which for ASCII is the
            // same as the whole text's.
            text.chars().map(|char| single(&self.lower(char.encode_utf8(&mut [0; 4]))).unwrap_or(char)).collect()
        } else {
            text.chars().map(|char| self.fold_char(char)).collect()
        };
        let folded = SharedString::from(folded);
        let mut texts = self.texts.borrow_mut();
        // Bounded, so labels that come and go can't grow it forever.
        if texts.len() > 50_000 {
            texts.clear();
        }
        texts.insert(SharedString::new(text), folded.clone());
        folded
    }
}

/// A folder for one locale (`createFolder`): see [`Folder`].
pub fn create_folder(locale: Option<&str>) -> Folder {
    Folder::new(locale)
}

// The one character a text is, if it's one.
fn single(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

// The locale `set_match_locale` asked for, and how many times it was set, so
// each thread's folder knows to start over.
static MATCH_LOCALE: RwLock<Option<SharedString>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static FOLDER: RefCell<Option<(u64, Rc<Folder>)>> = const { RefCell::new(None) };
}

/// Matches text in another locale than the user's, or in the user's again without one.
pub fn set_match_locale(locale: Option<&str>) {
    *MATCH_LOCALE.write().unwrap_or_else(|error| error.into_inner()) = locale.map(SharedString::new);
    GENERATION.fetch_add(1, AtomicOrdering::Relaxed);
}

// The folder for the user's language, or the one `set_match_locale` asked for.
fn user_folder() -> Rc<Folder> {
    let generation = GENERATION.load(AtomicOrdering::Relaxed);
    FOLDER.with(|cell| {
        if let Some((built, folder)) = &*cell.borrow()
            && *built == generation
        {
            return folder.clone();
        }
        let locale = MATCH_LOCALE
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .or_else(user_locale);
        let folder = Rc::new(Folder::new(locale.as_deref()));
        *cell.borrow_mut() = Some((generation, folder.clone()));
        folder
    })
}

/// Text folded for matching in the user's language: case and accents that
/// don't count there are gone. One character for each of the text's.
pub fn fold_text(text: &str) -> SharedString {
    user_folder().fold(text)
}

/// How many bytes of spaces and line breaks a text starts with, so
/// "startsWith" skips the indentation a label's markup may carry.
pub fn leading_spaces(text: &str) -> usize {
    text.bytes().take_while(|byte| *byte <= 32).count()
}

// Characters that end a word, so the one after them starts the next.
const SEPARATORS: [u8; 13] = [
    b' ', b'-', b'_', b'/', b'.', b':', b',', b'(', b')', b'[', b']', b'@', b'#',
];

/// Whether a word starts at a byte offset, after a separator or at the start.
pub fn starts_word(text: &str, index: usize) -> bool {
    index == 0 || text.as_bytes().get(index - 1).is_some_and(|byte| SEPARATORS.contains(byte))
}

// The byte offset of the character after the one at `index`.
pub(crate) fn next_char(text: &str, index: usize) -> usize {
    index + text[index..].chars().next().map_or(1, char::len_utf8)
}

/// The ranges of `text` that `query` matched, the way a filter matches:
/// [`MatchMode::Contains`] the first place it appears,
/// [`MatchMode::StartsWith`] the start, and [`MatchMode::Fuzzy`] a whole match
/// where there is one, else each letter in order. Empty when nothing matched,
/// like when only a keyword did.
pub fn match_ranges(text: &str, query: &str, mode: MatchMode) -> Vec<MatchRange> {
    let needle = fold_text(js_trim(query));
    if needle.is_empty() {
        return Vec::new();
    }
    let haystack = fold_text(text);
    let ranges = folded_ranges(&haystack, &needle, mode);
    if ranges.is_empty() {
        return ranges;
    }
    // Folding keeps characters one for one, so a range of the folded text's
    // characters is the same characters of the text.
    let folded = char_starts(&haystack);
    let original = char_starts(text);
    let to_text = |offset: usize| {
        let index = folded.partition_point(|start| *start < offset);
        original.get(index).copied().unwrap_or(text.len())
    };
    ranges
        .into_iter()
        .map(|range| MatchRange {
            start: to_text(range.start),
            end: to_text(range.end),
        })
        .collect()
}

// The ranges in the folded text.
fn folded_ranges(haystack: &str, needle: &str, mode: MatchMode) -> Vec<MatchRange> {
    if mode == MatchMode::StartsWith {
        let start = leading_spaces(haystack);
        return if haystack[start..].starts_with(needle) {
            vec![MatchRange {
                start,
                end: start + needle.len(),
            }]
        } else {
            Vec::new()
        };
    }
    if let Some(index) = haystack.find(needle) {
        // Fuzzy matching prefers a word start, like "push" in "Git push".
        let start = if mode == MatchMode::Fuzzy {
            word_start(haystack, needle, index)
        } else {
            index
        };
        return vec![MatchRange {
            start,
            end: start + needle.len(),
        }];
    }
    if mode == MatchMode::Fuzzy {
        letter_ranges(haystack, needle)
    } else {
        Vec::new()
    }
}

fn word_start(haystack: &str, needle: &str, first: usize) -> usize {
    let mut index = Some(first);
    while let Some(at) = index {
        if starts_word(haystack, at) {
            return at;
        }
        let from = next_char(haystack, at);
        index = haystack[from..].find(needle).map(|found| from + found);
    }
    first
}

// The query's letters in order, as ranges with neighbours joined. Spaces in
// the query only separate words.
fn letter_ranges(haystack: &str, needle: &str) -> Vec<MatchRange> {
    let mut ranges: Vec<MatchRange> = Vec::new();
    let mut position = 0;
    for char in needle.chars() {
        if char == ' ' {
            continue;
        }
        let Some(found) = haystack[position..].find(char).map(|found| position + found) else {
            return Vec::new();
        };
        let end = found + char.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == found => last.end = end,
            _ => ranges.push(MatchRange { start: found, end }),
        }
        position = end;
    }
    ranges
}

// The byte offset each character of a text starts at, and the text's length.
fn char_starts(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(start, _)| start)
        .chain(std::iter::once(text.len()))
        .collect()
}

/// Text split into the parts a query matched and the parts it didn't, in order.
pub fn match_segments(text: &str, ranges: &[MatchRange]) -> Vec<MatchSegment> {
    if ranges.is_empty() {
        return vec![MatchSegment {
            text: SharedString::new(text),
            matched: false,
        }];
    }
    let piece = |range: Range<usize>| SharedString::new(text.get(range).unwrap_or_default());
    let mut segments = Vec::new();
    let mut position = 0;
    for range in ranges {
        if range.start > position {
            segments.push(MatchSegment {
                text: piece(position..range.start),
                matched: false,
            });
        }
        segments.push(MatchSegment {
            text: piece(range.start..range.end),
            matched: true,
        });
        position = range.end;
    }
    if position < text.len() {
        segments.push(MatchSegment {
            text: piece(position..text.len()),
            matched: false,
        });
    }
    segments
}

/// Whether a character is whitespace or a line break the way JavaScript's
/// `\s` and `trim()` mean it.
pub fn is_js_space(char: char) -> bool {
    matches!(
        char,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// Text without the surrounding whitespace JavaScript's `trim()` removes.
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}
