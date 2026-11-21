//! Port of `components/progress/progress.ts`: a task's progress, or an
//! indeterminate state while how far it has come is unknown.

use gpui::{App, Context, Entity, Role, SharedString, Window};
use ui_sdk_gauge::{
    GaugeLabelDetails, GaugeOptions, GaugeSettings, GetValueLabel, NumberFormat, finite,
    format_gauge_value, gauge_aria, gauge_settings, indicator_style, labelled_by, percent_between,
};
use ui_sdk_kernel::roles::Role as AriaRole;
use ui_sdk_kernel::part::with_role;
use ui_sdk_kernel::{Agent, AgentState, AgentValue, Aria, Instance, Part, dev, flag_while_bound, name_of};
use ui_sdk_platform::is_rtl;
use ui_sdk_text::{NameFrom, NameSources, check_name};

/// What `get_value_label` receives after the value.
pub type ProgressLabelDetails = GaugeLabelDetails;

/// Whether the task's progress is unknown, under way, or done.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ProgressStatus {
    /// `data-indeterminate`.
    #[default]
    Indeterminate,
    /// `data-progressing`.
    Progressing,
    /// `data-complete`.
    Complete,
}

impl ProgressStatus {
    /// The original's name for it, like `"progressing"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ProgressStatus::Indeterminate => "indeterminate",
            ProgressStatus::Progressing => "progressing",
            ProgressStatus::Complete => "complete",
        }
    }

    pub fn is_indeterminate(self) -> bool {
        self == ProgressStatus::Indeterminate
    }

    pub fn is_progressing(self) -> bool {
        self == ProgressStatus::Progressing
    }

    pub fn is_complete(self) -> bool {
        self == ProgressStatus::Complete
    }
}

/// The options `Progress` takes.
#[derive(Clone, Default)]
pub struct ProgressOptions {
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
    /// value says too little, like "3 of 12 files". It receives the value,
    /// `None` while the bar is indeterminate, and details: the `formatted`
    /// value, empty then, its `percent`, `None` then too, `min` and `max`.
    pub get_value_label: Option<GetValueLabel>,
    /// How far the task has come, between `min` and `max`. `None` means it's
    /// under way but how far is unknown, and the bar is indeterminate. A
    /// value that isn't finite reads as 0.
    pub value: Option<f64>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl ProgressOptions {
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
        label: impl Fn(Option<f64>, &ProgressLabelDetails) -> String + 'static,
    ) -> Self {
        self.get_value_label = Some(std::rc::Rc::new(label));
        self
    }

    /// The value, or `None` while how far the task has come is unknown.
    pub fn value(mut self, value: impl Into<Option<f64>>) -> Self {
        self.value = value.into();
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What every part's `with` and the app read. Every part carries the
/// status, so any of them can style it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgressState {
    /// The value, or `None` while indeterminate.
    pub value: Option<f64>,
    /// Where the value sits between `min` and `max`, from 0 to 100, or
    /// `None` while indeterminate. The original's `--progress-value`, for
    /// anything else that should follow the value, like a gradient stop.
    pub percent: Option<f64>,
    /// Whether the progress is unknown, under way, or done.
    pub status: ProgressStatus,
}

/// What the Value part's `with` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressValueState {
    /// The value, or `None` while indeterminate.
    pub value: Option<f64>,
    /// Where the value sits between `min` and `max`, from 0 to 100, or
    /// `None` while indeterminate.
    pub percent: Option<f64>,
    /// Whether the progress is unknown, under way, or done.
    pub status: ProgressStatus,
    /// The formatted value, like "40%". Empty while indeterminate.
    pub text: SharedString,
}

/// A progress bar. Its entity is its controller; it has no api of its own,
/// since its value is the task's progress the app passes.
pub struct Progress {
    instance: Instance,
    settings: GaugeSettings,
    value: Option<f64>,
    // Whether development builds checked that something names the root.
    name_checked: bool,
}

// No value is an indeterminate bar; a value that isn't finite reads as 0.
fn value_of(options: &ProgressOptions) -> Option<f64> {
    options.value.map(|value| finite(Some(value), 0.0))
}

fn settings(options: &ProgressOptions) -> GaugeSettings {
    gauge_settings(&GaugeOptions {
        min: options.min,
        max: options.max,
        format: options.format.clone(),
        locale: options.locale.clone(),
        get_value_label: options.get_value_label.clone(),
    })
}

// Whether two settings render the same. A function in the options, which an
// app makes anew each time it renders, counts as the same function, so
// handing options over every frame doesn't render again for nothing.
fn renders_same(a: &GaugeSettings, b: &GaugeSettings) -> bool {
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

impl Progress {
    pub fn new(options: ProgressOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("progress").with_agent(cx);
        instance.debug = options.debug;
        Self {
            instance,
            settings: settings(&options),
            value: value_of(&options),
            name_checked: false,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). Pass the latest `value` as the task goes on: a progress bar
    /// shows the app's state and has no `on_value_change`.
    pub fn set_options(&mut self, options: ProgressOptions, cx: &mut Context<Self>) {
        let settings = settings(&options);
        let value = value_of(&options);
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

    /// Where the value sits between `min` and `max`, from 0 to 100, or
    /// `None` while indeterminate.
    pub fn percent(&self) -> Option<f64> {
        self.value
            .map(|value| percent_between(value, self.settings.min, self.settings.max))
    }

    /// Whether the progress is unknown, under way, or done.
    pub fn status(&self) -> ProgressStatus {
        match self.value {
            None => ProgressStatus::Indeterminate,
            Some(value) if value >= self.settings.max => ProgressStatus::Complete,
            Some(_) => ProgressStatus::Progressing,
        }
    }

    pub fn state(&self) -> ProgressState {
        ProgressState {
            value: self.value,
            percent: self.percent(),
            status: self.status(),
        }
    }

    /// The value as the Value part and screen readers show it, like "40%".
    /// Empty while indeterminate.
    pub fn text(&self, cx: &App) -> SharedString {
        format_gauge_value(self.value, &self.settings, cx).into()
    }

    /// The `root` part: the progress bar, with its range and value for
    /// screen readers. No value leaves the value out, which is how a busy,
    /// indeterminate bar reads. A bar's content never names it, so only a
    /// Label part or the app's `aria_label` does.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<ProgressState> {
        let progress = this.read(cx);
        let aria = gauge_aria(progress.value, &progress.settings, cx);
        let labels = match labelled_by(&progress.instance) {
            Some(label) => Aria::new().labelled_by(label),
            None => Aria::new(),
        };
        let mut part = Part::new(&progress.instance, "root", None, progress.state())
            .aria_role(AriaRole::ProgressBar);
        // Development builds check that something names it, once its
        // labelled-by knows whether a Label part is in the page.
        if dev::DEV && !progress.name_checked && flag_while_bound(&progress.instance, "label").is_some() {
            let entity = this.downgrade();
            part = part.keep_name().on_props(move |window, cx| {
                entity.update(cx, |progress, _| progress.check_name(window)).ok();
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
        let mut sources = NameSources::new(AriaRole::ProgressBar);
        if let Some(label) = name.aria_label {
            sources = sources.aria_label(label);
        }
        let from = NameFrom {
            component: "progress",
            id: Some(self.instance.id().clone()),
        };
        check_name(&self.instance.element_id("root", None), &sources, "Progress", from, window);
    }

    /// The `track` part, which gives the bar its size.
    pub fn track(this: &Entity<Self>, cx: &App) -> Part<ProgressState> {
        let progress = this.read(cx);
        Part::new(&progress.instance, "track", None, progress.state())
    }

    /// The `indicator` part, which fills the track: placed from the track's
    /// start, the right side right to left, as wide as the value. While
    /// indeterminate it has no width, for the app to style. Position it
    /// absolutely in the track, or let it flow.
    pub fn indicator(this: &Entity<Self>, cx: &App) -> Part<ProgressState> {
        let progress = this.read(cx);
        let state = progress.state();
        let rtl = is_rtl(cx);
        let part = Part::new(&progress.instance, "indicator", None, state);
        match indicator_style(state.percent) {
            Some(style) => style.apply(part, rtl),
            None => part,
        }
    }

    /// The `label` part, which names the bar while it's in the page. Give it
    /// its text as an `aria_label`, or as accessible text, since a div's
    /// plain text children don't name it in gpui.
    pub fn label(this: &Entity<Self>, cx: &App) -> Part<ProgressState> {
        let progress = this.read(cx);
        // A role of its own, so the root's labelled-by finds it.
        Part::new(&progress.instance, "label", None, progress.state())
            .map_el(|el| with_role(el, Role::Label))
    }

    /// The `value` part: the formatted value. Its text isn't announced on
    /// its own: the bar reads the value through its value text.
    pub fn value(this: &Entity<Self>, cx: &App) -> Part<ProgressValueState> {
        let progress = this.read(cx);
        let state = ProgressValueState {
            value: progress.value,
            percent: progress.percent(),
            status: progress.status(),
            text: progress.text(cx),
        };
        Part::new(&progress.instance, "value", None, state)
    }
}

/// `controlled: []`: `value` is a reading, not state a user changes, so it
/// has no actions. The bridge shows the reading nothing sets, `null` while
/// indeterminate, and its bounds.
impl Agent for Progress {
    const CONTROLLED: Option<&'static [&'static str]> = Some(&[]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", self.value.map_or(AgentValue::Null, AgentValue::from))
            .with("min", self.settings.min)
            .with("max", self.settings.max)
    }
}
