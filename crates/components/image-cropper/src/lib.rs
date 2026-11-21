//! Port of `components/image-cropper`: choose the part of an image to keep
//! by moving and resizing a selection, or by panning and zooming the image
//! under it, with a quarter-turn rotation and flips, ready for
//! `ui_sdk_image::crop_image`.
//!
//! `image-cropper.ts` → [`image_cropper`](mod@image_cropper),
//! `image-cropper-api.ts` → [`image_cropper_api`],
//! `image-cropper-geometry.ts` → [`image_cropper_geometry`],
//! `image-cropper-parts.ts` → [`image_cropper_parts`]. The component has no
//! `index.ts`; these are its re-exports.
//!
//! An app makes an [`ImageCropper`] with `cx.new(|cx| ImageCropper::new(options,
//! window, cx))` and renders [`ImageCropper::root`] on every frame, with
//! [`ImageCropper::viewport`] inside, holding [`ImageCropper::image`],
//! [`ImageCropper::selection`] (with [`ImageCropper::grid`] inside it) and
//! any [`ImageCropper::handle`]s beside it, and outside the viewport
//! [`ImageCropper::zoom`], [`ImageCropper::rotate_button`] and
//! [`ImageCropper::flip_button`]. The crop counts in the image's own pixels,
//! from the top left of the image as it shows once turned and flipped:
//! pass it to `crop_image` as it is.
//!
//! # In gpui
//!
//! - `src` is gpui's `ImageSource`, which the image part loads and decodes
//!   as `img` does (`ui_sdk_image::use_image`), reporting its natural size
//!   once it has loaded, or that it failed. gpui paints an image only as it
//!   is, so the image part paints a copy of it turned and flipped as the
//!   crop is, where the original rotates and scales its element. The copy
//!   is kept while the crop's rotation and flips stay, and leaves the sprite
//!   atlas when they change or the cropper goes. An image of more than a
//!   million pixels turns in the background, so a press doesn't hold up a
//!   frame, and shows again once it has.
//! - An SVG's natural size is the `width` and `height` it gives itself, or
//!   its `viewBox`'s, as an `<img>` reads it (`ui_sdk_image::use_natural_size`),
//!   where gpui decodes an SVG at twice that size, and the image part paints
//!   that copy.
//! - The viewport measures its content box, as the original reads its size
//!   and padding: a layer over its padding box, where the absolute parts
//!   are placed, and one that fills its content box. It's a flex column for
//!   that, so the parts in it are absolute, as the original's are.
//! - The zoom part is the original's native range input: a slider from 1 to
//!   `max_zoom` that takes a press and a drag along it, arrows by a
//!   hundredth of the range (as a range input with `step="any"` takes them),
//!   Page Up and Page Down by a tenth, Home and End, and a screen reader's
//!   adjustments, each of which sends its `input` and `change`. Its state's
//!   `fraction` is where to draw its thumb, which the browser draws for the
//!   original. Right to left, it runs from the right.
//! - A pinch: gpui follows one pointer at a time, so two fingers on the
//!   viewport can't pinch it; a trackpad's pinch, which gpui reports as
//!   such where a browser sends a Ctrl wheel, zooms a frame at its center
//!   with `Pinch`, and commits as it ends.
//! - A frame's pan follows the pointer through the cropper's drag session,
//!   which hears the release anywhere and cancels on Escape, where the
//!   original captures the pointer and listens on the document.
//! - Hidden parts: the selection and the handles are invisible and hidden
//!   from assistive technology until they show, the original's `hidden`.
//! - The wheel: gpui's deltas point the way the content moves, so a zoom
//!   reads them the other way round, as the browser's, with a line as 16
//!   pixels.
//! - `set_options` takes the window: a new aspect ratio fits the crop to it
//!   once the options are in.
//! - Aspect-ratio presets are the app's, as in the original: a new
//!   `aspect_ratio` fits the crop with `AspectRatioChange`.
//!
//! # Dropped, and why
//!
//! - `createImageCropperController`, `ImageCropperController` and the
//!   `controller` option: the app holds the entity, whose methods are the
//!   controller's.
//! - `complete` and `completeOnce`: `ImageCrop`'s rotation and flips always
//!   have a value, and a crop handed over twice compares equal by value.
//! - `live` attributes, written so a drag renders nothing: parts render
//!   from the entity's state.
//! - `draggable`, `user-select`, `touch-action` and `pointer-events`: gpui
//!   drags no images, selects no text outside text fields, and a drag claims
//!   its touch as it starts; parts with no handlers take no presses.
//! - The image's server render ("it shows as it is" until it loads): there's
//!   always a window, and nothing to paint until it loads.
//!
//! # Deviations
//!
//! - `rotate` takes a direction as an `i32`: negative turns
//!   counterclockwise, where the original takes `1 | -1`.
//! - The rotate and flip parts are [`ImageCropper::rotate_button`] and
//!   [`ImageCropper::flip_button`], named apart from the api's `rotate` and
//!   `flip`.

pub mod image_cropper;
pub mod image_cropper_api;
pub mod image_cropper_geometry;
pub mod image_cropper_parts;

pub use image_cropper::{
    ImageCropper, ImageCropperChangeOptions, ImageCropperCropCallback, ImageCropperCropChangeDetails,
    ImageCropperCropChangeReason, ImageCropperCropCommitDetails, ImageCropperFlipDirection,
    ImageCropperFlipProps, ImageCropperFlipState, ImageCropperHandlePosition, ImageCropperHandleProps,
    ImageCropperHandleState, ImageCropperMode, ImageCropperOptions, ImageCropperShape, ImageCropperState,
    ImageCropperViewportState, ImageCropperZoomState,
};
pub use image_cropper_geometry::{Layout, Rect, Size};
pub use image_cropper_parts::{CROP_KEYS, ZOOM_KEYS};
pub use ui_sdk_image::{ImageCrop, ImageRotation};
