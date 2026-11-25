//! Port of `components/color-picker/color-picker.ts`: a color picker around
//! one value written as a CSS color. This module is the public api and the
//! component; its state and what it does are `color_picker_api.rs`'s, and
//! its parts `color_picker_parts.rs`'s.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, Hsla, SharedString, Subscription, Window,
};
use ui_sdk_color::{Color, ColorFormat, ColorGamut};
use ui_sdk_direct_manipulation::Area;
use ui_sdk_drag_and_drop::{DragSession, create_drag_session};
use ui_sdk_focus::{RovingFocus, RovingFocusMoveContext, RovingFocusOptions, move_index, radio_step};
use ui_sdk_forms::{Association, ElementInternals, associate, focus_with};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentState, ChangeDetails, ChangeOptions, Controllable, Instance, Key, Parts, Reason,
    track_input,
};
use ui_sdk_native::{TextField, TextFieldEvent};

use crate::color_picker_api::{Settings, settings};
use crate::color_picker_channels::{
    AreaGradient, ChannelGradient, ColorChannel, ColorPickerSpace, black, color_in, in_space,
};
use crate::color_picker_parts::AreaCanvas;

/// Why a color picker's value wants to change: a press on the area's surface
/// or a slider's track (`TrackPress`), a drag (`Drag`), a key (`StepKey`),
/// a color typed into the input or a number into a channel input
/// (`InputCommit`), a preset (`ItemPress`), a form reset (`Reset`), or a
/// call (`Imperative`).
pub type ColorPickerValueChangeReason = Reason;

/// What `on_value_change` and `on_value_commit` receive after the value: the
/// reason, source and event, with `cancel()`, as every change's details
/// have, and the color. A commit ignores `cancel()`.
pub struct ColorPickerValueChangeDetails<'a> {
    details: &'a ChangeDetails,
    /// The value as a color, in the picker's space, ready for
    /// `ui_sdk_color`. It keeps a hue while the value turns gray.
    pub color: Color,
}

impl<'a> ColorPickerValueChangeDetails<'a> {
    pub(crate) fn new(details: &'a ChangeDetails, color: Color) -> Self {
        Self { details, color }
    }

    /// The change's own details, with its reason, source and event.
    pub fn details(&self) -> &ChangeDetails {
        self.details
    }
}

impl Deref for ColorPickerValueChangeDetails<'_> {
    type Target = ChangeDetails;

    fn deref(&self) -> &ChangeDetails {
        self.details
    }
}

impl fmt::Debug for ColorPickerValueChangeDetails<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ColorPickerValueChangeDetails")
            .field("details", self.details)
            .field("color", &self.color)
            .finish()
    }
}

/// What `on_value_commit` receives after the value.
pub type ColorPickerValueCommitDetails<'a> = ColorPickerValueChangeDetails<'a>;

/// `on_value_change`: the requested value and what explains it.
pub type ColorPickerValueChange =
    Rc<dyn Fn(&SharedString, &ColorPickerValueChangeDetails<'_>, &mut Window, &mut App)>;

/// `on_value_commit`: the committed value and what explains it.
pub type ColorPickerValueCommit =
    Rc<dyn Fn(&SharedString, &ColorPickerValueCommitDetails<'_>, &mut Window, &mut App)>;

/// Options for every write method: the reason and event `on_value_change`
/// receives.
pub type ColorPickerChangeOptions = ChangeOptions;

/// The options `ColorPicker` takes.
#[derive(Clone)]
pub struct ColorPickerOptions {
    /// The value, as a CSS color, like `"#0ea5e9"` or `"oklch(0.62 0.19
    /// 259)"`. Pass it to control the picker.
    pub value: Option<SharedString>,
    /// The value at first, when uncontrolled. A form reset returns to it.
    /// Black by default.
    pub default_value: Option<SharedString>,
    /// Runs while the value changes: on every step of a drag or a key.
    /// `details.color` is the color. It runs once the picker's update is
    /// over, so it may update any entity, the picker too.
    pub on_value_change: Option<ColorPickerValueChange>,
    /// Runs as the value is asked to change, before it does, inside the
    /// picker's update, with the color in `details.color`: `details.cancel()`
    /// stops it. It may update any entity but the picker.
    pub on_before_value_change: Option<ColorPickerValueChange>,
    /// Runs once an interaction that changed the value ends: the pointer
    /// lifts, a key is pressed, a swatch is picked. For saving. It runs once
    /// the picker's update is over, so it may update any entity, the picker
    /// too.
    pub on_value_commit: Option<ColorPickerValueCommit>,
    /// The space the parts edit in, which decides their channels: `Hsv` has
    /// hue, saturation and brightness, `Hsl` hue, saturation and lightness,
    /// `Srgb` red, green and blue, `Oklch` lightness, chroma and hue. `Hsv`
    /// by default.
    pub space: ColorPickerSpace,
    /// How the value is written: hex, or a space in its CSS syntax. Hex for
    /// `Hsv`, which CSS can't write, and the space otherwise, by default.
    pub format: Option<ColorFormat>,
    /// The colors the value can hold. A drag can go past the edge, and the
    /// value is the closest color inside it; once the drag ends, the picker
    /// moves there too. `P3` allows wide-gamut screens' colors, and `None`
    /// any color. sRGB by default.
    pub gamut: Option<ColorGamut>,
    /// Disables every part: they can't be focused or changed, and the value
    /// isn't submitted.
    pub disabled: bool,
    /// Keeps the parts focusable and the value submitted, but stops changes.
    pub read_only: bool,
    /// Marks it required. A color picker always has a value, so it never
    /// blocks a form.
    pub required: bool,
    /// The name the value submits under. Without one, the picker submits
    /// nothing.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for ColorPickerOptions {
    fn default() -> Self {
        Self {
            value: None,
            default_value: None,
            on_value_change: None,
            on_before_value_change: None,
            on_value_commit: None,
            space: ColorPickerSpace::Hsv,
            format: None,
            gamut: Some(ColorGamut::Srgb),
            disabled: false,
            read_only: false,
            required: false,
            name: None,
            form: None,
            debug: false,
        }
    }
}

impl ColorPickerOptions {
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&SharedString, &ColorPickerValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&SharedString, &ColorPickerValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_value_commit(
        mut self,
        callback: impl Fn(&SharedString, &ColorPickerValueCommitDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_commit = Some(Rc::new(callback));
        self
    }

    pub fn space(mut self, space: ColorPickerSpace) -> Self {
        self.space = space;
        self
    }

    pub fn format(mut self, format: ColorFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// The colors the value can hold, or `None` for any.
    pub fn gamut(mut self, gamut: impl Into<Option<ColorGamut>>) -> Self {
        self.gamut = gamut.into();
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

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `area` part takes, and its thumb and canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ColorPickerAreaProps {
    /// The channel across, left to right. Saturation, red or chroma, by
    /// space, by default.
    pub x: Option<ColorChannel>,
    /// The channel up, bottom to top. Brightness, lightness or green, by
    /// space, by default.
    pub y: Option<ColorChannel>,
}

impl ColorPickerAreaProps {
    pub fn x(mut self, channel: ColorChannel) -> Self {
        self.x = Some(channel);
        self
    }

    pub fn y(mut self, channel: ColorChannel) -> Self {
        self.y = Some(channel);
        self
    }
}

/// A slider's axis. Vertical sliders grow upward.
pub type ColorPickerSliderOrientation = gpui::Orientation;

/// The props the `slider` part takes, and its thumb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorPickerSliderProps {
    /// The channel the slider edits, like hue or alpha. Hue, or blue in
    /// sRGB, by default.
    pub channel: Option<ColorChannel>,
    /// The slider's axis. Vertical sliders grow upward. Horizontal by
    /// default.
    pub orientation: ColorPickerSliderOrientation,
}

impl Default for ColorPickerSliderProps {
    fn default() -> Self {
        Self {
            channel: None,
            orientation: ColorPickerSliderOrientation::Horizontal,
        }
    }
}

impl ColorPickerSliderProps {
    pub fn channel(mut self, channel: ColorChannel) -> Self {
        self.channel = Some(channel);
        self
    }

    pub fn orientation(mut self, orientation: ColorPickerSliderOrientation) -> Self {
        self.orientation = orientation;
        self
    }
}

/// The props the `channel-input` part takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ColorPickerChannelInputProps {
    /// The channel the input edits, like hue or alpha. Hue, or blue in sRGB,
    /// by default.
    pub channel: Option<ColorChannel>,
}

impl ColorPickerChannelInputProps {
    pub fn channel(mut self, channel: ColorChannel) -> Self {
        self.channel = Some(channel);
        self
    }
}

/// The props the `swatch-item` part takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorPickerSwatchItemProps {
    /// The color it picks, as any CSS color.
    pub value: SharedString,
}

impl ColorPickerSwatchItemProps {
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self { value: value.into() }
    }
}

/// What every part's `with` and the app read.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorPickerState {
    /// The value, as a CSS color.
    pub value: SharedString,
    /// The color the parts show, in the picker's space. During a drag it can
    /// be outside the gamut.
    pub color: Color,
    /// The space the parts edit in.
    pub space: ColorPickerSpace,
    /// Whether it's disabled, by itself or by a fieldset.
    pub disabled: bool,
    /// Whether it's read-only: the parts stay focusable, but nothing changes
    /// the value.
    pub read_only: bool,
    /// Whether it's marked required.
    pub required: bool,
    /// Whether the color the parts show is outside the gamut, so the value
    /// is the closest one inside.
    pub out_of_gamut: bool,
    /// The color as the parts paint it, fit into the gamut
    /// (`--color-picker-color`): a thumb or a swatch fills itself with it.
    pub fill: Hsla,
}

/// What the `area` part, its thumb and its canvas read: the picker's state,
/// with the area's channels and where its thumb sits.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorPickerAreaState {
    pub value: SharedString,
    pub color: Color,
    pub space: ColorPickerSpace,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub out_of_gamut: bool,
    pub fill: Hsla,
    /// The channel across.
    pub x: ColorChannel,
    /// The channel up.
    pub y: ColorChannel,
    /// Where the thumb sits across, from 0 to 1 (`--area-x`).
    pub fraction_x: f64,
    /// Where the thumb sits up, from 0 at the bottom to 1 (`--area-y`).
    pub fraction_y: f64,
    /// The gradients that draw the area, where they can: saturation by
    /// brightness or lightness (`--color-area-background`). Others take the
    /// area's canvas.
    pub background: Option<AreaGradient>,
}

/// What a `slider` part and its thumb read: the picker's state, with the
/// slider's channel and its value.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorPickerSliderState {
    pub value: SharedString,
    pub color: Color,
    pub space: ColorPickerSpace,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub out_of_gamut: bool,
    pub fill: Hsla,
    /// The channel the slider edits (`data-channel`).
    pub channel: ColorChannel,
    /// The channel's value, in the units people read it in: degrees,
    /// percentages, 0 to 255 for red.
    pub channel_value: f64,
    /// The slider's axis.
    pub orientation: ColorPickerSliderOrientation,
    /// Where the thumb sits, from 0 to 100 (`--color-slider-thumb-position`).
    pub percent: f64,
    /// The track's gradient (`--color-slider-background`).
    pub background: ChannelGradient,
}

/// What a `channel-input` part reads: the picker's state, with its channel
/// (`data-channel`).
#[derive(Clone, Debug, PartialEq)]
pub struct ColorPickerChannelInputState {
    pub value: SharedString,
    pub color: Color,
    pub space: ColorPickerSpace,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub out_of_gamut: bool,
    pub fill: Hsla,
    pub channel: ColorChannel,
}

/// What a `swatch-item` part's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorPickerSwatchItemState {
    /// Whether the picker's color looks the same as this swatch's, whatever
    /// each is written in.
    pub selected: bool,
    /// Whether pressing it changes nothing, since the picker is disabled or
    /// read-only.
    pub disabled: bool,
    /// Whether the picker is read-only (`data-readonly`).
    pub read_only: bool,
    /// The swatch's own color, as it paints, when its value is one
    /// (`--color-picker-color`).
    pub fill: Option<Hsla>,
}

// A channel input's native field, and what it hears from it.
pub(crate) struct ChannelField {
    pub(crate) field: Entity<TextField>,
    pub(crate) _subscriptions: Vec<Subscription>,
}

/// A color picker (`colorPicker`). Its entity is its controller.
pub struct ColorPicker {
    pub(crate) instance: Instance,
    pub(crate) value: Controllable<SharedString>,
    pub(crate) on_value_change: Option<ColorPickerValueChange>,
    pub(crate) on_before_value_change: Option<ColorPickerValueChange>,
    pub(crate) on_value_commit: Option<ColorPickerValueCommit>,
    // What a form reset returns to.
    pub(crate) initial: SharedString,
    // Whether the options pass a `default_value`, which a controlled picker
    // resets to.
    pub(crate) has_default: bool,
    // The color the parts edit, in the picker's space. It can sit outside
    // the gamut while a drag is on, and fits it once the drag ends.
    pub(crate) color: Color,
    pub(crate) settings: Settings,
    // What the text input holds while someone types, until it commits.
    pub(crate) text: Option<SharedString>,
    // The value an interaction started from, so it commits only what it
    // changed.
    pub(crate) start: Option<SharedString>,
    // The last value a change asked for. A controlled value only arrives
    // with the app's next render, after a key or a swatch has already
    // committed.
    pub(crate) changed: Option<SharedString>,
    pub(crate) drags: DragSession<Self>,
    // The areas the parts asked for, by their axes, in the order they did.
    pub(crate) areas: RefCell<Vec<(ColorPickerAreaProps, Area<Self>)>>,
    pub(crate) canvases: RefCell<HashMap<ColorPickerAreaProps, Rc<AreaCanvas>>>,
    // Each slider's track, by its channel, and its thumb's focus.
    pub(crate) sliders: Parts<ColorChannel, ()>,
    pub(crate) roving: RovingFocus<SharedString>,
    pub(crate) internals: ElementInternals,
    // The text input's native field, and each channel input's.
    pub(crate) input: Entity<TextField>,
    pub(crate) channel_fields: HashMap<ColorChannel, ChannelField>,
    _subscriptions: Vec<Subscription>,
}

impl ColorPicker {
    pub fn new(options: ColorPickerOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A change's source follows how the window was used.
        track_input(window, cx);
        let settings = settings(&options);
        let given = options.value.clone().or_else(|| options.default_value.clone());
        let color = given
            .as_ref()
            .and_then(|given| color_in(given, settings.space))
            .unwrap_or_else(|| black(settings.space));
        let start = given.unwrap_or_else(|| settings.output(&color));
        let mut value = Controllable::new("value", start.clone());
        value.set_controlled(options.value.clone());
        let mut instance = Instance::new("color-picker").with_agent(cx);
        instance.debug = options.debug;
        let internals = ElementInternals::new(cx);
        let roving = RovingFocus::new(swatch_focus(), window, cx);
        let input = cx.new(|cx| TextField::new(Self::input_options(&settings, false, &start), window, cx));
        // A label naming the picker, or a submit, focuses its first thumb.
        let entity = cx.weak_entity();
        let focused = focus_with(&internals, move |window, cx| {
            let target = entity
                .upgrade()
                .and_then(|picker| picker.read(cx).focus_target(window, cx));
            if let Some(target) = target {
                window.focus(&target, cx);
            }
        });
        let association = associate(
            &internals,
            Association::new()
                .reset(cx, |picker: &mut Self, window, cx| picker.reset(window, cx))
                .disabled(cx, |picker: &mut Self, _disabled, window, cx| {
                    picker.form_disabled_changed(window, cx)
                }),
        );
        let subscriptions = vec![
            focused,
            association,
            // The field's own events come in order: what an input method
            // composed as focus left, then the blur that commits it
            // (`focusout`).
            cx.subscribe_in(
                &input,
                window,
                |picker: &mut Self, _, event, window, cx| match event {
                    TextFieldEvent::Input(edit) => picker.set_text(edit.value.clone(), cx),
                    TextFieldEvent::Blur => {
                        let event = ui_sdk_kernel::modality::focus_event(window);
                        picker.commit_text(Some(event), window, cx);
                    }
                    _ => {}
                },
            ),
            cx.observe_keystrokes(Self::keystroke),
        ];
        let mut subscriptions = subscriptions;
        // Its field notifies it when it disables it, which the native fields
        // follow.
        let this = cx.entity();
        subscriptions.push(cx.observe_in(&this, window, |picker: &mut Self, _, _window, cx| {
            picker.sync_field_options(cx)
        }));
        let mut picker = Self {
            instance,
            value,
            on_value_change: options.on_value_change,
            on_before_value_change: options.on_before_value_change,
            on_value_commit: options.on_value_commit,
            has_default: options.default_value.is_some(),
            initial: options.default_value.unwrap_or_else(|| start.clone()),
            color,
            settings,
            text: None,
            start: None,
            changed: None,
            drags: create_drag_session(cx),
            areas: RefCell::new(Vec::new()),
            canvases: RefCell::new(HashMap::new()),
            sliders: Parts::new(),
            roving,
            internals,
            input,
            channel_fields: HashMap::new(),
            _subscriptions: subscriptions,
        };
        picker.sync_channel_fields(window, cx);
        picker.sync_form();
        let release = cx.on_release_in(window, |picker, window, cx| {
            for canvas in picker.canvases.borrow().values() {
                canvas.release(window, cx);
            }
        });
        picker._subscriptions.push(release);
        picker
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A value that arrives as the color the parts hold would write
    /// keeps that color, with its hue; any other reads anew, in the space
    /// that arrives.
    pub fn set_options(&mut self, options: ColorPickerOptions, window: &mut Window, cx: &mut Context<Self>) {
        let settings = settings(&options);
        let mut changed = settings != self.settings;
        if let Some(default) = &options.default_value {
            self.initial = default.clone();
        }
        self.has_default = options.default_value.is_some();
        let arriving = options.value.clone().unwrap_or_else(|| self.value.get().clone());
        let color = self.derive_color(&arriving, &settings);
        changed |= color != self.color;
        self.color = color;
        self.settings = settings;
        changed |= self.value.set_controlled(options.value);
        self.on_value_change = options.on_value_change;
        self.on_before_value_change = options.on_before_value_change;
        self.on_value_commit = options.on_value_commit;
        self.instance.debug = options.debug;
        self.sync_channel_fields(window, cx);
        // Ending a drag commits, which the app's callback may answer by
        // updating whatever renders the picker, so it waits for the options
        // to be in.
        if self.is_locked() && self.drags.is_running() {
            cx.defer_in(window, |picker, window, cx| picker.end_if_locked(window, cx));
        }
        self.sync_form();
        self.write_fields(window, cx);
        // Options arrive on every render, so only a change renders again.
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled value, or `None` to let the picker keep its
    /// own.
    pub fn set_controlled_value(
        &mut self,
        value: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(value) = &value {
            let settings = self.settings.clone();
            self.color = self.derive_color(value, &settings);
        }
        if self.value.set_controlled(value) {
            self.after_change(window, cx);
        }
    }

    // The color a value arriving with settings leaves the parts holding
    // (`deriveFromOptions`).
    fn derive_color(&self, arriving: &SharedString, settings: &Settings) -> Color {
        let kept = if settings.space == self.settings.space {
            self.color
        } else {
            in_space(&self.color, settings.space)
        };
        if arriving == self.value.get() || *arriving == settings.output(&kept) {
            return kept;
        }
        color_in(arriving, settings.space).unwrap_or(kept)
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The text input's native field.
    pub fn input_field(&self) -> &Entity<TextField> {
        &self.input
    }

    /// Its part in forms, on the root.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    // Enter in the text input or a channel input submits its form
    // implicitly, a native input's own action, once every listener had the
    // key and none prevented it.
    fn keystroke(&mut self, event: &gpui::KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.action.is_some() || window.default_prevented() || Key::of(&event.keystroke) != Key::Enter {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt {
            return;
        }
        let ours = std::iter::once(&self.input)
            .chain(self.channel_fields.values().map(|channel| &channel.field))
            .any(|field| {
                let field = field.read(cx);
                field.is_focused(window) && !field.is_composing() && !field.is_disabled()
            });
        // A channel input reports its change as it takes the key, after
        // this, so the submit waits for it, as `change` comes first in a
        // browser.
        if ours {
            cx.defer_in(window, |picker, window, cx| {
                ui_sdk_forms::submit_implicitly(picker.internals.form().as_ref(), window, cx)
            });
        }
    }
}

impl Focusable for ColorPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        let areas = self.areas.borrow();
        match areas.first() {
            Some((_, area)) => area.focus_handle(),
            None => self.input.read(cx).focus_handle(cx),
        }
    }
}

// The swatches' one tab stop, with the arrow keys between them, around
// neither end, and Home and End.
fn swatch_focus() -> RovingFocusOptions<SharedString> {
    RovingFocusOptions::default().remember(false).r#move(
        |event, context: &RovingFocusMoveContext<SharedString>, _cx| {
            if event.keystroke.modifiers.shift {
                return None;
            }
            let key = Key::of(&event.keystroke);
            let items = &context.items;
            if let Some(step) = radio_step(&key, context.rtl) {
                let index = items.iter().position(|item| item.key == context.from.key)?;
                return items
                    .get(move_index(index as isize, step, items.len(), false))
                    .map(|item| item.key.clone());
            }
            match key {
                Key::Home => items.first().map(|item| item.key.clone()),
                Key::End => items.last().map(|item| item.key.clone()),
                _ => None,
            }
        },
    )
}

/// `agent: { actions: ["setValue"] }` and `commits: ["value"]`.
impl Agent for ColorPicker {
    const ACTIONS: &'static [AgentAction<Self>] =
        &[AgentAction::new("setValue", |picker, args, window, cx| {
            let value = args.string(0)?;
            Ok(picker
                .set_value(value, ChangeOptions::default(), window, cx)
                .into())
        })];

    const COMMITS: &'static [&'static str] = &["value"];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with("value", self.value.get().to_string())
            .with("disabled", self.is_disabled())
            .with("readOnly", self.settings.read_only)
            .with("required", self.settings.required)
    }
}
