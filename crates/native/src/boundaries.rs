//! Where the caret stops in text: characters as a reader sees them
//! (grapheme clusters), words as Unicode segments them, paragraphs between
//! line breaks, and the offsets platform text APIs count in UTF-16.
//!
//! Words are Unicode's (UAX #29), with dictionaries for the scripts written
//! without spaces (Chinese, Japanese, Thai, Khmer, Lao, Burmese), as a Mac's
//! text does: a double click on `日本語を話す` selects `日本語`, not `日`.

use std::ops::Range;

use icu_segmenter::WordSegmenter;
use icu_segmenter::options::WordBreakInvariantOptions;
use unicode_segmentation::UnicodeSegmentation;

/// The text cut into words, runs of spaces and single marks of punctuation,
/// each with where it starts.
fn word_segments(text: &str) -> Vec<(usize, &str)> {
    let segmenter = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
    let mut out = Vec::new();
    let mut last = 0;
    for at in segmenter.segment_str(text) {
        if at > last {
            out.push((last, &text[last..at]));
        }
        last = at;
    }
    out
}

/// The nearest character boundary at or before `offset`.
pub(crate) fn floor_char(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// The start of the character before `offset`.
pub(crate) fn previous_grapheme(text: &str, offset: usize) -> usize {
    text[..offset]
        .grapheme_indices(true)
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

/// The end of the character after `offset`.
pub(crate) fn next_grapheme(text: &str, offset: usize) -> usize {
    text[offset..]
        .graphemes(true)
        .next()
        .map(|grapheme| offset + grapheme.len())
        .unwrap_or(text.len())
}

/// The start of the character `offset` falls in.
pub(crate) fn grapheme_start(text: &str, offset: usize) -> usize {
    let offset = floor_char(text, offset);
    if offset == text.len() {
        return offset;
    }
    let next = next_grapheme(text, offset);
    let start = previous_grapheme(text, next);
    start.min(offset)
}

// A segment with a letter or digit in it is a word. Spaces and punctuation
// between words are what word moves skip.
fn is_word(segment: &str) -> bool {
    segment.chars().any(char::is_alphanumeric)
}

fn is_space(segment: &str) -> bool {
    segment.chars().all(char::is_whitespace)
}

/// The start of the word before `offset`: Option-Left on a Mac, Control-Left
/// elsewhere.
pub(crate) fn previous_word_start(text: &str, offset: usize) -> usize {
    word_segments(&text[..offset])
        .into_iter()
        .rev()
        .find(|(_, segment)| is_word(segment))
        .map(|(start, _)| start)
        .unwrap_or(0)
}

/// The end of the word after `offset`: Option-Right on a Mac, Control-Right
/// on Linux.
pub(crate) fn next_word_end(text: &str, offset: usize) -> usize {
    word_segments(&text[offset..])
        .into_iter()
        .find(|(_, segment)| is_word(segment))
        .map(|(start, segment)| offset + start + segment.len())
        .unwrap_or(text.len())
}

/// The start of the word after the one at `offset`: Control-Right on Windows.
pub(crate) fn next_word_start(text: &str, offset: usize) -> usize {
    let mut first = true;
    for (start, segment) in word_segments(&text[offset..]) {
        let space = is_space(segment);
        if first {
            first = false;
            if !space {
                continue;
            }
        }
        if !space {
            return offset + start;
        }
    }
    text.len()
}

/// The word, run of spaces or punctuation that the character starting at
/// `offset` belongs to, what a double click selects. At the end of the text
/// or of a paragraph, the word before it.
pub fn word_at(text: &str, offset: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let offset = offset.min(text.len());
    let mut last = 0..0;
    for (start, segment) in word_segments(text) {
        let end = start + segment.len();
        if offset < end {
            if segment == "\n" && start > 0 {
                return last;
            }
            return start..end;
        }
        last = start..end;
    }
    last
}

/// The paragraph `offset` is in, without its line break: what a triple
/// click selects.
pub fn paragraph_at(text: &str, offset: usize) -> Range<usize> {
    let offset = floor_char(text, offset);
    paragraph_start(text, offset)..paragraph_end(text, offset)
}

/// The start of the paragraph `offset` is in.
pub(crate) fn paragraph_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map(|index| index + 1).unwrap_or(0)
}

/// The end of the paragraph `offset` is in, before its line break.
pub(crate) fn paragraph_end(text: &str, offset: usize) -> usize {
    text[offset..]
        .find('\n')
        .map(|index| offset + index)
        .unwrap_or(text.len())
}

/// How many characters a reader counts, which a maximum length limits.
pub(crate) fn char_count(text: &str) -> usize {
    text.chars().count()
}

/// The UTF-8 offset of a UTF-16 one, which the platform's text input counts in.
pub(crate) fn utf16_to_offset(text: &str, utf16: usize) -> usize {
    let mut offset = 0;
    let mut count = 0;
    for c in text.chars() {
        if count >= utf16 {
            break;
        }
        count += c.len_utf16();
        offset += c.len_utf8();
    }
    offset
}

/// The UTF-16 offset of a UTF-8 one.
pub(crate) fn offset_to_utf16(text: &str, offset: usize) -> usize {
    text[..floor_char(text, offset)].encode_utf16().count()
}

pub(crate) fn utf16_range_to_offsets(text: &str, range: &Range<usize>) -> Range<usize> {
    utf16_to_offset(text, range.start)..utf16_to_offset(text, range.end)
}

pub(crate) fn offsets_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    offset_to_utf16(text, range.start)..offset_to_utf16(text, range.end)
}

/// Where two texts differ: the range of `before` that was replaced, and what
/// replaced it (`textDiff` in the original's `caret.ts`).
pub(crate) fn text_diff(before: &str, after: &str) -> (Range<usize>, String) {
    let shortest = before.len().min(after.len());
    let mut start = 0;
    while start < shortest && before.as_bytes()[start] == after.as_bytes()[start] {
        start += 1;
    }
    let start = floor_char(before, start).min(floor_char(after, start));
    let mut end = 0;
    while end < shortest - start
        && before.as_bytes()[before.len() - 1 - end] == after.as_bytes()[after.len() - 1 - end]
    {
        end += 1;
    }
    let mut before_end = before.len() - end;
    let mut after_end = after.len() - end;
    while !before.is_char_boundary(before_end) || !after.is_char_boundary(after_end) {
        before_end += 1;
        after_end += 1;
    }
    (start..before_end, after[start..after_end].to_string())
}
