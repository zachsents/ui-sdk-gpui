//! Port of `components/image-cropper/image-cropper-parts.ts`: an image
//! cropper's parts and their attributes. State and what the parts do live in
//! `image_cropper_api.rs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui::accesskit::ActionData;
use gpui::{
    AccessibleAction, App, Bounds, Corners, Entity, ImageId, InteractiveElement, ParentElement, Pixels,
    RenderImage, StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, prelude::FluentBuilder,
    px,
};
use image::{ImageBuffer, Rgba, imageops};
use ui_sdk_direct_manipulation::{ResizeHandle, resize_cursors};
use ui_sdk_drag_and_drop::{BindDragOptions, PointerEvent, on_pointer_down};
use ui_sdk_image::{ImageRotation, ImageSource, use_image, use_natural_size};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, ChangeOptions, InputEvent, Key, Need, Part, Reason};
use ui_sdk_platform::is_rtl;

use crate::image_cropper::{
    ImageCropper, ImageCropperFlipProps, ImageCropperFlipState, ImageCropperHandlePosition,
    ImageCropperHandleProps, ImageCropperHandleState, ImageCropperMode, ImageCropperState,
    ImageCropperViewportState, ImageCropperZoomState,
};
use crate::image_cropper_api::{Dragging, SourceKey, Status};
use crate::image_cropper_geometry::{Rect, Size, shown_size, smallest};

/// The keys the selection and the handles handle.
pub const CROP_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "shift+ArrowUp",
    "shift+ArrowDown",
    "shift+ArrowLeft",
    "shift+ArrowRight",
    "+",
    "-",
    "=",
    "r",
    "shift+R",
];

/// The keys the zoom handles, as a native range input does.
pub const ZOOM_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "PageUp",
    "PageDown",
    "Home",
    "End",
];

// English by default. Pass `aria_label` in another language.
fn handle_name(position: ImageCropperHandlePosition) -> &'static str {
    match position {
        ImageCropperHandlePosition::N => "Top edge",
        ImageCropperHandlePosition::E => "Right edge",
        ImageCropperHandlePosition::S => "Bottom edge",
        ImageCropperHandlePosition::W => "Left edge",
        ImageCropperHandlePosition::Ne => "Top right corner",
        ImageCropperHandlePosition::Nw => "Top left corner",
        ImageCropperHandlePosition::Se => "Bottom right corner",
        ImageCropperHandlePosition::Sw => "Bottom left corner",
    }
}

// Where on the selection's box each handle sits, as fractions of its size.
fn handle_spot(position: ImageCropperHandlePosition) -> (f64, f64) {
    match position {
        ImageCropperHandlePosition::N => (0.5, 0.0),
        ImageCropperHandlePosition::E => (1.0, 0.5),
        ImageCropperHandlePosition::S => (0.5, 1.0),
        ImageCropperHandlePosition::W => (0.0, 0.5),
        ImageCropperHandlePosition::Ne => (1.0, 0.0),
        ImageCropperHandlePosition::Nw => (0.0, 0.0),
        ImageCropperHandlePosition::Se => (1.0, 1.0),
        ImageCropperHandlePosition::Sw => (0.0, 1.0),
    }
}

// A length in pixels, on hundredths, as the original writes it.
fn place(value: f64) -> Pixels {
    px((value * 100.0).round() as f32 / 100.0)
}

fn length(pixels: Pixels) -> f64 {
    f64::from(f32::from(pixels))
}

/// The image as the cropper paints it: the decoded image its source loads
/// into, and a copy turned and flipped as the crop is, since gpui paints an
/// image only as it is. Copies no longer shown leave the sprite atlas once
/// the frame is drawn, and the rest when the cropper goes.
#[derive(Default)]
pub(crate) struct ImageView {
    // The source and status the cropper last heard of.
    seen: RefCell<Option<(SourceKey, Status)>>,
    turned: RefCell<Option<Turned>>,
    // A large image being turned in the background, and how.
    turning: RefCell<Option<(ImageId, Orientation)>>,
    stale: RefCell<Vec<Arc<RenderImage>>>,
    // The viewport's padding box as it last laid out, which presses read,
    // and the content box last reported.
    padding: Cell<Option<Bounds<Pixels>>>,
    content: Cell<Option<Rect>>,
}

// How the crop turns and flips the image.
type Orientation = (ImageRotation, bool, bool);

// The most pixels an image turns in place, in the frame that needs it. A
// larger one, like a camera's photo, turns in the background, so a press
// doesn't hold the frame up, and shows once it has.
const TURN_IN_PLACE: u64 = 1_000_000;

// A turned copy, by the image it's made from and how it's turned.
struct Turned {
    from: ImageId,
    orientation: Orientation,
    image: Arc<RenderImage>,
}

impl ImageView {
    /// The viewport's padding box, in window coordinates, as it last laid
    /// out.
    pub(crate) fn padding_box(&self) -> Option<Bounds<Pixels>> {
        self.padding.get()
    }

    /// Forgets the source it last reported, so a new one reports as it
    /// draws, even one it has shown before.
    pub(crate) fn forget(&self) {
        *self.seen.borrow_mut() = None;
    }

    // Loads the source as it draws, tells the cropper when it loads or
    // fails, and gives the image turned and flipped as `orientation` says.
    fn watch(
        self: &Rc<Self>,
        cropper: &WeakEntity<ImageCropper>,
        source: Option<&ImageSource>,
        orientation: Orientation,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<RenderImage>> {
        // An image with no source never loads.
        let source = source?;
        let key = SourceKey::of(source);
        let image = use_image(source, window, cx);
        // An SVG's natural size is its own, which gpui draws at twice that.
        let (status, natural) = match use_natural_size(source, window, cx) {
            None => (Status::Loading, None),
            Some(Err(_)) => (Status::Error, None),
            Some(Ok(natural)) => (Status::Loaded, Some(Size::new(natural.width, natural.height))),
        };
        let seen = Some((key.clone(), status));
        if *self.seen.borrow() != seen {
            *self.seen.borrow_mut() = seen;
            let cropper = cropper.clone();
            // The parts show it next frame, and its change reports it now,
            // once the draw is over.
            window.defer(cx, move |window, cx| {
                cropper
                    .update(cx, |cropper, cx| {
                        cropper.image_status(key, status, natural, window, cx)
                    })
                    .ok();
            });
        }
        let image = image?.ok()?;
        self.turned(image, orientation, cropper, cx)
    }

    // The image turned clockwise by `rotation`, then mirrored, as the
    // original's transform shows it: `None` while a large one turns.
    fn turned(
        self: &Rc<Self>,
        image: Arc<RenderImage>,
        orientation: Orientation,
        cropper: &WeakEntity<ImageCropper>,
        cx: &mut App,
    ) -> Option<Arc<RenderImage>> {
        if orientation == (ImageRotation::Deg0, false, false) {
            self.turning.borrow_mut().take();
            if let Some(old) = self.turned.borrow_mut().take() {
                self.stale.borrow_mut().push(old.image);
            }
            return Some(image);
        }
        if let Some(copy) = self.turned.borrow().as_ref()
            && copy.from == image.id
            && copy.orientation == orientation
        {
            return Some(copy.image.clone());
        }
        let size = image.size(0);
        let (width, height) = (
            u32::try_from(size.width.0).ok()?,
            u32::try_from(size.height.0).ok()?,
        );
        if u64::from(width) * u64::from(height) <= TURN_IN_PLACE {
            let buffer = turn(image.as_bytes(0)?.to_vec(), width, height, orientation)?;
            let copy = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
            self.keep(image.id, orientation, copy.clone());
            return Some(copy);
        }
        let from = image.id;
        if *self.turning.borrow() == Some((from, orientation)) {
            return None;
        }
        *self.turning.borrow_mut() = Some((from, orientation));
        let bytes = image.as_bytes(0)?.to_vec();
        let work = cx
            .background_executor()
            .spawn(async move { turn(bytes, width, height, orientation) });
        let view = Rc::downgrade(self);
        let cropper = cropper.clone();
        cx.spawn(async move |cx| {
            let buffer = work.await;
            let Some(view) = view.upgrade() else {
                return;
            };
            // The crop may have turned again since, which a newer turn shows.
            if *view.turning.borrow() != Some((from, orientation)) {
                return;
            }
            view.turning.borrow_mut().take();
            if let Some(buffer) = buffer {
                view.keep(
                    from,
                    orientation,
                    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
                );
            }
            cropper.update(cx, |_cropper, cx| cx.notify()).ok();
        })
        .detach();
        None
    }

    // Keeps a turned copy, and lets the one before it go.
    fn keep(&self, from: ImageId, orientation: Orientation, image: Arc<RenderImage>) {
        let old = self.turned.borrow_mut().replace(Turned {
            from,
            orientation,
            image,
        });
        if let Some(old) = old {
            self.stale.borrow_mut().push(old.image);
        }
    }

    // Takes copies no longer shown out of the sprite atlas, once the frame
    // that stopped showing them is drawn.
    fn drop_stale(&self, window: &mut Window, cx: &mut App) {
        let stale: Vec<Arc<RenderImage>> = self.stale.borrow_mut().drain(..).collect();
        if stale.is_empty() {
            return;
        }
        window.defer(cx, move |window, cx| {
            for image in stale {
                cx.drop_image(image, Some(window));
            }
        });
    }

    /// Takes every copy out of the sprite atlas, as the cropper goes.
    pub(crate) fn release(&self, window: &mut Window, cx: &mut App) {
        let turned = self.turned.borrow_mut().take().map(|turned| turned.image);
        let stale: Vec<Arc<RenderImage>> = self.stale.borrow_mut().drain(..).chain(turned).collect();
        for image in stale {
            cx.drop_image(image, Some(window));
        }
    }
}

// The decoded image's first frame, `width` by `height`, turned and flipped.
// Its pixels are BGRA, which turning leaves as they are.
fn turn(
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    orientation: Orientation,
) -> Option<ImageBuffer<Rgba<u8>, Vec<u8>>> {
    let (rotation, flip_x, flip_y) = orientation;
    let buffer = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, bytes)?;
    let mut buffer = match rotation {
        ImageRotation::Deg0 => buffer,
        ImageRotation::Deg90 => imageops::rotate90(&buffer),
        ImageRotation::Deg180 => imageops::rotate180(&buffer),
        ImageRotation::Deg270 => imageops::rotate270(&buffer),
    };
    if flip_x {
        imageops::flip_horizontal_in_place(&mut buffer);
    }
    if flip_y {
        imageops::flip_vertical_in_place(&mut buffer);
    }
    Some(buffer)
}

impl ImageCropper {
    // The state of every part.
    fn part_state(&self) -> ImageCropperState {
        self.state()
    }

    /// The `root` part, a group around the parts, busy while the image
    /// loads. It follows a drag's pointer, so render it on every frame. Name
    /// it with `aria_label`.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<ImageCropperState> {
        let cropper = this.read(cx);
        let state = cropper.part_state();
        Part::new(&cropper.instance, "root", None, state)
            .aria_role(Role::Group)
            .aria(Aria::new().busy(state.loading))
            .child(cropper.drags.listener())
    }

    /// The `viewport` part: the area the image shows in, which a frame's
    /// pointer pans and the wheel or a pinch zooms. Give it a size; its
    /// padding keeps the image and the selection off its edges. It's
    /// `relative` and clips what's outside it, and lays out a layer that
    /// measures its content box, so the parts inside it are absolute.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> Part<ImageCropperViewportState> {
        let cropper = this.read(cx);
        let state = cropper.part_state();
        // The image's ratio as it shows, for a viewport sized to fit it.
        let aspect_ratio = match (state.crop, cropper.natural) {
            (Some(crop), Some(natural)) => {
                let shown = shown_size(natural, crop.rotation);
                Some((shown.width / shown.height) as f32)
            }
            _ => None,
        };
        let viewport = ImageCropperViewportState {
            crop: state.crop,
            zoom: state.zoom,
            loading: state.loading,
            error: state.error,
            dragging: state.dragging,
            mode: state.mode,
            shape: state.shape,
            disabled: state.disabled,
            aspect_ratio,
        };
        let frame = state.mode == ImageCropperMode::Frame;
        let inert = cropper.is_inert();
        let presses = this.downgrade();
        let wheels = this.downgrade();
        let pinches = this.downgrade();
        let part = Part::new(&cropper.instance, "viewport", None, viewport)
            .relative()
            .overflow_hidden()
            .flex()
            .flex_col()
            .when(frame && !inert, |part| part.cursor_move())
            .on_scroll(move |event, window, cx| {
                wheels
                    .update(cx, |cropper, cx| cropper.wheel(event, window, cx))
                    .ok();
            })
            .on_pinch(move |event, window, cx| {
                pinches
                    .update(cx, |cropper, cx| cropper.pinch(event, window, cx))
                    .ok();
            });
        let part = measure_viewport(part, cropper.view.clone(), this.downgrade());
        on_pointer_down(part, move |event, window, cx| {
            presses
                .update(cx, |cropper, cx| cropper.viewport_pointer_down(event, window, cx))
                .ok();
        })
    }

    /// The `image` part: the image, turned and flipped, where it shows in
    /// the viewport. Name it with `aria_label`, the original's `alt`.
    pub fn image(this: &Entity<Self>, cx: &App) -> Part<ImageCropperState> {
        let cropper = this.read(cx);
        let state = cropper.part_state();
        let layout = cropper.layout();
        let orientation = state.crop.map_or((ImageRotation::Deg0, false, false), |crop| {
            (crop.rotation, crop.flip_x, crop.flip_y)
        });
        let view = cropper.view.clone();
        let source = cropper.settings.src.clone();
        let entity = this.downgrade();
        let shown = layout.is_some();
        let part = Part::new(&cropper.instance, "image", None, state)
            .aria_role(Role::Img)
            .absolute();
        let part = match layout {
            Some(at) => part
                .left(place(at.image.left))
                .top(place(at.image.top))
                .w(place(at.image.width))
                .h(place(at.image.height)),
            None => part.top_0().left_0().size_0(),
        };
        part.child(
            canvas(
                move |_bounds, window, cx| {
                    let image = view.watch(&entity, source.as_ref(), orientation, window, cx);
                    view.drop_stale(window, cx);
                    image.filter(|_| shown)
                },
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

    /// The `selection` part: the crop, a slider you drag or move with the
    /// arrow keys. Plus and minus zoom, and R turns the image. It's hidden
    /// until the image loads. It's named "Crop" in English; pass
    /// `aria_label` in another language.
    pub fn selection(this: &Entity<Self>, cx: &App) -> Part<ImageCropperState> {
        let cropper = this.read(cx);
        let state = cropper.part_state();
        let inert = cropper.is_inert();
        let ready = cropper.is_ready();
        let boxed = state.mode == ImageCropperMode::Box;
        let presses = this.downgrade();
        let keys = this.downgrade();
        let part = Part::new(&cropper.instance, "selection", None, state)
            .aria_role(Role::Slider)
            .aria_label("Crop")
            .absolute()
            .when(boxed && !inert, |part| part.cursor_move());
        let part = if inert {
            part.unneeded(
                Need::Focus,
                "It takes no focus until the image loads, or while it's disabled.",
            )
        } else {
            part.focus_mode(FocusMode::TabStop, Some(&cropper.selection_focus), false)
        };
        let mut aria = Aria::new()
            .role_description("crop area")
            .disabled(state.disabled)
            .hidden(!ready);
        let part = match (state.crop, cropper.natural) {
            (Some(crop), Some(natural)) => {
                let shown = shown_size(natural, crop.rotation);
                aria = aria.value_text(format!(
                    "{} by {} pixels, at {}, {}",
                    crop.width, crop.height, crop.x, crop.y
                ));
                part.aria_min_numeric_value(0.0)
                    .aria_numeric_value(crop.x)
                    .aria_max_numeric_value(shown.width - crop.width)
            }
            // Hidden until the image loads, with a value all the same.
            _ => part.aria_min_numeric_value(0.0).aria_numeric_value(0.0),
        };
        let part = match cropper.layout() {
            Some(at) => part
                .left(place(at.selection.left))
                .top(place(at.selection.top))
                .w(place(at.selection.width))
                .h(place(at.selection.height)),
            None => part,
        };
        let part = part.when(!ready, |part| part.invisible()).aria(aria).on_keys(
            CROP_KEYS,
            move |event, window, cx| {
                keys.update(cx, |cropper, cx| {
                    cropper.keydown(event, ResizeHandle::Move, window, cx)
                })
                .ok();
            },
        );
        on_pointer_down(part, move |event, window, cx| {
            presses
                .update(cx, |cropper, cx| {
                    cropper.selection_pointer_down(event, ResizeHandle::Move, window, cx)
                })
                .ok();
        })
    }

    /// The `grid` part: lines across the selection, in thirds, for the app
    /// to draw. Its state's `dragging` shows them only during a drag. Put it
    /// inside the selection; it fills it and takes no presses.
    pub fn grid(this: &Entity<Self>, cx: &App) -> Part<ImageCropperState> {
        let cropper = this.read(cx);
        Part::new(&cropper.instance, "grid", None, cropper.part_state())
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
    }

    /// A `handle` part: an edge or corner of a box's selection, which goes
    /// beside the selection in the viewport, since a slider's children are
    /// hidden from assistive technology. Arrows move its edges. It sits on
    /// its point of the selection's edge, so give it a size and center it on
    /// that point. Hidden unless the image has loaded in a box.
    pub fn handle(
        this: &Entity<Self>,
        props: ImageCropperHandleProps,
        cx: &App,
    ) -> Part<ImageCropperHandleState> {
        let cropper = this.read(cx);
        let state = cropper.part_state();
        let position = props.position;
        let handle = position.handle();
        let inert = cropper.is_inert();
        let shows = cropper.is_ready() && state.mode == ImageCropperMode::Box;
        let focus = cropper.handles.focus_handle(&position, cx);
        let handle_state = ImageCropperHandleState {
            crop: state.crop,
            zoom: state.zoom,
            loading: state.loading,
            error: state.error,
            dragging: state.dragging,
            mode: state.mode,
            shape: state.shape,
            disabled: state.disabled,
            position,
            dragged: cropper.dragging == Some(Dragging::Handle(handle)),
        };
        let presses = this.downgrade();
        let keys = this.downgrade();
        let part = Part::new(&cropper.instance, "handle", Some(position.as_str()), handle_state)
            .aria_role(Role::Slider)
            .aria_label(handle_name(position))
            .absolute()
            .when(!inert, |part| part.cursor(resize_cursors(handle)));
        let part = if inert {
            part.unneeded(
                Need::Focus,
                "It takes no focus until the image loads, or while it's disabled.",
            )
        } else {
            part.focus_mode(FocusMode::TabStop, Some(&focus), false)
        };
        let part = match handle {
            ResizeHandle::N | ResizeHandle::S => part.aria_orientation(gpui::Orientation::Vertical.into()),
            ResizeHandle::E | ResizeHandle::W => part.aria_orientation(gpui::Orientation::Horizontal.into()),
            _ => part,
        };
        let mut aria = Aria::new()
            .role_description("resize handle")
            .disabled(state.disabled)
            .hidden(!shows);
        let part = match (state.crop, cropper.natural) {
            (Some(crop), Some(natural)) => {
                let (across, down) = handle_spot(position);
                let shown = shown_size(natural, crop.rotation);
                let least = smallest(shown, &cropper.limits());
                let vertical = matches!(handle, ResizeHandle::N | ResizeHandle::S);
                aria = aria.value_text(format!("{} by {} pixels", crop.width, crop.height));
                let part = part
                    .aria_numeric_value(if vertical { crop.height } else { crop.width })
                    .aria_min_numeric_value(if vertical { least.height } else { least.width }.ceil())
                    .aria_max_numeric_value(if vertical { shown.height } else { shown.width });
                match cropper.layout() {
                    Some(at) => part
                        .left(place(at.selection.left + across * at.selection.width))
                        .top(place(at.selection.top + down * at.selection.height)),
                    None => part,
                }
            }
            _ => part.aria_numeric_value(0.0),
        };
        let part = part.when(!shows, |part| part.invisible()).aria(aria).on_keys(
            CROP_KEYS,
            move |event, window, cx| {
                keys.update(cx, |cropper, cx| {
                    cropper.mark_handle_key(window, cx);
                    cropper.keydown(event, handle, window, cx);
                })
                .ok();
            },
        );
        on_pointer_down(part, move |event, window, cx| {
            presses
                .update(cx, |cropper, cx| {
                    cropper.mark_handle_press(event, window, cx);
                    cropper.selection_pointer_down(event, handle, window, cx);
                })
                .ok();
        })
    }

    /// The `zoom` part: a range from 1 to `max_zoom`, which the browser
    /// draws for the original's native range input. Draw its thumb from the
    /// state's `fraction`. A press moves it there and follows the pointer;
    /// arrows, Page Up, Page Down, Home and End move it. It's named "Zoom"
    /// in English; pass `aria_label` in another language.
    pub fn zoom(this: &Entity<Self>, cx: &App) -> Part<ImageCropperZoomState> {
        let cropper = this.read(cx);
        let state = cropper.part_state();
        let inert = cropper.is_inert();
        let max_zoom = cropper.settings.max_zoom;
        let fraction = if max_zoom > 1.0 {
            ((state.zoom - 1.0) / (max_zoom - 1.0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let zoom = ImageCropperZoomState {
            crop: state.crop,
            zoom: state.zoom,
            loading: state.loading,
            error: state.error,
            dragging: state.dragging,
            mode: state.mode,
            shape: state.shape,
            disabled: state.disabled,
            max_zoom,
            fraction,
        };
        let presses = this.downgrade();
        let keys = this.downgrade();
        let increments = this.downgrade();
        let decrements = this.downgrade();
        let sets = this.downgrade();
        let part = Part::new(&cropper.instance, "zoom", None, zoom)
            .aria_role(Role::Slider)
            .aria_label("Zoom")
            .focus_mode(FocusMode::Native, (!inert).then_some(&cropper.zoom_focus), inert)
            .aria_min_numeric_value(1.0)
            .aria_max_numeric_value(max_zoom)
            .aria_numeric_value(state.zoom)
            .aria(
                Aria::new()
                    .value_text(format!("{}%", (state.zoom * 100.0).round()))
                    .disabled(inert),
            )
            .on_keys(ZOOM_KEYS, move |event, window, cx| {
                keys.update(cx, |cropper, cx| cropper.zoom_key(event, window, cx))
                    .ok();
            })
            .on_a11y_action(AccessibleAction::Increment, move |_, window, cx| {
                increments
                    .update(cx, |cropper, cx| cropper.zoom_adjust(1.0, None, window, cx))
                    .ok();
            })
            .on_a11y_action(AccessibleAction::Decrement, move |_, window, cx| {
                decrements
                    .update(cx, |cropper, cx| cropper.zoom_adjust(-1.0, None, window, cx))
                    .ok();
            })
            .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
                let Some(ActionData::NumericValue(value)) = data else {
                    return;
                };
                let value = *value;
                sets.update(cx, |cropper, cx| {
                    cropper.zoom_adjust(0.0, Some(value), window, cx)
                })
                .ok();
            });
        let part = cropper.zoom_bounds.measure(part);
        on_pointer_down(part, move |event, window, cx| {
            presses
                .update(cx, |cropper, cx| cropper.zoom_press(event, window, cx))
                .ok();
        })
    }

    /// The `rotate` part, a button that turns the image a quarter clockwise.
    pub fn rotate_button(this: &Entity<Self>, cx: &App) -> Part<ImageCropperState> {
        let cropper = this.read(cx);
        let state = cropper.part_state();
        let inert = cropper.is_inert();
        let entity = this.downgrade();
        Part::new(&cropper.instance, "rotate", None, state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&cropper.rotate_focus), inert)
            .aria(Aria::new().disabled(inert))
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |cropper, cx| {
                        if cropper.is_inert() {
                            return;
                        }
                        cropper.rotate(1, ChangeOptions::event_of(Reason::RotatePress, event), window, cx);
                    })
                    .ok();
            })
    }

    /// A `flip` part, a button that mirrors the image, left to right or top
    /// to bottom.
    pub fn flip_button(
        this: &Entity<Self>,
        props: ImageCropperFlipProps,
        cx: &App,
    ) -> Part<ImageCropperFlipState> {
        let cropper = this.read(cx);
        let state = cropper.part_state();
        let inert = cropper.is_inert();
        let direction = props.direction;
        let focus = cropper.flips.focus_handle(&direction, cx);
        let flip = ImageCropperFlipState {
            crop: state.crop,
            zoom: state.zoom,
            loading: state.loading,
            error: state.error,
            dragging: state.dragging,
            mode: state.mode,
            shape: state.shape,
            disabled: state.disabled,
            direction,
        };
        let entity = this.downgrade();
        Part::new(&cropper.instance, "flip", Some(direction.as_str()), flip)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&focus), inert)
            .aria(Aria::new().disabled(inert))
            .on_press(move |event, window, cx| {
                entity
                    .update(cx, |cropper, cx| {
                        if cropper.is_inert() {
                            return;
                        }
                        cropper.flip(
                            direction,
                            ChangeOptions::event_of(Reason::FlipPress, event),
                            window,
                            cx,
                        );
                    })
                    .ok();
            })
    }

    // A press on the zoom: the value under it, then the pointer's, as a
    // native range input follows it, with `change` as it lifts.
    fn zoom_press(&mut self, event: &PointerEvent, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        let drags = self.drags.clone();
        if !drags.accepts(self, event, window, cx) || self.is_inert() {
            return;
        }
        let Some(bounds) = self.zoom_bounds.get() else {
            return;
        };
        if self.zoom_focus.is_rendered(window) {
            window.focus(&self.zoom_focus, cx);
            window.prevent_default();
        }
        let rtl = is_rtl(cx);
        let value = self.zoom_at(event.position, bounds, rtl);
        self.zoom_input(value, Some(event.event.clone()), window, cx);
        drags.start(
            event,
            BindDragOptions::new()
                .on_move(move |cropper: &mut Self, sample, window, cx| {
                    let value = cropper.zoom_at(sample.position(), bounds, rtl);
                    cropper.zoom_input(value, Some(sample.event.event.clone()), window, cx);
                })
                .on_end(|cropper: &mut Self, end, _canceled, window, cx| {
                    cropper.zoom_commit(Some(end.event.clone()), window, cx);
                }),
            window,
            cx,
        );
    }

    // The zoom a point along the range stands for. Right to left, the range
    // runs the other way.
    fn zoom_at(&self, point: gpui::Point<Pixels>, bounds: Bounds<Pixels>, rtl: bool) -> f64 {
        let width = length(bounds.size.width);
        let mut fraction = if width > 0.0 {
            ((length(point.x) - length(bounds.left())) / width).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if rtl {
            fraction = 1.0 - fraction;
        }
        1.0 + fraction * (self.settings.max_zoom - 1.0)
    }

    // A key on the zoom, as a native range input takes it: its step is a
    // hundredth of the range, since it takes any value, and Page Up and Page
    // Down move a tenth. Each key sends `input` and `change`.
    fn zoom_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || self.is_inert() {
            return;
        }
        let (min, max) = (1.0, self.settings.max_zoom);
        let step = (max - min) / 100.0;
        let large = ((max - min) / 10.0).max(step);
        let rtl = is_rtl(cx);
        let current = self.zoom_text();
        let next = match Key::of(&event.keystroke) {
            Key::ArrowUp => current + step,
            Key::ArrowDown => current - step,
            Key::ArrowRight => current + if rtl { -step } else { step },
            Key::ArrowLeft => current - if rtl { -step } else { step },
            Key::PageUp => current + large,
            Key::PageDown => current - large,
            Key::Home => min,
            Key::End => max,
            _ => return,
        };
        window.prevent_default();
        let event = Some(InputEvent::from(event));
        self.zoom_input(next.clamp(min, max), event.clone(), window, cx);
        self.zoom_commit(event, window, cx);
    }

    // A screen reader's adjustment of the zoom: a step either way, or a
    // value it sets, each with `input` and `change`.
    fn zoom_adjust(
        &mut self,
        steps: f64,
        value: Option<f64>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.is_inert() {
            return;
        }
        let (min, max) = (1.0, self.settings.max_zoom);
        let next = value.unwrap_or(self.zoom_text() + steps * (max - min) / 100.0);
        let event = Some(InputEvent::Accessibility);
        self.zoom_input(next.clamp(min, max), event.clone(), window, cx);
        self.zoom_commit(event, window, cx);
    }
}

// The viewport's layers that measure it: one over its padding box, where its
// absolute parts are placed, and one filling its content box, which the
// layout fits the image in (`measure`, with the viewport's padding).
fn measure_viewport<S: 'static>(
    part: Part<S>,
    view: Rc<ImageView>,
    cropper: WeakEntity<ImageCropper>,
) -> Part<S> {
    let padding = view.clone();
    part.child(
        canvas(
            move |bounds, _window, _cx| padding.padding.set(Some(bounds)),
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
    .child(
        canvas(
            move |bounds, window, cx| {
                let Some(padding) = view.padding.get() else {
                    return;
                };
                let content = Rect {
                    left: length(bounds.left() - padding.left()),
                    top: length(bounds.top() - padding.top()),
                    width: length(bounds.size.width),
                    height: length(bounds.size.height),
                };
                if view.content.get() == Some(content) {
                    return;
                }
                view.content.set(Some(content));
                let cropper = cropper.clone();
                window.defer(cx, move |_window, cx| {
                    cropper
                        .update(cx, |cropper, cx| cropper.measured(content, cx))
                        .ok();
                });
            },
            |_, _, _, _| {},
        )
        .flex_grow_1()
        .self_stretch(),
    )
}
