//! Port of `foundation/text/fuzzy.ts`.
//!
//! A small fuzzy scorer for filtering and ranking a list as the user types,
//! like a command palette's. It ignores case and accents the way the user's
//! language does, and prefers, in order: the whole text, a prefix, a word that
//! starts with the query, any substring, then the query's letters in order
//! with bonuses for word starts and runs, so "gp" finds "Git push". Scores run
//! from 0, no match, to 1.

use crate::r#match::{next_char, starts_word};
use crate::typeahead::normalize;

// How much of the text the query covers, as a small bonus that ranks "Set"
// above "Settings" for "set". Lengths count UTF-16 units, as the original's do.
fn coverage(needle: &str, haystack: &str) -> f64 {
    (0.09 * needle.encode_utf16().count() as f64) / haystack.encode_utf16().count() as f64
}

// Where the query appears at the start of a word.
fn word_start_index(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    let mut index = Some(from);
    while let Some(at) = index {
        if starts_word(haystack, at) {
            return Some(at);
        }
        let next = next_char(haystack, at);
        index = haystack[next..].find(needle).map(|found| next + found);
    }
    None
}

// The query's letters in order, anywhere in the text. Spaces in the query
// only separate words, so "git ps" finds "git push". Each letter scores more
// at the start of a word or right after the previous one.
fn subsequence_score(haystack: &str, needle: &str) -> f64 {
    let mut position = 0;
    // Where the previous letter ended, so the next one right there is a run.
    let mut previous_end: Option<usize> = None;
    let mut bonus = 0.0;
    let mut letters = 0.0;
    for char in needle.chars() {
        if char == ' ' {
            continue;
        }
        let Some(found) = haystack[position..].find(char).map(|found| position + found) else {
            return 0.0;
        };
        letters += 1.0;
        if starts_word(haystack, found) {
            bonus += 1.0;
        }
        if previous_end == Some(found) {
            bonus += 1.0;
        }
        position = found + char.len_utf8();
        previous_end = Some(position);
    }
    if letters == 0.0 {
        return 0.0;
    }
    0.1 + (0.4 * bonus) / (2.0 * letters)
}

/// How well `text` matches `query`, from 0 (no match) to 1 (the same text),
/// ignoring case and accents.
pub fn fuzzy_score(text: &str, query: &str) -> f64 {
    let needle = normalize(query);
    if needle.is_empty() {
        return 1.0;
    }
    let haystack = normalize(text);
    if haystack == needle {
        return 1.0;
    }
    match haystack.find(needle.as_ref()) {
        Some(0) => 0.9 + coverage(&needle, &haystack),
        Some(index) => {
            let start = if word_start_index(&haystack, &needle, index).is_some() {
                0.75
            } else {
                0.6
            };
            start + coverage(&needle, &haystack)
        }
        None => subsequence_score(&haystack, &needle),
    }
}

/// A filter that keeps items with the query's letters in order, scored by
/// [`fuzzy_score`] over the label and keywords. Keywords count a little less
/// than the label, so a label match wins a tie.
pub fn fuzzy_filter<K: AsRef<str>>(label: &str, keywords: impl IntoIterator<Item = K>, query: &str) -> f64 {
    let mut best = fuzzy_score(label, query);
    for keyword in keywords {
        best = best.max(fuzzy_score(keyword.as_ref(), query) * 0.95);
    }
    best
}
