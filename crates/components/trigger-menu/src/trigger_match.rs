//! Port of `components/trigger-menu/trigger-match.ts`: finding a trigger like
//! "@" or "/" before the caret, and remembering matches a user dismissed.
//! Pure: plain text in, plain data out.
//!
//! Offsets are byte offsets into the editor's text, as the text crate's
//! editor reads it, where the original counts UTF-16 units. Lengths, like a
//! rule's `max_length` and how far back matching reads, count characters.

use std::fmt;
use std::rc::Rc;

use gpui::SharedString;
use icu_properties::CodePointMapData;
use icu_properties::props::{GeneralCategory, GeneralCategoryGroup};
use ui_sdk_text::{TextRange, is_js_space};

/// Where a match may start. `Start`: offset 0 only, so a leading space
/// escapes it. `Line`: the start of a line. `Word`: the start, whitespace, an
/// opening bracket or quote, or right after a token. `Anywhere`: mid-word
/// too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TriggerBoundary {
    Start,
    Line,
    #[default]
    Word,
    Anywhere,
}

impl TriggerBoundary {
    /// The original's name for it, like `"word"`.
    pub fn as_str(self) -> &'static str {
        match self {
            TriggerBoundary::Start => "start",
            TriggerBoundary::Line => "line",
            TriggerBoundary::Word => "word",
            TriggerBoundary::Anywhere => "anywhere",
        }
    }
}

/// Whether the query may contain spaces, or up to how many words, like
/// `@next Wednesday` (`spaces`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TriggerSpaces {
    /// No spaces, `false`.
    #[default]
    No,
    /// Any number of words, `true`.
    Any,
    /// Up to this many words.
    Words(usize),
}

impl TriggerSpaces {
    // Whether a space may be in the query at all (`!!spaces`).
    fn allowed(self) -> bool {
        match self {
            TriggerSpaces::No => false,
            TriggerSpaces::Any => true,
            TriggerSpaces::Words(words) => words != 0,
        }
    }
}

impl From<bool> for TriggerSpaces {
    fn from(spaces: bool) -> Self {
        if spaces { TriggerSpaces::Any } else { TriggerSpaces::No }
    }
}

impl From<usize> for TriggerSpaces {
    fn from(words: usize) -> Self {
        TriggerSpaces::Words(words)
    }
}

/// Characters a query may contain, tested one at a time (`pattern`).
pub type TriggerPattern = Rc<dyn Fn(char) -> bool>;

/// The longest query by default, in characters.
pub const DEFAULT_MAX_LENGTH: usize = 50;

/// Whether a query may contain `char` by default: letters, numbers, marks,
/// `_`, `-` and `.` (`/[\p{L}\p{N}\p{M}_\-.]/u`).
pub fn default_pattern(char: char) -> bool {
    if matches!(char, '_' | '-' | '.') {
        return true;
    }
    let category = CodePointMapData::<GeneralCategory>::new().get(char);
    GeneralCategoryGroup::Letter.contains(category)
        || GeneralCategoryGroup::Number.contains(category)
        || GeneralCategoryGroup::Mark.contains(category)
}

/// How a trigger matches.
#[derive(Clone, Default)]
pub struct TriggerRule {
    /// The text that starts a match: `"@"`, `"/"`, `"#"`, or several
    /// characters like `"[["`.
    pub char: SharedString,
    /// Where a match may start. Defaults to `Word`.
    pub boundary: TriggerBoundary,
    /// Whether the query may contain spaces, or up to how many words, like
    /// `@next Wednesday`. Defaults to `No`.
    pub spaces: TriggerSpaces,
    /// Characters a query may contain, tested one at a time. Defaults to
    /// [`default_pattern`]: letters, numbers, marks, `_`, `-` and `.`.
    pub pattern: Option<TriggerPattern>,
    /// The longest query, in characters, before the match ends. Defaults
    /// to 50.
    pub max_length: Option<usize>,
}

impl TriggerRule {
    /// A rule for the text that starts a match, like `"@"`.
    pub fn new(char: impl Into<SharedString>) -> Self {
        Self {
            char: char.into(),
            ..Self::default()
        }
    }

    pub fn boundary(mut self, boundary: TriggerBoundary) -> Self {
        self.boundary = boundary;
        self
    }

    pub fn spaces(mut self, spaces: impl Into<TriggerSpaces>) -> Self {
        self.spaces = spaces.into();
        self
    }

    pub fn pattern(mut self, pattern: impl Fn(char) -> bool + 'static) -> Self {
        self.pattern = Some(Rc::new(pattern));
        self
    }

    pub fn max_length(mut self, max_length: usize) -> Self {
        self.max_length = Some(max_length);
        self
    }

    // Whether its queries may contain `char`.
    fn accepts(&self, char: char) -> bool {
        match &self.pattern {
            Some(pattern) => pattern(char),
            None => default_pattern(char),
        }
    }

    fn longest(&self) -> usize {
        self.max_length.unwrap_or(DEFAULT_MAX_LENGTH)
    }
}

impl fmt::Debug for TriggerRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TriggerRule")
            .field("char", &self.char)
            .field("boundary", &self.boundary)
            .field("spaces", &self.spaces)
            .field("pattern", &self.pattern.as_ref().map(|_| ".."))
            .field("max_length", &self.max_length)
            .finish()
    }
}

/// A trigger and the text after it, up to the caret.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TriggerMatch {
    /// The trigger's text, like `"@"`.
    pub trigger: SharedString,
    /// Where the trigger starts.
    pub start: usize,
    /// The caret: where the query ends.
    pub end: usize,
    /// The text between the trigger and the caret.
    pub query: SharedString,
}

impl TriggerMatch {
    /// Where the query starts, after the trigger as it was typed, which may
    /// be a full-width form.
    pub fn query_start(&self) -> usize {
        self.end - self.query.len()
    }
}

/// A match a user dismissed with Escape, which edits move along.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DismissedMatch {
    pub trigger: SharedString,
    pub start: usize,
    pub end: usize,
}

/// The match [`find_trigger_match`] found, and the index of the rule that
/// made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundMatch {
    pub rule: usize,
    pub r#match: TriggerMatch,
}

// Full-width forms an input method commits, like a Japanese keyboard's ＠.
fn full_width(char: char) -> char {
    match char {
        '＠' => '@',
        '＃' => '#',
        '／' => '/',
        '＋' => '+',
        '：' => ':',
        '［' => '[',
        other => other,
    }
}

/// The text with the full-width forms an input method commits, like a
/// Japanese keyboard's ＠, as their ASCII forms (`normalizeTriggerText`).
/// Each takes three bytes where its ASCII form takes one, so offsets keep
/// pointing into the text as typed.
pub fn normalize_trigger_text(text: &str) -> String {
    text.chars().map(full_width).collect()
}

// Characters after which a `Word` trigger may start: brackets and quotes that
// open a phrase, and a token, which reads as U+FFFC.
const OPENERS: [char; 6] = ['(', '[', '{', '"', '\'', '\u{FFFC}'];

/// Whether text after a trigger is a query it accepts (`isValidQuery`).
pub fn is_valid_query(query: &str, rule: &TriggerRule) -> bool {
    if query.chars().count() > rule.longest() {
        return false;
    }
    // A trigger followed by whitespace is just text, like "@ home".
    if query.starts_with(is_js_space) || query.contains('\n') {
        return false;
    }
    let spaces = rule.spaces;
    let mut words = 1;
    let mut previous = None;
    for char in query.chars() {
        if char == ' ' {
            // Two spaces in a row end a phrase.
            if !spaces.allowed() || previous == Some(' ') {
                return false;
            }
            words += 1;
        } else if !rule.accepts(char) {
            return false;
        }
        previous = Some(char);
    }
    match spaces {
        TriggerSpaces::Words(most) => words - usize::from(previous == Some(' ')) <= most,
        _ => true,
    }
}

/// Whether a trigger at byte `index` of `text`, which starts at `offset`, is
/// where its boundary allows (`isAtBoundary`).
pub fn is_at_boundary(text: &str, index: usize, offset: usize, boundary: TriggerBoundary) -> bool {
    let absolute = offset + index;
    if boundary == TriggerBoundary::Anywhere || absolute == 0 {
        return true;
    }
    if boundary == TriggerBoundary::Start {
        return false;
    }
    // The character before is outside the text: callers read one extra, so
    // this means unknown.
    let Some(before) = text[..index].chars().next_back() else {
        return false;
    };
    if boundary == TriggerBoundary::Line {
        return before == '\n';
    }
    is_js_space(before) || OPENERS.contains(&before)
}

/// How many characters before the caret matching needs: the longest query
/// and trigger, plus one for the character a boundary checks (`lookBehind`).
pub fn look_behind(rules: &[TriggerRule]) -> usize {
    let longest = rules
        .iter()
        .map(|rule| rule.longest() + rule.char.chars().count())
        .max()
        .unwrap_or(0);
    longest + 1
}

/// The trigger match that ends at the caret, if any (`findTriggerMatch`).
/// `text` is what comes before the caret and `offset` where it starts. Scans
/// back from the caret, so the nearest trigger wins, and stops at the first
/// character no query may contain.
pub fn find_trigger_match(text: &str, offset: usize, rules: &[TriggerRule]) -> Option<FoundMatch> {
    if rules.is_empty() {
        return None;
    }
    let source = normalize_trigger_text(text);
    // Each character's byte in the text as typed, and in `source`.
    let mut chars = Vec::new();
    let mut at = 0;
    for (index, char) in text.char_indices() {
        chars.push((index, at, full_width(char)));
        at += full_width(char).len_utf8();
    }
    let caret = chars.len();
    let limit = look_behind(rules);
    let mut index = caret;
    while index > 0 {
        index -= 1;
        if caret - index > limit {
            break;
        }
        let (start, source_start, char) = chars[index];
        for (rule_index, rule) in rules.iter().enumerate() {
            if rule.char.is_empty() || !source[source_start..].starts_with(rule.char.as_ref()) {
                continue;
            }
            let after = chars
                .get(index + rule.char.chars().count())
                .map_or(text.len(), |(after, _, _)| *after);
            let query = &text[after..];
            if is_valid_query(&normalize_trigger_text(query), rule)
                && is_at_boundary(&source, source_start, offset, rule.boundary)
            {
                return Some(FoundMatch {
                    rule: rule_index,
                    r#match: TriggerMatch {
                        trigger: rule.char.clone(),
                        start: offset + start,
                        end: offset + text.len(),
                        query: SharedString::from(query.to_string()),
                    },
                });
            }
        }
        let continues = rules
            .iter()
            .any(|rule| rule.char.contains(char) || (char == ' ' && rule.spaces.allowed()) || rule.accepts(char));
        if !continues {
            break;
        }
    }
    None
}

/// Moves a dismissed match along with an edit that replaced `range` with
/// `inserted` bytes (`mapDismissed`). Edits before it shift it, edits inside
/// or at its end grow or shrink it, and an edit that touches the trigger
/// itself clears it: the trigger was deleted or retyped. Caret moves and
/// other edits leave it.
pub fn map_dismissed(dismissed: &DismissedMatch, range: TextRange, inserted: usize) -> Option<DismissedMatch> {
    let shift = |offset: usize| (offset + inserted).saturating_sub(range.end - range.start);
    let trigger_end = dismissed.start + dismissed.trigger.len();
    if range.end <= dismissed.start {
        return Some(DismissedMatch {
            start: shift(dismissed.start),
            end: shift(dismissed.end),
            ..dismissed.clone()
        });
    }
    if range.start < trigger_end {
        return None;
    }
    if range.start <= dismissed.end {
        return Some(DismissedMatch {
            end: trigger_end.max(shift(dismissed.end)),
            ..dismissed.clone()
        });
    }
    Some(dismissed.clone())
}
