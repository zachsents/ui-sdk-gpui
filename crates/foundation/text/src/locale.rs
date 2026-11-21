//! Port of `foundation/text/locale.ts`.
//!
//! The page's and the user's language, in a form ICU4X accepts. Systems hand
//! out tags a strict parser rejects: `en_US` is common, a POSIX locale like
//! `de_DE.UTF-8@euro` carries an encoding and a modifier, and headless
//! Chromium on Linux reports `en-US@posix`.
//!
//! The original reads the nearest `lang` attribute, the document's, and the
//! browser's language. gpui has no elements to carry `lang` and no navigator,
//! so the page's language is the app's: [`set_page_locale`] sets it, like
//! `lang` on the page's root, and the user's language is the system's.

use std::sync::OnceLock;

use gpui::{App, Context, Global, SharedString, Subscription};
use icu_locale::LocaleCanonicalizer;
use icu_locale_core::Locale;

/// What a component formats in when nothing else says, so text never depends
/// on a missing language.
pub const DEFAULT_LOCALE: &str = "en-US";

/// A language tag ICU4X accepts, read from a BCP 47 or POSIX one (`en_US`), or
/// `None`: `to_locale(Some("en_US"))` is `"en-US"`, `to_locale(Some("??"))` is
/// `None`. Tags are canonicalized as `Intl.getCanonicalLocales` does, case and
/// aliases included, so `"iw"` reads as `"he"`.
pub fn to_locale(tag: Option<&str>) -> Option<SharedString> {
    let tag = tag.filter(|tag| !tag.is_empty())?;
    // A POSIX locale's encoding and modifier go, as in `de_DE.UTF-8@euro`.
    let tag = tag.split(['.', '@']).next().unwrap_or_default().replace('_', "-");
    let mut locale = Locale::try_from_str(&tag).ok()?;
    LocaleCanonicalizer::new_extended().canonicalize(&mut locale);
    Some(locale.to_string().into())
}

/// The user's language, which their typing is in: the system's first
/// preferred language, read once, as apps read it at launch. `None` when the
/// system gives none ICU4X accepts.
pub fn user_locale() -> Option<SharedString> {
    static USER: OnceLock<Option<SharedString>> = OnceLock::new();
    USER.get_or_init(|| to_locale(sys_locale::get_locale().as_deref()))
        .clone()
}

// The language the app set, `lang` on the page's root.
#[derive(Default)]
struct PageLocale(Option<SharedString>);

impl Global for PageLocale {}

/// Sets the language the app's text is in, like `lang` on the page's root:
/// numbers, dates and labels format in it unless a component's `locale`
/// option says otherwise. `None` goes back to the user's. A tag ICU4X rejects
/// counts as none, as a `lang` `Intl` rejects does. Redraws every window, so
/// what's shown follows.
pub fn set_page_locale(locale: Option<&str>, cx: &mut App) {
    let canonical = to_locale(locale);
    if cfg!(debug_assertions)
        && let Some(tag) = locale
        && canonical.is_none()
    {
        log::warn!("[ui-sdk] set_page_locale: {tag:?} isn't a language tag, so the user's language is used.");
    }
    cx.set_global(PageLocale(canonical));
    cx.refresh_windows();
}

/// The language of the app's text: what [`set_page_locale`] set, else the
/// user's. `None` only when neither is known.
///
/// The original reads the nearest `lang` around an element, so a section can
/// have its own. Here a component's `locale` option does that.
pub fn page_locale(cx: &App) -> Option<SharedString> {
    cx.try_global::<PageLocale>()
        .and_then(|page| page.0.clone())
        .or_else(user_locale)
}

/// Keeps a component's copy of the page's language current: calls `set`
/// whenever [`set_page_locale`] changes it. Start the copy with
/// [`page_locale`]. Parts that read [`locale_of`] as they render don't need
/// this; state that formats ahead of rendering does. Hold the subscription in
/// a field.
pub fn bind_page_locale<T: 'static>(
    cx: &mut Context<T>,
    set: impl Fn(&mut T, Option<SharedString>, &mut Context<T>) + 'static,
) -> Subscription {
    cx.observe_global::<PageLocale>(move |this, cx| {
        let locale = page_locale(cx);
        set(this, locale, cx);
    })
}

/// What a component formats in: its `locale` option, else the page's
/// language ([`page_locale`]), else English.
pub fn locale_of(locale: Option<&SharedString>, cx: &App) -> SharedString {
    match locale {
        Some(locale) => locale.clone(),
        None => page_locale(cx).unwrap_or_else(|| DEFAULT_LOCALE.into()),
    }
}

/// [`locale_of`] for state that keeps its own copy of the page's language,
/// like one [`bind_page_locale`] updates: the `locale` option, else that copy,
/// else English.
pub fn locale_from(locale: Option<&SharedString>, page_locale: Option<&SharedString>) -> SharedString {
    locale
        .or(page_locale)
        .cloned()
        .unwrap_or_else(|| DEFAULT_LOCALE.into())
}
