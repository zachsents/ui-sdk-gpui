//! Port of `components/image-cropper/image-cropper-api.ts`: an image
//! cropper's state and what it does. The crop is in the image's own pixels;
//! where things show on screen is worked out from it and the viewport's
//! size. Parts and their attributes are in `image_cropper_parts.rs`.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, ImageId, KeyDownEvent, MouseButton, PinchEvent, Pixels, Resource, ScrollDelta, ScrollEvent,
    ScrollSource, TouchPhase, Window,
};
use ui_sdk_direct_manipulation::ResizeHandle;
use ui_sdk_drag_and_drop::{BindDragOptions, DRAG_THRESHOLD, PointerEvent, PointerKind, PointerSample};
use ui_sdk_image::{ImageCrop, ImageSource};
use ui_sdk_kernel::{ChangeOptions, InputEvent, Key, Reason};
use ui_sdk_number::clamp;

use crate::image_cropper::{
    ImageCropper, ImageCropperFlipDirection, ImageCropperMode, ImageCropperOptions, ImageCropperShape,
};
use crate::image_cropper_geometry::{
    Crop, Layout, Limits, Point, Rect, Size, flip_crop, initial_crop, js_round, layout_of, resize_crop,
    rotate_crop, round, same_crop, zoom_crop, zoom_of,
};

/// Whether the image has loaded (`status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Loading,
    Loaded,
    Error,
}

/// What a pointer is changing: a handle or the whole crop, or the image
/// under a frame (`dragging`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dragging {
    Handle(ResizeHandle),
    Pan,
}

/// What tells one image from another, where the original compares `src`: a
/// new one starts its own crop.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SourceKey {
    Resource(Resource),
    Render(ImageId),
    Image(u64),
    // A custom loader, by its address: a new closure is a new source.
    Custom(usize),
}

impl SourceKey {
    pub(crate) fn of(source: &ImageSource) -> Self {
        match source {
            ImageSource::Resource(resource) => SourceKey::Resource(resource.clone()),
            ImageSource::Render(image) => SourceKey::Render(image.id),
            ImageSource::Image(image) => SourceKey::Image(image.id()),
            ImageSource::Custom(load) => SourceKey::Custom(Arc::as_ptr(load) as *const () as usize),
        }
    }
}

/// What the options set, besides the crop and the callbacks (`settings`).
#[derive(Clone)]
pub(crate) struct Settings {
    pub(crate) src: Option<ImageSource>,
    pub(crate) mode: ImageCropperMode,
    pub(crate) shape: ImageCropperShape,
    pub(crate) aspect_ratio: Option<f64>,
    pub(crate) min_width: f64,
    pub(crate) min_height: f64,
    pub(crate) max_zoom: f64,
    pub(crate) disabled: bool,
}

impl PartialEq for Settings {
    fn eq(&self, other: &Self) -> bool {
        self.src.as_ref().map(SourceKey::of) == other.src.as_ref().map(SourceKey::of)
            && self.mode == other.mode
            && self.shape == other.shape
            && self.aspect_ratio == other.aspect_ratio
            && self.min_width == other.min_width
            && self.min_height == other.min_height
            && self.max_zoom == other.max_zoom
            && self.disabled == other.disabled
    }
}

pub(crate) fn settings(options: &ImageCropperOptions) -> Settings {
    Settings {
        src: options.src.clone(),
        mode: options.mode,
        shape: options.shape,
        aspect_ratio: options.aspect_ratio.filter(|ratio| *ratio > 0.0),
        min_width: options.min_width.unwrap_or(1.0).max(1.0),
        min_height: options.min_height.unwrap_or(1.0).max(1.0),
        max_zoom: options.max_zoom.unwrap_or(10.0).max(1.0),
        disabled: options.disabled,
    }
}

/// A frame's pan, from the press: the crop it started from, where the
/// pointer pressed, and the layout then (`gesture`). gpui follows one
/// pointer, so there's no second finger's distance to pinch with.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Gesture {
    pub(crate) crop: Crop,
    pub(crate) center: Point,
    pub(crate) scale: f64,
}

// A length in pixels as a number.
fn length(pixels: Pixels) -> f64 {
    f64::from(f32::from(pixels))
}

impl ImageCropper {
    /// What keeps the crop's size in bounds (`limitsOf`).
    pub(crate) fn limits(&self) -> Limits {
        Limits {
            aspect: self.settings.aspect_ratio,
            min_width: self.settings.min_width,
            min_height: self.settings.min_height,
            max_zoom: self.settings.max_zoom,
        }
    }

    /// Whether the image has loaded. Parts read this, not the crop
    /// (`isReady`).
    pub(crate) fn is_ready(&self) -> bool {
        self.status == Status::Loaded
    }

    /// Whether nothing can change the crop (`isLocked`).
    pub(crate) fn is_locked(&self) -> bool {
        self.settings.disabled || !self.is_ready() || self.natural.is_none() || self.crop.get().is_none()
    }

    /// What parts read for whether nothing can change (`isInert`).
    pub(crate) fn is_inert(&self) -> bool {
        self.settings.disabled || !self.is_ready()
    }

    /// Where the image and the selection show, or `None` until the image and
    /// the viewport have sizes (`layout`).
    pub fn layout(&self) -> Option<Layout> {
        let crop = (*self.crop.get())?;
        let natural = self.natural?;
        let content = self.content?;
        if content.width == 0.0 || content.height == 0.0 || !self.is_ready() {
            return None;
        }
        Some(layout_of(
            &crop,
            natural,
            content,
            self.settings.mode,
            self.settings.aspect_ratio,
        ))
    }

    /// How far the crop magnifies, or 1 before there is one (`zoomText`).
    pub(crate) fn zoom_text(&self) -> f64 {
        match (*self.crop.get(), self.natural) {
            (Some(crop), Some(natural)) => zoom_of(&crop, natural),
            _ => 1.0,
        }
    }

    /// Asks for a crop, unless it's the crop already (`setCrop`).
    pub(crate) fn change_crop(
        &mut self,
        next: Crop,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(now) = *self.crop.get()
            && same_crop(&now, &next)
        {
            return false;
        }
        let done = self.crop.change(&self.instance, Some(next), options, window, cx);
        if done {
            self.changed = Some(next);
            cx.notify();
        }
        done
    }

    /// Starts an interaction from a crop (`startFrom`).
    pub(crate) fn start_from(&mut self, crop: Option<Crop>) {
        self.start = Some(crop);
        self.changed = None;
    }

    /// Ends an interaction: the crop commits if it changed (`commit`).
    pub(crate) fn commit(
        &mut self,
        reason: Reason,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.wheel_timer = None;
        let from = self.start.take();
        let now = self.changed.take().or(*self.crop.get());
        let (Some(from), Some(now)) = (from, now) else {
            return;
        };
        if from.is_some_and(|from| same_crop(&from, &now)) {
            return;
        }
        self.crop
            .commit_value(&self.instance, &Some(now), reason, event, window, cx);
    }

    /// A change made in one go, like a key or a press, which commits at once
    /// (`step`).
    pub(crate) fn step(
        &mut self,
        next: impl FnOnce(Crop, Size) -> Crop,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_locked() {
            return false;
        }
        let (Some(crop), Some(natural)) = (*self.crop.get(), self.natural) else {
            return false;
        };
        self.start_from(Some(crop));
        let reason = options.reason.unwrap_or(Reason::Imperative);
        let event = options.event.clone();
        let options = ChangeOptions {
            reason: Some(reason),
            ..options
        };
        let done = self.change_crop(next(crop, natural), options, window, cx);
        self.commit(reason, event, window, cx);
        done
    }

    /// A pointer's place on the image, in image pixels (`imagePoint`).
    pub(crate) fn image_point(&self, position: gpui::Point<Pixels>) -> Option<Point> {
        let at = self.layout()?;
        let origin = self.view.padding_box()?.origin;
        Some(Point {
            x: (length(position.x) - length(origin.x) - at.image.left) / at.scale,
            y: (length(position.y) - length(origin.y) - at.image.top) / at.scale,
        })
    }

    /// The crop, or `None` until the image loads.
    pub fn get_crop(&self) -> Option<ImageCrop> {
        *self.crop.get()
    }

    /// Sets the crop, as a user would. Returns whether it changed.
    pub fn set_crop(
        &mut self,
        crop: ImageCrop,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.step(move |_, _| round(crop), options, window, cx)
    }

    /// Turns the image a quarter clockwise, or counterclockwise for a
    /// negative `direction`. Returns whether the crop changed.
    pub fn rotate(
        &mut self,
        direction: i32,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let limits = self.limits();
        self.step(
            move |crop, natural| rotate_crop(crop, natural, direction, &limits),
            options,
            window,
            cx,
        )
    }

    /// Mirrors the image left to right, or top to bottom. Returns whether
    /// the crop changed.
    pub fn flip(
        &mut self,
        direction: ImageCropperFlipDirection,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.step(
            move |crop, natural| flip_crop(crop, natural, direction),
            options,
            window,
            cx,
        )
    }

    /// How far the crop magnifies: 1 when it's as large as the image allows
    /// at its ratio.
    pub fn get_zoom(&self) -> f64 {
        self.zoom_text()
    }

    /// Zooms to a level, about the crop's middle, within 1 and `max_zoom`.
    /// Returns whether the crop changed.
    pub fn set_zoom(
        &mut self,
        zoom: f64,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !zoom.is_finite() {
            return false;
        }
        let limits = self.limits();
        self.step(
            move |crop, natural| zoom_crop(crop, natural, zoom, &limits, None),
            options,
            window,
            cx,
        )
    }

    /// Moves focus to the selection, once it's in the page.
    pub fn focus(&self, window: &mut Window, cx: &mut gpui::App) {
        if self.is_ready() && self.selection_focus.is_rendered(window) {
            window.focus(&self.selection_focus, cx);
        }
    }

    /// What the image found as it drew: loaded, with its natural size, or
    /// failed. A report for an image the cropper no longer shows is stale.
    pub(crate) fn image_status(
        &mut self,
        key: SourceKey,
        status: Status,
        natural: Option<Size>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.src.as_ref().map(SourceKey::of) != Some(key) {
            return;
        }
        match (status, natural) {
            (Status::Loaded, Some(natural)) => self.loaded(natural, window, cx),
            (Status::Error, _) | (Status::Loaded, None) => self.failed(cx),
            (Status::Loading, _) => {}
        }
    }

    /// The image's natural size, once it loads. A new image starts its own
    /// crop (`loaded`).
    pub(crate) fn loaded(&mut self, natural: Size, window: &mut Window, cx: &mut Context<Self>) {
        if natural.width == 0.0 || natural.height == 0.0 {
            return self.failed(cx);
        }
        let source = self.settings.src.as_ref().map(SourceKey::of);
        self.natural = Some(natural);
        self.status = Status::Loaded;
        cx.notify();
        let kept = self.crop.get().is_some() && self.crop_for == source;
        if self.crop.is_controlled() || kept {
            return;
        }
        self.crop_for = source;
        self.start_from(None);
        let limits = self.limits();
        let crop = initial_crop(natural, &limits, self.settings.mode);
        self.change_crop(crop, ChangeOptions::reason(Reason::ImageLoad), window, cx);
        self.commit(Reason::ImageLoad, None, window, cx);
    }

    /// The image failed to load (`failed`).
    pub(crate) fn failed(&mut self, cx: &mut Context<Self>) {
        self.status = Status::Error;
        self.natural = None;
        cx.notify();
    }

    /// The viewport's content box, as it laid out (`measure`).
    pub(crate) fn measured(&mut self, content: Rect, cx: &mut Context<Self>) {
        if self.content == Some(content) {
            return;
        }
        self.content = Some(content);
        cx.notify();
    }

    // Whether a handle heard this press first: a handle still written
    // inside the selection sends its events to it too (`isHandle`).
    fn from_handle(&self, event: &PointerEvent) -> bool {
        self.handle_press
            .as_ref()
            .is_some_and(|press| press.is_same(event))
    }

    /// Marks a press a handle heard, which the selection around it leaves
    /// alone, until the press's dispatch is over.
    pub(crate) fn mark_handle_press(
        &mut self,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_press = Some(event.clone());
        let entity = cx.weak_entity();
        window.defer(cx, move |_window, cx| {
            entity.update(cx, |cropper, _cx| cropper.handle_press = None).ok();
        });
    }

    /// Marks a key a handle heard, the same way.
    pub(crate) fn mark_handle_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.handle_key = true;
        let entity = cx.weak_entity();
        window.defer(cx, move |_window, cx| {
            entity.update(cx, |cropper, _cx| cropper.handle_key = false).ok();
        });
    }

    /// A drag of a handle, or of the whole selection with `Move`. Shift on a
    /// corner keeps the ratio (`selectionPointerdown`).
    pub(crate) fn selection_pointer_down(
        &mut self,
        event: &PointerEvent,
        handle: ResizeHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A frame's press pans the image under it, which keeps focus where
        // it is, as the viewport's `preventDefault` does.
        if self.settings.mode == ImageCropperMode::Frame {
            if event.kind == PointerKind::Mouse && !self.is_locked() {
                window.prevent_default();
            }
            return;
        }
        let drags = self.drags.clone();
        if !drags.accepts(self, event, window, cx) || self.is_locked() {
            return;
        }
        if handle == ResizeHandle::Move && self.from_handle(event) {
            return;
        }
        let Some(at) = self.layout() else {
            return;
        };
        let (Some(from), Some(natural)) = (*self.crop.get(), self.natural) else {
            return;
        };
        let start = event.position;
        drags.start(
            event,
            BindDragOptions::new()
                .threshold(DRAG_THRESHOLD)
                .cancel_on_escape(true)
                .on_start(move |cropper: &mut Self, _event, _window, cx| {
                    cropper.start_from(Some(from));
                    cropper.dragging = Some(Dragging::Handle(handle));
                    cx.notify();
                    true
                })
                .on_move(move |cropper: &mut Self, sample, window, cx| {
                    let dx = length(sample.x - start.x) / at.scale;
                    let dy = length(sample.y - start.y) / at.scale;
                    let limits = cropper.limits();
                    let keep = sample.event.modifiers.shift;
                    let next = resize_crop(from, natural, handle, dx, dy, &limits, keep);
                    let options = ChangeOptions::event_of(Reason::Drag, sample.event.event.clone());
                    cropper.change_crop(next, options, window, cx);
                })
                .on_end(move |cropper: &mut Self, end, canceled, window, cx| {
                    cropper.dragging = None;
                    cx.notify();
                    if canceled && !cropper.is_locked() {
                        let options = ChangeOptions::event_of(Reason::Drag, end.event.clone());
                        cropper.change_crop(from, options, window, cx);
                    }
                    cropper.commit(Reason::Drag, Some(end.event.clone()), window, cx);
                }),
            window,
            cx,
        );
    }

    /// Pans the image under a frame with a pointer (`viewportPointerdown`).
    /// gpui follows one pointer at a time, so a pinch comes as a trackpad's
    /// [`pinch`](Self::pinch).
    pub(crate) fn viewport_pointer_down(
        &mut self,
        event: &PointerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_locked() || self.settings.mode != ImageCropperMode::Frame {
            return;
        }
        if event.kind == PointerKind::Mouse && event.button != MouseButton::Left {
            return;
        }
        let drags = self.drags.clone();
        if !drags.accepts(self, event, window, cx) {
            return;
        }
        window.prevent_default();
        self.gesture = self.begin(event);
        let crop = *self.crop.get();
        self.start_from(crop);
        self.dragging = Some(Dragging::Pan);
        cx.notify();
        drags.start(
            event,
            BindDragOptions::new()
                .cancel_on_escape(true)
                .on_move(|cropper: &mut Self, sample, window, cx| cropper.pan(sample, window, cx))
                .on_end(move |cropper: &mut Self, end, canceled, window, cx| {
                    cropper.gesture = None;
                    cropper.dragging = None;
                    cx.notify();
                    // Escape, or the system taking the pointer, puts the crop
                    // back. Like any drag, Escape reports the last pointer
                    // event, not the key.
                    if canceled
                        && !cropper.is_locked()
                        && let Some(crop) = crop
                    {
                        let options = ChangeOptions::event_of(Reason::Drag, end.event.clone());
                        cropper.change_crop(crop, options, window, cx);
                    }
                    cropper.commit(Reason::Drag, Some(end.event.clone()), window, cx);
                }),
            window,
            cx,
        );
    }

    // The pan's start: the crop, the press and the layout (`begin`).
    fn begin(&self, event: &PointerEvent) -> Option<Gesture> {
        let at = self.layout()?;
        let crop = (*self.crop.get())?;
        Some(Gesture {
            crop,
            center: Point {
                x: length(event.position.x),
                y: length(event.position.y),
            },
            scale: at.scale,
        })
    }

    // The pointer moved during a pan: the image follows it under the frame.
    fn pan(&mut self, sample: &PointerSample, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(gesture), Some(natural)) = (self.gesture, self.natural) else {
            return;
        };
        let dx = (gesture.center.x - length(sample.x)) / gesture.scale;
        let dy = (gesture.center.y - length(sample.y)) / gesture.scale;
        let limits = self.limits();
        let next = resize_crop(gesture.crop, natural, ResizeHandle::Move, dx, dy, &limits, false);
        let options = ChangeOptions::event_of(Reason::Drag, sample.event.event.clone());
        self.change_crop(next, options, window, cx);
    }

    /// The wheel zooms a frame at the pointer. A wheel has no release, so a
    /// pause commits (`wheel`).
    pub(crate) fn wheel(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        // A finger's pan sends no wheel in a browser: in a frame it pans.
        if self.is_locked()
            || self.settings.mode != ImageCropperMode::Frame
            || event.source == ScrollSource::Touch
        {
            return;
        }
        cx.stop_propagation();
        // A line is 16 pixels. gpui's deltas point the way the content moves,
        // the browser's the way the view does.
        let delta_y = match event.delta {
            ScrollDelta::Lines(lines) => -f64::from(lines.y) * 16.0,
            ScrollDelta::Pixels(pixels) => -length(pixels.y),
        };
        let delta = clamp(delta_y, -50.0, 50.0);
        let at = self.image_point(event.position);
        let (Some(crop), Some(natural)) = (*self.crop.get(), self.natural) else {
            return;
        };
        if self.start.is_none() {
            self.start_from(Some(crop));
        }
        let zoom = zoom_of(&crop, natural) * (-delta * 0.01).exp();
        let limits = self.limits();
        let next = zoom_crop(crop, natural, zoom, &limits, at);
        self.change_crop(next, ChangeOptions::event(Reason::Wheel, event), window, cx);
        self.commit_after_pause(Reason::Wheel, InputEvent::from(event), window, cx);
    }

    /// A trackpad's pinch zooms a frame at its center, where the original
    /// pinches with two fingers on the viewport. It commits as the pinch
    /// ends, or after a pause.
    pub(crate) fn pinch(&mut self, event: &PinchEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_locked() || self.settings.mode != ImageCropperMode::Frame {
            return;
        }
        cx.stop_propagation();
        let at = self.image_point(event.position);
        let (Some(crop), Some(natural)) = (*self.crop.get(), self.natural) else {
            return;
        };
        if self.start.is_none() {
            self.start_from(Some(crop));
        }
        let zoom = zoom_of(&crop, natural) * (1.0 + f64::from(event.delta));
        let limits = self.limits();
        let next = zoom_crop(crop, natural, zoom, &limits, at);
        self.change_crop(next, ChangeOptions::event(Reason::Pinch, event), window, cx);
        if event.phase == TouchPhase::Ended {
            self.commit(Reason::Pinch, Some(InputEvent::from(event)), window, cx);
        } else {
            self.commit_after_pause(Reason::Pinch, InputEvent::from(event), window, cx);
        }
    }

    // Commits once nothing has moved the crop for a moment.
    fn commit_after_pause(
        &mut self,
        reason: Reason,
        event: InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.wheel_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            this.update_in(cx, |cropper, window, cx| {
                cropper.commit(reason, Some(event), window, cx)
            })
            .ok();
        }));
    }

    /// Arrows move the crop, or a handle's edges; Shift moves ten times as
    /// far. Plus and minus zoom, and R turns the image, Shift with R the
    /// other way (`keydown`).
    pub(crate) fn keydown(
        &mut self,
        event: &KeyDownEvent,
        handle: ResizeHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.alt || modifiers.control || modifiers.platform || self.is_locked() {
            return;
        }
        if handle == ResizeHandle::Move && self.handle_key {
            return;
        }
        let key = Key::of(&event.keystroke);
        let direction = match key {
            Key::ArrowUp => Some((0.0, -1.0)),
            Key::ArrowDown => Some((0.0, 1.0)),
            Key::ArrowLeft => Some((-1.0, 0.0)),
            Key::ArrowRight => Some((1.0, 0.0)),
            _ => None,
        };
        if let Some((x, y)) = direction {
            let vertical = matches!(handle, ResizeHandle::N | ResizeHandle::S);
            let horizontal = matches!(handle, ResizeHandle::E | ResizeHandle::W);
            if (x != 0.0 && vertical) || (y != 0.0 && horizontal) {
                return;
            }
            window.prevent_default();
            // One step is a pixel on screen, however large the image is.
            let scale = self.layout().map_or(1.0, |at| at.scale);
            let unit = js_round(1.0 / scale).max(1.0);
            let by = unit * if modifiers.shift { 10.0 } else { 1.0 };
            let limits = self.limits();
            self.step(
                move |crop, natural| resize_crop(crop, natural, handle, x * by, y * by, &limits, false),
                ChangeOptions::event(Reason::StepKey, event),
                window,
                cx,
            );
            return;
        }
        match key.name() {
            "+" | "=" | "-" | "_" => {
                window.prevent_default();
                let factor = if matches!(key.name(), "+" | "=") {
                    1.25
                } else {
                    0.8
                };
                let limits = self.limits();
                self.step(
                    move |crop, natural| {
                        zoom_crop(crop, natural, zoom_of(&crop, natural) * factor, &limits, None)
                    },
                    ChangeOptions::event(Reason::ZoomKey, event),
                    window,
                    cx,
                );
            }
            "r" | "R" => {
                window.prevent_default();
                let direction = if modifiers.shift { -1 } else { 1 };
                self.rotate(
                    direction,
                    ChangeOptions::event(Reason::RotateKey, event),
                    window,
                    cx,
                );
            }
            _ => {}
        }
    }

    /// The zoom input moved to a value, the native range input's `input`
    /// (`zoomInput`).
    pub(crate) fn zoom_input(
        &mut self,
        value: f64,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_locked() {
            return;
        }
        let (Some(crop), Some(natural)) = (*self.crop.get(), self.natural) else {
            return;
        };
        if self.start.is_none() {
            self.start_from(Some(crop));
        }
        if value.is_finite() {
            let limits = self.limits();
            let next = zoom_crop(crop, natural, value, &limits, None);
            let options = ChangeOptions {
                reason: Some(Reason::ZoomInput),
                event,
                source: None,
            };
            self.change_crop(next, options, window, cx);
        }
        cx.notify();
    }

    /// The zoom input's change ended, its `change` (`zoomCommit`).
    pub(crate) fn zoom_commit(
        &mut self,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit(Reason::ZoomInput, event, window, cx);
    }

    /// Disabling it, or a new image, mid-drag ends the drag where it is,
    /// like a slider's.
    pub(crate) fn end_if_locked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_locked() || !self.drags.is_running() {
            return;
        }
        let drags = self.drags.clone();
        drags.cancel(self, window, cx);
    }
}
