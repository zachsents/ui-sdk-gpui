//! Port of `foundation/date/week.ts`.
//!
//! The first day of the week for a locale. Browsers give it through
//! `Intl.Locale#getWeekInfo`; ICU4X's week data is the same CLDR
//! `weekData`, read by the locale's region, or the region it most likely
//! means when it names none.

use std::cell::RefCell;
use std::collections::HashMap;

use icu_calendar::week::WeekInformation;
use icu_locale_core::extensions::unicode::key;

use crate::intl::{default_locale, parse_locale};

// The `-u-fw-` extension a tag can carry to pick the first day itself, like `en-US-u-fw-mon`.
const EXTENSION_DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

thread_local! {
    static CACHE: RefCell<HashMap<String, u32>> = RefCell::default();
}

/// The day a week starts on in a locale, 0 for Sunday to 6 for Saturday, as
/// `Date#getDay` counts: 0 in the United States, 1 in most of Europe, 6 in
/// much of the Middle East. A tag's `-u-fw-` extension wins. Without a
/// locale, the runtime's own. A tag that isn't well formed starts on Monday.
pub fn first_day_of_week(locale: Option<&str>) -> u32 {
    let key = locale.unwrap_or("").to_string();
    if let Some(day) = CACHE.with(|cache| cache.borrow().get(&key).copied()) {
        return day;
    }
    let day = resolve_first_day(locale);
    CACHE.with(|cache| cache.borrow_mut().insert(key, day));
    day
}

fn resolve_first_day(locale: Option<&str>) -> u32 {
    let tag = match locale {
        Some(locale) => match parse_locale(locale) {
            Some(tag) => tag,
            None => return 1,
        },
        None => default_locale(),
    };
    if let Some(extension) = tag.extensions.unicode.keywords.get(&key!("fw")) {
        let extension = extension.to_string().to_lowercase();
        if let Some(day) = EXTENSION_DAYS.iter().position(|day| *day == extension) {
            return day as u32;
        }
    }
    // ICU4X counts from 1 for Monday to 7 for Sunday, as `getWeekInfo` does.
    match WeekInformation::try_new((&tag).into()) {
        Ok(info) => info.first_weekday as u32 % 7,
        Err(_) => 1,
    }
}
