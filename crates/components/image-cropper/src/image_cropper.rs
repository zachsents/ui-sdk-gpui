//! Port of `components/image-cropper/image-cropper.ts`: an image cropper
//! around one crop of one image. This module is the public api and the
//! component; its state and what it does are `image_cropper_api.rs`'s, and
//! its parts `image_cropper_parts.rs`'s.

use std::rc::Rc;

use gpui::{App, Context, FocusHandle, Focusable, Subscription, Task, Window};
use ui_sdk_direct_manipulation::ResizeHandle;
use ui_sdk_drag_and_drop::{DragSession, PointerEvent, create_drag_session};
use ui_sdk_image::{ImageCrop, ImageRotation, ImageSource};
use ui_sdk_kernel::{
    Agent, AgentAction, AgentArgs, AgentError, AgentState, AgentValue, ChangeCallback, ChangeDetails,
    ChangeOptions, CommitCallback, Controllable, Instance, Measured, Parts, Reason, json,
};

use crate::image_cropper_api::{Dragging, Gesture, Settings, SourceKey, Status, settings};
use crate::image_cropper_geometry::{Rect, Size};
use crate::image_cropper_parts::ImageView;

/// Why an image cropper's crop wants to change: an image loaded
/// (`ImageLoad`), a drag (`Drag`), a pinch (`Pinch`), the wheel (`Wheel`),
/// an arrow key (`StepKey`), plus or minus (`ZoomKey`), the zoom input
/// (`ZoomInput`), R (`RotateKey`), the rotate part (`RotatePress`), the flip
/// part (`FlipPress`), a new aspect ratio (`AspectRatioChange`), or a call
/// (`Imperative`).
pub type ImageCropperCropChangeReason = Reason;

/// What `on_crop_change` receives after the crop.
pub type ImageCropperCropChangeDetails = ChangeDetails;

/// What `on_crop_commit` receives after the crop.
pub type ImageCropperCropCommitDetails = ChangeDetails;

/// Options for every write method: the reason and event `on_crop_change`
/// receives.
pub type ImageCropperChangeOptions = ChangeOptions;

/// `on_crop_change`, `on_before_crop_change` and `on_crop_commit`: the crop,
/// and what explains it.
pub type ImageCropperCropCallback = Rc<dyn Fn(&ImageCrop, &ChangeDetails, &mut Window, &mut App)>;

/// How the crop changes (`ImageCropperMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ImageCropperMode {
    /// Moves and resizes a selection over the whole image.
    #[default]
    Box,
    /// Pans and zooms the image under a selection that stays put, like an
    /// avatar editor.
    Frame,
}

impl ImageCropperMode {
    /// The original's name for it, its `data-mode`.
    pub fn as_str(self) -> &'static str {
        match self {
            ImageCropperMode::Box => "box",
            ImageCropperMode::Frame => "frame",
        }
    }
}

/// The selection's shape. A circle only changes how it looks, and the crop
/// is still its box; pass the same shape to `crop_image`
/// (`ImageCropperShape`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ImageCropperShape {
    #[default]
    Rectangle,
    Circle,
}

impl ImageCropperShape {
    /// The original's name for it, its `data-shape`.
    pub fn as_str(self) -> &'static str {
        match self {
            ImageCropperShape::Rectangle => "rectangle",
            ImageCropperShape::Circle => "circle",
        }
    }
}

/// Which way a flip mirrors the image: `Horizontal` left to right,
/// `Vertical` top to bottom (`ImageCropperFlipDirection`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ImageCropperFlipDirection {
    #[default]
    Horizontal,
    Vertical,
}

impl ImageCropperFlipDirection {
    /// The original's name for it, its `data-direction`.
    pub fn as_str(self) -> &'static str {
        match self {
            ImageCropperFlipDirection::Horizontal => "horizontal",
            ImageCropperFlipDirection::Vertical => "vertical",
        }
    }
}

/// The edge or corner a handle resizes, by compass point
/// (`ImageCropperHandlePosition`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageCropperHandlePosition {
    N,
    E,
    S,
    W,
    Ne,
    Nw,
    Se,
    Sw,
}

impl ImageCropperHandlePosition {
    /// Every position, the edges and then the corners.
    pub const ALL: [ImageCropperHandlePosition; 8] = [
        ImageCropperHandlePosition::N,
        ImageCropperHandlePosition::E,
        ImageCropperHandlePosition::S,
        ImageCropperHandlePosition::W,
        ImageCropperHandlePosition::Ne,
        ImageCropperHandlePosition::Nw,
        ImageCropperHandlePosition::Se,
        ImageCropperHandlePosition::Sw,
    ];

    /// The original's name for it, like `"nw"`, its `data-position`.
    pub fn as_str(self) -> &'static str {
        self.handle().as_str()
    }

    /// The resize handle it drags.
    pub fn handle(self) -> ResizeHandle {
        match self {
            ImageCropperHandlePosition::N => ResizeHandle::N,
            ImageCropperHandlePosition::E => ResizeHandle::E,
            ImageCropperHandlePosition::S => ResizeHandle::S,
            ImageCropperHandlePosition::W => ResizeHandle::W,
            ImageCropperHandlePosition::Ne => ResizeHandle::Ne,
            ImageCropperHandlePosition::Nw => ResizeHandle::Nw,
            ImageCropperHandlePosition::Se => ResizeHandle::Se,
            ImageCropperHandlePosition::Sw => ResizeHandle::Sw,
        }
    }
}

/// The options `ImageCropper` takes.
#[derive(Clone, Default)]
pub struct ImageCropperOptions {
    /// The image: a file, a URL, an asset, encoded bytes or a decoded image.
    /// A new one starts loading, and a crop of its own. A custom loader is
    /// told apart by its closure, so keep the same `ImageSource` across
    /// renders rather than making a new one in each.
    pub src: Option<ImageSource>,
    /// The crop, in the image's own pixels, from the top left of the image
    /// as it shows once turned and flipped, with its rotation and flips.
    /// Pass it straight to `crop_image`. Pass it to control the cropper.
    pub crop: Option<ImageCrop>,
    /// The crop at first, when uncontrolled. Without it, once the image
    /// loads, a crop at `aspect_ratio` in the middle: four fifths of the
    /// largest in a box, the largest in a frame.
    pub default_crop: Option<ImageCrop>,
    /// Runs while the crop changes: on every step of a drag, a key or the
    /// wheel, and once an image loads. It runs once the cropper's update is
    /// over, so it may update any entity, the cropper too.
    pub on_crop_change: Option<ImageCropperCropCallback>,
    /// Runs as the crop is asked to change, before it does, inside the
    /// cropper's update: `details.cancel()` stops it. It may update any entity
    /// but the cropper.
    pub on_before_crop_change: Option<ImageCropperCropCallback>,
    /// Runs once an interaction that changed the crop ends: the pointer
    /// lifts, a key is pressed, the wheel pauses. For saving. It runs once the
    /// cropper's update is over, so it may update any entity, the cropper
    /// too.
    pub on_crop_commit: Option<ImageCropperCropCallback>,
    /// `Box` moves and resizes a selection over the whole image. `Frame`
    /// keeps the selection still and as large as it fits, and pans and zooms
    /// the image under it. `Box` by default.
    pub mode: ImageCropperMode,
    /// Keeps the crop's width over its height, like `1.0` for a square or
    /// `16.0 / 9.0`. Changing it fits the crop to the new ratio.
    pub aspect_ratio: Option<f64>,
    /// The selection's shape. A circle is still a box to the crop, so pass
    /// `CropShape::Circle` to `crop_image` too. A rectangle by default.
    pub shape: ImageCropperShape,
    /// The narrowest the crop gets, in image pixels. 1 by default.
    pub min_width: Option<f64>,
    /// The shortest the crop gets, in image pixels. 1 by default.
    pub min_height: Option<f64>,
    /// How far the crop magnifies the image, where 1 is the largest crop the
    /// image allows. 10 by default.
    pub max_zoom: Option<f64>,
    /// Stops every change: the selection can't be focused, and the buttons
    /// and zoom are disabled.
    pub disabled: bool,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl ImageCropperOptions {
    pub fn src(mut self, src: impl Into<ImageSource>) -> Self {
        self.src = Some(src.into());
        self
    }

    pub fn crop(mut self, crop: ImageCrop) -> Self {
        self.crop = Some(crop);
        self
    }

    pub fn default_crop(mut self, crop: ImageCrop) -> Self {
        self.default_crop = Some(crop);
        self
    }

    pub fn on_crop_change(
        mut self,
        callback: impl Fn(&ImageCrop, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_crop_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_crop_change(
        mut self,
        callback: impl Fn(&ImageCrop, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_crop_change = Some(Rc::new(callback));
        self
    }

    pub fn on_crop_commit(
        mut self,
        callback: impl Fn(&ImageCrop, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_crop_commit = Some(Rc::new(callback));
        self
    }

    pub fn mode(mut self, mode: ImageCropperMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn aspect_ratio(mut self, ratio: impl Into<Option<f64>>) -> Self {
        self.aspect_ratio = ratio.into();
        self
    }

    pub fn shape(mut self, shape: ImageCropperShape) -> Self {
        self.shape = shape;
        self
    }

    pub fn min_width(mut self, width: f64) -> Self {
        self.min_width = Some(width);
        self
    }

    pub fn min_height(mut self, height: f64) -> Self {
        self.min_height = Some(height);
        self
    }

    pub fn max_zoom(mut self, zoom: f64) -> Self {
        self.max_zoom = Some(zoom);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `handle` part takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageCropperHandleProps {
    /// The edge or corner it resizes, like `Nw` for the top left corner.
    pub position: ImageCropperHandlePosition,
}

impl ImageCropperHandleProps {
    pub fn new(position: ImageCropperHandlePosition) -> Self {
        Self { position }
    }
}

/// The props the `flip` part takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageCropperFlipProps {
    /// Which way it mirrors the image. Horizontal by default.
    pub direction: ImageCropperFlipDirection,
}

impl ImageCropperFlipProps {
    pub fn direction(mut self, direction: ImageCropperFlipDirection) -> Self {
        self.direction = direction;
        self
    }
}

/// What every part's `with` and the app read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageCropperState {
    /// The crop, or `None` until the image loads.
    pub crop: Option<ImageCrop>,
    /// How far the crop magnifies: 1 when it's as large as the image allows
    /// at its ratio.
    pub zoom: f64,
    /// Whether the image is still loading, so nothing can change yet.
    pub loading: bool,
    /// Whether the image failed to load.
    pub error: bool,
    /// Whether a pointer is dragging the selection, a handle or the image.
    pub dragging: bool,
    /// How the crop changes.
    pub mode: ImageCropperMode,
    /// The selection's shape.
    pub shape: ImageCropperShape,
    /// Whether the `disabled` option stops every change.
    pub disabled: bool,
}

/// What the `viewport` part's `with` reads: the cropper's state, with the
/// image's ratio as it shows, for a viewport sized to fit it
/// (`--image-cropper-aspect-ratio`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageCropperViewportState {
    pub crop: Option<ImageCrop>,
    pub zoom: f64,
    pub loading: bool,
    pub error: bool,
    pub dragging: bool,
    pub mode: ImageCropperMode,
    pub shape: ImageCropperShape,
    pub disabled: bool,
    /// The image's width over its height as it shows, turned, once it has
    /// loaded and has a crop. Give it to the viewport's `aspect_ratio`.
    pub aspect_ratio: Option<f32>,
}

/// What a `handle` part's `with` reads: the cropper's state, with the edge
/// or corner it resizes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageCropperHandleState {
    pub crop: Option<ImageCrop>,
    pub zoom: f64,
    pub loading: bool,
    pub error: bool,
    /// Whether a pointer is dragging anything.
    pub dragging: bool,
    pub mode: ImageCropperMode,
    pub shape: ImageCropperShape,
    pub disabled: bool,
    /// The edge or corner it resizes (`data-position`).
    pub position: ImageCropperHandlePosition,
    /// Whether a pointer is dragging this handle (`data-dragging` on a
    /// handle).
    pub dragged: bool,
}

/// What the `zoom` part's `with` reads: the cropper's state, with where the
/// zoom sits along the range, for drawing its thumb, which the browser
/// draws for a native range input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageCropperZoomState {
    pub crop: Option<ImageCrop>,
    pub zoom: f64,
    pub loading: bool,
    pub error: bool,
    pub dragging: bool,
    pub mode: ImageCropperMode,
    pub shape: ImageCropperShape,
    pub disabled: bool,
    /// The range's end, `max_zoom`.
    pub max_zoom: f64,
    /// Where the zoom sits, from 0 at 1 to 1 at `max_zoom`.
    pub fraction: f64,
}

/// What a `flip` part's `with` reads: the cropper's state, with the way it
/// mirrors the image (`data-direction`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageCropperFlipState {
    pub crop: Option<ImageCrop>,
    pub zoom: f64,
    pub loading: bool,
    pub error: bool,
    pub dragging: bool,
    pub mode: ImageCropperMode,
    pub shape: ImageCropperShape,
    pub disabled: bool,
    pub direction: ImageCropperFlipDirection,
}

/// An image cropper (`imageCropper`). Its entity is its controller.
pub struct ImageCropper {
    pub(crate) instance: Instance,
    pub(crate) crop: Controllable<Option<ImageCrop>>,
    // The image the crop was made for, so a new one starts its own.
    pub(crate) crop_for: Option<SourceKey>,
    pub(crate) natural: Option<Size>,
    pub(crate) status: Status,
    // The viewport's content box, from its padding box, once it lays out.
    pub(crate) content: Option<Rect>,
    // What a pointer is changing: a handle, the whole crop, or the image
    // under a frame.
    pub(crate) dragging: Option<Dragging>,
    pub(crate) settings: Settings,
    pub(crate) drags: DragSession<Self>,
    // The crop an interaction started from, so it commits only what it
    // changed: `None` with no interaction, `Some(None)` from no crop yet.
    pub(crate) start: Option<Option<ImageCrop>>,
    // The last crop a change asked for. A controlled crop only arrives with
    // the app's next render, after a key or a press has already committed.
    pub(crate) changed: Option<ImageCrop>,
    pub(crate) wheel_timer: Option<Task<()>>,
    // The frame's pan, while a pointer drags the image under it.
    pub(crate) gesture: Option<Gesture>,
    // The aspect ratio the options had last, so a new one fits the crop.
    pub(crate) ratio: Option<f64>,
    // A press or a key a handle heard first, which the selection around it
    // leaves alone.
    pub(crate) handle_press: Option<PointerEvent>,
    pub(crate) handle_key: bool,
    pub(crate) selection_focus: FocusHandle,
    pub(crate) handles: Parts<ImageCropperHandlePosition, ()>,
    pub(crate) zoom_focus: FocusHandle,
    pub(crate) rotate_focus: FocusHandle,
    pub(crate) flips: Parts<ImageCropperFlipDirection, ()>,
    // Where the zoom laid out, which a press along it reads.
    pub(crate) zoom_bounds: Measured,
    pub(crate) view: Rc<ImageView>,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl ImageCropper {
    pub fn new(options: ImageCropperOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A change's source follows how the window was used.
        ui_sdk_kernel::track_input(window, cx);
        let settings = settings(&options);
        let given = options.crop.or(options.default_crop);
        let mut crop = Controllable::new("crop", given);
        crop.set_controlled(options.crop.map(Some));
        crop.set_on_change(change_callback(options.on_crop_change.clone()));
        crop.set_on_before_change(change_callback(options.on_before_crop_change.clone()));
        crop.set_on_commit(commit_callback(options.on_crop_commit.clone()));
        let mut instance = Instance::new("image-cropper").with_agent(cx);
        ImageCrop::register_data();
        instance.debug = options.debug;
        let crop_for = given.and(settings.src.as_ref().map(SourceKey::of));
        let view = Rc::new(ImageView::default());
        let releases = view.clone();
        let release = cx.on_release_in(window, move |_cropper, window, cx| releases.release(window, cx));
        Self {
            instance,
            crop,
            crop_for,
            natural: None,
            status: Status::Loading,
            content: None,
            dragging: None,
            ratio: settings.aspect_ratio,
            settings,
            drags: create_drag_session(cx),
            start: None,
            changed: None,
            wheel_timer: None,
            gesture: None,
            handle_press: None,
            handle_key: false,
            selection_focus: cx.focus_handle(),
            handles: Parts::new(),
            zoom_focus: cx.focus_handle(),
            rotate_focus: cx.focus_handle(),
            flips: Parts::new(),
            zoom_bounds: Measured::default(),
            view,
            _subscriptions: vec![release],
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A new image starts loading, and a new aspect ratio fits the
    /// crop to it once the options are in.
    pub fn set_options(&mut self, options: ImageCropperOptions, window: &mut Window, cx: &mut Context<Self>) {
        let settings = settings(&options);
        let mut changed = settings != self.settings;
        // A new image loads before it has a size or a crop of its own.
        let new_source =
            settings.src.as_ref().map(SourceKey::of) != self.settings.src.as_ref().map(SourceKey::of);
        if new_source {
            self.status = Status::Loading;
            self.natural = None;
            // The image part reports the new one as it draws, even one it
            // has shown before.
            self.view.forget();
        }
        self.settings = settings;
        changed |= self.crop.set_controlled(options.crop.map(Some));
        self.crop.set_on_change(change_callback(options.on_crop_change));
        self.crop.set_on_before_change(change_callback(options.on_before_crop_change));
        self.crop.set_on_commit(commit_callback(options.on_crop_commit));
        self.instance.debug = options.debug;
        // The aspect ratio changing fits the crop to it, once the options
        // are in.
        let ratio = self.settings.aspect_ratio;
        if ratio != self.ratio {
            self.ratio = ratio;
            if ratio.is_some() {
                cx.defer_in(window, |cropper, window, cx| {
                    let limits = cropper.limits();
                    cropper.step(
                        move |crop, natural| crate::image_cropper_geometry::fit_crop(crop, natural, &limits),
                        ChangeOptions::reason(Reason::AspectRatioChange),
                        window,
                        cx,
                    );
                });
            }
        }
        // Ending a drag commits, which the app's callback may answer by
        // updating whatever renders the cropper, so it waits for the options
        // to be in.
        if self.is_locked() && self.drags.is_running() {
            cx.defer_in(window, |cropper, window, cx| cropper.end_if_locked(window, cx));
        }
        if changed || new_source {
            cx.notify();
        }
    }

    /// Passes the controlled crop, or `None` to let the cropper keep its own.
    pub fn set_controlled_crop(&mut self, crop: Option<ImageCrop>, cx: &mut Context<Self>) {
        if self.crop.set_controlled(crop.map(Some)) {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// What every part's state holds.
    pub fn state(&self) -> ImageCropperState {
        ImageCropperState {
            crop: *self.crop.get(),
            zoom: self.zoom_text(),
            loading: self.status == Status::Loading,
            error: self.status == Status::Error,
            dragging: self.dragging.is_some(),
            mode: self.settings.mode,
            shape: self.settings.shape,
            disabled: self.settings.disabled,
        }
    }
}

impl Focusable for ImageCropper {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.selection_focus.clone()
    }
}

// `on_crop_change` or `on_before_crop_change` for the crop the cropper holds, which has one once the
// image loads.
fn change_callback(callback: Option<ImageCropperCropCallback>) -> Option<ChangeCallback<Option<ImageCrop>>> {
    let callback = callback?;
    Some(Rc::new(
        move |crop: &Option<ImageCrop>, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
            if let Some(crop) = crop {
                callback(crop, details, window, cx);
            }
        },
    ))
}

fn commit_callback(callback: Option<ImageCropperCropCallback>) -> Option<CommitCallback<Option<ImageCrop>>> {
    let callback = callback?;
    Some(Rc::new(
        move |crop: &Option<ImageCrop>, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
            if let Some(crop) = crop {
                callback(crop, details, window, cx);
            }
        },
    ))
}

/// A crop as an agent reads it.
pub(crate) fn crop_json(crop: &ImageCrop) -> AgentValue {
    json!({
        "x": crop.x,
        "y": crop.y,
        "width": crop.width,
        "height": crop.height,
        "rotation": crop.rotation.degrees(),
        "flipX": crop.flip_x,
        "flipY": crop.flip_y,
    })
}

// A crop an agent gives, as `{ x, y, width, height, rotation, flipX, flipY }`.
fn crop_of(args: &AgentArgs) -> Result<ImageCrop, AgentError> {
    let refuse = || AgentError::refused("setCrop takes a crop: { x, y, width, height }.");
    let AgentValue::Object(crop) = args.value(0) else {
        return Err(refuse());
    };
    let number = |key: &str| crop.get(key).and_then(AgentValue::as_f64);
    let (Some(x), Some(y), Some(width), Some(height)) =
        (number("x"), number("y"), number("width"), number("height"))
    else {
        return Err(refuse());
    };
    let rotation = match crop.get("rotation").and_then(AgentValue::as_i64) {
        Some(degrees) => ImageRotation::from_degrees(degrees).ok_or_else(refuse)?,
        None => ImageRotation::Deg0,
    };
    let flag = |key: &str| crop.get(key).and_then(AgentValue::as_bool).unwrap_or(false);
    Ok(ImageCrop {
        x,
        y,
        width,
        height,
        rotation,
        flip_x: flag("flipX"),
        flip_y: flag("flipY"),
    })
}

/// `agent: { actions: ["setCrop", "rotate", "flip", "setZoom"] }`,
/// `controlled: ["crop"]` and `commits: ["crop"]`.
impl Agent for ImageCropper {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setCrop", |cropper, args, window, cx| {
            let crop = crop_of(args)?;
            Ok(cropper
                .set_crop(crop, ChangeOptions::default(), window, cx)
                .into())
        }),
        AgentAction::new("rotate", |cropper, args, window, cx| {
            let direction = if args.is_missing(0) { 1.0 } else { args.number(0)? };
            let direction = if direction < 0.0 { -1 } else { 1 };
            Ok(cropper
                .rotate(direction, ChangeOptions::default(), window, cx)
                .into())
        }),
        AgentAction::new("flip", |cropper, args, window, cx| {
            let direction = match args.value(0).as_str() {
                Some("vertical") => ImageCropperFlipDirection::Vertical,
                _ => ImageCropperFlipDirection::Horizontal,
            };
            Ok(cropper
                .flip(direction, ChangeOptions::default(), window, cx)
                .into())
        }),
        AgentAction::new("setZoom", |cropper, args, window, cx| {
            let zoom = args.number(0)?;
            Ok(cropper
                .set_zoom(zoom, ChangeOptions::default(), window, cx)
                .into())
        }),
    ];

    const CONTROLLED: Option<&'static [&'static str]> = Some(&["crop"]);

    const COMMITS: &'static [&'static str] = &["crop"];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
            .with(
                "crop",
                self.crop.get().as_ref().map_or(AgentValue::Null, crop_json),
            )
            .with("zoom", self.zoom_text())
            .with("disabled", self.settings.disabled)
            .with("error", self.status == Status::Error)
    }
}
