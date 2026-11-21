//! Port of `components/token-input/token-input-model.ts`: TokenInput's value
//! as plain data, strings and atomic tokens, with offsets where a token
//! counts as one character, the codec that turns a value into text and back,
//! and edits by range.
//!
//! Offsets are byte offsets into the value's offset text, where each token is
//! U+FFFC, as the text crate's editor offsets are: a token spans
//! [`TOKEN_LEN`] bytes where the original counts one UTF-16 unit. Offsets
//! never split a token, so an edit that covers part of a token's range covers
//! all of it, and one that covers part of a character covers the character.

use std::cell::Cell;
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

use gpui::SharedString;
use ui_sdk_kernel::{AgentValue, json};
use ui_sdk_kernel::watchers::register_data;
use ui_sdk_text::{TextRange, Token, TokenStatus, is_js_space};

/// One piece of a token input's value: text, or a token.
///
/// `observe` and history read a value of them as the original's
/// ([`Segment::to_data`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    Text(SharedString),
    Token(Token),
}

/// A token's status as the original names it, like `"pending"`.
pub(crate) fn status_name(status: TokenStatus) -> &'static str {
    match status {
        TokenStatus::Pending => "pending",
        TokenStatus::Unresolved => "unresolved",
    }
}

impl Segment {
    /// The original's segment: the text, or the token's `type`, `value` and
    /// `label`, with its `id` and `status` when it has them.
    pub fn to_data(&self) -> AgentValue {
        match self {
            Segment::Text(text) => AgentValue::from(text.as_str()),
            Segment::Token(token) => {
                let mut data = json!({
                    "type": token.kind.as_str(),
                    "value": token.value.as_str(),
                    "label": token.label.as_str(),
                });
                if let Some(fields) = data.as_object_mut() {
                    if let Some(id) = &token.id {
                        fields.insert("id".into(), id.as_str().into());
                    }
                    if let Some(status) = token.status {
                        fields.insert("status".into(), status_name(status).into());
                    }
                }
                data
            }
        }
    }

    /// Lets `observe` and history read token input values as data. Token
    /// inputs call it as they're made.
    pub fn register_data() {
        register_data::<TokenInputValue>(|value| AgentValue::Array(value.iter().map(Segment::to_data).collect()));
    }

    /// The token, when it's one.
    pub fn token(&self) -> Option<&Token> {
        match self {
            Segment::Token(token) => Some(token),
            Segment::Text(_) => None,
        }
    }

    /// The text, when it's text.
    pub fn text(&self) -> Option<&SharedString> {
        match self {
            Segment::Text(text) => Some(text),
            Segment::Token(_) => None,
        }
    }

    /// How many offsets it spans: its text's length, or a token's.
    pub fn len(&self) -> usize {
        match self {
            Segment::Text(text) => text.len(),
            Segment::Token(_) => TOKEN_LEN,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl From<&str> for Segment {
    fn from(text: &str) -> Self {
        Segment::Text(SharedString::new(text))
    }
}

impl From<String> for Segment {
    fn from(text: String) -> Self {
        Segment::Text(text.into())
    }
}

impl From<SharedString> for Segment {
    fn from(text: SharedString) -> Self {
        Segment::Text(text)
    }
}

impl From<Token> for Segment {
    fn from(token: Token) -> Self {
        Segment::Token(token)
    }
}

/// A token input's value: text and tokens in order. Adjacent strings are
/// merged and empty ones dropped.
pub type TokenInputValue = Vec<Segment>;

/// `format`: the text a token is written as.
pub type TokenFormat = Rc<dyn Fn(&Token) -> String>;
/// `parse`: text back into a value, with tokens wherever their text appears.
pub type TokenParse = Rc<dyn Fn(&str) -> TokenInputValue>;
/// `escape`: how plain text is written so `parse` reads it back as text.
/// The token is the one that follows it, if any.
pub type TokenEscape = Rc<dyn Fn(&str, Option<&Token>) -> String>;

/// How tokens become text and back, for the clipboard, forms and drafts.
#[derive(Clone)]
pub struct TokenCodec {
    /// The text a token is written as.
    pub format: TokenFormat,
    /// Text back into a value, with tokens wherever their text appears.
    pub parse: TokenParse,
    /// How plain text is written so `parse` reads it back as text. Without
    /// one, the text as is.
    pub escape: Option<TokenEscape>,
}

impl TokenCodec {
    /// A codec from how it writes a token and reads text back.
    pub fn new(
        format: impl Fn(&Token) -> String + 'static,
        parse: impl Fn(&str) -> TokenInputValue + 'static,
    ) -> Self {
        Self {
            format: Rc::new(format),
            parse: Rc::new(parse),
            escape: None,
        }
    }

    /// How it writes plain text, so `parse` reads it back as text.
    pub fn escape(mut self, escape: impl Fn(&str, Option<&Token>) -> String + 'static) -> Self {
        self.escape = Some(Rc::new(escape));
        self
    }
}

impl Default for TokenCodec {
    fn default() -> Self {
        default_codec()
    }
}

impl PartialEq for TokenCodec {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.format, &other.format)
            && Rc::ptr_eq(&self.parse, &other.parse)
            && match (&self.escape, &other.escape) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl fmt::Debug for TokenCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenCodec")
            .field("escape", &self.escape.is_some())
            .finish_non_exhaustive()
    }
}

/// A token reads as this in text offsets and in what a trigger menu matches.
pub const TOKEN_CHAR: char = '\u{FFFC}';

/// How many offsets a token spans: U+FFFC's length in UTF-8.
pub const TOKEN_LEN: usize = TOKEN_CHAR.len_utf8();

thread_local! {
    static LAST_ID: Cell<u64> = const { Cell::new(0) };
}

fn next_id() -> SharedString {
    LAST_ID.with(|last| {
        let next = last.get() + 1;
        last.set(next);
        format!("t{next}").into()
    })
}

// A supplied id shaped like a generated one moves the count past it, so no
// generated id repeats it. Past 15 digits the count would lose precision and
// stall, and it never gets that far itself.
fn reserve_id(id: &str) {
    let Some(digits) = id.strip_prefix('t') else {
        return;
    };
    if !(1..=15).contains(&digits.len()) || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return;
    }
    if let Ok(n) = digits.parse::<u64>() {
        LAST_ID.with(|last| last.set(last.get().max(n)));
    }
}

/// Whether a segment is a token (`isToken`).
pub fn is_token(segment: Option<&Segment>) -> bool {
    matches!(segment, Some(Segment::Token(_)))
}

/// Merges adjacent strings, drops empty ones and gives tokens without one an
/// id. Tokens that have an id keep their identity, except one repeating an
/// earlier token's id, which gets a new one.
pub fn normalize_value(value: &[Segment]) -> TokenInputValue {
    for segment in value {
        if let Segment::Token(token) = segment
            && let Some(id) = &token.id
        {
            reserve_id(id);
        }
    }
    let mut ids: HashSet<SharedString> = HashSet::new();
    let mut result: TokenInputValue = Vec::with_capacity(value.len());
    for segment in value {
        match segment {
            Segment::Token(token) => {
                let token = match &token.id {
                    Some(id) if !id.is_empty() && !ids.contains(id) => token.clone(),
                    _ => Token {
                        id: Some(next_id()),
                        ..token.clone()
                    },
                };
                if let Some(id) = &token.id {
                    ids.insert(id.clone());
                }
                result.push(Segment::Token(token));
            }
            Segment::Text(text) => {
                if text.is_empty() {
                    continue;
                }
                match result.last_mut() {
                    Some(Segment::Text(last)) => *last = format!("{last}{text}").into(),
                    _ => result.push(Segment::Text(text.clone())),
                }
            }
        }
    }
    result
}

/// How many offsets a value spans. A token is [`TOKEN_LEN`].
pub fn value_length(value: &[Segment]) -> usize {
    value.iter().map(Segment::len).sum()
}

/// The value as text with every token as U+FFFC, so offsets line up.
pub fn offset_text(value: &[Segment]) -> String {
    let mut text = String::with_capacity(value_length(value));
    for segment in value {
        match segment {
            Segment::Text(part) => text.push_str(part),
            Segment::Token(_) => text.push(TOKEN_CHAR),
        }
    }
    text
}

// The character boundary at or before `offset`.
pub(crate) fn floor_char(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

// The character boundary at or after `offset`.
pub(crate) fn ceil_char(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

/// The part of a value between two offsets. A token or a character the
/// range covers part of comes whole.
pub fn slice_value(value: &[Segment], start: usize, end: usize) -> TokenInputValue {
    let mut result = Vec::new();
    let mut at = 0;
    for segment in value {
        let length = segment.len();
        let from = start.saturating_sub(at);
        let to = end.saturating_sub(at).min(length);
        if from < to {
            match segment {
                Segment::Token(_) => result.push(segment.clone()),
                Segment::Text(text) => {
                    let from = floor_char(text, from);
                    let to = ceil_char(text, to);
                    result.push(Segment::Text(SharedString::new(&text[from..to])));
                }
            }
        }
        at += length;
        if at >= end {
            break;
        }
    }
    result
}

/// A range widened to whole tokens and characters, within the value: the
/// range rule, so an edit never splits a token.
pub fn snap_range(value: &[Segment], range: TextRange) -> TextRange {
    let length = value_length(value);
    let start = range.start.min(range.end).min(length);
    let end = range.end.max(range.start).min(length);
    TextRange::new(snap_offset(value, start, false), snap_offset(value, end, true))
}

/// An offset moved to the edge of the token or character it falls inside:
/// its end when `up`, else its start.
pub fn snap_offset(value: &[Segment], offset: usize, up: bool) -> usize {
    let mut at = 0;
    for segment in value {
        let length = segment.len();
        if offset > at && offset < at + length {
            return match segment {
                Segment::Token(_) => {
                    if up {
                        at + length
                    } else {
                        at
                    }
                }
                Segment::Text(text) => {
                    let inside = offset - at;
                    at + if up {
                        ceil_char(text, inside)
                    } else {
                        floor_char(text, inside)
                    }
                }
            };
        }
        at += length;
    }
    offset.min(at)
}

/// Replaces a range with `content`. Offsets never split a token, so an edit
/// that covers part of a token's range covers all of it: the range rule.
pub fn replace_range(value: &[Segment], range: TextRange, content: &[Segment]) -> TokenInputValue {
    let range = snap_range(value, range);
    let mut next = slice_value(value, 0, range.start);
    next.extend(content.iter().cloned());
    next.extend(slice_value(value, range.end, usize::MAX));
    normalize_value(&next)
}

/// The token at an offset, if one starts there.
pub fn token_at(value: &[Segment], offset: usize) -> Option<&Token> {
    let mut at = 0;
    for segment in value {
        if at == offset
            && let Segment::Token(token) = segment
        {
            return Some(token);
        }
        at += segment.len();
        if at > offset {
            break;
        }
    }
    None
}

/// The token that ends at an offset, if one does: the one just before a
/// caret there.
pub fn token_before(value: &[Segment], offset: usize) -> Option<&Token> {
    offset.checked_sub(TOKEN_LEN).and_then(|start| token_at(value, start))
}

/// Where a token starts, or `None`.
pub fn offset_of_token(value: &[Segment], id: &str) -> Option<usize> {
    let mut at = 0;
    for segment in value {
        if let Segment::Token(token) = segment
            && token.id.as_deref() == Some(id)
        {
            return Some(at);
        }
        at += segment.len();
    }
    None
}

/// The tokens of a value.
pub fn tokens_of(value: &[Segment]) -> Vec<&Token> {
    value.iter().filter_map(Segment::token).collect()
}

/// Tokens in `before` that `after` no longer has.
pub fn removed_tokens(before: &[Segment], after: &[Segment]) -> Vec<Token> {
    let kept: HashSet<&SharedString> = tokens_of(after).iter().filter_map(|token| token.id.as_ref()).collect();
    tokens_of(before)
        .into_iter()
        .filter(|token| !token.id.as_ref().is_some_and(|id| kept.contains(id)))
        .cloned()
        .collect()
}

/// Whether two values hold the same text and the same tokens: ids, kinds,
/// values, labels, statuses and the same data.
pub fn same_value(a: &[Segment], b: &[Segment]) -> bool {
    a == b
}

// The character before `at`, going back, or the one at `at`, going forward.
fn char_toward(text: &str, at: usize, direction: isize) -> Option<char> {
    if direction < 0 {
        text[..at].chars().next_back()
    } else {
        text[at..].chars().next()
    }
}

// One character from `at` toward `direction`.
fn step(text: &str, at: usize, direction: isize) -> usize {
    match char_toward(text, at, direction) {
        Some(c) if direction < 0 => at - c.len_utf8(),
        Some(c) => at + c.len_utf8(),
        None => at,
    }
}

/// Where a word deletion from `offset` stops, going back or forward: past
/// spaces, then past the word. A token is a word of its own, so the deletion
/// stops at it, or removes just it when it's next.
pub fn word_boundary(value: &[Segment], offset: usize, direction: isize) -> usize {
    let text = offset_text(value);
    let mut at = floor_char(&text, offset);
    let start = at;
    let inside = |index: usize| if direction < 0 { index > 0 } else { index < text.len() };
    let space = |c: Option<char>| c.is_some_and(|c| is_js_space(c) && c != '\n');
    let token = |c: Option<char>| c == Some(TOKEN_CHAR);
    if token(char_toward(&text, at, direction)) {
        return step(&text, at, direction);
    }
    while inside(at) && space(char_toward(&text, at, direction)) {
        at = step(&text, at, direction);
    }
    if token(char_toward(&text, at, direction)) && at != start {
        return at;
    }
    while inside(at) {
        let c = char_toward(&text, at, direction);
        if c.is_some_and(is_js_space) || token(c) {
            break;
        }
        at = step(&text, at, direction);
    }
    if at == start && inside(at) {
        step(&text, at, direction)
    } else {
        at
    }
}

/// Where the line around `offset` starts or ends.
pub fn line_boundary(value: &[Segment], offset: usize, direction: isize) -> usize {
    let text = offset_text(value);
    let offset = floor_char(&text, offset);
    if direction < 0 {
        let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
        return if start == offset && offset > 0 {
            offset - 1
        } else {
            start
        };
    }
    match text[offset..].find('\n') {
        None => text.len(),
        Some(0) => offset + 1,
        Some(index) => offset + index,
    }
}

// The default codec writes `[@label](type:value)`, like a markdown link.
// Inside the brackets `\`, `[` and `]` are escaped, inside the parentheses
// `\`, `(` and `)`. In plain text, `[@` is escaped by an odd run of
// backslashes before it, so text that looks like a token stays text.
fn escape_in(text: &str, special: &[char]) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if special.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(next) = chars.next()
        {
            out.push(next);
            continue;
        }
        out.push(c);
    }
    out
}

// The run of backslashes a text ends with.
fn trailing_slashes(text: &str) -> &str {
    let kept = text.trim_end_matches('\\');
    &text[kept.len()..]
}

// What a link's escaped run matches from `at`: escaped characters, and any
// other but a backslash or one of `stops`. Returns where it ends.
fn escaped_run(text: &str, at: usize, stops: &[char]) -> usize {
    let mut index = at;
    let mut chars = text[at..].chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(next) => index += 1 + next.len_utf8(),
                None => break,
            }
        } else if stops.contains(&c) {
            break;
        } else {
            index += c.len_utf8();
        }
    }
    index
}

// The token written at `at`, `[@label](type:value)`, with where it ends.
fn link_at(text: &str, at: usize) -> Option<(Token, usize)> {
    if !text[at..].starts_with("[@") {
        return None;
    }
    let label_start = at + 2;
    let label_end = escaped_run(text, label_start, &[']']);
    if !text[label_end..].starts_with("](") {
        return None;
    }
    let kind_start = label_end + 2;
    let kind_len = text[kind_start..]
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
        .count();
    if kind_len == 0 || !text[kind_start + kind_len..].starts_with(':') {
        return None;
    }
    let kind_end = kind_start + kind_len;
    let value_start = kind_end + 1;
    let value_end = escaped_run(text, value_start, &['(', ')']);
    if !text[value_end..].starts_with(')') {
        return None;
    }
    let token = Token::new(
        SharedString::new(&text[kind_start..kind_end]),
        unescape(&text[value_start..value_end]),
        unescape(&text[label_start..label_end]),
    );
    Some((token, value_end + 1))
}

fn default_format(token: &Token) -> String {
    format!(
        "[@{}]({}:{})",
        escape_in(&token.label, &['\\', '[', ']']),
        token.kind,
        escape_in(&token.value, &['\\', '(', ')'])
    )
}

// Backslashes before `[@` double, plus one more when the bracket is literal.
// A run of backslashes at the end of text before a token doubles too.
fn default_escape(text: &str, next: Option<&Token>) -> String {
    let mut parts: Vec<&str> = text.split("[@").collect();
    let last = parts.pop().unwrap_or_default();
    let mut out = String::with_capacity(text.len() + 8);
    for part in parts {
        out.push_str(part);
        out.push_str(trailing_slashes(part));
        out.push_str("\\[@");
    }
    out.push_str(last);
    if next.is_some() {
        out.push_str(trailing_slashes(last));
    }
    out
}

fn default_parse(text: &str) -> TokenInputValue {
    let mut value: TokenInputValue = Vec::new();
    let mut plain = String::new();
    let mut index = 0;
    while index < text.len() {
        let Some(found) = text[index..].find("[@") else {
            break;
        };
        let bracket = index + found;
        // An odd run of backslashes before the bracket makes it literal; each
        // pair is one backslash.
        let slashes = text[index..bracket].len() - text[index..bracket].trim_end_matches('\\').len();
        plain.push_str(&text[index..bracket - slashes]);
        plain.push_str(&"\\".repeat(slashes / 2));
        if slashes % 2 == 1 {
            plain.push_str("[@");
            index = bracket + 2;
            continue;
        }
        let Some((token, end)) = link_at(text, bracket) else {
            plain.push_str("[@");
            index = bracket + 2;
            continue;
        };
        value.push(Segment::Text(std::mem::take(&mut plain).into()));
        value.push(Segment::Token(token));
        index = end;
    }
    plain.push_str(&text[index..]);
    value.push(Segment::Text(plain.into()));
    normalize_value(&value)
}

/// The codec TokenInput uses unless you pass your own: `[@label](type:value)`.
/// A type is letters, digits, `_` and `-`; any other type is written but
/// reads back as text.
pub fn default_codec() -> TokenCodec {
    thread_local! {
        static DEFAULT: TokenCodec = TokenCodec::new(default_format, default_parse).escape(default_escape);
    }
    DEFAULT.with(Clone::clone)
}

/// A value as codec text, for the clipboard, a form or a draft.
pub fn to_text(value: &[Segment], codec: &TokenCodec) -> String {
    let mut out = String::new();
    for (index, segment) in value.iter().enumerate() {
        match segment {
            Segment::Token(token) => out.push_str(&(codec.format)(token)),
            Segment::Text(text) => match &codec.escape {
                Some(escape) => {
                    let next = value.get(index + 1).and_then(Segment::token);
                    out.push_str(&escape(text, next));
                }
                None => out.push_str(text),
            },
        }
    }
    out
}

/// A value as people read it: text with each token's label.
pub fn to_label(value: &[Segment]) -> String {
    let mut out = String::new();
    for segment in value {
        match segment {
            Segment::Token(token) => out.push_str(&token.label),
            Segment::Text(text) => out.push_str(text),
        }
    }
    out
}

/// Codec text back into a value. Text that isn't a well-formed token stays
/// text.
pub fn from_text(text: &str, codec: &TokenCodec) -> TokenInputValue {
    (codec.parse)(text)
}
