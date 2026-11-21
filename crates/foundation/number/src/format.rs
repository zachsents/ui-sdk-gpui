//! Port of `foundation/number/format.ts`.
//!
//! Showing numbers in someone's language, with formatters built once per
//! locale and options. Pure, so it runs anywhere, like on a background thread.
//!
//! The original formats through `Intl.NumberFormat`. [`NumberFormatter`] is a
//! port of it: ECMA-402's options, rounding, signs and parts, on ICU4X's
//! compiled CLDR data, laid out the way ICU lays them out in a browser. Where
//! ICU4X has no data, it says so beside the fallback: the exponent symbol, the
//! range separator, NaN's name, and the units outside area, duration, length,
//! mass and volume.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::OnceLock;

use fixed_decimal::{RoundingIncrement, UnsignedDecimal, UnsignedRoundingMode};
use gpui::SharedString;
use icu_decimal::DecimalFormatterPreferences;
use icu_decimal::provider::{
    CompactPatterns, DecimalCompactLongV1, DecimalCompactShortV1, DecimalDigitsV1, DecimalSymbols,
    DecimalSymbolsV1,
};
use icu_experimental::dimension::currency::CurrencyType;
use icu_experimental::dimension::provider::currency::essentials::CurrencyEssentialsV1;
use icu_experimental::dimension::provider::currency::extended::CurrencyExtendedDataV1;
use icu_experimental::dimension::provider::currency::fractions::CurrencyFractionsV1;
use icu_experimental::dimension::provider::currency::no_currency::CurrencyPatternsNoCurrencyV1;
use icu_experimental::dimension::provider::currency::patterns::CurrencyPatternsDataV1;
use icu_experimental::dimension::provider::currency::symbols::{
    CurrencyDecimalSymbolsV1, CurrencySymbolsV1,
};
use icu_experimental::dimension::provider::percent::DecimalPercentV1;
use icu_experimental::dimension::provider::units::categorized_display_names::{
    UnitsNamesAreaCoreV1, UnitsNamesAreaExtendedV1, UnitsNamesAreaOutlierV1,
    UnitsNamesDurationCoreV1, UnitsNamesDurationExtendedV1, UnitsNamesDurationOutlierV1,
    UnitsNamesLengthCoreV1, UnitsNamesLengthExtendedV1, UnitsNamesLengthOutlierV1,
    UnitsNamesMassCoreV1, UnitsNamesMassExtendedV1, UnitsNamesMassOutlierV1,
    UnitsNamesVolumeCoreV1, UnitsNamesVolumeExtendedV1, UnitsNamesVolumeOutlierV1,
};
use icu_experimental::dimension::provider::units::display_names::UnitsDisplayNames;
use icu_experimental::dimension::provider::units::essentials::UnitsEssentialsV1;
use icu_locale_core::Locale;
use icu_locale_core::extensions::unicode::key;
use icu_locale_core::preferences::LocalePreferences;
use icu_locale_core::subtags::Subtag;
use icu_pattern::{
    DoublePlaceholderKey, DoublePlaceholderPattern, PatternItem, SinglePlaceholder,
    SinglePlaceholderPattern,
};
use icu_plurals::provider::PluralElementsPackedCow;
use icu_plurals::{PluralOperands, PluralRules, PluralRulesPreferences};
use icu_provider::marker::ErasedMarker;
use icu_provider::prelude::*;
use zerovec::ule::VarULE;

use crate::steps::decimals_of;

/// How values show: `Intl.NumberFormat` options, or a function of the value.
#[derive(Clone)]
pub enum NumberFormat {
    /// Options, as `Intl.NumberFormat` takes them.
    Options(NumberFormatOptions),
    /// Text of your own, like "3 of 5 stars".
    Function(Rc<dyn Fn(f64) -> String>),
}

impl NumberFormat {
    /// Text of your own for each value.
    pub fn function(format: impl Fn(f64) -> String + 'static) -> Self {
        NumberFormat::Function(Rc::new(format))
    }

    /// The options, unless it's a function.
    pub fn options(&self) -> Option<&NumberFormatOptions> {
        match self {
            NumberFormat::Options(options) => Some(options),
            NumberFormat::Function(_) => None,
        }
    }
}

impl Default for NumberFormat {
    fn default() -> Self {
        NumberFormat::Options(NumberFormatOptions::default())
    }
}

impl From<NumberFormatOptions> for NumberFormat {
    fn from(options: NumberFormatOptions) -> Self {
        NumberFormat::Options(options)
    }
}

impl fmt::Debug for NumberFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumberFormat::Options(options) => f.debug_tuple("Options").field(options).finish(),
            NumberFormat::Function(_) => f.write_str("Function"),
        }
    }
}

impl PartialEq for NumberFormat {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (NumberFormat::Options(a), NumberFormat::Options(b)) => a == b,
            (NumberFormat::Function(a), NumberFormat::Function(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// What a number is: a plain number, a share shown as a percent, an amount of
/// money, or a measure in a unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NumberStyle {
    #[default]
    Decimal,
    /// Formats 0.5 as 50%.
    Percent,
    /// Needs a `currency`.
    Currency,
    /// Needs a `unit`.
    Unit,
}

impl NumberStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            NumberStyle::Decimal => "decimal",
            NumberStyle::Percent => "percent",
            NumberStyle::Currency => "currency",
            NumberStyle::Unit => "unit",
        }
    }
}

/// How a currency shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CurrencyDisplay {
    /// The locale's symbol, like "€" or "US$".
    #[default]
    Symbol,
    /// The shortest symbol, like "$".
    NarrowSymbol,
    /// The ISO code, like "USD".
    Code,
    /// The name, like "US dollars".
    Name,
}

impl CurrencyDisplay {
    pub fn as_str(self) -> &'static str {
        match self {
            CurrencyDisplay::Symbol => "symbol",
            CurrencyDisplay::NarrowSymbol => "narrowSymbol",
            CurrencyDisplay::Code => "code",
            CurrencyDisplay::Name => "name",
        }
    }
}

/// How negative amounts of money show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CurrencySign {
    /// With a minus sign, like "-$5.00".
    #[default]
    Standard,
    /// The way the locale's accountants write them, like "($5.00)".
    Accounting,
}

impl CurrencySign {
    pub fn as_str(self) -> &'static str {
        match self {
            CurrencySign::Standard => "standard",
            CurrencySign::Accounting => "accounting",
        }
    }
}

/// How a unit shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UnitDisplay {
    /// Like "5 GB".
    #[default]
    Short,
    /// Like "5GB".
    Narrow,
    /// Like "5 gigabytes".
    Long,
}

impl UnitDisplay {
    pub fn as_str(self) -> &'static str {
        match self {
            UnitDisplay::Short => "short",
            UnitDisplay::Narrow => "narrow",
            UnitDisplay::Long => "long",
        }
    }
}

/// How a number's size shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Notation {
    /// Every digit, like "1,234,567".
    #[default]
    Standard,
    /// A power of ten, like "1.235E6".
    Scientific,
    /// A power of ten that's a multiple of three, like "1.235E6".
    Engineering,
    /// The locale's short words for powers of ten, like "1.2M".
    Compact,
}

impl Notation {
    pub fn as_str(self) -> &'static str {
        match self {
            Notation::Standard => "standard",
            Notation::Scientific => "scientific",
            Notation::Engineering => "engineering",
            Notation::Compact => "compact",
        }
    }
}

/// How compact notation writes its powers of ten.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CompactDisplay {
    /// Like "1.2K".
    #[default]
    Short,
    /// Like "1.2 thousand".
    Long,
}

impl CompactDisplay {
    pub fn as_str(self) -> &'static str {
        match self {
            CompactDisplay::Short => "short",
            CompactDisplay::Long => "long",
        }
    }
}

/// When a sign shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SignDisplay {
    /// For negative numbers, negative zero included.
    #[default]
    Auto,
    /// Never.
    Never,
    /// Always, zero included.
    Always,
    /// For positive and negative numbers, not zero.
    ExceptZero,
    /// For negative numbers, not negative zero.
    Negative,
}

impl SignDisplay {
    pub fn as_str(self) -> &'static str {
        match self {
            SignDisplay::Auto => "auto",
            SignDisplay::Never => "never",
            SignDisplay::Always => "always",
            SignDisplay::ExceptZero => "exceptZero",
            SignDisplay::Negative => "negative",
        }
    }
}

/// When digits are grouped, like the thousands in "1,234".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UseGrouping {
    /// Even where the locale waits for more digits.
    Always,
    /// As the locale groups.
    #[default]
    Auto,
    /// Only with at least two digits in the first group, like "12,345" and "1234".
    Min2,
    /// Never, `useGrouping: false`.
    Never,
}

impl UseGrouping {
    /// The option's value; `false` for [`UseGrouping::Never`].
    pub fn as_str(self) -> &'static str {
        match self {
            UseGrouping::Always => "always",
            UseGrouping::Auto => "auto",
            UseGrouping::Min2 => "min2",
            UseGrouping::Never => "false",
        }
    }
}

/// Which way a value rounds when it has more digits than it shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RoundingMode {
    /// Toward +∞.
    Ceil,
    /// Toward -∞.
    Floor,
    /// Away from zero.
    Expand,
    /// Toward zero.
    Trunc,
    /// To the nearest, halves toward +∞.
    HalfCeil,
    /// To the nearest, halves toward -∞.
    HalfFloor,
    /// To the nearest, halves away from zero.
    #[default]
    HalfExpand,
    /// To the nearest, halves toward zero.
    HalfTrunc,
    /// To the nearest, halves to an even digit.
    HalfEven,
}

impl RoundingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RoundingMode::Ceil => "ceil",
            RoundingMode::Floor => "floor",
            RoundingMode::Expand => "expand",
            RoundingMode::Trunc => "trunc",
            RoundingMode::HalfCeil => "halfCeil",
            RoundingMode::HalfFloor => "halfFloor",
            RoundingMode::HalfExpand => "halfExpand",
            RoundingMode::HalfTrunc => "halfTrunc",
            RoundingMode::HalfEven => "halfEven",
        }
    }
}

/// Which digit options win when both fraction and significant digits are set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RoundingPriority {
    /// Significant digits when they're set, else fraction digits.
    #[default]
    Auto,
    /// The one that keeps more digits.
    MorePrecision,
    /// The one that keeps fewer.
    LessPrecision,
}

impl RoundingPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            RoundingPriority::Auto => "auto",
            RoundingPriority::MorePrecision => "morePrecision",
            RoundingPriority::LessPrecision => "lessPrecision",
        }
    }
}

/// Whether a whole number keeps the decimals the minimum asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TrailingZeroDisplay {
    /// It does, like "$5.00".
    #[default]
    Auto,
    /// It doesn't, like "$5" beside "$5.50".
    StripIfInteger,
}

impl TrailingZeroDisplay {
    pub fn as_str(self) -> &'static str {
        match self {
            TrailingZeroDisplay::Auto => "auto",
            TrailingZeroDisplay::StripIfInteger => "stripIfInteger",
        }
    }
}

/// Every option `Intl.NumberFormat` takes. Unset ones take Intl's defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct NumberFormatOptions {
    /// The digits to write in, like `"arab"` or `"latn"`. The locale's own by default.
    pub numbering_system: Option<SharedString>,
    /// Decimal by default.
    pub style: Option<NumberStyle>,
    /// The ISO 4217 code a currency style shows, like `"EUR"`.
    pub currency: Option<SharedString>,
    /// The symbol by default.
    pub currency_display: Option<CurrencyDisplay>,
    /// A minus sign by default.
    pub currency_sign: Option<CurrencySign>,
    /// The unit a unit style shows, like `"gigabyte"` or `"kilometer-per-hour"`.
    pub unit: Option<SharedString>,
    /// Short by default.
    pub unit_display: Option<UnitDisplay>,
    /// Standard by default.
    pub notation: Option<Notation>,
    /// Short by default.
    pub compact_display: Option<CompactDisplay>,
    /// Auto by default.
    pub sign_display: Option<SignDisplay>,
    /// Auto by default, and min2 for compact notation.
    pub use_grouping: Option<UseGrouping>,
    /// From 1 to 21; 1 by default.
    pub minimum_integer_digits: Option<u8>,
    /// From 0 to 100; 0 by default, and the currency's digits for money.
    pub minimum_fraction_digits: Option<u8>,
    /// From 0 to 100; 3 by default, 0 for percents and the currency's digits for money.
    pub maximum_fraction_digits: Option<u8>,
    /// From 1 to 21.
    pub minimum_significant_digits: Option<u8>,
    /// From 1 to 21.
    pub maximum_significant_digits: Option<u8>,
    /// Auto by default.
    pub rounding_priority: Option<RoundingPriority>,
    /// Rounds to multiples of it in the last fraction digit: 1, 2, 5, 10, 20,
    /// 25, 50, 100, 200, 250, 500, 1000, 2000, 2500 or 5000. Needs fraction
    /// digits rounding, with the same minimum and maximum.
    pub rounding_increment: Option<u16>,
    /// Half expand by default.
    pub rounding_mode: Option<RoundingMode>,
    /// Auto by default.
    pub trailing_zero_display: Option<TrailingZeroDisplay>,
}

impl NumberFormatOptions {
    pub fn numbering_system(mut self, numbering_system: impl Into<SharedString>) -> Self {
        self.numbering_system = Some(numbering_system.into());
        self
    }

    pub fn style(mut self, style: NumberStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub fn currency(mut self, currency: impl Into<SharedString>) -> Self {
        self.currency = Some(currency.into());
        self
    }

    pub fn currency_display(mut self, currency_display: CurrencyDisplay) -> Self {
        self.currency_display = Some(currency_display);
        self
    }

    pub fn currency_sign(mut self, currency_sign: CurrencySign) -> Self {
        self.currency_sign = Some(currency_sign);
        self
    }

    pub fn unit(mut self, unit: impl Into<SharedString>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    pub fn unit_display(mut self, unit_display: UnitDisplay) -> Self {
        self.unit_display = Some(unit_display);
        self
    }

    pub fn notation(mut self, notation: Notation) -> Self {
        self.notation = Some(notation);
        self
    }

    pub fn compact_display(mut self, compact_display: CompactDisplay) -> Self {
        self.compact_display = Some(compact_display);
        self
    }

    pub fn sign_display(mut self, sign_display: SignDisplay) -> Self {
        self.sign_display = Some(sign_display);
        self
    }

    pub fn use_grouping(mut self, use_grouping: UseGrouping) -> Self {
        self.use_grouping = Some(use_grouping);
        self
    }

    pub fn minimum_integer_digits(mut self, digits: u8) -> Self {
        self.minimum_integer_digits = Some(digits);
        self
    }

    pub fn minimum_fraction_digits(mut self, digits: u8) -> Self {
        self.minimum_fraction_digits = Some(digits);
        self
    }

    pub fn maximum_fraction_digits(mut self, digits: u8) -> Self {
        self.maximum_fraction_digits = Some(digits);
        self
    }

    pub fn minimum_significant_digits(mut self, digits: u8) -> Self {
        self.minimum_significant_digits = Some(digits);
        self
    }

    pub fn maximum_significant_digits(mut self, digits: u8) -> Self {
        self.maximum_significant_digits = Some(digits);
        self
    }

    pub fn rounding_priority(mut self, rounding_priority: RoundingPriority) -> Self {
        self.rounding_priority = Some(rounding_priority);
        self
    }

    pub fn rounding_increment(mut self, rounding_increment: u16) -> Self {
        self.rounding_increment = Some(rounding_increment);
        self
    }

    pub fn rounding_mode(mut self, rounding_mode: RoundingMode) -> Self {
        self.rounding_mode = Some(rounding_mode);
        self
    }

    pub fn trailing_zero_display(mut self, trailing_zero_display: TrailingZeroDisplay) -> Self {
        self.trailing_zero_display = Some(trailing_zero_display);
        self
    }
}

/// What a part of a formatted number is, as `formatToParts` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NumberPartType {
    Integer,
    Group,
    Decimal,
    Fraction,
    MinusSign,
    PlusSign,
    PercentSign,
    Currency,
    Unit,
    Literal,
    Compact,
    ExponentSeparator,
    ExponentMinusSign,
    ExponentInteger,
    Nan,
    Infinity,
    ApproximatelySign,
}

impl NumberPartType {
    /// Intl's name for it, like `"minusSign"`.
    pub fn as_str(self) -> &'static str {
        match self {
            NumberPartType::Integer => "integer",
            NumberPartType::Group => "group",
            NumberPartType::Decimal => "decimal",
            NumberPartType::Fraction => "fraction",
            NumberPartType::MinusSign => "minusSign",
            NumberPartType::PlusSign => "plusSign",
            NumberPartType::PercentSign => "percentSign",
            NumberPartType::Currency => "currency",
            NumberPartType::Unit => "unit",
            NumberPartType::Literal => "literal",
            NumberPartType::Compact => "compact",
            NumberPartType::ExponentSeparator => "exponentSeparator",
            NumberPartType::ExponentMinusSign => "exponentMinusSign",
            NumberPartType::ExponentInteger => "exponentInteger",
            NumberPartType::Nan => "nan",
            NumberPartType::Infinity => "infinity",
            NumberPartType::ApproximatelySign => "approximatelySign",
        }
    }
}

/// A piece of a formatted number, like a digit group, the decimal separator or
/// the currency sign (`Intl.NumberFormatPart`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumberFormatPart {
    pub kind: NumberPartType,
    pub value: String,
}

/// The options a formatter settled on (`resolvedOptions()`). Fraction and
/// significant digits are there when its rounding reads them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedNumberFormatOptions {
    pub locale: SharedString,
    pub numbering_system: SharedString,
    pub style: NumberStyle,
    pub currency: Option<SharedString>,
    pub currency_display: Option<CurrencyDisplay>,
    pub currency_sign: Option<CurrencySign>,
    pub unit: Option<SharedString>,
    pub unit_display: Option<UnitDisplay>,
    pub minimum_integer_digits: u8,
    pub minimum_fraction_digits: Option<u8>,
    pub maximum_fraction_digits: Option<u8>,
    pub minimum_significant_digits: Option<u8>,
    pub maximum_significant_digits: Option<u8>,
    pub use_grouping: UseGrouping,
    pub notation: Notation,
    pub compact_display: Option<CompactDisplay>,
    pub sign_display: SignDisplay,
    pub rounding_increment: u16,
    pub rounding_mode: RoundingMode,
    pub rounding_priority: RoundingPriority,
    pub trailing_zero_display: TrailingZeroDisplay,
}

/// Compiled CLDR data for the patterns `icu_experimental` covers: percents,
/// currencies and units. Only these markers are built in, not the crate's own
/// compiled data, so nothing else of its 29 MB compiles into the app.
#[derive(Debug)]
pub(crate) struct Baked;

#[allow(unused_imports)]
const _: () = {
    use icu_experimental_data::*;
    pub mod icu {
        pub use icu_collections as collections;
        pub use icu_decimal as decimal;
        pub use icu_experimental as experimental;
        pub use icu_plurals as plurals;
    }
    make_provider!(Baked);
    impl_decimal_percent_v1!(Baked);
    impl_currency_essentials_v1!(Baked);
    impl_currency_symbols_v1!(Baked);
    impl_currency_decimal_symbols_v1!(Baked);
    impl_currency_extended_data_v1!(Baked);
    impl_currency_patterns_data_v1!(Baked);
    impl_currency_fractions_v1!(Baked);
    impl_currency_patterns_no_currency_v1!(Baked);
    impl_units_names_area_core_v1!(Baked);
    impl_units_names_area_extended_v1!(Baked);
    impl_units_names_area_outlier_v1!(Baked);
    impl_units_names_duration_core_v1!(Baked);
    impl_units_names_duration_extended_v1!(Baked);
    impl_units_names_duration_outlier_v1!(Baked);
    impl_units_names_length_core_v1!(Baked);
    impl_units_names_length_extended_v1!(Baked);
    impl_units_names_length_outlier_v1!(Baked);
    impl_units_names_mass_core_v1!(Baked);
    impl_units_names_mass_extended_v1!(Baked);
    impl_units_names_mass_outlier_v1!(Baked);
    impl_units_names_volume_core_v1!(Baked);
    impl_units_names_volume_extended_v1!(Baked);
    impl_units_names_volume_outlier_v1!(Baked);
    impl_units_essentials_v1!(Baked);
};

thread_local! {
    static FORMATTERS: RefCell<HashMap<(Option<String>, NumberFormatOptions), Rc<NumberFormatter>>> =
        RefCell::new(HashMap::new());
}

/// The formatter for a locale and options, built once and reused, since
/// formatters are slow to build. `None` is the system's language.
pub fn number_formatter(locale: Option<&str>, options: &NumberFormatOptions) -> Rc<NumberFormatter> {
    let key = (locale.map(str::to_owned), options.clone());
    if let Some(formatter) = FORMATTERS.with(|formatters| formatters.borrow().get(&key).cloned()) {
        return formatter;
    }
    let formatter = Rc::new(NumberFormatter::new(locale, options));
    FORMATTERS.with(|formatters| formatters.borrow_mut().insert(key, formatter.clone()));
    formatter
}

// A formatter keeps its resolved options, so its default decimals are read
// once, when it's built.
fn intl_decimals(format: Option<&NumberFormatOptions>) -> i32 {
    let default = NumberFormatOptions::default();
    let formatter = number_formatter(None, format.unwrap_or(&default));
    i32::from(formatter.resolved_options().maximum_fraction_digits.unwrap_or(0))
}

/// Intl shows at most three decimals by default, which would show 0.0001 as
/// 0. Without your own maximum, a value shows its step's decimals, and never
/// fewer than Intl would, so yen stay whole and dollars show cents.
pub fn display_options(format: Option<&NumberFormatOptions>, step: f64) -> NumberFormatOptions {
    let mut options = format.cloned().unwrap_or_default();
    if options.maximum_fraction_digits.is_none() && options.maximum_significant_digits.is_none() {
        let percent = options.style == Some(NumberStyle::Percent);
        let decimals = decimals_of(step) + if percent { -2 } else { 0 };
        let maximum = i32::from(options.minimum_fraction_digits.unwrap_or(0))
            .max(decimals)
            .max(intl_decimals(format));
        options.maximum_fraction_digits = Some(maximum.clamp(0, 100) as u8);
    }
    options
}

/// A value as people read it: the function's text, or the formatter's in the
/// locale, like "$1,234.50", "45%" or "1.2K". Empty for `None` and values that
/// aren't finite.
pub fn format_number(value: Option<f64>, locale: Option<&str>, format: Option<&NumberFormat>) -> String {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return String::new();
    };
    let default = NumberFormatOptions::default();
    let options = match format {
        Some(NumberFormat::Function(format)) => return format(value),
        Some(NumberFormat::Options(options)) => options,
        None => &default,
    };
    let formatter = number_formatter(locale, options);
    let text = formatter.format(value);
    // Some locales write a small number as a word alone and drop its sign, like
    // Arabic "يومان" for 2 and -2 days, so the locale's minus sign goes first.
    if value < 0.0
        && !formatter
            .format_to_parts(value)
            .iter()
            .any(|part| part.kind == NumberPartType::Integer)
    {
        let parts = formatter.format_to_parts(-3.0);
        let sign = parts
            .iter()
            .position(|part| part.kind == NumberPartType::MinusSign);
        let prefix: String = parts
            .iter()
            .take(sign.map_or(0, |sign| sign + 1))
            .map(|part| part.value.as_str())
            .collect();
        return format!("{prefix}{text}");
    }
    text
}

/// A range as people read it in a locale, like "$5 – $10" or "5–10%", with what
/// both ends share written once. A format function joins its two texts with a dash.
pub fn format_number_range(
    start: f64,
    end: f64,
    locale: Option<&str>,
    format: Option<&NumberFormat>,
) -> String {
    if !start.is_finite() || !end.is_finite() {
        return String::new();
    }
    let default = NumberFormatOptions::default();
    let options = match format {
        Some(NumberFormat::Function(format)) => return format!("{} – {}", format(start), format(end)),
        Some(NumberFormat::Options(options)) => options,
        None => &default,
    };
    let formatter = number_formatter(locale, options);
    // Both ends negative share one minus sign in `formatRange`, so -5% to -2%
    // would read "-5–2%", like -5% to 2%.
    if start < 0.0 && end < 0.0 {
        return format!("{} – {}", formatter.format(start), formatter.format(end));
    }
    formatter.format_range(start, end)
}

/// The parts a value formats into, like each digit group, the decimal
/// separator and the currency sign, for styling them apart or rolling digits
/// like an odometer. A format function's text is one literal part.
pub fn format_number_to_parts(
    value: Option<f64>,
    locale: Option<&str>,
    format: Option<&NumberFormat>,
) -> Vec<NumberFormatPart> {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return Vec::new();
    };
    let default = NumberFormatOptions::default();
    match format {
        Some(NumberFormat::Function(format)) => vec![NumberFormatPart {
            kind: NumberPartType::Literal,
            value: format(value),
        }],
        Some(NumberFormat::Options(options)) => number_formatter(locale, options).format_to_parts(value),
        None => number_formatter(locale, &default).format_to_parts(value),
    }
}

/// A port of `Intl.NumberFormat`: formats numbers in a locale, with options.
/// Build it through [`number_formatter`], which keeps one per locale and options.
pub struct NumberFormatter {
    resolved: ResolvedNumberFormatOptions,
    precision: Precision,
    notation: Notation,
    grouping: UseGrouping,
    sign_display: SignDisplay,
    // Percents format 0.5 as 50%.
    percent: bool,
    symbols: Symbols,
    numerals: [char; 10],
    plurals: Option<PluralRules>,
    compact: Option<DataPayload<ErasedMarker<CompactPatterns<'static, SinglePlaceholder>>>>,
    percent_data: Option<DataPayload<DecimalPercentV1>>,
    style: StyleData,
}

impl fmt::Debug for NumberFormatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NumberFormatter")
            .field("resolved", &self.resolved)
            .finish_non_exhaustive()
    }
}

impl NumberFormatter {
    /// A formatter for a locale and options, like `new Intl.NumberFormat`.
    /// `None` is the system's language. Where Intl throws on options, like a
    /// currency style without a currency, it falls back and warns in debug builds.
    pub fn new(locale: Option<&str>, options: &NumberFormatOptions) -> Self {
        let locale = resolve_locale(locale);
        let preferences = DecimalFormatterPreferences::from(&locale).locale_preferences;
        let numbering = numbering_of(&locale, options);
        let notation = options.notation.unwrap_or_default();

        let mut style = options.style.unwrap_or_default();
        let currency = if style == NumberStyle::Currency {
            let currency = options
                .currency
                .as_deref()
                .and_then(|code| CurrencyType::try_from_str(code).ok());
            if currency.is_none() {
                warn("A currency style needs a currency, an ISO 4217 code like \"EUR\".");
                style = NumberStyle::Decimal;
            }
            currency
        } else {
            None
        };
        let unit = if style == NumberStyle::Unit {
            let unit = options.unit.clone().filter(|unit| !unit.is_empty());
            if unit.is_none() {
                warn("A unit style needs a unit, like \"gigabyte\".");
                style = NumberStyle::Decimal;
            }
            unit
        } else {
            None
        };
        let currency_display = options.currency_display.unwrap_or_default();
        let currency_sign = options.currency_sign.unwrap_or_default();
        let unit_display = options.unit_display.unwrap_or_default();
        let compact_display = options.compact_display.unwrap_or_default();

        let (minimum_default, maximum_default) = match currency {
            Some(currency) if notation == Notation::Standard => {
                let digits = currency_digits(currency);
                (digits, digits)
            }
            _ if style == NumberStyle::Percent => (0, 0),
            _ => (0, 3),
        };
        let digits = resolve_digits(options, notation, minimum_default, maximum_default);
        let use_grouping = options.use_grouping.unwrap_or(if notation == Notation::Compact {
            UseGrouping::Min2
        } else {
            UseGrouping::Auto
        });
        let sign_display = options.sign_display.unwrap_or_default();

        let symbols = decimal_symbols(
            &DecimalSymbolsV1::make_locale(preferences),
            numbering.as_deref(),
            currency,
        );
        let (numerals, numbering_system) = numerals_of(numbering.as_deref(), &symbols.numbering);
        let plurals = PluralRules::try_new_cardinal(PluralRulesPreferences::from(&locale)).ok();
        let compact = if notation == Notation::Compact {
            match compact_display {
                CompactDisplay::Short => load_numbered::<DecimalCompactShortV1>(
                    &icu_decimal::provider::Baked,
                    &DecimalCompactShortV1::make_locale(preferences),
                    numbering.as_deref(),
                )
                .map(DataPayload::cast),
                CompactDisplay::Long => load_numbered::<DecimalCompactLongV1>(
                    &icu_decimal::provider::Baked,
                    &DecimalCompactLongV1::make_locale(preferences),
                    numbering.as_deref(),
                )
                .map(DataPayload::cast),
            }
        } else {
            None
        };
        let percent_data = load::<DecimalPercentV1>(
            &Baked,
            &DecimalPercentV1::make_locale(preferences),
            "",
        );
        let style_data = match (style, currency, &unit) {
            (NumberStyle::Currency, Some(currency), _) => StyleData::Currency(currency_data(
                preferences,
                numbering.as_deref(),
                currency,
                currency_display,
                currency_sign,
            )),
            (NumberStyle::Unit, _, Some(unit)) => {
                StyleData::Unit(unit_data(preferences, unit, unit_display, plurals.as_ref()))
            }
            (NumberStyle::Percent, _, _) => StyleData::Percent,
            _ => StyleData::Decimal,
        };

        let resolved = ResolvedNumberFormatOptions {
            locale: locale.to_string().into(),
            numbering_system: numbering_system.into(),
            style,
            currency: currency.map(|currency| currency.iso_code().as_str().to_owned().into()),
            currency_display: currency.map(|_| currency_display),
            currency_sign: currency.map(|_| currency_sign),
            unit: unit.clone(),
            unit_display: unit.as_ref().map(|_| unit_display),
            minimum_integer_digits: digits.minimum_integer,
            minimum_fraction_digits: digits.fraction.map(|(minimum, _)| minimum),
            maximum_fraction_digits: digits.fraction.map(|(_, maximum)| maximum),
            minimum_significant_digits: digits.significant.map(|(minimum, _)| minimum),
            maximum_significant_digits: digits.significant.map(|(_, maximum)| maximum),
            use_grouping,
            notation,
            compact_display: (notation == Notation::Compact).then_some(compact_display),
            sign_display,
            rounding_increment: digits.increment,
            rounding_mode: digits.mode,
            rounding_priority: digits.priority,
            trailing_zero_display: digits.trailing_zero_display,
        };
        Self {
            resolved,
            precision: digits.precision(),
            notation,
            grouping: use_grouping,
            sign_display,
            percent: style == NumberStyle::Percent,
            symbols,
            numerals,
            plurals,
            compact,
            percent_data,
            style: style_data,
        }
    }

    /// The options it settled on (`resolvedOptions()`).
    pub fn resolved_options(&self) -> &ResolvedNumberFormatOptions {
        &self.resolved
    }

    /// A value as people read it.
    pub fn format(&self, value: f64) -> String {
        self.layered(value, false).text()
    }

    /// The parts a value formats into (`formatToParts`).
    pub fn format_to_parts(&self, value: f64) -> Vec<NumberFormatPart> {
        self.layered(value, false).parts()
    }

    /// A range, with what both ends share written once, and "~" for ends that
    /// format the same (`formatRange`).
    pub fn format_range(&self, start: f64, end: f64) -> String {
        if start.is_nan() || end.is_nan() {
            warn("formatRange takes two numbers.");
            return format!("{}{RANGE_SEPARATOR}{}", self.format(start), self.format(end));
        }
        let first = self.layered(start, false);
        let second = self.layered(end, false);
        if first.text() == second.text() {
            return self.layered(start, true).text();
        }
        // ICU's collapsing: the outer affix, a unit or a currency's name, is
        // written once when both ends share it; the middle one, signs,
        // symbols and compact words, only when it's more than one character
        // too. The exponent of scientific notation never is.
        let collapse_outer = first.outer_prefix == second.outer_prefix
            && first.outer_suffix == second.outer_suffix;
        let collapse_middle = collapse_outer
            && first.middle_prefix == second.middle_prefix
            && first.middle_suffix == second.middle_suffix
            && first.middle_length() > 1;
        // Spaces go around the dash unless everything was collapsed.
        let spaced = matches!(self.notation, Notation::Scientific | Notation::Engineering)
            || (!collapse_middle && first.middle_length() > 0)
            || (!collapse_outer && first.outer_length() > 0);
        let mut before = String::new();
        before += &text_of(&first.outer_prefix);
        before += &text_of(&first.middle_prefix);
        before += &text_of(&first.number);
        if !collapse_middle {
            before += &text_of(&first.middle_suffix);
        }
        if !collapse_outer {
            before += &text_of(&first.outer_suffix);
        }
        let mut after = String::new();
        if !collapse_outer {
            after += &text_of(&second.outer_prefix);
        }
        if !collapse_middle {
            after += &text_of(&second.middle_prefix);
        }
        after += &text_of(&second.number);
        after += &text_of(&second.middle_suffix);
        after += &text_of(&second.outer_suffix);
        let mut text = before;
        if spaced && !text.ends_with(is_js_space) {
            text.push(' ');
        }
        text.push_str(RANGE_SEPARATOR);
        if spaced && !after.starts_with(is_js_space) {
            text.push(' ');
        }
        text += &after;
        text
    }

    // PartitionNumberPattern: the value rounded and written in the locale's
    // digits, inside its notation's, sign's and style's affixes.
    fn layered(&self, value: f64, approximately: bool) -> Layered {
        let nan = value.is_nan();
        let negative = !nan && value.is_sign_negative();
        let mut layered = Layered::default();
        let mut zero = false;
        let mut one = false;
        // The whole value's plural operands, which currency and unit names read.
        let mut operands = None;
        if nan {
            // ICU4X has no data for NaN's name, which is "NaN" in most locales.
            layered.number.push(part(NumberPartType::Nan, "NaN"));
        } else if value.is_infinite() {
            layered.number.push(part(NumberPartType::Infinity, "∞"));
        } else {
            let mut decimal = decimal_of(value.abs());
            if self.percent {
                decimal.multiply_pow10(2);
            }
            let (rounded, exponent) = self.round_with_exponent(&decimal, negative);
            zero = rounded.is_zero();
            one = exponent == 0
                && !zero
                && rounded.nonzero_magnitude_start() == 0
                && rounded.nonzero_magnitude_end() == 0
                && *rounded.magnitude_range().start() == 0
                && rounded.digit_at(0) == 1;
            self.push_digits(&rounded, &mut layered.number);
            match self.notation {
                Notation::Scientific | Notation::Engineering => {
                    self.push_exponent(exponent, &mut layered.number)
                }
                Notation::Compact if exponent > 0 => self.push_compact(
                    rounded.nonzero_magnitude_start() + exponent,
                    &rounded,
                    &mut layered,
                ),
                _ => {}
            }
            operands = Some(if self.notation == Notation::Compact && exponent > 0 {
                PluralOperands::from_significand_and_exponent(
                    &rounded,
                    exponent.clamp(0, i16::from(u8::MAX)) as u8,
                )
            } else {
                PluralOperands::from(&rounded)
            });
        }
        let sign = self.sign_of(negative, zero, nan);
        match &self.style {
            StyleData::Decimal => self.wrap_sign(&mut layered, sign, approximately),
            StyleData::Percent => self.wrap_percent(&mut layered, sign, approximately),
            StyleData::Currency(currency) => {
                self.wrap_currency(&mut layered, currency, sign, approximately, operands)
            }
            StyleData::Unit(unit) => {
                self.wrap_sign(&mut layered, sign, approximately);
                let mut items = self.unit_items(&unit.names, operands, one);
                if let Some((per, denominator)) = &unit.per {
                    items = per_items(per, items, denominator);
                }
                wrap_outer(&mut layered, items, "", NumberPartType::Unit);
            }
        }
        layered
    }

    // The rounded value and its power of ten (ComputeExponent): a scientific,
    // engineering or compact value is rounded at its scale, and a value that
    // rounds up into the next magnitude, like 999.9 to 1000, takes that
    // magnitude's exponent instead.
    fn round_with_exponent(&self, value: &UnsignedDecimal, negative: bool) -> (UnsignedDecimal, i16) {
        if self.notation == Notation::Standard || value.is_zero() {
            return (self.precision.round(value, negative), 0);
        }
        let magnitude = value.nonzero_magnitude_start();
        let exponent = self.exponent_for(magnitude);
        let rounded = self
            .precision
            .round(&value.clone().multiplied_pow10(-exponent), negative);
        if rounded.is_zero() || rounded.nonzero_magnitude_start() == magnitude - exponent {
            return (rounded, exponent);
        }
        let next = self.exponent_for(magnitude + 1);
        if next == exponent {
            return (rounded, exponent);
        }
        (
            self.precision
                .round(&value.clone().multiplied_pow10(-next), negative),
            next,
        )
    }

    // ComputeExponentForMagnitude.
    fn exponent_for(&self, magnitude: i16) -> i16 {
        match self.notation {
            Notation::Standard => 0,
            Notation::Scientific => magnitude,
            Notation::Engineering => magnitude.div_euclid(3) * 3,
            Notation::Compact => {
                let Some(compact) = &self.compact else {
                    return 0;
                };
                // The last pattern for a power of ten up to the magnitude, and
                // its count of zeros, as icu_decimal's compact formatter reads them.
                compact
                    .get()
                    .0
                    .iter()
                    .filter(|entry| i16::from(entry.sized) <= magnitude)
                    .last()
                    .map_or(0, |entry| {
                        i16::from(entry.sized) - i16::from(entry.variable.get_default().0.get())
                    })
            }
        }
    }

    // The digits, grouped, with the decimal separator and the fraction.
    fn push_digits(&self, value: &UnsignedDecimal, parts: &mut Vec<NumberFormatPart>) {
        let range = value.magnitude_range();
        let (lower, upper) = (*range.start(), *range.end());
        let mut integer = String::new();
        for magnitude in (0..=upper).rev() {
            integer.push(self.numeral(value.digit_at(magnitude)));
            if self.groups_after(upper, magnitude) {
                parts.push(part(NumberPartType::Integer, std::mem::take(&mut integer)));
                parts.push(part(NumberPartType::Group, self.symbols.group.clone()));
            }
        }
        parts.push(part(NumberPartType::Integer, integer));
        if lower < 0 {
            parts.push(part(NumberPartType::Decimal, self.symbols.decimal.clone()));
            let fraction: String = (lower..0)
                .rev()
                .map(|magnitude| self.numeral(value.digit_at(magnitude)))
                .collect();
            parts.push(part(NumberPartType::Fraction, fraction));
        }
    }

    fn numeral(&self, digit: u8) -> char {
        self.numerals[usize::from(digit.min(9))]
    }

    // Whether a group separator follows the digit at `magnitude`, as
    // icu_decimal's grouper decides, with ICU's strategies: always groups from
    // the first group, auto waits for the locale's minimum, min2 for two digits.
    fn groups_after(&self, upper: i16, magnitude: i16) -> bool {
        let (primary, secondary, locale_minimum) = self.symbols.grouping;
        if primary <= 0 || magnitude < primary {
            return false;
        }
        let minimum = match self.grouping {
            UseGrouping::Never => return false,
            UseGrouping::Always => 1,
            UseGrouping::Auto => locale_minimum.max(1),
            UseGrouping::Min2 => locale_minimum.max(2),
        };
        if upper < primary + minimum - 1 {
            return false;
        }
        let secondary = if secondary <= 0 { primary } else { secondary };
        (magnitude - primary) % secondary == 0
    }

    // ICU4X has no data for the exponent symbol, which is "E" in almost every locale.
    fn push_exponent(&self, exponent: i16, parts: &mut Vec<NumberFormatPart>) {
        parts.push(part(NumberPartType::ExponentSeparator, "E"));
        if exponent < 0 {
            let (prefix, suffix) = &self.symbols.minus;
            let minus = if prefix.is_empty() { suffix } else { prefix };
            parts.push(part(NumberPartType::ExponentMinusSign, minus.clone()));
        }
        let digits: String = exponent
            .unsigned_abs()
            .to_string()
            .bytes()
            .map(|digit| self.numeral(digit - b'0'))
            .collect();
        parts.push(part(NumberPartType::ExponentInteger, digits));
    }

    // The locale's compact affix for the value's power of ten, picked by the
    // plural of the significand, like "K" or " thousand".
    fn push_compact(&self, magnitude: i16, significand: &UnsignedDecimal, layered: &mut Layered) {
        let Some(compact) = &self.compact else {
            return;
        };
        let patterns = compact.get();
        let Some(entry) = patterns
            .0
            .iter()
            .filter(|entry| i16::from(entry.sized) <= magnitude)
            .last()
        else {
            return;
        };
        let pattern = match &self.plurals {
            Some(rules) => entry.variable.get(PluralOperands::from(significand), rules).1,
            None => entry.variable.get_default().1,
        };
        let mut seen = false;
        let mut prefix = Vec::new();
        for item in pattern.iter() {
            match item {
                PatternItem::Placeholder(_) => seen = true,
                PatternItem::Literal(text) if seen => {
                    push_trimmed(&mut layered.middle_suffix, text, NumberPartType::Compact)
                }
                PatternItem::Literal(text) => push_trimmed(&mut prefix, text, NumberPartType::Compact),
            }
        }
        layered.middle_prefix.splice(0..0, prefix);
    }

    // GetNumberFormatPattern's sign, from the rounded value.
    fn sign_of(&self, negative: bool, zero: bool, nan: bool) -> Sign {
        match self.sign_display {
            SignDisplay::Never => Sign::None,
            SignDisplay::Auto if !nan && negative => Sign::Minus,
            SignDisplay::Auto => Sign::None,
            SignDisplay::Always if !nan && negative => Sign::Minus,
            SignDisplay::Always => Sign::Plus,
            SignDisplay::ExceptZero if nan || zero => Sign::None,
            SignDisplay::ExceptZero if negative => Sign::Minus,
            SignDisplay::ExceptZero => Sign::Plus,
            SignDisplay::Negative if !nan && negative && !zero => Sign::Minus,
            SignDisplay::Negative => Sign::None,
        }
    }

    // The sign before and after the number, with the approximately sign where
    // the sign goes, like "~-5".
    fn sign_parts(&self, sign: Sign, approximately: bool) -> (Vec<NumberFormatPart>, Vec<NumberFormatPart>) {
        let mut before = Vec::new();
        let mut after = Vec::new();
        if approximately {
            before.push(part(NumberPartType::ApproximatelySign, self.approximately_sign()));
        }
        let (kind, (prefix, suffix)) = match sign {
            Sign::None => return (before, after),
            Sign::Minus => (NumberPartType::MinusSign, &self.symbols.minus),
            Sign::Plus => (NumberPartType::PlusSign, &self.symbols.plus),
        };
        if !prefix.is_empty() {
            before.push(part(kind, prefix.clone()));
        }
        if !suffix.is_empty() {
            after.push(part(kind, suffix.clone()));
        }
        (before, after)
    }

    fn approximately_sign(&self) -> String {
        self.percent_data
            .as_ref()
            .map_or_else(|| "~".to_owned(), |data| data.get().approximately_sign.to_string())
    }

    fn wrap_sign(&self, layered: &mut Layered, sign: Sign, approximately: bool) {
        let (before, after) = self.sign_parts(sign, approximately);
        layered.middle_prefix.splice(0..0, before);
        layered.middle_suffix.extend(after);
    }

    // A percent's pattern, unsigned or with the sign where the locale puts it.
    fn wrap_percent(&self, layered: &mut Layered, sign: Sign, approximately: bool) {
        let data = self.percent_data.as_ref().map(|data| data.get());
        let items = match data {
            Some(data) if sign == Sign::None && !approximately => single_items(&data.unsigned_pattern),
            Some(data) => double_items(&data.signed_pattern, Item::Sign),
            None => vec![Item::Sign, Item::Number, Item::Text("%".to_owned())],
        };
        let mut signs = Vec::new();
        if approximately {
            signs.push(part(NumberPartType::ApproximatelySign, self.approximately_sign()));
        }
        match sign {
            Sign::Minus => signs.push(part(
                NumberPartType::MinusSign,
                data.map_or_else(|| self.symbols.minus.0.clone(), |data| data.minus_sign.to_string()),
            )),
            Sign::Plus => signs.push(part(
                NumberPartType::PlusSign,
                data.map_or_else(|| self.symbols.plus.0.clone(), |data| data.plus_sign.to_string()),
            )),
            Sign::None => {}
        }
        let (before, after, _) = split_items(items, |item, parts| match item {
            Item::Text(text) => push_percent_text(parts, &text),
            Item::Sign => parts.extend(signs.iter().cloned()),
            Item::Number | Item::Symbol => {}
        });
        layered.middle_prefix.splice(0..0, before);
        layered.middle_suffix.extend(after);
    }

    fn wrap_currency(
        &self,
        layered: &mut Layered,
        currency: &CurrencyData,
        sign: Sign,
        approximately: bool,
        operands: Option<PluralOperands>,
    ) {
        let (items, symbol, sign_outside) = match currency {
            CurrencyData::Name { names, patterns, code } => {
                // A name goes around the number and its sign, like "-1.00 US dollars".
                self.wrap_sign(layered, sign, approximately);
                let name = names
                    .as_ref()
                    .map_or(code.as_str(), |names| self.plural(names.get(), operands));
                let items = patterns.as_ref().map_or_else(
                    || vec![Item::Number, Item::Text(" ".to_owned()), Item::Symbol],
                    |patterns| double_items(self.plural(patterns.get(), operands), Item::Symbol),
                );
                wrap_outer(layered, items, name, NumberPartType::Currency);
                return;
            }
            CurrencyData::Symbol {
                essentials,
                text,
                starts_with_letter,
                ends_with_letter,
                accounting,
            } => {
                let (starts, ends) = (*starts_with_letter, *ends_with_letter);
                match essentials {
                    Some(essentials) => {
                        let essentials = essentials.get();
                        let negative = (sign == Sign::Minus)
                            .then(|| {
                                if *accounting {
                                    essentials.get_negative_accounting(starts, ends)
                                } else {
                                    essentials.get_negative(starts, ends)
                                }
                            })
                            .flatten();
                        match negative {
                            Some(pattern) => (double_items(pattern, Item::Symbol), text.as_str(), false),
                            None => {
                                let pattern = if *accounting {
                                    essentials.get_positive_accounting(starts, ends)
                                } else {
                                    essentials.get_positive(starts, ends)
                                };
                                (double_items(pattern, Item::Symbol), text.as_str(), true)
                            }
                        }
                    }
                    None => (vec![Item::Symbol, Item::Number], text.as_str(), true),
                }
            }
            CurrencyData::Bare { patterns, accounting } => {
                let patterns = patterns.get();
                let negative = (sign == Sign::Minus)
                    .then(|| {
                        if *accounting {
                            patterns.get_negative_accounting()
                        } else {
                            patterns.get_negative()
                        }
                    })
                    .flatten();
                match negative {
                    Some(pattern) => (double_items(pattern, Item::Symbol), "", false),
                    None => {
                        let pattern = if *accounting {
                            patterns.get_positive_accounting()
                        } else {
                            patterns.get_positive()
                        };
                        (double_items(pattern, Item::Symbol), "", true)
                    }
                }
            }
        };
        let (mut before, mut after, _) = split_items(items, |item, parts| match item {
            Item::Text(text) => push_pattern_text(parts, &text),
            Item::Symbol if !symbol.is_empty() => parts.push(part(NumberPartType::Currency, symbol)),
            Item::Symbol | Item::Sign | Item::Number => {}
        });
        // Without a negative pattern of its own, the sign goes before the
        // whole pattern, like "-$5.00".
        if sign_outside {
            let (sign_before, sign_after) = self.sign_parts(sign, approximately);
            before.splice(0..0, sign_before);
            after.extend(sign_after);
        } else if approximately {
            before.insert(0, part(NumberPartType::ApproximatelySign, self.approximately_sign()));
        }
        layered.middle_prefix.splice(0..0, before);
        layered.middle_suffix.extend(after);
    }

    // A unit's pattern for the value, by its plural.
    fn unit_items(&self, names: &UnitNames, operands: Option<PluralOperands>, one: bool) -> Vec<Item> {
        match names {
            UnitNames::Data(names) => single_items(self.plural(names.get(), operands)),
            UnitNames::Builtin(patterns, width) => text_items(builtin_pattern(patterns, *width, one)),
            UnitNames::Unknown(unit) => vec![Item::Number, Item::Text(format!(" {unit}"))],
        }
    }

    fn plural<'a, V: VarULE + ?Sized>(
        &self,
        elements: &'a PluralElementsPackedCow<'_, V>,
        operands: Option<PluralOperands>,
    ) -> &'a V {
        match (&self.plurals, operands) {
            (Some(rules), Some(operands)) => elements.get(operands, rules),
            _ => elements.elements.get_default().1,
        }
    }
}

// The settings FormatNumericToString rounds with, resolved from the options.
#[derive(Clone, Copy, Debug)]
struct Precision {
    minimum_integer: i16,
    fraction: (i16, i16),
    significant: (i16, i16),
    kind: RoundingType,
    increment: (RoundingIncrement, i16),
    mode: RoundingMode,
    strip_if_integer: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoundingType {
    Fraction,
    Significant,
    MorePrecision,
    LessPrecision,
}

impl Precision {
    // FormatNumericToString on the value's magnitude: significant digits,
    // fraction digits, or whichever keeps more or fewer of them, then the
    // whole number's decimals dropped and the integer padded.
    fn round(&self, value: &UnsignedDecimal, negative: bool) -> UnsignedDecimal {
        let mode = unsigned_mode(self.mode, negative);
        let mut result = match self.kind {
            RoundingType::Significant => to_raw_precision(value, self.significant, mode).0,
            RoundingType::Fraction => to_raw_fixed(value, self.fraction, self.increment, mode).0,
            RoundingType::MorePrecision | RoundingType::LessPrecision => {
                let (significant, significant_magnitude) =
                    to_raw_precision(value, self.significant, mode);
                let (fixed, fixed_magnitude) = to_raw_fixed(value, self.fraction, self.increment, mode);
                let fixed_is_more_precise = fixed_magnitude < significant_magnitude;
                if (self.kind == RoundingType::MorePrecision) == fixed_is_more_precise {
                    fixed
                } else {
                    significant
                }
            }
        };
        if self.strip_if_integer {
            result.trim_end_if_integer();
        }
        result.pad_start(self.minimum_integer);
        result
    }
}

// ToRawPrecision: rounded to the maximum of significant digits, trailing
// zeros cut down to the minimum. Returns the magnitude it rounded at.
fn to_raw_precision(
    value: &UnsignedDecimal,
    (minimum, maximum): (i16, i16),
    mode: UnsignedRoundingMode,
) -> (UnsignedDecimal, i16) {
    let mut result = value.clone();
    if result.is_zero() {
        result.pad_end(1 - minimum);
        return (result, 1 - maximum);
    }
    let start = result.nonzero_magnitude_start();
    result.round_with_mode(start - maximum + 1, mode);
    // Rounding can carry into a new leading digit, like 9.99 to 10.0.
    let magnitude = result.nonzero_magnitude_start();
    result.pad_end((magnitude - minimum + 1).min(0));
    (result, magnitude - maximum + 1)
}

// ToRawFixed: rounded to the maximum of fraction digits, by the increment,
// trailing zeros cut down to the minimum. Returns the magnitude it rounded at.
fn to_raw_fixed(
    value: &UnsignedDecimal,
    (minimum, maximum): (i16, i16),
    (increment, shift): (RoundingIncrement, i16),
    mode: UnsignedRoundingMode,
) -> (UnsignedDecimal, i16) {
    let mut result = value.clone();
    result.round_with_mode_and_increment(shift - maximum, mode, increment);
    result.pad_end(-minimum);
    (result, -maximum)
}

// GetUnsignedRoundingMode: the modes that depend on the sign, for the value's
// magnitude.
fn unsigned_mode(mode: RoundingMode, negative: bool) -> UnsignedRoundingMode {
    match mode {
        RoundingMode::Ceil if negative => UnsignedRoundingMode::Trunc,
        RoundingMode::Ceil => UnsignedRoundingMode::Expand,
        RoundingMode::Floor if negative => UnsignedRoundingMode::Expand,
        RoundingMode::Floor => UnsignedRoundingMode::Trunc,
        RoundingMode::Expand => UnsignedRoundingMode::Expand,
        RoundingMode::Trunc => UnsignedRoundingMode::Trunc,
        RoundingMode::HalfCeil if negative => UnsignedRoundingMode::HalfTrunc,
        RoundingMode::HalfCeil => UnsignedRoundingMode::HalfExpand,
        RoundingMode::HalfFloor if negative => UnsignedRoundingMode::HalfExpand,
        RoundingMode::HalfFloor => UnsignedRoundingMode::HalfTrunc,
        RoundingMode::HalfExpand => UnsignedRoundingMode::HalfExpand,
        RoundingMode::HalfTrunc => UnsignedRoundingMode::HalfTrunc,
        RoundingMode::HalfEven => UnsignedRoundingMode::HalfEven,
    }
}

const INCREMENTS: [u16; 15] = [1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000];

// An increment as a multiple and a power of ten: 250 rounds to multiples of 25
// one place left of the last fraction digit.
fn increment_parts(increment: u16) -> (RoundingIncrement, i16) {
    match increment {
        2 => (RoundingIncrement::MultiplesOf2, 0),
        5 => (RoundingIncrement::MultiplesOf5, 0),
        10 => (RoundingIncrement::MultiplesOf1, 1),
        20 => (RoundingIncrement::MultiplesOf2, 1),
        25 => (RoundingIncrement::MultiplesOf25, 0),
        50 => (RoundingIncrement::MultiplesOf5, 1),
        100 => (RoundingIncrement::MultiplesOf1, 2),
        200 => (RoundingIncrement::MultiplesOf2, 2),
        250 => (RoundingIncrement::MultiplesOf25, 1),
        500 => (RoundingIncrement::MultiplesOf5, 2),
        1000 => (RoundingIncrement::MultiplesOf1, 3),
        2000 => (RoundingIncrement::MultiplesOf2, 3),
        2500 => (RoundingIncrement::MultiplesOf25, 2),
        5000 => (RoundingIncrement::MultiplesOf5, 3),
        _ => (RoundingIncrement::MultiplesOf1, 0),
    }
}

// What SetNumberFormatDigitOptions settles on.
struct Digits {
    minimum_integer: u8,
    fraction: Option<(u8, u8)>,
    significant: Option<(u8, u8)>,
    kind: RoundingType,
    increment: u16,
    mode: RoundingMode,
    priority: RoundingPriority,
    trailing_zero_display: TrailingZeroDisplay,
}

impl Digits {
    fn precision(&self) -> Precision {
        let (minimum_fraction, maximum_fraction) = self.fraction.unwrap_or((0, 0));
        let (minimum_significant, maximum_significant) = self.significant.unwrap_or((1, 21));
        Precision {
            minimum_integer: i16::from(self.minimum_integer),
            fraction: (i16::from(minimum_fraction), i16::from(maximum_fraction)),
            significant: (i16::from(minimum_significant), i16::from(maximum_significant)),
            kind: self.kind,
            increment: increment_parts(self.increment),
            mode: self.mode,
            strip_if_integer: self.trailing_zero_display == TrailingZeroDisplay::StripIfInteger,
        }
    }
}

// SetNumberFormatDigitOptions. Where Intl throws a RangeError or TypeError,
// the option falls back and a debug build warns.
fn resolve_digits(
    options: &NumberFormatOptions,
    notation: Notation,
    minimum_default: u8,
    maximum_default: u8,
) -> Digits {
    let minimum_integer = number_option(options.minimum_integer_digits, 1, 21, 1, "minimumIntegerDigits");
    let mut increment = options.rounding_increment.unwrap_or(1);
    if !INCREMENTS.contains(&increment) {
        warn(format_args!("roundingIncrement {increment} isn't one Intl takes."));
        increment = 1;
    }
    let mode = options.rounding_mode.unwrap_or_default();
    let priority = options.rounding_priority.unwrap_or_default();
    let trailing_zero_display = options.trailing_zero_display.unwrap_or_default();
    let maximum_default = if increment != 1 { minimum_default } else { maximum_default };
    let has_significant =
        options.minimum_significant_digits.is_some() || options.maximum_significant_digits.is_some();
    let has_fraction =
        options.minimum_fraction_digits.is_some() || options.maximum_fraction_digits.is_some();
    let mut need_significant = true;
    let mut need_fraction = true;
    if priority == RoundingPriority::Auto {
        need_significant = has_significant;
        if need_significant || (!has_fraction && notation == Notation::Compact) {
            need_fraction = false;
        }
    }
    let mut significant = None;
    if need_significant {
        significant = Some(if has_significant {
            let minimum = number_option(options.minimum_significant_digits, 1, 21, 1, "minimumSignificantDigits");
            let maximum = number_option(
                options.maximum_significant_digits,
                minimum,
                21,
                21,
                "maximumSignificantDigits",
            );
            (minimum, maximum)
        } else {
            (1, 21)
        });
    }
    let mut fraction = None;
    if need_fraction {
        fraction = Some(if has_fraction {
            let minimum = options
                .minimum_fraction_digits
                .map(|digits| number_option(Some(digits), 0, 100, 0, "minimumFractionDigits"));
            let maximum = options
                .maximum_fraction_digits
                .map(|digits| number_option(Some(digits), 0, 100, 0, "maximumFractionDigits"));
            match (minimum, maximum) {
                (None, Some(maximum)) => (minimum_default.min(maximum), maximum),
                (Some(minimum), None) => (minimum, maximum_default.max(minimum)),
                (Some(minimum), Some(maximum)) if minimum > maximum => {
                    warn("minimumFractionDigits is above maximumFractionDigits.");
                    (minimum, minimum)
                }
                (Some(minimum), Some(maximum)) => (minimum, maximum),
                (None, None) => (minimum_default, maximum_default),
            }
        } else {
            (minimum_default, maximum_default)
        });
    }
    let (kind, priority) = if !need_significant && !need_fraction {
        fraction = Some((0, 0));
        significant = Some((1, 2));
        (RoundingType::MorePrecision, RoundingPriority::MorePrecision)
    } else {
        match priority {
            RoundingPriority::Auto if has_significant => (RoundingType::Significant, RoundingPriority::Auto),
            RoundingPriority::Auto => (RoundingType::Fraction, RoundingPriority::Auto),
            RoundingPriority::MorePrecision => (RoundingType::MorePrecision, priority),
            RoundingPriority::LessPrecision => (RoundingType::LessPrecision, priority),
        }
    };
    if increment != 1 {
        if kind != RoundingType::Fraction {
            warn("roundingIncrement needs fraction digits, not significant ones.");
            increment = 1;
        } else if fraction.is_some_and(|(minimum, maximum)| minimum != maximum) {
            warn("roundingIncrement needs the same minimum and maximum fraction digits.");
            increment = 1;
        }
    }
    Digits {
        minimum_integer,
        fraction,
        significant,
        kind,
        increment,
        mode,
        priority,
        trailing_zero_display,
    }
}

// GetNumberOption: the option, its fallback when unset, or the nearest bound
// when out of range.
fn number_option(value: Option<u8>, minimum: u8, maximum: u8, fallback: u8, name: &str) -> u8 {
    match value {
        None => fallback,
        Some(value) if value < minimum || value > maximum => {
            warn(format_args!("{name} is {value}, out of {minimum} to {maximum}."));
            value.clamp(minimum, maximum)
        }
        Some(value) => value,
    }
}

// The decimal symbols, the currency's own where the locale has them, like
// escudos in Portugal, in the numbering system when it has some.
fn decimal_symbols(locale: &DataLocale, numbering: Option<&str>, currency: Option<CurrencyType>) -> Symbols {
    for attributes in numbering.into_iter().chain([""]) {
        if let Some(currency) = currency {
            let code = currency.iso_code();
            let key = if attributes.is_empty() {
                code.as_str().to_owned()
            } else {
                format!("{attributes}/{}", code.as_str())
            };
            if let Some(symbols) = load::<CurrencyDecimalSymbolsV1>(&Baked, locale, &key) {
                return Symbols::of(symbols.get());
            }
        }
        if let Some(symbols) = load::<DecimalSymbolsV1>(&icu_decimal::provider::Baked, locale, attributes) {
            return Symbols::of(symbols.get());
        }
    }
    Symbols::fallback()
}

const LATIN_DIGITS: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

// The digits of the requested numbering system when there are some, else of
// the symbols' own, and the system's name.
fn numerals_of(requested: Option<&str>, own: &str) -> ([char; 10], String) {
    let digits = |numbering: &str| {
        load::<DecimalDigitsV1>(&icu_decimal::provider::Baked, &DataLocale::default(), numbering)
            .map(|digits| *digits.get())
    };
    if let Some(requested) = requested
        && let Some(found) = digits(requested)
    {
        return (found, requested.to_owned());
    }
    (digits(own).unwrap_or(LATIN_DIGITS), own.to_owned())
}

// The numbering system the options or the locale's `-u-nu-` ask for.
fn numbering_of(locale: &Locale, options: &NumberFormatOptions) -> Option<String> {
    if let Some(numbering) = &options.numbering_system {
        match Subtag::try_from_str(numbering) {
            Ok(subtag) => return Some(subtag.as_str().to_ascii_lowercase()),
            Err(_) => warn(format_args!("numberingSystem \"{numbering}\" isn't one.")),
        }
    }
    locale
        .extensions
        .unicode
        .keywords
        .get(&key!("nu"))
        .and_then(|value| value.as_single_subtag())
        .map(|subtag| subtag.as_str().to_owned())
}

// CurrencyDigits: the decimals an amount of the currency shows.
fn currency_digits(currency: CurrencyType) -> u8 {
    load::<CurrencyFractionsV1>(&Baked, &DataLocale::default(), "")
        .map_or(2, |fractions| fractions.get().resolve(currency).digits)
}

fn currency_data(
    preferences: LocalePreferences,
    numbering: Option<&str>,
    currency: CurrencyType,
    display: CurrencyDisplay,
    sign: CurrencySign,
) -> CurrencyData {
    let code = currency.iso_code().as_str().to_owned();
    let accounting = sign == CurrencySign::Accounting;
    let locale = CurrencyEssentialsV1::make_locale(preferences);
    let essentials = || load_numbered::<CurrencyEssentialsV1>(&Baked, &locale, numbering);
    match display {
        CurrencyDisplay::Name => CurrencyData::Name {
            names: load::<CurrencyExtendedDataV1>(
                &Baked,
                &CurrencyExtendedDataV1::make_locale(preferences),
                &code,
            ),
            patterns: load::<CurrencyPatternsDataV1>(
                &Baked,
                &CurrencyPatternsDataV1::make_locale(preferences),
                "",
            ),
            code,
        },
        CurrencyDisplay::Code => CurrencyData::Symbol {
            essentials: essentials(),
            text: code,
            starts_with_letter: true,
            ends_with_letter: true,
            accounting,
        },
        CurrencyDisplay::Symbol | CurrencyDisplay::NarrowSymbol => {
            let key = if display == CurrencyDisplay::NarrowSymbol {
                format!("{code}n")
            } else {
                code.clone()
            };
            let symbol = load::<CurrencySymbolsV1>(
                &Baked,
                &CurrencySymbolsV1::make_locale(preferences),
                &key,
            );
            match symbol {
                // An empty symbol means the locale writes the amount alone.
                Some(symbol) if symbol.get().as_str().is_empty() => {
                    match load_numbered::<CurrencyPatternsNoCurrencyV1>(&Baked, &locale, numbering) {
                        Some(patterns) => CurrencyData::Bare {
                            patterns,
                            accounting,
                        },
                        None => CurrencyData::Symbol {
                            essentials: essentials(),
                            text: String::new(),
                            starts_with_letter: false,
                            ends_with_letter: false,
                            accounting,
                        },
                    }
                }
                Some(symbol) => {
                    let symbol = symbol.get();
                    CurrencyData::Symbol {
                        essentials: essentials(),
                        text: symbol.as_str().to_owned(),
                        starts_with_letter: symbol.starts_with_letter(),
                        ends_with_letter: symbol.ends_with_letter(),
                        accounting,
                    }
                }
                // Without a symbol, the code stands in.
                None => CurrencyData::Symbol {
                    essentials: essentials(),
                    text: code,
                    starts_with_letter: true,
                    ends_with_letter: true,
                    accounting,
                },
            }
        }
    }
}

fn unit_data(
    preferences: LocalePreferences,
    unit: &str,
    display: UnitDisplay,
    plurals: Option<&PluralRules>,
) -> UnitData {
    let names_of = |unit: &str| {
        unit_names(preferences, unit, display).unwrap_or_else(|| {
            warn(format_args!("\"{unit}\" isn't a unit Intl takes."));
            UnitNames::Unknown(unit.to_owned())
        })
    };
    // A compound like "kilometer-per-hour" is its numerator through the
    // locale's "per" pattern with its denominator's name.
    let (numerator, denominator) = match unit.split_once("-per-") {
        Some((numerator, denominator)) => (numerator, Some(denominator)),
        None => (unit, None),
    };
    let per = denominator.map(|denominator| {
        let pattern = load::<UnitsEssentialsV1>(
            &Baked,
            &UnitsEssentialsV1::make_locale(preferences),
            display.as_str(),
        )
        .map_or_else(
            || {
                if display == UnitDisplay::Long { "{0} per {1}" } else { "{0}/{1}" }.to_owned()
            },
            |essentials| essentials.get().per.to_string(),
        );
        (pattern, denominator_name(&names_of(denominator), plurals))
    });
    UnitData {
        names: names_of(numerator),
        per,
    }
}

// A unit's name alone, as its pattern for one writes it, like "hr".
fn denominator_name(names: &UnitNames, plurals: Option<&PluralRules>) -> String {
    let text = match names {
        UnitNames::Data(names) => {
            let names = names.get();
            let pattern = match plurals {
                Some(rules) => names.get(PluralOperands::from(1u32), rules),
                None => names.elements.get_default().1,
            };
            pattern
                .iter()
                .filter_map(|item| match item {
                    PatternItem::Literal(text) => Some(text),
                    PatternItem::Placeholder(_) => None,
                })
                .collect::<String>()
        }
        UnitNames::Builtin(patterns, width) => builtin_pattern(patterns, *width, true).replace("{0}", ""),
        UnitNames::Unknown(unit) => unit.clone(),
    };
    text.trim_matches(is_js_space).to_owned()
}

#[derive(Clone, Copy)]
enum UnitCategory {
    Area,
    Duration,
    Length,
    Mass,
    Volume,
}

// The category ICU4X files a unit's names under, for the units Intl takes.
fn unit_category(unit: &str) -> Option<UnitCategory> {
    Some(match unit {
        "acre" | "hectare" => UnitCategory::Area,
        "day" | "hour" | "microsecond" | "millisecond" | "minute" | "month" | "nanosecond"
        | "second" | "week" | "year" => UnitCategory::Duration,
        "centimeter" | "foot" | "inch" | "kilometer" | "meter" | "mile" | "mile-scandinavian"
        | "millimeter" | "yard" => UnitCategory::Length,
        "gram" | "kilogram" | "ounce" | "pound" | "stone" => UnitCategory::Mass,
        "fluid-ounce" | "gallon" | "liter" | "milliliter" => UnitCategory::Volume,
        _ => return None,
    })
}

// A unit's names from ICU4X's data, tried in its core, extended and outlier
// sets, or from the built-in table.
fn unit_names(preferences: LocalePreferences, unit: &str, display: UnitDisplay) -> Option<UnitNames> {
    let attributes = format!("{}-{unit}", display.as_str());
    macro_rules! lookup {
        ($($marker:ty),+) => {{
            $(
                if let Some(names) = load::<$marker>(&Baked, &<$marker>::make_locale(preferences), &attributes) {
                    return Some(UnitNames::Data(names.cast()));
                }
            )+
        }};
    }
    match unit_category(unit) {
        Some(UnitCategory::Area) => lookup!(UnitsNamesAreaCoreV1, UnitsNamesAreaExtendedV1, UnitsNamesAreaOutlierV1),
        Some(UnitCategory::Duration) => lookup!(
            UnitsNamesDurationCoreV1,
            UnitsNamesDurationExtendedV1,
            UnitsNamesDurationOutlierV1
        ),
        Some(UnitCategory::Length) => lookup!(
            UnitsNamesLengthCoreV1,
            UnitsNamesLengthExtendedV1,
            UnitsNamesLengthOutlierV1
        ),
        Some(UnitCategory::Mass) => lookup!(UnitsNamesMassCoreV1, UnitsNamesMassExtendedV1, UnitsNamesMassOutlierV1),
        Some(UnitCategory::Volume) => lookup!(
            UnitsNamesVolumeCoreV1,
            UnitsNamesVolumeExtendedV1,
            UnitsNamesVolumeOutlierV1
        ),
        None => lookup!(
            UnitsNamesAreaCoreV1,
            UnitsNamesAreaExtendedV1,
            UnitsNamesAreaOutlierV1,
            UnitsNamesDurationCoreV1,
            UnitsNamesDurationExtendedV1,
            UnitsNamesDurationOutlierV1,
            UnitsNamesLengthCoreV1,
            UnitsNamesLengthExtendedV1,
            UnitsNamesLengthOutlierV1,
            UnitsNamesMassCoreV1,
            UnitsNamesMassExtendedV1,
            UnitsNamesMassOutlierV1,
            UnitsNamesVolumeCoreV1,
            UnitsNamesVolumeExtendedV1,
            UnitsNamesVolumeOutlierV1
        ),
    }
    BUILTIN_UNITS
        .iter()
        .find(|(id, _)| *id == unit)
        .map(|(_, patterns)| UnitNames::Builtin(patterns, display))
}

// The units Intl takes that ICU4X has no names for, digital, temperature,
// angle and concentration ones, as CLDR's English writes them: long for one,
// long for other, short, narrow. Every locale reads them in English.
const BUILTIN_UNITS: &[(&str, [&str; 4])] = &[
    ("bit", ["{0} bit", "{0} bits", "{0} bit", "{0}bit"]),
    ("byte", ["{0} byte", "{0} bytes", "{0} byte", "{0}B"]),
    ("kilobit", ["{0} kilobit", "{0} kilobits", "{0} kb", "{0}kb"]),
    ("kilobyte", ["{0} kilobyte", "{0} kilobytes", "{0} kB", "{0}kB"]),
    ("megabit", ["{0} megabit", "{0} megabits", "{0} Mb", "{0}Mb"]),
    ("megabyte", ["{0} megabyte", "{0} megabytes", "{0} MB", "{0}MB"]),
    ("gigabit", ["{0} gigabit", "{0} gigabits", "{0} Gb", "{0}Gb"]),
    ("gigabyte", ["{0} gigabyte", "{0} gigabytes", "{0} GB", "{0}GB"]),
    ("terabit", ["{0} terabit", "{0} terabits", "{0} Tb", "{0}Tb"]),
    ("terabyte", ["{0} terabyte", "{0} terabytes", "{0} TB", "{0}TB"]),
    ("petabyte", ["{0} petabyte", "{0} petabytes", "{0} PB", "{0}PB"]),
    ("celsius", ["{0} degree Celsius", "{0} degrees Celsius", "{0}°C", "{0}°C"]),
    ("fahrenheit", ["{0} degree Fahrenheit", "{0} degrees Fahrenheit", "{0}°F", "{0}°"]),
    ("degree", ["{0} degree", "{0} degrees", "{0} deg", "{0}°"]),
    ("percent", ["{0} percent", "{0} percent", "{0}%", "{0}%"]),
];

fn builtin_pattern(patterns: &[&'static str; 4], width: UnitDisplay, one: bool) -> &'static str {
    match width {
        UnitDisplay::Long if one => patterns[0],
        UnitDisplay::Long => patterns[1],
        UnitDisplay::Short => patterns[2],
        UnitDisplay::Narrow => patterns[3],
    }
}

// A locale's data under some attributes, with the locale falling back along
// CLDR's parents.
fn load<M: DataMarker>(provider: &impl DataProvider<M>, locale: &DataLocale, attributes: &str) -> Option<DataPayload<M>> {
    let attributes = if attributes.is_empty() {
        DataMarkerAttributes::empty()
    } else {
        DataMarkerAttributes::try_from_str(attributes).ok()?
    };
    let mut metadata = DataRequestMetadata::default();
    metadata.silent = true;
    provider
        .load(DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, locale),
            metadata,
        })
        .ok()
        .map(|response| response.payload)
}

// A locale's data in a numbering system when there is some, else in its own.
fn load_numbered<M: DataMarker>(
    provider: &impl DataProvider<M>,
    locale: &DataLocale,
    numbering: Option<&str>,
) -> Option<DataPayload<M>> {
    numbering
        .and_then(|numbering| load(provider, locale, numbering))
        .or_else(|| load(provider, locale, ""))
}

// The locale a tag names, a POSIX one like "en_US.UTF-8" too, or the system's.
fn resolve_locale(tag: Option<&str>) -> Locale {
    if let Some(tag) = tag {
        if let Some(locale) = parse_locale(tag) {
            return locale;
        }
        warn(format_args!("\"{tag}\" isn't a language tag."));
    }
    system_locale().clone()
}

fn parse_locale(tag: &str) -> Option<Locale> {
    Locale::try_from_str(tag).ok().or_else(|| {
        // A POSIX locale's encoding and modifier go, as in `de_DE.UTF-8@euro`.
        let tag = tag.split(['.', '@']).next()?.replace('_', "-");
        Locale::try_from_str(&tag).ok()
    })
}

// The system's language, read once, which formatters use without a locale.
fn system_locale() -> &'static Locale {
    static SYSTEM: OnceLock<Locale> = OnceLock::new();
    SYSTEM.get_or_init(|| {
        sys_locale::get_locale()
            .and_then(|tag| parse_locale(&tag))
            .unwrap_or_else(|| icu_locale_core::locale!("en-US"))
    })
}

// A finite, non-negative double as the decimal it reads back as, like V8 and
// ICU take it, so 1.005 at two decimals is 1.01.
fn decimal_of(value: f64) -> UnsignedDecimal {
    let text = format!("{value:e}");
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    let mut decimal = UnsignedDecimal::try_from_str(mantissa).unwrap_or_default();
    decimal.multiply_pow10(exponent.parse().unwrap_or(0));
    decimal
}

fn warn(message: impl fmt::Display) {
    if cfg!(debug_assertions) {
        log::warn!("[ui-sdk] {message}");
    }
}

// The locale's symbols, kept as text.
struct Symbols {
    minus: (String, String),
    plus: (String, String),
    decimal: String,
    group: String,
    numbering: String,
    // The primary and secondary group sizes, and the digits the first group waits for.
    grouping: (i16, i16, i16),
}

impl Symbols {
    fn of(symbols: &DecimalSymbols<'_>) -> Self {
        let strings = &symbols.strings;
        let sizes = symbols.grouping_sizes;
        Self {
            minus: (
                strings.minus_sign_prefix().to_owned(),
                strings.minus_sign_suffix().to_owned(),
            ),
            plus: (
                strings.plus_sign_prefix().to_owned(),
                strings.plus_sign_suffix().to_owned(),
            ),
            decimal: strings.decimal_separator().to_owned(),
            group: strings.grouping_separator().to_owned(),
            numbering: strings.numsys().to_owned(),
            grouping: (
                i16::from(sizes.primary),
                i16::from(sizes.secondary),
                i16::from(sizes.min_grouping),
            ),
        }
    }

    // Root's symbols, if even those fail to load.
    fn fallback() -> Self {
        Self {
            minus: ("-".to_owned(), String::new()),
            plus: ("+".to_owned(), String::new()),
            decimal: ".".to_owned(),
            group: ",".to_owned(),
            numbering: "latn".to_owned(),
            grouping: (3, 3, 1),
        }
    }
}

enum StyleData {
    Decimal,
    Percent,
    Currency(CurrencyData),
    Unit(UnitData),
}

enum CurrencyData {
    /// A symbol, narrow symbol or code, through the locale's currency patterns.
    Symbol {
        essentials: Option<DataPayload<CurrencyEssentialsV1>>,
        text: String,
        starts_with_letter: bool,
        ends_with_letter: bool,
        accounting: bool,
    },
    /// No symbol: the locale writes the amount alone.
    Bare {
        patterns: DataPayload<CurrencyPatternsNoCurrencyV1>,
        accounting: bool,
    },
    /// Names by plural, like "US dollars", or the code without one.
    Name {
        names: Option<DataPayload<CurrencyExtendedDataV1>>,
        patterns: Option<DataPayload<CurrencyPatternsDataV1>>,
        code: String,
    },
}

struct UnitData {
    names: UnitNames,
    // The "per" pattern and the denominator's name, for a compound unit.
    per: Option<(String, String)>,
}

enum UnitNames {
    Data(DataPayload<ErasedMarker<UnitsDisplayNames<'static>>>),
    Builtin(&'static [&'static str; 4], UnitDisplay),
    Unknown(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sign {
    None,
    Minus,
    Plus,
}

// A pattern's pieces: text, the number, and its other placeholder, a sign or
// a currency.
#[derive(Clone, Debug)]
enum Item {
    Text(String),
    Number,
    Sign,
    Symbol,
}

fn single_items(pattern: &SinglePlaceholderPattern) -> Vec<Item> {
    pattern
        .iter()
        .map(|item| match item {
            PatternItem::Literal(text) => Item::Text(text.to_owned()),
            PatternItem::Placeholder(_) => Item::Number,
        })
        .collect()
}

fn double_items(pattern: &DoublePlaceholderPattern, other: Item) -> Vec<Item> {
    pattern
        .iter()
        .map(|item| match item {
            PatternItem::Literal(text) => Item::Text(text.to_owned()),
            PatternItem::Placeholder(DoublePlaceholderKey::Place0) => Item::Number,
            PatternItem::Placeholder(DoublePlaceholderKey::Place1) => other.clone(),
        })
        .collect()
}

// A pattern written as text, like "{0} GB".
fn text_items(pattern: &str) -> Vec<Item> {
    match pattern.split_once("{0}") {
        Some((before, after)) => vec![
            Item::Text(before.to_owned()),
            Item::Number,
            Item::Text(after.to_owned()),
        ],
        None => vec![Item::Text(pattern.to_owned())],
    }
}

// A "per" pattern like "{0}/{1}", with the numerator's pieces for {0} and the
// denominator's name for {1}.
fn per_items(pattern: &str, numerator: Vec<Item>, denominator: &str) -> Vec<Item> {
    let mut items = Vec::new();
    let mut rest = pattern;
    loop {
        let next = [rest.find("{0}"), rest.find("{1}")].into_iter().flatten().min();
        let Some(at) = next else {
            if !rest.is_empty() {
                items.push(Item::Text(rest.to_owned()));
            }
            return items;
        };
        if at > 0 {
            items.push(Item::Text(rest[..at].to_owned()));
        }
        if rest[at..].starts_with("{0}") {
            items.extend(numerator.iter().cloned());
        } else {
            items.push(Item::Text(denominator.to_owned()));
        }
        rest = &rest[at + 3..];
    }
}

// The parts of a pattern's pieces before and after the number, and whether it
// has one.
fn split_items(
    items: Vec<Item>,
    mut push: impl FnMut(Item, &mut Vec<NumberFormatPart>),
) -> (Vec<NumberFormatPart>, Vec<NumberFormatPart>, bool) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut seen = false;
    for item in items {
        if let Item::Number = item {
            seen = true;
            continue;
        }
        push(item, if seen { &mut after } else { &mut before });
    }
    (before, after, seen)
}

// A unit's or currency name's pattern around the number and its sign, with
// its text as one part between the spaces.
fn wrap_outer(layered: &mut Layered, items: Vec<Item>, symbol: &str, kind: NumberPartType) {
    let mut before = String::new();
    let mut after = String::new();
    let mut seen = false;
    for item in items {
        let text = if seen { &mut after } else { &mut before };
        match item {
            Item::Number => seen = true,
            Item::Text(piece) => text.push_str(&piece),
            Item::Symbol => text.push_str(symbol),
            Item::Sign => {}
        }
    }
    if !seen {
        // A pattern without the number says the whole value, like Arabic's
        // word for two days.
        *layered = Layered::default();
    }
    push_trimmed(&mut layered.outer_prefix, &before, kind);
    push_trimmed(&mut layered.outer_suffix, &after, kind);
}

// Text as a part of its kind, with the spaces around it as literals.
fn push_trimmed(parts: &mut Vec<NumberFormatPart>, text: &str, kind: NumberPartType) {
    let core = text.trim_matches(is_js_space);
    if core.is_empty() {
        if !text.is_empty() {
            parts.push(part(NumberPartType::Literal, text));
        }
        return;
    }
    let start = text.len() - text.trim_start_matches(is_js_space).len();
    let end = start + core.len();
    if start > 0 {
        parts.push(part(NumberPartType::Literal, &text[..start]));
    }
    parts.push(part(kind, core));
    if end < text.len() {
        parts.push(part(NumberPartType::Literal, &text[end..]));
    }
}

// A run of text and a run of spaces, alternately.
fn runs(text: &str) -> impl Iterator<Item = (&str, bool)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let first = rest.chars().next()?;
        let space = is_js_space(first);
        let end = rest
            .char_indices()
            .find(|(_, c)| is_js_space(*c) != space)
            .map_or(rest.len(), |(index, _)| index);
        let (run, next) = rest.split_at(end);
        rest = next;
        Some((run, space))
    })
}

// A percent pattern's text: the percent sign, and spaces and marks as literals.
fn push_percent_text(parts: &mut Vec<NumberFormatPart>, text: &str) {
    for (run, space) in runs(text) {
        let percent = !space && run.chars().any(|c| matches!(c, '%' | '٪' | '﹪' | '％' | '‰' | '؉'));
        parts.push(part(
            if percent { NumberPartType::PercentSign } else { NumberPartType::Literal },
            run,
        ));
    }
}

// A currency pattern's text: a minus sign of its own, and the rest as literals.
fn push_pattern_text(parts: &mut Vec<NumberFormatPart>, text: &str) {
    for (run, space) in runs(text) {
        let minus = !space && run.chars().any(|c| matches!(c, '-' | '−'));
        parts.push(part(
            if minus { NumberPartType::MinusSign } else { NumberPartType::Literal },
            run,
        ));
    }
}

// ICU4X has no data for the range separator, which is an en dash in most locales.
const RANGE_SEPARATOR: &str = "–";

// A formatted number in ICU's layers: a unit or a currency's name outside,
// signs, symbols and compact words in the middle, and the number itself.
#[derive(Default)]
struct Layered {
    outer_prefix: Vec<NumberFormatPart>,
    middle_prefix: Vec<NumberFormatPart>,
    number: Vec<NumberFormatPart>,
    middle_suffix: Vec<NumberFormatPart>,
    outer_suffix: Vec<NumberFormatPart>,
}

impl Layered {
    fn parts(self) -> Vec<NumberFormatPart> {
        let mut parts: Vec<NumberFormatPart> = Vec::new();
        for next in self
            .outer_prefix
            .into_iter()
            .chain(self.middle_prefix)
            .chain(self.number)
            .chain(self.middle_suffix)
            .chain(self.outer_suffix)
        {
            if next.value.is_empty() {
                continue;
            }
            // Pieces of one kind side by side, like a pattern's space and a
            // compact word's, are one part.
            match parts.last_mut() {
                Some(last) if last.kind == next.kind && next.kind != NumberPartType::Integer => {
                    last.value.push_str(&next.value)
                }
                _ => parts.push(next),
            }
        }
        parts
    }

    fn text(&self) -> String {
        [
            &self.outer_prefix,
            &self.middle_prefix,
            &self.number,
            &self.middle_suffix,
            &self.outer_suffix,
        ]
        .into_iter()
        .map(|parts| text_of(parts))
        .collect()
    }

    fn middle_length(&self) -> usize {
        length_of(&self.middle_prefix) + length_of(&self.middle_suffix)
    }

    fn outer_length(&self) -> usize {
        length_of(&self.outer_prefix) + length_of(&self.outer_suffix)
    }
}

fn text_of(parts: &[NumberFormatPart]) -> String {
    parts.iter().map(|part| part.value.as_str()).collect()
}

fn length_of(parts: &[NumberFormatPart]) -> usize {
    parts.iter().map(|part| part.value.chars().count()).sum()
}

fn part(kind: NumberPartType, value: impl Into<String>) -> NumberFormatPart {
    NumberFormatPart {
        kind,
        value: value.into(),
    }
}

/// Whether a character is JavaScript's whitespace, what `\s` and `trim` take:
/// line breaks, tabs, spaces, no-break spaces and the byte order mark.
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
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
