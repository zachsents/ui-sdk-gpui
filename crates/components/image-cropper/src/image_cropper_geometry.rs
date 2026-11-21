//! Port of `components/image-cropper/image-cropper-geometry.ts`: where a crop
//! sits on the image and the image on screen. Crops count in the image's own
//! pixels, from the top left of the image as it shows after it's turned and
//! flipped, as `crop_image` reads them.

use ui_sdk_direct_manipulation::{
    ResizeBounds, ResizeBox, ResizeHandle, ResizeLimits, ResizeSize, resize_box,
};
use ui_sdk_image::{ImageCrop, ImageRotation};
use ui_sdk_number::clamp;

use crate::image_cropper::{ImageCropperFlipDirection, ImageCropperMode};

/// A width and a height, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// A box by its left and top edges and its size, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// A point on the image, in its own pixels, or on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A crop with every field, the original's `Required<ImageCrop>`: the image
/// crate's [`ImageCrop`], whose rotation and flips always have a value.
pub type Crop = ImageCrop;

/// What keeps a crop's size in bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub aspect: Option<f64>,
    pub min_width: f64,
    pub min_height: f64,
    pub max_zoom: f64,
}

// `Math.min` and `Math.max`: a NaN anywhere gives NaN.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

// `Math.round`: halves round up, toward positive infinity.
pub(crate) fn js_round(value: f64) -> f64 {
    let up = value.ceil();
    if up - 0.5 > value { up - 1.0 } else { up }
}

/// The image's size as it shows, which a quarter turn swaps.
pub fn shown_size(natural: Size, rotation: ImageRotation) -> Size {
    if rotation.is_turned() {
        Size::new(natural.height, natural.width)
    } else {
        natural
    }
}

/// Whether two crops are the same in every field.
pub fn same_crop(a: &Crop, b: &Crop) -> bool {
    a == b
}

/// The crop on whole pixels, at least one by one.
pub fn round(crop: Crop) -> Crop {
    Crop {
        x: js_round(crop.x),
        y: js_round(crop.y),
        width: js_max(1.0, js_round(crop.width)),
        height: js_max(1.0, js_round(crop.height)),
        ..crop
    }
}

/// How far the crop magnifies: 1 when it's as large as the image allows at
/// its ratio.
pub fn zoom_of(crop: &Crop, natural: Size) -> f64 {
    let shown = shown_size(natural, crop.rotation);
    js_min(shown.width / crop.width, shown.height / crop.height)
}

// The largest box at `ratio` inside `shown`, which zoom 1 crops.
fn largest(shown: Size, ratio: Option<f64>) -> Size {
    let Some(ratio) = ratio.filter(|ratio| *ratio != 0.0 && !ratio.is_nan()) else {
        return shown;
    };
    let width = js_min(shown.width, shown.height * ratio);
    Size::new(width, width / ratio)
}

/// The smallest a crop gets, from the minimum size and the zoom limit.
pub fn smallest(shown: Size, limits: &Limits) -> Size {
    let Limits {
        aspect,
        min_width,
        min_height,
        max_zoom,
    } = *limits;
    let Some(aspect) = aspect.filter(|aspect| *aspect != 0.0 && !aspect.is_nan()) else {
        return Size::new(
            js_max(min_width, shown.width / max_zoom),
            js_max(min_height, shown.height / max_zoom),
        );
    };
    let width = js_max(
        js_max(min_width, min_height * aspect),
        largest(shown, Some(aspect)).width / max_zoom,
    );
    Size::new(width, width / aspect)
}

// A box of `size` centered on a point, kept inside the image.
fn centered(size: Size, center: Point, shown: Size) -> ResizeBox {
    ResizeBox {
        x: clamp(
            center.x - size.width / 2.0,
            0.0,
            js_max(0.0, shown.width - size.width),
        ),
        y: clamp(
            center.y - size.height / 2.0,
            0.0,
            js_max(0.0, shown.height - size.height),
        ),
        width: size.width,
        height: size.height,
    }
}

// The crop with the box's place and size.
fn with_box(crop: Crop, r#box: ResizeBox) -> Crop {
    Crop {
        x: r#box.x,
        y: r#box.y,
        width: r#box.width,
        height: r#box.height,
        ..crop
    }
}

/// The crop an image starts with, in the middle: in a box, most of the image
/// at the ratio, so its edges show; in a frame, the largest.
pub fn initial_crop(natural: Size, limits: &Limits, mode: ImageCropperMode) -> Crop {
    let full = largest(natural, limits.aspect);
    let min = smallest(natural, limits);
    let inset = if mode == ImageCropperMode::Frame {
        1.0
    } else {
        js_min(
            1.0,
            js_max(js_max(0.8, min.width / full.width), min.height / full.height),
        )
    };
    let size = Size::new(full.width * inset, full.height * inset);
    let r#box = centered(
        size,
        Point {
            x: natural.width / 2.0,
            y: natural.height / 2.0,
        },
        natural,
    );
    round(with_box(Crop::default(), r#box))
}

/// The crop after a handle moves by `dx` and `dy` image pixels, or the whole
/// crop with `Move`. `keep_ratio` keeps the ratio it has, like Shift on a
/// corner.
pub fn resize_crop(
    crop: Crop,
    natural: Size,
    handle: ResizeHandle,
    dx: f64,
    dy: f64,
    limits: &Limits,
    keep_ratio: bool,
) -> Crop {
    let shown = shown_size(natural, crop.rotation);
    let aspect = limits
        .aspect
        .or_else(|| keep_ratio.then(|| crop.width / crop.height));
    let min = smallest(shown, &Limits { aspect, ..*limits });
    let mut resize = ResizeLimits::new()
        .bounds(ResizeBounds::new(0.0, 0.0, shown.width, shown.height))
        .min(ResizeSize::new(min.width, min.height));
    resize.aspect = aspect;
    let from = ResizeBox::new(crop.x, crop.y, crop.width, crop.height);
    round(with_box(crop, resize_box(from, handle, dx, dy, &resize)))
}

/// The crop at a zoom, keeping the image point `at` where it is, or the
/// crop's middle. Its ratio stays.
pub fn zoom_crop(crop: Crop, natural: Size, zoom: f64, limits: &Limits, at: Option<Point>) -> Crop {
    let shown = shown_size(natural, crop.rotation);
    let ratio = limits.aspect.unwrap_or(crop.width / crop.height);
    let full = largest(shown, Some(ratio));
    let least = smallest(
        shown,
        &Limits {
            aspect: Some(ratio),
            ..*limits
        },
    );
    let most = js_max(1.0, js_min(full.width / least.width, full.height / least.height));
    let next = clamp(zoom, 1.0, most);
    let width = full.width / next;
    let height = full.height / next;
    let point = at.unwrap_or(Point {
        x: crop.x + crop.width / 2.0,
        y: crop.y + crop.height / 2.0,
    });
    let k = width / crop.width;
    let r#box = ResizeBox {
        x: clamp(point.x - (point.x - crop.x) * k, 0.0, shown.width - width),
        y: clamp(
            point.y - (point.y - crop.y) * (height / crop.height),
            0.0,
            shown.height - height,
        ),
        width,
        height,
    };
    round(with_box(crop, r#box))
}

/// The crop after the image turns a quarter clockwise, or counterclockwise
/// for a negative `direction`. The crop turns with it; at a fixed ratio it
/// keeps its zoom instead. A flip on one axis is on the other once turned.
pub fn rotate_crop(crop: Crop, natural: Size, direction: i32, limits: &Limits) -> Crop {
    let shown = shown_size(natural, crop.rotation);
    let clockwise = direction >= 0;
    let r#box = if clockwise {
        ResizeBox {
            x: shown.height - crop.y - crop.height,
            y: crop.x,
            width: crop.height,
            height: crop.width,
        }
    } else {
        ResizeBox {
            x: crop.y,
            y: shown.width - crop.x - crop.width,
            width: crop.height,
            height: crop.width,
        }
    };
    let turn = if clockwise { 90 } else { 270 };
    let rotation = ImageRotation::from_degrees(i64::from(crop.rotation.degrees()) + turn).unwrap_or_default();
    let next = Crop {
        rotation,
        flip_x: crop.flip_y,
        flip_y: crop.flip_x,
        ..with_box(crop, r#box)
    };
    let Some(aspect) = limits.aspect else {
        return round(next);
    };
    let turned = shown_size(natural, rotation);
    let full = largest(turned, Some(aspect));
    let zoom = js_min(
        zoom_of(&crop, natural),
        full.width / smallest(turned, limits).width,
    );
    let size = Size::new(full.width / zoom, full.height / zoom);
    let center = Point {
        x: r#box.x + r#box.width / 2.0,
        y: r#box.y + r#box.height / 2.0,
    };
    round(with_box(next, centered(size, center, turned)))
}

/// The crop after the image mirrors left to right, or top to bottom.
pub fn flip_crop(crop: Crop, natural: Size, direction: ImageCropperFlipDirection) -> Crop {
    let shown = shown_size(natural, crop.rotation);
    match direction {
        ImageCropperFlipDirection::Horizontal => Crop {
            x: shown.width - crop.x - crop.width,
            flip_x: !crop.flip_x,
            ..crop
        },
        ImageCropperFlipDirection::Vertical => Crop {
            y: shown.height - crop.y - crop.height,
            flip_y: !crop.flip_y,
            ..crop
        },
    }
}

/// The crop at a new ratio, as large as it was, about its middle.
pub fn fit_crop(crop: Crop, natural: Size, limits: &Limits) -> Crop {
    let Some(aspect) = limits.aspect else {
        return crop;
    };
    if (crop.width / aspect - crop.height).abs() <= 1.0 {
        return crop;
    }
    let shown = shown_size(natural, crop.rotation);
    let full = largest(shown, Some(aspect));
    let zoom = js_min(
        zoom_of(&crop, natural),
        full.width / smallest(shown, limits).width,
    );
    let size = Size::new(full.width / zoom, full.height / zoom);
    let center = Point {
        x: crop.x + crop.width / 2.0,
        y: crop.y + crop.height / 2.0,
    };
    round(with_box(crop, centered(size, center, shown)))
}

/// Where the image and the selection sit in the viewport, in pixels from
/// its padding box, and how many of them one image pixel takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub scale: f64,
    pub image: Rect,
    pub selection: Rect,
}

/// Where the image and the selection sit in the viewport's content box
/// `content`, and how many pixels one image pixel takes. A box shows the
/// whole image and the crop on it; a frame shows the crop as large as it
/// fits, with the image around it.
pub fn layout_of(
    crop: &Crop,
    natural: Size,
    content: Rect,
    mode: ImageCropperMode,
    aspect: Option<f64>,
) -> Layout {
    let shown = shown_size(natural, crop.rotation);
    if mode == ImageCropperMode::Frame {
        let ratio = aspect.unwrap_or(crop.width / crop.height);
        let width = js_min(content.width, content.height * ratio);
        let height = width / ratio;
        let selection = Rect {
            left: content.left + (content.width - width) / 2.0,
            top: content.top + (content.height - height) / 2.0,
            width,
            height,
        };
        let scale = width / crop.width;
        let image = Rect {
            left: selection.left - crop.x * scale,
            top: selection.top - crop.y * scale,
            width: shown.width * scale,
            height: shown.height * scale,
        };
        return Layout {
            scale,
            image,
            selection,
        };
    }
    let scale = js_min(content.width / shown.width, content.height / shown.height);
    let image = Rect {
        left: content.left + (content.width - shown.width * scale) / 2.0,
        top: content.top + (content.height - shown.height * scale) / 2.0,
        width: shown.width * scale,
        height: shown.height * scale,
    };
    let selection = Rect {
        left: image.left + crop.x * scale,
        top: image.top + crop.y * scale,
        width: crop.width * scale,
        height: crop.height * scale,
    };
    Layout {
        scale,
        image,
        selection,
    }
}
