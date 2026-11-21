//! Port of `foundation/number/parse.ts`.
//!
//! Reading numbers people type in their locale's format, and the
//! `"start/end"` string a range submits as one form value. Pure, so it runs
//! anywhere.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::format::{
    NumberFormatOptions, NumberPartType, NumberStyle, UseGrouping, is_js_space, number_formatter,
};
use crate::steps::{decimals_of, round_to};

fn is_minus_sign(c: char) -> bool {
    matches!(c, '\u{2212}' | '\u{2012}' | '\u{2013}' | '\u{fe63}' | '\u{ff0d}')
}

fn is_plus_sign(c: char) -> bool {
    matches!(c, '\u{fe62}' | '\u{ff0b}')
}

// The full-width digits, comma and period East Asian input methods commit.
fn half_width(c: char) -> char {
    match c {
        '\u{ff10}'..='\u{ff19}' | '\u{ff0c}' | '\u{ff0e}' => {
            char::from_u32(u32::from(c) - 0xfee0).unwrap_or(c)
        }
        _ => c,
    }
}

// Bidirectional marks and other invisible format characters some locales add.
fn is_format_character(c: char) -> bool {
    matches!(
        c,
        '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

fn is_space(c: char) -> bool {
    is_js_space(c) || c == '\u{a0}' || c == '\u{202f}'
}

fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

struct Symbols {
    group: String,
    decimal: String,
    // Currency, percent and unit text, and literals, which carry no number.
    strip: Vec<String>,
    // The locale's own digits, mapped to 0-9.
    digits: HashMap<char, char>,
    // Divides parsed values, like 100 for percents, which format 0.5 as 50%.
    divisor: f64,
    // Values some locales write as a word alone, like Arabic "يومان" for 2 days.
    words: HashMap<String, f64>,
}

thread_local! {
    static SYMBOLS: RefCell<HashMap<(Option<String>, NumberFormatOptions), Rc<Symbols>>> =
        RefCell::new(HashMap::new());
}

// What a locale's formatted numbers are made of, learned from its formatter itself.
fn symbols_of(locale: Option<&str>, options: &NumberFormatOptions) -> Rc<Symbols> {
    let key = (locale.map(str::to_owned), options.clone());
    if let Some(symbols) = SYMBOLS.with(|symbols| symbols.borrow().get(&key).cloned()) {
        return symbols;
    }
    let formatter = number_formatter(locale, options);
    // Long unit and currency names change with the number, in up to six plural
    // forms, and some locales write a small number as a word alone.
    let samples = [-12345.6, 12345.6, 0.0, 1.0, 2.0, 3.0, 5.0, 11.0, 21.0, 22.0, 1.5, 0.1];
    let mut words = HashMap::new();
    let mut parts = Vec::new();
    for sample in samples {
        let sample_parts = formatter.format_to_parts(sample);
        let key = word_key(&formatter.format(sample));
        if sample.fract() == 0.0
            && sample > 0.0
            && !words.contains_key(&key)
            && !sample_parts.iter().any(|part| part.kind == NumberPartType::Integer)
        {
            words.insert(key, sample);
        }
        parts.extend(sample_parts);
    }
    let find = |kind: NumberPartType| {
        parts
            .iter()
            .find(|part| part.kind == kind)
            .map(|part| part.value.clone())
    };
    // Formats that show no decimals or no grouping, like percents, yen or
    // `UseGrouping::Never`, still read the locale's own separators.
    let numbering = formatter.resolved_options().numbering_system.clone();
    let separators = number_formatter(
        locale,
        &NumberFormatOptions::default()
            .numbering_system(numbering.clone())
            .minimum_fraction_digits(1),
    )
    .format_to_parts(12345.6);
    let separator = |kind: NumberPartType| {
        find(kind).or_else(|| {
            separators
                .iter()
                .find(|part| part.kind == kind)
                .map(|part| part.value.clone())
        })
    };
    let mut strip: Vec<String> = Vec::new();
    for part in &parts {
        if matches!(
            part.kind,
            NumberPartType::Currency
                | NumberPartType::PercentSign
                | NumberPartType::Unit
                | NumberPartType::Literal
                | NumberPartType::PlusSign
        ) {
            let value = part.value.trim_matches(is_js_space);
            if !value.is_empty() && !strip.iter().any(|seen| seen == value) {
                strip.push(value.to_owned());
            }
        }
    }
    // Longest first, so "US$" goes before "$".
    strip.sort_by_key(|symbol| std::cmp::Reverse(symbol.encode_utf16().count()));
    let digit_formatter = number_formatter(
        locale,
        &NumberFormatOptions::default()
            .use_grouping(UseGrouping::Never)
            .numbering_system(numbering),
    );
    let mut digits = HashMap::new();
    for digit in 0..10u8 {
        let text = digit_formatter.format(f64::from(digit));
        let mut chars = text.chars();
        // Text is read a character at a time, so only a one-character digit matches.
        if let (Some(c), None) = (chars.next(), chars.next()) {
            digits.insert(c, char::from(b'0' + digit));
        }
    }
    let symbols = Rc::new(Symbols {
        group: separator(NumberPartType::Group).unwrap_or_else(|| ",".to_owned()),
        decimal: separator(NumberPartType::Decimal).unwrap_or_else(|| ".".to_owned()),
        strip,
        digits,
        divisor: if options.style == Some(NumberStyle::Percent) { 100.0 } else { 1.0 },
        words,
    });
    SYMBOLS.with(|cache| cache.borrow_mut().insert(key, symbols.clone()));
    symbols
}

fn word_key(text: &str) -> String {
    text.chars()
        .filter(|c| !is_format_character(*c))
        .map(|c| if is_minus_sign(c) { '-' } else { c })
        .filter(|c| !is_space(*c))
        .collect()
}

// A value written as a word alone, with the minus sign `format_number` adds.
fn word_value(text: &str, symbols: &Symbols) -> Option<f64> {
    let key = word_key(text);
    let (negative, key) = match key.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, key.as_str()),
    };
    let value = *symbols.words.get(key)?;
    Some(if negative { -value } else { value })
}

// Accounting formats wrap negatives in parentheses, like ($1,234.56).
fn is_parenthesized(text: &str) -> bool {
    let inner = text.trim_matches(is_js_space);
    inner.len() >= 2
        && inner.starts_with('(')
        && inner.ends_with(')')
        && !inner[1..inner.len() - 1].contains(is_line_terminator)
}

// Text reduced to ASCII digits, one "-", and "." as the decimal separator.
fn normalize(text: &str, symbols: &Symbols) -> String {
    let mut result: String = text
        .chars()
        .filter(|c| !is_format_character(*c))
        .map(|c| {
            if is_minus_sign(c) {
                '-'
            } else if is_plus_sign(c) {
                '+'
            } else {
                half_width(c)
            }
        })
        .collect();
    let negative = is_parenthesized(&result);
    result = result
        .chars()
        .map(|c| symbols.digits.get(&c).copied().unwrap_or(c))
        .collect();
    for symbol in &symbols.strip {
        result = result.replace(symbol.as_str(), "");
    }
    let group_is_space = symbols.group.trim_matches(is_js_space).is_empty();
    if !group_is_space
        && let Some(decimal_at) = result.find(symbols.decimal.as_str())
    {
        let after = (decimal_at + result[decimal_at..].chars().next().map_or(1, char::len_utf8))
            .min(result.len());
        if result[after..].contains(symbols.group.as_str()) {
            return "?".to_owned();
        }
    }
    if !group_is_space {
        result = result.replace(symbols.group.as_str(), "");
    }
    result.retain(|c| !is_space(c));
    // Swiss numbers group with an apostrophe, typed either straight or curly.
    if symbols.group == "'" || symbols.group == "\u{2019}" {
        result.retain(|c| !is_apostrophe(c));
    }
    if symbols.decimal != "." {
        // "." can only be a decimal point when the locale doesn't use it to group.
        if symbols.group == "." {
            result = result.replace('.', "");
        }
        result = result.replace(symbols.decimal.as_str(), ".");
    }
    // Accounting formats and some locales put the sign last.
    if trailing_sign(&result) {
        result = format!("-{}", &result[..result.len() - 1]);
    }
    if let Some(rest) = result.strip_prefix('+') {
        result = rest.to_owned();
    }
    if negative {
        format!("-{}", result.replace(['(', ')'], ""))
    } else {
        result
    }
}

// `/^\d.*-$/`: a digit first, a minus last, and no line break between.
fn trailing_sign(text: &str) -> bool {
    text.len() >= 2
        && text.as_bytes()[0].is_ascii_digit()
        && text.ends_with('-')
        && !text[1..text.len() - 1].contains(is_line_terminator)
}

// `/^-?(\d+\.?\d*|\.\d+)$/`
fn is_complete(text: &str) -> bool {
    let text = text.strip_prefix('-').unwrap_or(text);
    let (integer, fraction) = match text.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (text, None),
    };
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    match fraction {
        None => !integer.is_empty() && digits(integer),
        Some(fraction) => {
            digits(integer) && digits(fraction) && (!integer.is_empty() || !fraction.is_empty())
        }
    }
}

// `/^-?\d*\.?\d*$/`
fn is_partial(text: &str) -> bool {
    let text = text.strip_prefix('-').unwrap_or(text);
    let (integer, fraction) = text.split_once('.').unwrap_or((text, ""));
    integer.bytes().all(|byte| byte.is_ascii_digit()) && fraction.bytes().all(|byte| byte.is_ascii_digit())
}

/// Reads a number typed in a locale's format: its digits, group and decimal
/// separators, minus signs, and currency, percent and unit text. Percent
/// formats divide by 100. `None` for text that isn't a number.
pub fn parse_number(text: &str, locale: Option<&str>, format: Option<&NumberFormatOptions>) -> Option<f64> {
    let default = NumberFormatOptions::default();
    let symbols = symbols_of(locale, format.unwrap_or(&default));
    if let Some(word) = word_value(text, &symbols) {
        return Some(word / symbols.divisor);
    }
    let normalized = normalize(text, &symbols);
    if !is_complete(&normalized) {
        return None;
    }
    let number = js_number(&normalized);
    let value = number / symbols.divisor;
    value
        .is_finite()
        .then(|| round_to(value, decimals_of(number) + 2))
}

/// Whether text could still become a number as someone types it, like "-",
/// "1." or "$1,2". Without `allow_negative`, a minus sign isn't accepted.
pub fn is_partial_number(
    text: &str,
    locale: Option<&str>,
    format: Option<&NumberFormatOptions>,
    allow_negative: bool,
) -> bool {
    let default = NumberFormatOptions::default();
    let symbols = symbols_of(locale, format.unwrap_or(&default));
    if let Some(word) = word_value(text, &symbols) {
        return allow_negative || word >= 0.0;
    }
    let normalized = normalize(text, &symbols);
    if !is_partial(&normalized) {
        return false;
    }
    allow_negative || !normalized.starts_with('-')
}

/// The numbers in a `"start/end"` string, or `None` when any part isn't one.
pub fn parse_range(value: &str) -> Option<Vec<f64>> {
    let parts: Vec<f64> = value
        .split('/')
        .map(|part| {
            if part.trim_matches(is_js_space).is_empty() {
                f64::NAN
            } else {
                js_number(part)
            }
        })
        .collect();
    parts.iter().all(|part| part.is_finite()).then_some(parts)
}

// `Number(text)`: a decimal literal with an optional sign and exponent,
// "Infinity", or an unsigned hexadecimal, octal or binary integer, between
// whitespace; empty text is 0, anything else NaN.
fn js_number(text: &str) -> f64 {
    let text = text.trim_matches(is_js_space);
    if text.is_empty() {
        return 0.0;
    }
    let bytes = text.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'0' {
        let radix = match bytes[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let mut exact: u128 = 0;
            let mut approximate = 0.0f64;
            let mut overflow = false;
            for c in text[2..].chars() {
                let Some(digit) = c.to_digit(radix) else {
                    return f64::NAN;
                };
                if !overflow {
                    match exact
                        .checked_mul(u128::from(radix))
                        .and_then(|value| value.checked_add(u128::from(digit)))
                    {
                        Some(value) => exact = value,
                        None => {
                            overflow = true;
                            approximate = exact as f64;
                        }
                    }
                }
                if overflow {
                    approximate = approximate * f64::from(radix) + f64::from(digit);
                }
            }
            return if overflow { approximate } else { exact as f64 };
        }
    }
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let magnitude = if unsigned == "Infinity" {
        f64::INFINITY
    } else if is_decimal_literal(unsigned) {
        unsigned.parse().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    };
    if negative { -magnitude } else { magnitude }
}

// StrUnsignedDecimalLiteral: digits with an optional point and fraction, or a
// point and digits, then an optional exponent.
fn is_decimal_literal(text: &str) -> bool {
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(at) => (&text[..at], Some(&text[at + 1..])),
        None => (text, None),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(integer) || !digits(fraction) || (integer.is_empty() && fraction.is_empty()) {
        return false;
    }
    match exponent {
        None => true,
        Some(exponent) => {
            let exponent = exponent
                .strip_prefix(['+', '-'])
                .unwrap_or(exponent);
            !exponent.is_empty() && digits(exponent)
        }
    }
}
