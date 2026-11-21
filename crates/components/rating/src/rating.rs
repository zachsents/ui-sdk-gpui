//! Port of `components/rating/rating.ts`: a score picked from a row of
//! stars, a radio per star or per part of one, with a preview while a
//! pointer moves over them.
//!
//! A rating is a radio group: it keys its radios by score as text, so
//! Radio's api and parts run it unchanged, and turns the keys back into
//! scores for the app.

use std::rc::Rc;

use gpui::{
    App, Context, Entity, InteractiveElement, SharedString, StatefulInteractiveElement, Styled,
    Window, relative,
};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, AgentSurface, Aria, ChangeCallback, ChangeDetails,
    ChangeOptions, Instance, Need, Part, Reason,
};
use ui_sdk_platform::is_rtl;
use ui_sdk_radio::{
    RadioGroupApi, RadioGroupOptions, RadioHost, RadioProps, RadioSlotNames, RadioState,
    bind_radio_group, radio_group_attrs, radio_item,
};

/// Why a rating's value wants to change: a star's radio was pressed
/// (`ItemPress`), an arrow key moved to one (`ListNavigation`), its form
/// reset (`Reset`), or a call (`Imperative`).
pub type RatingValueChangeReason = Reason;

/// Options for every write method.
pub type RatingChangeOptions = ChangeOptions;

/// `get_value_label`: names a score, given the score and `max`.
pub type ValueLabelFn = Rc<dyn Fn(f64, usize) -> SharedString>;

/// The options `Rating` takes.
#[derive(Clone)]
pub struct RatingOptions {
    /// The score. Pass it to control the rating. `Some(None)` means no score
    /// yet.
    pub value: Option<Option<f64>>,
    /// The score at first, when uncontrolled. A form reset returns to it.
    pub default_value: Option<Option<f64>>,
    /// Runs when a user or a form reset wants to change the score.
    /// `details.reason` says why (`ItemPress`, `ListNavigation`, `Reset`,
    /// `Imperative`), and `details.cancel()` stops it. It runs inside the
    /// rating's update, so it may update any entity but the rating.
    pub on_value_change: Option<ChangeCallback<Option<f64>>>,
    /// How many stars there are. Defaults to 5.
    pub max: usize,
    /// The smallest change, as a fraction of a star: `0.5` makes half stars,
    /// each half its own radio. Defaults to 1.
    pub step: f64,
    /// Lets a press on the checked star clear the score.
    pub deselectable: bool,
    /// Disables every star: they can't be focused or changed, and the score
    /// isn't submitted.
    pub disabled: bool,
    /// Shows the score as an image, named by `get_value_label`, that can't be
    /// changed. Any score shows, like an average of 4.2.
    pub read_only: bool,
    /// Blocks form submission until there's a score, with the browser's own
    /// message.
    pub required: bool,
    /// The name the score submits under. Without one, the rating submits
    /// nothing.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when the rating isn't inside that form.
    pub form: Option<SharedString>,
    /// Whether arrow keys wrap from the last star to the first. Defaults to
    /// `false`.
    pub r#loop: bool,
    /// Names a score, for each star's radio and for a read-only rating. By
    /// default "1 star", or "3 stars".
    pub get_value_label: Option<ValueLabelFn>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for RatingOptions {
    fn default() -> Self {
        Self {
            value: None,
            default_value: None,
            on_value_change: None,
            max: 5,
            step: 1.0,
            deselectable: false,
            disabled: false,
            read_only: false,
            required: false,
            name: None,
            form: None,
            r#loop: false,
            get_value_label: None,
            debug: false,
        }
    }
}

impl RatingOptions {
    pub fn value(mut self, value: Option<f64>) -> Self {
        self.value = Some(value);
        self
    }

    pub fn default_value(mut self, value: Option<f64>) -> Self {
        self.default_value = Some(value);
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&Option<f64>, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn max(mut self, max: usize) -> Self {
        self.max = max;
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    pub fn deselectable(mut self, deselectable: bool) -> Self {
        self.deselectable = deselectable;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn form(mut self, form: impl Into<SharedString>) -> Self {
        self.form = Some(form.into());
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = r#loop;
        self
    }

    pub fn get_value_label(mut self, label: impl Fn(f64, usize) -> SharedString + 'static) -> Self {
        self.get_value_label = Some(Rc::new(label));
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The state the `root` part hands `with`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RatingState {
    /// The score, or `None` before anyone rates.
    pub value: Option<f64>,
    /// The score a pointer previews over the stars, or `None` while none
    /// does.
    pub highlighted: Option<f64>,
    /// Whether the rating is disabled, by itself or by a fieldset.
    pub disabled: bool,
    /// Whether the rating only shows its score.
    pub read_only: bool,
    /// Whether a form won't submit until there's a score.
    pub required: bool,
}

/// The state a star hands `with`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RatingItemState {
    /// Which star this is, from 1.
    pub index: usize,
    /// How much of this star the shown score covers, from 0 to 1, which the
    /// original's `--rating-fill` carries.
    pub fill: f64,
    /// Whether the shown score covers the whole star (`data-filled`).
    pub filled: bool,
    /// Whether the shown score ends partway through the star
    /// (`data-partial`).
    pub partial: bool,
    /// Whether the score ends in this star.
    pub checked: bool,
    /// Whether a pointer previews a score that reaches this star.
    pub highlighted: bool,
    /// Whether the rating is disabled, by itself or by a fieldset.
    pub disabled: bool,
    /// Whether the rating only shows its score.
    pub read_only: bool,
}

const SLOT_NAMES: RadioSlotNames = RadioSlotNames {
    group: "root",
    item: "radio",
};

fn default_label(value: f64) -> SharedString {
    if value == 1.0 {
        "1 star".into()
    } else {
        format!("{value} stars").into()
    }
}

// A score as the text its radio is keyed by, like `String(value)`.
fn key_of(value: Option<f64>) -> Option<SharedString> {
    value.map(|value| SharedString::from(value.to_string()))
}

fn score_of(key: Option<&SharedString>) -> Option<f64> {
    key.map(|key| key.parse().unwrap_or(f64::NAN))
}

// `Number(x.toFixed(4))`.
fn to_fixed4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// The scores of the radios inside star `index`, lowest first: one per
/// star, or one per part of it with a `step` below 1.
pub fn rating_item_values(index: usize, step: f64) -> Vec<f64> {
    let parts = parts_per_star(step);
    (0..parts)
        .map(|part| to_fixed4(index as f64 - 1.0 + (part + 1) as f64 / parts as f64))
        .collect()
}

// A step that makes no parts, like 0, gives one radio per star.
fn parts_per_star(step: f64) -> usize {
    let parts = (1.0 / step).round();
    if parts.is_finite() { parts.max(1.0) as usize } else { 1 }
}

// Rating keys its radios by score as text, so Radio's api and parts run it
// unchanged.
fn radio_options(options: &RatingOptions) -> RadioGroupOptions {
    let on_value_change = options.on_value_change.clone().map(|callback| -> ChangeCallback<Option<SharedString>> {
        Rc::new(move |key: &Option<SharedString>, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
            callback(&score_of(key.as_ref()), details, window, cx)
        })
    });
    RadioGroupOptions {
        value: options.value.map(key_of),
        default_value: options.default_value.map(key_of),
        on_value_change,
        disabled: options.disabled,
        read_only: options.read_only,
        required: options.required,
        name: options.name.clone(),
        form: options.form.clone(),
        r#loop: options.r#loop,
        debug: options.debug,
        ..RadioGroupOptions::default()
    }
}

/// A rating (`rating`). Its entity is its controller.
pub struct Rating {
    radio: RadioGroupApi,
    max: usize,
    step: f64,
    deselectable: bool,
    // Labels live in state so a new `get_value_label` or `max` renames the
    // stars.
    get_value_label: Option<ValueLabelFn>,
}

impl RadioHost for Rating {
    fn radio(&self) -> &RadioGroupApi {
        &self.radio
    }

    fn radio_mut(&mut self) -> &mut RadioGroupApi {
        &mut self.radio
    }

    // A press on the checked star clears the score when it's deselectable.
    fn check(
        &mut self,
        key: SharedString,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let checked = self.radio.get_value().as_ref() == Some(&key);
        let next = if checked && self.deselectable { None } else { Some(key) };
        self.set_radio_value(next, options, window, cx)
    }
}

impl Rating {
    pub fn new(options: RatingOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("rating").with_agent(cx);
        instance.debug = options.debug;
        let radio = RadioGroupApi::new(instance, &radio_options(&options), SLOT_NAMES, window, cx);
        Self {
            radio,
            max: options.max,
            step: options.step,
            deselectable: options.deselectable,
            get_value_label: options.get_value_label,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: RatingOptions, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = self.radio.set_options(&radio_options(&options));
        let same_label = match (&self.get_value_label, &options.get_value_label) {
            (Some(was), Some(now)) => Rc::ptr_eq(was, now),
            (None, None) => true,
            _ => false,
        };
        changed |= self.max != options.max || self.step != options.step || !same_label;
        self.max = options.max;
        self.step = options.step;
        self.deselectable = options.deselectable;
        self.get_value_label = options.get_value_label;
        self.sync_radio(window, cx);
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled score, `Some(None)` for no score, or `None` to
    /// let the rating keep its own.
    pub fn set_controlled_value(&mut self, value: Option<Option<f64>>, window: &mut Window, cx: &mut Context<Self>) {
        if self.radio.set_controlled_value(value.map(key_of)) {
            cx.notify();
        }
        self.sync_radio(window, cx);
    }

    pub fn instance(&self) -> &Instance {
        self.radio.instance()
    }

    /// The rating's radio group, which its parts run on.
    pub fn api(&self) -> &RadioGroupApi {
        &self.radio
    }

    /// What the root part's state holds.
    pub fn state(&self) -> RatingState {
        RatingState {
            value: self.get_value(),
            highlighted: score_of(self.radio.highlighted()),
            disabled: self.radio.is_disabled(),
            read_only: self.radio.is_read_only(),
            required: self.radio.is_required(),
        }
    }

    /// The score, or `None` before anyone rates.
    pub fn get_value(&self) -> Option<f64> {
        score_of(self.radio.get_value().as_ref())
    }

    /// Sets the score, as a user would, or with `None` clears it. Disabled
    /// and read-only ratings ignore it. Returns false when canceled.
    pub fn set_value(
        &mut self,
        value: Option<f64>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.set_radio_value(key_of(value), options, window, cx)
    }

    /// Moves focus to the checked star's radio, or the first one.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.radio.focus(window, cx);
    }

    /// How many stars there are.
    pub fn max(&self) -> usize {
        self.max
    }

    /// How many radios split each star.
    pub fn parts_per_star(&self) -> usize {
        parts_per_star(self.step)
    }

    /// The scores of the radios inside star `index`, lowest first, which
    /// [`Rating::radio`] renders.
    pub fn values(&self, index: usize) -> Vec<f64> {
        rating_item_values(index, self.step)
    }

    // A score's name.
    fn label_of(&self, value: f64) -> SharedString {
        match &self.get_value_label {
            Some(label) => label(value, self.max),
            None => default_label(value),
        }
    }

    fn interactive(&self) -> bool {
        !self.radio.is_disabled() && !self.radio.is_read_only()
    }

    fn highlight(&mut self, key: Option<SharedString>, cx: &mut Context<Self>) {
        if self.radio.set_highlighted(key) {
            cx.notify();
        }
    }

    // How much of star `index` the shown score covers: the previewed one, or
    // the score.
    fn fill_of(&self, index: usize) -> f64 {
        let shown = score_of(self.radio.highlighted())
            .or_else(|| self.get_value())
            .unwrap_or(0.0);
        to_fixed4(shown - (index as f64 - 1.0)).clamp(0.0, 1.0)
    }

    /// The state star `index` renders with.
    pub fn item_state(&self, index: usize) -> RatingItemState {
        let highlighted = score_of(self.radio.highlighted());
        let fill = self.fill_of(index);
        RatingItemState {
            index,
            fill,
            filled: fill == 1.0,
            partial: fill > 0.0 && fill < 1.0,
            checked: self
                .get_value()
                .is_some_and(|value| value.ceil() == index as f64),
            highlighted: highlighted.is_some_and(|highlighted| index as f64 <= highlighted.ceil()),
            disabled: self.radio.is_disabled(),
            read_only: self.radio.is_read_only(),
        }
    }

    /// The `root` part, around the stars: a radio group, or with `read_only`
    /// one image named by `get_value_label`, with the rating's form side.
    /// Build it before the stars: it starts their radios' render, so the
    /// order they render in is page order. It takes `on_hover`, which ends a
    /// preview as the pointer leaves the stars.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<RatingState> {
        let rating = this.read(cx);
        let radio = &rating.radio;
        let state = rating.state();
        let part = Part::new(radio.instance(), SLOT_NAMES.group, None, state)
            .aria_role(if state.read_only { Role::Img } else { Role::RadioGroup })
            .unneeded(Need::Keys, "Focus stays on a star's radio, whose keydown handles them.");
        let part = if state.read_only {
            // A read-only rating is one image, which names the score itself.
            radio.sync_form();
            part.map_el(|el| radio.track(el))
                .aria_label(rating.label_of(state.value.unwrap_or(0.0)))
        } else {
            bind_radio_group(radio, radio_group_attrs(radio, part))
        };
        radio.begin(cx);
        let entity = this.downgrade();
        part.on_hover(move |hovered, _window, cx| {
            if !*hovered {
                entity.update(cx, |rating, cx| rating.highlight(None, cx)).ok();
            }
        })
    }

    /// The `item` part for star `index`, from 1: put its icon inside, styled
    /// from how full the state says it is, and the radios that pick its
    /// scores, [`Rating::radio`] for each of [`Rating::values`], which sit
    /// over the icon, so it needs no events of its own.
    pub fn item(this: &Entity<Self>, index: usize, cx: &App) -> Part<RatingItemState> {
        let rating = this.read(cx);
        let state = rating.item_state(index);
        Part::new(rating.radio.instance(), "item", Some(&index.to_string()), state).relative()
    }

    /// The `radio` part for the score `value`, inside its star: a radio
    /// named by `get_value_label`, which covers its share of the star. A
    /// pointer moving over it previews its score.
    pub fn radio(this: &Entity<Self>, value: f64, cx: &App) -> Part<RadioState> {
        let rating = this.read(cx);
        let key = SharedString::from(value.to_string());
        let parts = rating.parts_per_star() as f64;
        let share = ((value - value.ceil() + 1.0) * parts).round();
        let start = relative(((share - 1.0) / parts) as f32);
        let read_only = rating.radio.is_read_only();
        let handle = rating.radio.roving().focus_handle(&key, cx);
        // The radios split their star between them, under its icon.
        let part = radio_item(this, key.clone(), RadioProps::default(), cx)
            .aria_label(rating.label_of(value))
            .absolute()
            .top_0()
            .bottom_0()
            .w(relative((1.0 / parts) as f32));
        let mut part = if is_rtl(cx) { part.right(start) } else { part.left(start) };
        if read_only {
            // A read-only rating is one image, which names the score itself.
            part = part
                .aria(Aria::new().hidden(true))
                .focus_mode(FocusMode::Roving { tab_stop: false }, Some(&handle), false);
        }
        let entity = this.downgrade();
        part.on_mouse_move(move |_, _window, cx| {
            entity
                .update(cx, |rating, cx| {
                    if !rating.interactive() || rating.radio.item_props(&key).disabled {
                        return;
                    }
                    rating.highlight(Some(key.clone()), cx);
                })
                .ok();
        })
    }
}

/// `agent: { actions: ["setValue"], closed: true }`.
impl Agent for Rating {
    const SURFACE: AgentSurface = AgentSurface::new().closed(true);

    const ACTIONS: &'static [AgentAction<Self>] = &[AgentAction::new("setValue", |rating, args, window, cx| {
        let value = if args.is_missing(0) {
            None
        } else {
            Some(args.number(0)?)
        };
        Ok(rating.set_value(value, ChangeOptions::default(), window, cx).into())
    })];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", self.get_value())
            .with("disabled", self.radio.is_disabled())
            .with("readOnly", self.radio.is_read_only())
            .with("required", self.radio.is_required())
            .with("max", self.max)
    }
}
