//! Port of `foundation/forms/files.ts`: file rules for FileInput: which
//! files to accept, and why the others were rejected. Pure, so they run on
//! any value with a file's name, type and size.

use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;

use fixed_decimal::{Decimal, SignedRoundingMode, UnsignedRoundingMode};
use gpui::SharedString;
use icu_decimal::DecimalFormatter;
use icu_locale_core::Locale;

/// What the file checks read: a picked file, or any value with its name,
/// type and size.
pub trait FileLike {
    /// The file name, with its extension.
    fn name(&self) -> &str;
    /// The MIME type, like `"image/png"`. Empty when the platform doesn't
    /// know it.
    fn mime_type(&self) -> &str;
    /// The size in bytes.
    fn size(&self) -> u64;
    /// Where it sat inside the folder it came from, for files from a dropped
    /// folder or a directory picker.
    fn relative_path(&self) -> Option<&str> {
        None
    }
}

/// A file a form submits: what a browser's `File` holds, and where it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FormFile {
    /// The file name, with its extension.
    pub name: SharedString,
    /// The MIME type, like `"image/png"`. Empty when unknown.
    pub mime_type: SharedString,
    /// The size in bytes.
    pub size: u64,
    /// Where it is on disk, for a file picked or dropped there.
    pub path: Option<PathBuf>,
    /// Where it sat inside the folder it came from.
    pub relative_path: Option<SharedString>,
}

impl FileLike for FormFile {
    fn name(&self) -> &str {
        &self.name
    }

    fn mime_type(&self) -> &str {
        &self.mime_type
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn relative_path(&self) -> Option<&str> {
        self.relative_path.as_deref()
    }
}

/// Why a file was rejected.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FileRejectionReason {
    /// It matches nothing in `accept`.
    FileType,
    /// It's bigger than `max_size`.
    FileTooLarge,
    /// It's smaller than `min_size`.
    FileTooSmall,
    /// More files came than `max_files` allows, or more than one without
    /// `multiple`.
    TooManyFiles,
    /// A reason `validate` gave, like `"duplicate"`.
    Custom(SharedString),
}

impl FileRejectionReason {
    /// The name the original reports, like `"file-too-large"`.
    pub fn as_str(&self) -> &str {
        match self {
            FileRejectionReason::FileType => "file-type",
            FileRejectionReason::FileTooLarge => "file-too-large",
            FileRejectionReason::FileTooSmall => "file-too-small",
            FileRejectionReason::TooManyFiles => "too-many-files",
            FileRejectionReason::Custom(reason) => reason,
        }
    }
}

/// A file that didn't pass the checks, with every reason it failed.
#[derive(Clone, Debug, PartialEq)]
pub struct FileRejection<F> {
    /// The file that failed. It never joins the value.
    pub file: F,
    /// Every reason, in the order they're checked: type, size, your
    /// `validate`, then the count.
    pub reasons: Vec<FileRejectionReason>,
}

/// The files to accept, like the `accept` attribute: MIME types
/// (`"image/png"`), wildcards (`"image/*"`) and extensions (`".pdf"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Accept {
    /// A comma-separated list, like `"image/*,.pdf"`.
    List(SharedString),
    /// One token each.
    Tokens(Vec<SharedString>),
}

impl From<&str> for Accept {
    fn from(list: &str) -> Self {
        Accept::List(SharedString::from(list.to_string()))
    }
}

impl From<SharedString> for Accept {
    fn from(list: SharedString) -> Self {
        Accept::List(list)
    }
}

impl From<Vec<SharedString>> for Accept {
    fn from(tokens: Vec<SharedString>) -> Self {
        Accept::Tokens(tokens)
    }
}

/// A check of your own: a reason to reject the file, or nothing to accept
/// it. The slice holds the files of the same choice, drop or paste accepted
/// before this one.
pub type FileValidator<F> = Rc<dyn Fn(&F, &[F]) -> Option<SharedString>>;

/// The checks a FileInput runs on every file it receives.
#[derive(Clone)]
pub struct FileRules<F> {
    /// The files to accept. Without it, any file.
    pub accept: Option<Accept>,
    /// Accepts more than one file at a time. With `Some(false)`, the first
    /// file is accepted and the rest are rejected.
    pub multiple: Option<bool>,
    /// The most files accepted at a time. The first ones that pass the other
    /// checks are accepted, and the rest are rejected.
    pub max_files: Option<usize>,
    /// The largest size accepted, in bytes.
    pub max_size: Option<u64>,
    /// The smallest size accepted, in bytes.
    pub min_size: Option<u64>,
    /// A check of your own, which runs only on files that passed the others.
    pub validate: Option<FileValidator<F>>,
}

impl<F> Default for FileRules<F> {
    fn default() -> Self {
        Self {
            accept: None,
            multiple: None,
            max_files: None,
            max_size: None,
            min_size: None,
            validate: None,
        }
    }
}

/// The tokens of an `accept` list: lowercase, trimmed, without empty ones.
pub fn parse_accept(accept: Option<&Accept>) -> Vec<String> {
    let tokens: Vec<&str> = match accept {
        None => return Vec::new(),
        Some(Accept::List(list)) => list.split(',').collect(),
        Some(Accept::Tokens(tokens)) => tokens.iter().map(|token| token.as_ref()).collect(),
    };
    tokens
        .into_iter()
        .map(|token| token.trim().to_lowercase())
        .filter(|token| !token.is_empty())
        .collect()
}

fn is_extension(token: &str) -> bool {
    token.starts_with('.')
}

fn is_any(token: &str) -> bool {
    token == "*" || token == "*/*"
}

// "text/csv; charset=utf-8" is a text/csv.
fn essence(mime: &str) -> String {
    mime.split(';').next().unwrap_or("").trim().to_lowercase()
}

fn matches_type(mime: &str, token: &str) -> bool {
    if is_any(token) {
        return true;
    }
    if mime.is_empty() {
        return false;
    }
    if let Some(prefix) = token.strip_suffix('*')
        && token.ends_with("/*")
    {
        return mime.starts_with(prefix);
    }
    mime == token
}

/// Whether a file matches an `accept` list. Extensions match the name, so a
/// `.csv` that Windows reports as `application/vnd.ms-excel`, or a file the
/// platform has no MIME type for, still matches `".csv"`.
pub fn accepts_file(file: &impl FileLike, tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return true;
    }
    let name = file.name().to_lowercase();
    let mime = essence(file.mime_type());
    tokens.iter().any(|token| {
        if is_extension(token) {
            name.ends_with(token.as_str())
        } else {
            matches_type(&mime, token)
        }
    })
}

// The MIME types common extensions arrive with, so a drag can be judged
// before names are readable. Windows reports CSV files as Excel files.
const EXTENSION_TYPES: &[(&str, &[&str])] = &[
    (".pdf", &["application/pdf"]),
    (".png", &["image/png"]),
    (".jpg", &["image/jpeg"]),
    (".jpeg", &["image/jpeg"]),
    (".gif", &["image/gif"]),
    (".webp", &["image/webp"]),
    (".svg", &["image/svg+xml"]),
    (".txt", &["text/plain"]),
    (".md", &["text/markdown", "text/x-markdown"]),
    (".csv", &["text/csv", "application/vnd.ms-excel"]),
    (".json", &["application/json"]),
    (".zip", &["application/zip", "application/x-zip-compressed"]),
    (".mp4", &["video/mp4"]),
    (".mp3", &["audio/mpeg"]),
];

fn extension_types(token: &str) -> Option<&'static [&'static str]> {
    EXTENSION_TYPES
        .iter()
        .find(|(extension, _)| *extension == token)
        .map(|(_, types)| *types)
}

/// Whether a dragged item's MIME type matches, before the drop. Names aren't
/// readable during a drag, so an extension matches through the types it
/// usually arrives with. It's `None` when only an extension we don't know
/// could tell, or when the platform reports no type. Callers treat that as
/// accepted.
pub fn accepts_type(mime: &str, tokens: &[String]) -> Option<bool> {
    if tokens.is_empty() {
        return Some(true);
    }
    let mime = essence(mime);
    if mime.is_empty() {
        return None;
    }
    let matches = tokens.iter().any(|token| {
        if is_extension(token) {
            extension_types(token).is_some_and(|types| types.contains(&mime.as_str()))
        } else {
            matches_type(&mime, token)
        }
    });
    if matches {
        return Some(true);
    }
    if tokens
        .iter()
        .any(|token| is_extension(token) && extension_types(token).is_none())
    {
        None
    } else {
        Some(false)
    }
}

/// How many files the rules accept at a time.
pub fn file_limit(multiple: Option<bool>, max_files: Option<usize>) -> usize {
    if multiple == Some(false) {
        return max_files.unwrap_or(1).min(1);
    }
    max_files.unwrap_or(usize::MAX)
}

/// The files `validate_files` accepted, and the ones it rejected.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedFiles<F> {
    pub accepted: Vec<F>,
    pub rejected: Vec<FileRejection<F>>,
}

/// Splits files into the accepted ones and the rejected ones, with every
/// reason each failed. Files keep their order. When there are too many, the
/// first ones that pass the other checks are accepted.
pub fn validate_files<F: FileLike + Clone>(files: &[F], rules: &FileRules<F>) -> ValidatedFiles<F> {
    let tokens = parse_accept(rules.accept.as_ref());
    let limit = file_limit(rules.multiple, rules.max_files);
    let mut accepted: Vec<F> = Vec::new();
    let mut rejected: Vec<FileRejection<F>> = Vec::new();
    for file in files {
        let mut reasons = Vec::new();
        if !accepts_file(file, &tokens) {
            reasons.push(FileRejectionReason::FileType);
        }
        if rules.max_size.is_some_and(|max| file.size() > max) {
            reasons.push(FileRejectionReason::FileTooLarge);
        }
        if rules.min_size.is_some_and(|min| file.size() < min) {
            reasons.push(FileRejectionReason::FileTooSmall);
        }
        // Your check runs only on files that passed ours, so it can be costly.
        if reasons.is_empty()
            && let Some(validate) = &rules.validate
            && let Some(custom) = validate(file, &accepted).filter(|custom| !custom.is_empty())
        {
            reasons.push(FileRejectionReason::Custom(custom));
        }
        if reasons.is_empty() && accepted.len() >= limit {
            reasons.push(FileRejectionReason::TooManyFiles);
        }
        if reasons.is_empty() {
            accepted.push(file.clone());
        } else {
            rejected.push(FileRejection {
                file: file.clone(),
                reasons,
            });
        }
    }
    ValidatedFiles { accepted, rejected }
}

/// What a drag looks like it will bring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DragStatus {
    Accept,
    Reject,
}

/// Whether dragged items look acceptable, from what a drag exposes: their
/// MIME types and their count. `Reject` when one has a type `accept` rules
/// out or there are too many; otherwise `Accept`, since names and sizes are
/// only known after the drop.
pub fn drag_status<F>(types: &[impl AsRef<str>], rules: &FileRules<F>) -> DragStatus {
    if types.len() > file_limit(rules.multiple, rules.max_files) {
        return DragStatus::Reject;
    }
    let tokens = parse_accept(rules.accept.as_ref());
    if types
        .iter()
        .any(|mime| accepts_type(mime.as_ref(), &tokens) == Some(false))
    {
        DragStatus::Reject
    } else {
        DragStatus::Accept
    }
}

// The units a size reads in, short, as CLDR writes them in English.
const SIZE_UNITS: [&str; 5] = ["byte", "kB", "MB", "GB", "TB"];

/// A file size for people, in the locale: 1536 bytes reads "1.5 kB". The
/// number follows the locale, or the system's without one. ICU4X has no
/// names for byte units, so they read as CLDR's English ones.
pub fn format_file_size(bytes: u64, locale: Option<&str>) -> String {
    let mut value = bytes as f64;
    let mut unit = 0;
    while value.round() >= 1000.0 && unit < SIZE_UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let fraction_digits = if value < 10.0 && unit > 0 { 1 } else { 0 };
    let number = format_number(value, fraction_digits, locale);
    if unit == 0 {
        // The short form of a byte is the word in the singular, like
        // "70 byte", so bytes read long.
        let word = if bytes == 1 { "byte" } else { "bytes" };
        format!("{number} {word}")
    } else {
        format!("{number} {}", SIZE_UNITS[unit])
    }
}

// `value` with at most `fraction_digits` digits after the point, rounded
// half away from zero like `Intl.NumberFormat`, in the locale's digits and
// separators.
fn format_number(value: f64, fraction_digits: i16, locale: Option<&str>) -> String {
    let Ok(mut decimal) = Decimal::from_str(&format!("{value}")) else {
        return format!("{value}");
    };
    decimal.round_with_mode(
        -fraction_digits,
        SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
    );
    decimal.absolute.trim_end();
    let locale = locale
        .and_then(|locale| Locale::from_str(locale).ok())
        .or_else(|| sys_locale::get_locale().and_then(|locale| Locale::from_str(&locale).ok()))
        .unwrap_or(Locale::UNKNOWN);
    match DecimalFormatter::try_new((&locale).into(), Default::default()) {
        Ok(formatter) => formatter.format_to_string(&decimal),
        Err(_) => decimal.to_string(),
    }
}

/// Records where a file sat inside a dropped folder, as folders are read.
pub fn set_relative_path(file: &mut FormFile, path: impl Into<SharedString>) {
    file.relative_path = Some(path.into());
}

/// Where a file sat inside the folder it came from, like
/// `"photos/2026/a.jpg"`, for files from a dropped folder or a directory
/// picker. Just the name otherwise.
pub fn relative_path(file: &impl FileLike) -> SharedString {
    SharedString::from(
        file.relative_path()
            .filter(|path| !path.is_empty())
            .unwrap_or(file.name())
            .to_string(),
    )
}
