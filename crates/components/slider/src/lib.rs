//! Port of `components/slider`: a number or a range picked along a scale, by
//! dragging a thumb, pressing the track, or with the arrow keys.
//!
//! `slider.ts` → [`slider`](mod@slider), `slider-api.ts` → [`slider_api`],
//! `slider-values.ts` → [`slider_values`], `slider-generic.ts` →
//! [`slider_generic`] (its value type `T` is [`SliderValue`]), `index.ts` →
//! these re-exports.
//!
//! An app makes a [`Slider`] with `cx.new(|cx| Slider::new(options, window,
//! cx))`, `Slider<f64>` for one thumb or `Slider<Vec<f64>>` for one number
//! per thumb, and renders [`Slider::root`] (build it first, and on every
//! frame), with [`Slider::value`] and [`Slider::control`] inside, the
//! [`Slider::track`] inside the control, and in the track the
//! [`Slider::indicator`] and a [`Slider::thumb`] per value. A press on the
//! control moves the nearest thumb there and drags it; a press on a thumb
//! drags it from where it is. Thumbs never pass each other and stay
//! `min_steps_between_thumbs` apart. Arrows step, Page Up, Page Down and
//! Shift with an arrow take the large step, Home and End go as far as the
//! thumb's neighbours let it; right to left swaps Left and Right on a
//! horizontal slider, and a vertical one grows upward. `on_value_change`
//! runs on every step, and `on_value_commit` once the pointer lifts, the key
//! comes up or a screen reader adjusts it, both with the thumb that moved.
//!
//! A single thumb is the control a label or a field names, since the root
//! clears `set_control` on its part in forms; a range is a group the label
//! names, which focuses its first thumb. The slider reports its value to its
//! form under `name`: the number, or `"start/end"` for a range. A field
//! disables it with its own `disabled`, written on the thumb or the root,
//! as the original's adapters pass the field's `disabled` in.
//!
//! # Deviations
//!
//! - The value type is a type parameter, [`SliderValue`], for `f64` and
//!   `Vec<f64>`, where the original takes `number | number[]` and narrows it.
//! - `on_value_change` and `on_value_commit` get a
//!   [`SliderValueChangeDetails`], which derefs to the kernel's
//!   `ChangeDetails` and carries `thumb`.
//! - The `thumb-input` slot, each thumb's visually hidden native range
//!   input, is the thumb itself: gpui has no native inputs, so the thumb is
//!   the slider that takes focus, keys and ARIA (its value, bounds, step,
//!   value text and orientation), and a screen reader's adjustments come as
//!   accesskit's `Increment` and `Decrement` (a step, as a native range takes
//!   them) and `SetValue`. They commit at once, and the form hears `Input`
//!   and `Change` from the thumb, as from the native input. Style focus with
//!   `focus_visible` on the thumb, the original's `:has(:focus-visible)`.
//!   Each thumb has its own part in forms, which stands for its native
//!   input: with one thumb it's the control a label or a field finds, and it
//!   submits the value under a field's name when the slider has none of its
//!   own, as the original's field names the slider's own element.
//! - Along the track, thumbs are centered on their value with auto margins
//!   inside a box as long as the track, the original's fallback for
//!   `justify-self`, which gpui lacks, so give each thumb a size along the
//!   track. Across it, the track lays out along the slider's axis and the
//!   thumb takes `align-self: center`, where the original translates it.
//! - `z-index`: gpui paints thumbs in the order they render. A press over
//!   overlapping thumbs lands on the one the original raises, the dragged one
//!   and then the one that moved last.
//! - Focus leaving a thumb while its key is down commits, as `focusout`
//!   does: the thumb listens for its blur while a key is down.
//! - `set_options` takes the window, since options that disable the slider
//!   or make it read-only mid-drag end the drag at once. A field or a
//!   fieldset disabling it ends the drag as its write notifies the slider.
//! - `--slider-thumb-position`, `--slider-indicator-start`,
//!   `--slider-indicator-size` and the `data-*` attributes are part state;
//!   the thumbs and the indicator place themselves in the track, which is
//!   `relative`, as the original's inline styles do. A thumb's own
//!   `data-dragging` is [`SliderThumbState::dragged`].
//!
//! # Dropped, and why
//!
//! - `createSliderController`, `SliderController` and the `controller`
//!   option: the app holds the entity, whose methods are the controller's.
//! - The `input` slot, a hidden input for React and Vue: the slider reports
//!   to forms through its root's internals.
//! - `restore`, the `Restore` reason and `parseRange` of saved state: gpui
//!   restores no forms after a reload.
//! - `valueAttribute`: values written as attributes, for web components.
//! - `thumbInputId` and the root's `${id}-group` id: a field writes the id
//!   its label points at on the control's part in forms, and the kernel
//!   makes element ids.
//! - `syncInput` and `rangeInputStyle`: there's no native input to keep in
//!   step or hide.
//! - `bindPageLocale` and `pageLocale`: parts format in `locale_of` as they
//!   render, which follows `set_page_locale`.
//! - `thumbValues`, `holdsValue`, `heldState` and `selects`: so a move
//!   renders only the thumbs it changes. Parts render from the entity's
//!   state.
//! - The `value` part's `for`: accesskit has no relation for it, and the
//!   part is a status that's never announced, since each thumb says its
//!   value.
//! - `touch-action: none`: following a touch claims it as it starts.

pub mod slider;
pub mod slider_api;
pub mod slider_generic;
pub mod slider_values;

pub use slider::{
    Slider, SliderChangeOptions, SliderIndicatorState, SliderOptions, SliderOrientation, SliderState,
    SliderThumbState, SliderValueChange, SliderValueChangeDetails, SliderValueChangeReason,
    SliderValueCommit, SliderValueCommitDetails, SliderValueState,
};
pub use slider_api::format_range;
pub use slider_generic::SliderValue;
pub use slider_values::{
    KeyTargetOptions, SliderRange, ThumbBounds, ThumbPress, bounds_between, fraction_along, key_target,
    move_thumb, normalize_values, percent_of, stack_at, thumb_bounds, thumb_for_direction, thumb_for_press,
    value_at,
};
