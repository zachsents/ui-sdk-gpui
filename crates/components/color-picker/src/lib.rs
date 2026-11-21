//! Port of `components/color-picker`: choose a color by dragging across an
//! area and along sliders, typing any CSS color, or picking a preset. The
//! value is a CSS color string, and every part edits it.
//!
//! `color-picker.ts` → [`color_picker`](mod@color_picker),
//! `color-picker-api.ts` → [`color_picker_api`],
//! `color-picker-channels.ts` → [`color_picker_channels`],
//! `color-picker-parts.ts` → [`color_picker_parts`]. The component has no
//! `index.ts`; these are its re-exports, with the color types it shares
//! from `ui_sdk_color`.
//!
//! An app makes a [`ColorPicker`] with `cx.new(|cx| ColorPicker::new(options,
//! window, cx))` and renders [`ColorPicker::root`] on every frame, with the
//! parts it needs inside: [`ColorPicker::area`] (with
//! [`ColorPicker::area_canvas`] and [`ColorPicker::area_thumb`] inside),
//! [`ColorPicker::slider`]s (each with its [`ColorPicker::slider_thumb`]),
//! [`ColorPicker::swatch`], [`ColorPicker::input`],
//! [`ColorPicker::channel_input`]s and [`ColorPicker::swatches`] around
//! [`ColorPicker::swatch_item`]s. The parts keep a hue while the color turns
//! gray or black, and a drag can go past the gamut's edge, with the value
//! the closest color inside it, until the drag ends.
//!
//! # In gpui
//!
//! - What the original writes as CSS variables is part state: `fill` on
//!   every part (`--color-picker-color`), an area's `background`
//!   (`--color-area-background`) and its thumb's `fraction_x` and
//!   `fraction_y` (`--area-x`, `--area-y`), a slider's `background`
//!   (`--color-slider-background`) and `percent`
//!   (`--color-slider-thumb-position`). gpui's gradients have two stops, so
//!   [`ChannelGradient`] and [`AreaGradient`] are elements of layered
//!   gradients ([`ChannelGradient::element`], [`AreaGradient::element`]) to
//!   put first in their part.
//! - gpui paints in sRGB, so a fill or a gradient clips a wider color, as a
//!   browser's `rgb()` does. The area's canvas is an image, at most 160
//!   pixels across like the original's, painted through gpui's sprite atlas;
//!   a P3 gamut's colors in it are the sRGB they clip to, while its edge is
//!   still P3's. A picture no longer shown leaves the atlas.
//! - The text input and the channel inputs are the native crate's text
//!   fields: a text field, and number fields over each channel's range and
//!   step, which step with their arrows and report a change on Enter, blur
//!   or a step, as `<input type="number">` does. The picker makes one for
//!   each channel of its space, so render one channel input per channel.
//!   Enter in any of them submits its form implicitly, as a native input's
//!   does.
//! - The thumbs place themselves as the original's inline styles do: an
//!   area's from its start and top, a slider's from its start, or bottom
//!   when vertical. gpui has no `translate`, so center a thumb with negative
//!   margins half its size.
//! - The root's part in forms submits the value under `name`; a field's
//!   label names the group and focuses its first thumb.
//! - `set_options` and `set_controlled_value` take the window: the inputs
//!   show a new value at once.
//!
//! # Dropped, and why
//!
//! - The eye dropper: the `eye-dropper` slot, `pickFromScreen`,
//!   `setEyeDropper`, the `eyeDropper` state and the `EyeDropperPick`
//!   reason. The original uses the browser's `EyeDropper`, and gpui has no
//!   way to sample a color from the screen.
//! - `createColorPickerController`, `ColorPickerController` and the
//!   `controller` option: the app holds the entity, whose methods are the
//!   controller's.
//! - The `hidden-input` slot: the root reports to forms through its
//!   internals.
//! - `restore` and the `Restore` reason: gpui restores no forms after a
//!   reload.
//! - `takeTypedText`: text typed before the page's script ran. There is
//!   always a window.
//! - `spaceOf`'s fallback for an unknown space: spaces are an enum.
//! - `setRTL` and the `rtl` state: right to left is the app's direction,
//!   read as parts render.
//! - `touch-action`: a drag claims its touch as it starts.
//!
//! # Deviations
//!
//! - `on_value_change` and `on_value_commit` get a
//!   [`ColorPickerValueChangeDetails`], which derefs to the kernel's
//!   `ChangeDetails` and carries `color`.
//! - `gamut` is `Option<ColorGamut>`: `None` is the original's `false`, any
//!   color.
//! - Several parts of one kind, like two sliders for one channel, share a
//!   key: the channels a slider, a channel input or an area edits are what
//!   tells them apart.

pub mod color_picker;
pub mod color_picker_api;
pub mod color_picker_channels;
pub mod color_picker_parts;

pub use color_picker::{
    ColorPicker, ColorPickerAreaProps, ColorPickerAreaState, ColorPickerChangeOptions,
    ColorPickerChannelInputProps, ColorPickerChannelInputState, ColorPickerOptions,
    ColorPickerSliderOrientation, ColorPickerSliderProps, ColorPickerSliderState, ColorPickerState,
    ColorPickerSwatchItemProps, ColorPickerSwatchItemState, ColorPickerValueChange,
    ColorPickerValueChangeDetails, ColorPickerValueChangeReason, ColorPickerValueCommit,
    ColorPickerValueCommitDetails,
};
pub use color_picker_api::value_text;
pub use color_picker_channels::{
    AreaGradient, Channel, ChannelGradient, ChannelSpec, ChannelUnit, ColorChannel, ColorPickerSpace,
    DefaultAxes, area_gradient, black, channel_gradient, channel_of, channel_text, color_in, css_of,
    default_axes, describe_color, in_space, read_channel, shown, with_channel,
};
pub use color_picker_parts::{SLIDER_KEYS, SWATCH_KEYS};
pub use ui_sdk_color::{Color, ColorFormat, ColorGamut};
