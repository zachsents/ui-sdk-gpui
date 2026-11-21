//! Port of `components/meter/meter.ts`: a measurement within a known range,
//! like storage used, a battery level or an account quota. It fills a track
//! to show the value, and screen readers hear the same value.

use std::rc::Rc;

use gpui::{App, Context, Entity, Role, SharedString, Window};
use ui_sdk_gauge::{
    GaugeLabelDetails, GaugeOptions, GaugeSettings, GetValueLabel, NumberFormat, finite,
    format_gauge_value, gauge_aria, gauge_settings, indicator_style, labelled_by, percent_between,
};
use ui_sdk_kernel::roles::Role as AriaRole;
use ui_sdk_kernel::part::with_role;
use ui_sdk_kernel::{Agent, AgentState, Aria, Instance, Part, dev, flag_while_bound, name_of};
use ui_sdk_platform::is_rtl;
use ui_sdk_text::{NameFrom, NameSources, check_name};

/// What `get_value_label` receives after the value.
pub type MeterLabelDetails = GaugeLabelDetails;

/// The options `Meter` takes.
#[derive(Clone, Default)]
pub struct MeterOptions {
    /// The lowest value. 0 by default.
    pub min: Option<f64>,
    /// The highest value. 100 by default. A maximum below the minimum is the
    /// minimum.
    pub max: Option<f64>,
    /// How the value reads, for screen readers and the Value part: number
    /// format options, or a function of the value. A percent style shows
    /// where the value sits between `min` and `max`; any other style shows
    /// the value itself, like a unit style with `"gigabyte"`. A percent
    /// style by default.
    pub format: Option<NumberFormat>,
    /// The locale the value is formatted in. The app's language by default
    /// (`ui_sdk_text::page_locale`).
    pub locale: Option<SharedString>,
    /// The text screen readers announce for the value, when the formatted
    /// value says too little, like "750 of 1000 requests". It receives the
    /// value, which a meter always has, and details: the `formatted` value,
    /// its `percent`, `min` and `max`.
    pub get_value_label: Option<GetValueLabel>,
    /// The reading, between `min` and `max`. Outside them, the fill, the
    /// percentage and the value screen readers hear are clamped, while the
    /// state keeps the value as given. A meter always has a value, since
    /// there's no busy state: a missing or non-finite one reads as the
    /// minimum, an empty meter.
    pub value: Option<f64>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl MeterOptions {
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

    /// The value is never missing, so the label gets a number, and the
    /// settings Progress shares take it as their own.
    pub fn get_value_label(
        mut self,
        label: impl Fn(f64, &MeterLabelDetails) -> String + 'static,
    ) -> Self {
        self.get_value_label = Some(Rc::new(move |value: Option<f64>, details: &MeterLabelDetails| {
            label(value.unwrap_or(details.min), details)
        }));
        self
    }

    pub fn value(mut self, value: f64) -> Self {
        self.value = Some(value);
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What every part's `with` and the app read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterState {
    /// The value, as given.
    pub value: f64,
    /// Where the value sits between `min` and `max`, from 0 to 100. The
    /// original's `--meter-value`, for anything else that should follow the
    /// value, like a ring's sweep.
    pub percent: f64,
}

/// What the Value part's `with` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterValueState {
    /// The value, as given.
    pub value: f64,
    /// Where the value sits between `min` and `max`, from 0 to 100.
    pub percent: f64,
    /// The formatted value, like "40%".
    pub text: SharedString,
}

/// A meter. Its entity is its controller; it has no api of its own, since
/// its value is a reading the app passes, not state a user changes.
pub struct Meter {
    instance: Instance,
    settings: GaugeSettings,
    value: f64,
    // Whether development builds checked that something names the root.
    name_checked: bool,
}

// The settings from the options. A missing or non-finite value reads as the
// minimum: an empty meter.
fn settings(options: &MeterOptions) -> (GaugeSettings, f64) {
    let shared = gauge_settings(&GaugeOptions {
        min: options.min,
        max: options.max,
        format: options.format.clone(),
        locale: options.locale.clone(),
        get_value_label: options.get_value_label.clone(),
    });
    let value = finite(options.value, shared.min);
    (shared, value)
}

// Whether two settings render the same. A function in the options, which an
// app makes anew each time it renders, counts as the same function, so
// handing options over every frame doesn't render again for nothing.
pub(crate) fn renders_same(a: &GaugeSettings, b: &GaugeSettings) -> bool {
    let format = match (&a.format, &b.format) {
        (Some(NumberFormat::Function(_)), Some(NumberFormat::Function(_))) => true,
        (a, b) => a == b,
    };
    a.min == b.min
        && a.max == b.max
        && a.locale == b.locale
        && format
        && a.get_value_label.is_some() == b.get_value_label.is_some()
}

impl Meter {
    pub fn new(options: MeterOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (settings, value) = settings(&options);
        let mut instance = Instance::new("meter").with_agent(cx);
        instance.debug = options.debug;
        Self {
            instance,
            settings,
            value,
            name_checked: false,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). Pass a new `value` when the reading changes: a meter follows
    /// the app's state and has no `on_value_change`.
    pub fn set_options(&mut self, options: MeterOptions, cx: &mut Context<Self>) {
        let (settings, value) = settings(&options);
        let changed = !renders_same(&self.settings, &settings) || self.value != value;
        self.settings = settings;
        self.value = value;
        self.instance.debug = options.debug;
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The bounds, format and locale the parts render from.
    pub fn settings(&self) -> &GaugeSettings {
        &self.settings
    }

    /// Where the value sits between `min` and `max`, from 0 to 100.
    pub fn percent(&self) -> f64 {
        percent_between(self.value, self.settings.min, self.settings.max)
    }

    pub fn state(&self) -> MeterState {
        MeterState {
            value: self.value,
            percent: self.percent(),
        }
    }

    /// The value as the Value part and screen readers show it, like "40%".
    pub fn text(&self, cx: &App) -> SharedString {
        format_gauge_value(Some(self.value), &self.settings, cx).into()
    }

    /// The `root` part: the meter, with its range and value for screen
    /// readers. A meter's content never names it, so only a Label part or
    /// the app's `aria_label` does.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<MeterState> {
        let meter = this.read(cx);
        let aria = gauge_aria(Some(meter.value), &meter.settings, cx);
        let labels = match labelled_by(&meter.instance) {
            Some(label) => Aria::new().labelled_by(label),
            None => Aria::new(),
        };
        let mut part = Part::new(&meter.instance, "root", None, meter.state()).aria_role(AriaRole::Meter);
        // Development builds check that something names it, once its
        // labelled-by knows whether a Label part is in the page.
        if dev::DEV && !meter.name_checked && flag_while_bound(&meter.instance, "label").is_some() {
            let entity = this.downgrade();
            part = part.keep_name().on_props(move |window, cx| {
                entity.update(cx, |meter, _| meter.check_name(window)).ok();
            });
        }
        aria.apply(part).aria(labels)
    }

    // The root rendered: development builds check, once, that something
    // names it (`checkName`), from the name it rendered with. A labelled-by
    // names it, since its targets' text can't be read.
    fn check_name(&mut self, window: &Window) {
        if self.name_checked {
            return;
        }
        let Some(name) = name_of(&self.instance, "root", None) else {
            return;
        };
        self.name_checked = true;
        if !name.labelled_by.is_empty() {
            return;
        }
        let mut sources = NameSources::new(AriaRole::Meter);
        if let Some(label) = name.aria_label {
            sources = sources.aria_label(label);
        }
        let from = NameFrom {
            component: "meter",
            id: Some(self.instance.id().clone()),
        };
        check_name(&self.instance.element_id("root", None), &sources, "Meter", from, window);
    }

    /// The `track` part, which gives the bar its size.
    pub fn track(this: &Entity<Self>, cx: &App) -> Part<MeterState> {
        let meter = this.read(cx);
        Part::new(&meter.instance, "track", None, meter.state())
    }

    /// The `indicator` part, which fills the track: placed from the track's
    /// start, the right side right to left, as wide as the value. Position it
    /// absolutely in the track, or let it flow.
    pub fn indicator(this: &Entity<Self>, cx: &App) -> Part<MeterState> {
        let meter = this.read(cx);
        let state = meter.state();
        let rtl = is_rtl(cx);
        let part = Part::new(&meter.instance, "indicator", None, state);
        match indicator_style(Some(state.percent)) {
            Some(style) => style.apply(part, rtl),
            None => part,
        }
    }

    /// The `label` part, which names the meter while it's in the page. Give
    /// it its text as an `aria_label`, or as accessible text, since a div's
    /// plain text children don't name it in gpui.
    pub fn label(this: &Entity<Self>, cx: &App) -> Part<MeterState> {
        let meter = this.read(cx);
        // A role of its own, so the root's labelled-by finds it.
        Part::new(&meter.instance, "label", None, meter.state()).map_el(|el| with_role(el, Role::Label))
    }

    /// The `value` part: the formatted value. Its text isn't announced on
    /// its own: the meter reads the value through its value text.
    pub fn value(this: &Entity<Self>, cx: &App) -> Part<MeterValueState> {
        let meter = this.read(cx);
        let state = MeterValueState {
            value: meter.value,
            percent: meter.percent(),
            text: meter.text(cx),
        };
        Part::new(&meter.instance, "value", None, state)
    }
}

/// `controlled: []`: `value` is a reading, not state a user changes, so it
/// has no actions. The bridge shows the reading nothing sets, and its bounds.
impl Agent for Meter {
    const CONTROLLED: Option<&'static [&'static str]> = Some(&[]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", self.value)
            .with("min", self.settings.min)
            .with("max", self.settings.max)
    }
}
