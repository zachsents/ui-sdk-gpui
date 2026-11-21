//! Port of `foundation/gauge/gauge.ts`.
//!
//! Progress and Meter show a number between bounds as a bar. This module
//! holds where the value sits, how it reads, and the ARIA and placement of
//! their track, indicator, label and value parts.

use std::fmt;
use std::rc::Rc;

use gpui::{App, ElementId, SharedString, StatefulInteractiveElement, Styled, relative};
use ui_sdk_kernel::{Aria, Instance, Part, flag_while_bound};
use ui_sdk_number::{NumberFormat, NumberFormatOptions, NumberStyle, clamp, format_number};
use ui_sdk_text::locale_of;

/// What `get_value_label` receives after the value, which is always there for
/// a meter and `None` while a progress bar is indeterminate.
#[derive(Clone, Debug, PartialEq)]
pub struct GaugeLabelDetails {
    /// The value as `format` and `locale` show it. Empty while there is no value.
    pub formatted: String,
    /// Where the value sits between `min` and `max`, from 0 to 100. `None`
    /// while there is no value.
    pub percent: Option<f64>,
    /// The lowest value.
    pub min: f64,
    /// The highest value.
    pub max: f64,
}

/// The text screen readers announce for a value, from the value and its details.
pub type GetValueLabel = Rc<dyn Fn(Option<f64>, &GaugeLabelDetails) -> String>;

/// The options Progress and Meter share.
#[derive(Clone, Default)]
pub struct GaugeOptions {
    /// The lowest value. 0 by default.
    pub min: Option<f64>,
    /// The highest value. 100 by default.
    pub max: Option<f64>,
    /// How the value reads, for screen readers and the Value part: number
    /// format options, or a function of the value. A percent style shows
    /// where the value sits between `min` and `max`; any other style shows
    /// the value itself, like a unit style with `"gigabyte"`. A percent style
    /// by default.
    pub format: Option<NumberFormat>,
    /// The locale the value is formatted in. The app's language by default
    /// (`ui_sdk_text::page_locale`).
    pub locale: Option<SharedString>,
    /// The text screen readers announce for the value, when the formatted
    /// value says too little, like "3 of 12 files". It receives the value,
    /// `None` while a progress bar is indeterminate, and details: the
    /// `formatted` value, its `percent`, `min` and `max`.
    pub get_value_label: Option<GetValueLabel>,
}

impl GaugeOptions {
    pub fn min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    pub fn format(mut self, format: impl Into<NumberFormat>) -> Self {
        self.format = Some(format.into());
        self
    }

    pub fn locale(mut self, locale: impl Into<SharedString>) -> Self {
        self.locale = Some(locale.into());
        self
    }

    pub fn get_value_label(
        mut self,
        label: impl Fn(Option<f64>, &GaugeLabelDetails) -> String + 'static,
    ) -> Self {
        self.get_value_label = Some(Rc::new(label));
        self
    }
}

impl fmt::Debug for GaugeOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GaugeOptions")
            .field("min", &self.min)
            .field("max", &self.max)
            .field("format", &self.format)
            .field("locale", &self.locale)
            .field("get_value_label", &self.get_value_label.is_some())
            .finish()
    }
}

/// The settings parts render from, kept in state so a change re-renders them.
///
/// The original keeps the page's language here too, read once the root
/// binds. Parts read the app's through `locale_of` as they render instead.
#[derive(Clone)]
pub struct GaugeSettings {
    pub min: f64,
    pub max: f64,
    pub format: Option<NumberFormat>,
    pub locale: Option<SharedString>,
    pub get_value_label: Option<GetValueLabel>,
}

impl fmt::Debug for GaugeSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GaugeSettings")
            .field("min", &self.min)
            .field("max", &self.max)
            .field("format", &self.format)
            .field("locale", &self.locale)
            .field("get_value_label", &self.get_value_label.is_some())
            .finish()
    }
}

impl PartialEq for GaugeSettings {
    fn eq(&self, other: &Self) -> bool {
        self.min == other.min
            && self.max == other.max
            && self.format == other.format
            && self.locale == other.locale
            && match (&self.get_value_label, &other.get_value_label) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

/// The settings from the options. A maximum below the minimum is the minimum.
pub fn gauge_settings(options: &GaugeOptions) -> GaugeSettings {
    let min = finite(options.min, 0.0);
    GaugeSettings {
        min,
        max: min.max(finite(options.max, 100.0)),
        format: options.format.clone(),
        locale: options.locale.clone(),
        get_value_label: options.get_value_label.clone(),
    }
}

/// The value if it's a finite number, else `fallback`.
pub fn finite(value: Option<f64>, fallback: f64) -> f64 {
    value.filter(|value| value.is_finite()).unwrap_or(fallback)
}

/// Where a value sits between the bounds, from 0 to 100, clamped.
pub fn percent_between(value: f64, min: f64, max: f64) -> f64 {
    if max <= min {
        return if value >= max { 100.0 } else { 0.0 };
    }
    ((clamp(value, min, max) - min) / (max - min)) * 100.0
}

/// The value as the Value part and screen readers show it, in the settings'
/// locale, else the app's. A percent style formats the share of the range,
/// so 30 of 0 to 60 reads 50%. Any other style formats the value as it is.
/// Empty while there is no value.
pub fn format_gauge_value(value: Option<f64>, settings: &GaugeSettings, cx: &App) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let locale = locale_of(settings.locale.as_ref(), cx);
    match &settings.format {
        Some(NumberFormat::Function(format)) => format(value),
        Some(format @ NumberFormat::Options(options)) if options.style != Some(NumberStyle::Percent) => {
            format_number(Some(value), Some(locale.as_ref()), Some(format))
        }
        format => {
            let percent = NumberFormat::Options(NumberFormatOptions::default().style(NumberStyle::Percent));
            format_number(
                Some(percent_between(value, settings.min, settings.max) / 100.0),
                Some(locale.as_ref()),
                Some(format.as_ref().unwrap_or(&percent)),
            )
        }
    }
}

// What screen readers announce: the custom label, or the formatted value.
fn gauge_value_text(value: Option<f64>, settings: &GaugeSettings, cx: &App) -> Option<String> {
    let formatted = format_gauge_value(value, settings, cx);
    let Some(get_value_label) = &settings.get_value_label else {
        return (!formatted.is_empty()).then_some(formatted);
    };
    let percent = value.map(|value| percent_between(value, settings.min, settings.max));
    Some(get_value_label(
        value,
        &GaugeLabelDetails {
            formatted,
            percent,
            min: settings.min,
            max: settings.max,
        },
    ))
}

/// The range ARIA of the root, which carries the role.
#[derive(Clone, Debug, PartialEq)]
pub struct GaugeAria {
    /// `aria-valuemin`.
    pub min: f64,
    /// `aria-valuemax`.
    pub max: f64,
    /// `aria-valuenow`, clamped to the bounds. `None` leaves it out, which is
    /// how a busy, indeterminate bar reads.
    pub now: Option<f64>,
    /// `aria-valuetext`: the custom label, or the formatted value.
    pub text: Option<String>,
}

impl GaugeAria {
    /// The ARIA gpui's setters leave out: `aria-valuetext`.
    pub fn aria(&self) -> Aria {
        match &self.text {
            Some(text) => Aria::new().value_text(text.clone()),
            None => Aria::new(),
        }
    }

    /// Puts the range ARIA on the root part: gpui's numeric minimum, maximum
    /// and value, and the value text.
    pub fn apply<S>(&self, part: Part<S>) -> Part<S> {
        let part = part
            .aria_min_numeric_value(self.min)
            .aria_max_numeric_value(self.max);
        let part = match self.now {
            Some(now) => part.aria_numeric_value(now),
            None => part,
        };
        part.aria(self.aria())
    }
}

/// The range ARIA of the root, which carries the role. A value of `None`
/// leaves `aria-valuenow` out, which is how a busy, indeterminate bar reads.
pub fn gauge_aria(value: Option<f64>, settings: &GaugeSettings, cx: &App) -> GaugeAria {
    GaugeAria {
        min: settings.min,
        max: settings.max,
        now: value.map(|value| clamp(value, settings.min, settings.max)),
        text: gauge_value_text(value, settings, cx),
    }
}

/// How the indicator fills the track: from the track's start, as wide as the value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IndicatorStyle {
    /// Its width, a fraction of the track's, from 0 to 1.
    pub width: f32,
}

impl IndicatorStyle {
    /// Places the indicator at the track's start, the right side when the
    /// layout runs right to left (`inset-inline-start: 0`), as wide as the
    /// value. Position it absolutely in the track, or let it flow.
    pub fn apply<E: Styled>(self, el: E, rtl: bool) -> E {
        let el = if rtl { el.right_0() } else { el.left_0() };
        el.w(relative(self.width))
    }
}

/// The indicator's placement for a percent, or none while there is no value.
pub fn indicator_style(percent: Option<f64>) -> Option<IndicatorStyle> {
    percent.map(|percent| IndicatorStyle {
        width: (round_percent(percent) / 100.0) as f32,
    })
}

/// A percent rounded to four decimals, as styles carry it.
pub fn round_percent(percent: f64) -> f64 {
    // `Math.round`, which rounds halves up.
    let scaled = percent * 1e4;
    let floor = scaled.floor();
    let rounded = if scaled - floor >= 0.5 { floor + 1.0 } else { floor };
    rounded / 1e4
}

/// The id a Label part takes, which the root's `aria-labelledby` points at:
/// the element id of the instance's `label` slot.
pub fn label_id(instance: &Instance) -> ElementId {
    instance.element_id("label", None)
}

/// What the root's `aria-labelledby` points at. Until the component's parts
/// have drawn, it points at a label wherever one renders; once they have,
/// only while a Label part is in the page, which the kernel's part registry
/// knows. Label parts name the root while any is on the page, which the
/// original counts by hand (`gaugeApi`).
pub fn labelled_by(instance: &Instance) -> Option<ElementId> {
    match flag_while_bound(instance, "label") {
        Some(false) => None,
        Some(true) | None => Some(label_id(instance)),
    }
}
