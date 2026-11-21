//! Port of `foundation/image/crop.ts`: cutting a region out of an image into a
//! new file.
//!
//! The original loads a URL, a file or an element, draws the region on a
//! canvas and encodes the canvas. Here the source is gpui's [`ImageSource`],
//! loaded and decoded the way `img` loads it, the region is drawn with
//! `tiny-skia` through the canvas's own transform, and the `image` crate
//! encodes it, all on the background executor.

use std::fmt;
use std::sync::Arc;

use futures::AsyncReadExt as _;
use gpui::http_client::HttpClient;
use gpui::{
    App, Asset, AssetSource, Image, ImageAssetLoader, ImageCacheError, ImageFormat,
    ImgResourceLoader, RenderImage, Resource, SharedString, Size, Task, Window, size,
};
use image::{ExtendedColorType, ImageEncoder, RgbaImage, imageops};
use tiny_skia::{FillRule, FilterQuality, Mask, PathBuilder, Pixmap, PixmapPaint, Rect, Transform};
use ui_sdk_kernel::watchers::register_data;
use ui_sdk_kernel::{AgentValue, json};

/// What [`crop_image`] reads: gpui's [`ImageSource`]. A file on disk, a URL or
/// an embedded asset (`Resource`), encoded bytes (`Image`, like a `Blob`), or
/// an image already decoded (`Render`, like an `<img>`, a canvas or an
/// `ImageBitmap`). `From<&str>` and `From<PathBuf>` make one.
pub use gpui::ImageSource;

/// A quarter turn clockwise, in degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum ImageRotation {
    #[default]
    Deg0 = 0,
    Deg90 = 90,
    Deg180 = 180,
    Deg270 = 270,
}

impl ImageRotation {
    /// The turn in degrees: 0, 90, 180 or 270.
    pub fn degrees(self) -> u16 {
        self as u16
    }

    /// The turn of any whole number of quarter turns, like `450` or `-90`
    /// (270), or `None` for an angle between them.
    pub fn from_degrees(degrees: i64) -> Option<Self> {
        match degrees.rem_euclid(360) {
            0 => Some(ImageRotation::Deg0),
            90 => Some(ImageRotation::Deg90),
            180 => Some(ImageRotation::Deg180),
            270 => Some(ImageRotation::Deg270),
            _ => None,
        }
    }

    /// Whether the turn swaps the image's width and height.
    pub fn is_turned(self) -> bool {
        !self.degrees().is_multiple_of(180)
    }
}

/// A region of an image, in the image's own pixels, as it shows after its
/// rotation and flips.
///
/// `observe` and history read it as the original's crop
/// ([`ImageCrop::to_data`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImageCrop {
    /// Pixels from the left edge of the turned image.
    pub x: f64,
    /// Pixels from the top edge of the turned image.
    pub y: f64,
    /// The region's width, in the image's own pixels.
    pub width: f64,
    /// The region's height, in the image's own pixels.
    pub height: f64,
    /// Turns the image clockwise before the region is taken. None by default.
    pub rotation: ImageRotation,
    /// Mirrors the turned image left to right.
    pub flip_x: bool,
    /// Mirrors the turned image top to bottom.
    pub flip_y: bool,
}

impl ImageCrop {
    /// The original's crop: its `x`, `y`, `width`, `height`, `rotation`,
    /// `flipX` and `flipY`.
    pub fn to_data(&self) -> AgentValue {
        json!({
            "x": self.x,
            "y": self.y,
            "width": self.width,
            "height": self.height,
            "rotation": self.rotation.degrees(),
            "flipX": self.flip_x,
            "flipY": self.flip_y,
        })
    }

    /// Lets `observe` and history read an image cropper's crop, `None` or a
    /// crop, as data. Image croppers call it as they're made.
    pub fn register_data() {
        register_data::<Option<ImageCrop>>(|crop| crop.as_ref().map_or(AgentValue::Null, ImageCrop::to_data));
    }

    /// A region of the image, not turned or flipped.
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
            ..Default::default()
        }
    }

    pub fn rotation(mut self, rotation: ImageRotation) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn flip_x(mut self, flip_x: bool) -> Self {
        self.flip_x = flip_x;
        self
    }

    pub fn flip_y(mut self, flip_y: bool) -> Self {
        self.flip_y = flip_y;
        self
    }
}

/// The shape [`crop_image`] keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CropShape {
    /// The whole region.
    #[default]
    Rectangle,
    /// The ellipse inside the region, with the corners left transparent,
    /// which PNG and WebP keep and JPEG fills with black.
    Circle,
}

/// Options for [`crop_image`].
#[derive(Clone, Debug, PartialEq)]
pub struct CropImageOptions {
    /// The file's format, the original's `type`. A format that can't be
    /// written writes PNG, which the returned file's format says. PNG by
    /// default.
    pub format: ImageFormat,
    /// From 0 to 1, for JPEG. Left out, it's 0.92, as browsers pick. WebP is
    /// written lossless, so it doesn't take one.
    pub quality: Option<f64>,
    /// The output's width in pixels. Left out, it follows `height` at the
    /// crop's ratio, or the crop's own width.
    pub width: Option<f64>,
    /// The output's height in pixels. Left out, it follows `width` at the
    /// crop's ratio, or the crop's own height.
    pub height: Option<f64>,
    /// [`CropShape::Circle`] keeps the ellipse inside the crop. A rectangle by
    /// default.
    pub shape: CropShape,
    /// The file's name. Left out, it's the source file's name with the new
    /// extension, or `"image"` and the extension.
    pub name: Option<SharedString>,
}

impl Default for CropImageOptions {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            quality: None,
            width: None,
            height: None,
            shape: CropShape::Rectangle,
            name: None,
        }
    }
}

impl CropImageOptions {
    pub fn format(mut self, format: ImageFormat) -> Self {
        self.format = format;
        self
    }

    pub fn quality(mut self, quality: f64) -> Self {
        self.quality = Some(quality);
        self
    }

    pub fn width(mut self, width: f64) -> Self {
        self.width = Some(width);
        self
    }

    pub fn height(mut self, height: f64) -> Self {
        self.height = Some(height);
        self
    }

    pub fn shape(mut self, shape: CropShape) -> Self {
        self.shape = shape;
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// What [`crop_image`] returns: an image file, as the original's `File`. Its
/// image is the encoded file, its format the file's type: `img` shows it, and
/// its bytes are what an upload sends.
#[derive(Clone, Debug)]
pub struct ImageFile {
    /// The file's name, like `"photo.png"`.
    pub name: SharedString,
    /// The encoded image.
    pub image: Arc<Image>,
}

impl ImageFile {
    /// The file's format.
    pub fn format(&self) -> ImageFormat {
        self.image.format()
    }

    /// The file's MIME type, like `"image/png"`.
    pub fn mime_type(&self) -> &'static str {
        self.image.format().mime_type()
    }

    /// The file's bytes.
    pub fn bytes(&self) -> &[u8] {
        self.image.bytes()
    }
}

/// Why [`crop_image`] failed.
#[derive(Clone, Debug)]
pub enum CropImageError {
    /// The crop had no width or no height.
    EmptyCrop,
    /// The image couldn't load or decode.
    Load(ImageCacheError),
    /// The image couldn't be encoded.
    Encode(SharedString),
}

impl fmt::Display for CropImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CropImageError::EmptyCrop => f.write_str("A crop needs a width and a height above 0."),
            CropImageError::Load(error) => write!(f, "The image couldn't load: {error}"),
            CropImageError::Encode(reason) => write!(f, "The image couldn't be encoded: {reason}"),
        }
    }
}

impl std::error::Error for CropImageError {}

impl From<ImageCacheError> for CropImageError {
    fn from(error: ImageCacheError) -> Self {
        CropImageError::Load(error)
    }
}

/// A decoded image's size, in its pixels: the natural size of any image but
/// an SVG, which gpui draws at twice its own size. [`use_natural_size`] reads
/// an SVG's.
pub fn size_of(image: &RenderImage) -> Size<u32> {
    let natural = image.size(0);
    size(natural.width.0.max(0) as u32, natural.height.0.max(0) as u32)
}

// A decoded image's size as a natural size.
fn pixels_of(image: &RenderImage) -> Size<f64> {
    let pixels = size_of(image);
    size(f64::from(pixels.width), f64::from(pixels.height))
}

// Where an SVG's bytes are: in the source, or in a file, at a URL or in an
// asset.
#[derive(Clone, Hash)]
enum SvgSource {
    Image(Arc<Image>),
    Resource(Resource),
}

impl SvgSource {
    // The source, when it's an SVG. gpui decodes an `Image` by its format,
    // and a file, a URL or an asset by its bytes, which are read again only
    // for one named like an SVG.
    fn of(source: &ImageSource) -> Option<Self> {
        match source {
            ImageSource::Image(image) => {
                (image.format() == ImageFormat::Svg).then(|| SvgSource::Image(image.clone()))
            }
            ImageSource::Resource(resource) => {
                named_like_svg(resource).then(|| SvgSource::Resource(resource.clone()))
            }
            ImageSource::Render(_) | ImageSource::Custom(_) => None,
        }
    }
}

// Whether a file, a URL or an asset ends in `.svg` or `.svgz`.
fn named_like_svg(resource: &Resource) -> bool {
    let name = match resource {
        Resource::Path(path) => {
            return path.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case("svg") || extension.eq_ignore_ascii_case("svgz")
            });
        }
        Resource::Embedded(path) => path.as_ref(),
        // A URL's path, without its query or fragment.
        Resource::Uri(uri) => uri.as_ref().split(['?', '#']).next().unwrap_or_default(),
    };
    let name = name.to_ascii_lowercase();
    name.ends_with(".svg") || name.ends_with(".svgz")
}

// An SVG's natural size, as an `<img>` reads it: the `width` and `height` it
// gives itself, or its `viewBox`'s. gpui draws an SVG at twice that size, so
// the image it decodes doesn't say. `None` for one that can't be read, or a
// file named like an SVG that holds a photo, as gpui reads it.
enum SvgNatural {}

impl Asset for SvgNatural {
    type Source = SvgSource;
    type Output = Option<Size<f64>>;

    fn load(source: Self::Source, cx: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let assets = cx.asset_source().clone();
        async move {
            match source {
                SvgSource::Image(image) => svg_size(image.bytes()),
                SvgSource::Resource(resource) => {
                    let bytes = read(resource, client, assets).await?;
                    if image::guess_format(&bytes).is_ok() {
                        return None;
                    }
                    svg_size(&bytes)
                }
            }
        }
    }
}

fn svg_size(bytes: &[u8]) -> Option<Size<f64>> {
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).ok()?;
    let own = tree.size();
    (own.width() > 0.0 && own.height() > 0.0)
        .then(|| size(f64::from(own.width()), f64::from(own.height())))
}

// A file's, a URL's or an asset's bytes, read as gpui reads them for `img`.
async fn read(
    resource: Resource,
    client: Arc<dyn HttpClient>,
    assets: Arc<dyn AssetSource>,
) -> Option<Vec<u8>> {
    match resource {
        Resource::Path(path) => std::fs::read(&*path).ok(),
        Resource::Embedded(path) => assets.load(&path).ok().flatten().map(|bytes| bytes.into_owned()),
        Resource::Uri(uri) => {
            let mut response = client.get(uri.as_ref(), ().into(), true).await.ok()?;
            if !response.status().is_success() {
                return None;
            }
            let mut body = Vec::new();
            response.body_mut().read_to_end(&mut body).await.ok()?;
            Some(body)
        }
    }
}

// Decodes an `Image`'s bytes, as gpui's own decoder for `img` does, keeping the
// error, which `Image::use_render_image` drops.
enum DecodeImage {}

impl Asset for DecodeImage {
    type Source = Arc<Image>;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        async move { source.to_image_data(renderer).map_err(ImageCacheError::from) }
    }
}

/// The image a source names, decoded, as `img` shows it this frame: `None`
/// while it loads, then the image or why it failed. A file, a URL or an asset
/// shares `img`'s cache, so it loads once for both; the window draws again
/// when it's done. This is the original's `complete` and `naturalWidth`,
/// which say whether an image loaded or broke.
pub fn use_image(
    source: &ImageSource,
    window: &mut Window,
    cx: &mut App,
) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
    match source {
        ImageSource::Resource(resource) => window.use_asset::<ImgResourceLoader>(resource, cx),
        ImageSource::Render(image) => Some(Ok(image.clone())),
        ImageSource::Image(image) => window.use_asset::<DecodeImage>(image, cx),
        ImageSource::Custom(load) => load(window, cx),
    }
}

/// The natural size of the image a source names, in its own pixels, as `img`
/// shows it this frame: the original's `naturalWidth` and `naturalHeight`.
/// `None` while it loads, then the size or why it failed. An SVG's is the
/// `width` and `height` it gives itself, or its `viewBox`'s, as an `<img>`
/// reads it, where gpui draws it at twice that. gpui tells an SVG file, URL
/// or asset by its bytes, and this by its name, `.svg` or `.svgz`: one named
/// otherwise counts in the pixels gpui draws it with. A crop counts in this
/// size, which [`crop_image`] cuts in.
pub fn use_natural_size(
    source: &ImageSource,
    window: &mut Window,
    cx: &mut App,
) -> Option<Result<Size<f64>, ImageCacheError>> {
    let image = match use_image(source, window, cx)? {
        Ok(image) => image,
        Err(error) => return Some(Err(error)),
    };
    let own = match SvgSource::of(source) {
        Some(svg) => window.use_asset::<SvgNatural>(&svg, cx)?,
        None => None,
    };
    Some(Ok(own.unwrap_or_else(|| pixels_of(&image))))
}

// An SVG source's own size, read on the background executor, or from what
// `use_natural_size` read already. `None` for any other source.
fn own_size(source: &ImageSource, cx: &mut App) -> Task<Option<Size<f64>>> {
    let Some(svg) = SvgSource::of(source) else {
        return Task::ready(None);
    };
    if let Some(read) = cx.fetch_asset::<SvgNatural>(&svg) {
        return Task::ready(read);
    }
    let reading = <SvgNatural as Asset>::load(svg, cx);
    cx.background_executor().spawn(reading)
}

/// Loads and decodes a source on the background executor: the original's
/// `load`. A file, a URL or an asset `img` already loaded comes from its
/// cache. A custom source can only load in a window, so it fails here: pass
/// what [`use_image`] gives for it.
pub fn load_image(
    source: &ImageSource,
    cx: &mut App,
) -> Task<Result<Arc<RenderImage>, ImageCacheError>> {
    match source {
        ImageSource::Resource(resource) => {
            // `img` shares this cache: an image it loaded is ready, and one it
            // hasn't starts loading for it while the crop loads its own copy.
            if let Some(loaded) = cx.fetch_asset::<ImgResourceLoader>(resource) {
                return Task::ready(loaded);
            }
            let loading = <ImageAssetLoader as Asset>::load(resource.clone(), cx);
            cx.background_executor().spawn(loading)
        }
        ImageSource::Render(image) => Task::ready(Ok(image.clone())),
        ImageSource::Image(image) => {
            let loading = <DecodeImage as Asset>::load(image.clone(), cx);
            cx.background_executor().spawn(loading)
        }
        ImageSource::Custom(_) => Task::ready(Err(ImageCacheError::Asset(
            "A custom image source can't load outside a window.".into(),
        ))),
    }
}

fn output_size(crop: &ImageCrop, options: &CropImageOptions) -> (u32, u32) {
    let ratio = crop.width / crop.height;
    let width = options.width.unwrap_or(match options.height {
        None => crop.width,
        Some(height) => height * ratio,
    });
    let height = options.height.unwrap_or(width / ratio);
    let side = |value: f64| js_round(value).max(1.0).min(u32::MAX as f64) as u32;
    (side(width), side(height))
}

// The decoded image as a premultiplied pixmap, resized along its own axes by
// `scale`, in its pixels, when that shrinks it. Browsers draw a shrinking
// image from a smaller copy (mipmaps) at "high" smoothing, so it doesn't
// alias.
fn source_pixmap(image: &RenderImage, scale: (f64, f64)) -> Option<Pixmap> {
    let natural = size_of(image);
    let bytes = image.as_bytes(0)?;
    let mut rgba = Vec::with_capacity(bytes.len());
    for pixel in bytes.as_chunks::<4>().0 {
        // gpui keeps frames as BGRA with straight alpha.
        let (blue, green, red, alpha) = (pixel[0], pixel[1], pixel[2], pixel[3]);
        rgba.extend_from_slice(&[
            premultiply(red, alpha),
            premultiply(green, alpha),
            premultiply(blue, alpha),
            alpha,
        ]);
    }
    let width = |scale: f64, natural: u32| {
        if scale < 1.0 {
            js_round(f64::from(natural) * scale).max(1.0) as u32
        } else {
            natural
        }
    };
    let target = (width(scale.0, natural.width), width(scale.1, natural.height));
    let (rgba, target) = if target != (natural.width, natural.height) {
        let premultiplied = RgbaImage::from_raw(natural.width, natural.height, rgba)?;
        // Resampling premultiplied pixels keeps transparent edges from darkening.
        let mut resized =
            imageops::resize(&premultiplied, target.0, target.1, imageops::FilterType::CatmullRom);
        for pixel in resized.pixels_mut() {
            let alpha = pixel[3];
            for channel in &mut pixel.0[..3] {
                *channel = (*channel).min(alpha);
            }
        }
        (resized.into_raw(), target)
    } else {
        (rgba, target)
    };
    Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(target.0, target.1)?)
}

fn premultiply(channel: u8, alpha: u8) -> u8 {
    ((u16::from(channel) * u16::from(alpha) + 127) / 255) as u8
}

// Draws the crop of an image whose natural size is `natural`, which its
// pixels cover, like an SVG gpui drew at twice its own size.
fn draw_crop(
    image: &RenderImage,
    natural: Size<f64>,
    crop: &ImageCrop,
    output: (u32, u32),
    circle: bool,
) -> Option<Pixmap> {
    let (natural_width, natural_height) = (natural.width, natural.height);
    let rotation = crop.rotation;
    let turned = rotation.is_turned();
    let shown = if turned {
        (natural_height, natural_width)
    } else {
        (natural_width, natural_height)
    };
    let scale = (
        f64::from(output.0) / crop.width,
        f64::from(output.1) / crop.height,
    );
    // The scale along the image's own axes, which a quarter turn swaps, in
    // its pixels.
    let own_scale = if turned { (scale.1, scale.0) } else { scale };
    let pixels = pixels_of(image);
    let source = source_pixmap(
        image,
        (
            own_scale.0 * natural_width / pixels.width,
            own_scale.1 * natural_height / pixels.height,
        ),
    )?;
    let resized = (f64::from(source.width()), f64::from(source.height()));
    let transform = Transform::from_scale(scale.0 as f32, scale.1 as f32)
        .pre_translate((shown.0 / 2.0 - crop.x) as f32, (shown.1 / 2.0 - crop.y) as f32)
        .pre_scale(
            if crop.flip_x { -1.0 } else { 1.0 },
            if crop.flip_y { -1.0 } else { 1.0 },
        )
        .pre_rotate(f32::from(rotation.degrees()))
        // The source is the image drawn at `-natural / 2`, resized.
        .pre_scale(
            (natural_width / resized.0) as f32,
            (natural_height / resized.1) as f32,
        )
        .pre_translate((-resized.0 / 2.0) as f32, (-resized.1 / 2.0) as f32);
    let mut target = Pixmap::new(output.0, output.1)?;
    let mask = if circle {
        let mut mask = Mask::new(output.0, output.1)?;
        let oval = Rect::from_xywh(0.0, 0.0, output.0 as f32, output.1 as f32)?;
        mask.fill_path(
            &PathBuilder::from_oval(oval)?,
            FillRule::Winding,
            true,
            Transform::identity(),
        );
        Some(mask)
    } else {
        None
    };
    let paint = PixmapPaint {
        quality: smoothing(transform),
        ..PixmapPaint::default()
    };
    target.draw_pixmap(0, 0, source.as_ref(), &paint, transform, mask.as_ref());
    Some(target)
}

// "high" smoothing as a canvas applies it: none when the source lands on
// whole pixels at its own size, bicubic when it grows, and bilinear on the
// already shrunk copy otherwise.
fn smoothing(transform: Transform) -> FilterQuality {
    let near = |value: f32, target: f32| (value - target).abs() < 1e-4;
    let whole = |value: f32| near(value, value.round());
    let (sx, sy, kx, ky) = (transform.sx, transform.sy, transform.kx, transform.ky);
    let axis_aligned = (near(kx, 0.0) && near(ky, 0.0)) || (near(sx, 0.0) && near(sy, 0.0));
    let scale_x = (sx * sx + ky * ky).sqrt();
    let scale_y = (kx * kx + sy * sy).sqrt();
    if axis_aligned
        && near(scale_x, 1.0)
        && near(scale_y, 1.0)
        && whole(transform.tx)
        && whole(transform.ty)
    {
        FilterQuality::Nearest
    } else if scale_x > 1.0 + 1e-4 || scale_y > 1.0 + 1e-4 {
        FilterQuality::Bicubic
    } else {
        FilterQuality::Bilinear
    }
}

// Encodes the drawn region as the format, or as PNG when the format can't be
// written, the way a canvas does. JPEG has no alpha, so transparent pixels
// turn black: the premultiplied channels are the color over black.
fn encode(
    pixmap: &Pixmap,
    format: ImageFormat,
    quality: Option<f64>,
) -> Result<(ImageFormat, Vec<u8>), CropImageError> {
    let (width, height) = (pixmap.width(), pixmap.height());
    let format = match format {
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Webp => format,
        _ => ImageFormat::Png,
    };
    let encoding_failed = |error: image::ImageError| CropImageError::Encode(error.to_string().into());
    let mut bytes = Vec::new();
    if format == ImageFormat::Jpeg {
        let rgb: Vec<u8> = pixmap
            .pixels()
            .iter()
            .flat_map(|pixel| [pixel.red(), pixel.green(), pixel.blue()])
            .collect();
        let quality = quality
            .filter(|quality| (0.0..=1.0).contains(quality))
            .unwrap_or(0.92);
        let quality = js_round(quality * 100.0).clamp(1.0, 100.0) as u8;
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode(&rgb, width, height, ExtendedColorType::Rgb8)
            .map_err(encoding_failed)?;
        return Ok((format, bytes));
    }
    let rgba: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    if format == ImageFormat::Webp {
        image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
            .encode(&rgba, width, height, ExtendedColorType::Rgba8)
            .map_err(encoding_failed)?;
    } else {
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&rgba, width, height, ExtendedColorType::Rgba8)
            .map_err(encoding_failed)?;
    }
    Ok((format, bytes))
}

// The name of the file a source was read from, without its extension, as a
// `File`'s `name` reads: only a file on disk has one.
fn base_name(source: &ImageSource) -> Option<String> {
    let ImageSource::Resource(Resource::Path(path)) = source else {
        return None;
    };
    let name = path.file_name()?.to_string_lossy();
    if name.is_empty() {
        return None;
    }
    // `name.replace(/\.[^.]*$/, "")`
    Some(match name.rfind('.') {
        Some(dot) => name[..dot].to_string(),
        None => name.to_string(),
    })
}

fn name_of(base: Option<String>, format: ImageFormat, name: Option<SharedString>) -> SharedString {
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        return name;
    }
    let base = base.unwrap_or_else(|| "image".to_string());
    format!("{base}.{}", format.extension()).into()
}

/// Cuts a region out of an image and returns it as a new image file. The
/// region is in the image's own pixels, after `rotation` and the flips, as an
/// Image Cropper hands it back: an SVG's in the size it gives itself
/// ([`use_natural_size`]), though gpui draws it at twice that. Loading,
/// drawing and encoding run on the background executor; drop the task to
/// cancel it.
///
/// ```text
/// let avatar = crop_image(
///     photo,
///     ImageCrop::new(120., 40., 600., 600.),
///     CropImageOptions::default()
///         .format(ImageFormat::Webp)
///         .width(256.)
///         .shape(CropShape::Circle),
///     cx,
/// );
/// ```
pub fn crop_image(
    source: impl Into<ImageSource>,
    crop: ImageCrop,
    options: CropImageOptions,
    cx: &mut App,
) -> Task<Result<ImageFile, CropImageError>> {
    if !(crop.width > 0.0 && crop.height > 0.0) {
        return Task::ready(Err(CropImageError::EmptyCrop));
    }
    let source = source.into();
    let base = base_name(&source);
    let loading = load_image(&source, cx);
    let own = own_size(&source, cx);
    cx.background_executor().spawn(async move {
        let image = loading.await?;
        let natural = own.await.unwrap_or_else(|| pixels_of(&image));
        let output = output_size(&crop, &options);
        let pixmap = draw_crop(&image, natural, &crop, output, options.shape == CropShape::Circle)
            .ok_or_else(|| CropImageError::Encode("the region can't be drawn".into()))?;
        let (format, bytes) = encode(&pixmap, options.format, options.quality)?;
        Ok(ImageFile {
            name: name_of(base, format, options.name),
            image: Arc::new(Image::from_bytes(format, bytes)),
        })
    })
}

/// `Math.round`: halves round up, toward positive infinity.
fn js_round(value: f64) -> f64 {
    let up = value.ceil();
    if up - 0.5 > value { up - 1.0 } else { up }
}
