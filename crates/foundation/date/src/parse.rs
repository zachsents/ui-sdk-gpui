//! Port of `foundation/date/parse.ts`.
//!
//! Reading a date someone typed, in the order their locale writes dates. The
//! order, separators and month names all come from `Intl`, so "16.10.2026"
//! is October 16 in German, "10/16/2026" is in American English, and
//! "16 oct 2026" works in French and English alike.
//!
//! The original indexes text as JavaScript does, by UTF-16 code unit, and
//! its Unicode classes see half of a character beyond the Basic Multilingual
//! Plane as nothing. Lengths and word edges here count the same way, so text
//! reads as it does in a browser.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use icu_locale_core::Locale;

use crate::format::{date_formatter, month_names, weekday_names};
use crate::intl::{
    DateTimeField, DateTimeFormatOptions, DateTimeFormatPartType, DateTimeStyle, DateTimeValue,
    MonthWidth, NumericWidth, TextWidth, default_locale, is_decimal_digit, is_letter, is_mark,
    is_number, native_digits, normalize_nfkc, parse_locale, to_locale_lower_case,
    to_locale_upper_case,
};
use crate::plain_date::{ISODate, date_parts, iso_date};

/// A date field's part, in the order a locale writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateField {
    Year,
    Month,
    Day,
}

impl DateField {
    /// The field's name, as the original writes it: `"year"`, `"month"` or `"day"`.
    pub fn as_str(self) -> &'static str {
        match self {
            DateField::Year => "year",
            DateField::Month => "month",
            DateField::Day => "day",
        }
    }

    fn of(r#type: DateTimeFormatPartType) -> Option<Self> {
        Some(match r#type {
            DateTimeFormatPartType::Year => DateField::Year,
            DateTimeFormatPartType::Month => DateField::Month,
            DateTimeFormatPartType::Day => DateField::Day,
            _ => return None,
        })
    }
}

/// How a locale writes a date in numbers, from [`date_pattern`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatePattern {
    /// The fields in the locale's order, like `[Month, Day, Year]` in American English.
    pub fields: Vec<DateField>,
    /// What goes between the fields, like `"/"` or `"."`.
    pub separator: String,
    /// A placeholder for an empty field, like `"MM/DD/YYYY"`, `"TT.MM.JJJJ"`
    /// in German or `"ДД.ММ.ГГГГ"` in Russian.
    pub placeholder: String,
    /// December 31 of this year in the locale's numeric form, like
    /// `"12/31/2026"`, for a hint that shows the order.
    pub example: String,
}

/// Options for [`parse_date`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParseDateOptions {
    /// The date whose year a date typed without one takes, and whose century
    /// a two-digit year lands near. Today by default.
    pub reference_date: Option<ISODate>,
}

impl ParseDateOptions {
    pub fn reference_date(mut self, date: impl Into<ISODate>) -> Self {
        self.reference_date = Some(date.into());
        self
    }
}

// Marks that set text direction, which right-to-left locales put around
// separators. They're invisible, so neither a hint nor a parser should keep them.
fn without_direction_marks(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}'))
        .collect()
}

fn field_letter(field: DateField) -> &'static str {
    match field {
        DateField::Year => "Y",
        DateField::Month => "M",
        DateField::Day => "D",
    }
}

fn numeric_date() -> DateTimeFormatOptions {
    DateTimeFormatOptions::default()
        .year(NumericWidth::Numeric)
        .month(MonthWidth::TwoDigit)
        .day(NumericWidth::TwoDigit)
}

thread_local! {
    static PATTERNS: RefCell<HashMap<String, DatePattern>> = RefCell::default();
}

/// How a locale writes a date in numbers: its field order, separator, a
/// placeholder and an example.
///
/// # Panics
///
/// When the locale isn't well formed, as `Intl` throws a `RangeError`.
pub fn date_pattern(locale: Option<&str>) -> DatePattern {
    let key = locale.unwrap_or("").to_string();
    if let Some(pattern) = PATTERNS.with(|patterns| patterns.borrow().get(&key).cloned()) {
        return pattern;
    }
    let pattern = read_pattern(locale);
    PATTERNS.with(|patterns| patterns.borrow_mut().insert(key, pattern.clone()));
    pattern
}

fn read_pattern(locale: Option<&str>) -> DatePattern {
    let year = this_year();
    let parts = date_formatter(locale, &numeric_date()).format_to_parts(DateTimeValue::new(year, 12, 31));
    let names = field_names(locale);
    let mut fields = Vec::new();
    let mut separator = String::new();
    let mut placeholder = String::new();
    for part in &parts {
        let value = without_direction_marks(&part.value);
        if let Some(field) = DateField::of(part.r#type) {
            fields.push(field);
            let letter = &names[field as usize];
            placeholder.push_str(&letter.repeat(if field == DateField::Year { 4 } else { 2 }));
        } else {
            if fields.len() == 1 && separator.is_empty() {
                separator = value.clone();
            }
            placeholder.push_str(&value);
        }
    }
    let example = parts
        .iter()
        .map(|part| without_direction_marks(&part.value))
        .collect();
    DatePattern {
        fields,
        separator,
        placeholder,
        example,
    }
}

// The letter each field's placeholder repeats: the first letter of its name in
// the locale when the script has capitals ("T" for Tag, "J" for Jahr), else
// the English one.
fn field_names(locale: Option<&str>) -> [String; 3] {
    let english = || [DateField::Year, DateField::Month, DateField::Day].map(|field| field_letter(field).to_string());
    let resolved = match locale {
        Some(tag) => match parse_locale(tag) {
            Some(locale) => locale,
            None => return english(),
        },
        None => default_locale(),
    };
    let letter = |field: DateField, name: DateTimeField| {
        let name = crate::intl::date_time_field_name(Some(&resolved.to_string()), name);
        // `name[0]`: half of a character beyond the Basic Multilingual Plane
        // has no case, so it reads as caseless.
        match name.chars().next() {
            Some(first) if (first as u32) <= 0xFFFF => {
                let upper = to_locale_upper_case(&first.to_string(), &resolved);
                if upper != to_locale_lower_case(&upper, &resolved) {
                    upper
                } else {
                    field_letter(field).to_string()
                }
            }
            _ => field_letter(field).to_string(),
        }
    };
    [
        letter(DateField::Year, DateTimeField::Year),
        letter(DateField::Month, DateTimeField::Month),
        letter(DateField::Day, DateTimeField::Day),
    ]
}

/// The ISO date in text someone typed, or `None` when it isn't a real date.
/// It reads the locale's order with any separator ("16.10.2026",
/// "16/10/26", "16 10 2026"), digits alone ("16102026"), month names
/// ("16 Oct 2026", "October 16"), the locale's own digits, and ISO dates. A
/// missing year is the reference date's, today's by default. Years read in
/// the locale's calendar, like Buddhist years in Thai. A two-digit year lands
/// in the hundred years that end 20 years after the reference year: "26" is
/// 2026 and "70" is 1970 in 2026.
///
/// # Panics
///
/// When the locale isn't well formed, as `Intl` throws a `RangeError`.
pub fn parse_date(text: &str, locale: Option<&str>, options: &ParseDateOptions) -> Option<ISODate> {
    let normalized = to_latin_digits(js_trim(&without_direction_marks(text)), locale);
    if normalized.is_empty() {
        return None;
    }
    let DatePattern {
        fields, separator, ..
    } = date_pattern(locale);
    // Kyrgyz writes numeric dates year, day, month with dashes, so there a
    // "yyyy-mm-dd" shape is the locale's own and not ISO.
    let year_day_month = separator == "-"
        && fields.first() == Some(&DateField::Year)
        && fields.get(1) != Some(&DateField::Month);
    if !year_day_month && date_parts(&normalized).is_some() {
        return Some(normalized.into());
    }
    // `Number(referenceDate.slice(0, 4)) || new Date().getFullYear()`
    let reference: String = options
        .reference_date
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(4)
        .collect();
    let reference_year = match js_number(&reference) {
        year if year == 0.0 || year.is_nan() => f64::from(this_year()),
        year => year,
    };

    let resolved = lowercase_locale(locale);
    let mut named_month: Option<u32> = None;
    let mut words = to_locale_lower_case(&normalized, &resolved);
    // Names with a space, apostrophe or number in them, like Irish "deireadh
    // fómhair" or Vietnamese "tháng 10", are found before the text splits.
    for (name, month) in &months(locale).spaced {
        if let Some(at) = word_index(&words, name) {
            named_month = Some(*month);
            words = format!("{} {}", &words[..at], &words[at + name.len()..]);
            break;
        }
    }
    let mut numbers: Vec<String> = Vec::new();
    for token in tokens(&words) {
        if token.bytes().all(|byte| byte.is_ascii_digit()) {
            numbers.push(token.to_string());
            continue;
        }
        let month = month_from_name(token, locale);
        if month.is_some() && named_month.is_none() {
            named_month = month;
        } else if !is_pattern_word(token, locale) {
            return None;
        }
    }

    let mut values = Values::default();
    if let Some(named_month) = named_month {
        if numbers.len() > 2 || numbers.is_empty() {
            return None;
        }
        // With one number beside the name, it's the day: "October 16". Two read
        // in the long date's order, then the numeric one, like Malagasy "2026 Okt 16".
        let orders: Vec<Vec<DateField>> = if numbers.len() == 1 {
            vec![vec![DateField::Day]]
        } else {
            vec![named_order(locale), fields.clone()]
        };
        for order in orders {
            let mut named = Values {
                month: Some(named_month.to_string()),
                ..Default::default()
            };
            for (index, field) in order.iter().filter(|field| **field != DateField::Month).enumerate() {
                named.set(*field, numbers.get(index).cloned());
            }
            if let Some(date) = date_of(&named, locale, reference_year) {
                return Some(date);
            }
        }
        return None;
    } else if numbers.len() == 1 {
        let digits = &numbers[0];
        if digits.len() != 8 && digits.len() != 6 {
            return None;
        }
        let year_length = digits.len() - 4;
        let mut offset = 0;
        for field in &fields {
            let length = if *field == DateField::Year { year_length } else { 2 };
            let end = (offset + length).min(digits.len());
            values.set(*field, Some(digits[offset.min(end)..end].to_string()));
            offset += length;
        }
    } else if numbers.len() == 2 || numbers.len() == 3 {
        // Zulu writes "2026-10-16" in numbers but "10/16/26" short, so a date
        // typed with the short date's separator reads in the short date's order.
        let short = short_pattern(locale);
        let typed = typed_separator(&normalized);
        let all = if typed.as_deref() == Some(short.separator.as_str())
            && short.separator != js_trim(&separator)
        {
            short.fields
        } else {
            fields
        };
        let order: Vec<DateField> = if numbers.len() == 3 {
            all
        } else {
            all.into_iter().filter(|field| *field != DateField::Year).collect()
        };
        for (index, field) in order.iter().enumerate() {
            values.set(*field, numbers.get(index).cloned());
        }
    } else {
        return None;
    }

    date_of(&values, locale, reference_year)
}

// The fields a typed date fills, each its digits, as the original keeps them in
// a `Partial<Record<DateField, string>>`.
#[derive(Clone, Debug, Default)]
struct Values {
    year: Option<String>,
    month: Option<String>,
    day: Option<String>,
}

impl Values {
    fn set(&mut self, field: DateField, value: Option<String>) {
        match field {
            DateField::Year => self.year = value,
            DateField::Month => self.month = value,
            DateField::Day => self.day = value,
        }
    }
}

fn date_of(values: &Values, locale: Option<&str>, reference_year: f64) -> Option<ISODate> {
    let offset = year_offset(locale);
    let year = match &values.year {
        None => reference_year,
        Some(year) => full_year(year, reference_year + offset) - offset,
    };
    let number = |value: &Option<String>| value.as_deref().map_or(f64::NAN, js_number);
    iso_date_of(year, number(&values.month), number(&values.day))
}

// `isoDate` with JavaScript's numbers, which are dates only when whole.
fn iso_date_of(year: f64, month: f64, day: f64) -> Option<ISODate> {
    let whole = |value: f64| {
        (value.fract() == 0.0 && value.abs() <= f64::from(i32::MAX)).then_some(value as i32)
    };
    iso_date(whole(year)?, whole(month)?, whole(day)?)
}

thread_local! {
    static YEAR_OFFSETS: RefCell<HashMap<String, f64>> = RefCell::default();
}

// How far the years a locale writes are from Gregorian years, like 543 for
// the Buddhist years of Thai. Japanese years restart with each era, so they read as Gregorian.
fn year_offset(locale: Option<&str>) -> f64 {
    let key = locale.unwrap_or("").to_string();
    if let Some(offset) = YEAR_OFFSETS.with(|offsets| offsets.borrow().get(&key).copied()) {
        return offset;
    }
    let offset = read_year_offset(locale);
    YEAR_OFFSETS.with(|offsets| offsets.borrow_mut().insert(key, offset));
    offset
}

fn read_year_offset(locale: Option<&str>) -> f64 {
    let formatter = date_formatter(locale, &numeric_date());
    let parts = formatter.format_to_parts(DateTimeValue::new(2026, 12, 31));
    let year = parts
        .iter()
        .find(|part| part.r#type == DateTimeFormatPartType::Year)
        .map_or(String::new(), |part| part.value.clone());
    let year = js_number(&to_latin_digits(&year, locale));
    if year == 0.0 || year.is_nan() || formatter.resolved_options().calendar == "japanese" {
        return 0.0;
    }
    year - 2026.0
}

fn full_year(text: &str, reference_year: f64) -> f64 {
    let year = js_number(text);
    if text.encode_utf16().count() > 2 {
        return year;
    }
    // The century that puts a two-digit year 80 years before to 20 after the reference.
    let century = ((reference_year + 20.0) / 100.0).floor() * 100.0;
    if century + year > reference_year + 20.0 {
        century - 100.0 + year
    } else {
        century + year
    }
}

struct FieldOrders {
    named: Vec<DateField>,
    short: DatePattern,
}

thread_local! {
    static FIELD_ORDERS: RefCell<HashMap<String, std::rc::Rc<FieldOrders>>> = RefCell::default();
}

fn field_orders_of(locale: Option<&str>) -> std::rc::Rc<FieldOrders> {
    let key = locale.unwrap_or("").to_string();
    if let Some(orders) = FIELD_ORDERS.with(|orders| orders.borrow().get(&key).cloned()) {
        return orders;
    }
    let fields_of = |options: &DateTimeFormatOptions| -> Vec<DateField> {
        date_formatter(locale, options)
            .format_to_parts(DateTimeValue::new(2026, 10, 16))
            .iter()
            .filter_map(|part| DateField::of(part.r#type))
            .collect()
    };
    let short_date = DateTimeFormatOptions::default().date_style(DateTimeStyle::Short);
    let short_parts = date_formatter(locale, &short_date).format_to_parts(DateTimeValue::new(2026, 10, 16));
    // `findIndex` is -1 without a field, so the part after it is the first.
    let first_field = short_parts
        .iter()
        .position(|part| DateField::of(part.r#type).is_some())
        .map_or(0, |index| index + 1);
    let orders = std::rc::Rc::new(FieldOrders {
        named: fields_of(
            &DateTimeFormatOptions::default()
                .year(NumericWidth::Numeric)
                .month(MonthWidth::Long)
                .day(NumericWidth::Numeric),
        ),
        short: DatePattern {
            fields: fields_of(&short_date),
            separator: short_parts
                .get(first_field)
                .map_or(String::new(), |part| js_trim(&without_direction_marks(&part.value)).to_string()),
            ..date_pattern(locale)
        },
    });
    FIELD_ORDERS.with(|cache| cache.borrow_mut().insert(key, orders.clone()));
    orders
}

// The order a date with a month name writes its numbers in, like Swedish
// "16 oktober 2026", though its numeric dates go year first.
fn named_order(locale: Option<&str>) -> Vec<DateField> {
    field_orders_of(locale).named.clone()
}

fn short_pattern(locale: Option<&str>) -> DatePattern {
    field_orders_of(locale).short.clone()
}

// `normalized.match(/\d(\D+)\d/)?.[1]?.trim()`: the first run of non-digits
// with a digit on each side.
fn typed_separator(text: &str) -> Option<String> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (index, (_, c)) in chars.iter().enumerate() {
        if !c.is_ascii_digit() {
            continue;
        }
        let run_start = index + 1;
        let mut run_end = run_start;
        while run_end < chars.len() && !chars[run_end].1.is_ascii_digit() {
            run_end += 1;
        }
        if run_end > run_start && run_end < chars.len() {
            let from = chars[run_start].0;
            let to = chars[run_end].0;
            return Some(js_trim(&text[from..to]).to_string());
        }
    }
    None
}

// Whether a character counts for one of the original's Unicode classes. Half
// of a character past the Basic Multilingual Plane, which is all JavaScript
// sees when it indexes a string, matches none.
fn in_class(c: char, class: impl Fn(char) -> bool) -> bool {
    (c as u32) <= 0xFFFF && class(c)
}

// Where a name stands in the text as whole words, or `None`.
fn word_index(text: &str, name: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(found) = text.get(from..).and_then(|rest| rest.find(name)) {
        let at = from + found;
        let after = text[at + name.len()..].chars().next();
        let before = text[..at].chars().next_back();
        let ends = !after.is_some_and(|c| in_class(c, |c| is_letter(c) || is_mark(c) || is_number(c)));
        let starts = !before.is_some_and(|c| in_class(c, |c| is_letter(c) || is_mark(c)));
        if ends && starts {
            return Some(at);
        }
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

// `text.match(/\p{N}+|[\p{L}\p{M}]+/gu)`: runs of numbers, and runs of letters
// and marks.
fn tokens(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index].1;
        let class: Option<fn(char) -> bool> = if is_number(c) {
            Some(is_number)
        } else if is_letter(c) || is_mark(c) {
            Some(|c| is_letter(c) || is_mark(c))
        } else {
            None
        };
        let Some(class) = class else {
            index += 1;
            continue;
        };
        let start = chars[index].0;
        while index < chars.len() && class(chars[index].1) {
            index += 1;
        }
        let end = chars.get(index).map_or(text.len(), |(at, _)| *at);
        tokens.push(&text[start..end]);
    }
    tokens
}

// `text.match(/[\p{L}\p{M}]+/gu)`: runs of letters and marks.
fn letter_runs(text: &str) -> Vec<&str> {
    tokens(text)
        .into_iter()
        .filter(|token| token.chars().next().is_some_and(|c| is_letter(c) || is_mark(c)))
        .collect()
}

// The month names a locale writes, in the order they were added, as a `Map` keeps them.
#[derive(Default)]
struct Lookup {
    entries: Vec<(String, u32)>,
    index: HashMap<String, usize>,
}

impl Lookup {
    fn set(&mut self, name: String, month: u32) {
        match self.index.get(&name) {
            Some(at) => self.entries[*at].1 = month,
            None => {
                self.index.insert(name.clone(), self.entries.len());
                self.entries.push((name, month));
            }
        }
    }

    fn get(&self, name: &str) -> Option<u32> {
        self.index.get(name).map(|at| self.entries[*at].1)
    }
}

struct Months {
    lookup: Lookup,
    spaced: Vec<(String, u32)>,
}

thread_local! {
    static MONTH_LOOKUPS: RefCell<HashMap<String, std::rc::Rc<Months>>> = RefCell::default();
}

fn months(locale: Option<&str>) -> std::rc::Rc<Months> {
    let key = locale.unwrap_or("").to_string();
    if let Some(found) = MONTH_LOOKUPS.with(|lookups| lookups.borrow().get(&key).cloned()) {
        return found;
    }
    let lookup = month_lookup(locale);
    let mut spaced: Vec<(String, u32)> = lookup
        .entries
        .iter()
        .filter(|(name, _)| {
            name.chars().any(is_letter) && name.chars().any(|c| !(is_letter(c) || is_mark(c)))
        })
        .cloned()
        .collect();
    // Longest first, by JavaScript's length; the sort is stable.
    spaced.sort_by_key(|(name, _)| std::cmp::Reverse(name.encode_utf16().count()));
    let found = std::rc::Rc::new(Months { lookup, spaced });
    MONTH_LOOKUPS.with(|lookups| lookups.borrow_mut().insert(key, found.clone()));
    found
}

// The month a word names, matching the locale's long and short names in both
// grammatical forms, and any unambiguous start of three letters or more. A
// word the locale writes onto the name, like Hebrew "ב" in "באוקטובר", counts.
fn month_from_name(word: &str, locale: Option<&str>) -> Option<u32> {
    let months = months(locale);
    let lookup = &months.lookup;
    if let Some(exact) = lookup.get(word) {
        return Some(exact);
    }
    let length = word.encode_utf16().count();
    // `for (let index = 1; index < word.length - 2; index++)`, at each place a
    // character starts.
    let mut unit = 0;
    for (at, c) in word.char_indices() {
        if unit >= 1
            && unit + 2 < length
            && let Some(month) = lookup.get(&word[at..])
            && is_pattern_word(&word[..at], locale)
        {
            return Some(month);
        }
        unit += c.len_utf16();
    }
    if length < 3 {
        return None;
    }
    let mut found = None;
    for (name, month) in &lookup.entries {
        if name.starts_with(word) {
            if found.is_some_and(|found| found != *month) {
                return None;
            }
            found = Some(*month);
        }
    }
    found
}

fn month_lookup(locale: Option<&str>) -> Lookup {
    let resolved = lowercase_locale(locale);
    let mut lookup = Lookup::default();
    let mut add = |name: &str, month: u32| {
        let lower = to_locale_lower_case(name, &resolved);
        let name = lower.strip_suffix('.').unwrap_or(&lower).to_string();
        lookup.set(name, month);
    };
    for width in [TextWidth::Long, TextWidth::Short] {
        for (index, name) in month_names(locale, width).iter().enumerate() {
            add(name, index as u32 + 1);
        }
        // Inside a date, some languages use another form: "16 января", not "январь".
        let in_date = date_formatter(
            locale,
            &DateTimeFormatOptions::default()
                .day(NumericWidth::Numeric)
                .month(MonthWidth::from(width)),
        );
        for index in 0..12 {
            let parts = in_date.format_to_parts(DateTimeValue::new(2026, index + 1, 16));
            if let Some(name) = parts.iter().find(|part| part.r#type == DateTimeFormatPartType::Month) {
                add(&name.value, index as u32 + 1);
            }
        }
    }
    lookup
}

thread_local! {
    static PATTERN_WORDS: RefCell<HashMap<String, std::rc::Rc<HashSet<String>>>> = RefCell::default();
}

// Words a typed or pasted date may carry besides the month: the ones a locale
// writes between a date's numbers, like Korean "년" or Spanish "de", and
// weekday names, as in "Fri, Oct 16, 2026".
fn is_pattern_word(word: &str, locale: Option<&str>) -> bool {
    let key = locale.unwrap_or("").to_string();
    if let Some(words) = PATTERN_WORDS.with(|words| words.borrow().get(&key).cloned()) {
        return words.contains(word);
    }
    let resolved = lowercase_locale(locale);
    let mut words = HashSet::new();
    for style in [
        DateTimeStyle::Short,
        DateTimeStyle::Medium,
        DateTimeStyle::Long,
        DateTimeStyle::Full,
    ] {
        let parts = date_formatter(locale, &DateTimeFormatOptions::default().date_style(style))
            .format_to_parts(DateTimeValue::new(2026, 10, 16));
        for part in parts {
            if part.r#type == DateTimeFormatPartType::Literal {
                let lower = to_locale_lower_case(&part.value, &resolved);
                for literal in letter_runs(&lower) {
                    words.insert(literal.to_string());
                }
            }
        }
    }
    for width in [TextWidth::Long, TextWidth::Short] {
        for name in weekday_names(locale, width, 0) {
            let lower = to_locale_lower_case(&name, &resolved);
            words.insert(lower.strip_suffix('.').unwrap_or(&lower).to_string());
        }
    }
    let found = words.contains(word);
    PATTERN_WORDS.with(|cache| cache.borrow_mut().insert(key, std::rc::Rc::new(words)));
    found
}

thread_local! {
    static DIGIT_MAPS: RefCell<HashMap<String, std::rc::Rc<HashMap<char, char>>>> = RefCell::default();
}

// The text with the locale's own digits, like Arabic-Indic "١٦", read as Latin ones.
fn to_latin_digits(text: &str, locale: Option<&str>) -> String {
    let key = locale.unwrap_or("").to_string();
    let digits = DIGIT_MAPS.with(|maps| maps.borrow().get(&key).cloned()).unwrap_or_else(|| {
        let native = native_digits(&numbers_locale(locale));
        let digits: HashMap<char, char> = native
            .chars()
            .enumerate()
            .map(|(index, digit)| (digit, char::from_digit(9_u32.saturating_sub(index as u32), 10).unwrap_or('0')))
            .collect();
        let digits = std::rc::Rc::new(digits);
        DIGIT_MAPS.with(|maps| maps.borrow_mut().insert(key, digits.clone()));
        digits
    });
    // Full-width digits, which East Asian input methods commit, fold to ASCII.
    text.chars()
        .map(|c| {
            if !is_decimal_digit(c) {
                return c.to_string();
            }
            match digits.get(&c) {
                Some(latin) => latin.to_string(),
                None => normalize_nfkc(&c.to_string()),
            }
        })
        .collect()
}

// The locale `toLocaleLowerCase` and `Intl.NumberFormat` read: the tag's, or
// the runtime's without one.
fn lowercase_locale(locale: Option<&str>) -> Locale {
    match locale {
        Some(tag) => parse_locale(tag)
            .unwrap_or_else(|| panic!("Incorrect locale information provided: \"{tag}\"")),
        None => default_locale(),
    }
}

fn numbers_locale(locale: Option<&str>) -> Locale {
    crate::intl::resolve_locale(locale)
}

fn this_year() -> i32 {
    jiff::ZonedDateTime::now().year().into()
}

/// The whitespace JavaScript's `trim` removes: Unicode's `White_Space` but the
/// next line character, and the byte order mark.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c == '\u{FEFF}' || (c.is_whitespace() && c != '\u{85}'))
}

/// `Number(text)`: the number a string reads as, `0` for blank text and `NaN`
/// for anything else that isn't one.
fn js_number(text: &str) -> f64 {
    let text = js_trim(text);
    if text.is_empty() {
        return 0.0;
    }
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let radix = match text.get(..2) {
        Some("0x" | "0X") => Some(16),
        Some("0o" | "0O") => Some(8),
        Some("0b" | "0B") => Some(2),
        _ => None,
    };
    if let Some(radix) = radix {
        let digits = &text[2..];
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return f64::NAN;
        }
        return digits
            .chars()
            .fold(0.0, |value, c| value * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0)));
    }
    // A decimal literal: a sign, digits with a point, and an exponent.
    let bytes = text.as_bytes();
    let mut at = usize::from(matches!(bytes[0], b'+' | b'-'));
    let digits = |from: usize| {
        let mut end = from;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        end
    };
    let integer = digits(at);
    let mut seen = integer > at;
    at = integer;
    if bytes.get(at) == Some(&b'.') {
        let fraction = digits(at + 1);
        seen |= fraction > at + 1;
        at = fraction;
    }
    if !seen {
        return f64::NAN;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        let mut exponent = at + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        let end = digits(exponent);
        if end == exponent {
            return f64::NAN;
        }
        at = end;
    }
    if at != bytes.len() {
        return f64::NAN;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}
