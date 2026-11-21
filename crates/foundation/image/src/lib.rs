//! Port of `foundation/image`: cut a region out of an image into a new file,
//! like the crop an Image Cropper hands back.
//!
//! `image.ts` and `index.ts` both re-export `crop.ts`'s API; this crate root
//! does for both. The original marks it experimental.
//!
//! # gpui
//!
//! What the browser does for the original, gpui and two crates do here:
//!
//! - The source is gpui's [`ImageSource`]: a file, a URL, an embedded asset,
//!   encoded bytes or a decoded [`gpui::RenderImage`]. [`load_image`] loads
//!   and decodes one as `img` does (the original's `load`), reusing what
//!   `img` already loaded, and [`use_image`] gives it during a frame, `None`
//!   until it's ready, as an `<img>`'s `complete` and `naturalWidth` tell
//!   whether it loaded. [`use_natural_size`] is its natural size then, and
//!   [`size_of`] a decoded image's pixels.
//! - An SVG's natural size is the `width` and `height` it gives itself, or
//!   its `viewBox`'s, as an `<img>` reads it, read with `usvg`. gpui draws an
//!   SVG at twice that, and [`crop_image`] cuts its crop from that copy at
//!   its own size. gpui tells an SVG file, URL or asset by its bytes, this
//!   crate by its name, `.svg` or `.svgz`: one named otherwise counts in the
//!   pixels gpui draws it with.
//! - The canvas becomes a `tiny-skia` pixmap drawn through the same chain of
//!   scale, translate, flip and rotate, and the encoder is the `image` crate's.
//!   Smoothing follows a canvas's "high" quality: an exact copy at the
//!   image's own size, bicubic when it grows, and a premultiplied Catmull-Rom
//!   copy when it shrinks, as browsers draw from mipmaps.
//! - [`crop_image`] returns a gpui `Task` that runs on the background
//!   executor, the original's promise, and resolves to an [`ImageFile`]: its
//!   name and the encoded [`gpui::Image`], which `img` shows.
//!
//! # Deviations
//!
//! - `type` is [`CropImageOptions::format`], a gpui [`gpui::ImageFormat`]
//!   instead of a MIME string. PNG, JPEG and WebP are written, the formats
//!   browsers write; any other falls back to PNG, as a browser's does.
//! - WebP is written lossless, the only WebP the `image` crate writes, so
//!   `quality` applies to JPEG alone. JPEG's default quality is 0.92, the
//!   one browsers pick.
//! - The crop's `rotation`, `flipX` and `flipY` always have a value
//!   (`Default` is unturned and unflipped), so the original's `Required<ImageCrop>`
//!   is [`ImageCrop`] itself. A rotation is [`ImageRotation`], whose
//!   `degrees` and `from_degrees` do the arithmetic the original does on
//!   numbers.
//! - The output name only takes a source's base name from a file on disk,
//!   as only a `File` has a name in the original.
//! - Dropped: `crossOrigin` on URL images, which only matters to a canvas a
//!   page can taint, and closing the `ImageBitmap`, which Rust's ownership
//!   does.

pub mod crop;

pub use crop::{
    CropImageError, CropImageOptions, CropShape, ImageCrop, ImageFile, ImageRotation, ImageSource,
    crop_image, load_image, size_of, use_image, use_natural_size,
};
