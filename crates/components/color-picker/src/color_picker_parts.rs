//! Port of `components/color-picker/color-picker-parts.ts`: a color picker's
//! parts and their attributes. State and what the parts do live in
//! `color_picker_api.rs`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Bounds, Corners, Entity, Focusable, KeyDownEvent, ParentElement, Pixels, RenderImage,
    StatefulInteractiveElement, Styled, Window, canvas, prelude::FluentBuilder, relative,
};
use image::ImageBuffer;
use ui_sdk_color::{Color, ColorGamut, ColorSpace, convert_color, is_in_gamut, parse_color};
use ui_sdk_direct_manipulation::Area;
use ui_sdk_drag_and_drop::on_pointer_down;
use ui_sdk_focus::RovingMember;
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, ChangeOptions, Key, Need, Part, Reason};
use ui_sdk_native::TextField;
use ui_sdk_platform::is_rtl;

use crate::color_picker::{
    ColorPicker, ColorPickerAreaProps, ColorPickerAreaState, ColorPickerChannelInputProps,
    ColorPickerChannelInputState, ColorPickerSliderOrientation, ColorPickerSliderProps,
    ColorPickerSliderState, ColorPickerState, ColorPickerSwatchItemProps, ColorPickerSwatchItemState,
};
use crate::color_picker_api::value_text;
use crate::color_picker_channels::{
    Channel, area_gradient, channel_gradient, describe_color, js_round, read_channel, shown, with_channel,
};

/// The keys a thumb handles: the area's or a slider's.
pub const SLIDER_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "shift+ArrowUp",
    "shift+ArrowDown",
    "shift+ArrowLeft",
    "shift+ArrowRight",
    "PageUp",
    "PageDown",
    "Home",
    "End",
];

/// The keys a swatch item handles.
pub const SWATCH_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Home",
    "End",
    "Enter",
    " ",
];

fn percent_of(value: f64, spec: &Channel) -> f64 {
    js_round((value / spec.max) * 1e4) / 100.0
}

// The key a part of an area renders with, for its element id: its channels.
pub(crate) fn area_key(x: &Channel, y: &Channel) -> String {
    format!("{}-{}", x.name.as_str(), y.name.as_str())
}

// The area a press on one of its parts is for.
fn area_of(
    picker: &ColorPicker,
    props: ColorPickerAreaProps,
    cx: &mut gpui::Context<ColorPicker>,
) -> Area<ColorPicker> {
    let entity = cx.weak_entity();
    picker.area_for(props, entity, cx)
}

/// What an area's canvas last painted, and the copies it no longer shows,
/// which leave the sprite atlas once the frame is drawn, and the rest when
/// the picker goes.
#[derive(Default)]
pub(crate) struct AreaCanvas {
    painted: RefCell<Option<(CanvasKey, Arc<RenderImage>)>>,
    stale: RefCell<Vec<Arc<RenderImage>>>,
}

// Only the channels the area doesn't show change its picture.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CanvasKey {
    held: Color,
    gamut: Option<ColorGamut>,
    rtl: bool,
    width: u32,
    height: u32,
}

impl AreaCanvas {
    // The picture for a canvas this size, painted again only when what it
    // shows changed.
    #[allow(clippy::too_many_arguments)]
    fn image(
        &self,
        bounds: Bounds<Pixels>,
        color: &Color,
        x: &Channel,
        y: &Channel,
        gamut: Option<ColorGamut>,
        rtl: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<RenderImage>> {
        let client_width = f64::from(f32::from(bounds.size.width)).round();
        let client_height = f64::from(f32::from(bounds.size.height)).round();
        let width = client_width.clamp(2.0, 160.0);
        let height = js_round((width * client_height) / client_width.max(1.0)).max(2.0);
        let held = with_channel(&with_channel(&Color { alpha: 1.0, ..*color }, x, 0.0), y, 0.0);
        let key = CanvasKey {
            held,
            gamut,
            rtl,
            width: width as u32,
            height: height as u32,
        };
        if let Some((painted, image)) = self.painted.borrow().as_ref()
            && *painted == key
        {
            return Some(image.clone());
        }
        let image = paint_area(key.width, key.height, color, x, y, gamut, rtl)?;
        if let Some((_, old)) = self.painted.borrow_mut().replace((key, image.clone())) {
            self.stale.borrow_mut().push(old);
        }
        let stale: Vec<Arc<RenderImage>> = self.stale.borrow_mut().drain(..).collect();
        if !stale.is_empty() {
            window.defer(cx, move |window, cx| {
                for image in stale {
                    cx.drop_image(image, Some(window));
                }
            });
        }
        Some(image)
    }

    /// Takes its pictures out of the sprite atlas, as the picker goes.
    pub(crate) fn release(&self, window: &mut Window, cx: &mut App) {
        let painted = self.painted.borrow_mut().take().map(|(_, image)| image);
        let stale: Vec<Arc<RenderImage>> = self.stale.borrow_mut().drain(..).chain(painted).collect();
        for image in stale {
            cx.drop_image(image, Some(window));
        }
    }
}

/// Every pixel of the area for the color's other channels, in the gamut's
/// space. Colors outside it stay transparent, so the edge shows its shape
/// (`paintArea`). gpui paints in sRGB, so a P3 gamut's colors are written as
/// the sRGB they clip to, while its edge is still P3's.
fn paint_area(
    width: u32,
    height: u32,
    color: &Color,
    x: &Channel,
    y: &Channel,
    gamut: Option<ColorGamut>,
    rtl: bool,
) -> Option<Arc<RenderImage>> {
    let p3 = gamut == Some(ColorGamut::P3);
    let target = if p3 {
        ColorSpace::DisplayP3
    } else {
        ColorSpace::Srgb
    };
    let base = Color { alpha: 1.0, ..*color };
    let (w, h) = (f64::from(width), f64::from(height));
    let row_color = |row: f64| with_channel(&base, y, (1.0 - row / (h - 1.0)) * y.max);
    let color_at = |at_row: &Color, column: f64| {
        let along = column / (w - 1.0);
        let along = if rtl { 1.0 - along } else { along };
        convert_color(&with_channel(at_row, x, along * x.max), target)
            .coords
            .map(|coord| coord.unwrap_or(0.0))
    };
    // Tighter than `is_in_gamut`, whose tolerance near black would paint a
    // sliver of colors past the edge.
    let fits = |coords: [f64; 3]| coords.iter().all(|value| (-0.000001..=1.000001).contains(value));
    let byte = |value: f64| js_round(value.clamp(0.0, 1.0) * 255.0) as u8;
    let (columns, rows) = (width as usize, height as usize);
    let mut inside = vec![false; columns * rows];
    let mut pixels = vec![0u8; columns * rows * 4];
    for row in 0..rows {
        let at_row = row_color(row as f64);
        for column in 0..columns {
            let coords = color_at(&at_row, column as f64);
            let pixel = row * columns + column;
            inside[pixel] = fits(coords);
            let [red, green, blue] = if p3 {
                convert_color(&Color::new(ColorSpace::DisplayP3, coords, 1.0), ColorSpace::Srgb)
                    .coords
                    .map(|coord| coord.unwrap_or(0.0))
            } else {
                coords
            };
            // gpui's images are BGRA.
            pixels[pixel * 4] = byte(blue);
            pixels[pixel * 4 + 1] = byte(green);
            pixels[pixel * 4 + 2] = byte(red);
        }
    }
    // The canvas is smaller than the area, so a hard edge would show as
    // steps. A pixel on the edge is as opaque as the share of it inside.
    let offsets = [-1.0 / 3.0, 0.0, 1.0 / 3.0];
    for row in 0..rows {
        for column in 0..columns {
            let pixel = row * columns + column;
            let own = inside[pixel];
            let edge = (column > 0 && inside[pixel - 1] != own)
                || (column < columns - 1 && inside[pixel + 1] != own)
                || (row > 0 && inside[pixel - columns] != own)
                || (row < rows - 1 && inside[pixel + columns] != own);
            let mut share = if own { 1.0 } else { 0.0 };
            if edge {
                let mut count = 0.0;
                for down in offsets {
                    let at_row = row_color(row as f64 + down);
                    for across in offsets {
                        if fits(color_at(&at_row, column as f64 + across)) {
                            count += 1.0;
                        }
                    }
                }
                share = count / (offsets.len() * offsets.len()) as f64;
            }
            pixels[pixel * 4 + 3] = js_round(share * 255.0) as u8;
        }
    }
    let buffer = ImageBuffer::from_raw(width, height, pixels)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

impl ColorPicker {
    /// What every part's state holds (`pickerState`).
    pub fn state(&self) -> ColorPickerState {
        let gamut = self.settings.gamut;
        ColorPickerState {
            value: self.value.get().clone(),
            color: self.color,
            space: self.settings.space,
            disabled: self.is_disabled(),
            read_only: self.settings.read_only,
            required: self.settings.required,
            out_of_gamut: gamut.is_some_and(|gamut| !is_in_gamut(&self.color, gamut)),
            fill: shown(&self.color, gamut).to_hsla(),
        }
    }

    fn area_state(&self, props: ColorPickerAreaProps, area: &Area<Self>, rtl: bool) -> ColorPickerAreaState {
        let state = self.state();
        let (x, y) = self.axes(props);
        let fractions = area.fractions(self);
        ColorPickerAreaState {
            value: state.value,
            color: state.color,
            space: state.space,
            disabled: state.disabled,
            read_only: state.read_only,
            required: state.required,
            out_of_gamut: state.out_of_gamut,
            fill: state.fill,
            x: x.name,
            y: y.name,
            fraction_x: fractions.x,
            fraction_y: fractions.y,
            background: area_gradient(&self.color, x.name, y.name, rtl),
        }
    }

    fn slider_state(&self, props: ColorPickerSliderProps, rtl: bool) -> ColorPickerSliderState {
        let state = self.state();
        let spec = self.channel(props.channel);
        let value = read_channel(&self.color, &spec);
        let vertical = props.orientation == ColorPickerSliderOrientation::Vertical;
        ColorPickerSliderState {
            value: state.value,
            color: state.color,
            space: state.space,
            disabled: state.disabled,
            read_only: state.read_only,
            required: state.required,
            out_of_gamut: state.out_of_gamut,
            fill: state.fill,
            channel: spec.name,
            channel_value: value,
            orientation: props.orientation,
            percent: percent_of(value, &spec),
            background: channel_gradient(&self.color, &spec, vertical, rtl),
        }
    }

    pub(crate) fn canvas_for(&self, props: ColorPickerAreaProps) -> Rc<AreaCanvas> {
        self.canvases.borrow_mut().entry(props).or_default().clone()
    }

    /// The `root` part, a group around the parts, with the picker's form
    /// side. It follows a drag's pointer, so render it on every frame. Name
    /// it with `aria_label`, or with a field's label.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<ColorPickerState> {
        let picker = this.read(cx);
        picker.sync_form();
        // A group takes the labels and the field's descriptions; whether
        // it's disabled or read-only is each part's to say.
        let mut aria = picker.internals.aria();
        aria.disabled = false;
        aria.read_only = false;
        aria.required = false;
        Part::new(&picker.instance, "root", None, picker.state())
            .map_el(|el| picker.internals.track(el))
            .aria_role(Role::Group)
            .aria(aria)
            .child(picker.drags.listener())
    }

    /// An `area` part: a surface for two channels, like saturation across
    /// and brightness up. A press moves the thumb there and starts a drag,
    /// which it follows. Draw it with its state's `background` where
    /// gradients can, or put an `area_canvas` in it. Give it a size; it's
    /// `relative`, for its thumb.
    pub fn area(this: &Entity<Self>, props: ColorPickerAreaProps, cx: &App) -> Part<ColorPickerAreaState> {
        let picker = this.read(cx);
        let area = picker.area_for(props, this.downgrade(), cx);
        let state = picker.area_state(props, &area, is_rtl(cx));
        let (x, y) = picker.axes(props);
        let entity = this.downgrade();
        let part = Part::new(&picker.instance, "area", Some(&area_key(&x, &y)), state).relative();
        let part = area.measure(part).child(area.listener());
        on_pointer_down(part, move |event, window, cx| {
            entity
                .update(cx, |picker, cx| {
                    picker.area_pointer_down(&area, event, window, cx)
                })
                .ok();
        })
    }

    /// An `area-thumb` part, inside its area: where the color sits, a slider
    /// screen readers hear as both channels, like "Saturation 76%,
    /// Brightness 96%". It's placed in the area from its start and its top;
    /// center it on that point. Named in English, like "Saturation and
    /// brightness"; pass `aria_label` in another language.
    pub fn area_thumb(
        this: &Entity<Self>,
        props: ColorPickerAreaProps,
        cx: &App,
    ) -> Part<ColorPickerAreaState> {
        let picker = this.read(cx);
        let area = picker.area_for(props, this.downgrade(), cx);
        let rtl = is_rtl(cx);
        let state = picker.area_state(props, &area, rtl);
        let attrs = area.thumb_attrs(picker);
        let (x, y) = picker.axes(props);
        let disabled = picker.is_disabled();
        let entity = this.downgrade();
        let keyed = area.clone();
        let part = Part::new(&picker.instance, "area-thumb", Some(&area_key(&x, &y)), state)
            .aria_role(Role::Slider)
            .focus_mode(FocusMode::Native, Some(attrs.focus_handle()), disabled)
            .aria_label(format!("{} and {}", x.label, y.label.to_lowercase()))
            .aria_min_numeric_value(attrs.min)
            .aria_max_numeric_value(attrs.max)
            .aria_numeric_value(attrs.now)
            .aria(
                Aria::new()
                    .role_description("2D slider")
                    .value_text(attrs.text.clone())
                    .disabled(disabled)
                    .read_only(picker.settings.read_only),
            )
            .on_keys(SLIDER_KEYS, move |event, window, cx| {
                entity
                    .update(cx, |picker, cx| picker.area_keydown(&keyed, event, window, cx))
                    .ok();
            });
        // A press on the thumb is the area's, as it bubbles up in the DOM,
        // even where the thumb overhangs the area's edge.
        let pressed = this.downgrade();
        let part = on_pointer_down(part, move |event, window, cx| {
            pressed
                .update(cx, |picker, cx| {
                    picker.area_pointer_down(&area_of(picker, props, cx), event, window, cx)
                })
                .ok();
        });
        area.thumb_style(picker).apply(part, rtl)
    }

    /// An `area-canvas` part, inside its area: paints it pixel by pixel, for
    /// channels no gradient draws, like OKLCH's chroma by lightness, whose
    /// gamut's edge is a curve. It fills the area and takes no presses.
    pub fn area_canvas(
        this: &Entity<Self>,
        props: ColorPickerAreaProps,
        cx: &App,
    ) -> Part<ColorPickerAreaState> {
        let picker = this.read(cx);
        let area = picker.area_for(props, this.downgrade(), cx);
        let rtl = is_rtl(cx);
        let state = picker.area_state(props, &area, rtl);
        let (x, y) = picker.axes(props);
        let painter = picker.canvas_for(props);
        let color = picker.color;
        let gamut = picker.settings.gamut;
        let key = area_key(&x, &y);
        Part::new(&picker.instance, "area-canvas", Some(&key), state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(
                canvas(
                    move |bounds, window, cx| painter.image(bounds, &color, &x, &y, gamut, rtl, window, cx),
                    |bounds, image, window, _cx| {
                        if let Some(image) = image {
                            window
                                .paint_image(bounds, bounds, Corners::default(), image, 0, false)
                                .ok();
                        }
                    },
                )
                .size_full(),
            )
    }

    /// A `slider` part: a track for one channel, drawn by its state's
    /// `background`. A press moves its thumb there and follows the pointer.
    /// Give it a size; it's `relative`, for its thumb.
    pub fn slider(
        this: &Entity<Self>,
        props: ColorPickerSliderProps,
        cx: &App,
    ) -> Part<ColorPickerSliderState> {
        let picker = this.read(cx);
        let state = picker.slider_state(props, is_rtl(cx));
        let channel = state.channel;
        let entity = this.downgrade();
        let part = Part::new(&picker.instance, "slider", Some(channel.as_str()), state)
            .relative()
            .map_el(|el| picker.sliders.measure(channel, el));
        on_pointer_down(part, move |event, window, cx| {
            entity
                .update(cx, |picker, cx| {
                    picker.slider_pointer_down(event, props, window, cx)
                })
                .ok();
        })
    }

    /// A `slider-thumb` part, inside its slider, with the same props: where
    /// the channel sits, a slider. It's placed from the track's start, or its
    /// bottom when vertical; center it on that point. Named in English, like
    /// "Hue"; pass `aria_label` in another language.
    pub fn slider_thumb(
        this: &Entity<Self>,
        props: ColorPickerSliderProps,
        cx: &App,
    ) -> Part<ColorPickerSliderState> {
        let picker = this.read(cx);
        let rtl = is_rtl(cx);
        let state = picker.slider_state(props, rtl);
        let spec = picker.channel(props.channel);
        let handle = picker.sliders.focus_handle(&spec.name, cx);
        let disabled = picker.is_disabled();
        let value = state.channel_value;
        let at = relative((state.percent / 100.0) as f32);
        let vertical = props.orientation == ColorPickerSliderOrientation::Vertical;
        let entity = this.downgrade();
        let part = Part::new(&picker.instance, "slider-thumb", Some(spec.name.as_str()), state)
            .aria_role(Role::Slider)
            .focus_mode(FocusMode::Native, Some(&handle), disabled)
            .aria_label(spec.label)
            .aria_min_numeric_value(0.0)
            .aria_max_numeric_value(spec.max)
            .aria_numeric_value(value)
            .aria_orientation(props.orientation)
            .aria(
                Aria::new()
                    .value_text(value_text(value, &spec))
                    .disabled(disabled)
                    .read_only(picker.settings.read_only),
            )
            .on_keys(SLIDER_KEYS, move |event, window, cx| {
                entity
                    .update(cx, |picker, cx| {
                        picker.slider_keydown(event, props.channel, window, cx)
                    })
                    .ok();
            });
        // A press on the thumb is the slider's, as it bubbles up in the DOM,
        // even where the thumb overhangs the track's end. Hearing presses
        // makes the thumb relative, so it's placed after.
        let pressed = this.downgrade();
        let part = on_pointer_down(part, move |event, window, cx| {
            pressed
                .update(cx, |picker, cx| {
                    picker.slider_pointer_down(event, props, window, cx)
                })
                .ok();
        })
        .absolute();
        if vertical {
            part.bottom(at)
        } else if rtl {
            part.right(at)
        } else {
            part.left(at)
        }
    }

    /// The `input` part: a text field that reads any CSS color on Enter or
    /// blur. Text that isn't one goes back to the value; Escape puts the
    /// value back too. Name it with `aria_label`, like "Hex".
    pub fn input(this: &Entity<Self>, cx: &App) -> Part<ColorPickerState> {
        let picker = this.read(cx);
        let field = picker.input.read(cx);
        let focus = field.focus_handle(cx);
        let disabled = field.is_disabled();
        let read_only = field.is_read_only();
        let (el, _) = TextField::root(&picker.input, cx).into_element_parts();
        let keys = this.downgrade();
        Part::new(&picker.instance, "input", None, picker.state())
            .map_el(move |_| el)
            .aria_role(Role::Textbox)
            .focus_mode(FocusMode::Native, Some(&focus), disabled)
            .aria(Aria::new().disabled(disabled).read_only(read_only))
            .on_keys(&["Enter", "Escape"], move |event, window, cx| {
                keys.update(cx, |picker, cx| picker.input_keydown(event, window, cx))
                    .ok();
            })
    }

    // Enter reads the text; Escape, while there's text, puts the value back.
    fn input_keydown(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        match Key::of(&event.keystroke) {
            Key::Enter => self.commit_text(Some(event.into()), window, cx),
            Key::Escape if self.text.is_some() => {
                window.prevent_default();
                self.revert_text(window, cx);
            }
            _ => {}
        }
    }

    /// A `channel-input` part: a number field for one channel, in the units
    /// people read it in: 0 to 255 for red, a percentage for saturation. It
    /// sets the channel on Enter, blur or a step of its arrows. One for each
    /// channel. Named in English, like "Red"; pass `aria_label` in another
    /// language.
    pub fn channel_input(
        this: &Entity<Self>,
        props: ColorPickerChannelInputProps,
        cx: &App,
    ) -> Part<ColorPickerChannelInputState> {
        let picker = this.read(cx);
        let state = picker.state();
        let spec = picker.channel(props.channel);
        let input = ColorPickerChannelInputState {
            value: state.value,
            color: state.color,
            space: state.space,
            disabled: state.disabled,
            read_only: state.read_only,
            required: state.required,
            out_of_gamut: state.out_of_gamut,
            fill: state.fill,
            channel: spec.name,
        };
        let part = Part::new(&picker.instance, "channel-input", Some(spec.name.as_str()), input);
        // The field's element becomes the part's first, so the name and the
        // range go on it.
        let part = match picker.channel_fields.get(&spec.name) {
            Some(channel) => {
                let field = channel.field.read(cx);
                let focus = field.focus_handle(cx);
                let disabled = field.is_disabled();
                let read_only = field.is_read_only();
                let number = field.value_as_number();
                let (el, _) = TextField::root(&channel.field, cx).into_element_parts();
                part.map_el(move |_| el)
                    .aria_role(Role::SpinButton)
                    .focus_mode(FocusMode::Native, Some(&focus), disabled)
                    .when_some(number, |part, number| part.aria_numeric_value(number))
                    .aria(Aria::new().disabled(disabled).read_only(read_only))
            }
            None => part
                .aria_role(Role::SpinButton)
                .unneeded(Need::Focus, "Its channel isn't one of the space's."),
        };
        part.aria_label(spec.label)
            .aria_min_numeric_value(0.0)
            .aria_max_numeric_value(spec.max)
            .aria_numeric_value_step(spec.step)
            .unneeded(
                Need::Keys,
                "Its native number field steps with the arrow keys itself.",
            )
    }

    /// The `swatch` part: the color itself, an image named for screen
    /// readers, like "dark vivid blue", in English. Fill it with its state's
    /// `fill`.
    pub fn swatch(this: &Entity<Self>, cx: &App) -> Part<ColorPickerState> {
        let picker = this.read(cx);
        let name = describe_color(&shown(&picker.color, picker.settings.gamut));
        Part::new(&picker.instance, "swatch", None, picker.state())
            .aria_role(Role::Img)
            .aria_label(name)
    }

    /// The `swatches` part: a list of preset colors, one tab stop, with the
    /// arrow keys between them. Build it before its items.
    pub fn swatches(this: &Entity<Self>, cx: &App) -> Part<ColorPickerState> {
        let picker = this.read(cx);
        picker.roving.begin(cx);
        let state = picker.state();
        let aria = Aria::new().disabled(state.disabled).read_only(state.read_only);
        Part::new(&picker.instance, "swatches", None, state)
            .aria_role(Role::Listbox)
            .unneeded(Need::Keys, "Focus stays on a swatch, whose keydown handles them.")
            .aria_orientation(gpui::Orientation::Horizontal)
            .aria(aria)
    }

    /// A `swatch-item` part, inside the swatches: a preset color. Pressing it
    /// sets the value. It's selected while the picker's color looks the same,
    /// however either is written. Named for its color in English, like
    /// "vivid red"; pass `aria_label` in another language.
    pub fn swatch_item(
        this: &Entity<Self>,
        props: ColorPickerSwatchItemProps,
        cx: &App,
    ) -> Part<ColorPickerSwatchItemState> {
        let picker = this.read(cx);
        let value = props.value;
        let selected = picker.is_selected(&value);
        let color = parse_color(&value);
        let locked = picker.is_locked();
        let disabled = picker.is_disabled();
        let handle = picker.roving.focus_handle(&value, cx);
        let member = RovingMember::new().disabled(locked).preferred(selected);
        let tab_stop = picker.roving.bind(value.clone(), &handle, member, cx);
        let state = ColorPickerSwatchItemState {
            selected,
            disabled: locked,
            read_only: picker.settings.read_only,
            fill: color.map(|color| color.to_hsla()),
        };
        let label = match &color {
            Some(color) => describe_color(color),
            None => value.to_string(),
        };
        let entity = this.downgrade();
        let roving = picker.roving.clone();
        let (pressed, keyed) = (value.clone(), value.clone());
        Part::new(&picker.instance, "swatch-item", Some(&value), state)
            .aria_role(Role::Option)
            .focus_mode(
                FocusMode::Roving {
                    tab_stop: tab_stop && !disabled,
                },
                Some(&handle),
                false,
            )
            .aria_selected(selected)
            .aria_label(label)
            .aria(Aria::new().disabled(locked))
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |picker, cx| {
                        picker.choose(
                            &pressed,
                            ChangeOptions::event_of(Reason::ItemPress, event),
                            window,
                            cx,
                        );
                    })
                    .ok();
            })
            .on_keys(SWATCH_KEYS, move |event, window, cx| {
                roving.keydown(event, &keyed, window, cx);
            })
    }
}
